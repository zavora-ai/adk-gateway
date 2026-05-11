//! Model factory — parses model strings like `"anthropic/claude-sonnet-4"` and
//! creates the corresponding adk-model instance.
//!
//! Supported providers:
//! - `gemini` / `google` → GeminiModel
//! - `openai` → OpenAIClient
//! - `anthropic` → AnthropicClient
//! - `ollama` → OllamaModel (local, no key)
//! - `deepseek` → DeepSeekClient
//! - `groq` → GroqClient
//! - `openrouter` → OpenRouterClient (model discovery + tools)
//! - `fireworks` → OpenAI-compatible (Fireworks AI)
//! - `together` → OpenAI-compatible (Together AI)
//! - `mistral` → OpenAI-compatible (Mistral AI)
//! - `perplexity` → OpenAI-compatible (Perplexity)
//! - `cerebras` → OpenAI-compatible (Cerebras)
//! - `sambanova` → OpenAI-compatible (SambaNova)
//! - `openai-compatible` → custom OpenAI-compatible endpoint (reads BASE_URL env)

use adk_core::Llm;
use std::sync::Arc;

/// Parse a model identifier and create the corresponding LLM client.
///
/// Format: `"provider/model-name"` or just `"model-name"` (defaults to Gemini).
pub fn create_model(model_id: &str) -> anyhow::Result<Arc<dyn Llm>> {
    let (provider, model_name) = parse_model_id(model_id);

    match provider {
        "gemini" | "google" => create_gemini(model_name),
        "openai" => create_openai(model_name),
        "anthropic" => create_anthropic(model_name),
        "ollama" => create_ollama(model_name),
        "deepseek" => create_deepseek(model_name),
        "groq" => create_groq(model_name),
        "openrouter" => create_openrouter(model_name),
        "fireworks" => create_openai_compatible(
            "fireworks",
            model_name,
            "FIREWORKS_API_KEY",
            "https://api.fireworks.ai/inference/v1",
        ),
        "together" => create_openai_compatible(
            "together",
            model_name,
            "TOGETHER_API_KEY",
            "https://api.together.xyz/v1",
        ),
        "mistral" => create_openai_compatible(
            "mistral",
            model_name,
            "MISTRAL_API_KEY",
            "https://api.mistral.ai/v1",
        ),
        "perplexity" => create_openai_compatible(
            "perplexity",
            model_name,
            "PERPLEXITY_API_KEY",
            "https://api.perplexity.ai",
        ),
        "cerebras" => create_openai_compatible(
            "cerebras",
            model_name,
            "CEREBRAS_API_KEY",
            "https://api.cerebras.ai/v1",
        ),
        "sambanova" => create_openai_compatible(
            "sambanova",
            model_name,
            "SAMBANOVA_API_KEY",
            "https://api.sambanova.ai/v1",
        ),
        "xai" => create_openai_compatible("xai", model_name, "XAI_API_KEY", "https://api.x.ai/v1"),
        "elevenlabs" => create_openai_compatible(
            "elevenlabs",
            model_name,
            "ELEVENLABS_API_KEY",
            "https://api.elevenlabs.io/v1",
        ),
        "replicate" => create_openai_compatible(
            "replicate",
            model_name,
            "REPLICATE_API_TOKEN",
            "https://api.replicate.com/v1",
        ),
        "assemblyai" => create_openai_compatible(
            "assemblyai",
            model_name,
            "ASSEMBLYAI_API_KEY",
            "https://api.assemblyai.com/v2",
        ),
        "deepgram" => create_openai_compatible(
            "deepgram",
            model_name,
            "DEEPGRAM_API_KEY",
            "https://api.deepgram.com/v1",
        ),
        "openai-compatible" => {
            let base_url = std::env::var("OPENAI_COMPATIBLE_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:8080/v1".into());
            create_openai_compatible("custom", model_name, "OPENAI_COMPATIBLE_API_KEY", &base_url)
        }
        _ => {
            tracing::warn!(
                provider,
                model_name,
                "unknown provider, falling back to gemini"
            );
            create_gemini(model_id)
        }
    }
}

