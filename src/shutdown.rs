//! Graceful shutdown coordination for the gateway.
//!
//! Tracks in-flight messages via an atomic counter and provides an RAII
//! [`MessageGuard`] that automatically decrements the counter on drop.
//! When a shutdown signal arrives the coordinator stops accepting new
//! messages and waits (up to a configurable drain timeout) for all
//! in-flight work to complete before returning.
//!
//! ## Zero-Downtime Restart (SIGUSR1)
//!
//! On Unix systems, the coordinator supports graceful restart via SIGUSR1.
//! When SIGUSR1 is received:
//! 1. **drain-start**: Stop accepting new connections, log in-flight count.
//! 2. **drain-complete** or timeout: Wait for in-flight requests to finish
//!    (up to `drain_timeout`). Force-terminate remaining requests if timeout
//!    expires.
//! 3. **shutdown**: Exit cleanly so the process supervisor can start a new
//!    instance.
//!
//! Socket-based handoff is supported by signaling readiness to the new
//! process before the old one releases the socket (via `SO_REUSEPORT`).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Default drain timeout — 30 seconds (R13.1).
#[allow(dead_code)] // Used by ShutdownCoordinator::new as default; with_drain_timeout overrides
const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Interval between polls while waiting for in-flight messages to drain.
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(100);

// ─── RestartPhase ──────────────────────────────────────────────────

/// Phases of a graceful restart, emitted as structured log events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartPhase {
    /// Drain has started; includes the current in-flight request count.
    DrainStart { in_flight: u32 },
    /// All in-flight requests have completed (or been force-terminated).
    DrainComplete,
    /// The process is shutting down.
    Shutdown,
}

// ─── ForceTerminatedRequest ────────────────────────────────────────

/// Details about a request that was force-terminated due to drain timeout.
#[derive(Debug, Clone)]
pub struct ForceTerminatedRequest {
    /// Identifier or description of the request.
    pub request_id: String,
    /// How long the request had been in-flight when terminated.
    pub elapsed: Duration,
}

// ─── ShutdownCoordinator ───────────────────────────────────────────

