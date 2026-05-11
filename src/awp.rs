//! AWP (Agentic Web Protocol) integration for adk-gateway.
//!
//! Uses `AwpState::builder()` from adk-awp for clean state construction,
//! `FileConsentService` for durable consent records, and `awp_routes()`
//! for the standard 7 AWP endpoints. Adds gateway-specific consent HTTP
//! endpoints and health reporting tied to LLM availability.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use adk_awp::{AwpState, BusinessContextLoader, FileConsentService};

// Re-export so gateway_state.rs can reference the type.
pub use adk_awp::AwpState as AwpGatewayState;

// ── Configuration ──────────────────────────────────────────────────

/// Configuration for AWP protocol support within the gateway.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct AwpConfig {
    /// Enable AWP protocol endpoints. Default: false (opt-in).
    pub enabled: bool,
    /// Path to business.toml relative to the gateway config file directory.
    pub business_toml: PathBuf,
    /// Whether to watch business.toml for hot-reload changes.
    pub hot_reload: bool,
    /// Path to consent storage file. Default: "data/consent.json".
    pub consent_file: PathBuf,
}

impl Default for AwpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            business_toml: PathBuf::from("business.toml"),
            hot_reload: true,
            consent_file: PathBuf::from("data/consent.json"),
        }
    }
}

// ── Build ──────────────────────────────────────────────────────────

/// Build the AWP shared state from configuration.
///
/// Returns `None` if AWP is disabled or business.toml is not found.
pub async fn build_awp_state(
    awp_config: &AwpConfig,
    config_dir: &Path,
) -> anyhow::Result<Option<AwpState>> {
    if !awp_config.enabled {
        tracing::info!("AWP protocol disabled");
        return Ok(None);
    }

    let toml_path = if awp_config.business_toml.is_relative() {
        config_dir.join(&awp_config.business_toml)
    } else {
        awp_config.business_toml.clone()
    };

    if !toml_path.exists() {
        tracing::warn!(
            path = %toml_path.display(),
            "business.toml not found — AWP endpoints will not be available"
        );
        return Ok(None);
    }

    let loader = BusinessContextLoader::from_file(&toml_path)
        .map_err(|e| anyhow::anyhow!("failed to load business.toml: {e}"))?;

    tracing::info!(
        path = %toml_path.display(),
        site = %loader.load().site_name,
        capabilities = loader.load().capabilities.len(),
        "AWP business context loaded"
    );

    if awp_config.hot_reload {
        loader
            .watch(toml_path.clone())
            .await
            .map_err(|e| anyhow::anyhow!("failed to start business.toml watcher: {e}"))?;
        tracing::info!("AWP business.toml hot-reload enabled");
    }

    // Use FileConsentService for durable consent records (GDPR/KPA compliance)
    let consent_path = if awp_config.consent_file.is_relative() {
        config_dir.join(&awp_config.consent_file)
    } else {
        awp_config.consent_file.clone()
    };

    let consent_service: Arc<dyn adk_awp::ConsentService> =
        match FileConsentService::new(&consent_path) {
            Ok(svc) => {
                tracing::info!(path = %consent_path.display(), "AWP consent storage initialized");
                Arc::new(svc)
            }
            Err(e) => {
                tracing::warn!(
                    path = %consent_path.display(),
                    error = %e,
                    "failed to initialize file consent service, falling back to in-memory"
                );
                Arc::new(adk_awp::InMemoryConsentService::new())
            }
        };

    let state = AwpState::builder(loader.context_ref())
        .consent_service(consent_service)
        .build();

    tracing::info!("AWP protocol state initialized");
    Ok(Some(state))
}

// ── Route merging ──────────────────────────────────────────────────

/// Merge AWP protocol routes into an existing axum router.
///
/// Adds the 7 standard AWP endpoints (discovery, manifest, health, a2a,
/// events) plus gateway-specific consent endpoints. Version negotiation
/// middleware is applied automatically by `awp_routes()`.
pub fn merge_awp_routes(router: axum::Router, awp_state: Option<AwpState>) -> axum::Router {
    let Some(state) = awp_state else {
        return router;
    };

    // Standard AWP endpoints from adk-awp (with version negotiation middleware)
    let awp_router = adk_awp::awp_routes(state.clone());

    // Gateway-specific consent endpoints
    let consent_routes = axum::Router::new()
        .route("/awp/consent", axum::routing::post(handle_consent_capture))
        .route(
            "/awp/consent/check",
            axum::routing::get(handle_consent_check),
        )
        .route(
            "/awp/consent/revoke",
            axum::routing::post(handle_consent_revoke),
        )
        .with_state(state);

    tracing::info!(
        "AWP endpoints registered: /.well-known/awp.json, /awp/manifest, \
         /awp/health, /awp/a2a, /awp/events/*, /awp/consent/*"
    );

    router.merge(awp_router).merge(consent_routes)
}

// ── Health reporting (called from gateway message processing) ──────

/// Report that the LLM backend is degrading (e.g. high latency).
pub async fn report_degrading(state: &AwpState, reason: &str) {
    if let Err(e) = state.health.report_degrading(reason).await {
        tracing::debug!(error = %e, "health transition to degrading rejected");
    }
}

/// Report that the LLM backend has recovered.
pub async fn report_healthy(state: &AwpState) {
    if let Err(e) = state.health.report_healthy().await {
        tracing::debug!(error = %e, "health transition to healthy rejected");
    }
}

