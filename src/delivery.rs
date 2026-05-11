//! Delivery strategies for streaming vs batch response delivery.
//!
//! `DeliveryStrategy` abstracts how partial and final response text is
//! delivered to the user through a channel. Two implementations:
//!
//! - `StreamingDelivery`: sends an initial placeholder, then edits in-place
//!   as partial events arrive, rate-limited to 1 edit/sec (R2.1, R2.2, R2.3).
//! - `BatchDelivery`: ignores partials, sends a single message on completion (R2.4).
//!
//! A factory function `select_strategy` picks the right one based on channel
//! capabilities and the configured `streamMode` (R2.5).

use crate::channel::{Channel, ChannelType, EditMessage, OutboundMessage};

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Instant;

/// Reference to a message being delivered — carries the context needed
/// to send or edit messages on a channel.
#[derive(Debug, Clone)]
pub struct MessageRef {
    /// The channel type (for building edit/send messages)
    pub channel_type: ChannelType,
    /// Account identifier for multi-account support
    pub account_id: String,
    /// Recipient / chat ID
    pub recipient_id: String,
    /// Platform message ID of the in-flight message (set after first send)
    pub message_id: Option<String>,
    /// Platform message ID to reply to (the user's original message)
    pub reply_to: Option<String>,
}

/// Abstracts streaming vs batch response delivery.
#[async_trait]
pub trait DeliveryStrategy: Send + Sync {
    /// Called on each partial event (streaming mode uses this to edit in-place).
    async fn on_partial(&self, text: &str, msg_ref: &MessageRef) -> anyhow::Result<()>;

    /// Called with the final complete text.
    async fn on_complete(&self, text: &str, msg_ref: &MessageRef) -> anyhow::Result<()>;
}

/// Internal mutable state for `StreamingDelivery`.
struct StreamingState {
    /// The platform message ID of the placeholder we sent.
    sent_message_id: Option<String>,
    /// When we last successfully sent an edit.
    last_edit: Option<Instant>,
    /// The most recent accumulated text (for retry on edit failure).
    latest_text: String,
}

/// Streaming delivery: send placeholder, edit in-place, rate-limit edits.
pub struct StreamingDelivery {
    channel: Arc<dyn Channel>,
    state: Mutex<StreamingState>,
    /// Minimum interval between edits (default 1 second).
    edit_interval: std::time::Duration,
}

impl StreamingDelivery {
    /// Create a new streaming delivery strategy for the given channel.
    pub fn new(channel: Arc<dyn Channel>) -> Self {
        Self {
            channel,
            state: Mutex::new(StreamingState {
                sent_message_id: None,
                last_edit: None,
                latest_text: String::new(),
            }),
            edit_interval: std::time::Duration::from_secs(1),
        }
    }

    /// Try to edit the in-flight message. On rate-limit failure, wait and retry
    /// with the latest accumulated text (R2.5).
    async fn try_edit(
        &self,
        msg_ref: &MessageRef,
        text: &str,
        message_id: &str,
    ) -> anyhow::Result<()> {
        let edit_msg = EditMessage {
            channel_type: msg_ref.channel_type,
            account_id: msg_ref.account_id.clone(),
            message_id: message_id.to_string(),
            recipient_id: msg_ref.recipient_id.clone(),
            text: text.to_string(),
        };

        match self.channel.edit(edit_msg).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let err_str = e.to_string();
                // "message is not modified" is benign — content hasn't changed
                if err_str.contains("message is not modified") {
                    tracing::debug!("edit skipped: message content unchanged");
                    return Ok(());
                }
                tracing::warn!(error = %e, "edit failed, retrying after rate-limit window");
                // Wait for the rate-limit window then retry with latest text
                tokio::time::sleep(self.edit_interval).await;

                let state = self.state.lock().await;
                let retry_text = if state.latest_text.is_empty() {
                    text.to_string()
                } else {
                    state.latest_text.clone()
                };
                drop(state);

                let retry_msg = EditMessage {
                    channel_type: msg_ref.channel_type,
                    account_id: msg_ref.account_id.clone(),
                    message_id: message_id.to_string(),
                    recipient_id: msg_ref.recipient_id.clone(),
                    text: retry_text,
                };
                self.channel.edit(retry_msg).await
            }
        }
    }
}

