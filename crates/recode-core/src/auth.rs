use indexmap::IndexMap;
use std::path::{Path, PathBuf};

/// Credentials are stored separately from configuration so config files can be
/// committed. The format is intentionally compatible with opencode's
/// `auth.json`: provider id → `{ "type": "api", "key": "..." }`.
#[derive(Clone, Debug, Default)]
pub struct AuthStore {
    path: PathBuf,
    entries: IndexMap<String, String>,
}

impl AuthStore {
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .map(parse_entries)
            .unwrap_or_default();
        Self { path, entries }
    }

    pub fn get(&self, provider: &str) -> Option<&str> {
        self.entries.get(provider).map(String::as_str)
    }

    pub fn set(&mut self, provider: &str, key: &str) -> Result<(), String> {
        self.entries.insert(provider.to_owned(), key.to_owned());
        self.save()
    }

    pub fn remove(&mut self, provider: &str) -> Result<bool, String> {
        let removed = self.entries.shift_remove(provider).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn list(&self) -> impl Iterator<Item = (&str, bool)> {
        self.entries
            .iter()
            .map(|(provider, key)| (provider.as_str(), !key.is_empty()))
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create config dir: {error}"))?;
        }
        let mut object = serde_json::Map::new();
        for (provider, key) in &self.entries {
            object.insert(
                provider.clone(),
                serde_json::json!({"type": "api", "key": key}),
            );
        }
        let text = serde_json::to_string_pretty(&serde_json::Value::Object(object))
            .map_err(|error| format!("cannot serialize credentials: {error}"))?;
        std::fs::write(&self.path, text)
            .map_err(|error| format!("cannot write auth store: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn parse_entries(value: serde_json::Value) -> IndexMap<String, String> {
    let mut entries = IndexMap::new();
    let Some(object) = value.as_object() else {
        return entries;
    };
    for (provider, credential) in object {
        let key = credential
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                credential
                    .get("key")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| {
                credential
                    .get("access")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            });
        if let Some(key) = key {
            entries.insert(provider.clone(), key);
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_opencode_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{"anthropic":{"type":"api","key":"sk-test"},"local":"plain"}"#,
        )
        .unwrap();
        let store = AuthStore::load(&path);
        assert_eq!(store.get("anthropic"), Some("sk-test"));
        assert_eq!(store.get("local"), Some("plain"));
    }

    #[test]
    fn writes_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let mut store = AuthStore::load(&path);
        store.set("openai", "sk-openai").unwrap();
        let store = AuthStore::load(&path);
        assert_eq!(store.get("openai"), Some("sk-openai"));
    }
}
