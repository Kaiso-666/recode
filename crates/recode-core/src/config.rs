use crate::auth::AuthStore;
use crate::jsonc;
use crate::paths::RecodePaths;
use recode_schema::config::{AgentConfig, Config, ModelConfig, ProviderConfig};
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
        Self {
            paths,
            global,
            project,
            merged,
            auth,
            project_root,
        }
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

    pub fn model_config(&self, model: &ModelRef) -> ModelConfig {
        if let Some(config) = self
            .merged
            .provider
            .get(&model.provider_id)
            .and_then(|provider| provider.models.get(&model.model_id))
        {
            return config.clone();
        }
        ModelConfig {
            name: model.model_id.clone(),
            ..ModelConfig::default()
        }
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
        // Accept a bare model id when it is unique across configured providers.
        let mut matches = Vec::new();
        for (provider_id, provider) in &self.merged.provider {
            if provider.models.contains_key(value) {
                matches.push(ModelRef::new(provider_id.clone(), value));
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
        self.auth.get(provider).map(str::to_owned)
    }

    pub fn has_credential(&self, provider: &str) -> bool {
        if let Some(config) = self.merged.provider.get(provider) {
            if config
                .options
                .api_key
                .as_deref()
                .is_some_and(|key| !key.is_empty())
            {
                return true;
            }
        }
        if self.auth.get(provider).is_some_and(|key| !key.is_empty()) {
            return true;
        }
        if let Some(spec) = recode_provider::registry::spec(provider) {
            if spec.keyless {
                return true;
            }
            if spec.env.iter().any(|name| std::env::var(name).is_ok()) {
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

    pub fn providers(&self) -> Vec<ProviderSummary> {
        let mut output = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for spec in recode_provider::registry::BUILTIN_PROVIDERS {
            seen.insert(spec.id);
            let config = self.merged.provider.get(spec.id);
            let models: Vec<String> = config
                .map(|config| config.models.keys().cloned().collect())
                .unwrap_or_default();
            let models = if models.is_empty() {
                spec.default_model
                    .map(|model| vec![model.to_owned()])
                    .unwrap_or_default()
            } else {
                models
            };
            output.push(ProviderSummary {
                id: spec.id.to_owned(),
                name: config
                    .map(|config| config.name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| spec.name.to_owned()),
                configured: self.has_credential(spec.id),
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
}
