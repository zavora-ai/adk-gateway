//! Property-based tests for EventStreamCollector.
//!
//! Feature: gateway-production-maturity, Property 1: Event stream collection produces correct text
//! **Validates: Requirements R1.1, R1.2, R1.5**

use adk_core::{AdkError, Content, Event, Part};
use adk_gateway::event_stream::EventStreamCollector;
use futures::stream;
use proptest::prelude::*;
use std::pin::Pin;

type EventStream = Pin<Box<dyn futures::Stream<Item = Result<Event, AdkError>> + Send>>;

// ── Helper functions ───────────────────────────────────────────────

/// Build an Event with the given author, partial flag, and text content.
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

/// Convert a Vec of results into a pinned EventStream.
fn events_to_stream(events: Vec<Result<Event, AdkError>>) -> EventStream {
    Box::pin(stream::iter(events))
}

// ── Strategies ─────────────────────────────────────────────────────

/// Arbitrary non-empty text content for events.
fn arb_event_text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 _.!?]{1,100}"
}

/// Represents a partial or final event with text content.
#[derive(Debug, Clone)]
struct TextEvent {
    text: String,
}

/// Strategy for a single partial event.
fn arb_text_event() -> impl Strategy<Value = TextEvent> {
    arb_event_text().prop_map(|text| TextEvent { text })
}

// ── Property tests ─────────────────────────────────────────────────

// Feature: gateway-production-maturity, Property 1: Event stream collection produces correct text
// **Validates: Requirements R1.1, R1.2, R1.5**
proptest! {
    /// R1.1: When a final event exists, its text is used as the response.
    ///
    /// Generate 0..5 partial events followed by exactly one final event.
    /// The collected response text must equal the final event's text.
    #[test]
    fn final_event_text_is_used_as_response(
        partials in prop::collection::vec(arb_text_event(), 0..5),
        final_ev in arb_text_event(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut events: Vec<Result<Event, AdkError>> = partials
                .iter()
                .map(|te| Ok(make_event("assistant", true, &te.text)))
                .collect();
            events.push(Ok(make_event("assistant", false, &final_ev.text)));

            let resp = EventStreamCollector::new(events_to_stream(events))
                .collect()
                .await;

            prop_assert_eq!(resp.text, final_ev.text);
            Ok(())
        })?;
    }

    /// R1.2: When only partial events exist (no final), the accumulated partial text is used.
    ///
    /// Generate 1..6 partial events with no final event.
    /// The collected response text must equal all partial texts concatenated.
    #[test]
    fn last_partial_text_used_when_no_final(
        partials in prop::collection::vec(arb_text_event(), 1..6),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let expected_text: String = partials.iter().map(|te| te.text.as_str()).collect();

            let events: Vec<Result<Event, AdkError>> = partials
                .iter()
                .map(|te| Ok(make_event("assistant", true, &te.text)))
                .collect();

            let resp = EventStreamCollector::new(events_to_stream(events))
                .collect()
                .await;

            prop_assert_eq!(resp.text, expected_text);
            Ok(())
        })?;
    }

    /// R1.5: The `partial` flag is what determines partial vs final, not content heuristics.
    ///
    /// Generate two events with identical text content but different partial flags.
    /// The one marked as final (partial=false) should be used as the response,
    /// proving that the flag — not the content — drives the distinction.
    #[test]
    fn partial_flag_determines_event_type_not_content(
        text in arb_event_text(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Same text for both partial and final — only the flag differs
            let events: Vec<Result<Event, AdkError>> = vec![
                Ok(make_event("assistant", true, &text)),
                Ok(make_event("assistant", false, &text)),
            ];

            let resp = EventStreamCollector::new(events_to_stream(events))
                .collect()
                .await;

            // The final event's text is used (R1.1), which happens to be the same text.
            // The key property: the collector used the final event (partial=false),
            // not the partial one, even though content is identical.
            prop_assert_eq!(resp.text, text);
            Ok(())
        })?;
    }

    /// R1.5 (stronger): When partial and final events have DIFFERENT text,
    /// the final event text wins regardless of ordering or content similarity.
    #[test]
    fn final_event_wins_over_partial_with_different_text(
        partial_text in arb_event_text(),
        final_text in arb_event_text(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let events: Vec<Result<Event, AdkError>> = vec![
                Ok(make_event("assistant", true, &partial_text)),
                Ok(make_event("assistant", false, &final_text)),
            ];

            let resp = EventStreamCollector::new(events_to_stream(events))
                .collect()
                .await;

            // Final event text always wins when present
            prop_assert_eq!(resp.text, final_text);
            Ok(())
        })?;
    }
}

// ── Strategies for tool-call exclusion property ────────────────────