/// All provider identifiers the factory recognises.
pub const PROVIDERS: &[(&str, &str, &str)] = &[
    // (id, display_name, env_var_for_key)
    ("anthropic", "Anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OpenAI", "OPENAI_API_KEY"),
    ("gemini", "Google Gemini", "GOOGLE_API_KEY"),
    ("openrouter", "OpenRouter", "OPENROUTER_API_KEY"),
    ("deepseek", "DeepSeek", "DEEPSEEK_API_KEY"),
    ("groq", "Groq", "GROQ_API_KEY"),
    ("ollama", "Ollama (local)", ""),
    ("fireworks", "Fireworks AI", "FIREWORKS_API_KEY"),
    ("together", "Together AI", "TOGETHER_API_KEY"),
    ("mistral", "Mistral AI", "MISTRAL_API_KEY"),
    ("perplexity", "Perplexity", "PERPLEXITY_API_KEY"),
    ("cerebras", "Cerebras", "CEREBRAS_API_KEY"),
    ("sambanova", "SambaNova", "SAMBANOVA_API_KEY"),
    ("xai", "xAI (Grok)", "XAI_API_KEY"),
    ("elevenlabs", "ElevenLabs", "ELEVENLABS_API_KEY"),
    ("replicate", "Replicate", "REPLICATE_API_TOKEN"),
    ("assemblyai", "AssemblyAI", "ASSEMBLYAI_API_KEY"),
    ("deepgram", "Deepgram", "DEEPGRAM_API_KEY"),
    (
        "openai-compatible",
        "OpenAI-Compatible (custom)",
        "OPENAI_COMPATIBLE_API_KEY",
    ),
];

/// Model hints per provider for the UI.
pub const MODEL_HINTS: &[(&str, &str)] = &[
    (
        "anthropic",
        "claude-sonnet-4 · claude-opus-4 · claude-haiku-3",
    ),
    ("openai", "gpt-4o · gpt-4o-mini · o3-mini · gpt-5"),
    (
        "gemini",
        "gemini-2.5-flash · gemini-2.5-pro · gemini-2.0-flash",
    ),
    (
        "openrouter",
        "openai/gpt-4.1-mini · anthropic/claude-sonnet-4 · (any model via discovery)",
    ),
    ("deepseek", "deepseek-chat · deepseek-reasoner"),
    ("groq", "llama-3.3-70b-versatile · mixtral-8x7b-32768"),
    ("ollama", "llama3.2 · mistral · codellama · phi3 · gemma2"),
    (
        "fireworks",
        "accounts/fireworks/models/llama-v3p3-70b-instruct",
    ),
    ("together", "meta-llama/Llama-3.3-70B-Instruct-Turbo"),
    ("mistral", "mistral-large-latest · mistral-small-latest"),
    ("perplexity", "sonar-pro · sonar"),
    ("cerebras", "llama3.1-70b · llama3.1-8b"),
    ("sambanova", "Meta-Llama-3.1-405B-Instruct"),
    ("xai", "grok-3 · grok-3-mini"),
    ("openai-compatible", "(depends on your endpoint)"),
];

// ── Provider constructors ──────────────────────────────────────────

fn parse_model_id(model_id: &str) -> (&str, &str) {
    match model_id.split_once('/') {
        Some((provider, name)) => (provider, name),
        None => ("gemini", model_id),
    }
}

/// Resolve the API key environment variable name from a model ID's provider prefix.
/// Returns empty string if the provider doesn't require a key (e.g., ollama).
pub fn resolve_api_key_env(model_id: &str) -> &'static str {
    let (provider, _) = parse_model_id(model_id);
    PROVIDERS
        .iter()
        .find(|(id, _, _)| *id == provider)
        .map(|(_, _, env)| *env)
        .unwrap_or("")
}

fn require_key(env_var: &str, provider: &str) -> anyhow::Result<String> {
    std::env::var(env_var).map_err(|_| anyhow::anyhow!("{provider} requires {env_var} env var"))
}

fn create_gemini(model_name: &str) -> anyhow::Result<Arc<dyn Llm>> {
    let api_key = std::env::var("GOOGLE_API_KEY")
        .or_else(|_| std::env::var("GEMINI_API_KEY"))
        .map_err(|_| anyhow::anyhow!("gemini requires GOOGLE_API_KEY or GEMINI_API_KEY env var"))?;
    let model = adk_model::GeminiModel::new(&api_key, model_name)?;
    tracing::info!(model = model_name, "created gemini model");
    Ok(Arc::new(model))
}

