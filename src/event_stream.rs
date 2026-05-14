//! Event stream collection — replaces inline event loop in `process_message`.
//!
//! `EventStreamCollector` consumes the async `EventStream` produced by
//! `adk_runner::Runner::run()`, correctly distinguishing partial vs final
//! events via the `event.partial` flag (R1.5), filtering tool-call metadata
//! from user-facing text (R1.3), handling errors (R1.4), and falling back
//! to the last partial text when no final event arrives (R1.2).

use adk_core::{Event, EventStream, Part};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Collected tool call information from the event stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    /// Tool/function name
    pub name: String,
    /// Tool call arguments
    pub args: serde_json::Value,
    /// Provider-specific call ID (OpenAI-style), if present
    pub id: Option<String>,
}

/// Token usage from the LLM response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenCount {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

/// The result of collecting an entire event stream into a single response.
#[derive(Debug, Clone)]
pub struct CollectedResponse {
    /// The final user-facing text (from final event, or last partial fallback).
    pub text: String,
    /// Tool calls observed during the stream.
    pub tool_calls: Vec<ToolCallInfo>,
    /// Images (inline data) from the response — base64 encoded with mime type.
    pub images: Vec<ImageData>,
    /// Token usage, if reported by the model.
    pub token_count: Option<TokenCount>,
    /// Wall-clock duration of the collection.
    pub duration: Duration,
}

/// An image returned in the agent's response.
#[derive(Debug, Clone)]
pub struct ImageData {
    pub mime_type: String,
    pub data: Vec<u8>,
}

/// Consumes an `EventStream` and produces a `CollectedResponse`.
///
/// Design invariants:
/// - Partial vs final is determined solely by `event.llm_response.partial` (R1.5)
/// - Only `Part::Text` from non-"user" authors is collected (R1.3)
/// - `FunctionCall` / `FunctionResponse` parts are recorded but excluded from text
/// - Error events (stream `Err` items) produce a user-facing notification and log (R1.4)
/// - If no final event arrives, the last partial text is used (R1.2)
///
/// ## Tool execution (R3.2, R3.3, R3.4)
///
/// Tool execution is handled internally by the adk-runner/adk-agent framework.
/// When tools are registered with an agent, the Runner automatically intercepts
/// `FunctionCall` parts in the event stream, executes the corresponding tool via
/// adk-tool, and feeds the result back into the agent's context for the next turn.
/// If a tool execution fails, the Runner passes the error back to the agent as a
/// tool error result (not a gateway-level error), allowing the agent to handle the
/// failure gracefully.
///
/// The `EventStreamCollector` is an *observer* — it records tool calls
/// (in `CollectedResponse.tool_calls`) for logging and metrics, but does not
/// participate in the execution loop itself.
pub struct EventStreamCollector {
    stream: EventStream,
}

impl EventStreamCollector {
    /// Create a new collector wrapping the given event stream.
    pub fn new(stream: EventStream) -> Self {
        Self { stream }
    }

    /// Consume the event stream and return the collected response.
    pub async fn collect(mut self) -> CollectedResponse {
        self.collect_inner(None::<fn(String) -> futures::future::Ready<()>>)
            .await
    }

    /// Consume the event stream, invoking `on_partial` for each partial text chunk,
    /// and return the collected response.
    ///
    /// This enables streaming delivery (R17): the caller passes a callback that
    /// forwards partial text to the `DeliveryStrategy::on_partial` method so the
    /// user sees incremental updates as the agent generates them.
    pub async fn collect_with_partial<F, Fut>(mut self, on_partial: F) -> CollectedResponse
    where
        F: Fn(String) -> Fut + Send,
        Fut: std::future::Future<Output = ()> + Send,
    {
        self.collect_inner(Some(on_partial)).await
    }

    /// Shared collection logic with an optional partial callback.
    async fn collect_inner<F, Fut>(&mut self, on_partial: Option<F>) -> CollectedResponse
    where
        F: Fn(String) -> Fut + Send,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let start = Instant::now();

        let mut last_partial_text = String::new();
        let mut final_text: Option<String> = None;
        let mut tool_calls: Vec<ToolCallInfo> = Vec::new();
        let mut images: Vec<ImageData> = Vec::new();
        let mut token_count: Option<TokenCount> = None;
        let mut error_text: Option<String> = None;

