use crate::error::{http_error, Result};
use crate::sse::SseDecoder;
use crate::Provider;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use recode_schema::llm::{
    ContentBlock, FinishReason, LlmError, LlmEvent, LlmRequest, LlmRole, ToolChoice, Usage,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

pub struct AnthropicProvider {
    id: String,
    base_url: String,
    api_key: String,
    headers: reqwest::header::HeaderMap,
    client: reqwest::Client,
    default_model: Option<String>,
}

pub struct AnthropicOptions {
    pub id: String,
    pub base_url: String,
    pub api_key: String,
    pub headers: reqwest::header::HeaderMap,
    pub timeout_ms: Option<u64>,
    pub default_model: Option<String>,
}

impl AnthropicProvider {
    pub fn new(options: AnthropicOptions) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(
                options.timeout_ms.unwrap_or(600_000),
            ))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            id: options.id,
            base_url: options.base_url.trim_end_matches('/').to_owned(),
            api_key: options.api_key,
            headers: options.headers,
            client,
            default_model: options.default_model,
        }
    }

    fn url(&self) -> String {
        if self.base_url.ends_with("/messages") {
            self.base_url.clone()
        } else {
            format!("{}/v1/messages", self.base_url)
        }
    }

    fn body(&self, request: &LlmRequest) -> Value {
        let mut body = Map::new();
        body.insert("model".into(), json!(request.model.model_id));
        body.insert(
            "max_tokens".into(),
            json!(request.max_tokens.unwrap_or(8192)),
        );
        body.insert("stream".into(), json!(true));
        if !request.system.is_empty() {
            body.insert("system".into(), json!(request.system));
        }
        body.insert(
            "messages".into(),
            Value::Array(to_messages(&request.messages)),
        );
        if !request.tools.is_empty() {
            body.insert(
                "tools".into(),
                Value::Array(
                    request
                        .tools
                        .iter()
                        .map(|tool| {
                            json!({
                                "name": tool.name,
                                "description": tool.description,
                                "input_schema": tool.input_schema
                            })
                        })
                        .collect(),
                ),
            );
            match &request.tool_choice {
                ToolChoice::Auto => {
                    body.insert("tool_choice".into(), json!({"type": "auto"}));
                }
                ToolChoice::Required => {
                    body.insert("tool_choice".into(), json!({"type": "any"}));
                }
                ToolChoice::None => {
                    body.insert("tool_choice".into(), json!({"type": "none"}));
                }
                ToolChoice::Tool { name } => {
                    body.insert("tool_choice".into(), json!({"type": "tool", "name": name}));
                }
            }
        }
        if let Some(temperature) = request.temperature {
            body.insert("temperature".into(), json!(temperature));
        }
        Value::Object(body)
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn default_model(&self) -> Option<&str> {
        self.default_model.as_deref()
    }

    async fn stream(
        &self,
        request: LlmRequest,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, std::result::Result<LlmEvent, LlmError>>> {
        let (tx, rx) = mpsc::channel(128);
        let body = self.body(&request);
        let url = self.url();
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let headers = self.headers.clone();
        let provider_id = self.id.clone();
        tokio::spawn(async move {
            if let Err(error) = run_stream(
                client,
                url,
                api_key,
                headers,
                provider_id,
                body,
                tx.clone(),
                cancel,
            )
            .await
            {
                let _ = tx.send(Err(error)).await;
            }
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_stream(
    client: reqwest::Client,
    url: String,
    api_key: String,
    headers: reqwest::header::HeaderMap,
    provider_id: String,
    body: Value,
    tx: mpsc::Sender<std::result::Result<LlmEvent, LlmError>>,
    cancel: CancellationToken,
) -> Result<()> {
    let mut builder = client
        .post(&url)
        .header("accept", "text/event-stream")
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .json(&body);
    if !api_key.is_empty() {
        builder = builder.header("x-api-key", &api_key);
    }
    builder = builder.headers(headers);

    let response = tokio::select! {
        _ = cancel.cancelled() => return Ok(()),
        response = builder.send() => response.map_err(|error| {
            LlmError::new(format!("{provider_id}: request failed: {error}"), true)
        })?,
    };
    let status = response.status();
    if !status.is_success() {
        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();
        return Err(http_error(status.as_u16(), &headers, &body));
    }

    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    let mut state = StreamState::default();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let bytes = chunk.map_err(|error| {
            LlmError::new(format!("{provider_id}: stream error: {error}"), true)
        })?;
        for event in decoder.push(&bytes) {
            handle_event(&event.event, &event.data, &mut state, &tx).await?;
        }
    }
    for event in decoder.finish() {
        handle_event(&event.event, &event.data, &mut state, &tx).await?;
    }
    state.finish(&tx).await
}

#[derive(Default)]
struct CallAccum {
    id: String,
    name: String,
    arguments: String,
    ended: bool,
}

#[derive(Default)]
struct StreamState {
    calls: HashMap<u32, CallAccum>,
    usage: Usage,
    text: String,
    finish_reason: Option<FinishReason>,
    finished: bool,
}

async fn handle_event(
    kind: &Option<String>,
    data: &str,
    state: &mut StreamState,
    tx: &mpsc::Sender<std::result::Result<LlmEvent, LlmError>>,
) -> Result<()> {
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        tracing::debug!("ignoring non-JSON Anthropic SSE payload");
        return Ok(());
    };
    let kind = kind
        .as_deref()
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or_default();
    match kind {
        "message_start" => {
            if let Some(usage) = value
                .get("message")
                .and_then(|message| message.get("usage"))
            {
                state.usage = parse_usage(usage, &state.usage);
            }
        }
        "content_block_start" => {
            let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
            if let Some(block) = value.get("content_block") {
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let entry = state.calls.entry(index).or_default();
                    entry.id = id.clone();
                    entry.name = name.clone();
                    send(tx, LlmEvent::ToolCallStart { index, id, name }).await?;
                }
            }
        }
        "content_block_delta" => {
            let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
            if let Some(delta) = value.get("delta") {
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                state.text.push_str(text);
                                send(tx, LlmEvent::TextDelta(text.to_owned())).await?;
                            }
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(thinking) = delta.get("thinking").and_then(Value::as_str) {
                            if !thinking.is_empty() {
                                send(
                                    tx,
                                    LlmEvent::ReasoningDelta {
                                        delta: thinking.to_owned(),
                                        signature: None,
                                    },
                                )
                                .await?;
                            }
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                            if let Some(entry) = state.calls.get_mut(&index) {
                                entry.arguments.push_str(partial);
                                send(
                                    tx,
                                    LlmEvent::ToolCallDelta {
                                        index,
                                        delta: partial.to_owned(),
                                    },
                                )
                                .await?;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        "content_block_stop" => {
            let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
            if let Some(entry) = state.calls.get_mut(&index) {
                if !entry.ended {
                    entry.ended = true;
                    let input = if entry.arguments.trim().is_empty() {
                        Value::Object(Map::new())
                    } else {
                        serde_json::from_str(&entry.arguments)
                            .unwrap_or_else(|_| Value::Object(Map::new()))
                    };
                    send(
                        tx,
                        LlmEvent::ToolCallEnd {
                            index,
                            id: entry.id.clone(),
                            name: entry.name.clone(),
                            input,
                        },
                    )
                    .await?;
                }
            }
        }
        "message_delta" => {
            if let Some(delta) = value.get("delta") {
                if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
                    state.finish_reason = Some(map_finish_reason(reason));
                }
            }
            if let Some(usage) = value.get("usage") {
                state.usage = parse_usage(usage, &state.usage);
            }
        }
        "message_stop" => {
            state.finish(tx).await?;
        }
        "error" => {
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("provider returned an error");
            return Err(LlmError::new(message.to_owned(), true));
        }
        "ping" | "message_start_ack" => {}
        _ => {}
    }
    Ok(())
}

impl StreamState {
    async fn finish(
        &mut self,
        tx: &mpsc::Sender<std::result::Result<LlmEvent, LlmError>>,
    ) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let mut indexes: Vec<u32> = self.calls.keys().copied().collect();
        indexes.sort_unstable();
        for index in indexes {
            let entry = self.calls.get_mut(&index).expect("index came from map");
            if entry.ended {
                continue;
            }
            entry.ended = true;
            let input = if entry.arguments.trim().is_empty() {
                Value::Object(Map::new())
            } else {
                serde_json::from_str(&entry.arguments).unwrap_or_else(|_| Value::Object(Map::new()))
            };
            send(
                tx,
                LlmEvent::ToolCallEnd {
                    index,
                    id: entry.id.clone(),
                    name: entry.name.clone(),
                    input,
                },
            )
            .await?;
        }
        let had_calls = !self.calls.is_empty();
        let reason = self.finish_reason.unwrap_or(if had_calls {
            FinishReason::ToolCalls
        } else {
            FinishReason::Stop
        });
        send(tx, LlmEvent::Usage(self.usage.clone())).await?;
        send(
            tx,
            LlmEvent::Finish {
                reason,
                usage: self.usage.clone(),
                text: self.text.clone(),
            },
        )
        .await?;
        Ok(())
    }
}

async fn send(
    tx: &mpsc::Sender<std::result::Result<LlmEvent, LlmError>>,
    event: LlmEvent,
) -> Result<()> {
    tx.send(Ok(event))
        .await
        .map_err(|_| LlmError::new("stream consumer dropped", false))
}

fn parse_usage(value: &Value, previous: &Usage) -> Usage {
    let number = |key: &str| value.get(key).and_then(Value::as_u64);
    let input = number("input_tokens").unwrap_or(previous.input_tokens);
    let output = number("output_tokens").unwrap_or(previous.output_tokens);
    let cache_read = number("cache_read_input_tokens").unwrap_or(previous.cache_read_tokens);
    let cache_write = number("cache_creation_input_tokens").unwrap_or(previous.cache_write_tokens);
    Usage {
        input_tokens: input,
        output_tokens: output,
        reasoning_tokens: previous.reasoning_tokens,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        total_tokens: input + output + cache_read + cache_write,
        cost: previous.cost,
    }
}

fn map_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "end_turn" | "stop_sequence" | "pause_turn" => FinishReason::Stop,
        "max_tokens" => FinishReason::Length,
        "tool_use" => FinishReason::ToolCalls,
        "refusal" => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
    }
}

fn to_messages(messages: &[recode_schema::llm::LlmMessage]) -> Vec<Value> {
    let mut output: Vec<Value> = Vec::new();
    for message in messages {
        let (role, content) = match message.role {
            LlmRole::User => ("user", user_blocks(&message.content)),
            LlmRole::Assistant => ("assistant", assistant_blocks(&message.content)),
            LlmRole::Tool => ("user", tool_result_blocks(&message.content)),
            LlmRole::System => (
                "user",
                vec![json!({"type": "text", "text": message.text()})],
            ),
        };
        if content.is_empty() {
            continue;
        }
        // Anthropic requires alternating roles; merge adjacent same-role turns.
        if let Some(last) = output.last_mut() {
            if last.get("role").and_then(Value::as_str) == Some(role) {
                if let Some(blocks) = last.get_mut("content").and_then(Value::as_array_mut) {
                    blocks.extend(content);
                    continue;
                }
            }
        }
        output.push(json!({"role": role, "content": content}));
    }
    output
}

fn user_blocks(content: &[ContentBlock]) -> Vec<Value> {
    let mut blocks = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text } => {
                if !text.is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
            }
            ContentBlock::File { mime, url, .. } if mime.starts_with("image/") => {
                blocks.push(json!({"type": "image", "source": {"type": "url", "url": url}}));
            }
            _ => {}
        }
    }
    blocks
}

