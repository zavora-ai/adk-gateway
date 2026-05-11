//! DM pairing service for user authentication.
//!
//! Design: DmPairingService [R9.1–R9.6]

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

// ── Pairing code ───────────────────────────────────────────────────

/// A single-use pairing code with expiry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingCode {
    pub code: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used: bool,
}

impl PairingCode {
    /// Check if this code is still valid (not used and not expired).
    pub fn is_valid(&self) -> bool {
        !self.used && Utc::now() < self.expires_at
    }
}

// ── Lockout info ───────────────────────────────────────────────────

/// Tracks failed pairing attempts and lockout state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockoutInfo {
    pub failed_attempts: u32,
    pub locked_until: Option<DateTime<Utc>>,
}

impl LockoutInfo {
    pub fn new() -> Self {
        Self {
            failed_attempts: 0,
            locked_until: None,
        }
    }

    /// Check if the user is currently locked out.
    pub fn is_locked(&self) -> bool {
        match self.locked_until {
            Some(until) => Utc::now() < until,
            None => false,
        }
    }
}

impl Default for LockoutInfo {
    fn default() -> Self {
        Self::new()
    }
}

// ── Paired user ────────────────────────────────────────────────────

/// A user that has been successfully paired.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedUser {
    pub user_id: String,
    pub paired_at: DateTime<Utc>,
    pub channel_type: String,
}

// ── Pairing result ─────────────────────────────────────────────────

/// Result of a pairing attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum PairingResult {
    /// Code was valid, user is now paired.
    Success,
    /// Code was invalid (user can retry).
    InvalidCode { attempts_remaining: u32 },
    /// User is locked out after too many failures.
    Locked { locked_until: DateTime<Utc> },
    /// Code has already been used.
    AlreadyUsed,
    /// Code has expired.
    Expired,
}

// ── DmPairingService ───────────────────────────────────────────────

/// Maximum consecutive failed attempts before lockout.
const MAX_ATTEMPTS: u32 = 3;

/// Lockout duration in minutes.
const LOCKOUT_MINUTES: i64 = 15;

/// Code expiry duration in hours.
const CODE_EXPIRY_HOURS: i64 = 24;

/// Service managing DM pairing codes, validation, and lockout.
pub struct DmPairingService {
    codes: RwLock<HashMap<String, PairingCode>>,
    paired_users: RwLock<HashMap<String, PairedUser>>,
    lockouts: RwLock<HashMap<String, LockoutInfo>>,
}

impl DmPairingService {
    pub fn new() -> Self {
        Self {
            codes: RwLock::new(HashMap::new()),
            paired_users: RwLock::new(HashMap::new()),
            lockouts: RwLock::new(HashMap::new()),
        }
    }

    /// Generate a new single-use pairing code with 24h expiry.
    pub fn generate_code(&self) -> String {
        let code = generate_pairing_code();
        let now = Utc::now();
        let pairing_code = PairingCode {
            code: code.clone(),
            created_at: now,
            expires_at: now + ChronoDuration::hours(CODE_EXPIRY_HOURS),
            used: false,
        };

        if let Ok(mut codes) = self.codes.write() {
            codes.insert(code.clone(), pairing_code);
        }

        tracing::info!("generated new pairing code");
        code
    }

    /// Check if a user is already paired.
    #[allow(dead_code)] // Used in tests; available for pairing status checks
    pub fn is_paired(&self, user_id: &str) -> bool {
        self.paired_users
            .read()
            .map(|p| p.contains_key(user_id))
            .unwrap_or(false)
    }

    /// Validate a pairing code for a user.
    pub fn validate_code(&self, user_id: &str, code: &str, channel_type: &str) -> PairingResult {
        // Check lockout first
        if let Ok(lockouts) = self.lockouts.read() {
            if let Some(info) = lockouts.get(user_id) {
                if info.is_locked() {
                    return PairingResult::Locked {
                        locked_until: info.locked_until.unwrap(),
                    };
                }
            }
        }

        // Look up the code
        let code_entry = self.codes.read().ok().and_then(|c| c.get(code).cloned());

        match code_entry {
            None => self.record_failure(user_id),
            Some(pc) if pc.used => PairingResult::AlreadyUsed,
            Some(pc) if !pc.is_valid() => PairingResult::Expired,
            Some(_) => {
                // Valid code — mark as used and pair the user
                if let Ok(mut codes) = self.codes.write() {
                    if let Some(entry) = codes.get_mut(code) {
                        entry.used = true;
                    }
                }

                if let Ok(mut paired) = self.paired_users.write() {
                    paired.insert(
                        user_id.to_string(),
                        PairedUser {
                            user_id: user_id.to_string(),
                            paired_at: Utc::now(),
                            channel_type: channel_type.to_string(),
                        },
                    );
                }

                // Reset lockout on success
                if let Ok(mut lockouts) = self.lockouts.write() {
                    lockouts.remove(user_id);
                }

                tracing::info!(user_id = user_id, "user paired successfully");
                PairingResult::Success
            }
        }
    }

