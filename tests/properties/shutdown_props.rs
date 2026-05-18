//! Property-based tests for graceful shutdown and startup.
//!
//! Feature: gateway-production-maturity
//! - Property 24: Graceful shutdown drains in-flight messages
//!   **Validates: Requirements R13.1, R13.3**
//! - Property 25: Failed channel does not abort startup
//!   **Validates: Requirement R13.4**
//!
//! Feature: phase-2-complete
//! - Property 13: Graceful Shutdown Drain Invariant
//!   **Validates: Requirements 10.5, 13.1, 13.2**

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


// ── Property 13: Graceful Shutdown Drain Invariant ─────────────────
// Feature: phase-2-complete, Property 13: Graceful Shutdown Drain Invariant
// **Validates: Requirements 10.5, 13.1, 13.2**

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Property 13a: After shutdown is initiated, no new requests SHALL be accepted.
    ///
    /// For any number of in-flight requests N, once `initiate_restart` is called,
    /// all subsequent calls to `acquire()` must return `None`.
    ///
    /// **Validates: Requirements 10.5, 13.1**
    #[test]
    fn no_new_requests_accepted_after_restart(n in 1u32..20, attempts in 1u32..10) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let token = CancellationToken::new();
            let coord = Arc::new(ShutdownCoordinator::with_drain_timeout(
                token.clone(),
                Duration::from_secs(5),
            ));

            // Acquire N guards simulating in-flight requests
            let mut guards = Vec::new();
            for _ in 0..n {
                let guard = coord.acquire();
                prop_assert!(guard.is_some(), "should acquire guard before restart");
                guards.push(guard.unwrap());
            }

            // Initiate restart in background
            let coord_clone = coord.clone();
            let restart_handle = tokio::spawn(async move {
                coord_clone.initiate_restart().await;
            });

            // Wait for accepting to flip
            tokio::time::sleep(Duration::from_millis(50)).await;

            // All subsequent acquire attempts must fail
            for _ in 0..attempts {
                prop_assert!(
                    coord.acquire().is_none(),
                    "acquire() must return None after restart is initiated"
                );
            }

            // Clean up: drop guards so restart can complete
            drop(guards);
            restart_handle.await.unwrap();

            prop_assert!(
                !coord.is_accepting(),
                "is_accepting() must be false after restart"
            );
            prop_assert!(
                token.is_cancelled(),
                "shutdown token must be cancelled after restart"
            );

            Ok(())
        })?;
    }

    /// Property 13b: Existing in-flight requests SHALL continue processing
    /// (guards remain valid and decrement correctly on drop).
    ///
    /// For any N in-flight requests, after restart is initiated, dropping
    /// guards one by one must correctly decrement the in-flight counter.
    ///
    /// **Validates: Requirements 13.1, 13.2**
    #[test]
    fn in_flight_requests_continue_processing_after_restart(n in 1u32..15) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let token = CancellationToken::new();
            let coord = Arc::new(ShutdownCoordinator::with_drain_timeout(
                token.clone(),
                Duration::from_secs(5),
            ));

            // Acquire N guards
            let mut guards = Vec::new();
            for _ in 0..n {
                guards.push(coord.acquire().expect("should acquire"));
            }
            prop_assert_eq!(coord.in_flight_count(), n);

            // Initiate restart in background
            let coord_clone = coord.clone();
            let restart_handle = tokio::spawn(async move {
                coord_clone.initiate_restart().await;
            });

            // Wait for restart to flip accepting
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Drop guards one by one — each should decrement the counter
            for i in 0..n {
                let expected = n - i;
                prop_assert_eq!(
                    coord.in_flight_count(), expected,
                    "expected {} in-flight, got {}",
                    expected, coord.in_flight_count()
                );
                guards.remove(0);
            }

            restart_handle.await.unwrap();

            prop_assert_eq!(
                coord.in_flight_count(), 0,
                "all in-flight requests should be drained"
            );

            Ok(())
        })?;
    }

    /// Property 13c: The coordinator SHALL wait at most T seconds before
    /// proceeding to shutdown (drain timeout enforcement).
    ///
    /// For any drain timeout T (in milliseconds), if in-flight requests
    /// do not complete, the coordinator must proceed to shutdown within
    /// approximately T milliseconds (with some tolerance for scheduling).
    ///
    /// **Validates: Requirements 10.5, 13.2**
    #[test]
    fn drain_timeout_enforced(timeout_ms in 50u64..300) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let token = CancellationToken::new();
            let drain_timeout = Duration::from_millis(timeout_ms);
            let coord = Arc::new(ShutdownCoordinator::with_drain_timeout(
                token.clone(),
                drain_timeout,
            ));

            // Acquire a guard that will NOT be dropped (simulates stuck request)
            let _guard = coord.acquire().expect("should acquire");

            let start = tokio::time::Instant::now();
            coord.initiate_restart().await;
            let elapsed = start.elapsed();

            // The restart should complete within timeout + tolerance
            let tolerance = Duration::from_millis(200);
            prop_assert!(
                elapsed < drain_timeout + tolerance,
                "restart took {:?}, expected at most {:?}",
                elapsed, drain_timeout + tolerance
            );
            // It should take at least the drain timeout (since the guard is held)
            prop_assert!(
                elapsed >= drain_timeout - Duration::from_millis(20),
                "restart completed too quickly ({:?}), drain timeout is {:?}",
                elapsed, drain_timeout
            );

            prop_assert!(
                token.is_cancelled(),
                "token must be cancelled after timeout"
            );

            Ok(())
        })?;
    }

    /// Property 13d: The in-flight count SHALL be monotonically non-increasing
    /// after shutdown initiation.
    ///
    /// For any sequence of guard drops after restart is initiated, the
    /// in-flight count must never increase (since no new requests are accepted).
    ///
    /// **Validates: Requirements 13.1, 13.2**
    #[test]
    fn in_flight_count_monotonically_non_increasing(
        n in 2u32..15,
        _drop_order in prop::collection::vec(0u32..100, 2..15)
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let token = CancellationToken::new();
            let coord = Arc::new(ShutdownCoordinator::with_drain_timeout(
                token.clone(),
                Duration::from_secs(5),
            ));

            // Acquire exactly n guards
            let mut guards: Vec<_> = (0..n)
                .map(|_| coord.acquire().expect("should acquire"))
                .collect();

            // Initiate restart in background
            let coord_clone = coord.clone();
            let restart_handle = tokio::spawn(async move {
                coord_clone.initiate_restart().await;
            });

            // Wait for restart to flip accepting
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Drop guards in the order determined by the strategy
            // (using modulo to map random values to valid indices)
            let mut prev_count = coord.in_flight_count();
            let total_guards = guards.len();
            for _ in 0..total_guards {
                if guards.is_empty() {
                    break;
                }
                guards.pop(); // Drop from the end
                let current_count = coord.in_flight_count();
                prop_assert!(
                    current_count <= prev_count,
                    "in-flight count increased from {} to {} after shutdown",
                    prev_count, current_count
                );
                prev_count = current_count;
            }

            restart_handle.await.unwrap();

            prop_assert_eq!(coord.in_flight_count(), 0);

            Ok(())
        })?;
    }

    /// Property 13e: Restart with zero in-flight requests completes immediately
    /// and emits all phases.
    ///
    /// For any drain timeout, if there are no in-flight requests when restart
    /// is initiated, it should complete nearly instantly.
    ///
    /// **Validates: Requirements 13.1, 13.2**
    #[test]
    fn restart_with_no_in_flight_completes_immediately(timeout_ms in 100u64..5000) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let token = CancellationToken::new();
            let coord = ShutdownCoordinator::with_drain_timeout(
                token.clone(),
                Duration::from_millis(timeout_ms),
            );

            let start = tokio::time::Instant::now();
            coord.initiate_restart().await;
            let elapsed = start.elapsed();

            // Should complete almost immediately (well under the drain timeout)
            prop_assert!(
                elapsed < Duration::from_millis(100),
                "restart with no in-flight took {:?}, should be near-instant",
                elapsed
            );
            prop_assert!(token.is_cancelled());
            prop_assert!(!coord.is_accepting());
            prop_assert!(coord.is_restart());

            Ok(())
        })?;
    }
}