/// Represents either a text event or a function-call event in a mixed stream.
#[derive(Debug, Clone)]
enum MixedEvent {
    Text {
        text: String,
    },
    FunctionCall {
        name: String,
        arg_key: String,
        id: String,
    },
}

/// Strategy for generating a mixed event (text or function call).
fn arb_mixed_event() -> impl Strategy<Value = MixedEvent> {
    prop_oneof![
        arb_event_text().prop_map(|text| MixedEvent::Text { text }),
        ("[a-z_]{1,20}", "[a-z_]{1,10}", "[a-z0-9]{1,10}")
            .prop_map(|(name, arg_key, id)| MixedEvent::FunctionCall { name, arg_key, id }),
    ]
}

/// Build an Event from a MixedEvent.
fn mixed_event_to_event(me: &MixedEvent, partial: bool) -> Event {
    let mut event = Event::new("test-invocation");
    event.author = "assistant".to_string();
    event.llm_response.partial = partial;
    match me {
        MixedEvent::Text { text } => {
            event.llm_response.content = Some(Content {
                role: "model".to_string(),
                parts: vec![Part::Text { text: text.clone() }],
            });
        }
        MixedEvent::FunctionCall { name, arg_key, id } => {
            event.llm_response.content = Some(Content {
                role: "model".to_string(),
                parts: vec![Part::FunctionCall {
                    name: name.clone(),
                    args: serde_json::json!({ arg_key.clone(): "value" }),
                    id: Some(id.clone()),
                    thought_signature: None,
                }],
            });
        }
    }
    event
}

// Feature: gateway-production-maturity, Property 2: Tool-call events are excluded from user-facing response
// **Validates: Requirements R1.3**
proptest! {
    /// R1.3: Tool-call events are excluded from user-facing response text,
    /// but ARE recorded in CollectedResponse.tool_calls.
    ///
    /// Generate an arbitrary sequence of mixed text and FunctionCall events,
    /// always ending with a final text event so there is deterministic output.
    /// Assert that:
    ///   1. The collected response text equals exactly the final text — no tool metadata injected
    ///   2. Every FunctionCall event is recorded in tool_calls with correct name, args, and id
    #[test]
    fn tool_call_events_excluded_from_response_text(
        mixed_events in prop::collection::vec(arb_mixed_event(), 1..10),
        final_text in arb_event_text(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Build the event stream: mixed events as partials, then a final text event
            let mut events: Vec<Result<Event, AdkError>> = mixed_events
                .iter()
                .map(|me| Ok(mixed_event_to_event(me, true)))
                .collect();
            // Always end with a final text event so we have a deterministic expected text
            events.push(Ok(make_event("assistant", false, &final_text)));

            // Collect expected tool calls from the input
            let expected_calls: Vec<(&String, &String, &String)> = mixed_events
                .iter()
                .filter_map(|me| match me {
                    MixedEvent::FunctionCall { name, arg_key, id } => Some((name, arg_key, id)),
                    _ => None,
                })
                .collect();

            let resp = EventStreamCollector::new(events_to_stream(events))
                .collect()
                .await;

            // Property 1: Response text equals exactly the final text event.
            // This proves no tool-call metadata was injected into the user-facing text.
            prop_assert_eq!(&resp.text, &final_text);

            // Property 2: All FunctionCall events are recorded in tool_calls
            // with correct name, args, and id.
            prop_assert_eq!(
                resp.tool_calls.len(),
                expected_calls.len(),
                "Expected {} tool calls, got {}",
                expected_calls.len(),
                resp.tool_calls.len()
            );
            for (tc, (expected_name, expected_arg_key, expected_id)) in
                resp.tool_calls.iter().zip(expected_calls.iter())
            {
                prop_assert_eq!(
                    &tc.name, *expected_name,
                    "Tool call name mismatch: expected '{}', got '{}'",
                    expected_name, tc.name
                );
                // Verify args contain the expected key
                prop_assert!(
                    tc.args.get(*expected_arg_key).is_some(),
                    "Tool call args should contain key '{}', got: {:?}",
                    expected_arg_key, tc.args
                );
                // Verify id is recorded
                prop_assert_eq!(
                    tc.id.as_deref(),
                    Some(expected_id.as_str()),
                    "Tool call id mismatch: expected '{}', got '{:?}'",
                    expected_id, tc.id
                );
            }

            Ok(())
        })?;
    }
}

// ── Strategies for error event handling property ───────────────────

/// Arbitrary non-empty error message content.
fn arb_error_msg() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 _.!?]{1,120}"
}

