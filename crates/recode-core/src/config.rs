use crate::auth::AuthStore;
use crate::jsonc;
use crate::paths::RecodePaths;
use recode_provider::catalog::{Catalog, CatalogRoute};
use recode_provider::registry::{self, Api};
use recode_schema::config::{
    AgentConfig, Config, ModelConfig, ModelCost, ModelLimit, ProviderConfig,
};
use recode_schema::model::ModelRef;
use recode_schema::permission::Ruleset;
use std::path::{Path, PathBuf};

/// Loaded configuration plus credentials. Immutable for the lifetime of an
/// engine; `recode` restarts config on explicit reload.
#[derive(Clone)]
pub struct ConfigManager {
    pub paths: RecodePaths,
    pub global: Config,
    pub project: Config,
    pub merged: Config,
    pub auth: AuthStore,
    pub catalog: Catalog,
    pub project_root: PathBuf,
}

impl ConfigManager {
    pub fn load(paths: RecodePaths, project_root: impl AsRef<Path>) -> Self {
        let project_root = project_root.as_ref().to_path_buf();
        let global = load_file(&paths.global_config);
        let mut project = Config::default();
        for name in ["recode.json", "recode.local.json"] {
            let path = project_root.join(name);
            let loaded = load_file(&path);
            project.merge(loaded);
        }
        let mut merged = Config::default();
        merged.merge(global.clone());
        merged.merge(project.clone());
        if let Ok(model) = std::env::var("RECODE_MODEL") {
            if !model.trim().is_empty() {
                merged.model = Some(model);
            }
        }
        let auth = AuthStore::load(&paths.auth_path);
        let catalog = Catalog::load(paths.data_dir.join("models.json"));
        Self {
            paths,
            global,
            project,
            merged,
            auth,
            catalog,
            project_root,
        }
    }

    /// Load configuration and replace the catalog in one step, used after
    /// `recode models --refresh` writes a fresh snapshot.
    pub fn reload_catalog(&mut self) {
        self.catalog = Catalog::load(self.paths.data_dir.join("models.json"));
    }

    pub fn ruleset(&self) -> Ruleset {
        self.merged.permission.to_ruleset()
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.merged.provider.get(id)
    }

    pub fn agent(&self, id: &str) -> Option<&AgentConfig> {
        self.merged.agent.get(id)
    }

    pub fn default_agent(&self) -> String {
        self.merged
            .default_agent
            .clone()
            .unwrap_or_else(|| "build".to_owned())
    }

    /// The configured TUI theme name, if any.
    pub fn theme_name(&self) -> Option<&str> {
        self.merged.theme.as_deref()
    }

    pub fn model_config(&self, model: &ModelRef) -> ModelConfig {
        if let Some(config) = self
            .merged
            .provider
            .get(&model.provider_id)
            .and_then(|provider| provider.models.get(&model.model_id))
        {
            return config.clone();
        }
        if let Some(entry) = self.catalog.model(&model.provider_id, &model.model_id) {
            let defaults = ModelLimit::default();
            return ModelConfig {
                name: if entry.name.is_empty() {
                    model.model_id.clone()
                } else {
                    entry.name.clone()
                },
                attachment: entry.attachment,
                reasoning: entry.reasoning,
                tool_call: entry.tool_call,
                temperature: None,
                limit: ModelLimit {
                    context: if entry.context == 0 {
                        defaults.context
                    } else {
                        entry.context
                    },
                    output: if entry.output == 0 {
                        defaults.output
                    } else {
                        entry.output
                    },
                },
                cost: ModelCost {
                    input: entry.cost_input,
                    output: entry.cost_output,
                    cache_read: entry.cost_cache_read,
                    cache_write: entry.cost_cache_write,
                },
                options: serde_json::Value::Null,
            };
        }
        ModelConfig {
            name: model.model_id.clone(),
            ..ModelConfig::default()
        }
    }

    /// Resolve the wire protocol and endpoint for a provider/model pair.
    /// Explicit configuration wins, then built-in defaults, then models.dev.
    pub fn route(&self, provider: &str, model: &str) -> Option<(Api, String)> {
        let config = self.merged.provider.get(provider);
        let builtin = registry::spec(provider);
        let catalog = self.catalog.route(provider, model);
        let api = config
            .and_then(|config| config.api.as_deref())
            .and_then(Api::parse)
            .or_else(|| builtin.map(|spec| spec.api))
            .or_else(|| catalog.map(|route| route.api))?;
        let base_url = config
            .and_then(|config| config.options.base_url.clone())
            .filter(|url| !url.is_empty())
            .or_else(|| builtin.map(|spec| spec.base_url.to_owned()))
            .or_else(|| catalog.map(|route| route.base_url.clone()))?;
        Some((api, base_url))
    }