#[async_trait]
impl DeliveryStrategy for StreamingDelivery {
    async fn on_partial(&self, text: &str, msg_ref: &MessageRef) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;
        // Always track the latest text for retry purposes
        state.latest_text = text.to_string();

        if state.sent_message_id.is_none() {
            // First partial: send initial placeholder message
            let outbound = OutboundMessage {
                channel_type: msg_ref.channel_type,
                account_id: msg_ref.account_id.clone(),
                recipient_id: msg_ref.recipient_id.clone(),
                text: text.to_string(),
                reply_to: msg_ref.reply_to.clone(),
                is_partial: true,
            };
            let sent_id = self.channel.send(outbound).await?;
            // Use the platform message ID returned by send() for subsequent edits
            state.sent_message_id = sent_id.or_else(|| msg_ref.message_id.clone());
            state.last_edit = Some(Instant::now());
            return Ok(());
        }

        // Subsequent partials: rate-limit to 1 edit/sec (R2.2)
        if let Some(last) = state.last_edit {
            if last.elapsed() < self.edit_interval {
                // Throttled — skip this edit, latest_text is already stored for next time
                return Ok(());
            }
        }

        let message_id = state.sent_message_id.clone().unwrap();
        state.last_edit = Some(Instant::now());
        drop(state);

        self.try_edit(msg_ref, text, &message_id).await
    }

    async fn on_complete(&self, text: &str, msg_ref: &MessageRef) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;
        state.latest_text = text.to_string();

        if let Some(ref message_id) = state.sent_message_id {
            let message_id = message_id.clone();
            drop(state);
            // Final edit with complete text (R2.3)
            self.try_edit(msg_ref, text, &message_id).await
        } else {
            drop(state);
            // No placeholder was ever sent (e.g. response arrived instantly),
            // just send the full message.
            let outbound = OutboundMessage {
                channel_type: msg_ref.channel_type,
                account_id: msg_ref.account_id.clone(),
                recipient_id: msg_ref.recipient_id.clone(),
                text: text.to_string(),
                reply_to: msg_ref.reply_to.clone(),
                is_partial: false,
            };
            self.channel.send(outbound).await?;
            Ok(())
        }
    }
}

/// Batch delivery: ignore partials, send a single message on completion (R2.4).
pub struct BatchDelivery {
    channel: Arc<dyn Channel>,
}

impl BatchDelivery {
    /// Create a new batch delivery strategy for the given channel.
    pub fn new(channel: Arc<dyn Channel>) -> Self {
        Self { channel }
    }
}

#[async_trait]
impl DeliveryStrategy for BatchDelivery {
    async fn on_partial(&self, _text: &str, _msg_ref: &MessageRef) -> anyhow::Result<()> {
        // Batch mode ignores partial events
        Ok(())
    }

    async fn on_complete(&self, text: &str, msg_ref: &MessageRef) -> anyhow::Result<()> {
        let outbound = OutboundMessage {
            channel_type: msg_ref.channel_type,
            account_id: msg_ref.account_id.clone(),
            recipient_id: msg_ref.recipient_id.clone(),
            text: text.to_string(),
            reply_to: msg_ref.reply_to.clone(),
            is_partial: false,
        };
        self.channel.send(outbound).await?;
        Ok(())
    }
}

/// Select the appropriate delivery strategy based on channel capabilities
/// and the configured stream mode.
///
/// Uses `StreamingDelivery` when the channel supports editing AND the
/// stream mode is not `"complete"`. Otherwise falls back to `BatchDelivery`.
pub fn select_strategy(
    channel: Arc<dyn Channel>,
    stream_mode: Option<&str>,
) -> Arc<dyn DeliveryStrategy> {
    let use_streaming =
        channel.supports_editing() && stream_mode.map(|m| m != "complete").unwrap_or(true);

    if use_streaming {
        tracing::debug!(
            channel = %channel.channel_type(),
            "using streaming delivery (edit-in-place)"
        );
        Arc::new(StreamingDelivery::new(channel))
    } else {
        tracing::debug!(
            channel = %channel.channel_type(),
            "using batch delivery (single message)"
        );
        Arc::new(BatchDelivery::new(channel))
    }
}

