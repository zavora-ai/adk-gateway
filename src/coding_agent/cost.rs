//! Cost tracking and spending limit enforcement for coding agent executions.
//!
//! Provides per-agent cumulative cost records, cost cap enforcement, and
//! billing period reset functionality.

use chrono::{DateTime, Utc};
use dashmap::DashMap;

use super::error::CodingAgentError;
use super::models::TokenUsage;

/// Cumulative cost record for a single agent within a billing period.
#[derive(Debug, Clone)]
pub struct AgentCostRecord {
    /// The agent this record belongs to.
    pub agent_id: String,
    /// Total input tokens consumed across all tasks in this period.
    pub total_input_tokens: u64,
    /// Total output tokens generated across all tasks in this period.
    pub total_output_tokens: u64,
    /// Estimated total cost in USD for this period.
    pub estimated_total_cost_usd: f64,
    /// Number of tasks executed in this period.
    pub task_count: u64,
    /// Start of the current billing period.
    pub period_start: DateTime<Utc>,
}

/// Tracks token usage and cost for coding agent executions.
///
/// Thread-safe via `DashMap`. Supports per-agent cost accumulation,
/// cost cap enforcement, and billing period resets.
pub struct CostTracker {
    /// Per-agent cumulative cost records.
    agent_costs: DashMap<String, AgentCostRecord>,
    /// Per-agent cost caps from configuration (USD).
    task_caps: DashMap<String, f64>,
}

impl CostTracker {
    /// Create a new `CostTracker` with no existing records.
    pub fn new() -> Self {
        Self {
            agent_costs: DashMap::new(),
            task_caps: DashMap::new(),
        }
    }

    /// Record token usage from a completed task for the given agent.
    ///
    /// Accumulates input tokens, output tokens, estimated cost, and increments
    /// the task count. If no record exists for the agent, one is created with
    /// the current time as the period start.
    pub fn record_usage(&self, agent_id: &str, usage: &TokenUsage) {
        self.agent_costs
            .entry(agent_id.to_string())
            .and_modify(|record| {
                record.total_input_tokens += usage.input_tokens;
                record.total_output_tokens += usage.output_tokens;
                record.estimated_total_cost_usd += usage.estimated_cost_usd;
                record.task_count += 1;
            })
            .or_insert_with(|| AgentCostRecord {
                agent_id: agent_id.to_string(),
                total_input_tokens: usage.input_tokens,
                total_output_tokens: usage.output_tokens,
                estimated_total_cost_usd: usage.estimated_cost_usd,
                task_count: 1,
                period_start: Utc::now(),
            });
    }

    /// Check whether the current cost exceeds the configured cap for an agent.
    ///
    /// Returns `Ok(())` if the cost is within the cap, or
    /// `Err(CodingAgentError::CostCapExceeded)` if the cap is exceeded.
    pub fn check_cost_cap(
        &self,
        _agent_id: &str,
        current_cost: f64,
        cap: f64,
    ) -> Result<(), CodingAgentError> {
        if current_cost > cap {
            Err(CodingAgentError::CostCapExceeded {
                spent_usd: current_cost,
                cap_usd: cap,
            })
        } else {
            Ok(())
        }
    }

    /// Retrieve the cost statistics for a given agent.
    ///
    /// Returns `None` if no usage has been recorded for the agent.
    pub fn get_agent_stats(&self, agent_id: &str) -> Option<AgentCostRecord> {
        self.agent_costs.get(agent_id).map(|r| r.clone())
    }

    /// Reset the billing period for a given agent.
    ///
    /// Clears all accumulated totals and sets the period start to the current time.
    /// This is typically called at the start of a new monthly billing period.
    pub fn reset_period(&self, agent_id: &str) {
        if let Some(mut record) = self.agent_costs.get_mut(agent_id) {
            record.total_input_tokens = 0;
            record.total_output_tokens = 0;
            record.estimated_total_cost_usd = 0.0;
            record.task_count = 0;
            record.period_start = Utc::now();
        }
    }

    /// Set a per-task cost cap for an agent.
    pub fn set_task_cap(&self, agent_id: &str, cap_usd: f64) {
        self.task_caps.insert(agent_id.to_string(), cap_usd);
    }

    /// Get the configured per-task cost cap for an agent.
    pub fn get_task_cap(&self, agent_id: &str) -> Option<f64> {
        self.task_caps.get(agent_id).map(|v| *v)
    }
}