// Feature: gateway-production-maturity, Property 3: Error events produce user notification
// **Validates: Requirements R1.4**
proptest! {
    /// R1.4: When the event stream yields an error event, the collected response
    /// text contains an error notification for the user, and the error message
    /// content is included in that notification.
    ///
    /// Generate an arbitrary error message, build a stream with optional leading
    /// partial events followed by an Err(AdkError). Assert that:
    ///   1. The response text contains an error indicator (e.g. "Error")
    ///   2. The original error message is present in the response text
    #[test]
    fn error_events_produce_user_notification(
        partials in prop::collection::vec(arb_text_event(), 0..4),
        error_msg in arb_error_msg(),
    ) {
        use adk_core::{ErrorComponent, ErrorCategory};

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut events: Vec<Result<Event, AdkError>> = partials
                .iter()
                .map(|te| Ok(make_event("assistant", true, &te.text)))
                .collect();
            // Append an error event using AdkError
            events.push(Err(AdkError::new(
                ErrorComponent::Agent,
                ErrorCategory::Internal,
                "TEST_ERR",
                &error_msg,
            )));

            let resp = EventStreamCollector::new(events_to_stream(events))
                .collect()
                .await;

            // The response text must contain an error notification
            prop_assert!(
                resp.text.contains("Error"),
                "Response should contain error notification, got: '{}'",
                resp.text
            );
            // The original error message must be included in the notification
            prop_assert!(
                resp.text.contains(&error_msg),
                "Response should contain the error message '{}', got: '{}'",
                error_msg, resp.text
            );
            Ok(())
        })?;
    }
}

// ── Property 24: EventStreamCollector captures tool calls ──────────
// Feature: gateway-full-wiring, Property 24: EventStreamCollector captures tool calls
// **Validates: Requirements 19.10**

/// Represents a function call event with name and args.
#[derive(Debug, Clone)]
struct FnCallEvent {
    name: String,
    arg_key: String,
    arg_value: String,
    id: String,
}

fn arb_fn_call_event() -> impl Strategy<Value = FnCallEvent> {
    (
        "[a-z_]{1,15}",
        "[a-z_]{1,10}",
        "[a-z0-9]{1,10}",
        "[a-z0-9]{1,10}",
    )
        .prop_map(|(name, arg_key, arg_value, id)| FnCallEvent {
            name,
            arg_key,
            arg_value,
            id,
        })
}

/// Build an Event containing a FunctionCall part.
fn fn_call_to_event(fc: &FnCallEvent) -> Event {
    let mut event = Event::new("test-invocation");
    event.author = "assistant".to_string();
    event.llm_response.partial = true;
    event.llm_response.content = Some(Content {
        role: "model".to_string(),
        parts: vec![Part::FunctionCall {
            name: fc.name.clone(),
            args: serde_json::json!({ fc.arg_key.clone(): fc.arg_value.clone() }),
            id: Some(fc.id.clone()),
            thought_signature: None,
        }],
    });
    event
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 24: For any event stream containing N FunctionCall parts,
    /// CollectedResponse.tool_calls should have exactly N entries with
    /// correct name and args.
    #[test]
    fn event_stream_collector_captures_tool_calls(
        fn_calls in prop::collection::vec(arb_fn_call_event(), 0..8),
        final_text in arb_event_text(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let n = fn_calls.len();

            // Build event stream: function call events as partials, then a final text event
            let mut events: Vec<Result<Event, AdkError>> = fn_calls
                .iter()
                .map(|fc| Ok(fn_call_to_event(fc)))
                .collect();
            // Add a final text event so the collector has a deterministic response
            events.push(Ok(make_event("assistant", false, &final_text)));

            let resp = EventStreamCollector::new(events_to_stream(events))
                .collect()
                .await;

            // tool_calls should have exactly N entries
            prop_assert_eq!(
                resp.tool_calls.len(), n,
                "expected {} tool calls, got {}",
                n, resp.tool_calls.len()
            );

            // Each tool call should have the correct name and args
            for (tc, fc) in resp.tool_calls.iter().zip(fn_calls.iter()) {
                prop_assert_eq!(
                    &tc.name, &fc.name,
                    "tool call name mismatch: expected '{}', got '{}'",
                    fc.name, tc.name
                );
                prop_assert!(
                    tc.args.get(&fc.arg_key).is_some(),
                    "tool call args should contain key '{}', got: {:?}",
                    fc.arg_key, tc.args
                );
                prop_assert_eq!(
                    tc.id.as_deref(), Some(fc.id.as_str()),
                    "tool call id mismatch: expected '{}', got '{:?}'",
                    fc.id, tc.id
                );
            }

            // Response text should be the final text (not polluted by tool calls)
            prop_assert_eq!(
                &resp.text, &final_text,
                "response text should be the final text event"
            );

            Ok(())
        })?;
    }
}
