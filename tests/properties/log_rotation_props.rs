//! Property-based tests for Log Retention Correctness.
//!
//! Feature: phase-2-complete, Property 12: Log Retention Correctness
//!
//! *For any* set of log files with creation dates and a retention period of D days,
//! the `files_to_delete` function SHALL return exactly those files whose creation date
//! is more than D days before the current time. Files within the retention window SHALL
//! never be returned for deletion.
//!
//! **Validates: Requirements 12.2, 12.3**

use adk_gateway::config::{LogFileInfo, LogRotationConfig, RotationPolicy};
use chrono::{DateTime, Duration, TimeZone, Utc};
use proptest::prelude::*;
use std::path::PathBuf;

// ── Strategies ─────────────────────────────────────────────────────

/// Generate a retention period in days [0, 365].
fn retention_days_strategy() -> impl Strategy<Value = u32> {
    0u32..=365u32
}

/// Generate a "now" timestamp within a reasonable range.
fn now_strategy() -> impl Strategy<Value = DateTime<Utc>> {
    // Generate timestamps between 2020-01-01 and 2030-12-31
    (2020i32..=2030i32, 1u32..=12u32, 1u32..=28u32, 0u32..=23u32, 0u32..=59u32)
        .prop_map(|(year, month, day, hour, minute)| {
            Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
                .unwrap()
        })
}

/// Generate a file age in days relative to "now" [0, 730] (up to 2 years old).
fn file_age_days_strategy() -> impl Strategy<Value = i64> {
    0i64..=730i64
}

/// Generate a set of log files with varying ages.
fn log_files_strategy(
    now: DateTime<Utc>,
) -> impl Strategy<Value = Vec<LogFileInfo>> {
    prop::collection::vec(
        (file_age_days_strategy(), 1u64..=1_000_000u64),
        0..50,
    )
    .prop_map(move |entries| {
        entries
            .into_iter()
            .enumerate()
            .map(|(i, (age_days, size_bytes))| LogFileInfo {
                path: PathBuf::from(format!("/logs/adk-gateway.log.file-{}", i)),
                created_at: now - Duration::days(age_days),
                size_bytes,
            })
            .collect()
    })
}

/// Generate a complete test scenario: retention days, now timestamp, and files.
fn scenario_strategy() -> impl Strategy<Value = (u32, DateTime<Utc>, Vec<LogFileInfo>)> {
    (retention_days_strategy(), now_strategy()).prop_flat_map(|(retention, now)| {
        log_files_strategy(now).prop_map(move |files| (retention, now, files))
    })
}