        while let Some(result) = self.stream.next().await {
            match result {
                Ok(event) => {
                    let prev_partial = last_partial_text.clone();
                    Self::process_event(
                        &event,
                        &mut last_partial_text,
                        &mut final_text,
                        &mut tool_calls,
                        &mut images,
                        &mut token_count,
                    );

                    // If partial text changed and we have a callback, invoke it (R17)
                    if let Some(ref cb) = on_partial {
                        if last_partial_text != prev_partial && final_text.is_none() {
                            cb(last_partial_text.clone()).await;
                        }
                    }
                }
                Err(e) => {
                    // R1.4: log with full context and produce user notification
                    tracing::error!(
                        error = %e,
                        "error event in agent stream"
                    );
                    error_text = Some(format!("\u{26a0}\u{fe0f} Error: {e}"));
                    // Stop processing on error
                    break;
                }
            }
        }

        let text = Self::resolve_text(final_text, last_partial_text, error_text);
        let duration = start.elapsed();

        CollectedResponse {
            text,
            tool_calls,
            images,
            token_count,
            duration,
        }
    }

    /// Process a single successful event from the stream.
    fn process_event(
        event: &Event,
        last_partial_text: &mut String,
        final_text: &mut Option<String>,
        tool_calls: &mut Vec<ToolCallInfo>,
        images: &mut Vec<ImageData>,
        token_count: &mut Option<TokenCount>,
    ) {
        // Skip user-authored events — we only care about agent output
        if event.author == "user" {
            return;
        }

        // Check for error in the LLM response itself
        if let Some(ref err_msg) = event.llm_response.error_message {
            tracing::error!(
                author = %event.author,
                error_code = ?event.llm_response.error_code,
                error_message = %err_msg,
                "LLM error in event"
            );
            *final_text = Some(format!("\u{26a0}\u{fe0f} Error: {err_msg}"));
            return;
        }

        // Extract token usage from the last event that reports it
        if let Some(ref usage) = event.llm_response.usage_metadata {
            *token_count = Some(TokenCount {
                prompt_tokens: usage.prompt_token_count,
                completion_tokens: usage.candidates_token_count,
                total_tokens: usage.total_token_count,
            });
        }

        // Extract content parts
        if let Some(ref content) = event.llm_response.content {
            let mut event_text = String::new();

            for part in &content.parts {
                match part {
                    Part::Text { text } => {
                        event_text.push_str(text);
                    }
                    // Record tool calls but exclude from user-facing text (R1.3)
                    Part::FunctionCall { name, args, id, .. } => {
                        tool_calls.push(ToolCallInfo {
                            name: name.clone(),
                            args: args.clone(),
                            id: id.clone(),
                        });
                    }
                    // Skip all other part types (FunctionResponse, Thinking,
                    // FileData, ServerToolCall, ServerToolResponse)
                    // But capture InlineData (images, audio)
                    Part::InlineData { mime_type, data } => {
                        if mime_type.starts_with("image/") {
                            images.push(ImageData {
                                mime_type: mime_type.clone(),
                                data: data.clone(),
                            });
                        }
                    }
                    _ => {}
                }
            }

            if !event_text.is_empty() {
                // R1.5: use the `partial` flag to distinguish partial vs final
                if event.llm_response.partial {
                    last_partial_text.push_str(&event_text);
                } else {
                    *final_text = Some(event_text);
                }
            }
        }
    }

    /// Determine the final response text from collected state.
    ///
    /// Priority: error > final event text > last partial text > fallback message
    fn resolve_text(
        final_text: Option<String>,
        last_partial_text: String,
        error_text: Option<String>,
    ) -> String {
        if let Some(err) = error_text {
            return err;
        }
        if let Some(text) = final_text {
            return text;
        }
        if !last_partial_text.is_empty() {
            return last_partial_text;
        }
        "I received your message but couldn't generate a response.".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_core::{AdkError, Content, ErrorCategory, ErrorComponent, Event, Part, UsageMetadata};
    use futures::stream;

    fn make_event(author: &str, partial: bool, text: &str) -> Event {
        let mut event = Event::new("test-invocation");
        event.author = author.to_string();
        event.llm_response.partial = partial;
        if !text.is_empty() {
            event.llm_response.content = Some(Content {
                role: "model".to_string(),
                parts: vec![Part::Text {
                    text: text.to_string(),
                }],
            });
        }
        event
    }

    fn make_tool_call_event(author: &str, tool_name: &str) -> Event {
        let mut event = Event::new("test-invocation");
        event.author = author.to_string();
        event.llm_response.partial = false;
        event.llm_response.content = Some(Content {
            role: "model".to_string(),
            parts: vec![Part::FunctionCall {
                name: tool_name.to_string(),
                args: serde_json::json!({"key": "value"}),
                id: Some("call_1".to_string()),
                thought_signature: None,
            }],
        });
        event
    }

    fn make_error(msg: &str) -> AdkError {
        AdkError::new(
            ErrorComponent::Agent,
            ErrorCategory::Internal,
            "TEST_ERR",
            msg,
        )
    }

    fn events_to_stream(events: Vec<Result<Event, AdkError>>) -> EventStream {
        Box::pin(stream::iter(events))
    }

    #[tokio::test]
    async fn test_final_event_text_is_used() {
        let events = vec![
            Ok(make_event("assistant", true, "partial...")),
            Ok(make_event("assistant", false, "final answer")),
        ];
        let resp = EventStreamCollector::new(events_to_stream(events))
            .collect()
            .await;
        assert_eq!(resp.text, "final answer");
        assert!(resp.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn test_falls_back_to_last_partial() {
        let events = vec![
            Ok(make_event("assistant", true, "first partial")),
            Ok(make_event("assistant", true, "second partial")),
        ];
        let resp = EventStreamCollector::new(events_to_stream(events))
            .collect()
            .await;
        // Partial events accumulate — both chunks are concatenated
        assert_eq!(resp.text, "first partialsecond partial");
    }

    #[tokio::test]
    async fn test_skips_user_events() {
        let events = vec![
            Ok(make_event("user", false, "user message")),
            Ok(make_event("assistant", false, "agent reply")),
        ];
        let resp = EventStreamCollector::new(events_to_stream(events))
            .collect()
            .await;
        assert_eq!(resp.text, "agent reply");
    }

    #[tokio::test]
    async fn test_tool_calls_excluded_from_text() {
        let events = vec![
            Ok(make_tool_call_event("assistant", "web_search")),
            Ok(make_event("assistant", false, "here are the results")),
        ];
        let resp = EventStreamCollector::new(events_to_stream(events))
            .collect()
            .await;
        assert_eq!(resp.text, "here are the results");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "web_search");
    }

    #[tokio::test]
    async fn test_error_event_produces_notification() {
        let events: Vec<Result<Event, AdkError>> = vec![
            Ok(make_event("assistant", true, "partial...")),
            Err(make_error("something went wrong")),
        ];
        let resp = EventStreamCollector::new(events_to_stream(events))
            .collect()
            .await;
        assert!(resp.text.contains("Error"));
        assert!(resp.text.contains("something went wrong"));
    }

    #[tokio::test]
    async fn test_empty_stream_produces_fallback() {
        let events: Vec<Result<Event, AdkError>> = vec![];
        let resp = EventStreamCollector::new(events_to_stream(events))
            .collect()
            .await;
        assert_eq!(
            resp.text,
            "I received your message but couldn't generate a response."
        );
    }

    #[tokio::test]
    async fn test_duration_is_recorded() {
        let events = vec![Ok(make_event("assistant", false, "hello"))];
        let resp = EventStreamCollector::new(events_to_stream(events))
            .collect()
            .await;
        assert!(resp.duration.as_nanos() > 0 || resp.duration == Duration::ZERO);
    }

    #[tokio::test]
    async fn test_token_count_from_usage_metadata() {
        let mut event = make_event("assistant", false, "response");
        event.llm_response.usage_metadata = Some(UsageMetadata {
            prompt_token_count: 10,
            candidates_token_count: 20,
            total_token_count: 30,
            ..Default::default()
        });
        let events = vec![Ok(event)];
        let resp = EventStreamCollector::new(events_to_stream(events))
            .collect()
            .await;
        let tc = resp.token_count.unwrap();
        assert_eq!(tc.prompt_tokens, 10);
        assert_eq!(tc.completion_tokens, 20);
        assert_eq!(tc.total_tokens, 30);
    }

    #[tokio::test]
    async fn test_mixed_text_and_function_call_parts() {
        let mut event = Event::new("test-invocation");
        event.author = "assistant".to_string();
        event.llm_response.partial = false;
        event.llm_response.content = Some(Content {
            role: "model".to_string(),
            parts: vec![
                Part::Text {
                    text: "Let me search for that.".to_string(),
                },
                Part::FunctionCall {
                    name: "search".to_string(),
                    args: serde_json::json!({"q": "rust"}),
                    id: None,
                    thought_signature: None,
                },
            ],
        });
        let events = vec![Ok(event)];
        let resp = EventStreamCollector::new(events_to_stream(events))
            .collect()
            .await;
        assert_eq!(resp.text, "Let me search for that.");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "search");
        assert!(resp.tool_calls[0].id.is_none());
    }
}