impl Default for CostTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_usage(input: u64, output: u64, cost: f64) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            estimated_cost_usd: cost,
        }
    }

    #[test]
    fn test_new_tracker_has_no_records() {
        let tracker = CostTracker::new();
        assert!(tracker.get_agent_stats("agent-1").is_none());
    }

    #[test]
    fn test_record_usage_creates_new_record() {
        let tracker = CostTracker::new();
        let usage = make_usage(100, 50, 0.01);

        tracker.record_usage("agent-1", &usage);

        let stats = tracker.get_agent_stats("agent-1").unwrap();
        assert_eq!(stats.agent_id, "agent-1");
        assert_eq!(stats.total_input_tokens, 100);
        assert_eq!(stats.total_output_tokens, 50);
        assert!((stats.estimated_total_cost_usd - 0.01).abs() < f64::EPSILON);
        assert_eq!(stats.task_count, 1);
    }

    #[test]
    fn test_record_usage_accumulates_totals() {
        let tracker = CostTracker::new();

        tracker.record_usage("agent-1", &make_usage(100, 50, 0.01));
        tracker.record_usage("agent-1", &make_usage(200, 100, 0.02));
        tracker.record_usage("agent-1", &make_usage(300, 150, 0.03));

        let stats = tracker.get_agent_stats("agent-1").unwrap();
        assert_eq!(stats.total_input_tokens, 600);
        assert_eq!(stats.total_output_tokens, 300);
        assert!((stats.estimated_total_cost_usd - 0.06).abs() < 1e-10);
        assert_eq!(stats.task_count, 3);
    }

    #[test]
    fn test_record_usage_separate_agents() {
        let tracker = CostTracker::new();

        tracker.record_usage("agent-1", &make_usage(100, 50, 0.01));
        tracker.record_usage("agent-2", &make_usage(200, 100, 0.02));

        let stats1 = tracker.get_agent_stats("agent-1").unwrap();
        let stats2 = tracker.get_agent_stats("agent-2").unwrap();

        assert_eq!(stats1.total_input_tokens, 100);
        assert_eq!(stats2.total_input_tokens, 200);
        assert_eq!(stats1.task_count, 1);
        assert_eq!(stats2.task_count, 1);
    }

    #[test]
    fn test_check_cost_cap_within_limit() {
        let tracker = CostTracker::new();
        let result = tracker.check_cost_cap("agent-1", 4.99, 5.0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_cost_cap_at_exact_limit() {
        let tracker = CostTracker::new();
        // At exactly the cap, should not exceed (not strictly greater)
        let result = tracker.check_cost_cap("agent-1", 5.0, 5.0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_cost_cap_exceeded() {
        let tracker = CostTracker::new();
        let result = tracker.check_cost_cap("agent-1", 5.01, 5.0);
        assert!(result.is_err());

        match result.unwrap_err() {
            CodingAgentError::CostCapExceeded { spent_usd, cap_usd } => {
                assert!((spent_usd - 5.01).abs() < f64::EPSILON);
                assert!((cap_usd - 5.0).abs() < f64::EPSILON);
            }
            other => panic!("Expected CostCapExceeded, got: {:?}", other),
        }
    }

    #[test]
    fn test_get_agent_stats_returns_none_for_unknown() {
        let tracker = CostTracker::new();
        assert!(tracker.get_agent_stats("nonexistent").is_none());
    }

    #[test]
    fn test_reset_period_clears_totals() {
        let tracker = CostTracker::new();

        tracker.record_usage("agent-1", &make_usage(1000, 500, 1.50));
        tracker.record_usage("agent-1", &make_usage(2000, 1000, 3.00));

        // Verify accumulated before reset
        let before = tracker.get_agent_stats("agent-1").unwrap();
        assert_eq!(before.total_input_tokens, 3000);
        assert_eq!(before.task_count, 2);

        tracker.reset_period("agent-1");

        let after = tracker.get_agent_stats("agent-1").unwrap();
        assert_eq!(after.total_input_tokens, 0);
        assert_eq!(after.total_output_tokens, 0);
        assert!((after.estimated_total_cost_usd).abs() < f64::EPSILON);
        assert_eq!(after.task_count, 0);
        assert_eq!(after.agent_id, "agent-1");
        // period_start should be updated (at least not before the original)
        assert!(after.period_start >= before.period_start);
    }

    #[test]
    fn test_reset_period_no_op_for_unknown_agent() {
        let tracker = CostTracker::new();
        // Should not panic
        tracker.reset_period("nonexistent");
        assert!(tracker.get_agent_stats("nonexistent").is_none());
    }

    #[test]
    fn test_set_and_get_task_cap() {
        let tracker = CostTracker::new();
        tracker.set_task_cap("agent-1", 10.0);

        let cap = tracker.get_task_cap("agent-1");
        assert_eq!(cap, Some(10.0));
    }

    #[test]
    fn test_get_task_cap_returns_none_for_unknown() {
        let tracker = CostTracker::new();
        assert!(tracker.get_task_cap("nonexistent").is_none());
    }

    #[test]
    fn test_default_creates_empty_tracker() {
        let tracker = CostTracker::default();
        assert!(tracker.get_agent_stats("any").is_none());
    }

    #[test]
    fn test_zero_usage_record() {
        let tracker = CostTracker::new();
        tracker.record_usage("agent-1", &make_usage(0, 0, 0.0));

        let stats = tracker.get_agent_stats("agent-1").unwrap();
        assert_eq!(stats.total_input_tokens, 0);
        assert_eq!(stats.total_output_tokens, 0);
        assert!((stats.estimated_total_cost_usd).abs() < f64::EPSILON);
        assert_eq!(stats.task_count, 1);
    }

    #[test]
    fn test_check_cost_cap_zero_cap() {
        let tracker = CostTracker::new();
        // Any positive cost exceeds a zero cap
        let result = tracker.check_cost_cap("agent-1", 0.001, 0.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_check_cost_cap_zero_cost_zero_cap() {
        let tracker = CostTracker::new();
        // Zero cost does not exceed zero cap
        let result = tracker.check_cost_cap("agent-1", 0.0, 0.0);
        assert!(result.is_ok());
    }
}