// ── Property Tests ─────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    // Feature: phase-2-complete, Property 12: Log Retention Correctness
    // **Validates: Requirements 12.2, 12.3**

    /// Property 12a: files_to_delete returns ONLY files older than retention period.
    ///
    /// For any set of log files and retention period D, every file returned by
    /// files_to_delete SHALL have a creation date more than D days before now.
    #[test]
    fn prop_deleted_files_are_all_expired(
        (retention_days, now, files) in scenario_strategy()
    ) {
        let config = LogRotationConfig {
            rotation: RotationPolicy::Daily,
            retention_days,
            max_file_size_mb: 100,
            format: None,
        };

        let to_delete = config.files_to_delete(&files, now);
        let cutoff = now - Duration::days(retention_days as i64);

        for path in &to_delete {
            let file = files.iter().find(|f| &f.path == path).unwrap();
            prop_assert!(
                file.created_at < cutoff,
                "File {:?} with created_at={} was marked for deletion but is within retention window (cutoff={})",
                file.path, file.created_at, cutoff
            );
        }
    }

    /// Property 12b: files_to_delete returns ALL files older than retention period.
    ///
    /// For any set of log files and retention period D, every file whose creation
    /// date is more than D days before now SHALL be included in the result.
    #[test]
    fn prop_all_expired_files_are_deleted(
        (retention_days, now, files) in scenario_strategy()
    ) {
        let config = LogRotationConfig {
            rotation: RotationPolicy::Daily,
            retention_days,
            max_file_size_mb: 100,
            format: None,
        };

        let to_delete = config.files_to_delete(&files, now);
        let cutoff = now - Duration::days(retention_days as i64);

        for file in &files {
            if file.created_at < cutoff {
                prop_assert!(
                    to_delete.contains(&file.path),
                    "File {:?} with created_at={} is expired (cutoff={}) but was NOT marked for deletion",
                    file.path, file.created_at, cutoff
                );
            }
        }
    }

    /// Property 12c: Files within the retention window are NEVER returned for deletion.
    ///
    /// For any set of log files and retention period D, no file whose creation date
    /// is within D days of now SHALL appear in the deletion list.
    #[test]
    fn prop_retained_files_never_deleted(
        (retention_days, now, files) in scenario_strategy()
    ) {
        let config = LogRotationConfig {
            rotation: RotationPolicy::Daily,
            retention_days,
            max_file_size_mb: 100,
            format: None,
        };

        let to_delete = config.files_to_delete(&files, now);
        let cutoff = now - Duration::days(retention_days as i64);

        for file in &files {
            if file.created_at >= cutoff {
                prop_assert!(
                    !to_delete.contains(&file.path),
                    "File {:?} with created_at={} is within retention window (cutoff={}) but was marked for deletion",
                    file.path, file.created_at, cutoff
                );
            }
        }
    }

    /// Property 12d: The result set size equals the count of expired files.
    ///
    /// For any set of log files and retention period D, the number of files
    /// returned by files_to_delete SHALL equal the number of files whose
    /// creation date is more than D days before now.
    #[test]
    fn prop_deletion_count_matches_expired_count(
        (retention_days, now, files) in scenario_strategy()
    ) {
        let config = LogRotationConfig {
            rotation: RotationPolicy::Daily,
            retention_days,
            max_file_size_mb: 100,
            format: None,
        };

        let to_delete = config.files_to_delete(&files, now);
        let cutoff = now - Duration::days(retention_days as i64);

        let expected_count = files.iter().filter(|f| f.created_at < cutoff).count();
        prop_assert_eq!(
            to_delete.len(),
            expected_count,
            "Expected {} files to delete but got {}",
            expected_count,
            to_delete.len()
        );
    }

    /// Property 12e: Retention is independent of file size.
    ///
    /// For any set of log files, the files_to_delete decision is based solely
    /// on creation date and retention period, not on file size.
    #[test]
    fn prop_retention_independent_of_file_size(
        retention_days in retention_days_strategy(),
        now in now_strategy(),
        age_days in file_age_days_strategy(),
        size1 in 1u64..=10_000_000_000u64,
        size2 in 1u64..=10_000_000_000u64,
    ) {
        let config = LogRotationConfig {
            rotation: RotationPolicy::Daily,
            retention_days,
            max_file_size_mb: 100,
            format: None,
        };

        let created_at = now - Duration::days(age_days);

        let files_small = vec![LogFileInfo {
            path: PathBuf::from("/logs/test.log"),
            created_at,
            size_bytes: size1,
        }];

        let files_large = vec![LogFileInfo {
            path: PathBuf::from("/logs/test.log"),
            created_at,
            size_bytes: size2,
        }];

        let result_small = config.files_to_delete(&files_small, now);
        let result_large = config.files_to_delete(&files_large, now);

        prop_assert_eq!(
            result_small.len(),
            result_large.len(),
            "Deletion decision should be independent of file size"
        );
    }

    /// Property 12f: Increasing retention period never increases deletions.
    ///
    /// For any set of log files, if retention period D2 > D1, then
    /// files_to_delete(D2) ⊆ files_to_delete(D1).
    #[test]
    fn prop_longer_retention_fewer_deletions(
        now in now_strategy(),
        ages_and_sizes in prop::collection::vec(
            (file_age_days_strategy(), 1u64..=1_000_000u64),
            0..30
        ),
        d1 in 0u32..=180u32,
        d2_extra in 1u32..=180u32,
    ) {
        let d2 = d1.saturating_add(d2_extra);

        let files: Vec<LogFileInfo> = ages_and_sizes.into_iter().enumerate().map(|(i, (age, size))| LogFileInfo {
            path: PathBuf::from(format!("/logs/file-{}", i)),
            created_at: now - Duration::days(age),
            size_bytes: size,
        }).collect();

        let config_short = LogRotationConfig {
            rotation: RotationPolicy::Daily,
            retention_days: d1,
            max_file_size_mb: 100,
            format: None,
        };

        let config_long = LogRotationConfig {
            rotation: RotationPolicy::Daily,
            retention_days: d2,
            max_file_size_mb: 100,
            format: None,
        };

        let delete_short = config_short.files_to_delete(&files, now);
        let delete_long = config_long.files_to_delete(&files, now);

        // Every file deleted with longer retention should also be deleted with shorter retention
        for path in &delete_long {
            prop_assert!(
                delete_short.contains(path),
                "File {:?} deleted with retention={} but not with retention={}",
                path, d2, d1
            );
        }

        // Longer retention should delete fewer or equal files
        prop_assert!(
            delete_long.len() <= delete_short.len(),
            "Longer retention ({}) deleted {} files but shorter retention ({}) deleted {} files",
            d2, delete_long.len(), d1, delete_short.len()
        );
    }
}
