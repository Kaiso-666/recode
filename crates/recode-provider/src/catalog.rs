//! Narrow, cacheable view of the models.dev catalog.
//!
//! The upstream document is roughly 10 MB and changes often. `recode models
//! --refresh` downloads it once, keeps only the fields the runtime needs, and
//! writes a compact snapshot next to the database. Configuration loading only
//! ever reads that snapshot.

use crate::registry::Api;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CATALOG_URL: &str = "https://models.opencode.ai/experimental/v2.0/api.json?type=chat";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    #[serde(default)]
    pub fetched_at: Option<u64>,
    #[serde(default)]
    pub providers: BTreeMap<String, CatalogProvider>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogProvider {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub models: Vec<CatalogModel>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub context: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cost_input: f64,
    #[serde(default)]
    pub cost_output: f64,
    #[serde(default)]
    pub cost_cache_read: f64,
    #[serde(default)]
    pub cost_cache_write: f64,
    #[serde(default)]
    pub tool_call: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub attachment: bool,
    #[serde(default)]
    pub routes: Vec<CatalogRoute>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogRoute {
    pub api: Api,
    pub base_url: String,
}

impl Catalog {
    pub fn load(path: impl AsRef<Path>) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| {
                let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
                serde_json::from_slice(bytes).ok()
            })
            .unwrap_or_default()
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create catalog dir: {error}"))?;
        }
        let text = serde_json::to_string(self)
            .map_err(|error| format!("cannot serialize catalog: {error}"))?;
        std::fs::write(path, text).map_err(|error| format!("cannot write catalog: {error}"))
    }

    pub fn provider(&self, id: &str) -> Option<&CatalogProvider> {
        self.providers.get(id)
    }

    pub fn model(&self, provider: &str, model: &str) -> Option<&CatalogModel> {
        self.provider(provider)?
            .models
            .iter()
            .find(|item| item.id == model)
    }

    /// The route for a model, falling back to the provider's first routed
    /// model when the exact id is unknown (for example a hand-written config).
    pub fn route(&self, provider: &str, model: &str) -> Option<&CatalogRoute> {
        let provider = self.provider(provider)?;
        provider
            .models
            .iter()
            .find(|item| item.id == model)
            .and_then(|item| item.routes.first())
            .or_else(|| provider.models.iter().find_map(|item| item.routes.first()))
    }

    pub fn from_models_dev(bytes: &[u8]) -> Result<Self, String> {
        let root: Value = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid models.dev catalog: {error}"))?;
        let object = root
            .as_object()
            .ok_or_else(|| "models.dev catalog is not an object".to_owned())?;
        let mut providers = BTreeMap::new();
        for (id, value) in object {
            let Some(models) = value.get("models").and_then(Value::as_object) else {
                continue;
            };
            let mut parsed = Vec::new();
            for (model_id, model) in models {
                if model.get("type").and_then(Value::as_str).unwrap_or("chat") != "chat" {
                    continue;
                }
                if model.get("status").and_then(Value::as_str) == Some("deprecated") {
                    continue;
                }
                let routes = routes(model.get("api"));
                if routes.is_empty() {
                    continue;
                }
                let limit = model.get("limit");
                let cost = model.pointer("/cost/text");
                parsed.push(CatalogModel {
                    id: model_id.clone(),
                    name: model
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(model_id)
                        .to_owned(),
                    context: number(limit.and_then(|value| value.get("context"))),
                    output: number(limit.and_then(|value| value.get("output"))),
                    cost_input: price(cost.and_then(|value| value.pointer("/input/price"))),
                    cost_output: price(cost.and_then(|value| value.pointer("/output/price"))),
                    cost_cache_read: price(
                        cost.and_then(|value| value.pointer("/input/cache/read")),
                    ),
                    cost_cache_write: price(
                        cost.and_then(|value| value.pointer("/input/cache/write")),
                    ),
                    tool_call: model
                        .pointer("/capabilities/tools/supported")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    reasoning: model
                        .pointer("/capabilities/reasoning/supported")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    attachment: model
                        .pointer("/modalities/input")
                        .and_then(Value::as_array)
                        .is_some_and(|inputs| {
                            inputs
                                .iter()
                                .filter_map(Value::as_str)
                                .any(|kind| kind != "text")
                        }),
                    routes,
                });
            }
            if parsed.is_empty() {
                continue;
            }
            parsed.sort_by_key(|model| model.name.to_lowercase());
            providers.insert(
                id.clone(),
                CatalogProvider {
                    id: id.clone(),
                    name: value
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_owned(),
                    env: string_list(value.get("env")),
                    doc: value.get("doc").and_then(Value::as_str).map(str::to_owned),
                    models: parsed,
                },
            );
        }
        Ok(Self {
            fetched_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_secs()),
            providers,
        })
    }
}

