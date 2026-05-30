//! Streaming task executor for coding agents.
//!
//! This module provides a streaming-aware task executor that forwards real-time
//! updates from ACP agents to users via the delivery system. Unlike the basic
//! executor which blocks until completion, this streams:
//!
//! - Text chunks as they're generated
//! - Tool call notifications (🔧 Reading file..., ✅ Done)
//! - Status changes (Starting, Running, etc.)
//! - Permission requests
//!
//! ## Integration
//!
//! The `StreamingTaskExecutor` wraps the `StreamingAcpClient` and integrates with
//! the gateway's `DeliveryStrategy` to forward updates to users in real-time.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tracing::{info, warn, error};

use crate::channel::{Channel, ChannelKey, ChannelType};
use crate::delivery::{DeliveryStrategy, MessageRef, select_strategy};

use super::acp_streaming::{
    CodingAgentUpdate, CodingAgentUpdateStream, StreamingAcpClient,
    format_update_for_display,
};
use super::models::{TaskError, TaskRequest, TaskResult};
use super::registry::CodingAgentRegistry;

/// Configuration for streaming task execution.
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Minimum interval between status/tool updates sent to user (default: 500ms)
    pub update_interval: Duration,
    /// Whether to show agent thoughts/reasoning (default: false)
    pub show_thoughts: bool,
    /// Whether to stream text chunks or wait for completion (default: true)
    pub stream_text: bool,
    /// Maximum text chunk size before sending (default: 100 chars)
    pub text_chunk_size: usize,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            update_interval: Duration::from_millis(500),
            show_thoughts: false,
            stream_text: true,
            text_chunk_size: 100,
        }
    }
}

/// Streaming task executor that forwards ACP updates to users.
pub struct StreamingTaskExecutor {
    /// The streaming ACP client
    client: StreamingAcpClient,
    /// Registry for looking up agent configs
    registry: Arc<CodingAgentRegistry>,
    /// Channel map for looking up channels to deliver results
    channel_map: Arc<DashMap<ChannelKey, Arc<dyn Channel>>>,
    /// Streaming configuration
    config: StreamingConfig,
}

impl StreamingTaskExecutor {
    /// Create a new streaming task executor.
    pub fn new(
        registry: Arc<CodingAgentRegistry>,
        channel_map: Arc<DashMap<ChannelKey, Arc<dyn Channel>>>,
    ) -> Self {
        Self {
            client: StreamingAcpClient::new(),
            registry,
            channel_map,
            config: StreamingConfig::default(),
        }
    }

    /// Create with custom streaming configuration.
    pub fn with_config(
        registry: Arc<CodingAgentRegistry>,
        channel_map: Arc<DashMap<ChannelKey, Arc<dyn Channel>>>,
        config: StreamingConfig,
    ) -> Self {
        Self {
            client: StreamingAcpClient::new(),
            registry,
            channel_map,
            config,
        }
    }

    /// Execute a task with streaming updates delivered to the user.
    ///
    /// This method:
    /// 1. Looks up the channel for the reply target
    /// 2. Creates a delivery strategy
    /// 3. Starts the ACP streaming session
    /// 4. Forwards updates to the delivery strategy in real-time
    /// 5. Returns the final result when complete
    ///
    /// # Arguments
    /// * `agent_id` - The agent to execute on
    /// * `request` - The task request (includes reply_to for delivery)
    pub async fn execute_with_streaming(
        &self,
        agent_id: &str,
        request: &TaskRequest,
    ) -> Result<TaskResult, TaskError> {
        // Look up agent config
        let agent = self.registry.get_agent(agent_id).ok_or_else(|| {
            TaskError::AgentDisconnected {
                agent_id: agent_id.to_string(),
            }
        })?;

        // Build message ref from request
        let msg_ref = msg_ref_from_request(request);

        // Look up the channel for delivery (best-effort).
        // If no channel is found (e.g. cron/internal triggers, or the channel
        // isn't registered yet), we STILL execute the task — we just skip live
        // streaming. The result is returned and recorded in task history either
        // way. Failing here would mean the agent never runs at all.
        let channel_key = ChannelKey {
            channel_type: msg_ref.channel_type,
            account_id: msg_ref.account_id.clone(),
        };
        let delivery = self.channel_map.get(&channel_key)
            .map(|c| select_strategy(c.value().clone(), Some("partial")));

        if delivery.is_none() {
            warn!(
                channel_type = ?msg_ref.channel_type,
                account_id = %msg_ref.account_id,
                recipient = %msg_ref.recipient_id,
                "No delivery channel found — task will execute without live streaming"
            );
        }

        info!(
            agent_id = %agent_id,
            channel_type = ?msg_ref.channel_type,
            recipient = %msg_ref.recipient_id,
            has_delivery = delivery.is_some(),
            "Starting streaming task execution"
        );

        // Start streaming
        let mut stream = self.client
            .execute_streaming(agent_id, &agent.config, request)
            .await?;

        // Process updates, forwarding to delivery if available
        match delivery {
            Some(delivery) => self.process_stream(&mut stream, delivery, msg_ref).await,
            None => self.process_stream_no_delivery(&mut stream).await,
        }
    }

