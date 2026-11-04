use serde::{Deserialize, Serialize};

/// A fully-qualified model reference (`provider/model`), optionally pinned to
/// a variant such as a reasoning effort.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

impl ModelRef {
    pub fn new(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            variant: None,
        }
    }

    pub fn with_variant(mut self, variant: impl Into<String>) -> Self {
        self.variant = Some(variant.into());
        self
    }

    /// Parse `provider/model` or `provider/model#variant`.
    pub fn parse(value: &str) -> Option<Self> {
        let (model, variant) = match value.split_once('#') {
            Some((model, variant)) => (model, Some(variant.to_owned())),
            None => (value, None),
        };
        let (provider_id, model_id) = model.split_once('/')?;
        if provider_id.is_empty() || model_id.is_empty() {
            return None;
        }
        Some(Self {
            provider_id: provider_id.to_owned(),
            model_id: model_id.to_owned(),
            variant,
        })
    }

    pub fn qualified(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

impl std::fmt::Display for ModelRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.provider_id, self.model_id)?;
        if let Some(variant) = &self.variant {
            write!(f, "#{variant}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_qualified_names() {
        let model = ModelRef::parse("anthropic/claude-sonnet-4-5#high").unwrap();
        assert_eq!(model.provider_id, "anthropic");
        assert_eq!(model.model_id, "claude-sonnet-4-5");
        assert_eq!(model.variant.as_deref(), Some("high"));
        assert_eq!(model.to_string(), "anthropic/claude-sonnet-4-5#high");
    }

    #[test]
    fn rejects_unqualified_names() {
        assert!(ModelRef::parse("gpt-4o").is_none());
        assert!(ModelRef::parse("/gpt").is_none());
        assert!(ModelRef::parse("openai/").is_none());
    }
}