// ── Consent handlers ───────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct ConsentBody {
    subject: String,
    purpose: String,
}

async fn handle_consent_capture(
    axum::extract::State(state): axum::extract::State<AwpState>,
    axum::Json(body): axum::Json<ConsentBody>,
) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    if body.subject.is_empty() || body.purpose.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "error": "subject and purpose are required" })),
        );
    }
    match state
        .consent_service
        .capture_consent(&body.subject, &body.purpose)
        .await
    {
        Ok(()) => (
            axum::http::StatusCode::CREATED,
            axum::Json(serde_json::json!({
                "status": "captured",
                "subject": body.subject,
                "purpose": body.purpose,
            })),
        ),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

#[derive(serde::Deserialize)]
struct ConsentCheckQuery {
    subject: String,
    purpose: String,
}

async fn handle_consent_check(
    axum::extract::State(state): axum::extract::State<AwpState>,
    axum::extract::Query(params): axum::extract::Query<ConsentCheckQuery>,
) -> axum::Json<serde_json::Value> {
    let consented = state
        .consent_service
        .check_consent(&params.subject, &params.purpose)
        .await
        .unwrap_or(false);
    axum::Json(serde_json::json!({
        "subject": params.subject,
        "purpose": params.purpose,
        "consented": consented,
    }))
}

async fn handle_consent_revoke(
    axum::extract::State(state): axum::extract::State<AwpState>,
    axum::Json(body): axum::Json<ConsentBody>,
) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    match state
        .consent_service
        .revoke_consent(&body.subject, &body.purpose)
        .await
    {
        Ok(()) => (
            axum::http::StatusCode::OK,
            axum::Json(serde_json::json!({
                "status": "revoked",
                "subject": body.subject,
                "purpose": body.purpose,
            })),
        ),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TOML: &str = r#"
site_name = "Test Gateway"
site_description = "Test"
domain = "localhost"

[[capabilities]]
name = "health"
description = "Health check"
endpoint = "/health"
method = "GET"
access_level = "anonymous"

[[policies]]
name = "privacy"
description = "Privacy policy"
policy_type = "privacy"
"#;

    #[test]
    fn test_default_config() {
        let cfg = AwpConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.business_toml, PathBuf::from("business.toml"));
        assert!(cfg.hot_reload);
        assert_eq!(cfg.consent_file, PathBuf::from("data/consent.json"));
    }

    #[test]
    fn test_config_serde_round_trip() {
        let cfg = AwpConfig {
            enabled: true,
            business_toml: PathBuf::from("custom/path.toml"),
            hot_reload: false,
            consent_file: PathBuf::from("custom/consent.json"),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let deserialized: AwpConfig = serde_json::from_str(&json).unwrap();
        assert!(deserialized.enabled);
        assert_eq!(
            deserialized.business_toml,
            PathBuf::from("custom/path.toml")
        );
        assert!(!deserialized.hot_reload);
        assert_eq!(
            deserialized.consent_file,
            PathBuf::from("custom/consent.json")
        );
    }

    #[tokio::test]
    async fn test_build_disabled() {
        let cfg = AwpConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(build_awp_state(&cfg, Path::new("."))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_build_missing_toml() {
        let cfg = AwpConfig {
            enabled: true,
            business_toml: PathBuf::from("nonexistent.toml"),
            ..Default::default()
        };
        assert!(build_awp_state(&cfg, Path::new("."))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_build_valid_with_file_consent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("business.toml"), SAMPLE_TOML).unwrap();

        let cfg = AwpConfig {
            enabled: true,
            hot_reload: false,
            consent_file: PathBuf::from("consent.json"),
            ..Default::default()
        };
        let state = build_awp_state(&cfg, dir.path()).await.unwrap().unwrap();
        assert_eq!(state.business_context.load().site_name, "Test Gateway");
    }

    #[tokio::test]
    async fn test_merge_none_is_noop() {
        let router: axum::Router = axum::Router::new();
        let _ = merge_awp_routes(router, None);
    }

    #[tokio::test]
    async fn test_merge_with_state() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("business.toml"), SAMPLE_TOML).unwrap();

        let cfg = AwpConfig {
            enabled: true,
            hot_reload: false,
            consent_file: PathBuf::from("consent.json"),
            ..Default::default()
        };
        let state = build_awp_state(&cfg, dir.path()).await.unwrap();

        let router: axum::Router = axum::Router::new();
        let merged = merge_awp_routes(router, state);

        use axum::body::Body;
        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .uri("/.well-known/awp.json")
            .body(Body::empty())
            .unwrap();
        let resp = merged.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_reporting() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("business.toml"), SAMPLE_TOML).unwrap();

        let cfg = AwpConfig {
            enabled: true,
            hot_reload: false,
            consent_file: PathBuf::from("consent.json"),
            ..Default::default()
        };
        let state = build_awp_state(&cfg, dir.path()).await.unwrap().unwrap();

        let snap = state.health.snapshot().await;
        assert_eq!(snap.state, adk_awp::HealthState::Healthy);

        report_degrading(&state, "high latency").await;
        let snap = state.health.snapshot().await;
        assert_eq!(snap.state, adk_awp::HealthState::Degrading);

        report_healthy(&state).await;
        let snap = state.health.snapshot().await;
        assert_eq!(snap.state, adk_awp::HealthState::Healthy);
    }
}