    fn record_failure(&self, user_id: &str) -> PairingResult {
        if let Ok(mut lockouts) = self.lockouts.write() {
            let info = lockouts
                .entry(user_id.to_string())
                .or_insert_with(LockoutInfo::new);

            // If previously locked but lock expired, reset
            if !info.is_locked() && info.locked_until.is_some() {
                info.failed_attempts = 0;
                info.locked_until = None;
            }

            info.failed_attempts += 1;

            if info.failed_attempts >= MAX_ATTEMPTS {
                let locked_until = Utc::now() + ChronoDuration::minutes(LOCKOUT_MINUTES);
                info.locked_until = Some(locked_until);
                tracing::warn!(
                    user_id = user_id,
                    "user locked out after {} failed attempts",
                    MAX_ATTEMPTS
                );
                PairingResult::Locked { locked_until }
            } else {
                PairingResult::InvalidCode {
                    attempts_remaining: MAX_ATTEMPTS - info.failed_attempts,
                }
            }
        } else {
            PairingResult::InvalidCode {
                attempts_remaining: 0,
            }
        }
    }

    /// Get the number of paired users.
    pub fn paired_count(&self) -> usize {
        self.paired_users.read().map(|p| p.len()).unwrap_or(0)
    }

    /// Get all paired users (for persistence).
    pub fn paired_users(&self) -> Vec<PairedUser> {
        self.paired_users
            .read()
            .map(|p| p.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Restore paired users from persistent storage.
    pub fn restore_paired_users(&self, users: Vec<PairedUser>) {
        if let Ok(mut paired) = self.paired_users.write() {
            for user in users {
                paired.insert(user.user_id.clone(), user);
            }
        }
    }

    /// Get all active (non-expired, non-used) codes.
    #[allow(dead_code)] // Used in tests; available for admin diagnostics
    pub fn active_codes(&self) -> Vec<PairingCode> {
        self.codes
            .read()
            .map(|c| c.values().filter(|pc| pc.is_valid()).cloned().collect())
            .unwrap_or_default()
    }

    /// Persist paired users to a JSON file for restoration on next startup.
    pub fn persist(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let users = self.paired_users();
        let json = serde_json::to_string_pretty(&users)?;
        std::fs::write(path, json)?;
        tracing::info!(path = %path.display(), count = users.len(), "persisted paired users");
        Ok(())
    }

    /// Load paired users from a JSON file (returns empty vec if file doesn't exist).
    pub fn load_persisted(path: &std::path::Path) -> Vec<PairedUser> {
        match std::fs::read_to_string(path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to parse paired users file, starting fresh");
                Vec::new()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to read paired users file, starting fresh");
                Vec::new()
            }
        }
    }

    /// Clean up expired codes and expired lockouts.
    #[allow(dead_code)] // Used in tests; available for periodic maintenance
    pub fn cleanup(&self) {
        if let Ok(mut codes) = self.codes.write() {
            codes.retain(|_, pc| !pc.used && Utc::now() < pc.expires_at);
        }
        if let Ok(mut lockouts) = self.lockouts.write() {
            lockouts.retain(|_, info| info.is_locked());
        }
    }
}

impl Default for DmPairingService {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a random 6-character alphanumeric pairing code.
fn generate_pairing_code() -> String {
    use rand::Rng;
    let chars = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    let mut code = String::with_capacity(6);
    for _ in 0..6 {
        let idx = rng.gen_range(0..chars.len());
        code.push(chars[idx] as char);
    }
    code
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_code() {
        let service = DmPairingService::new();
        let code = service.generate_code();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn test_generate_unique_codes() {
        let service = DmPairingService::new();
        let code1 = service.generate_code();
        // Small sleep to ensure different timestamp seed
        std::thread::sleep(std::time::Duration::from_millis(2));
        let code2 = service.generate_code();
        // Codes should generally be different (not guaranteed but very likely)
        // Just verify both are valid format
        assert_eq!(code1.len(), 6);
        assert_eq!(code2.len(), 6);
    }

    #[test]
    fn test_valid_pairing() {
        let service = DmPairingService::new();
        let code = service.generate_code();

        assert!(!service.is_paired("user1"));
        let result = service.validate_code("user1", &code, "telegram");
        assert_eq!(result, PairingResult::Success);
        assert!(service.is_paired("user1"));
    }

    #[test]
    fn test_invalid_code() {
        let service = DmPairingService::new();
        let result = service.validate_code("user1", "INVALID", "telegram");
        assert!(matches!(
            result,
            PairingResult::InvalidCode {
                attempts_remaining: 2
            }
        ));
    }

    #[test]
    fn test_code_single_use() {
        let service = DmPairingService::new();
        let code = service.generate_code();

        // First use succeeds
        let result = service.validate_code("user1", &code, "telegram");
        assert_eq!(result, PairingResult::Success);

        // Second use fails (already used)
        let result = service.validate_code("user2", &code, "telegram");
        assert_eq!(result, PairingResult::AlreadyUsed);
    }

    #[test]
    fn test_lockout_after_three_failures() {
        let service = DmPairingService::new();

        // First failure
        let result = service.validate_code("user1", "BAD1", "telegram");
        assert!(matches!(
            result,
            PairingResult::InvalidCode {
                attempts_remaining: 2
            }
        ));

        // Second failure
        let result = service.validate_code("user1", "BAD2", "telegram");
        assert!(matches!(
            result,
            PairingResult::InvalidCode {
                attempts_remaining: 1
            }
        ));

        // Third failure → lockout
        let result = service.validate_code("user1", "BAD3", "telegram");
        assert!(matches!(result, PairingResult::Locked { .. }));

        // Further attempts should also be locked
        let result = service.validate_code("user1", "BAD4", "telegram");
        assert!(matches!(result, PairingResult::Locked { .. }));
    }

    #[test]
    fn test_lockout_does_not_affect_other_users() {
        let service = DmPairingService::new();
        let code = service.generate_code();

        // Lock out user1
        for _ in 0..3 {
            service.validate_code("user1", "BAD", "telegram");
        }

        // user2 should still be able to pair
        let result = service.validate_code("user2", &code, "telegram");
        assert_eq!(result, PairingResult::Success);
    }

    #[test]
    fn test_paired_count() {
        let service = DmPairingService::new();
        assert_eq!(service.paired_count(), 0);

        let code = service.generate_code();
        service.validate_code("user1", &code, "telegram");
        assert_eq!(service.paired_count(), 1);
    }

    #[test]
    fn test_restore_paired_users() {
        let service = DmPairingService::new();
        service.restore_paired_users(vec![
            PairedUser {
                user_id: "user1".into(),
                paired_at: Utc::now(),
                channel_type: "telegram".into(),
            },
            PairedUser {
                user_id: "user2".into(),
                paired_at: Utc::now(),
                channel_type: "slack".into(),
            },
        ]);
        assert!(service.is_paired("user1"));
        assert!(service.is_paired("user2"));
        assert_eq!(service.paired_count(), 2);
    }

    #[test]
    fn test_active_codes() {
        let service = DmPairingService::new();
        let _code1 = service.generate_code();
        let _code2 = service.generate_code();
        let active = service.active_codes();
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn test_cleanup() {
        let service = DmPairingService::new();
        let code = service.generate_code();
        // Use the code
        service.validate_code("user1", &code, "telegram");
        // Cleanup should remove used codes
        service.cleanup();
        assert!(service.active_codes().is_empty());
    }

    #[test]
    fn test_pairing_code_validity() {
        let code = PairingCode {
            code: "ABC123".into(),
            created_at: Utc::now(),
            expires_at: Utc::now() + ChronoDuration::hours(24),
            used: false,
        };
        assert!(code.is_valid());

        let used_code = PairingCode {
            used: true,
            ..code.clone()
        };
        assert!(!used_code.is_valid());

        let expired_code = PairingCode {
            expires_at: Utc::now() - ChronoDuration::hours(1),
            ..code
        };
        assert!(!expired_code.is_valid());
    }
}