    pub fn catalog_route(&self, provider: &str, model: &str) -> Option<&CatalogRoute> {
        self.catalog.route(provider, model)
    }

    /// Environment variables that may hold a credential, in precedence order.
    pub fn provider_env(&self, provider: &str) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(config) = self.merged.provider.get(provider) {
            names.extend(config.env.iter().cloned());
        }
        if let Some(spec) = registry::spec(provider) {
            names.extend(spec.env.iter().map(|name| (*name).to_owned()));
        }
        if let Some(entry) = self.catalog.provider(provider) {
            names.extend(entry.env.iter().cloned());
        }
        let mut unique = Vec::with_capacity(names.len());
        for name in names {
            if !unique.contains(&name) {
                unique.push(name);
            }
        }
        unique
    }

    /// A credential from config or the environment, without the auth store or
    /// the free-tier fallback.
    pub fn configured_key(&self, provider: &str) -> Option<String> {
        if let Some(key) = self
            .merged
            .provider
            .get(provider)
            .and_then(|config| config.options.api_key.clone())
            .filter(|key| !key.is_empty())
        {
            return Some(key);
        }
        for name in self.provider_env(provider) {
            if let Ok(value) = std::env::var(&name) {
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
        None
    }

    /// A credential the user supplied, without the free-tier fallback.
    pub fn explicit_key(&self, provider: &str) -> Option<String> {
        self.configured_key(provider)
            .or_else(|| self.auth.get(provider).map(str::to_owned))
    }

    /// The credential used for a provider: explicit config, the auth store,
    /// an environment variable, then a provider's free-tier key.
    pub fn provider_key(&self, provider: &str) -> Option<String> {
        self.explicit_key(provider).or_else(|| {
            registry::spec(provider)
                .and_then(registry::free_key)
                .map(str::to_owned)
        })
    }

    /// Resolve the model to use, in precedence order:
    /// explicit argument → session (caller) → config → first usable provider.
    pub fn resolve_model(&self, explicit: Option<&str>) -> Result<ModelRef, String> {
        if let Some(explicit) = explicit.filter(|value| !value.trim().is_empty()) {
            return self.parse_model(explicit);
        }
        if let Some(model) = &self.merged.model {
            return self.parse_model(model);
        }
        for (id, provider) in &self.merged.provider {
            if let Some((model, _)) = provider.models.first() {
                return Ok(ModelRef::new(id.clone(), model.clone()));
            }
        }
        let defaults = [
            ("anthropic", "claude-sonnet-4-5"),
            ("openai", "gpt-5"),
            ("deepseek", "deepseek-chat"),
            ("gemini", "gemini-2.5-pro"),
            ("opencode", "big-pickle"),
            ("ollama", "qwen2.5-coder:7b"),
        ];
        for (provider, model) in defaults {
            if self.has_credential(provider) {
                return Ok(ModelRef::new(provider, model));
            }
        }
        Err("no model configured. Run `recode auth login <provider>` or set \"model\" in recode.json".to_owned())
    }

    fn parse_model(&self, value: &str) -> Result<ModelRef, String> {
        if let Some(model) = ModelRef::parse(value) {
            return Ok(model);
        }
        // Accept a bare model id when it is unique across configured providers
        // or a built-in provider's advertised models.
        let mut matches = Vec::new();
        for (provider_id, provider) in &self.merged.provider {
            if provider.models.contains_key(value) {
                matches.push(ModelRef::new(provider_id.clone(), value));
            }
        }
        for spec in recode_provider::registry::BUILTIN_PROVIDERS {
            let known = spec.default_model == Some(value) || spec.models.contains(&value);
            if known && !matches.iter().any(|model| model.provider_id == spec.id) {
                matches.push(ModelRef::new(spec.id, value));
            }
        }
        for provider in self.catalog.providers.values() {
            let known = provider.models.iter().any(|model| model.id == value);
            if known && !matches.iter().any(|model| model.provider_id == provider.id) {
                matches.push(ModelRef::new(provider.id.clone(), value));
            }
        }
        match matches.len() {
            1 => Ok(matches.remove(0)),
            0 => Err(format!(
                "invalid model {value:?}: expected provider/model (for example anthropic/claude-sonnet-4-5)"
            )),
            _ => Err(format!(
                "model {value:?} exists in multiple providers; qualify it with a provider id"
            )),
        }
    }

    pub fn credential(&self, provider: &str) -> Option<String> {
        self.provider_key(provider)
    }

    pub fn has_credential(&self, provider: &str) -> bool {
        if self.provider_key(provider).is_some() {
            return true;
        }
        if let Some(spec) = recode_provider::registry::spec(provider) {
            if spec.keyless {
                return true;
            }
        }
        // Custom providers may be keyless (local endpoints).
        self.merged
            .provider
            .get(provider)
            .is_some_and(|config| config.api.is_some() && config.options.base_url.is_some())
            && self
                .merged
                .provider
                .get(provider)
                .is_some_and(|config| config.options.api_key.is_none())
    }

    /// Models advertised for a provider, optionally narrowed to the free set
    /// when the user has no credential. Opencode disables paid models in that
    /// state; we do the same and keep the built-in list as a fallback.
    fn catalog_models(&self, provider: &str, configured: bool) -> Vec<String> {
        let Some(entry) = self.catalog.provider(provider) else {
            return Vec::new();
        };
        entry
            .models
            .iter()
            .filter(|model| configured || is_free(model))
            .map(|model| model.id.clone())
            .collect()
    }

    pub fn providers(&self) -> Vec<ProviderSummary> {
        let mut output = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for spec in recode_provider::registry::BUILTIN_PROVIDERS {
            seen.insert(spec.id.to_owned());
            let config = self.merged.provider.get(spec.id);
            let configured = self.has_credential(spec.id);
            let paid_access = self.explicit_key(spec.id).is_some() || spec.keyless;
            let configured_models: Vec<String> = config
                .map(|config| config.models.keys().cloned().collect())
                .unwrap_or_default();
            let catalog_models = self.catalog_models(spec.id, paid_access);
            let models = if !configured_models.is_empty() {
                configured_models
            } else if !catalog_models.is_empty() {
                catalog_models
            } else if spec.models.is_empty() {
                spec.default_model
                    .map(|model| vec![model.to_owned()])
                    .unwrap_or_default()
            } else {
                spec.models
                    .iter()
                    .map(|model| (*model).to_owned())
                    .collect()
            };
            output.push(ProviderSummary {
                id: spec.id.to_owned(),
                name: config
                    .map(|config| config.name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| spec.name.to_owned()),
                configured,
                models,
            });
        }
        for (id, provider) in &self.catalog.providers {
            if seen.contains(id.as_str()) {
                continue;
            }
            seen.insert(id.clone());
            let config = self.merged.provider.get(id);
            let configured = self.has_credential(id);
            let configured_models: Vec<String> = config
                .map(|config| config.models.keys().cloned().collect())
                .unwrap_or_default();
            let models = if configured_models.is_empty() {
                provider
                    .models
                    .iter()
                    .map(|model| model.id.clone())
                    .collect()
            } else {
                configured_models
            };
            output.push(ProviderSummary {
                id: id.clone(),
                name: config
                    .map(|config| config.name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| provider.name.clone()),
                configured,
                models,
            });
        }
        for (id, config) in &self.merged.provider {
            if seen.contains(id.as_str()) {
                continue;
            }
            output.push(ProviderSummary {
                id: id.clone(),
                name: if config.name.is_empty() {
                    id.clone()
                } else {
                    config.name.clone()
                },
                configured: self.has_credential(id),
                models: config.models.keys().cloned().collect(),
            });
        }
        output
    }
}

fn is_free(model: &recode_provider::catalog::CatalogModel) -> bool {
    model.cost_input == 0.0 && model.cost_output == 0.0
}

#[derive(Clone, Debug)]
pub struct ProviderSummary {
    pub id: String,
    pub name: String,
    pub configured: bool,
    pub models: Vec<String>,
}

fn load_file(path: &Path) -> Config {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Config::default();
    };
    match jsonc::parse(&text) {
        Ok(value) => match serde_json::from_value::<Config>(value) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "invalid config; ignoring");
                Config::default()
            }
        },
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "invalid JSONC; ignoring");
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_theme_name_from_the_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path().join("project");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::write(
            project_root.join("recode.json"),
            r#"{ "theme": "gruvbox" }"#,
        )
        .unwrap();
        let paths = RecodePaths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            db_path: dir.path().join("data/recode.db"),
            auth_path: dir.path().join("config/auth.json"),
            log_dir: dir.path().join("data/log"),
            global_config: dir.path().join("config/recode.json"),
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        let manager = ConfigManager::load(paths, &project_root);
        assert_eq!(manager.theme_name(), Some("gruvbox"));
    }

    #[test]
    fn project_config_overrides_global() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path().join("project");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::write(
            project_root.join("recode.json"),
            r#"{ "model": "openai/gpt-5", "permission": { "bash": "allow" } }"#,
        )
        .unwrap();
        let paths = RecodePaths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            db_path: dir.path().join("data/recode.db"),
            auth_path: dir.path().join("config/auth.json"),
            log_dir: dir.path().join("data/log"),
            global_config: dir.path().join("config/recode.json"),
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(
            &paths.global_config,
            r#"{ "model": "anthropic/claude-sonnet-4-5" }"#,
        )
        .unwrap();
        let manager = ConfigManager::load(paths, &project_root);
        assert_eq!(
            manager.resolve_model(None).unwrap().qualified(),
            "openai/gpt-5"
        );
        assert!(manager
            .ruleset()
            .evaluate("bash", "ls")
            .is_some_and(|decision| decision == recode_schema::permission::Decision::Allow));
    }

    #[test]
    fn catalog_providers_add_routes_and_model_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path().join("project");
        std::fs::create_dir_all(&project_root).unwrap();
        let paths = RecodePaths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            db_path: dir.path().join("data/recode.db"),
            auth_path: dir.path().join("config/auth.json"),
            log_dir: dir.path().join("data/log"),
            global_config: dir.path().join("config/recode.json"),
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        let catalog = recode_provider::catalog::Catalog::from_models_dev(
            br#"{
                "acme": {
                    "name": "Acme",
                    "env": ["ACME_API_KEY"],
                    "models": {
                        "acme-large": {
                            "type": "chat",
                            "name": "Acme Large",
                            "limit": {"context": 123456, "output": 4096},
                            "cost": {"text": {"input": {"price": 2}, "output": {"price": 8}}},
                            "api": {"chat-completions": {"base_url": "https://acme.test/openai/v1"}}
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        catalog.save(paths.data_dir.join("models.json")).unwrap();
        let manager = ConfigManager::load(paths, &project_root);
        let acme = manager
            .providers()
            .into_iter()
            .find(|provider| provider.id == "acme")
            .expect("catalog provider");
        assert_eq!(acme.name, "Acme");
        assert!(acme.models.contains(&"acme-large".to_owned()));
        assert!(!acme.configured);
        assert_eq!(
            manager.route("acme", "acme-large"),
            Some((Api::OpenAi, "https://acme.test/openai/v1".to_owned()))
        );
        let model = manager.resolve_model(Some("acme-large")).unwrap();
        assert_eq!(model.qualified(), "acme/acme-large");
        let config = manager.model_config(&model);
        assert_eq!(config.limit.context, 123_456);
        assert_eq!(config.limit.output, 4096);
        assert_eq!(config.cost.input, 2.0);
        assert_eq!(config.cost.output, 8.0);
    }

    #[test]
    fn paid_catalog_models_need_an_explicit_key() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path().join("project");
        std::fs::create_dir_all(&project_root).unwrap();
        let paths = RecodePaths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            db_path: dir.path().join("data/recode.db"),
            auth_path: dir.path().join("config/auth.json"),
            log_dir: dir.path().join("data/log"),
            global_config: dir.path().join("config/recode.json"),
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        let catalog = recode_provider::catalog::Catalog::from_models_dev(
            br#"{
                "opencode": {
                    "name": "OpenCode Zen",
                    "models": {
                        "free-model": {
                            "type": "chat",
                            "name": "Free",
                            "cost": {"text": {"input": {"price": 0}, "output": {"price": 0}}},
                            "api": {"chat-completions": {"base_url": "https://opencode.test/v1"}}
                        },
                        "paid-model": {
                            "type": "chat",
                            "name": "Paid",
                            "cost": {"text": {"input": {"price": 2}, "output": {"price": 8}}},
                            "api": {"chat-completions": {"base_url": "https://opencode.test/v1"}}
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        catalog.save(paths.data_dir.join("models.json")).unwrap();
        let manager = ConfigManager::load(paths.clone(), &project_root);
        let models = manager
            .providers()
            .into_iter()
            .find(|provider| provider.id == "opencode")
            .unwrap()
            .models;
        assert_eq!(models, ["free-model"]);

        std::fs::write(
            &paths.global_config,
            r#"{"provider":{"opencode":{"options":{"apiKey":"test"}}}}"#,
        )
        .unwrap();
        let manager = ConfigManager::load(paths, &project_root);
        let models = manager
            .providers()
            .into_iter()
            .find(|provider| provider.id == "opencode")
            .unwrap()
            .models;
        assert!(models.contains(&"paid-model".to_owned()));
    }

    #[test]
    fn opencode_free_models_are_listed_and_resolvable() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path().join("project");
        std::fs::create_dir_all(&project_root).unwrap();
        let paths = RecodePaths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            db_path: dir.path().join("data/recode.db"),
            auth_path: dir.path().join("config/auth.json"),
            log_dir: dir.path().join("data/log"),
            global_config: dir.path().join("config/recode.json"),
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        let manager = ConfigManager::load(paths, &project_root);
        let opencode = manager
            .providers()
            .into_iter()
            .find(|provider| provider.id == "opencode")
            .expect("opencode provider");
        assert!(opencode.configured);
        assert!(opencode.models.contains(&"big-pickle".to_owned()));
        assert_eq!(
            manager
                .resolve_model(Some("big-pickle"))
                .unwrap()
                .qualified(),
            "opencode/big-pickle"
        );
    }
}