fn assistant_blocks(content: &[ContentBlock]) -> Vec<Value> {
    let mut blocks = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text } => {
                if !text.is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
            }
            ContentBlock::Reasoning {
                text,
                signature: Some(signature),
            } => {
                blocks.push(json!({
                    "type": "thinking",
                    "thinking": text,
                    "signature": signature
                }));
            }
            ContentBlock::Reasoning { .. } => {}
            ContentBlock::ToolCall {
                id, name, input, ..
            } => {
                blocks.push(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input
                }));
            }
            _ => {}
        }
    }
    blocks
}

fn tool_result_blocks(content: &[ContentBlock]) -> Vec<Value> {
    let mut blocks = Vec::new();
    for block in content {
        if let ContentBlock::ToolResult {
            id,
            content,
            is_error,
            ..
        } = block
        {
            blocks.push(json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": content,
                "is_error": is_error
            }));
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_consecutive_tool_results_into_user_turns() {
        let messages = vec![
            recode_schema::llm::LlmMessage::user("hi"),
            recode_schema::llm::LlmMessage::assistant(vec![ContentBlock::ToolCall {
                id: "toolu_1".into(),
                name: "read".into(),
                input: json!({}),
                namespace: None,
            }]),
            recode_schema::llm::LlmMessage::tool(vec![ContentBlock::ToolResult {
                id: "toolu_1".into(),
                name: "read".into(),
                content: "a".into(),
                is_error: false,
            }]),
            recode_schema::llm::LlmMessage::tool(vec![ContentBlock::ToolResult {
                id: "toolu_2".into(),
                name: "read".into(),
                content: "b".into(),
                is_error: false,
            }]),
        ];
        let mapped = to_messages(&messages);
        let last = mapped.last().unwrap();
        assert_eq!(last["role"], "user");
        assert_eq!(last["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn maps_stop_reasons() {
        assert_eq!(map_finish_reason("tool_use"), FinishReason::ToolCalls);
        assert_eq!(map_finish_reason("max_tokens"), FinishReason::Length);
        assert_eq!(map_finish_reason("end_turn"), FinishReason::Stop);
    }
}
