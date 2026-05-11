//! Structured logging configuration and secret redaction.
//!
//! Design: TelemetryExporter, Security Architecture [R14.1, R14.3]

use regex::Regex;
use std::sync::OnceLock;

// ── Secret patterns ────────────────────────────────────────────────

/// Known secret patterns to redact from log output.
#[allow(dead_code)] // Used by secret_patterns() and SecretRedactor; initialized on first use
static SECRET_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

#[allow(dead_code)] // Used by SecretRedactor::redact; lazily initializes SECRET_PATTERNS
fn secret_patterns() -> &'static Vec<Regex> {
    SECRET_PATTERNS.get_or_init(|| {
        vec![
            // Bot tokens (Telegram-style: digits:alphanumeric)
            Regex::new(r"\b\d{8,12}:[A-Za-z0-9_-]{30,50}\b").unwrap(),
            // Bearer tokens in headers
            Regex::new(r"(?i)bearer\s+[A-Za-z0-9._~+/=-]{20,}").unwrap(),
            // Generic API keys (long hex or base64 strings)
            Regex::new(r#"(?i)(api[_-]?key|token|secret|password)\s*[=:]\s*['"]?[A-Za-z0-9._~+/=-]{16,}['"]?"#).unwrap(),
            // Slack tokens (xoxb-, xoxp-, xapp-)
            Regex::new(r"xox[bpa]-[A-Za-z0-9-]{10,}").unwrap(),
            // JWT tokens (three base64 segments separated by dots)
            Regex::new(r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+").unwrap(),
        ]
    })
}

// ── SecretRedactor ─────────────────────────────────────────────────

/// Redacts known secret patterns from text.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Used in tests; available for log output redaction
pub struct SecretRedactor {
    replacement: String,
}

impl SecretRedactor {
    #[allow(dead_code)] // Used in tests; constructs default redactor
    pub fn new() -> Self {
        Self {
            replacement: "***REDACTED***".to_string(),
        }
    }

    #[allow(dead_code)] // Used in tests; constructs redactor with custom replacement
    pub fn with_replacement(replacement: &str) -> Self {
        Self {
            replacement: replacement.to_string(),
        }
    }

    /// Redact all known secret patterns from the input string.
    #[allow(dead_code)] // Used in tests; available for log output redaction
    pub fn redact(&self, input: &str) -> String {
        let mut result = input.to_string();
        for pattern in secret_patterns() {
            result = pattern
                .replace_all(&result, self.replacement.as_str())
                .to_string();
        }
        result
    }
}

impl Default for SecretRedactor {
    fn default() -> Self {
        Self::new()
    }
}

// ── Config redaction ───────────────────────────────────────────────

/// Fields that should be redacted when displaying config.
const SENSITIVE_FIELDS: &[&str] = &[
    "bot_token",
    "botToken",
    "app_token",
    "appToken",
    "token",
    "password",
    "secret",
    "api_key",
    "apiKey",
    "connection_string",
    "connectionString",
];

/// Redact sensitive values in a JSON value tree.
/// Replaces values of known sensitive keys with `"***"`.
pub fn redact_config(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut redacted = serde_json::Map::new();
            for (key, val) in map {
                if SENSITIVE_FIELDS.iter().any(|&f| key == f) {
                    // Only redact if the value is a non-null string
                    match val {
                        serde_json::Value::String(s) if !s.is_empty() => {
                            redacted.insert(key.clone(), serde_json::Value::String("***".into()));
                        }
                        _ => {
                            redacted.insert(key.clone(), redact_config(val));
                        }
                    }
                } else {
                    redacted.insert(key.clone(), redact_config(val));
                }
            }
            serde_json::Value::Object(redacted)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(redact_config).collect())
        }
        other => other.clone(),
    }
}

// ── Structured logging setup ───────────────────────────────────────

/// Configuration for the telemetry/logging subsystem.
#[derive(Debug, Clone)]
pub struct TelemetrySetup {
    pub json_format: bool,
    pub otel_endpoint: Option<String>,
}

impl TelemetrySetup {
    pub fn from_config(config: &crate::config::TelemetryConfig) -> Self {
        Self {
            json_format: matches!(config.log_format, crate::config::LogFormat::Json),
            otel_endpoint: config.otel_endpoint.clone(),
        }
    }

    /// Initialize the tracing subscriber based on the telemetry configuration.
    ///
    /// Configures JSON or text format logging based on `json_format`, and
    /// applies the given `EnvFilter` for log level control.
    pub fn init(&self, filter: tracing_subscriber::EnvFilter) {
        use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

        if self.json_format {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json().with_target(true))
                .init();
        } else {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().with_target(true))
                .init();
        }
    }

    /// Describe the telemetry configuration for display.
    pub fn describe(&self) -> String {
        let format = if self.json_format { "JSON" } else { "text" };
        let otel = match &self.otel_endpoint {
            Some(ep) => format!("enabled ({})", ep),
            None => "disabled".to_string(),
        };
        format!("log_format={format}, otel={otel}")
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_redact_no_secrets() {
        let redactor = SecretRedactor::new();
        let input = "this is a normal log message with no secrets";
        let result = redactor.redact(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_redact_custom_replacement() {
        let redactor = SecretRedactor::with_replacement("[HIDDEN]");
        let input = "token: xoxb-test-placeholder";
        let result = redactor.redact(input);
        assert!(result.contains("[HIDDEN]"));
    }

    #[test]
    fn test_redact_config_simple() {
        let config = json!({
            "gateway": {
                "port": 18789,
                "auth": {
                    "token": "my-secret-token",
                    "password": "my-password"
                }
            },
            "channels": {
                "telegram": {
                    "botToken": "123:ABC",
                    "enabled": true
                }
            }
        });

        let redacted = redact_config(&config);
        assert_eq!(redacted["gateway"]["auth"]["token"], "***");
        assert_eq!(redacted["gateway"]["auth"]["password"], "***");
        assert_eq!(redacted["channels"]["telegram"]["botToken"], "***");
        assert_eq!(redacted["gateway"]["port"], 18789);
        assert_eq!(redacted["channels"]["telegram"]["enabled"], true);
    }

    #[test]
    fn test_redact_config_nested_array() {
        let config = json!({
            "items": [
                {"token": "secret1", "name": "a"},
                {"token": "secret2", "name": "b"}
            ]
        });

        let redacted = redact_config(&config);
        assert_eq!(redacted["items"][0]["token"], "***");
        assert_eq!(redacted["items"][1]["token"], "***");
        assert_eq!(redacted["items"][0]["name"], "a");
    }

    #[test]
    fn test_redact_config_empty_token() {
        let config = json!({
            "token": "",
            "password": null
        });

        let redacted = redact_config(&config);
        // Empty string should not be redacted
        assert_eq!(redacted["token"], "");
        // Null should not be redacted
        assert!(redacted["password"].is_null());
    }

    #[test]
    fn test_telemetry_setup_describe() {
        let setup = TelemetrySetup {
            json_format: true,
            otel_endpoint: Some("http://localhost:4317".into()),
        };
        let desc = setup.describe();
        assert!(desc.contains("JSON"));
        assert!(desc.contains("localhost:4317"));
    }

    #[test]
    fn test_telemetry_setup_no_otel() {
        let setup = TelemetrySetup {
            json_format: false,
            otel_endpoint: None,
        };
        let desc = setup.describe();
        assert!(desc.contains("text"));
        assert!(desc.contains("disabled"));
    }
}
