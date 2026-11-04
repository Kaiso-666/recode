use crate::error::{http_error, truncate, Result};
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

/// Client for the OpenAI Chat Completions protocol, including every provider
/// that implements it (OpenRouter, Groq, DeepSeek, Ollama, LM Studio, vLLM,
/// Gemini's compatibility endpoint, …).
pub struct OpenAiProvider {
    id: String,
    base_url: String,
    api_key: Option<String>,
    headers: reqwest::header::HeaderMap,
    client: reqwest::Client,
    body_extra: Map<String, Value>,
    default_model: Option<String>,
}

pub struct OpenAiOptions {
    pub id: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: reqwest::header::HeaderMap,
    pub timeout_ms: Option<u64>,
    pub body_extra: Map<String, Value>,
    pub default_model: Option<String>,
}

impl OpenAiProvider {
    pub fn new(options: OpenAiOptions) -> Self {
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
            body_extra: options.body_extra,
            default_model: options.default_model,
        }
    }

    fn url(&self) -> String {
        if self.base_url.ends_with("/chat/completions") {
            self.base_url.clone()
        } else {
            format!("{}/chat/completions", self.base_url)
        }
    }

    fn body(&self, request: &LlmRequest) -> Value {
        let mut messages = Vec::new();
        if !request.system.is_empty() {
            messages.push(json!({"role": "system", "content": request.system}));
        }
        messages.extend(to_messages(&request.messages));
        let mut body = Map::new();
        body.insert("model".into(), json!(request.model.model_id));
        body.insert("stream".into(), json!(true));
        body.insert("stream_options".into(), json!({"include_usage": true}));
        body.insert("messages".into(), Value::Array(messages));
        if !request.tools.is_empty() {
            body.insert(
                "tools".into(),
                Value::Array(
                    request
                        .tools
                        .iter()
                        .map(|tool| {
                            json!({
                                "type": "function",
                                "function": {
                                    "name": tool.name,
                                    "description": tool.description,
                                    "parameters": tool.input_schema
                                }
                            })
                        })
                        .collect(),
                ),
            );
            match &request.tool_choice {
                ToolChoice::Auto => {
                    body.insert("tool_choice".into(), json!("auto"));
                }
                ToolChoice::Required => {
                    body.insert("tool_choice".into(), json!("required"));
                }
                ToolChoice::None => {
                    body.insert("tool_choice".into(), json!("none"));
                }
                ToolChoice::Tool { name } => {
                    body.insert(
                        "tool_choice".into(),
                        json!({"type": "function", "function": {"name": name}}),
                    );
                }
            }
        }
        if let Some(temperature) = request.temperature {
            body.insert("temperature".into(), json!(temperature));
        }
        if let Some(max_tokens) = request.max_tokens {
            body.insert("max_tokens".into(), json!(max_tokens));
        }
        for (key, value) in &self.body_extra {
            body.insert(key.clone(), value.clone());
        }
        Value::Object(body)
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
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
    api_key: Option<String>,
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
        .json(&body);
    if let Some(api_key) = api_key.as_deref().filter(|key| !key.is_empty()) {
        builder = builder.bearer_auth(api_key);
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
            if event.data.trim() == "[DONE]" {
                state.finish(&tx).await?;
                return Ok(());
            }
            let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
                tracing::debug!(provider = %provider_id, "ignoring non-JSON SSE payload");
                continue;
            };
            state.handle(&value, &tx).await?;
        }
    }
    for event in decoder.finish() {
        if event.data.trim() == "[DONE]" {
            break;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&event.data) {
            state.handle(&value, &tx).await?;
        }
    }
    state.finish(&tx).await
}

#[derive(Default)]
struct CallAccum {
    id: String,
    name: String,
    arguments: String,
    started: bool,
}

#[derive(Default)]
struct StreamState {
    calls: HashMap<u32, CallAccum>,
    usage: Usage,
    text: String,
    finish_reason: Option<FinishReason>,
    finished: bool,
}

