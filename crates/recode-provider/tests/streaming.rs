use futures_util::StreamExt;
use recode_provider::anthropic::{AnthropicOptions, AnthropicProvider};
use recode_provider::openai::{OpenAiOptions, OpenAiProvider};
use recode_provider::Provider;
use recode_schema::llm::{FinishReason, LlmEvent, LlmRequest};
use recode_schema::model::ModelRef;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// Serve one HTTP response with a fixed body and return the base URL.
async fn serve_once(body: String) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = vec![0u8; 65_536];
        let mut read = 0;
        loop {
            let count = socket.read(&mut buffer[read..]).await.unwrap();
            if count == 0 {
                break;
            }
            read += count;
            if buffer[..read]
                .windows(4)
                .any(|window| window == b"\r\n\r\n")
            {
                break;
            }
            if read >= buffer.len() {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.shutdown().await;
    });
    format!("http://{address}")
}

fn request(model: &str) -> LlmRequest {
    LlmRequest {
        model: ModelRef::new("test", model),
        system: "test".into(),
        messages: vec![recode_schema::llm::LlmMessage::user("hi")],
        tools: vec![],
        tool_choice: Default::default(),
        temperature: None,
        max_tokens: Some(1000),
        metadata: Default::default(),
    }
}

#[tokio::test]
async fn openai_streams_text_and_tool_calls() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"file_path\\\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"a.txt\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\n",
        "data: [DONE]\n\n"
    )
    .to_owned();
    let base_url = serve_once(body).await;
    let provider = OpenAiProvider::new(OpenAiOptions {
        id: "test".into(),
        base_url,
        api_key: Some("test".into()),
        headers: Default::default(),
        timeout_ms: None,
        body_extra: Default::default(),
        default_model: None,
    });
    let mut stream = provider
        .stream(request("gpt-test"), CancellationToken::new())
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut finish = None;
    for event in events {
        match event {
            LlmEvent::TextDelta(delta) => text.push_str(&delta),
            LlmEvent::ToolCallStart { id, name, .. } => calls.push((id, name, String::new())),
            LlmEvent::ToolCallDelta { delta, .. } => {
                if let Some(call) = calls.last_mut() {
                    call.2.push_str(&delta);
                }
            }
            LlmEvent::Finish { reason, usage, .. } => finish = Some((reason, usage)),
            _ => {}
        }
    }
    assert_eq!(text, "Hello");
    assert_eq!(calls[0].0, "call_1");
    assert_eq!(calls[0].1, "read");
    assert_eq!(calls[0].2, "{\"file_path\":\"a.txt\"}");
    let (reason, usage) = finish.unwrap();
    assert_eq!(reason, FinishReason::ToolCalls);
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 5);
}

#[tokio::test]
async fn anthropic_streams_text_and_tool_use() {
    let body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_read_input_tokens\":3}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"bash\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"ls\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    )
    .to_owned();
    let base_url = serve_once(body).await;
    let provider = AnthropicProvider::new(AnthropicOptions {
        id: "test".into(),
        base_url,
        api_key: "test".into(),
        headers: Default::default(),
        timeout_ms: None,
        default_model: None,
    });
    let mut stream = provider
        .stream(request("claude-test"), CancellationToken::new())
        .await
        .unwrap();
    let mut text = String::new();
    let mut tool_input = None;
    let mut finish = None;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            LlmEvent::TextDelta(delta) => text.push_str(&delta),
            LlmEvent::ToolCallEnd { input, .. } => tool_input = Some(input),
            LlmEvent::Finish { reason, usage, .. } => finish = Some((reason, usage)),
            _ => {}
        }
    }
    assert_eq!(text, "Hi");
    assert_eq!(tool_input.unwrap()["command"], "ls");
    let (reason, usage) = finish.unwrap();
    assert_eq!(reason, FinishReason::ToolCalls);
    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.output_tokens, 7);
    assert_eq!(usage.cache_read_tokens, 3);
}
