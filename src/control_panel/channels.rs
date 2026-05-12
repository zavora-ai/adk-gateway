//! Channels API handlers (GET + POST).

use super::settings::SettingsResponse;
use super::ControlPanelState;
use std::sync::Arc;

/// GET /ui/api/channels — return current channel config with tokens masked.
pub(crate) async fn channels_get(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let channels = &config.channels;

    axum::Json(serde_json::json!({
        "ok": true,
        "data": {
            "telegram": channels.telegram.as_ref().map(|tg| serde_json::json!({
                "enabled": tg.enabled,
                "bot_token": if tg.bot_token.is_empty() { "" } else { "••••••••" },
                "dm_policy": tg.dm_policy,
                "stream_mode": tg.stream_mode.as_deref().unwrap_or("partial"),
            })),
            "slack": channels.slack.as_ref().map(|sl| serde_json::json!({
                "enabled": sl.enabled,
                "bot_token": if sl.bot_token.is_empty() { "" } else { "••••••••" },
                "app_token": if sl.app_token.is_empty() { "" } else { "••••••••" },
                "dm_policy": sl.dm_policy,
            })),
            "whatsapp": channels.whatsapp.as_ref().map(|wa| serde_json::json!({
                "enabled": wa.enabled,
                "phone_number_id": &wa.phone_number_id,
                "access_token": if wa.access_token.is_empty() { "" } else { "••••••••" },
                "verify_token": &wa.verify_token,
                "webhook_path": &wa.webhook_path,
            })),
            "discord": channels.discord.as_ref().map(|dc| serde_json::json!({
                "enabled": dc.enabled,
                "bot_token": if dc.bot_token.is_empty() { "" } else { "••••••••" },
                "application_id": &dc.application_id,
                "guild_ids": &dc.guild_ids,
            })),
            "matrix": channels.matrix.as_ref().map(|mx| serde_json::json!({
                "enabled": mx.enabled,
                "homeserver_url": &mx.homeserver_url,
                "access_token": if mx.access_token.is_empty() { "" } else { "••••••••" },
                "user_id": &mx.user_id,
                "room_ids": &mx.room_ids,
            })),
        }
    }))
}

#[derive(serde::Deserialize)]
pub(crate) struct ChannelsPayload {
    telegram: Option<TelegramPayload>,
    slack: Option<SlackPayload>,
    whatsapp: Option<WhatsAppPayload>,
    discord: Option<DiscordPayload>,
    matrix: Option<MatrixPayload>,
}

#[derive(serde::Deserialize)]
struct TelegramPayload {
    enabled: bool,
    bot_token: Option<String>,
    dm_policy: Option<String>,
    stream_mode: Option<String>,
}

#[derive(serde::Deserialize)]
struct SlackPayload {
    enabled: bool,
    bot_token: Option<String>,
    app_token: Option<String>,
    dm_policy: Option<String>,
}

#[derive(serde::Deserialize)]
struct WhatsAppPayload {
    enabled: bool,
    phone_number_id: Option<String>,
    access_token: Option<String>,
    verify_token: Option<String>,
    webhook_path: Option<String>,
}

#[derive(serde::Deserialize)]
struct DiscordPayload {
    enabled: bool,
    bot_token: Option<String>,
    application_id: Option<String>,
    guild_ids: Option<Vec<String>>,
}

#[derive(serde::Deserialize)]
struct MatrixPayload {
    enabled: bool,
    homeserver_url: Option<String>,
    access_token: Option<String>,
    user_id: Option<String>,
    room_ids: Option<Vec<String>>,
}

