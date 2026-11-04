use crate::{
    anthropic::AnthropicOptions, anthropic::AnthropicProvider, openai::OpenAiOptions,
    openai::OpenAiProvider, Provider,
};
use recode_schema::config::ProviderConfig;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    OpenAi,
    Anthropic,
}

impl Api {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "openai" | "openai-compatible" | "openai_compatible" => Some(Api::OpenAi),
            "anthropic" => Some(Api::Anthropic),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ProviderSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub api: Api,
    pub base_url: &'static str,
    pub env: &'static [&'static str],
    pub default_model: Option<&'static str>,
    /// Providers that need no credential (local servers).
    pub keyless: bool,
}

pub const BUILTIN_PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "anthropic",
        name: "Anthropic",
        api: Api::Anthropic,
        base_url: "https://api.anthropic.com",
        env: &["ANTHROPIC_API_KEY"],
        default_model: Some("claude-sonnet-4-5"),
        keyless: false,
    },
    ProviderSpec {
        id: "openai",
        name: "OpenAI",
        api: Api::OpenAi,
        base_url: "https://api.openai.com/v1",
        env: &["OPENAI_API_KEY"],
        default_model: Some("gpt-5"),
        keyless: false,
    },
    ProviderSpec {
        id: "openrouter",
        name: "OpenRouter",
        api: Api::OpenAi,
        base_url: "https://openrouter.ai/api/v1",
        env: &["OPENROUTER_API_KEY"],
        default_model: None,
        keyless: false,
    },
    ProviderSpec {
        id: "deepseek",
        name: "DeepSeek",
        api: Api::OpenAi,
        base_url: "https://api.deepseek.com/v1",
        env: &["DEEPSEEK_API_KEY"],
        default_model: Some("deepseek-chat"),
        keyless: false,
    },
    ProviderSpec {
        id: "groq",
        name: "Groq",
        api: Api::OpenAi,
        base_url: "https://api.groq.com/openai/v1",
        env: &["GROQ_API_KEY"],
        default_model: Some("llama-3.3-70b-versatile"),
        keyless: false,
    },
    ProviderSpec {
        id: "gemini",
        name: "Google Gemini",
        api: Api::OpenAi,
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        env: &["GEMINI_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"],
        default_model: Some("gemini-2.5-pro"),
        keyless: false,
    },
    ProviderSpec {
        id: "xai",
        name: "xAI",
        api: Api::OpenAi,
        base_url: "https://api.x.ai/v1",
        env: &["XAI_API_KEY"],
        default_model: Some("grok-4"),
        keyless: false,
    },
    ProviderSpec {
        id: "mistral",
        name: "Mistral",
        api: Api::OpenAi,
        base_url: "https://api.mistral.ai/v1",
        env: &["MISTRAL_API_KEY"],
        default_model: None,
        keyless: false,
    },
    ProviderSpec {
        id: "cerebras",
        name: "Cerebras",
        api: Api::OpenAi,
        base_url: "https://api.cerebras.ai/v1",
        env: &["CEREBRAS_API_KEY"],
        default_model: None,
        keyless: false,
    },
    ProviderSpec {
        id: "ollama",
        name: "Ollama",
        api: Api::OpenAi,
        base_url: "http://localhost:11434/v1",
        env: &[],
        default_model: Some("qwen2.5-coder:7b"),
        keyless: true,
    },
    ProviderSpec {
        id: "lmstudio",
        name: "LM Studio",
        api: Api::OpenAi,
        base_url: "http://localhost:1234/v1",
        env: &[],
        default_model: None,
        keyless: true,
    },
];

pub fn spec(id: &str) -> Option<&'static ProviderSpec> {
    BUILTIN_PROVIDERS.iter().find(|spec| spec.id == id)
}

pub struct BuildOptions {
    pub id: String,
    pub api: Api,
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: reqwest::header::HeaderMap,
    pub timeout_ms: Option<u64>,
    pub body_extra: serde_json::Map<String, serde_json::Value>,
    pub default_model: Option<String>,
}

/// Construct a provider from merged config. Returns `None` when the protocol
/// is unknown or a credential is missing for a provider that requires one.
pub fn build(options: BuildOptions) -> Option<Arc<dyn Provider>> {
    match options.api {
        Api::OpenAi => Some(Arc::new(OpenAiProvider::new(OpenAiOptions {
            id: options.id,
            base_url: options.base_url,
            api_key: options.api_key,
            headers: options.headers,
            timeout_ms: options.timeout_ms,
            body_extra: options.body_extra,
            default_model: options.default_model,
        }))),
        Api::Anthropic => {
            let api_key = options.api_key?;
            Some(Arc::new(AnthropicProvider::new(AnthropicOptions {
                id: options.id,
                base_url: options.base_url,
                api_key,
                headers: options.headers,
                timeout_ms: options.timeout_ms,
                default_model: options.default_model,
            })))
        }
    }
}

/// Resolve protocol, base URL and credential for a provider id, preferring
/// explicit config over built-in defaults.
pub fn resolve_spec(
    id: &str,
    config: Option<&ProviderConfig>,
    credential: Option<String>,
) -> Option<(Api, String, Option<String>, bool)> {
    let builtin = spec(id);
    let api = config
        .and_then(|config| config.api.as_deref())
        .and_then(Api::parse)
        .or_else(|| builtin.map(|spec| spec.api))?;
    let base_url = config
        .and_then(|config| config.options.base_url.clone())
        .or_else(|| builtin.map(|spec| spec.base_url.to_owned()))?;
    let keyless = builtin.map(|spec| spec.keyless).unwrap_or(false);
    let env_key =
        builtin.and_then(|spec| spec.env.iter().find_map(|name| std::env::var(name).ok()));
    let api_key = config
        .and_then(|config| config.options.api_key.clone())
        .or(credential)
        .or(env_key);
    Some((api, base_url, api_key, keyless))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_ids_are_unique() {
        let mut ids: Vec<&str> = BUILTIN_PROVIDERS.iter().map(|spec| spec.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BUILTIN_PROVIDERS.len());
    }

    #[test]
    fn local_providers_are_keyless() {
        assert!(spec("ollama").unwrap().keyless);
        assert!(!spec("anthropic").unwrap().keyless);
    }
}
