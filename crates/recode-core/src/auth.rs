use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// OAuth credentials use the same shape as opencode's `auth.json`:
/// `{ "type": "oauth", "access": "...", "refresh": "...", "expires": ms }`.
/// `expires` is milliseconds since the Unix epoch.
#[derive(Clone, Debug, PartialEq)]
pub struct OAuthCredential {
    pub access: String,
    pub refresh: String,
    pub expires: u64,
    pub method: String,
    pub server: Option<String>,
    pub org_id: Option<String>,
}

impl OAuthCredential {
    pub fn expired(&self) -> bool {
        now_ms().saturating_add(60_000) >= self.expires
    }

    pub fn server(&self) -> &str {
        self.server
            .as_deref()
            .unwrap_or("https://opencode.ai/console")
    }
}

#[derive(Clone, Debug)]
pub enum StoredCredential {
    Api(String),
    OAuth(OAuthCredential),
}

impl StoredCredential {
    fn secret(&self) -> &str {
        match self {
            Self::Api(key) => key,
            Self::OAuth(oauth) => &oauth.access,
        }
    }
}

/// Credentials are stored separately from configuration so config files can be
/// committed. The format is compatible with opencode's `auth.json`: provider
/// id → an API key or an OAuth token set.
#[derive(Clone, Debug, Default)]
pub struct AuthStore {
    path: PathBuf,
    entries: IndexMap<String, StoredCredential>,
}

impl AuthStore {
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(parse_entries)
            .unwrap_or_default();
        Self { path, entries }
    }

    /// The access token or API key, whichever this provider uses.
    pub fn get(&self, provider: &str) -> Option<&str> {
        self.entries.get(provider).map(StoredCredential::secret)
    }

    pub fn oauth(&self, provider: &str) -> Option<&OAuthCredential> {
        match self.entries.get(provider)? {
            StoredCredential::OAuth(oauth) => Some(oauth),
            StoredCredential::Api(_) => None,
        }
    }

    pub fn set(&mut self, provider: &str, key: &str) -> Result<(), String> {
        self.entries
            .insert(provider.to_owned(), StoredCredential::Api(key.to_owned()));
        self.save()
    }

    pub fn set_oauth(&mut self, provider: &str, credential: OAuthCredential) -> Result<(), String> {
        self.entries
            .insert(provider.to_owned(), StoredCredential::OAuth(credential));
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
            .map(|(provider, credential)| (provider.as_str(), !credential.secret().is_empty()))
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create config dir: {error}"))?;
        }
        let mut object = Map::new();
        for (provider, credential) in &self.entries {
            object.insert(provider.clone(), credential_json(credential));
        }
        let text = serde_json::to_string_pretty(&Value::Object(object))
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

fn credential_json(credential: &StoredCredential) -> Value {
    match credential {
        StoredCredential::Api(key) => json!({"type": "api", "key": key}),
        StoredCredential::OAuth(oauth) => {
            let mut object = json!({
                "type": "oauth",
                "methodID": oauth.method,
                "access": oauth.access,
                "refresh": oauth.refresh,
                "expires": oauth.expires,
            });
            let mut metadata = Map::new();
            if let Some(server) = &oauth.server {
                metadata.insert("server".into(), Value::String(server.clone()));
            }
            if let Some(org_id) = &oauth.org_id {
                metadata.insert("orgID".into(), Value::String(org_id.clone()));
            }
            if !metadata.is_empty() {
                object["metadata"] = Value::Object(metadata);
            }
            object
        }
    }
}

fn parse_entries(value: Value) -> IndexMap<String, StoredCredential> {
    let mut entries = IndexMap::new();
    let Some(object) = value.as_object() else {
        return entries;
    };
    for (provider, credential) in object {
        let Some(parsed) = parse_credential(credential) else {
            continue;
        };
        entries.insert(provider.clone(), parsed);
    }
    entries
}

fn parse_credential(value: &Value) -> Option<StoredCredential> {
    if let Some(key) = value.as_str() {
        return (!key.is_empty()).then(|| StoredCredential::Api(key.to_owned()));
    }
    let object = value.as_object()?;
    let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
    let access = object.get("access").and_then(Value::as_str);
    let refresh = object.get("refresh").and_then(Value::as_str);
    if kind == "oauth" || (access.is_some() && refresh.is_some()) {
        let access = access?;
        if access.is_empty() {
            return None;
        }
        let metadata = object.get("metadata").and_then(Value::as_object);
        return Some(StoredCredential::OAuth(OAuthCredential {
            access: access.to_owned(),
            refresh: refresh.unwrap_or_default().to_owned(),
            expires: object.get("expires").and_then(Value::as_u64).unwrap_or(0),
            method: object
                .get("methodID")
                .or_else(|| object.get("method"))
                .and_then(Value::as_str)
                .unwrap_or("device")
                .to_owned(),
            server: metadata
                .and_then(|metadata| metadata.get("server"))
                .or_else(|| object.get("server"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            org_id: metadata
                .and_then(|metadata| metadata.get("orgID"))
                .or_else(|| object.get("orgID"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        }));
    }
    let key = object
        .get("key")
        .and_then(Value::as_str)
        .or(access)
        .filter(|key| !key.is_empty())?;
    Some(StoredCredential::Api(key.to_owned()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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
            r#"{
                "anthropic": {"type": "api", "key": "sk-test"},
                "local": "plain",
                "opencode": {
                    "type": "oauth",
                    "access": "access-token",
                    "refresh": "refresh-token",
                    "expires": 1234567890,
                    "metadata": {"server": "https://console.test", "orgID": "org-1"}
                }
            }"#,
        )
        .unwrap();
        let store = AuthStore::load(&path);
        assert_eq!(store.get("anthropic"), Some("sk-test"));
        assert_eq!(store.get("local"), Some("plain"));
        assert_eq!(store.get("opencode"), Some("access-token"));
        let oauth = store.oauth("opencode").unwrap();
        assert_eq!(oauth.refresh, "refresh-token");
        assert_eq!(oauth.server(), "https://console.test");
        assert_eq!(oauth.org_id.as_deref(), Some("org-1"));
        assert!(oauth.expired());
    }

    #[test]
    fn writes_and_reloads_api_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let mut store = AuthStore::load(&path);
        store.set("openai", "sk-openai").unwrap();
        let store = AuthStore::load(&path);
        assert_eq!(store.get("openai"), Some("sk-openai"));
    }

    #[test]
    fn writes_and_reloads_oauth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let mut store = AuthStore::load(&path);
        store
            .set_oauth(
                "opencode",
                OAuthCredential {
                    access: "access".into(),
                    refresh: "refresh".into(),
                    expires: now_ms() + 3_600_000,
                    method: "device".into(),
                    server: Some("https://console.test".into()),
                    org_id: Some("org".into()),
                },
            )
            .unwrap();
        let store = AuthStore::load(&path);
        let oauth = store.oauth("opencode").unwrap();
        assert_eq!(oauth.access, "access");
        assert_eq!(oauth.refresh, "refresh");
        assert!(!oauth.expired());
        assert_eq!(store.get("opencode"), Some("access"));
    }
}
