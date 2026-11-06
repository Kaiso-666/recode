use crate::auth::{AuthStore, OAuthCredential};
use crate::config::ConfigManager;
use crate::oauth;
use parking_lot::Mutex;
use recode_provider::registry::{self, Api, BuildOptions};
use recode_provider::Provider;
use recode_schema::model::ModelRef;
use std::collections::HashMap;
use std::sync::Arc;

type CacheKey = (String, String);

/// Lazily-built provider clients, keyed by `(provider, model)`. Configuration
/// is immutable for an engine, so a built client can be cached forever. The
/// model is part of the key because models.dev can route two models of the
/// same provider through different protocols or base URLs.
pub struct Providers {
    config: Arc<ConfigManager>,
    cache: Mutex<HashMap<CacheKey, Arc<dyn Provider>>>,
    auth: Mutex<AuthStore>,
    refresh: tokio::sync::Mutex<()>,
}

impl Providers {
    pub fn new(config: Arc<ConfigManager>) -> Self {
        let auth = AuthStore::load(&config.paths.auth_path);
        Self {
            config,
            cache: Mutex::new(HashMap::new()),
            auth: Mutex::new(auth),
            refresh: tokio::sync::Mutex::new(()),
        }
    }

    /// Build using the provider's default route; callers that know the model
    /// should prefer [`Providers::get_for`].
    pub async fn get(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        self.build(provider_id, "").await
    }

    pub async fn get_for(&self, model: &ModelRef) -> Result<Arc<dyn Provider>, String> {
        self.build(&model.provider_id, &model.model_id).await
    }

    async fn build(&self, provider_id: &str, model_id: &str) -> Result<Arc<dyn Provider>, String> {
        let key = (provider_id.to_owned(), model_id.to_owned());
        if let Some(provider) = self.cache.lock().get(&key) {
            return Ok(provider.clone());
        }
        let provider_config = self.config.provider(provider_id);
        let (api, base_url) = self.config.route(provider_id, model_id).ok_or_else(|| {
            format!(
                "unknown provider {provider_id:?}. Add it under \"provider\" in recode.json, \
                 refresh the catalog with `recode models --refresh`, or use a built-in provider."
            )
        })?;
        let mut api_key = self.config.configured_key(provider_id);
        if api_key.is_none() {
            let oauth = self.auth.lock().oauth(provider_id).cloned();
            api_key = match oauth {
                Some(oauth) => Some(self.token(provider_id, oauth).await?),
                None => self.auth.lock().get(provider_id).map(str::to_owned),
            };
        }
        if api_key.is_none() {
            api_key = registry::spec(provider_id)
                .and_then(registry::free_key)
                .map(str::to_owned);
        }
        let keyless = registry::spec(provider_id).is_some_and(|spec| spec.keyless);
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
            .or_else(|| (!model_id.is_empty()).then(|| model_id.to_owned()))
            .or_else(|| {
                registry::spec(provider_id)
                    .and_then(|spec| spec.default_model)
                    .map(str::to_owned)
            })
            .or_else(|| {
                self.config
                    .catalog
                    .provider(provider_id)
                    .and_then(|provider| provider.models.first())
                    .map(|model| model.id.clone())
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
        self.cache.lock().insert(key, provider.clone());
        Ok(provider)
    }

    async fn token(&self, provider: &str, oauth: OAuthCredential) -> Result<String, String> {
        if !oauth.expired() {
            return Ok(oauth.access);
        }
        if oauth.refresh.is_empty() {
            return Err(format!(
                "the {provider:?} session expired. Run `recode auth login {provider}` again."
            ));
        }
        // Serialize refreshes so two concurrent steps cannot rotate the same
        // refresh token twice.
        let _refresh = self.refresh.lock().await;
        let current = self.auth.lock().oauth(provider).cloned().unwrap_or(oauth);
        if !current.expired() {
            return Ok(current.access);
        }
        let updated = oauth::refresh(current.server(), &current.refresh).await?;
        self.auth
            .lock()
            .set_oauth(provider, updated.clone())
            .map_err(|error| format!("cannot store refreshed credential: {error}"))?;
        Ok(updated.access)
    }
}
