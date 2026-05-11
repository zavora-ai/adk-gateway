//! Property-based tests for delivery strategies.
//!
//! Feature: gateway-production-maturity
//! - Property 4: Streaming delivery rate limiting invariant
//!   **Validates: Requirements R2.1, R2.2, R2.3**
//! - Property 5: Batch delivery sends exactly one message
//!   **Validates: Requirements R2.4**

use adk_gateway::channel::{Channel, ChannelType, EditMessage, InboundMessage, OutboundMessage};
use adk_gateway::delivery::{BatchDelivery, DeliveryStrategy, MessageRef, StreamingDelivery};
use async_trait::async_trait;
use proptest::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

// ── Mock channel ───────────────────────────────────────────────────

/// A mock channel that counts send/edit calls for property testing.
struct MockChannel {
    send_count: AtomicU32,
    edit_count: AtomicU32,
}

impl MockChannel {
    fn new() -> Self {
        Self {
            send_count: AtomicU32::new(0),
            edit_count: AtomicU32::new(0),
        }
    }

    fn sends(&self) -> u32 {
        self.send_count.load(Ordering::SeqCst)
    }

    fn edits(&self) -> u32 {
        self.edit_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Channel for MockChannel {
    fn channel_type(&self) -> ChannelType {
        ChannelType::Telegram
    }

    async fn start(&self, _tx: mpsc::Sender<InboundMessage>) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send(&self, _msg: OutboundMessage) -> anyhow::Result<Option<String>> {
        self.send_count.fetch_add(1, Ordering::SeqCst);
        Ok(Some("mock-123".to_string()))
    }

    async fn edit(&self, _msg: EditMessage) -> anyhow::Result<()> {
        self.edit_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn supports_editing(&self) -> bool {
        true
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

// ── Helpers ────────────────────────────────────────────────────────

fn test_msg_ref() -> MessageRef {
    MessageRef {
        channel_type: ChannelType::Telegram,
        account_id: "default".to_string(),
        recipient_id: "user123".to_string(),
        message_id: Some("msg_1".to_string()),
        reply_to: Some("orig_msg".to_string()),
    }
}

// ── Property tests ─────────────────────────────────────────────────

// Feature: gateway-production-maturity, Property 4: Streaming delivery rate limiting invariant
// **Validates: Requirements R2.1, R2.2, R2.3**
proptest! {
    /// Property 4: For N partial events arriving within ~0 seconds (T≈0),
    /// at most T+1 = 1 edit calls should be made.
    ///
    /// The first partial triggers a `send()` (placeholder), not an edit.
    /// All subsequent partials arrive instantly and are throttled by the
    /// 1-edit-per-second rate limiter, so zero edits should occur.
    ///
    /// General invariant: for N partials in T seconds, at most T+1 edits.
    /// Since T≈0 here, we expect at most 1 edit (and in practice 0, since
    /// the rate limiter hasn't expired after the first send).
    #[test]
    fn streaming_delivery_rate_limits_edits(n in 2u32..20) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ch = Arc::new(MockChannel::new());
            let strategy = StreamingDelivery::new(ch.clone());
            let msg_ref = test_msg_ref();

            // Send N partial events as fast as possible (T ≈ 0 seconds)
            for i in 0..n {
                let text = format!("partial text {}", i);
                strategy.on_partial(&text, &msg_ref).await.unwrap();
            }

            // The first partial triggers a send (placeholder), not an edit
            prop_assert_eq!(
                ch.sends(), 1,
                "exactly one send (placeholder) expected, got {}",
                ch.sends()
            );

            // T ≈ 0 seconds, so at most T+1 = 1 edit call.
            // In practice, all partials after the first are throttled,
            // so we expect 0 edits. The invariant is: edits <= 1.
            let edits = ch.edits();
            prop_assert!(
                edits <= 1,
                "for {} partials in ~0 seconds, expected at most 1 edit, got {}",
                n, edits
            );

            Ok(())
        })?;
    }
}

// Feature: gateway-production-maturity, Property 5: Batch delivery sends exactly one message
// **Validates: Requirements R2.4**
proptest! {
    /// Property 5: Batch delivery sends exactly one message.
    ///
    /// Given N partial events (0..20) followed by one complete event with
    /// arbitrary text, BatchDelivery ignores all partials and sends exactly
    /// one message on complete. Zero edit calls are ever made.
    #[test]
    fn batch_delivery_sends_exactly_one_message(
        n in 0u32..20,
        complete_text in "\\PC{1,100}",
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ch = Arc::new(MockChannel::new());
            let strategy = BatchDelivery::new(ch.clone());
            let msg_ref = test_msg_ref();

            // Send N partial events — all should be ignored
            for i in 0..n {
                let partial = format!("partial {}", i);
                strategy.on_partial(&partial, &msg_ref).await.unwrap();
            }

            // After partials, no sends or edits should have occurred
            prop_assert_eq!(
                ch.sends(), 0,
                "expected 0 sends after {} partials, got {}",
                n, ch.sends()
            );
            prop_assert_eq!(
                ch.edits(), 0,
                "expected 0 edits after {} partials, got {}",
                n, ch.edits()
            );

            // Send the complete event
            strategy.on_complete(&complete_text, &msg_ref).await.unwrap();

            // Exactly 1 send call for the complete message
            prop_assert_eq!(
                ch.sends(), 1,
                "expected exactly 1 send after complete, got {}",
                ch.sends()
            );

            // Zero edit calls — batch mode never edits
            prop_assert_eq!(
                ch.edits(), 0,
                "expected 0 edits in batch mode, got {}",
                ch.edits()
            );

            Ok(())
        })?;
    }
}

// ── Property 22: split_message preserves content ───────────────────
// Feature: gateway-full-wiring, Property 22: split_message preserves content
// **Validates: Requirements 19.3**

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 22: For any non-empty string and max_len > 0, joining
    /// split_message output should produce content matching the original,
    /// and each chunk should have length <= max_len.
    #[test]
    fn split_message_preserves_content(
        text in "[a-zA-Z0-9 \n.!?,]{1,200}",
        max_len in 1usize..=100,
    ) {
        let chunks = adk_gateway::delivery::split_message(&text, max_len);

        // Must produce at least one chunk
        prop_assert!(
            !chunks.is_empty(),
            "split_message should produce at least one chunk"
        );

        // Each chunk must have length <= max_len
        for (i, chunk) in chunks.iter().enumerate() {
            prop_assert!(
                chunk.len() <= max_len,
                "chunk {} has length {} which exceeds max_len {}",
                i, chunk.len(), max_len
            );
        }

        // Joining chunks (preserving whitespace boundaries) should produce
        // content whose non-whitespace characters match the original
        let joined: String = chunks.join(" ");
        let original_non_ws: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let joined_non_ws: String = joined.chars().filter(|c| !c.is_whitespace()).collect();

        prop_assert_eq!(
            original_non_ws, joined_non_ws,
            "non-whitespace content should be preserved after split and join"
        );
    }
}