pub(crate) async fn channels_save(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<ChannelsPayload>,
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

    // Load existing config to preserve tokens when payload sends empty strings
    let existing_config = state.config.load();

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

    if config_value.get("channels").is_none() {
        config_value["channels"] = serde_json::json!({});
    }

    // Telegram
    if let Some(tg) = &payload.telegram {
        if tg.enabled {
            // Use provided token, or fall back to existing token if empty
            let token = match tg.bot_token.as_deref() {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => existing_config
                    .channels
                    .telegram
                    .as_ref()
                    .map(|existing| existing.bot_token.clone())
                    .unwrap_or_default(),
            };
            if token.is_empty() {
                return axum::Json(SettingsResponse {
                    ok: false,
                    message: "Telegram bot token is required when enabled".into(),
                });
            }
            let dm_policy = tg.dm_policy.as_deref().unwrap_or("open");
            if !["open", "allowlist", "pairing", "disabled"].contains(&dm_policy) {
                return axum::Json(SettingsResponse {
                    ok: false,
                    message: format!("Invalid DM policy: {dm_policy}"),
                });
            }
            config_value["channels"]["telegram"] = serde_json::json!({
                "enabled": true,
                "botToken": token,
                "dmPolicy": dm_policy,
                "streamMode": tg.stream_mode.as_deref().unwrap_or("partial"),
                "allowFrom": ["*"],
            });
        } else {
            config_value["channels"]
                .as_object_mut()
                .map(|o| o.remove("telegram"));
        }
    }

    // Slack
    if let Some(sl) = &payload.slack {
        if sl.enabled {
            let bot_token = match sl.bot_token.as_deref() {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => existing_config
                    .channels
                    .slack
                    .as_ref()
                    .map(|existing| existing.bot_token.clone())
                    .unwrap_or_default(),
            };
            let app_token = match sl.app_token.as_deref() {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => existing_config
                    .channels
                    .slack
                    .as_ref()
                    .map(|existing| existing.app_token.clone())
                    .unwrap_or_default(),
            };
            if bot_token.is_empty() || app_token.is_empty() {
                return axum::Json(SettingsResponse {
                    ok: false,
                    message: "Slack bot token and app token are both required when enabled".into(),
                });
            }
            config_value["channels"]["slack"] = serde_json::json!({
                "enabled": true,
                "botToken": bot_token,
                "appToken": app_token,
                "dmPolicy": sl.dm_policy.as_deref().unwrap_or("open"),
                "allowFrom": ["*"],
            });
        } else {
            config_value["channels"]
                .as_object_mut()
                .map(|o| o.remove("slack"));
        }
    }

    // WhatsApp
    if let Some(wa) = &payload.whatsapp {
        if wa.enabled {
            let phone_number_id = wa.phone_number_id.as_deref().unwrap_or("");
            let access_token = match wa.access_token.as_deref() {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => existing_config
                    .channels
                    .whatsapp
                    .as_ref()
                    .map(|existing| existing.access_token.clone())
                    .unwrap_or_default(),
            };
            if phone_number_id.is_empty() || access_token.is_empty() {
                return axum::Json(SettingsResponse {
                    ok: false,
                    message: "WhatsApp phone number ID and access token are required when enabled"
                        .into(),
                });
            }
            config_value["channels"]["whatsapp"] = serde_json::json!({
                "enabled": true,
                "phoneNumberId": phone_number_id,
                "accessToken": access_token,
                "verifyToken": wa.verify_token.as_deref().unwrap_or(""),
                "webhookPath": wa.webhook_path.as_deref().unwrap_or("/webhook/whatsapp"),
            });
        } else {
            config_value["channels"]
                .as_object_mut()
                .map(|o| o.remove("whatsapp"));
        }
    }

    // Discord
    if let Some(dc) = &payload.discord {
        if dc.enabled {
            let bot_token = match dc.bot_token.as_deref() {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => existing_config
                    .channels
                    .discord
                    .as_ref()
                    .map(|existing| existing.bot_token.clone())
                    .unwrap_or_default(),
            };
            if bot_token.is_empty() {
                return axum::Json(SettingsResponse {
                    ok: false,
                    message: "Discord bot token is required when enabled".into(),
                });
            }
            config_value["channels"]["discord"] = serde_json::json!({
                "enabled": true,
                "botToken": bot_token,
                "applicationId": dc.application_id.as_deref().unwrap_or(""),
                "guildIds": dc.guild_ids.as_deref().unwrap_or(&[]),
            });
        } else {
            config_value["channels"]
                .as_object_mut()
                .map(|o| o.remove("discord"));
        }
    }

    // Matrix
    if let Some(mx) = &payload.matrix {
        if mx.enabled {
            let homeserver_url = mx.homeserver_url.as_deref().unwrap_or("");
            let access_token = match mx.access_token.as_deref() {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => existing_config
                    .channels
                    .matrix
                    .as_ref()
                    .map(|existing| existing.access_token.clone())
                    .unwrap_or_default(),
            };
            if homeserver_url.is_empty() || access_token.is_empty() {
                return axum::Json(SettingsResponse {
                    ok: false,
                    message: "Matrix homeserver URL and access token are required when enabled"
                        .into(),
                });
            }
            config_value["channels"]["matrix"] = serde_json::json!({
                "enabled": true,
                "homeserverUrl": homeserver_url,
                "accessToken": access_token,
                "userId": mx.user_id.as_deref().unwrap_or(""),
                "roomIds": mx.room_ids.as_deref().unwrap_or(&[]),
            });
        } else {
            config_value["channels"]
                .as_object_mut()
                .map(|o| o.remove("matrix"));
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

    tracing::info!("channels saved to {}", config_path.display());

    axum::Json(SettingsResponse {
        ok: true,
        message: "Channels saved and applied.".into(),
    })
}

/// POST /ui/api/channels/telegram/probe — test Telegram bot token connectivity.
pub(crate) async fn telegram_probe(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let tg_config = match &config.channels.telegram {
        Some(tg) if !tg.bot_token.is_empty() => tg.clone(),
        _ => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": "Telegram is not configured or bot token is empty"
            }));
        }
    };

    let channel = crate::channel::telegram::TelegramChannel::new(tg_config);
    let result = channel.probe().await;

    let data = match result {
        crate::channel::telegram::ProbeResult::Connected { bot_username } => {
            serde_json::json!({
                "ok": true,
                "data": {
                    "status": "connected",
                    "bot_username": bot_username,
                }
            })
        }
        crate::channel::telegram::ProbeResult::InvalidToken => {
            serde_json::json!({
                "ok": true,
                "data": {
                    "status": "invalid_token",
                }
            })
        }
        crate::channel::telegram::ProbeResult::Unreachable { .. } => {
            serde_json::json!({
                "ok": true,
                "data": {
                    "status": "unreachable",
                }
            })
        }
        crate::channel::telegram::ProbeResult::Error { message } => {
            serde_json::json!({
                "ok": true,
                "data": {
                    "status": format!("error: {}", message),
                }
            })
        }
    };

    axum::Json(data)
}