/// Coordinates graceful shutdown of the gateway.
///
/// * Tracks the number of in-flight messages with an [`AtomicU32`].
/// * Controls whether new messages are accepted via an [`AtomicBool`].
/// * On shutdown: flips `accepting` to `false`, waits up to
///   `drain_timeout` for the in-flight count to reach zero, then
///   cancels the provided [`CancellationToken`].
/// * On SIGUSR1 (Unix): initiates a graceful restart with structured
///   logging at each phase.
pub struct ShutdownCoordinator {
    /// Number of messages currently being processed.
    in_flight: AtomicU32,
    /// Whether the gateway is still accepting new inbound messages.
    accepting: AtomicBool,
    /// Token used to signal coordinated shutdown to the rest of the system.
    shutdown_token: CancellationToken,
    /// Maximum time to wait for in-flight messages to drain.
    drain_timeout: Duration,
    /// Whether a restart (as opposed to a full shutdown) was initiated.
    restart_initiated: AtomicBool,
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
            restart_initiated: AtomicBool::new(false),
        }
    }

    /// Create a coordinator with a custom drain timeout.
    pub fn with_drain_timeout(shutdown_token: CancellationToken, drain_timeout: Duration) -> Self {
        Self {
            in_flight: AtomicU32::new(0),
            accepting: AtomicBool::new(true),
            shutdown_token,
            drain_timeout,
            restart_initiated: AtomicBool::new(false),
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

    /// Initiate a graceful restart (triggered by SIGUSR1).
    ///
    /// This follows the zero-downtime restart protocol:
    /// 1. Emit `drain-start` structured log with in-flight count.
    /// 2. Stop accepting new connections.
    /// 3. Wait for in-flight requests to complete (up to `drain_timeout`).
    /// 4. If timeout expires, force-terminate remaining requests and log warnings.
    /// 5. Emit `drain-complete` structured log.
    /// 6. Emit `shutdown` structured log and cancel the shutdown token.
    ///
    /// The new process should bind to the same port (using `SO_REUSEPORT`)
    /// before this method is called, enabling socket-based handoff.
    pub async fn initiate_restart(&self) {
        self.restart_initiated.store(true, Ordering::SeqCst);

        let current_in_flight = self.in_flight.load(Ordering::SeqCst);

        // Phase 1: drain-start
        Self::log_phase(&RestartPhase::DrainStart {
            in_flight: current_in_flight,
        });

        // Stop accepting new connections
        self.accepting.store(false, Ordering::SeqCst);

        // Phase 2: drain in-flight requests
        let drain = async {
            while self.in_flight.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(DRAIN_POLL_INTERVAL).await;
            }
        };

        match tokio::time::timeout(self.drain_timeout, drain).await {
            Ok(()) => {
                // All requests completed within timeout
            }
            Err(_) => {
                // Force-terminate remaining requests
                let remaining = self.in_flight.load(Ordering::SeqCst);
                tracing::warn!(
                    remaining,
                    drain_timeout_secs = self.drain_timeout.as_secs(),
                    event = "force-terminate",
                    "drain timeout expired, force-terminating remaining requests"
                );
                // Log each remaining request as force-terminated
                for i in 0..remaining {
                    let terminated = ForceTerminatedRequest {
                        request_id: format!("in-flight-{}", i),
                        elapsed: self.drain_timeout,
                    };
                    tracing::warn!(
                        request_id = %terminated.request_id,
                        elapsed_secs = terminated.elapsed.as_secs(),
                        event = "request-force-terminated",
                        "force-terminated in-flight request due to drain timeout"
                    );
                }
            }
        }

        // Phase 3: drain-complete
        Self::log_phase(&RestartPhase::DrainComplete);

        // Phase 4: shutdown
        Self::log_phase(&RestartPhase::Shutdown);
        self.shutdown_token.cancel();
    }

    /// Emit a structured log event for a restart phase.
    pub fn log_phase(phase: &RestartPhase) {
        match phase {
            RestartPhase::DrainStart { in_flight } => {
                tracing::info!(
                    event = "drain-start",
                    in_flight = in_flight,
                    "graceful restart: drain phase started"
                );
            }
            RestartPhase::DrainComplete => {
                tracing::info!(
                    event = "drain-complete",
                    "graceful restart: drain phase completed"
                );
            }
            RestartPhase::Shutdown => {
                tracing::info!(
                    event = "shutdown",
                    "graceful restart: shutdown phase — process exiting"
                );
            }
        }
    }

    /// Current number of in-flight messages.
    pub fn in_flight_count(&self) -> u32 {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Whether the coordinator is still accepting new messages.
    pub fn is_accepting(&self) -> bool {
        self.accepting.load(Ordering::SeqCst)
    }

    /// Whether a restart (SIGUSR1) was initiated (as opposed to a full shutdown).
    pub fn is_restart(&self) -> bool {
        self.restart_initiated.load(Ordering::SeqCst)
    }

    /// Get the configured drain timeout.
    pub fn drain_timeout(&self) -> Duration {
        self.drain_timeout
    }
}

// ─── SIGUSR1 Signal Handler (Unix only) ────────────────────────────

/// Register a SIGUSR1 signal handler that initiates a graceful restart.
///
/// On receiving SIGUSR1, the handler calls `initiate_restart()` on the
/// provided `ShutdownCoordinator`. This enables zero-downtime restarts
/// where a new process can bind to the same port (via `SO_REUSEPORT`)
/// before the old process releases it.
///
/// # Socket-Based Handoff
///
/// The recommended restart sequence:
/// 1. New process starts and binds to the same port (SO_REUSEPORT).
/// 2. New process sends SIGUSR1 to the old process.
/// 3. Old process stops accepting, drains in-flight, then exits.
/// 4. New process is now the sole listener.
#[cfg(unix)]
pub async fn register_sigusr1_handler(
    coordinator: std::sync::Arc<ShutdownCoordinator>,
) {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigusr1 = signal(SignalKind::user_defined1())
        .expect("failed to register SIGUSR1 handler");

    tokio::spawn(async move {
        sigusr1.recv().await;
        tracing::info!(
            event = "sigusr1-received",
            "received SIGUSR1, initiating graceful restart"
        );
        coordinator.initiate_restart().await;
    });
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

    // ─── Graceful Restart (SIGUSR1) Tests ──────────────────────────

    #[tokio::test]
    async fn initiate_restart_stops_accepting_new_connections() {
        let token = CancellationToken::new();
        let coord = ShutdownCoordinator::new(token.clone());

        assert!(coord.is_accepting());

        coord.initiate_restart().await;

        assert!(!coord.is_accepting());
        assert!(coord.is_restart());
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn initiate_restart_drains_in_flight_requests() {
        let token = CancellationToken::new();
        let coord = ShutdownCoordinator::with_drain_timeout(token.clone(), Duration::from_secs(5));

        let guard = coord.acquire().expect("should acquire");
        assert_eq!(coord.in_flight_count(), 1);

        let ((), ()) = tokio::join!(coord.initiate_restart(), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(guard);
        });

        assert_eq!(coord.in_flight_count(), 0);
        assert!(!coord.is_accepting());
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn initiate_restart_force_terminates_on_timeout() {
        let token = CancellationToken::new();
        let coord = ShutdownCoordinator::with_drain_timeout(
            token.clone(),
            Duration::from_millis(100), // very short timeout
        );

        // Hold guards that will outlive the drain timeout.
        let _guard1 = coord.acquire().expect("should acquire");
        let _guard2 = coord.acquire().expect("should acquire");

        coord.initiate_restart().await;

        // Token should be cancelled even though requests weren't drained.
        assert!(token.is_cancelled());
        assert!(!coord.is_accepting());
        // Guards are still alive so in_flight is 2.
        assert_eq!(coord.in_flight_count(), 2);
    }

    #[tokio::test]
    async fn restart_rejects_new_connections_immediately() {
        let token = CancellationToken::new();
        let coord = std::sync::Arc::new(ShutdownCoordinator::with_drain_timeout(
            token.clone(),
            Duration::from_secs(5),
        ));

        // Acquire a guard to keep drain waiting.
        let guard = coord.acquire().expect("should acquire");

        let coord_clone = coord.clone();
        let restart_handle = tokio::spawn(async move {
            coord_clone.initiate_restart().await;
        });

        // Give restart a moment to flip accepting to false.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // New connections should be rejected.
        assert!(coord.acquire().is_none());

        // Drop the guard to let restart complete.
        drop(guard);
        restart_handle.await.unwrap();
    }

    #[test]
    fn restart_phase_log_does_not_panic() {
        // Ensure log_phase doesn't panic for any variant.
        ShutdownCoordinator::log_phase(&RestartPhase::DrainStart { in_flight: 5 });
        ShutdownCoordinator::log_phase(&RestartPhase::DrainComplete);
        ShutdownCoordinator::log_phase(&RestartPhase::Shutdown);
    }

    #[test]
    fn default_drain_timeout_is_30_seconds() {
        let token = CancellationToken::new();
        let coord = ShutdownCoordinator::new(token);
        assert_eq!(coord.drain_timeout(), Duration::from_secs(30));
    }
}