impl StreamState {
    async fn handle(
        &mut self,
        value: &Value,
        tx: &mpsc::Sender<std::result::Result<LlmEvent, LlmError>>,
    ) -> Result<()> {
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("provider returned an error");
            return Err(LlmError::new(message.to_owned(), true));
        }
        if let Some(usage) = value.get("usage").filter(|value| !value.is_null()) {
            self.usage = parse_usage(usage);
        }
        let choice = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first());
        let Some(choice) = choice else {
            return Ok(());
        };
        if let Some(delta) = choice.get("delta") {
            if let Some(text) = delta.get("content").and_then(content_text) {
                if !text.is_empty() {
                    self.text.push_str(&text);
                    send(tx, LlmEvent::TextDelta(text)).await?;
                }
            }
            if let Some(reasoning) = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(Value::as_str)
            {
                if !reasoning.is_empty() {
                    send(
                        tx,
                        LlmEvent::ReasoningDelta {
                            delta: reasoning.to_owned(),
                            signature: None,
                        },
                    )
                    .await?;
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for (position, call) in calls.iter().enumerate() {
                    let index = call
                        .get("index")
                        .and_then(Value::as_u64)
                        .map(|value| value as u32)
                        .unwrap_or(position as u32);
                    let entry = self.calls.entry(index).or_default();
                    if let Some(id) = call.get("id").and_then(Value::as_str) {
                        if !id.is_empty() {
                            entry.id = id.to_owned();
                        }
                    }
                    if let Some(function) = call.get("function") {
                        if let Some(name) = function.get("name").and_then(Value::as_str) {
                            if !name.is_empty() {
                                entry.name.push_str(name);
                            }
                        }
                        if let Some(arguments) = function.get("arguments") {
                            if let Some(arguments) = arguments.as_str() {
                                entry.arguments.push_str(arguments);
                                if entry.started {
                                    send(
                                        tx,
                                        LlmEvent::ToolCallDelta {
                                            index,
                                            delta: arguments.to_owned(),
                                        },
                                    )
                                    .await?;
                                }
                            }
                        }
                    }
                    if !entry.started && !entry.name.is_empty() {
                        entry.started = true;
                        if entry.id.is_empty() {
                            entry.id = format!("call_{index}");
                        }
                        send(
                            tx,
                            LlmEvent::ToolCallStart {
                                index,
                                id: entry.id.clone(),
                                name: entry.name.clone(),
                            },
                        )
                        .await?;
                        if !entry.arguments.is_empty() {
                            send(
                                tx,
                                LlmEvent::ToolCallDelta {
                                    index,
                                    delta: entry.arguments.clone(),
                                },
                            )
                            .await?;
                        }
                    }
                    if entry.started {
                        // Argument deltas were already forwarded above.
                    }
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(map_finish_reason(reason));
        }
        Ok(())
    }

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
        let mut had_calls = false;
        for index in indexes {
            let Some(call) = self.calls.get(&index) else {
                continue;
            };
            if !call.started {
                continue;
            }
            had_calls = true;
            let input = if call.arguments.trim().is_empty() {
                Value::Object(Map::new())
            } else {
                serde_json::from_str(&call.arguments).unwrap_or_else(|_| Value::Object(Map::new()))
            };
            send(
                tx,
                LlmEvent::ToolCallEnd {
                    index,
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input,
                },
            )
            .await?;
        }
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

/// Some providers stream arguments on the same chunk that creates the call,
/// before we have emitted the start event; the start-emission path replays
/// whatever was accumulated at that point.
async fn send(
    tx: &mpsc::Sender<std::result::Result<LlmEvent, LlmError>>,
    event: LlmEvent,
) -> Result<()> {
    tx.send(Ok(event))
        .await
        .map_err(|_| LlmError::new("stream consumer dropped", false))
}

fn content_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    let array = value.as_array()?;
    let mut output = String::new();
    for part in array {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            output.push_str(text);
        }
    }
    Some(output)
}

