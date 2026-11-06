//! OpenCode Console device authorization and token refresh.
//!
//! The device flow is the same one `opencode auth login opencode` uses:
//! request a code, show it, poll the token endpoint, then store the OAuth
//! credential. Paid Zen models use the access token; free models do not need
//! an account at all.

use crate::auth::OAuthCredential;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub const DEFAULT_SERVER: &str = "https://opencode.ai/console";
pub const CLIENT_ID: &str = "opencode-cli";

#[derive(Clone, Debug)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

pub async fn device_code(server: &str) -> Result<DeviceCode, String> {
    let value = post(
        server,
        "/auth/device/code",
        json!({"client_id": CLIENT_ID, "supports_org_scope": true}),
    )
    .await?;
    Ok(DeviceCode {
        device_code: string(&value, "device_code")?,
        user_code: string(&value, "user_code")?,
        verification_uri_complete: string(&value, "verification_uri_complete")?,
        expires_in: value
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(600),
        interval: value
            .get("interval")
            .and_then(Value::as_u64)
            .unwrap_or(5)
            .max(1),
    })
}

/// Poll until the user approves the code. Handles `authorization_pending` and
/// `slow_down` exactly like the RFC 8628 device flow.
pub async fn poll_token(server: &str, device: &DeviceCode) -> Result<OAuthCredential, String> {
    let deadline = Instant::now() + Duration::from_secs(device.expires_in.max(60));
    let mut interval = device.interval;
    loop {
        if Instant::now() >= deadline {
            return Err(
                "device authorization expired; run `recode auth login opencode` again".to_owned(),
            );
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let value = post(
            server,
            "/auth/device/token",
            json!({
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                "device_code": device.device_code,
                "client_id": CLIENT_ID,
            }),
        )
        .await?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            match error {
                "authorization_pending" => continue,
                "slow_down" => {
                    interval = interval.saturating_add(5);
                    continue;
                }
                "expired_token" => {
                    return Err(
                        "device authorization expired; run `recode auth login opencode` again"
                            .to_owned(),
                    )
                }
                "access_denied" => return Err("device authorization was denied".to_owned()),
                other => return Err(format!("device authorization failed: {other}")),
            }
        }
        return credential(value, server, None);
    }
}

/// Exchange a refresh token for a new access token.
pub async fn refresh(server: &str, refresh: &str) -> Result<OAuthCredential, String> {
    let value = post(
        server,
        "/auth/device/token",
        json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh,
            "client_id": CLIENT_ID,
        }),
    )
    .await?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(format!(
            "cannot refresh OpenCode Console session: {error}; run `recode auth login opencode` again"
        ));
    }
    credential(value, server, Some(refresh))
}

fn credential(
    value: Value,
    server: &str,
    fallback_refresh: Option<&str>,
) -> Result<OAuthCredential, String> {
    let access = string(&value, "access_token")?;
    let refresh = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .or(fallback_refresh)
        .unwrap_or_default()
        .to_owned();
    let expires_in = value
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(3600);
    Ok(OAuthCredential {
        access,
        refresh,
        expires: now_ms().saturating_add(expires_in.saturating_mul(1000)),
        method: "device".to_owned(),
        server: Some(server.trim_end_matches('/').to_owned()),
        org_id: value
            .get("org_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

async fn post(server: &str, path: &str, body: Value) -> Result<Value, String> {
    let url = format!("{}{}", server.trim_end_matches('/'), path);
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("cannot build OAuth client: {error}"))?
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("cannot reach OpenCode Console: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("cannot read OpenCode Console response: {error}"))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("invalid OpenCode Console response ({status}): {error}"))?;
    if !status.is_success() && value.get("error").is_none() {
        return Err(format!("OpenCode Console returned HTTP {status}"));
    }
    Ok(value)
}

fn string(value: &Value, field: &str) -> Result<String, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("OpenCode Console response is missing {field}"))
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn parses_token_metadata() {
        let credential = credential(
            json!({
                "access_token": "access",
                "refresh_token": "refresh",
                "expires_in": 3600,
                "org_id": "org-1"
            }),
            "https://console.test/",
            None,
        )
        .unwrap();
        assert_eq!(credential.access, "access");
        assert_eq!(credential.refresh, "refresh");
        assert_eq!(credential.org_id.as_deref(), Some("org-1"));
        assert_eq!(credential.server(), "https://console.test");
        assert!(!credential.expired());
    }

    #[tokio::test]
    async fn refresh_posts_the_grant_and_parses_the_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0_u8; 8192];
            let read = socket.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let body = json!({
                "access_token": "new-access",
                "refresh_token": "new-refresh",
                "expires_in": 3600
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            request
        });
        let credential = refresh(&format!("http://{address}"), "old-refresh")
            .await
            .unwrap();
        let request = server.await.unwrap();
        assert!(request.contains("POST /auth/device/token"));
        assert!(request.contains("old-refresh"));
        assert_eq!(credential.access, "new-access");
        assert_eq!(credential.refresh, "new-refresh");
    }
}