pub async fn fetch() -> Result<Catalog, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|error| format!("cannot build HTTP client: {error}"))?;
    let response = client
        .get(CATALOG_URL)
        .send()
        .await
        .map_err(|error| format!("cannot fetch models.dev catalog: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "models.dev catalog returned HTTP {}",
            response.status()
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("cannot read models.dev catalog: {error}"))?;
    Catalog::from_models_dev(&bytes)
}

fn routes(value: Option<&Value>) -> Vec<CatalogRoute> {
    let Some(api) = value.and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut routes: Vec<(u8, CatalogRoute)> = api
        .iter()
        .filter_map(|(protocol, entry)| {
            let (rank, api) = match protocol.as_str() {
                "chat-completions" => (0, Api::OpenAi),
                "messages" => (1, Api::Anthropic),
                _ => return None,
            };
            let base_url = entry.get("base_url").and_then(Value::as_str)?;
            if base_url.is_empty() {
                return None;
            }
            Some((
                rank,
                CatalogRoute {
                    api,
                    base_url: base_url.trim_end_matches('/').to_owned(),
                },
            ))
        })
        .collect();
    routes.sort_by_key(|(rank, _)| *rank);
    routes.dedup_by_key(|(_, route)| route.api);
    routes.into_iter().map(|(_, route)| route).collect()
}

fn number(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

fn price(value: Option<&Value>) -> f64 {
    value
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(0.0)
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"{
        "acme": {
            "id": "acme",
            "name": "Acme",
            "env": ["ACME_API_KEY"],
            "models": {
                "acme-large": {
                    "id": "acme-large",
                    "type": "chat",
                    "name": "Acme Large",
                    "limit": {"context": 200000, "output": 32000},
                    "cost": {"text": {"input": {"price": 1.5, "cache": {"read": 0.15}}, "output": {"price": 6}}},
                    "capabilities": {"tools": {"supported": true}, "reasoning": {"supported": true}},
                    "modalities": {"input": ["text", "image/png"], "output": ["text"]},
                    "api": {
                        "messages": {"base_url": "https://acme.test/anthropic/v1"},
                        "chat-completions": {"base_url": "https://acme.test/openai/v1"}
                    }
                },
                "acme-image": {
                    "id": "acme-image",
                    "type": "image",
                    "name": "Acme Image",
                    "api": {"images": {"base_url": "https://acme.test/images"}}
                }
            }
        }
    }"#;

    #[test]
    fn parses_only_supported_chat_models() {
        let catalog = Catalog::from_models_dev(SAMPLE).unwrap();
        let provider = catalog.provider("acme").unwrap();
        assert_eq!(provider.env, ["ACME_API_KEY"]);
        assert_eq!(provider.models.len(), 1);
        let model = catalog.model("acme", "acme-large").unwrap();
        assert_eq!(model.context, 200_000);
        assert_eq!(model.output, 32_000);
        assert_eq!(model.cost_input, 1.5);
        assert_eq!(model.cost_cache_read, 0.15);
        assert!(model.tool_call);
        assert!(model.reasoning);
        assert!(model.attachment);
        assert_eq!(model.routes.len(), 2);
        assert_eq!(model.routes[0].api, Api::OpenAi);
        assert_eq!(model.routes[1].api, Api::Anthropic);
    }

    #[test]
    fn route_falls_back_to_the_provider() {
        let catalog = Catalog::from_models_dev(SAMPLE).unwrap();
        let route = catalog.route("acme", "not-a-model").unwrap();
        assert_eq!(route.base_url, "https://acme.test/openai/v1");
    }

    #[test]
    fn cache_round_trips() {
        let catalog = Catalog::from_models_dev(SAMPLE).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        catalog.save(&path).unwrap();
        let loaded = Catalog::load(&path);
        assert_eq!(
            loaded.model("acme", "acme-large").unwrap().name,
            "Acme Large"
        );
    }

    #[test]
    fn accepts_a_utf8_bom() {
        let catalog = Catalog::from_models_dev(SAMPLE).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend(serde_json::to_vec(&catalog).unwrap());
        std::fs::write(&path, bytes).unwrap();
        assert!(Catalog::load(&path).provider("acme").is_some());
    }
}
