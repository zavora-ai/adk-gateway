//! Agent & Model save API handler.

use super::settings::SettingsResponse;
use super::ControlPanelState;
use std::sync::Arc;

/// GET /ui/api/agent — returns current agent model configuration.
/// Category fields are returned as arrays (fallback chains) or null.
/// Also includes model_hints per provider for the UI model selector.
pub(crate) async fn agent_get(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let cat = &config.agent.model;

    // Build model_hints map from the MODEL_HINTS constant
    let model_hints: serde_json::Map<String, serde_json::Value> = crate::model_factory::MODEL_HINTS
        .iter()
        .map(|(provider, hints)| {
            (
                provider.to_string(),
                serde_json::Value::String(hints.to_string()),
            )
        })
        .collect();

    // Read cloud_provider from config file if present
    let cloud_provider = state.config_path.as_ref().and_then(|p| {
        let raw = std::fs::read_to_string(p).ok()?;
        let val: serde_json::Value = serde_json::from_str(&raw)
            .or_else(|_| json5::from_str(&raw))
            .ok()?;
        val.get("agent")?.get("cloud_provider").cloned()
    });

    let mut data = serde_json::json!({
        "primary": cat.primary(),
        "vision": cat.vision,
        "omni": cat.omni,
        "image_generation": cat.image_generation,
        "tts": cat.tts,
        "stt": cat.stt,
        "code": cat.code,
        "embedding": cat.embedding,
        "search": cat.search,
        "music": cat.music,
        "model_hints": model_hints,
    });

    if let Some(cp) = cloud_provider {
        data["cloud_provider"] = cp;
    }

    axum::Json(serde_json::json!({
        "ok": true,
        "data": data
    }))
}

#[derive(serde::Deserialize)]
pub(crate) struct AgentPayload {
    provider: Option<String>,
    model_name: Option<String>,
    base_url: Option<String>,
    #[serde(default)]
    api_keys: std::collections::HashMap<String, String>,
    // Category fields — accept string or array for backward compat
    primary: Option<serde_json::Value>,
    vision: Option<serde_json::Value>,
    omni: Option<serde_json::Value>,
    image_generation: Option<serde_json::Value>,
    tts: Option<serde_json::Value>,
    stt: Option<serde_json::Value>,
    code: Option<serde_json::Value>,
    embedding: Option<serde_json::Value>,
    search: Option<serde_json::Value>,
    music: Option<serde_json::Value>,
    // Enterprise cloud provider config
    cloud_provider: Option<serde_json::Value>,
}

/// Convert a JSON value (string or array of strings) to a JSON array for config storage.
fn to_model_array(val: &serde_json::Value) -> Option<serde_json::Value> {
    match val {
        serde_json::Value::String(s) if !s.is_empty() => {
            Some(serde_json::Value::Array(vec![serde_json::Value::String(
                s.clone(),
            )]))
        }
        serde_json::Value::Array(arr) => {
            let filtered: Vec<serde_json::Value> = arr
                .iter()
                .filter_map(|v| {
                    v.as_str()
                        .filter(|s| !s.is_empty())
                        .map(|s| serde_json::Value::String(s.to_string()))
                })
                .collect();
            if filtered.is_empty() {
                None
            } else {
                Some(serde_json::Value::Array(filtered))
            }
        }
        _ => None,
    }
}