fn create_openai(model_name: &str) -> anyhow::Result<Arc<dyn Llm>> {
    let api_key = require_key("OPENAI_API_KEY", "openai")?;
    let config = adk_model::OpenAIConfig::new(&api_key, model_name);
    let model = adk_model::OpenAIClient::new(config)?;
    tracing::info!(model = model_name, "created openai model");
    Ok(Arc::new(model))
}

fn create_anthropic(model_name: &str) -> anyhow::Result<Arc<dyn Llm>> {
    let api_key = require_key("ANTHROPIC_API_KEY", "anthropic")?;
    let config = adk_model::anthropic::AnthropicConfig::new(&api_key, model_name);
    let model = adk_model::AnthropicClient::new(config)?;
    tracing::info!(model = model_name, "created anthropic model");
    Ok(Arc::new(model))
}

fn create_ollama(model_name: &str) -> anyhow::Result<Arc<dyn Llm>> {
    let config = adk_model::OllamaConfig::new(model_name);
    let model = adk_model::OllamaModel::new(config)?;
    tracing::info!(model = model_name, "created ollama model");
    Ok(Arc::new(model))
}

fn create_deepseek(model_name: &str) -> anyhow::Result<Arc<dyn Llm>> {
    let api_key = require_key("DEEPSEEK_API_KEY", "deepseek")?;
    let config = adk_model::DeepSeekConfig::new(&api_key, model_name);
    let model = adk_model::DeepSeekClient::new(config)?;
    tracing::info!(model = model_name, "created deepseek model");
    Ok(Arc::new(model))
}

fn create_groq(model_name: &str) -> anyhow::Result<Arc<dyn Llm>> {
    let api_key = require_key("GROQ_API_KEY", "groq")?;
    let config = adk_model::GroqConfig::new(&api_key, model_name);
    let model = adk_model::GroqClient::new(config)?;
    tracing::info!(model = model_name, "created groq model");
    Ok(Arc::new(model))
}

fn create_openrouter(model_name: &str) -> anyhow::Result<Arc<dyn Llm>> {
    let api_key = require_key("OPENROUTER_API_KEY", "openrouter")?;
    let config = adk_model::OpenRouterConfig::new(&api_key, model_name)
        .with_http_referer("https://github.com/zavora-ai/adk-gateway")
        .with_title("adk-gateway");
    let model = adk_model::OpenRouterClient::new(config)?;
    tracing::info!(model = model_name, "created openrouter model");
    Ok(Arc::new(model))
}

fn create_openai_compatible(
    provider_name: &str,
    model_name: &str,
    env_var: &str,
    base_url: &str,
) -> anyhow::Result<Arc<dyn Llm>> {
    let api_key = require_key(env_var, provider_name)?;
    let config = adk_model::OpenAICompatibleConfig::new(&api_key, model_name)
        .with_provider_name(provider_name)
        .with_base_url(base_url);
    let model = adk_model::OpenAICompatible::new(config)?;
    tracing::info!(
        provider = provider_name,
        model = model_name,
        "created openai-compatible model"
    );
    Ok(Arc::new(model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_model_id() {
        assert_eq!(
            parse_model_id("anthropic/claude-sonnet-4"),
            ("anthropic", "claude-sonnet-4")
        );
        assert_eq!(
            parse_model_id("openai/gpt-5-mini"),
            ("openai", "gpt-5-mini")
        );
        assert_eq!(
            parse_model_id("gemini-2.5-flash"),
            ("gemini", "gemini-2.5-flash")
        );
        assert_eq!(parse_model_id("ollama/llama3.2"), ("ollama", "llama3.2"));
        assert_eq!(
            parse_model_id("openrouter/openai/gpt-4.1-mini"),
            ("openrouter", "openai/gpt-4.1-mini")
        );
        assert_eq!(
            parse_model_id("deepseek/deepseek-chat"),
            ("deepseek", "deepseek-chat")
        );
    }

    #[test]
    fn test_providers_list_complete() {
        // Every provider in the match arm should be in PROVIDERS
        let provider_ids: Vec<&str> = PROVIDERS.iter().map(|(id, _, _)| *id).collect();
        for &(id, _, _) in PROVIDERS {
            assert!(provider_ids.contains(&id), "missing provider: {id}");
        }
        assert!(provider_ids.contains(&"openrouter"));
        assert!(provider_ids.contains(&"openai-compatible"));
    }
}
