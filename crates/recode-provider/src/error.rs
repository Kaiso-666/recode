use recode_schema::llm::LlmError;
use reqwest::header::HeaderMap;
use serde_json::Value;

pub type Result<T> = std::result::Result<T, LlmError>;

/// Turn a non-success HTTP response into a structured, retry-aware error.
pub fn http_error(status: u16, headers: &HeaderMap, body: &str) -> LlmError {
    let retryable = status == 408 || status == 409 || status == 429 || (500..600).contains(&status);
    let retry_after_ms = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| seconds * 1000);
    let message = extract_error(body).unwrap_or_else(|| {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            format!("HTTP {status}")
        } else {
            format!("HTTP {status}: {}", truncate(trimmed, 800))
        }
    });
    LlmError {
        message,
        retryable,
        status: Some(status),
        retry_after_ms,
    }
}

/// Pull a human-readable message out of the many error shapes providers use.
fn extract_error(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let error = value.get("error").unwrap_or(&value);
    if let Some(message) = error.get("message").and_then(Value::as_str) {
        return Some(message.to_owned());
    }
    if let Some(message) = error.as_str() {
        return Some(message.to_owned());
    }
    None
}

pub fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let mut truncated: String = value.chars().take(limit).collect();
    truncated.push('…');
    truncated
}
