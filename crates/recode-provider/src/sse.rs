//! Incremental Server-Sent Events decoder.
//!
//! Tolerant of arbitrary chunk boundaries, CRLF, comments, multi-line `data:`
//! fields, and the `event:`/`id:`/`retry:` fields used by Anthropic and
//! OpenAI. No assumptions about HTTP framing.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
    pub retry: Option<u64>,
}

#[derive(Default)]
struct Pending {
    event: Option<String>,
    data: Vec<String>,
    id: Option<String>,
    retry: Option<u64>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.event.is_none() && self.data.is_empty() && self.id.is_none() && self.retry.is_none()
    }
}

#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    pending: Pending,
    last_event_id: Option<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes; returns every complete event decoded by this chunk.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=newline).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if let Some(event) = self.process_line(&line) {
                events.push(event);
            }
        }
        events
    }

    /// Flush a trailing event that lacked the final blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = String::from_utf8_lossy(&self.buffer).to_string();
            self.buffer.clear();
            if let Some(event) = self.process_line(&line) {
                events.push(event);
            }
        }
        if let Some(event) = self.dispatch() {
            events.push(event);
        }
        events
    }

    fn process_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => self.pending.event = Some(value.to_owned()),
            "data" => self.pending.data.push(value.to_owned()),
            "id" => {
                if !value.contains('\0') {
                    self.pending.id = Some(value.to_owned());
                    self.last_event_id = Some(value.to_owned());
                }
            }
            "retry" => {
                if let Ok(retry) = value.parse::<u64>() {
                    self.pending.retry = Some(retry);
                }
            }
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.pending.is_empty() {
            return None;
        }
        let mut pending = std::mem::take(&mut self.pending);
        // An event with only an id/retry and no data is not dispatched by the
        // SSE spec; keep the decoder aligned with browsers.
        if pending.data.is_empty() && pending.event.is_none() {
            return None;
        }
        let data = pending.data.join("\n");
        let id = pending.id.take().or_else(|| self.last_event_id.clone());
        Some(SseEvent {
            event: pending.event.take(),
            data,
            id,
            retry: pending.retry.take(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_simple_events() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"event: message\ndata: hello\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message"));
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn handles_arbitrary_chunk_boundaries() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"da").is_empty());
        assert!(decoder.push(b"ta: hel").is_empty());
        let events = decoder.push(b"lo\n");
        assert!(events.is_empty(), "blank line not yet received");
        let events = decoder.push(b"\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn joins_multiline_data_and_strips_crlf() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: one\r\ndata: two\r\n\r\n");
        assert_eq!(events[0].data, "one\ntwo");
    }

    #[test]
    fn ignores_comments() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b": ping\ndata: x\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "x");
    }

    #[test]
    fn flush_trailing_event() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: tail").is_empty());
        let events = decoder.finish();
        assert_eq!(events[0].data, "tail");
    }

    #[test]
    fn handles_json_data() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(
            br#"data: {"type":"content_block_delta","delta":{"text":"hi"}}

"#,
        );
        let value: serde_json::Value = serde_json::from_str(&events[0].data).unwrap();
        assert_eq!(value["delta"]["text"], "hi");
    }
}
