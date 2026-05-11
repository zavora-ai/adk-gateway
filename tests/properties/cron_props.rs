//! Property-based tests for cron scheduler.
//!
//! Feature: gateway-production-maturity
//! - Property 20: Cron job scheduling from config
//!   **Validates: Requirements R10.1, R10.5**

use adk_gateway::config::{CronDelivery, CronJob};
use adk_gateway::cron::{CronMessageKind, CronScheduler};
use proptest::prelude::*;
use std::collections::HashSet;
use tokio::sync::mpsc;

/// Strategy for generating a valid cron job.
fn cron_job_strategy() -> impl Strategy<Value = CronJob> {
    (
        "[a-z][a-z0-9\\-]{2,15}",      // id
        "@every [1-9][0-9]{0,2}[smh]", // schedule
        "[a-zA-Z0-9 ]{1,50}",          // message
        any::<bool>(),                 // has delivery target
    )
        .prop_map(|(id, schedule, message, has_delivery)| {
            let deliver_to = if has_delivery {
                Some(CronDelivery {
                    channel: "telegram".to_string(),
                    target: "user123".to_string(),
                })
            } else {
                None
            };
            CronJob {
                id,
                schedule,
                message,
                deliver_to,
            }
        })
}

/// Strategy for generating a set of cron jobs with unique IDs.
fn cron_jobs_strategy(max_len: usize) -> impl Strategy<Value = Vec<CronJob>> {
    prop::collection::vec(cron_job_strategy(), 0..=max_len).prop_map(|jobs| {
        // Deduplicate by ID, keeping the first occurrence
        let mut seen = HashSet::new();
        jobs.into_iter()
            .filter(|j| seen.insert(j.id.clone()))
            .collect()
    })
}

// ── Property 20: Cron job scheduling from config ───────────────────
// **Validates: Requirements 10.1, 10.5**
//
// For any set of cron job configurations, the CronScheduler should have
// exactly one active job per config entry. On hot-reload (reconcile),
// removed jobs should be cancelled and new jobs should be scheduled,
// with the final active set matching the new config.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(60))]

    #[test]
    fn cron_scheduling_matches_config(
        jobs in cron_jobs_strategy(8)
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, _rx) = mpsc::channel(64);
            let mut scheduler = CronScheduler::new(tx);

            // Schedule all jobs
            scheduler.reconcile(&jobs);

            // Active job IDs should match config job IDs exactly
            let active: HashSet<String> = scheduler.active_job_ids().into_iter().collect();
            let expected: HashSet<String> = jobs.iter().map(|j| j.id.clone()).collect();

            prop_assert_eq!(
                &active, &expected,
                "active jobs should match config: active={:?}, expected={:?}",
                active, expected
            );

            // Each job should be individually active
            for job in &jobs {
                prop_assert!(
                    scheduler.is_active(&job.id),
                    "job {} should be active",
                    job.id
                );
            }

            Ok(())
        })?;
    }

    #[test]
    fn cron_reconcile_updates_job_set(
        initial_jobs in cron_jobs_strategy(6),
        new_jobs in cron_jobs_strategy(6)
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, _rx) = mpsc::channel(64);
            let mut scheduler = CronScheduler::new(tx);

            // Initial scheduling
            scheduler.reconcile(&initial_jobs);

            // Hot-reload with new set
            scheduler.reconcile(&new_jobs);

            // After reconcile, active set should match new_jobs exactly
            let active: HashSet<String> = scheduler.active_job_ids().into_iter().collect();
            let expected: HashSet<String> = new_jobs.iter().map(|j| j.id.clone()).collect();

            prop_assert_eq!(
                &active, &expected,
                "after reconcile, active jobs should match new config"
            );

            // Jobs removed from config should not be active
            let initial_ids: HashSet<String> = initial_jobs.iter().map(|j| j.id.clone()).collect();
            for id in &initial_ids {
                if !expected.contains(id) {
                    prop_assert!(
                        !scheduler.is_active(id),
                        "removed job {} should not be active after reconcile",
                        id
                    );
                }
            }

            Ok(())
        })?;
    }

    #[test]
    fn cron_parse_message_ask_prefix(
        prompt in "[a-zA-Z0-9 ]{1,50}"
    ) {
        let message = format!("ask:{}", prompt);
        match CronScheduler::parse_message(&message) {
            CronMessageKind::AgentPrompt(p) => {
                prop_assert_eq!(p, prompt.trim());
            }
            CronMessageKind::DirectMessage(_) => {
                prop_assert!(false, "ask: prefix should produce AgentPrompt");
            }
        }
    }

    #[test]
    fn cron_parse_message_no_prefix(
        message in "[a-zA-Z][a-zA-Z0-9 ]{1,50}"
    ) {
        prop_assume!(!message.starts_with("ask:"));
        match CronScheduler::parse_message(&message) {
            CronMessageKind::DirectMessage(m) => {
                prop_assert_eq!(m, message.as_str());
            }
            CronMessageKind::AgentPrompt(_) => {
                prop_assert!(false, "non-ask: message should produce DirectMessage");
            }
        }
    }
}
