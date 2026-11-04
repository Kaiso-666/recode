use crate::config::ConfigManager;
use parking_lot::Mutex;
use recode_provider::registry::{self, Api, BuildOptions};
use recode_provider::Provider;
use std::collections::HashMap;
use std::sync::Arc;

/// Lazily-built provider clients, keyed by provider id. Configuration is
/// immutable for an engine, so a built client can be cached forever.
pub struct Providers {
    config: Arc<ConfigManager>,
    cache: Mutex<HashMap<String, Arc<dyn Provider>>>,
}

impl Providers {
    pub fn new(config: Arc<ConfigManager>) -> Self {
        Self {
            config,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        if let Some(provider) = self.cache.lock().get(provider_id) {
            return Ok(provider.clone());
        }
        let provider_config = self.config.provider(provider_id);
        let credential = self.config.credential(provider_id);
        let (api, base_url, api_key, keyless) =
            registry::resolve_spec(provider_id, provider_config, credential).ok_or_else(|| {
                format!(
                    "unknown provider {provider_id:?}. Add it under \"provider\" in recode.json or use a built-in provider."
                )
            })?;
        if api == Api::Anthropic && api_key.as_deref().unwrap_or("").is_empty() {
            return Err(format!(
                "provider {provider_id:?} has no API key. Run `recode auth login {provider_id}` or set the provider's env variable."
            ));
        }
        if !keyless && api_key.as_deref().unwrap_or("").is_empty() && api == Api::OpenAi {
            // OpenAI-compatible endpoints can be keyless (local); remote servers
            // will return 401, which is a clearer error than refusing locally.
        }
        let mut headers = reqwest::header::HeaderMap::new();
        let mut body_extra = serde_json::Map::new();
        let mut default_model = provider_config
            .and_then(|config| config.models.keys().next().cloned())
            .or_else(|| {
                registry::spec(provider_id)
                    .and_then(|spec| spec.default_model)
                    .map(str::to_owned)
            });
        let mut timeout_ms = None;
        if let Some(config) = provider_config {
            if let Some(model) = config.models.keys().next() {
                default_model = Some(model.clone());
            }
            timeout_ms = config.options.timeout_ms;
            body_extra.extend(config.options.body.clone());
            for (name, value) in &config.options.headers {
                let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|error| format!("invalid header name {name:?}: {error}"))?;
                let value = reqwest::header::HeaderValue::from_str(value)
                    .map_err(|error| format!("invalid header value for {name}: {error}"))?;
                headers.insert(name, value);
            }
        }
        let provider = registry::build(BuildOptions {
            id: provider_id.to_owned(),
            api,
            base_url,
            api_key,
            headers,
            timeout_ms,
            body_extra,
            default_model,
        })
        .ok_or_else(|| format!("provider {provider_id:?} is misconfigured"))?;
        self.cache
            .lock()
            .insert(provider_id.to_owned(), provider.clone());
        Ok(provider)
    }
}
