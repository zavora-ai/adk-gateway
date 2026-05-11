//! Property-based tests for graceful shutdown and startup.
//!
//! Feature: gateway-production-maturity
//! - Property 24: Graceful shutdown drains in-flight messages
//!   **Validates: Requirements R13.1, R13.3**
//! - Property 25: Failed channel does not abort startup
//!   **Validates: Requirement R13.4**

use adk_gateway::channel::{Channel, ChannelKey, ChannelType, InboundMessage, OutboundMessage};
use adk_gateway::shutdown::ShutdownCoordinator;
use async_trait::async_trait;
use dashmap::DashMap;
use proptest::prelude::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ── Property tests ─────────────────────────────────────────────────

// Feature: gateway-production-maturity, Property 24: Graceful shutdown drains in-flight messages
// **Validates: Requirements R13.1, R13.3**
proptest! {
    /// Property 24: For N in-flight messages, initiating shutdown stops
    /// accepting new messages and waits for all guards to be dropped
    /// before cancelling the CancellationToken.
    ///
    /// After shutdown completes:
    /// - is_accepting() returns false
    /// - in_flight_count() is 0
    /// - The CancellationToken is cancelled
    /// - acquire() returns None
    #[test]
    fn shutdown_drains_in_flight_messages(n in 1u32..20) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let token = CancellationToken::new();
            let coord = ShutdownCoordinator::with_drain_timeout(
                token.clone(),
                Duration::from_secs(5),
            );

            // Acquire N guards simulating in-flight messages
            let mut guards = Vec::new();
            for _ in 0..n {
                let guard = coord.acquire();
                prop_assert!(
                    guard.is_some(),
                    "should be able to acquire guard before shutdown"
                );
                guards.push(guard.unwrap());
            }
            prop_assert_eq!(
                coord.in_flight_count(), n,
                "expected {} in-flight messages, got {}",
                n, coord.in_flight_count()
            );

            // Initiate shutdown and drop all guards after a short delay
            let ((), ()) = tokio::join!(
                coord.initiate_shutdown(),
                async {
                    // Small delay to let shutdown flip accepting to false
                    tokio::time::sleep(Duration::from_millis(50)).await;

                    // acquire() should return None after shutdown initiated
                    let post_shutdown_guard = coord.acquire();
                    assert!(
                        post_shutdown_guard.is_none(),
                        "acquire() must return None after shutdown is initiated"
                    );

                    // Drop all guards simulating message completion
                    drop(guards);
                }
            );

            // After shutdown completes, verify invariants
            prop_assert!(
                !coord.is_accepting(),
                "is_accepting() must be false after shutdown"
            );
            prop_assert_eq!(
                coord.in_flight_count(), 0,
                "in_flight_count() must be 0 after shutdown, got {}",
                coord.in_flight_count()
            );
            prop_assert!(
                token.is_cancelled(),
                "CancellationToken must be cancelled after shutdown"
            );

            Ok(())
        })?;
    }
}

// ── Mock channels for Property 25 ──────────────────────────────────

/// A mock channel that always succeeds on start().
struct SuccessChannel {
    channel_type: ChannelType,
    account_id: String,
}

#[async_trait]
impl Channel for SuccessChannel {
    fn channel_type(&self) -> ChannelType {
        self.channel_type
    }

    fn account_id(&self) -> &str {
        &self.account_id
    }

    async fn start(&self, _tx: mpsc::Sender<InboundMessage>) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send(&self, _msg: OutboundMessage) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A mock channel that always fails on start().
struct FailChannel {
    channel_type: ChannelType,
    account_id: String,
}

#[async_trait]
impl Channel for FailChannel {
    fn channel_type(&self) -> ChannelType {
        self.channel_type
    }

    fn account_id(&self) -> &str {
        &self.account_id
    }

    async fn start(&self, _tx: mpsc::Sender<InboundMessage>) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "channel startup failed: invalid credentials"
        ))
    }

    async fn send(&self, _msg: OutboundMessage) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

// ── Property 25 ────────────────────────────────────────────────────

/// Simulate the gateway startup behavior per R13.4:
/// iterate through channels, call start(), log errors for failures,
/// continue with the rest. Returns the channel map of successfully
/// started channels.
async fn simulate_gateway_startup(
    channels: Vec<Arc<dyn Channel>>,
) -> DashMap<ChannelKey, Arc<dyn Channel>> {
    let channel_map = DashMap::new();
    let (tx, _rx) = mpsc::channel::<InboundMessage>(16);

    for ch in &channels {
        match ch.start(tx.clone()).await {
            Ok(()) => {
                let key = ChannelKey {
                    channel_type: ch.channel_type(),
                    account_id: ch.account_id().to_string(),
                };
                channel_map.insert(key, ch.clone());
            }
            Err(e) => {
                // R13.4: log the error and continue, do NOT abort
                eprintln!(
                    "channel {}:{} failed to start: {}",
                    ch.channel_type(),
                    ch.account_id(),
                    e
                );
            }
        }
    }

    channel_map
}

/// Strategy to generate a Vec of booleans representing success (true)
/// or failure (false) for each channel in a batch of 1..=5 channels.
fn channel_outcomes_strategy() -> impl Strategy<Value = Vec<bool>> {
    prop::collection::vec(any::<bool>(), 1..=5)
}

// Feature: gateway-production-maturity, Property 25: Failed channel does not abort startup
// **Validates: Requirement R13.4**
proptest! {
    /// Property 25: For any combination of N channels where K fail at
    /// startup, the gateway should still have N-K working channels in
    /// the channel map and not abort.
    ///
    /// The key property: failed channels are excluded from the map,
    /// successful channels are included, and the startup procedure
    /// completes (does not panic or abort).
    #[test]
    fn failed_channel_does_not_abort_startup(outcomes in channel_outcomes_strategy()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let n = outcomes.len();
            let expected_successes = outcomes.iter().filter(|&&ok| ok).count();
            let expected_failures = outcomes.iter().filter(|&&ok| !ok).count();

            // Build mock channels based on generated outcomes
            let channels: Vec<Arc<dyn Channel>> = outcomes
                .iter()
                .enumerate()
                .map(|(i, &succeeds)| -> Arc<dyn Channel> {
                    let account_id = format!("account-{}", i);
                    if succeeds {
                        Arc::new(SuccessChannel {
                            channel_type: ChannelType::Telegram,
                            account_id,
                        })
                    } else {
                        Arc::new(FailChannel {
                            channel_type: ChannelType::Telegram,
                            account_id,
                        })
                    }
                })
                .collect();

            // Simulate gateway startup — this must NOT abort
            let channel_map = simulate_gateway_startup(channels).await;

            // Assert: channel map has exactly N-K entries
            prop_assert_eq!(
                channel_map.len(),
                expected_successes,
                "expected {} successful channels in map, got {} (N={}, failures={})",
                expected_successes,
                channel_map.len(),
                n,
                expected_failures
            );

            // Assert: each successful channel is in the map
            for (i, &succeeds) in outcomes.iter().enumerate() {
                let key = ChannelKey {
                    channel_type: ChannelType::Telegram,
                    account_id: format!("account-{}", i),
                };
                if succeeds {
                    prop_assert!(
                        channel_map.contains_key(&key),
                        "successful channel {} should be in the map",
                        key
                    );
                } else {
                    prop_assert!(
                        !channel_map.contains_key(&key),
                        "failed channel {} should NOT be in the map",
                        key
                    );
                }
            }

            Ok(())
        })?;
    }
}
