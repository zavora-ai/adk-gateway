//! Graceful shutdown coordination for the gateway.
//!
//! Tracks in-flight messages via an atomic counter and provides an RAII
//! [`MessageGuard`] that automatically decrements the counter on drop.
//! When a shutdown signal arrives the coordinator stops accepting new
//! messages and waits (up to a configurable drain timeout) for all
//! in-flight work to complete before returning.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Default drain timeout — 30 seconds (R13.1).
#[allow(dead_code)] // Used by ShutdownCoordinator::new as default; with_drain_timeout overrides
const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Interval between polls while waiting for in-flight messages to drain.
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(100);

// ─── ShutdownCoordinator ───────────────────────────────────────────

/// Coordinates graceful shutdown of the gateway.
///
/// * Tracks the number of in-flight messages with an [`AtomicU32`].
/// * Controls whether new messages are accepted via an [`AtomicBool`].
/// * On shutdown: flips `accepting` to `false`, waits up to
///   `drain_timeout` for the in-flight count to reach zero, then
///   cancels the provided [`CancellationToken`].
pub struct ShutdownCoordinator {
    /// Number of messages currently being processed.
    in_flight: AtomicU32,
    /// Whether the gateway is still accepting new inbound messages.
    accepting: AtomicBool,
    /// Token used to signal coordinated shutdown to the rest of the system.
    shutdown_token: CancellationToken,
    /// Maximum time to wait for in-flight messages to drain.
    drain_timeout: Duration,
}

impl ShutdownCoordinator {
    /// Create a new coordinator that will cancel `shutdown_token` when
    /// shutdown completes.
    #[allow(dead_code)] // Used in tests; with_drain_timeout is preferred in production
    pub fn new(shutdown_token: CancellationToken) -> Self {
        Self {
            in_flight: AtomicU32::new(0),
            accepting: AtomicBool::new(true),
            shutdown_token,
            drain_timeout: DEFAULT_DRAIN_TIMEOUT,
        }
    }

    /// Create a coordinator with a custom drain timeout.
    pub fn with_drain_timeout(shutdown_token: CancellationToken, drain_timeout: Duration) -> Self {
        Self {
            in_flight: AtomicU32::new(0),
            accepting: AtomicBool::new(true),
            shutdown_token,
            drain_timeout,
        }
    }

    /// Try to acquire a [`MessageGuard`] for processing a new message.
    ///
    /// Returns `Some(MessageGuard)` if the gateway is still accepting
    /// messages, or `None` if shutdown has been initiated.
    pub fn acquire(&self) -> Option<MessageGuard<'_>> {
        if !self.accepting.load(Ordering::SeqCst) {
            return None;
        }
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        // Double-check: if shutdown raced between the accepting check and
        // the increment, undo the increment and bail out.
        if !self.accepting.load(Ordering::SeqCst) {
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(MessageGuard { coordinator: self })
    }

    /// Initiate graceful shutdown.
    ///
    /// 1. Sets `accepting` to `false` so no new messages are accepted.
    /// 2. Waits up to `drain_timeout` for all in-flight messages to finish.
    /// 3. Cancels the `shutdown_token` to notify the rest of the system.
    pub async fn initiate_shutdown(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        tracing::info!("shutdown initiated, draining in-flight messages…");

        let drain = async {
            while self.in_flight.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(DRAIN_POLL_INTERVAL).await;
            }
        };

        match tokio::time::timeout(self.drain_timeout, drain).await {
            Ok(()) => {
                tracing::info!("all in-flight messages drained");
            }
            Err(_) => {
                tracing::warn!(
                    remaining = self.in_flight.load(Ordering::SeqCst),
                    "drain timeout reached, forcing shutdown"
                );
            }
        }

        self.shutdown_token.cancel();
    }

    /// Current number of in-flight messages.
    pub fn in_flight_count(&self) -> u32 {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Whether the coordinator is still accepting new messages.
    pub fn is_accepting(&self) -> bool {
        self.accepting.load(Ordering::SeqCst)
    }
}

// ─── MessageGuard ──────────────────────────────────────────────────

/// RAII guard representing an in-flight message.
///
/// Created via [`ShutdownCoordinator::acquire`]. When dropped the
/// in-flight counter is decremented automatically.
pub struct MessageGuard<'a> {
    coordinator: &'a ShutdownCoordinator,
}

impl Drop for MessageGuard<'_> {
    fn drop(&mut self) {
        self.coordinator.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

// ─── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_increments_and_drop_decrements() {
        let token = CancellationToken::new();
        let coord = ShutdownCoordinator::new(token);

        assert_eq!(coord.in_flight_count(), 0);
        assert!(coord.is_accepting());

        let g1 = coord.acquire().expect("should acquire");
        assert_eq!(coord.in_flight_count(), 1);

        let g2 = coord.acquire().expect("should acquire");
        assert_eq!(coord.in_flight_count(), 2);

        drop(g1);
        assert_eq!(coord.in_flight_count(), 1);

        drop(g2);
        assert_eq!(coord.in_flight_count(), 0);
    }

    #[test]
    fn acquire_returns_none_after_accepting_set_false() {
        let token = CancellationToken::new();
        let coord = ShutdownCoordinator::new(token);

        // Simulate shutdown by flipping accepting off.
        coord.accepting.store(false, Ordering::SeqCst);
        assert!(!coord.is_accepting());
        assert!(coord.acquire().is_none());
    }

    #[tokio::test]
    async fn initiate_shutdown_drains_then_cancels_token() {
        let token = CancellationToken::new();
        let coord = ShutdownCoordinator::with_drain_timeout(token.clone(), Duration::from_secs(5));

        // Acquire a guard simulating an in-flight message.
        let guard = coord.acquire().expect("should acquire");

        // Run shutdown and a delayed guard-drop concurrently.
        let ((), ()) = tokio::join!(coord.initiate_shutdown(), async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(guard);
        });

        assert!(!coord.is_accepting());
        assert_eq!(coord.in_flight_count(), 0);
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn initiate_shutdown_times_out_and_still_cancels() {
        let token = CancellationToken::new();
        let coord = ShutdownCoordinator::with_drain_timeout(
            token.clone(),
            Duration::from_millis(100), // very short timeout
        );

        // Hold a guard that will outlive the drain timeout.
        let _guard = coord.acquire().expect("should acquire");

        coord.initiate_shutdown().await;

        // Token should still be cancelled even though drain timed out.
        assert!(token.is_cancelled());
        // The guard is still alive so in_flight is 1.
        assert_eq!(coord.in_flight_count(), 1);
    }

    #[tokio::test]
    async fn shutdown_with_no_in_flight_completes_immediately() {
        let token = CancellationToken::new();
        let coord = ShutdownCoordinator::new(token.clone());

        coord.initiate_shutdown().await;

        assert!(token.is_cancelled());
        assert!(!coord.is_accepting());
        assert_eq!(coord.in_flight_count(), 0);
    }

    #[test]
    fn multiple_guards_track_correctly() {
        let token = CancellationToken::new();
        let coord = ShutdownCoordinator::new(token);

        let guards: Vec<_> = (0..10)
            .map(|_| coord.acquire().expect("should acquire"))
            .collect();
        assert_eq!(coord.in_flight_count(), 10);

        drop(guards);
        assert_eq!(coord.in_flight_count(), 0);
    }
}
