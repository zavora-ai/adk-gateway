//! Fallback model chain — tries models in order until one succeeds.
//!
//! R16: When the primary model for a category fails (API error, timeout, rate limit),
//! automatically tries the next model in the fallback chain. Transparent to the user.

use adk_core::Llm;
use std::sync::Arc;

use crate::model_factory;

/// A chain of models to try in order. First success wins.
#[derive(Clone)]
pub struct FallbackModelChain {
    /// Pre-created model instances (eager for primary category).
    models: Vec<Arc<dyn Llm>>,
    /// Model IDs for logging.
    model_ids: Vec<String>,
}

/// Result of creating a fallback chain — includes which models failed to initialize.
pub struct ChainBuildResult {
    pub chain: FallbackModelChain,
    pub failed: Vec<(String, String)>, // (model_id, error)
}

impl FallbackModelChain {
    /// Create a fallback chain from a list of model IDs.
    /// Models that fail to initialize are skipped with a warning.
    /// Returns at least one model or an error if all fail.
    pub fn build(model_ids: &[String]) -> anyhow::Result<ChainBuildResult> {
        let mut models = Vec::new();
        let mut valid_ids = Vec::new();
        let mut failed = Vec::new();

        for id in model_ids {
            match model_factory::create_model(id) {
                Ok(model) => {
                    models.push(model);
                    valid_ids.push(id.clone());
                }
                Err(e) => {
                    tracing::warn!(model = %id, error = %e, "failed to create fallback model, skipping");
                    failed.push((id.clone(), e.to_string()));
                }
            }
        }

        if models.is_empty() {
            anyhow::bail!(
                "all models in fallback chain failed to initialize: {:?}",
                model_ids
            );
        }

        Ok(ChainBuildResult {
            chain: FallbackModelChain {
                models,
                model_ids: valid_ids,
            },
            failed,
        })
    }

    /// Create a chain from a single pre-built model (no fallbacks).
    pub fn single(model: Arc<dyn Llm>, model_id: String) -> Self {
        Self {
            models: vec![model],
            model_ids: vec![model_id],
        }
    }

    /// Create an empty chain for testing purposes only.
    /// This chain has no models and should not be used for actual LLM calls.
    #[cfg(test)]
    pub fn new_empty_for_test() -> Self {
        Self {
            models: vec![],
            model_ids: vec![],
        }
    }

    /// Create a chain from pre-built models for testing.
    /// Allows tests to inject mock Llm implementations directly.
    #[allow(dead_code)]
    pub fn from_models_for_test(models: Vec<Arc<dyn Llm>>, model_ids: Vec<String>) -> Self {
        Self { models, model_ids }
    }

    /// The primary (first) model in the chain.
    pub fn primary(&self) -> &Arc<dyn Llm> {
        &self.models[0]
    }

    /// The primary model ID.
    /// Used by logging and diagnostics when reporting fallback usage.
    #[allow(dead_code)]
    pub fn primary_id(&self) -> &str {
        &self.model_ids[0]
    }

    /// Number of models in the chain.
    /// Used by monitoring and diagnostics.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Whether the chain has fallback models (more than one).
    pub fn has_fallbacks(&self) -> bool {
        self.models.len() > 1
    }

    /// All model IDs in the chain.
    /// Used by monitoring and diagnostics.
    #[allow(dead_code)]
    pub fn model_ids(&self) -> &[String] {
        &self.model_ids
    }

    /// Execute an LLM request with automatic fallback on failure.
    /// Returns the successful response and the outcome describing which model was used.
    ///
    /// For single-model chains, this is a direct pass-through with no retry overhead.
    /// For multi-model chains, each model is tried in order until one succeeds.
    pub async fn run_with_fallback<F, Fut, T, E>(
        &self,
        request_fn: F,
    ) -> Result<(T, FallbackOutcome), Vec<(String, String)>>
    where
        F: Fn(Arc<dyn Llm>) -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: std::fmt::Display,
    {
        let mut errors: Vec<(String, String)> = Vec::new();

        for (idx, model) in self.models.iter().enumerate() {
            match request_fn(model.clone()).await {
                Ok(response) => {
                    let outcome = if idx == 0 {
                        FallbackOutcome::PrimarySuccess
                    } else {
                        tracing::warn!(
                            primary = %self.model_ids[0],
                            fallback = %self.model_ids[idx],
                            index = idx,
                            primary_error = %errors[0].1,
                            "using fallback model"
                        );
                        FallbackOutcome::FallbackUsed {
                            primary_id: self.model_ids[0].clone(),
                            fallback_id: self.model_ids[idx].clone(),
                            fallback_index: idx,
                            primary_error: errors[0].1.clone(),
                        }
                    };
                    return Ok((response, outcome));
                }
                Err(e) => {
                    errors.push((self.model_ids[idx].clone(), e.to_string()));
                }
            }
        }

        Err(errors)
    }
}

/// Outcome of a fallback chain attempt.
#[derive(Debug)]
pub enum FallbackOutcome {
    /// Primary model succeeded.
    PrimarySuccess,
    /// Primary failed, a fallback model succeeded.
    FallbackUsed {
        primary_id: String,
        fallback_id: String,
        fallback_index: usize,
        primary_error: String,
    },
    /// All models in the chain failed.
    #[allow(dead_code)]
    AllFailed {
        errors: Vec<(String, String)>, // (model_id, error)
    },
}

impl FallbackOutcome {
    /// Whether the outcome represents a degraded state (fallback was used).
    pub fn is_degraded(&self) -> bool {
        matches!(self, FallbackOutcome::FallbackUsed { .. })
    }

    /// Whether all models failed.
    /// Used by callers to determine if the request should be retried or reported.
    #[allow(dead_code)]
    pub fn is_failed(&self) -> bool {
        matches!(self, FallbackOutcome::AllFailed { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_chain_has_no_fallbacks() {
        // We can't easily create a real Llm in tests without API keys,
        // but we can test the structural methods.
        let chain = FallbackModelChain {
            models: vec![],
            model_ids: vec!["test/model".to_string()],
        };
        assert_eq!(chain.primary_id(), "test/model");
        assert_eq!(chain.len(), 0); // no actual models, just IDs for this test
        assert!(!chain.has_fallbacks());
    }

    #[test]
    fn multi_chain_has_fallbacks() {
        let chain = FallbackModelChain {
            models: vec![],
            model_ids: vec!["a/1".to_string(), "b/2".to_string()],
        };
        assert_eq!(chain.primary_id(), "a/1");
        assert!(chain.model_ids().len() == 2);
    }

    #[test]
    fn resolve_api_key_env_known_provider() {
        assert_eq!(
            model_factory::resolve_api_key_env("anthropic/claude-sonnet-4"),
            "ANTHROPIC_API_KEY"
        );
        assert_eq!(
            model_factory::resolve_api_key_env("openai/gpt-5.5"),
            "OPENAI_API_KEY"
        );
        assert_eq!(
            model_factory::resolve_api_key_env("gemini/gemini-2.5-flash"),
            "GOOGLE_API_KEY"
        );
    }

    #[test]
    fn resolve_api_key_env_no_key_provider() {
        assert_eq!(model_factory::resolve_api_key_env("ollama/llama3"), "");
    }

    #[test]
    fn resolve_api_key_env_unknown_provider() {
        assert_eq!(model_factory::resolve_api_key_env("unknown/model"), "");
    }
}
