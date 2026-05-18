//! Config encryption module — AES-256-GCM encryption for sensitive configuration values.
//!
//! Encrypted values use the `enc:` prefix for identification, allowing plaintext and
//! encrypted values to coexist during migration. Sensitive fields are detected by
//! convention: field names containing `key`, `token`, `secret`, or `password`.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rand::RngCore;
use thiserror::Error;

/// Prefix for encrypted values.
pub const ENCRYPTED_PREFIX: &str = "enc:";

/// Nonce size for AES-256-GCM (96 bits / 12 bytes).
const NONCE_SIZE: usize = 12;

/// Sensitive field name patterns (lowercase).
const SENSITIVE_PATTERNS: &[&str] = &["key", "token", "secret", "password"];

/// Errors that can occur during config encryption/decryption.
#[derive(Debug, Error, PartialEq, Clone)]
pub enum CryptoError {
    #[error("encryption failed: {0}")]
    EncryptionFailed(String),

    #[error("decryption failed: {0}")]
    DecryptionFailed(String),

    #[error("invalid encrypted value: {0}")]
    InvalidFormat(String),

    #[error("missing decryption key: encrypted values present but no key available")]
    MissingKey,

    #[error("invalid key length: expected 32 bytes, got {0}")]
    InvalidKeyLength(usize),
}

/// AES-256-GCM encryption for sensitive config values.
pub struct ConfigEncryption {
    key: [u8; 32],
}

impl ConfigEncryption {
    /// Create a new `ConfigEncryption` instance from a 32-byte key.
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// Create from a key slice, validating length.
    pub fn from_slice(key_bytes: &[u8]) -> Result<Self, CryptoError> {
        if key_bytes.len() != 32 {
            return Err(CryptoError::InvalidKeyLength(key_bytes.len()));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(key_bytes);
        Ok(Self::new(key))
    }

    /// Load encryption key from a file path.
    pub fn from_key_file(path: &std::path::Path) -> Result<Self, CryptoError> {
        let key_data = std::fs::read(path).map_err(|e| {
            CryptoError::DecryptionFailed(format!("failed to read key file: {e}"))
        })?;

        // Support both raw 32-byte keys and base64-encoded keys
        if key_data.len() == 32 {
            return Self::from_slice(&key_data);
        }

        // Try base64 decode (trim whitespace first)
        let trimmed = String::from_utf8_lossy(&key_data);
        let trimmed = trimmed.trim();
        match BASE64.decode(trimmed.as_bytes()) {
            Ok(decoded) if decoded.len() == 32 => Self::from_slice(&decoded),
            Ok(decoded) => Err(CryptoError::InvalidKeyLength(decoded.len())),
            Err(_) => Err(CryptoError::InvalidKeyLength(key_data.len())),
        }
    }

    /// Encrypt a plaintext value, returning `enc:<base64(nonce+ciphertext)>`.
    pub fn encrypt(&self, plaintext: &str) -> Result<String, CryptoError> {
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;

        // Concatenate nonce + ciphertext and base64 encode
        let mut combined = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);

        Ok(format!("{}{}", ENCRYPTED_PREFIX, BASE64.encode(&combined)))
    }

