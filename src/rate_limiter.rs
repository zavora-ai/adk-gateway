//! Sliding-window rate limiter for tool invocations.
//!
//! Tracks tool calls within a configurable time window and enforces rate limits
//! to prevent runaway agent loops. When the rate is exceeded, the limiter signals
//! a pause. After repeated triggers within a single request, it signals termination.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::config::RateLimitConfig;

// ── Rate Limit Decision ────────────────────────────────────────────

/// The decision returned after recording a tool invocation.
#[derive(Debug, Clone, PartialEq)]
pub enum RateLimitDecision {
    /// The invocation is allowed; proceed normally.
    Allow,
    /// The rate limit was exceeded; pause for the given duration before resuming.
    Pause { duration: Duration },
    /// The rate limit was triggered too many times; terminate the request.
    Terminate { reason: String },
}

// ── Rate Limiter ───────────────────────────────────────────────────

/// A sliding-window rate limiter for tool invocations within a single request.
///
/// Tracks timestamps of recent tool calls and enforces a maximum call rate.
/// When the rate is exceeded, it signals a pause. After `max_triggers` pauses
/// in a single request, it signals termination.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    /// Timestamps of tool invocations within the sliding window.
    window: VecDeque<Instant>,
    /// Number of times the rate limit has been triggered in this request.
    trigger_count: u32,
    /// Configuration for rate limiting behavior.
    config: RateLimitConfig,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            window: VecDeque::new(),
            trigger_count: 0,
            config,
        }
    }

    /// Create a new rate limiter with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(RateLimitConfig::default())
    }

    /// Record a tool invocation and return the rate limit decision.
    ///
    /// This method:
    /// 1. Removes expired entries from the sliding window
    /// 2. Adds the current invocation timestamp
    /// 3. Checks if the window count exceeds the threshold
    /// 4. Returns `Allow`, `Pause`, or `Terminate` accordingly
    pub fn record_invocation(&mut self, tool_name: &str, now: Instant) -> RateLimitDecision {
        // Remove entries outside the sliding window
        let window_duration = Duration::from_secs(self.config.window_secs);
        while let Some(&front) = self.window.front() {
            if now.duration_since(front) > window_duration {
                self.window.pop_front();
            } else {
                break;
            }
        }

        // Add the current invocation
        self.window.push_back(now);

        let count = self.window.len() as u32;

        // Check if we exceed the threshold
        if count > self.config.max_calls {
            self.trigger_count += 1;

            // Check if we've hit the termination threshold
            if self.trigger_count >= self.config.max_triggers {
                let reason = format!(
                    "Rate limit triggered {} times in this request (tool: '{}', {} calls in {}s window). Terminating to prevent runaway loop.",
                    self.trigger_count, tool_name, count, self.config.window_secs
                );
                warn!(
                    tool_name = %tool_name,
                    trigger_count = self.trigger_count,
                    window_count = count,
                    window_secs = self.config.window_secs,
                    "Rate limiter terminating request"
                );
                return RateLimitDecision::Terminate { reason };
            }

            let cooldown = Duration::from_secs(self.config.cooldown_secs);
            info!(
                tool_name = %tool_name,
                trigger_count = self.trigger_count,
                window_count = count,
                max_calls = self.config.max_calls,
                window_secs = self.config.window_secs,
                cooldown_secs = self.config.cooldown_secs,
                "Rate limit exceeded, pausing execution"
            );
            return RateLimitDecision::Pause { duration: cooldown };
        }

        RateLimitDecision::Allow
    }

    /// Get the current count of invocations within the sliding window.
    ///
    /// This prunes expired entries before counting.
    pub fn window_count(&self, now: Instant) -> u32 {
        let window_duration = Duration::from_secs(self.config.window_secs);
        self.window
            .iter()
            .filter(|&&ts| now.duration_since(ts) <= window_duration)
            .count() as u32
    }

    /// Get the number of times the rate limit has been triggered.
    pub fn trigger_count(&self) -> u32 {
        self.trigger_count
    }

    /// Reset the rate limiter state (e.g., for a new request).
    pub fn reset(&mut self) {
        self.window.clear();
        self.trigger_count = 0;
    }

    /// Get a reference to the current configuration.
    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> RateLimitConfig {
        RateLimitConfig::default()
    }

    #[test]
    fn test_allow_under_threshold() {
        let mut limiter = RateLimiter::new(default_config());
        let now = Instant::now();

        // 10 calls should all be allowed (threshold is 10)
        for i in 0..10 {
            let decision = limiter.record_invocation("test_tool", now + Duration::from_millis(i));
            assert_eq!(decision, RateLimitDecision::Allow);
        }
    }

    #[test]
    fn test_pause_on_exceed_threshold() {
        let config = RateLimitConfig {
            max_calls: 3,
            window_secs: 5,
            cooldown_secs: 2,
            max_triggers: 3,
        };
        let mut limiter = RateLimiter::new(config);
        let now = Instant::now();

        // First 3 calls are allowed
        for i in 0..3 {
            let decision =
                limiter.record_invocation("test_tool", now + Duration::from_millis(i * 10));
            assert_eq!(decision, RateLimitDecision::Allow);
        }

        // 4th call exceeds threshold → Pause
        let decision = limiter.record_invocation("test_tool", now + Duration::from_millis(30));
        assert_eq!(
            decision,
            RateLimitDecision::Pause {
                duration: Duration::from_secs(2)
            }
        );
    }

    #[test]
    fn test_terminate_after_max_triggers() {
        let config = RateLimitConfig {
            max_calls: 2,
            window_secs: 5,
            cooldown_secs: 1,
            max_triggers: 3,
        };
        let mut limiter = RateLimiter::new(config);
        let now = Instant::now();

        // Trigger 1: calls 1, 2 allowed, call 3 triggers pause
        limiter.record_invocation("tool_a", now);
        limiter.record_invocation("tool_a", now + Duration::from_millis(1));
        let d1 = limiter.record_invocation("tool_a", now + Duration::from_millis(2));
        assert!(matches!(d1, RateLimitDecision::Pause { .. }));
        assert_eq!(limiter.trigger_count(), 1);

        // Trigger 2: another call still in window
        let d2 = limiter.record_invocation("tool_a", now + Duration::from_millis(3));
        assert!(matches!(d2, RateLimitDecision::Pause { .. }));
        assert_eq!(limiter.trigger_count(), 2);

        // Trigger 3: should terminate
        let d3 = limiter.record_invocation("tool_a", now + Duration::from_millis(4));
        assert!(matches!(d3, RateLimitDecision::Terminate { .. }));
        assert_eq!(limiter.trigger_count(), 3);
    }

    #[test]
    fn test_sliding_window_expires_old_entries() {
        let config = RateLimitConfig {
            max_calls: 3,
            window_secs: 2,
            cooldown_secs: 1,
            max_triggers: 3,
        };
        let mut limiter = RateLimiter::new(config);
        let now = Instant::now();

        // Add 3 calls at time 0
        for i in 0..3 {
            limiter.record_invocation("tool_a", now + Duration::from_millis(i * 10));
        }

        // At time 0, window count is 3
        assert_eq!(limiter.window_count(now + Duration::from_millis(30)), 3);

        // After 3 seconds (beyond the 2s window), old entries expire
        let later = now + Duration::from_secs(3);
        assert_eq!(limiter.window_count(later), 0);

        // New call after expiry should be allowed
        let decision = limiter.record_invocation("tool_a", later);
        assert_eq!(decision, RateLimitDecision::Allow);
        assert_eq!(limiter.window_count(later), 1);
    }

    #[test]
    fn test_window_count_accuracy() {
        let config = RateLimitConfig {
            max_calls: 10,
            window_secs: 5,
            cooldown_secs: 3,
            max_triggers: 3,
        };
        let mut limiter = RateLimiter::new(config);
        let now = Instant::now();

        // Add 5 calls
        for i in 0..5 {
            limiter.record_invocation("tool_a", now + Duration::from_secs(i));
        }

        // At time 4s, all 5 are within the 5s window
        assert_eq!(limiter.window_count(now + Duration::from_secs(4)), 5);

        // At time 6s, the first call (at 0s) is outside the window
        assert_eq!(limiter.window_count(now + Duration::from_secs(6)), 4);

        // At time 10s, only the call at 5s... wait, last call was at 4s
        // At time 10s, calls at 0,1,2,3,4 are all > 5s old
        assert_eq!(limiter.window_count(now + Duration::from_secs(10)), 0);
    }

    #[test]
    fn test_reset_clears_state() {
        let config = RateLimitConfig {
            max_calls: 2,
            window_secs: 5,
            cooldown_secs: 1,
            max_triggers: 3,
        };
        let mut limiter = RateLimiter::new(config);
        let now = Instant::now();

        // Trigger a pause
        limiter.record_invocation("tool_a", now);
        limiter.record_invocation("tool_a", now + Duration::from_millis(1));
        limiter.record_invocation("tool_a", now + Duration::from_millis(2));
        assert_eq!(limiter.trigger_count(), 1);

        // Reset
        limiter.reset();
        assert_eq!(limiter.trigger_count(), 0);
        assert_eq!(limiter.window_count(now + Duration::from_millis(3)), 0);

        // After reset, calls are allowed again
        let decision = limiter.record_invocation("tool_a", now + Duration::from_millis(4));
        assert_eq!(decision, RateLimitDecision::Allow);
    }

    #[test]
    fn test_per_agent_config() {
        // Simulate per-agent configuration with different limits
        let strict_config = RateLimitConfig {
            max_calls: 2,
            window_secs: 10,
            cooldown_secs: 5,
            max_triggers: 2,
        };
        let lenient_config = RateLimitConfig {
            max_calls: 100,
            window_secs: 1,
            cooldown_secs: 1,
            max_triggers: 10,
        };

        let mut strict_limiter = RateLimiter::new(strict_config.clone());
        let mut lenient_limiter = RateLimiter::new(lenient_config.clone());
        let now = Instant::now();

        // Strict limiter triggers on 3rd call
        strict_limiter.record_invocation("tool", now);
        strict_limiter.record_invocation("tool", now + Duration::from_millis(1));
        let strict_decision =
            strict_limiter.record_invocation("tool", now + Duration::from_millis(2));
        assert!(matches!(strict_decision, RateLimitDecision::Pause { .. }));

        // Lenient limiter allows many more calls
        for i in 0..100 {
            let decision =
                lenient_limiter.record_invocation("tool", now + Duration::from_millis(i));
            assert_eq!(decision, RateLimitDecision::Allow);
        }
    }

    #[test]
    fn test_different_tool_names_still_counted() {
        let config = RateLimitConfig {
            max_calls: 3,
            window_secs: 5,
            cooldown_secs: 2,
            max_triggers: 3,
        };
        let mut limiter = RateLimiter::new(config);
        let now = Instant::now();

        // Different tool names all count toward the same window
        limiter.record_invocation("read_file", now);
        limiter.record_invocation("write_file", now + Duration::from_millis(1));
        limiter.record_invocation("list_dir", now + Duration::from_millis(2));

        // 4th call (any tool) exceeds threshold
        let decision = limiter.record_invocation("exec_cmd", now + Duration::from_millis(3));
        assert!(matches!(decision, RateLimitDecision::Pause { .. }));
    }

    #[test]
    fn test_default_config_values() {
        let config = RateLimitConfig::default();
        assert_eq!(config.max_calls, 10);
        assert_eq!(config.window_secs, 5);
        assert_eq!(config.cooldown_secs, 3);
        assert_eq!(config.max_triggers, 3);
    }

    #[test]
    fn test_exactly_at_threshold_is_allowed() {
        let config = RateLimitConfig {
            max_calls: 5,
            window_secs: 5,
            cooldown_secs: 1,
            max_triggers: 3,
        };
        let mut limiter = RateLimiter::new(config);
        let now = Instant::now();

        // Exactly 5 calls (== max_calls) should be allowed
        for i in 0..5 {
            let decision =
                limiter.record_invocation("tool", now + Duration::from_millis(i * 10));
            assert_eq!(decision, RateLimitDecision::Allow);
        }

        // 6th call (> max_calls) triggers pause
        let decision = limiter.record_invocation("tool", now + Duration::from_millis(50));
        assert!(matches!(decision, RateLimitDecision::Pause { .. }));
    }
}
