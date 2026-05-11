//! Exponential backoff reconnection logic for channels (R8.1–R8.5).
//!
//! Provides `ReconnectPolicy` (configuration) and `ReconnectState` (runtime
//! tracking) that channels use to implement automatic reconnection with
//! exponential backoff after connection drops.

use crate::channel::ChannelStatus;
use std::time::Duration;

/// Configuration for reconnection behaviour.
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    /// Initial delay before the first reconnection attempt (default: 1s).
    pub initial_delay: Duration,
    /// Maximum delay between reconnection attempts (default: 5min).
    pub max_delay: Duration,
    /// Number of consecutive failures before marking the channel as failed.
    pub max_attempts: u32,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(300), // 5 minutes
            max_attempts: 10,
        }
    }
}

/// Runtime state tracking for an ongoing reconnection sequence.
#[derive(Debug, Clone)]
pub struct ReconnectState {
    /// How many consecutive reconnection attempts have been made.
    pub attempts: u32,
    /// Timestamp of the last reconnection attempt.
    pub last_attempt: Option<std::time::Instant>,
    /// The delay that will be used for the *next* attempt.
    pub current_delay: Duration,
    /// The policy governing this reconnection sequence.
    policy: ReconnectPolicy,
}

impl ReconnectState {
    /// Create a new `ReconnectState` with the given policy.
    pub fn new(policy: ReconnectPolicy) -> Self {
        let initial = policy.initial_delay;
        Self {
            attempts: 0,
            last_attempt: None,
            current_delay: initial,
            policy,
        }
    }

    /// Record a failed attempt and return the delay to wait before the
    /// next retry. The delay doubles on each call (exponential backoff)
    /// and is capped at `policy.max_delay`.
    pub fn next_delay(&mut self) -> Duration {
        self.attempts += 1;
        self.last_attempt = Some(std::time::Instant::now());

        let delay = self.current_delay;

        // Double for next time, capped at max_delay
        self.current_delay =
            std::cmp::min(self.current_delay.saturating_mul(2), self.policy.max_delay);

        delay
    }

    /// Returns `true` when the channel should be marked as permanently
    /// failed (i.e. consecutive attempts ≥ `max_attempts`).
    pub fn should_mark_failed(&self) -> bool {
        self.attempts >= self.policy.max_attempts
    }

    /// Reset the state after a successful reconnection.
    pub fn reset(&mut self) {
        self.attempts = 0;
        self.last_attempt = None;
        self.current_delay = self.policy.initial_delay;
    }

    /// Return the `ChannelStatus` that should be reported based on the
    /// current reconnection state.
    pub fn channel_status(&self) -> ChannelStatus {
        if self.should_mark_failed() {
            ChannelStatus::Failed
        } else if self.attempts > 0 {
            ChannelStatus::Reconnecting
        } else {
            ChannelStatus::Connected
        }
    }

    /// Return a reference to the underlying policy.
    pub fn policy(&self) -> &ReconnectPolicy {
        &self.policy
    }
}

// ── Unit tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_values() {
        let p = ReconnectPolicy::default();
        assert_eq!(p.initial_delay, Duration::from_secs(1));
        assert_eq!(p.max_delay, Duration::from_secs(300));
        assert_eq!(p.max_attempts, 10);
    }

    #[test]
    fn exponential_backoff_doubles() {
        let policy = ReconnectPolicy {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(300),
            max_attempts: 10,
        };
        let mut state = ReconnectState::new(policy);

        assert_eq!(state.next_delay(), Duration::from_secs(1));
        assert_eq!(state.next_delay(), Duration::from_secs(2));
        assert_eq!(state.next_delay(), Duration::from_secs(4));
        assert_eq!(state.next_delay(), Duration::from_secs(8));
        assert_eq!(state.next_delay(), Duration::from_secs(16));
    }

    #[test]
    fn backoff_caps_at_max_delay() {
        let policy = ReconnectPolicy {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
            max_attempts: 20,
        };
        let mut state = ReconnectState::new(policy);

        // 1, 2, 4, 8, 10, 10, ...
        assert_eq!(state.next_delay(), Duration::from_secs(1));
        assert_eq!(state.next_delay(), Duration::from_secs(2));
        assert_eq!(state.next_delay(), Duration::from_secs(4));
        assert_eq!(state.next_delay(), Duration::from_secs(8));
        assert_eq!(state.next_delay(), Duration::from_secs(10));
        assert_eq!(state.next_delay(), Duration::from_secs(10));
    }

    #[test]
    fn should_mark_failed_after_max_attempts() {
        let policy = ReconnectPolicy {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(300),
            max_attempts: 3,
        };
        let mut state = ReconnectState::new(policy);

        assert!(!state.should_mark_failed());
        state.next_delay();
        assert!(!state.should_mark_failed());
        state.next_delay();
        assert!(!state.should_mark_failed());
        state.next_delay(); // 3rd attempt
        assert!(state.should_mark_failed());
    }

    #[test]
    fn reset_clears_state() {
        let policy = ReconnectPolicy {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(300),
            max_attempts: 10,
        };
        let mut state = ReconnectState::new(policy);

        state.next_delay();
        state.next_delay();
        assert_eq!(state.attempts, 2);
        assert!(state.last_attempt.is_some());

        state.reset();
        assert_eq!(state.attempts, 0);
        assert!(state.last_attempt.is_none());
        assert_eq!(state.current_delay, Duration::from_secs(1));
        // After reset, first delay should be initial again
        assert_eq!(state.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn channel_status_transitions() {
        let policy = ReconnectPolicy {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(300),
            max_attempts: 3,
        };
        let mut state = ReconnectState::new(policy);

        assert_eq!(state.channel_status(), ChannelStatus::Connected);

        state.next_delay();
        assert_eq!(state.channel_status(), ChannelStatus::Reconnecting);

        state.next_delay();
        assert_eq!(state.channel_status(), ChannelStatus::Reconnecting);

        state.next_delay(); // 3rd = max_attempts
        assert_eq!(state.channel_status(), ChannelStatus::Failed);

        // Reset brings it back to Connected
        state.reset();
        assert_eq!(state.channel_status(), ChannelStatus::Connected);
    }

    #[test]
    fn zero_attempts_is_not_failed() {
        let policy = ReconnectPolicy {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(300),
            max_attempts: 0,
        };
        let state = ReconnectState::new(policy);
        // max_attempts=0 means immediately failed
        assert!(state.should_mark_failed());
    }
}