    /// Decrypt an `enc:...` value back to plaintext.
    pub fn decrypt(&self, encrypted: &str) -> Result<String, CryptoError> {
        let encoded = encrypted
            .strip_prefix(ENCRYPTED_PREFIX)
            .ok_or_else(|| CryptoError::InvalidFormat("missing 'enc:' prefix".to_string()))?;

        let combined = BASE64
            .decode(encoded.as_bytes())
            .map_err(|e| CryptoError::InvalidFormat(format!("invalid base64: {e}")))?;

        if combined.len() < NONCE_SIZE {
            return Err(CryptoError::InvalidFormat(
                "ciphertext too short to contain nonce".to_string(),
            ));
        }

        let (nonce_bytes, ciphertext) = combined.split_at(NONCE_SIZE);
        let nonce = Nonce::from_slice(nonce_bytes);

        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))?;

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| CryptoError::DecryptionFailed(format!("decryption failed: {e}")))?;

        String::from_utf8(plaintext)
            .map_err(|e| CryptoError::DecryptionFailed(format!("invalid UTF-8: {e}")))
    }

    /// Check if a field name is sensitive (contains key, token, secret, password).
    ///
    /// The check is case-insensitive.
    pub fn is_sensitive_field(field_name: &str) -> bool {
        let lower = field_name.to_lowercase();
        SENSITIVE_PATTERNS.iter().any(|p| lower.contains(p))
    }

    /// Check if a value is already encrypted (starts with `enc:`).
    pub fn is_encrypted(value: &str) -> bool {
        value.starts_with(ENCRYPTED_PREFIX)
    }

    /// Encrypt all sensitive fields in a config JSON value in-place.
    ///
    /// Walks the JSON tree recursively. For any string value whose field name
    /// is sensitive and whose value is not already encrypted, encrypts it.
    pub fn encrypt_config(&self, config: &mut serde_json::Value) {
        self.walk_and_encrypt(config, None);
    }

    /// Decrypt all encrypted fields in a config JSON value in-place.
    ///
    /// Walks the JSON tree recursively. For any string value starting with `enc:`,
    /// decrypts it in place. Returns an error if any decryption fails.
    pub fn decrypt_config(&self, config: &mut serde_json::Value) -> Result<(), CryptoError> {
        self.walk_and_decrypt(config)
    }

    /// Recursively walk JSON and encrypt sensitive plaintext fields.
    fn walk_and_encrypt(&self, value: &mut serde_json::Value, field_name: Option<&str>) {
        match value {
            serde_json::Value::Object(map) => {
                let keys: Vec<String> = map.keys().cloned().collect();
                for key in keys {
                    if let Some(v) = map.get_mut(&key) {
                        self.walk_and_encrypt(v, Some(&key));
                    }
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr.iter_mut() {
                    self.walk_and_encrypt(item, None);
                }
            }
            serde_json::Value::String(s) => {
                if let Some(name) = field_name {
                    if Self::is_sensitive_field(name) && !Self::is_encrypted(s) {
                        if let Ok(encrypted) = self.encrypt(s) {
                            *s = encrypted;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Recursively walk JSON and decrypt encrypted values.
    fn walk_and_decrypt(&self, value: &mut serde_json::Value) -> Result<(), CryptoError> {
        match value {
            serde_json::Value::Object(map) => {
                let keys: Vec<String> = map.keys().cloned().collect();
                for key in keys {
                    if let Some(v) = map.get_mut(&key) {
                        self.walk_and_decrypt(v)?;
                    }
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr.iter_mut() {
                    self.walk_and_decrypt(item)?;
                }
            }
            serde_json::Value::String(s) => {
                if Self::is_encrypted(s) {
                    let decrypted = self.decrypt(s)?;
                    *s = decrypted;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Check if a JSON config contains any encrypted values.
pub fn has_encrypted_values(config: &serde_json::Value) -> bool {
    match config {
        serde_json::Value::Object(map) => map.values().any(|v| has_encrypted_values(v)),
        serde_json::Value::Array(arr) => arr.iter().any(|v| has_encrypted_values(v)),
        serde_json::Value::String(s) => ConfigEncryption::is_encrypted(s),
        _ => false,
    }
}

/// Validate that encrypted values can be decrypted at startup.
/// If encrypted values are present but no key is available, returns an error.
pub fn validate_encryption_at_startup(
    config: &serde_json::Value,
    key_file: Option<&std::path::Path>,
) -> Result<Option<ConfigEncryption>, CryptoError> {
    let has_encrypted = has_encrypted_values(config);

    if !has_encrypted {
        // No encrypted values — key is optional
        if let Some(path) = key_file {
            if path.exists() {
                return Ok(Some(ConfigEncryption::from_key_file(path)?));
            }
        }
        return Ok(None);
    }

    // Encrypted values present — key is required
    let path = key_file.ok_or(CryptoError::MissingKey)?;
    if !path.exists() {
        return Err(CryptoError::MissingKey);
    }
    Ok(Some(ConfigEncryption::from_key_file(path)?))
}

/// Encrypt sensitive fields in a config file in-place (CLI command implementation).
pub fn encrypt_config_file(
    config_path: &std::path::Path,
    key_file: &std::path::Path,
) -> Result<u32, CryptoError> {
    let encryptor = ConfigEncryption::from_key_file(key_file)?;

    let raw = std::fs::read_to_string(config_path).map_err(|e| {
        CryptoError::EncryptionFailed(format!("failed to read config file: {e}"))
    })?;

    let mut config: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        CryptoError::EncryptionFailed(format!("failed to parse config file: {e}"))
    })?;

    let count = count_sensitive_plaintext(&config);
    encryptor.encrypt_config(&mut config);

    let output = serde_json::to_string_pretty(&config).map_err(|e| {
        CryptoError::EncryptionFailed(format!("failed to serialize config: {e}"))
    })?;

    std::fs::write(config_path, output).map_err(|e| {
        CryptoError::EncryptionFailed(format!("failed to write config file: {e}"))
    })?;

    Ok(count)
}

/// Count the number of sensitive plaintext fields in a config.
fn count_sensitive_plaintext(value: &serde_json::Value) -> u32 {
    count_sensitive_plaintext_inner(value, None)
}

fn count_sensitive_plaintext_inner(value: &serde_json::Value, field_name: Option<&str>) -> u32 {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| count_sensitive_plaintext_inner(v, Some(k)))
            .sum(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .map(|v| count_sensitive_plaintext_inner(v, None))
            .sum(),
        serde_json::Value::String(s) => {
            if let Some(name) = field_name {
                if ConfigEncryption::is_sensitive_field(name) && !ConfigEncryption::is_encrypted(s) {
                    return 1;
                }
            }
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
            0x1d, 0x1e, 0x1f, 0x20,
        ]
    }

    #[test]
    fn test_encrypt_decrypt_round_trip() {
        let enc = ConfigEncryption::new(test_key());
        let plaintext = "my-secret-api-key-12345";
        let encrypted = enc.encrypt(plaintext).unwrap();
        assert!(encrypted.starts_with(ENCRYPTED_PREFIX));
        let decrypted = enc.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_produces_enc_prefix() {
        let enc = ConfigEncryption::new(test_key());
        let encrypted = enc.encrypt("hello").unwrap();
        assert!(encrypted.starts_with("enc:"));
    }

    #[test]
    fn test_encrypt_different_nonces() {
        let enc = ConfigEncryption::new(test_key());
        let e1 = enc.encrypt("same").unwrap();
        let e2 = enc.encrypt("same").unwrap();
        // Different nonces produce different ciphertexts
        assert_ne!(e1, e2);
        // But both decrypt to the same value
        assert_eq!(enc.decrypt(&e1).unwrap(), "same");
        assert_eq!(enc.decrypt(&e2).unwrap(), "same");
    }

    #[test]
    fn test_decrypt_invalid_prefix() {
        let enc = ConfigEncryption::new(test_key());
        let result = enc.decrypt("not-encrypted");
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_invalid_base64() {
        let enc = ConfigEncryption::new(test_key());
        let result = enc.decrypt("enc:not-valid-base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_wrong_key() {
        let enc1 = ConfigEncryption::new(test_key());
        let mut other_key = test_key();
        other_key[0] = 0xFF;
        let enc2 = ConfigEncryption::new(other_key);

        let encrypted = enc1.encrypt("secret").unwrap();
        let result = enc2.decrypt(&encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_sensitive_field() {
        assert!(ConfigEncryption::is_sensitive_field("api_key"));
        assert!(ConfigEncryption::is_sensitive_field("API_KEY"));
        assert!(ConfigEncryption::is_sensitive_field("bot_token"));
        assert!(ConfigEncryption::is_sensitive_field("BOT_TOKEN"));
        assert!(ConfigEncryption::is_sensitive_field("client_secret"));
        assert!(ConfigEncryption::is_sensitive_field("CLIENT_SECRET"));
        assert!(ConfigEncryption::is_sensitive_field("db_password"));
        assert!(ConfigEncryption::is_sensitive_field("PASSWORD"));
        assert!(ConfigEncryption::is_sensitive_field("signing_secret"));
        assert!(ConfigEncryption::is_sensitive_field("webhook_token"));

        // Non-sensitive fields
        assert!(!ConfigEncryption::is_sensitive_field("name"));
        assert!(!ConfigEncryption::is_sensitive_field("port"));
        assert!(!ConfigEncryption::is_sensitive_field("host"));
        assert!(!ConfigEncryption::is_sensitive_field("model"));
        assert!(!ConfigEncryption::is_sensitive_field("timeout"));
    }

    #[test]
    fn test_is_encrypted() {
        assert!(ConfigEncryption::is_encrypted("enc:abc123"));
        assert!(ConfigEncryption::is_encrypted("enc:"));
        assert!(!ConfigEncryption::is_encrypted("not-encrypted"));
        assert!(!ConfigEncryption::is_encrypted(""));
        assert!(!ConfigEncryption::is_encrypted("ENC:uppercase"));
    }

    #[test]
    fn test_encrypt_config_json() {
        let enc = ConfigEncryption::new(test_key());
        let mut config = serde_json::json!({
            "channels": {
                "telegram": {
                    "bot_token": "123456:ABC-DEF",
                    "chat_id": "12345"
                },
                "slack": {
                    "bot_token": "xoxb-slack-token",
                    "signing_secret": "slack-secret",
                    "channel": "general"
                }
            },
            "gateway": {
                "port": 8080,
                "api_key": "my-api-key"
            }
        });

        enc.encrypt_config(&mut config);

        // Sensitive fields should be encrypted
        let tg_token = config["channels"]["telegram"]["bot_token"].as_str().unwrap();
        assert!(tg_token.starts_with("enc:"));

        let slack_token = config["channels"]["slack"]["bot_token"].as_str().unwrap();
        assert!(slack_token.starts_with("enc:"));

        let slack_secret = config["channels"]["slack"]["signing_secret"].as_str().unwrap();
        assert!(slack_secret.starts_with("enc:"));

        let api_key = config["gateway"]["api_key"].as_str().unwrap();
        assert!(api_key.starts_with("enc:"));

        // Non-sensitive fields should remain unchanged
        assert_eq!(config["channels"]["telegram"]["chat_id"], "12345");
        assert_eq!(config["channels"]["slack"]["channel"], "general");
        assert_eq!(config["gateway"]["port"], 8080);
    }

    #[test]
    fn test_decrypt_config_json() {
        let enc = ConfigEncryption::new(test_key());
        let mut config = serde_json::json!({
            "channels": {
                "telegram": {
                    "bot_token": "123456:ABC-DEF",
                    "chat_id": "12345"
                }
            }
        });

        // Encrypt then decrypt
        enc.encrypt_config(&mut config);
        enc.decrypt_config(&mut config).unwrap();

        assert_eq!(config["channels"]["telegram"]["bot_token"], "123456:ABC-DEF");
        assert_eq!(config["channels"]["telegram"]["chat_id"], "12345");
    }

    #[test]
    fn test_plaintext_and_encrypted_coexist() {
        let enc = ConfigEncryption::new(test_key());
        let encrypted_value = enc.encrypt("already-encrypted").unwrap();

        let mut config = serde_json::json!({
            "api_key": encrypted_value,
            "new_token": "plaintext-token",
            "name": "not-sensitive"
        });

        enc.encrypt_config(&mut config);

        // Already encrypted value should not be double-encrypted
        let api_key = config["api_key"].as_str().unwrap();
        assert_eq!(api_key, &encrypted_value);

        // New plaintext sensitive field should be encrypted
        let new_token = config["new_token"].as_str().unwrap();
        assert!(new_token.starts_with("enc:"));
        assert_ne!(new_token, "plaintext-token");

        // Non-sensitive field unchanged
        assert_eq!(config["name"], "not-sensitive");
    }

    #[test]
    fn test_has_encrypted_values() {
        let config_with = serde_json::json!({
            "api_key": "enc:abc123",
            "name": "test"
        });
        assert!(has_encrypted_values(&config_with));

        let config_without = serde_json::json!({
            "api_key": "plaintext",
            "name": "test"
        });
        assert!(!has_encrypted_values(&config_without));
    }

    #[test]
    fn test_validate_encryption_missing_key() {
        let config = serde_json::json!({
            "api_key": "enc:abc123"
        });
        let result = validate_encryption_at_startup(&config, None);
        assert!(matches!(result, Err(CryptoError::MissingKey)));
    }

    #[test]
    fn test_empty_string_encrypt_decrypt() {
        let enc = ConfigEncryption::new(test_key());
        let encrypted = enc.encrypt("").unwrap();
        assert!(encrypted.starts_with("enc:"));
        let decrypted = enc.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_unicode_encrypt_decrypt() {
        let enc = ConfigEncryption::new(test_key());
        let plaintext = "こんにちは世界 🌍 émojis & spëcial chars";
        let encrypted = enc.encrypt(plaintext).unwrap();
        let decrypted = enc.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_from_slice_invalid_length() {
        let result = ConfigEncryption::from_slice(&[0u8; 16]);
        assert!(matches!(result, Err(CryptoError::InvalidKeyLength(16))));
    }

    #[test]
    fn test_from_slice_valid() {
        let key = [0x42u8; 32];
        let enc = ConfigEncryption::from_slice(&key).unwrap();
        let encrypted = enc.encrypt("test").unwrap();
        let decrypted = enc.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, "test");
    }
}