fn parse_usage(value: &Value) -> Usage {
    let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
    let input = number(value, "prompt_tokens");
    let output = number(value, "completion_tokens");
    let total = value
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(input + output);
    let cached = value
        .get("prompt_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning = value
        .get("completion_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Usage {
        input_tokens: input,
        output_tokens: output,
        reasoning_tokens: reasoning,
        cache_read_tokens: cached,
        cache_write_tokens: 0,
        total_tokens: total,
        cost: 0.0,
    }
}

fn map_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "stop" | "end_turn" => FinishReason::Stop,
        "length" | "max_tokens" => FinishReason::Length,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        "error" => FinishReason::Error,
        _ => FinishReason::Unknown,
    }
}

fn to_messages(messages: &[recode_schema::llm::LlmMessage]) -> Vec<Value> {
    let mut output = Vec::new();
    for message in messages {
        match message.role {
            LlmRole::User => {
                let mut texts = String::new();
                let mut parts = Vec::new();
                let mut has_file = false;
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } => {
                            texts.push_str(text);
                        }
                        ContentBlock::File {
                            mime,
                            filename: _,
                            url,
                        } if mime.starts_with("image/") => {
                            has_file = true;
                            parts.push(json!({"type": "image_url", "image_url": {"url": url}}));
                        }
                        _ => {}
                    }
                }
                if has_file {
                    let mut content = vec![json!({"type": "text", "text": texts})];
                    content.extend(parts);
                    output.push(json!({"role": "user", "content": content}));
                } else {
                    output.push(json!({"role": "user", "content": texts}));
                }
            }
            LlmRole::Assistant => {
                let mut content = String::new();
                let mut calls = Vec::new();
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } => content.push_str(text),
                        ContentBlock::ToolCall {
                            id, name, input, ..
                        } => {
                            calls.push(json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into())
                                }
                            }));
                        }
                        _ => {}
                    }
                }
                let mut message = Map::new();
                message.insert("role".into(), json!("assistant"));
                if !content.is_empty() || calls.is_empty() {
                    message.insert("content".into(), json!(content));
                }
                if !calls.is_empty() {
                    message.insert("tool_calls".into(), Value::Array(calls));
                }
                output.push(Value::Object(message));
            }
            LlmRole::Tool => {
                for block in &message.content {
                    if let ContentBlock::ToolResult {
                        id,
                        name: _,
                        content,
                        is_error: _,
                    } = block
                    {
                        output.push(json!({
                            "role": "tool",
                            "tool_call_id": id,
                            "content": truncate(content, 200_000)
                        }));
                    }
                }
            }
            LlmRole::System => {
                output.push(json!({"role": "system", "content": message.text()}));
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_tool_loop_messages() {
        let messages = vec![
            recode_schema::llm::LlmMessage::user("read a.txt"),
            recode_schema::llm::LlmMessage::assistant(vec![
                ContentBlock::text("reading"),
                ContentBlock::ToolCall {
                    id: "call_1".into(),
                    name: "read".into(),
                    input: json!({"path": "a.txt"}),
                    namespace: None,
                },
            ]),
            recode_schema::llm::LlmMessage::tool(vec![ContentBlock::ToolResult {
                id: "call_1".into(),
                name: "read".into(),
                content: "hello".into(),
                is_error: false,
            }]),
        ];
        let mapped = to_messages(&messages);
        assert_eq!(mapped[0]["role"], "user");
        assert_eq!(mapped[1]["tool_calls"][0]["function"]["name"], "read");
        assert_eq!(mapped[2]["role"], "tool");
        assert_eq!(mapped[2]["tool_call_id"], "call_1");
    }

    #[test]
    fn parses_usage_details() {
        let usage = parse_usage(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "total_tokens": 120,
            "prompt_tokens_details": {"cached_tokens": 40},
            "completion_tokens_details": {"reasoning_tokens": 5}
        }));
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cache_read_tokens, 40);
        assert_eq!(usage.reasoning_tokens, 5);
        assert_eq!(usage.total_tokens, 120);
    }
}