pub(crate) async fn agent_save(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<AgentPayload>,
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

    // Determine primary model ID from either the new `primary` field or legacy provider+model_name
    // For enterprise cloud provider configs, primary is not required
    let is_cloud_provider = payload.cloud_provider.is_some();
    let primary_id = if let Some(ref pv) = payload.primary {
        match pv {
            serde_json::Value::String(s) => {
                if s.trim().is_empty() {
                    if !is_cloud_provider {
                        return axum::Json(SettingsResponse {
                            ok: false,
                            message: "Primary model is required".into(),
                        });
                    }
                    String::new()
                } else {
                    s.trim().to_string()
                }
            }
            serde_json::Value::Array(arr) => {
                let first = arr
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if first.is_empty() && !is_cloud_provider {
                    return axum::Json(SettingsResponse {
                        ok: false,
                        message: "Primary model is required".into(),
                    });
                }
                first
            }
            _ => {
                if !is_cloud_provider {
                    return axum::Json(SettingsResponse {
                        ok: false,
                        message: "Primary model must be a string or array".into(),
                    });
                }
                String::new()
            }
        }
    } else if let Some(ref model_name) = payload.model_name {
        if model_name.trim().is_empty() {
            return axum::Json(SettingsResponse {
                ok: false,
                message: "Model name is required".into(),
            });
        }
        let provider = payload.provider.as_deref().unwrap_or("gemini");
        if provider == "gemini" && !model_name.contains('/') {
            format!("gemini/{}", model_name.trim())
        } else {
            format!("{}/{}", provider, model_name.trim())
        }
    } else if is_cloud_provider {
        String::new()
    } else {
        return axum::Json(SettingsResponse {
            ok: false,
            message: "Primary model is required".into(),
        });
    };

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

    // Build the model config object with array values
    let mut model_obj = serde_json::Map::new();

    // Primary: store as array if payload.primary is an array with fallbacks, else string
    if let Some(ref pv) = payload.primary {
        if let Some(arr) = to_model_array(pv) {
            if let serde_json::Value::Array(ref a) = arr {
                if a.len() == 1 {
                    model_obj.insert("primary".into(), a[0].clone());
                } else {
                    model_obj.insert("primary".into(), arr);
                }
            }
        } else {
            model_obj.insert(
                "primary".into(),
                serde_json::Value::String(primary_id.clone()),
            );
        }
    } else {
        model_obj.insert(
            "primary".into(),
            serde_json::Value::String(primary_id.clone()),
        );
    }

    // Category fields: store as arrays
    let category_fields = [
        ("vision", &payload.vision),
        ("omni", &payload.omni),
        ("image_generation", &payload.image_generation),
        ("tts", &payload.tts),
        ("stt", &payload.stt),
        ("code", &payload.code),
        ("embedding", &payload.embedding),
        ("search", &payload.search),
        ("music", &payload.music),
    ];

    for (key, val) in &category_fields {
        if let Some(v) = val {
            if let Some(arr) = to_model_array(v) {
                model_obj.insert((*key).into(), arr);
            }
        }
    }

    config_value["agent"]["model"] = serde_json::Value::Object(model_obj);

    // Persist cloud_provider config if provided
    if let Some(ref cp) = payload.cloud_provider {
        config_value["agent"]["cloud_provider"] = cp.clone();
    }

    for &(provider_id, _, env_var) in crate::model_factory::PROVIDERS {
        if env_var.is_empty() {
            continue;
        }
        if let Some(key) = payload.api_keys.get(provider_id) {
            if !key.is_empty() {
                if key.chars().any(|c| c.is_control()) {
                    return axum::Json(SettingsResponse {
                        ok: false,
                        message: format!("API key for {} contains invalid characters", provider_id),
                    });
                }
                unsafe {
                    std::env::set_var(env_var, key);
                }
                if provider_id == "gemini" {
                    unsafe {
                        std::env::set_var("GEMINI_API_KEY", key);
                    }
                }
            }
        }
    }

    if let Some(ref url) = payload.base_url {
        if !url.is_empty() {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return axum::Json(SettingsResponse {
                    ok: false,
                    message: "Base URL must start with http:// or https://".into(),
                });
            }
            unsafe {
                std::env::set_var("OPENAI_COMPATIBLE_BASE_URL", url);
            }
        }
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

    tracing::info!("agent config saved to {}", config_path.display());

    axum::Json(SettingsResponse {
        ok: true,
        message: "Agent config saved and applied.".into(),
    })
}