    /// Execute without looking up channel - use provided delivery strategy.
    pub async fn execute_with_delivery(
        &self,
        agent_id: &str,
        request: &TaskRequest,
        delivery: Arc<dyn DeliveryStrategy>,
        msg_ref: MessageRef,
    ) -> Result<TaskResult, TaskError> {
        // Look up agent config
        let agent = self.registry.get_agent(agent_id).ok_or_else(|| {
            TaskError::AgentDisconnected {
                agent_id: agent_id.to_string(),
            }
        })?;

        // Start streaming
        let mut stream = self.client
            .execute_streaming(agent_id, &agent.config, request)
            .await?;

        // Process updates and forward to delivery
        self.process_stream(&mut stream, delivery, msg_ref).await
    }

    /// Process the update stream and forward to delivery.
    async fn process_stream(
        &self,
        stream: &mut CodingAgentUpdateStream,
        delivery: Arc<dyn DeliveryStrategy>,
        msg_ref: MessageRef,
    ) -> Result<TaskResult, TaskError> {
        let mut accumulated_text = String::new();
        let mut last_update_sent = std::time::Instant::now();
        let mut pending_status_updates: Vec<String> = Vec::new();

        while let Some(update) = stream.recv().await {
            match update {
                CodingAgentUpdate::Text(text) => {
                    accumulated_text.push_str(&text);
                    
                    // Stream text if enabled and we have enough
                    if self.config.stream_text && 
                       accumulated_text.len() >= self.config.text_chunk_size {
                        let _ = delivery.on_partial(&accumulated_text, &msg_ref).await;
                    }
                }
                
                CodingAgentUpdate::Thought(thought) => {
                    if self.config.show_thoughts {
                        // Format thought with dimmed styling
                        let thought_text = format!("💭 _{}_", thought.trim());
                        pending_status_updates.push(thought_text);
                    }
                }
                
                CodingAgentUpdate::Status(status) => {
                    if let Some(formatted) = format_update_for_display(&CodingAgentUpdate::Status(status)) {
                        pending_status_updates.push(formatted);
                    }
                }
                
                CodingAgentUpdate::ToolCallStarted { title } => {
                    let formatted = format!("🔧 {}", title);
                    pending_status_updates.push(formatted);
                }
                
                CodingAgentUpdate::ToolCallCompleted { title } => {
                    let formatted = format!("✅ {}", title);
                    pending_status_updates.push(formatted);
                }
                
                CodingAgentUpdate::PermissionRequested { title, approved } => {
                    let formatted = if approved {
                        format!("🔓 {}", title)
                    } else {
                        format!("🔒 Denied: {}", title)
                    };
                    pending_status_updates.push(formatted);
                }
                
                CodingAgentUpdate::Done { output, duration, success, error } => {
                    // Send final complete message
                    if success {
                        let _ = delivery.on_complete(&output, &msg_ref).await;
                        return Ok(TaskResult {
                            output,
                            modified_files: vec![],
                            duration_ms: duration.as_millis() as u64,
                            token_usage: None,
                        });
                    } else {
                        // Send error message
                        let error_msg = error.clone().unwrap_or_else(|| "Unknown error".to_string());
                        let _ = delivery.on_complete(&format!("❌ Task failed: {}", error_msg), &msg_ref).await;
                        return Err(TaskError::ExecutionError {
                            message: error_msg,
                            partial_output: if output.is_empty() { None } else { Some(output) },
                        });
                    }
                }
            }

            // Send accumulated status updates at configured interval
            if !pending_status_updates.is_empty() && 
               last_update_sent.elapsed() >= self.config.update_interval {
                // Combine status updates with current text
                let status_block = pending_status_updates.join("\n");
                let combined = if accumulated_text.is_empty() {
                    status_block
                } else {
                    format!("{}\n\n---\n{}", status_block, accumulated_text)
                };
                
                let _ = delivery.on_partial(&combined, &msg_ref).await;
                pending_status_updates.clear();
                last_update_sent = std::time::Instant::now();
            }
        }

        // Stream ended without Done - shouldn't happen but handle gracefully
        error!("ACP stream ended unexpectedly without Done update");
        Err(TaskError::ExecutionError {
            message: "Stream ended unexpectedly".to_string(),
            partial_output: if accumulated_text.is_empty() { None } else { Some(accumulated_text) },
        })
    }