/// Split a message into chunks that fit within `max_len`.
///
/// Tries to split at newlines or spaces to avoid breaking words.
/// Each chunk is guaranteed to have length ≤ `max_len`.
pub fn split_message(text: &str, max_len: usize) -> Vec<String> {
    if max_len == 0 {
        return vec![text.to_string()];
    }
    if text.len() <= max_len {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_string());
            break;
        }
        let slice = &remaining[..max_len];
        let at = slice
            .rfind('\n')
            .or_else(|| slice.rfind(' '))
            .unwrap_or(max_len);
        let at = if at == 0 { max_len } else { at };
        chunks.push(remaining[..at].to_string());
        remaining = remaining[at..].trim_start();
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{Channel, ChannelType, EditMessage, InboundMessage, OutboundMessage};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::sync::mpsc;

    /// A mock channel that tracks send/edit calls for testing.
    struct MockChannel {
        editing_supported: bool,
        send_count: AtomicU32,
        edit_count: AtomicU32,
        /// If set, the first N edits will fail to simulate rate limiting.
        fail_first_n_edits: AtomicU32,
    }

    impl MockChannel {
        fn new(editing_supported: bool) -> Self {
            Self {
                editing_supported,
                send_count: AtomicU32::new(0),
                edit_count: AtomicU32::new(0),
                fail_first_n_edits: AtomicU32::new(0),
            }
        }

        fn with_failing_edits(mut self, n: u32) -> Self {
            self.fail_first_n_edits = AtomicU32::new(n);
            self
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
            Ok(Some("mock-msg-123".to_string()))
        }

        async fn edit(&self, _msg: EditMessage) -> anyhow::Result<()> {
            let remaining = self.fail_first_n_edits.load(Ordering::SeqCst);
            if remaining > 0 {
                self.fail_first_n_edits.fetch_sub(1, Ordering::SeqCst);
                return Err(anyhow::anyhow!("rate limited"));
            }
            self.edit_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn supports_editing(&self) -> bool {
            self.editing_supported
        }

        async fn shutdown(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn test_msg_ref() -> MessageRef {
        MessageRef {
            channel_type: ChannelType::Telegram,
            account_id: "default".to_string(),
            recipient_id: "user123".to_string(),
            message_id: Some("msg_1".to_string()),
            reply_to: Some("orig_msg".to_string()),
        }
    }

    #[tokio::test]
    async fn test_batch_ignores_partials() {
        let ch = Arc::new(MockChannel::new(false));
        let strategy = BatchDelivery::new(ch.clone());
        let msg_ref = test_msg_ref();

        strategy.on_partial("hello", &msg_ref).await.unwrap();
        strategy.on_partial("hello world", &msg_ref).await.unwrap();
        assert_eq!(ch.sends(), 0);
        assert_eq!(ch.edits(), 0);
    }

    #[tokio::test]
    async fn test_batch_sends_on_complete() {
        let ch = Arc::new(MockChannel::new(false));
        let strategy = BatchDelivery::new(ch.clone());
        let msg_ref = test_msg_ref();

        strategy.on_complete("final text", &msg_ref).await.unwrap();
        assert_eq!(ch.sends(), 1);
        assert_eq!(ch.edits(), 0);
    }

    #[tokio::test]
    async fn test_streaming_sends_placeholder_on_first_partial() {
        let ch = Arc::new(MockChannel::new(true));
        let strategy = StreamingDelivery::new(ch.clone());
        let msg_ref = test_msg_ref();

        strategy.on_partial("hel", &msg_ref).await.unwrap();
        assert_eq!(ch.sends(), 1, "should send placeholder on first partial");
        assert_eq!(ch.edits(), 0, "no edits yet on first partial");
    }

    #[tokio::test]
    async fn test_streaming_rate_limits_edits() {
        let ch = Arc::new(MockChannel::new(true));
        let strategy = StreamingDelivery::new(ch.clone());
        let msg_ref = test_msg_ref();

        // First partial sends placeholder
        strategy.on_partial("a", &msg_ref).await.unwrap();
        // Immediate second partial should be throttled
        strategy.on_partial("ab", &msg_ref).await.unwrap();
        // Immediate third partial should also be throttled
        strategy.on_partial("abc", &msg_ref).await.unwrap();

        assert_eq!(ch.sends(), 1, "only one send (placeholder)");
        assert_eq!(ch.edits(), 0, "edits throttled within 1 sec");
    }

    #[tokio::test]
    async fn test_streaming_complete_performs_final_edit() {
        let ch = Arc::new(MockChannel::new(true));
        let strategy = StreamingDelivery::new(ch.clone());
        let msg_ref = test_msg_ref();

        strategy.on_partial("partial", &msg_ref).await.unwrap();
        strategy.on_complete("final", &msg_ref).await.unwrap();

        assert_eq!(ch.sends(), 1, "placeholder send");
        assert_eq!(ch.edits(), 1, "final edit");
    }

    #[tokio::test]
    async fn test_streaming_complete_without_partial_sends_message() {
        let ch = Arc::new(MockChannel::new(true));
        let strategy = StreamingDelivery::new(ch.clone());
        let msg_ref = test_msg_ref();

        // No partials, just complete
        strategy
            .on_complete("instant response", &msg_ref)
            .await
            .unwrap();
        assert_eq!(ch.sends(), 1, "should send as regular message");
        assert_eq!(ch.edits(), 0, "no edits needed");
    }

    #[tokio::test]
    async fn test_select_strategy_streaming_when_editing_supported() {
        let ch = Arc::new(MockChannel::new(true));
        let strategy = select_strategy(ch, None);
        // We can't downcast easily, but we can test behavior
        let msg_ref = test_msg_ref();
        // on_partial should not error
        strategy.on_partial("test", &msg_ref).await.unwrap();
    }

    #[tokio::test]
    async fn test_select_strategy_batch_when_complete_mode() {
        let ch = Arc::new(MockChannel::new(true));
        let strategy = select_strategy(ch.clone(), Some("complete"));
        let msg_ref = test_msg_ref();

        // Partials should be ignored (batch mode)
        strategy.on_partial("test", &msg_ref).await.unwrap();
        assert_eq!(ch.sends(), 0, "batch ignores partials");
    }

    #[tokio::test]
    async fn test_select_strategy_batch_when_no_editing() {
        let ch = Arc::new(MockChannel::new(false));
        let strategy = select_strategy(ch.clone(), Some("partial"));
        let msg_ref = test_msg_ref();

        strategy.on_partial("test", &msg_ref).await.unwrap();
        assert_eq!(
            ch.sends(),
            0,
            "batch ignores partials even with partial mode"
        );
    }

    #[tokio::test]
    async fn test_streaming_edit_failure_retries() {
        let ch = Arc::new(MockChannel::new(true).with_failing_edits(1));
        let strategy = StreamingDelivery {
            channel: ch.clone(),
            state: Mutex::new(StreamingState {
                sent_message_id: None,
                last_edit: None,
                latest_text: String::new(),
            }),
            // Use a very short interval for testing
            edit_interval: std::time::Duration::from_millis(10),
        };
        let msg_ref = test_msg_ref();

        // Send placeholder
        strategy.on_partial("a", &msg_ref).await.unwrap();
        assert_eq!(ch.sends(), 1);

        // Wait past rate limit
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // This edit will fail once, then retry successfully
        strategy.on_partial("ab", &msg_ref).await.unwrap();
        assert_eq!(ch.edits(), 1, "retry should succeed");
    }
}
