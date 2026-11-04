use crate::permission::{Decision, Rule, Ruleset};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

fn default_context_limit() -> u64 {
    200_000
}

fn default_output_limit() -> u64 {
    32_000
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLimit {
    #[serde(default = "default_context_limit")]
    pub context: u64,
    #[serde(default = "default_output_limit")]
    pub output: u64,
}

impl Default for ModelLimit {
    fn default() -> Self {
        Self {
            context: default_context_limit(),
            output: default_output_limit(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    /// USD per million input tokens.
    #[serde(default)]
    pub input: f64,
    /// USD per million output tokens.
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub attachment: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default = "crate::default_true")]
    pub tool_call: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub limit: ModelLimit,
    #[serde(default)]
    pub cost: ModelCost,
    /// Provider-specific knobs passed through to the adapter.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub options: serde_json::Value,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            attachment: false,
            reasoning: false,
            tool_call: true,
            temperature: None,
            limit: ModelLimit::default(),
            cost: ModelCost::default(),
            options: serde_json::Value::Null,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(default)]
    pub name: String,
    /// SDK package hint retained for config compatibility; unused by the Rust
    /// runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm: Option<String>,
    /// Wire protocol override: `openai`, `anthropic`, or `gemini`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// Environment variables that may supply the credential.
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub options: ProviderOptions,
    #[serde(default)]
    pub models: IndexMap<String, ModelConfig>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            npm: None,
            api: None,
            env: Vec::new(),
            options: ProviderOptions::default(),
            models: IndexMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub headers: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Extra body fields merged into every request.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub body: IndexMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspServerConfig {
    /// Executable and arguments, e.g. `["rust-analyzer"]`.
    #[serde(default)]
    pub command: Vec<String>,
    /// File extensions handled by the server, e.g. `[".rs"]`.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// LSP language id; defaults to the extension without the dot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_id: Option<String>,
    /// Only start the server when one of these files exists in the project.
    #[serde(default)]
    pub root_markers: Vec<String>,
    #[serde(default = "crate::default_true")]
    pub enabled: bool,
    /// Sent as `initializationOptions`.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub initialization: serde_json::Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatterConfig {
    /// Executable and arguments; the file path is appended.
    #[serde(default)]
    pub command: Vec<String>,
    /// File extensions handled by the formatter, e.g. `[".rs"]`.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Formatters are opt-in: absent means disabled.
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    #[serde(default)]
    pub description: String,
    /// `primary`, `subagent`, or `all`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Tool allowlist/denylist; absent means inherit.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub tools: IndexMap<String, bool>,
    #[serde(default)]
    pub permission: PermissionConfig,
    #[serde(default)]
    pub disable: bool,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PermissionValue {
    Action(Decision),
    Map(IndexMap<String, Decision>),
}

/// Raw permission configuration: `{"bash": "allow", "edit": {"*.env": "deny"}}`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermissionConfig(pub IndexMap<String, PermissionValue>);

impl PermissionConfig {
    pub fn to_ruleset(&self) -> Ruleset {
        let mut rules = Vec::new();
        for (permission, value) in &self.0 {
            match value {
                PermissionValue::Action(action) => {
                    rules.push(Rule::new(permission.clone(), "*", *action));
                }
                PermissionValue::Map(map) => {
                    for (pattern, action) in map {
                        rules.push(Rule::new(permission.clone(), pattern.clone(), *action));
                    }
                }
            }
        }
        Ruleset::new(rules)
    }

    pub fn merge(&mut self, other: PermissionConfig) {
        for (key, value) in other.0 {
            match (self.0.get_mut(&key), value) {
                (Some(PermissionValue::Map(existing)), PermissionValue::Map(incoming)) => {
                    existing.extend(incoming);
                }
                (_, value) => {
                    self.0.insert(key, value);
                }
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLocalConfig {
    pub command: Vec<String>,
    #[serde(default)]
    pub environment: IndexMap<String, String>,
    #[serde(default = "crate::default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRemoteConfig {
    pub url: String,
    #[serde(default)]
    pub headers: IndexMap<String, String>,
    #[serde(default = "crate::default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpServerConfig {
    Local(McpLocalConfig),
    Remote(McpRemoteConfig),
}

impl McpServerConfig {
    pub fn enabled(&self) -> bool {
        match self {
            McpServerConfig::Local(config) => config.enabled,
            McpServerConfig::Remote(config) => config.enabled,
        }
    }
}

fn default_true_value() -> bool {
    true
}

/// The merged runtime configuration. Unknown keys are preserved in the
/// `extra` bag so forward-compatible configs do not fail to load.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub small_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_agent: Option<String>,
    #[serde(default)]
    pub provider: IndexMap<String, ProviderConfig>,
    #[serde(default)]
    pub agent: IndexMap<String, AgentConfig>,
    #[serde(default)]
    pub permission: PermissionConfig,
    #[serde(default)]
    pub instructions: Vec<String>,
    #[serde(default)]
    pub mcp: IndexMap<String, McpServerConfig>,
    /// Language servers started on demand. Built-ins cover rust, typescript,
    /// python, go and clang; entries here override or disable them.
    #[serde(default)]
    pub lsp: IndexMap<String, LspServerConfig>,
    /// Optional post-edit formatters. Built-ins are disabled until enabled.
    #[serde(default)]
    pub formatter: IndexMap<String, FormatterConfig>,
    #[serde(default)]
    pub disabled_providers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoupdate: Option<bool>,
    /// Keep a shadow-git snapshot of the worktree before every prompt so
    /// sessions can restore files. Defaults to true when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(default)]
    pub keybinds: serde_json::Value,
    #[serde(default)]
    pub tui: serde_json::Value,
    #[serde(default)]
    pub experimental: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default = "default_true_value")]
    pub telemetry: bool,
    /// Unknown keys from newer config versions.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: None,
            model: None,
            small_model: None,
            default_agent: None,
            provider: IndexMap::new(),
            agent: IndexMap::new(),
            permission: PermissionConfig::default(),
            instructions: Vec::new(),
            mcp: IndexMap::new(),
            lsp: IndexMap::new(),
            formatter: IndexMap::new(),
            disabled_providers: Vec::new(),
            autoupdate: None,
            snapshot: None,
            share: None,
            theme: None,
            keybinds: serde_json::Value::Null,
            tui: serde_json::Value::Null,
            experimental: serde_json::Value::Null,
            username: None,
            telemetry: true,
            extra: serde_json::Map::new(),
        }
    }
}

impl Config {
    /// Merge a higher-precedence config over this one.
    pub fn merge(&mut self, other: Config) {
        if other.model.is_some() {
            self.model = other.model;
        }
        if other.small_model.is_some() {
            self.small_model = other.small_model;
        }
        if other.default_agent.is_some() {
            self.default_agent = other.default_agent;
        }
        if other.autoupdate.is_some() {
            self.autoupdate = other.autoupdate;
        }
        if other.snapshot.is_some() {
            self.snapshot = other.snapshot;
        }
        if other.share.is_some() {
            self.share = other.share;
        }
        if other.theme.is_some() {
            self.theme = other.theme;
        }
        if !other.keybinds.is_null() {
            self.keybinds = other.keybinds;
        }
        if !other.tui.is_null() {
            self.tui = other.tui;
        }
        if !other.experimental.is_null() {
            self.experimental = other.experimental;
        }
        if other.username.is_some() {
            self.username = other.username;
        }
        for (id, provider) in other.provider {
            match self.provider.get_mut(&id) {
                Some(existing) => {
                    if !provider.name.is_empty() {
                        existing.name = provider.name;
                    }
                    if provider.api.is_some() {
                        existing.api = provider.api;
                    }
                    if provider.options.api_key.is_some() {
                        existing.options.api_key = provider.options.api_key;
                    }
                    if provider.options.base_url.is_some() {
                        existing.options.base_url = provider.options.base_url;
                    }
                    existing.options.headers.extend(provider.options.headers);
                    existing.options.body.extend(provider.options.body);
                    existing.models.extend(provider.models);
                }
                None => {
                    self.provider.insert(id, provider);
                }
            }
        }
        for (id, agent) in other.agent {
            match self.agent.get_mut(&id) {
                Some(existing) => {
                    if agent.model.is_some() {
                        existing.model = agent.model;
                    }
                    if agent.prompt.is_some() {
                        existing.prompt = agent.prompt;
                    }
                    if agent.temperature.is_some() {
                        existing.temperature = agent.temperature;
                    }
                    existing.tools.extend(agent.tools);
                    existing.permission.merge(agent.permission.clone());
                    existing.disable |= agent.disable;
                }
                None => {
                    self.agent.insert(id, agent);
                }
            }
        }
        self.permission.merge(other.permission);
        for instruction in other.instructions {
            if !self.instructions.contains(&instruction) {
                self.instructions.push(instruction);
            }
        }
        self.mcp.extend(other.mcp);
        self.lsp.extend(other.lsp);
        self.formatter.extend(other.formatter);
        for provider in other.disabled_providers {
            if !self.disabled_providers.contains(&provider) {
                self.disabled_providers.push(provider);
            }
        }
        if !other.telemetry {
            self.telemetry = false;
        }
        self.extra.extend(other.extra);
    }

    pub fn model_ref(&self) -> Option<crate::model::ModelRef> {
        self.model
            .as_deref()
            .and_then(crate::model::ModelRef::parse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_permission_shorthand_and_maps() {
        let config: Config = serde_json::from_value(serde_json::json!({
            "permission": {
                "bash": "allow",
                "edit": { "*.env": "deny", "src/**": "allow" }
            }
        }))
        .unwrap();
        let rules = config.permission.to_ruleset();
        assert_eq!(rules.evaluate("bash", "ls"), Some(Decision::Allow));
        assert_eq!(rules.evaluate("edit", ".env"), Some(Decision::Deny));
        assert_eq!(rules.evaluate("edit", "src/lib.rs"), Some(Decision::Allow));
    }

    #[test]
    fn config_round_trips() {
        let config = Config::default();
        let json = serde_json::to_value(&config).unwrap();
        let back: Config = serde_json::from_value(json).unwrap();
        assert!(back.model.is_none());
        assert!(back.telemetry);
    }
}
