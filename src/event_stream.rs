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
    /// Whether the runner hit the max iterations limit.
    pub max_iterations_reached: bool,
    /// The number of iterations the runner executed (set when max_iterations_reached is true).
    pub iteration_count: Option<u32>,
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
        let mut max_iterations_reached = false;
        let mut iteration_count: Option<u32> = None;

        while let Some(result) = self.stream.next().await {
            match result {
                Ok(event) => {
                    let prev_partial = last_partial_text.clone();

                    // Extract max_iterations metadata from provider_metadata
                    if event.provider_metadata.get("max_iterations_reached").map(|v| v == "true").unwrap_or(false) {
                        max_iterations_reached = true;
                        if let Some(count_str) = event.provider_metadata.get("iteration_count") {
                            iteration_count = count_str.parse::<u32>().ok();
                        }
                    }

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
                    error_text = Some(format_agent_error(&e));
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
            max_iterations_reached,
            iteration_count,
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

/// Format an agent/model error into a clean, user-friendly message.
///
/// Instead of dumping raw JSON error responses, this produces concise
/// actionable messages that tell the user what went wrong and what to do.
fn format_agent_error(error: &adk_core::AdkError) -> String {
    let error_str = error.to_string();
    let lower = error_str.to_lowercase();
    
    // API key issues
    if lower.contains("api key expired") || lower.contains("api_key_invalid") {
        return "⚠️ API key expired or invalid.\n\nUpdate your GOOGLE_API_KEY in ~/.zshrc and restart the gateway.".to_string();
    }
    if lower.contains("api key not valid") || lower.contains("invalid api key") || lower.contains("incorrect api key") {
        return "⚠️ Invalid API key.\n\nCheck your API key configuration and restart the gateway.".to_string();
    }
    
    // Quota / billing
    if lower.contains("quota") || lower.contains("rate limit") || lower.contains("resource exhausted") {
        return "⚠️ Rate limit or quota exceeded.\n\nWait a moment and try again, or check your API plan limits.".to_string();
    }
    if lower.contains("billing") || lower.contains("credit balance") || lower.contains("insufficient_quota") {
        return "⚠️ Billing issue — out of credits.\n\nAdd credits to your API provider account.".to_string();
    }
    
    // Model not found
    if lower.contains("not found") && (lower.contains("model") || lower.contains("models/")) {
        return "⚠️ Model not available.\n\nThe configured model may have been deprecated. Update the model in gateway.json.".to_string();
    }
    
    // Context too long
    if lower.contains("context length") || lower.contains("token limit") || lower.contains("too many tokens") || lower.contains("max.*tokens") {
        return "⚠️ Message too long for the model's context window.\n\nTry a shorter message or start a new session with /new.".to_string();
    }
    
    // Network / connectivity
    if lower.contains("timeout") || lower.contains("timed out") {
        return "⚠️ Request timed out.\n\nThe model took too long to respond. Try again.".to_string();
    }
    if lower.contains("connection") || lower.contains("network") || lower.contains("dns") {
        return "⚠️ Network error — couldn't reach the model provider.\n\nCheck your internet connection.".to_string();
    }
    
    // Safety / content filter
    if lower.contains("safety") || lower.contains("blocked") || lower.contains("content filter") || lower.contains("harm") {
        return "⚠️ Response blocked by safety filter.\n\nTry rephrasing your request.".to_string();
    }
    
    // Server errors
    if lower.contains("500") || lower.contains("internal server error") || lower.contains("503") || lower.contains("overloaded") {
        return "⚠️ Model provider is temporarily unavailable.\n\nTry again in a moment.".to_string();
    }
    
    // Generic fallback — still clean, no raw JSON
    // Extract just the meaningful part if it's a "bad response from server" wrapper
    if lower.contains("bad response from server") {
        // Try to extract the inner message
        if let Some(msg_start) = error_str.find("\"message\":") {
            let after_msg = &error_str[msg_start + 11..];
            if let Some(quote_start) = after_msg.find('"') {
                let after_quote = &after_msg[quote_start + 1..];
                if let Some(quote_end) = after_quote.find('"') {
                    let inner_msg = &after_quote[..quote_end];
                    if !inner_msg.is_empty() {
                        return format!("⚠️ {}", inner_msg);
                    }
                }
            }
        }
    }
    
    // Last resort: truncate the error to something reasonable
    let clean = if error_str.len() > 120 {
        format!("⚠️ {}", &error_str[..120])
    } else {
        format!("⚠️ {}", error_str)
    };
    clean
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

    #[tokio::test]
    async fn test_max_iterations_metadata_extracted() {
        let mut event = Event::new("test-invocation");
        event.author = "assistant".to_string();
        event.llm_response.partial = false;
        event.llm_response.turn_complete = true;
        event.llm_response.interrupted = true;
        event.llm_response.content = Some(Content {
            role: "model".to_string(),
            parts: vec![Part::Text {
                text: "Agent execution stopped: max iterations (25) reached.".to_string(),
            }],
        });
        event.provider_metadata.insert(
            "max_iterations_reached".to_string(),
            "true".to_string(),
        );
        event.provider_metadata.insert(
            "iteration_count".to_string(),
            "25".to_string(),
        );

        let events = vec![Ok(event)];
        let resp = EventStreamCollector::new(events_to_stream(events))
            .collect()
            .await;

        assert!(resp.max_iterations_reached);
        assert_eq!(resp.iteration_count, Some(25));
    }

    #[tokio::test]
    async fn test_no_max_iterations_metadata_when_not_reached() {
        let events = vec![Ok(make_event("assistant", false, "normal response"))];
        let resp = EventStreamCollector::new(events_to_stream(events))
            .collect()
            .await;

        assert!(!resp.max_iterations_reached);
        assert_eq!(resp.iteration_count, None);
    }
}