    /// Process the update stream without a delivery channel.
    ///
    /// Used when no channel is available (cron/internal triggers, or the
    /// channel isn't registered). The task still executes fully and the result
    /// is returned for recording in task history — there's just no live
    /// streaming to a user.
    async fn process_stream_no_delivery(
        &self,
        stream: &mut CodingAgentUpdateStream,
    ) -> Result<TaskResult, TaskError> {
        let mut accumulated_text = String::new();

        while let Some(update) = stream.recv().await {
            match update {
                CodingAgentUpdate::Text(text) => {
                    accumulated_text.push_str(&text);
                }
                CodingAgentUpdate::Done { output, duration, success, error } => {
                    if success {
                        return Ok(TaskResult {
                            output,
                            modified_files: vec![],
                            duration_ms: duration.as_millis() as u64,
                            token_usage: None,
                        });
                    } else {
                        let error_msg = error.unwrap_or_else(|| "Unknown error".to_string());
                        return Err(TaskError::ExecutionError {
                            message: error_msg,
                            partial_output: if output.is_empty() { None } else { Some(output) },
                        });
                    }
                }
                // Other updates (status, tool calls, thoughts) are ignored — no delivery target.
                _ => {}
            }
        }

        error!("ACP stream ended unexpectedly without Done update (no-delivery path)");
        Err(TaskError::ExecutionError {
            message: "Stream ended unexpectedly".to_string(),
            partial_output: if accumulated_text.is_empty() { None } else { Some(accumulated_text) },
        })
    }

    /// Get usage statistics from the underlying client.
    pub fn usage_stats(&self) -> adk_acp::AcpUsageStats {
        self.client.usage_stats()
    }
}

/// Create a message ref from a task request's reply target.
pub fn msg_ref_from_request(request: &TaskRequest) -> MessageRef {
    // Map channel type string to enum
    let channel_type = match request.reply_to.channel_type.to_lowercase().as_str() {
        "telegram" => ChannelType::Telegram,
        "slack" => ChannelType::Slack,
        "discord" => ChannelType::Discord,
        "whatsapp" => ChannelType::Whatsapp,
        "matrix" => ChannelType::Matrix,
        _ => ChannelType::Telegram, // Default fallback
    };
    
    MessageRef {
        channel_type,
        account_id: "default".to_string(),
        recipient_id: request.reply_to.channel_id.clone(),
        message_id: request.reply_to.message_id.clone(),
        reply_to: request.reply_to.message_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::models::{ReplyTarget, TaskTrigger};
    use std::path::PathBuf;

    fn make_request(description: &str) -> TaskRequest {
        TaskRequest {
            description: description.to_string(),
            trigger: TaskTrigger::ControlPanel {
                user_id: "test".to_string(),
            },
            workspace: Some(PathBuf::from("/tmp/test")),
            file_context: None,
            reply_to: ReplyTarget {
                channel_type: "telegram".to_string(),
                channel_id: "123".to_string(),
                message_id: Some("456".to_string()),
            },
        }
    }

    #[test]
    fn test_streaming_config_defaults() {
        let config = StreamingConfig::default();
        assert_eq!(config.update_interval, Duration::from_millis(500));
        assert!(!config.show_thoughts);
        assert!(config.stream_text);
        assert_eq!(config.text_chunk_size, 100);
    }

    #[test]
    fn test_msg_ref_from_request() {
        let request = make_request("test task");
        let msg_ref = msg_ref_from_request(&request);
        
        assert_eq!(msg_ref.channel_type, ChannelType::Telegram);
        assert_eq!(msg_ref.recipient_id, "123");
        assert_eq!(msg_ref.message_id, Some("456".to_string()));
    }

    #[test]
    fn test_msg_ref_channel_type_mapping() {
        let mut request = make_request("test");
        
        request.reply_to.channel_type = "slack".to_string();
        assert_eq!(msg_ref_from_request(&request).channel_type, ChannelType::Slack);
        
        request.reply_to.channel_type = "discord".to_string();
        assert_eq!(msg_ref_from_request(&request).channel_type, ChannelType::Discord);
        
        request.reply_to.channel_type = "TELEGRAM".to_string(); // case insensitive
        assert_eq!(msg_ref_from_request(&request).channel_type, ChannelType::Telegram);
        
        request.reply_to.channel_type = "unknown".to_string();
        assert_eq!(msg_ref_from_request(&request).channel_type, ChannelType::Telegram); // fallback
    }
}
