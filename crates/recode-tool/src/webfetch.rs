use crate::{Tool, ToolContext};
use async_trait::async_trait;
use futures_util::StreamExt;
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub struct WebFetchTool;

#[derive(Deserialize)]
struct Input {
    url: String,
    #[serde(default)]
    format: Option<String>,
}

const MAX_BYTES: usize = 2 * 1024 * 1024;
const USER_AGENT: &str =
    "Mozilla/5.0 (compatible; ReCode/0.1; +https://github.com/Kaiso-666/recode)";

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "webfetch"
    }

    fn description(&self) -> &str {
        "Fetch a URL and return its contents as readable text. HTML is converted to \
         plain text (scripts, styles, and tags stripped). Limited to 2 MB and 30 seconds. \
         Use format \"html\" to receive the raw document."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "http:// or https:// URL."},
                "format": {
                    "type": "string",
                    "enum": ["text", "html", "markdown"],
                    "description": "Output format. Default text."
                }
            },
            "required": ["url"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> &str {
        "webfetch"
    }

    fn default_decision(&self) -> Decision {
        Decision::Ask
    }

    fn title(&self, input: &Value) -> String {
        format!(
            "Fetch {}",
            crate::truncate_middle(input.get("url").and_then(Value::as_str).unwrap_or(""), 80)
        )
    }

    fn permission_patterns(&self, input: &Value) -> Vec<String> {
        input
            .get("url")
            .and_then(Value::as_str)
            .map(|url| vec![url.to_owned()])
            .unwrap_or_default()
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid webfetch arguments: {error}")))?;
        let url = url::Url::parse(&input.url)
            .map_err(|error| ToolFailure::new(format!("invalid URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ToolFailure::new("only http and https URLs are supported"));
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|error| ToolFailure::new(format!("cannot create HTTP client: {error}")))?;
        let response = tokio::select! {
            _ = context.abort.cancelled() => return Err(ToolFailure::new("aborted")),
            response = client.get(url.clone()).send() => response
                .map_err(|error| ToolFailure::new(format!("request failed: {error}")))?,
        };
        let status = response.status();
        if !status.is_success() {
            return Err(ToolFailure::new(format!(
                "HTTP {} for {url}",
                status.as_u16()
            )));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("text/plain")
            .to_owned();
        if let Some(length) = response.content_length() {
            if length as usize > MAX_BYTES {
                return Err(ToolFailure::new(format!(
                    "response is too large ({length} bytes, limit {MAX_BYTES})"
                )));
            }
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| ToolFailure::new(format!("read failed: {error}")))?;
            if body.len() + chunk.len() > MAX_BYTES {
                body.extend_from_slice(&chunk[..MAX_BYTES - body.len()]);
                break;
            }
            body.extend_from_slice(&chunk);
        }
        let raw = String::from_utf8_lossy(&body).to_string();
        let format = input.format.as_deref().unwrap_or("text");
        let output = if format == "html" || !content_type.contains("html") {
            raw
        } else {
            html_to_text(&raw)
        };
        Ok(ToolOutcome {
            title: Some(format!("Fetch {url}")),
            output,
            metadata: Some(
                json!({"url": url.to_string(), "contentType": content_type, "format": format}),
            ),
            attachments: Vec::new(),
        })
    }
}

/// Conservative HTML → text conversion. Not a full parser; good enough for
/// documentation pages and issue trackers without pulling in an HTML engine.
pub fn html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len() / 2);
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0usize;
    while let Some(start) = lower[cursor..].find('<') {
        let start = cursor + start;
        text.push_str(&html[cursor..start]);
        let Some(end) = lower[start..].find('>') else {
            break;
        };
        let tag = &lower[start + 1..start + end];
        let name = tag
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or_default();
        match name {
            "script" | "style" | "noscript" | "template" | "svg" => {
                if !tag.starts_with('/') {
                    if let Some(close) = lower[start..].find(&format!("</{name}")) {
                        cursor = start + close;
                        if let Some(end) = lower[cursor..].find('>') {
                            cursor += end + 1;
                            continue;
                        }
                    }
                }
            }
            "br" | "p" | "div" | "section" | "article" | "li" | "tr" | "h1" | "h2" | "h3"
            | "h4" | "h5" | "h6" | "pre" | "blockquote" => {
                text.push('\n');
            }
            "td" | "th" => text.push('\t'),
            _ => {}
        }
        cursor = start + end + 1;
    }
    text.push_str(&html[cursor..]);
    decode_entities(&text)
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .split("\n\n\n")
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_owned()
}

fn decode_entities(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find('&') {
        output.push_str(&rest[..index]);
        rest = &rest[index..];
        let Some(end) = rest.find(';').filter(|end| *end <= 10) else {
            output.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => {
                if let Some(hex) = entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                {
                    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                } else if let Some(decimal) = entity.strip_prefix('#') {
                    decimal.parse::<u32>().ok().and_then(char::from_u32)
                } else {
                    None
                }
            }
        };
        match decoded {
            Some(character) => {
                output.push(character);
                rest = &rest[end + 1..];
            }
            None => {
                output.push('&');
                rest = &rest[1..];
            }
        }
    }
    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_scripts() {
        let html = "<html><head><style>body{}</style></head><body><h1>Title</h1><p>Hello &amp; welcome</p><script>bad()</script></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Title"));
        assert!(text.contains("Hello & welcome"));
        assert!(!text.contains("bad()"));
        assert!(!text.contains("<h1>"));
    }
}
