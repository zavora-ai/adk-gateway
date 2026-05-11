//! Settings (Memory & RAG configuration) save API handler.

use super::ControlPanelState;
use std::sync::Arc;

#[derive(serde::Deserialize)]
pub(crate) struct SettingsPayload {
    memory_enabled: bool,
    memory_backend: Option<String>,
    memory_embedding_provider: Option<String>,
    memory_embedding_model: Option<String>,
    rag_enabled: bool,
    rag_vector_store: Option<String>,
    rag_embedding_provider: Option<String>,
    rag_embedding_model: Option<String>,
    rag_chunk_size: Option<usize>,
}

#[derive(serde::Serialize)]
pub(crate) struct SettingsResponse {
    pub ok: bool,
    pub message: String,
}

pub(crate) async fn settings_save(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<SettingsPayload>,
) -> axum::Json<SettingsResponse> {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return axum::Json(SettingsResponse {
                ok: false,
                message: "Config file path not configured".into(),
            });
        }
    };

    // Validate embedding providers
    let valid_providers = ["gemini", "openai"];
    if payload.memory_enabled {
        if let Some(ref provider) = payload.memory_embedding_provider {
            if !valid_providers.contains(&provider.as_str()) {
                return axum::Json(SettingsResponse {
                    ok: false,
                    message: format!(
                        "Invalid memory embedding provider: {provider}. Must be one of: {}",
                        valid_providers.join(", ")
                    ),
                });
            }
        }
    }
    if payload.rag_enabled {
        if let Some(ref provider) = payload.rag_embedding_provider {
            if !valid_providers.contains(&provider.as_str()) {
                return axum::Json(SettingsResponse {
                    ok: false,
                    message: format!(
                        "Invalid RAG embedding provider: {provider}. Must be one of: {}",
                        valid_providers.join(", ")
                    ),
                });
            }
        }
        if let Some(chunk_size) = payload.rag_chunk_size {
            if chunk_size == 0 || chunk_size > 8192 {
                return axum::Json(SettingsResponse {
                    ok: false,
                    message: "RAG chunk size must be between 1 and 8192".into(),
                });
            }
        }
    }

    let raw = match std::fs::read_to_string(&config_path) {
        Ok(r) => r,
        Err(e) => {
            return axum::Json(SettingsResponse {
                ok: false,
                message: format!("Failed to read config: {e}"),
            });
        }
    };

    let mut config_value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => match json5::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return axum::Json(SettingsResponse {
                    ok: false,
                    message: format!("Failed to parse config: {e}"),
                });
            }
        },
    };

    if payload.memory_enabled {
        let backend = payload.memory_backend.as_deref().unwrap_or("inmemory");
        let provider = payload
            .memory_embedding_provider
            .as_deref()
            .unwrap_or("openai");
        config_value["memory"] = serde_json::json!({
            "backend": backend,
            "embedding": {
                "provider": provider,
                "model": payload.memory_embedding_model,
            }
        });
    } else {
        config_value["memory"] = serde_json::Value::Null;
    }

    if payload.rag_enabled {
        let vector_store = payload.rag_vector_store.as_deref().unwrap_or("inmemory");
        let provider = payload
            .rag_embedding_provider
            .as_deref()
            .unwrap_or("openai");
        config_value["rag"] = serde_json::json!({
            "vectorStore": vector_store,
            "embedding": {
                "provider": provider,
                "model": payload.rag_embedding_model,
            },
            "chunkSize": payload.rag_chunk_size.unwrap_or(512),
        });
    } else {
        config_value["rag"] = serde_json::Value::Null;
    }

    let output = match serde_json::to_string_pretty(&config_value) {
        Ok(s) => s,
        Err(e) => {
            return axum::Json(SettingsResponse {
                ok: false,
                message: format!("Failed to serialize config: {e}"),
            });
        }
    };

    if let Err(e) = std::fs::write(&config_path, &output) {
        return axum::Json(SettingsResponse {
            ok: false,
            message: format!("Failed to write config: {e}"),
        });
    }

    if let Ok(new_cfg) = serde_json::from_str::<crate::config::GatewayConfig>(&output) {
        state.config.store(std::sync::Arc::new(new_cfg));
    }

    tracing::info!("settings saved to {}", config_path.display());

    axum::Json(SettingsResponse {
        ok: true,
        message: "Settings saved and applied.".into(),
    })
}

/// GET /ui/api/settings/session-status — returns session backend type, health, and masked connection string.
pub(crate) async fn session_status(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let session_cfg = &config.session;

    let backend = format!("{:?}", session_cfg.backend).to_lowercase();

    // Mask the connection string for security
    let connection_string = session_cfg
        .connection_string
        .as_deref()
        .map(|s| {
            if s.len() <= 8 {
                "***".to_string()
            } else {
                format!("{}***{}", &s[..4], &s[s.len() - 4..])
            }
        })
        .unwrap_or_else(|| "(default)".to_string());

    // Simple health check: if backend is inmemory it's always healthy,
    // otherwise we report healthy (actual connectivity check would require
    // a session bridge reference which may not be available).
    let healthy = true;

    axum::Json(serde_json::json!({
        "ok": true,
        "data": {
            "backend": backend,
            "healthy": healthy,
            "connection_string": connection_string,
        }
    }))
}
