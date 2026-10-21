//! Minimal SSE decoding plus OpenAI-compatible stream delta handling.

use std::collections::HashMap;

use serde_json::Value;

/// Incremental SSE decoder: feed raw bytes, get complete event payloads back.
#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a chunk; returns every payload terminated by a blank line.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(payload) = self.line(line.trim_end_matches(['\n', '\r'])) {
                out.push(payload);
            }
        }
        out
    }

    /// Flush whatever is left when the stream closes (some providers skip the
    /// final blank line or never send `[DONE]`).
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned();
            let line = line.trim_end_matches(['\n', '\r']);
            if let Some(payload) = self.line(line) {
                out.push(payload);
            }
        }
        if !self.data.is_empty() {
            out.push(self.data.join("\n"));
            self.data.clear();
        }
        out
    }

    fn line(&mut self, line: &str) -> Option<String> {
        if line.is_empty() {
            if self.data.is_empty() {
                return None;
            }
            let joined = self.data.join("\n");
            self.data.clear();
            return Some(joined);
        }
        if line.starts_with(':') {
            return None; // comment / keepalive
        }
        if let Some(rest) = line.strip_prefix("data:") {
            self.data
                .push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        }
        None
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Delta {
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCallDelta>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallDelta {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Delta(Delta),
    Done,
    Error(String),
    Ignore,
}

/// Parse one SSE payload from an OpenAI-compatible streaming endpoint.
pub fn parse_frame(payload: &str) -> Frame {
    let trimmed = payload.trim();
    if trimmed == "[DONE]" {
        return Frame::Done;
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return Frame::Ignore;
    };
    if let Some(err) = value.get("error") {
        return Frame::Error(error_message(err));
    }
    let Some(choice) = value
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
    else {
        return Frame::Ignore;
    };
    let node = choice
        .get("delta")
        .or_else(|| choice.get("message"))
        .unwrap_or(&Value::Null);
    let mut delta = Delta {
        text: text_field(node, "content"),
        ..Default::default()
    };
    for key in ["reasoning_content", "reasoning", "thinking"] {
        delta.reasoning = text_field(node, key);
        if !delta.reasoning.is_empty() {
            break;
        }
    }
    if let Some(calls) = node.get("tool_calls").and_then(|c| c.as_array()) {
        for (pos, call) in calls.iter().enumerate() {
            let index = call
                .get("index")
                .and_then(|i| i.as_u64())
                .map(|i| i as usize)
                .unwrap_or(pos);
            let function = call.get("function");
            delta.tool_calls.push(ToolCallDelta {
                index,
                // Providers stream continuation fragments with `"id": ""`;
                // an empty id must never look like the start of a new call.
                id: non_empty(call.get("id").and_then(|v| v.as_str())),
                name: non_empty(
                    function
                        .and_then(|f| f.get("name"))
                        .and_then(|v| v.as_str()),
                ),
                arguments: function
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            });
        }
    }
    if delta.text.is_empty() && delta.reasoning.is_empty() && delta.tool_calls.is_empty() {
        Frame::Ignore
    } else {
        Frame::Delta(delta)
    }
}

/// A string field, tolerating the `[{ "text": .. }]` shape some providers use.
fn text_field(node: &Value, key: &str) -> String {
    match node.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                p.get("text")
                    .and_then(|t| t.as_str())
                    .or_else(|| p.as_str())
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn error_message(err: &Value) -> String {
    if let Some(s) = err.as_str() {
        return s.to_string();
    }
    if let Some(s) = err.get("message").and_then(|m| m.as_str()) {
        return s.to_string();
    }
    err.to_string()
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|s| !s.is_empty()).map(str::to_string)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Default)]
struct Slot {
    id: Option<String>,
    name: String,
    args: String,
}

/// Accumulates streamed tool-call fragments. A fresh `id` on an index that
/// already has one opens a new slot (Ollama-style reruns on one index).
#[derive(Default)]
pub struct ToolCallAccumulator {
    slots: Vec<Slot>,
    map: HashMap<usize, usize>,
}

impl ToolCallAccumulator {
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn apply(&mut self, delta: ToolCallDelta) {
        let pos = match self.map.get(&delta.index).copied() {
            Some(pos) => {
                let restarted = match (&self.slots[pos].id, &delta.id) {
                    (Some(old), Some(new)) => old != new,
                    _ => false,
                };
                if restarted {
                    self.slots.push(Slot::default());
                    self.slots.len() - 1
                } else {
                    pos
                }
            }
            None => {
                self.slots.push(Slot::default());
                self.slots.len() - 1
            }
        };
        self.map.insert(delta.index, pos);
        let slot = &mut self.slots[pos];
        if slot.id.is_none() {
            slot.id = delta.id;
        }
        if let Some(name) = delta.name
            && !name.is_empty()
        {
            slot.name = name;
        }
        if let Some(args) = delta.arguments {
            slot.args.push_str(&args);
        }
    }

    /// Drain into finalized calls, assigning stable fallback ids.
    pub fn take(&mut self) -> Vec<ToolCall> {
        let slots = std::mem::take(&mut self.slots);
        self.map.clear();
        slots
            .into_iter()
            .enumerate()
            .map(|(i, slot)| ToolCall {
                id: slot.id.unwrap_or_else(|| format!("call_{i}")),
                name: if slot.name.is_empty() {
                    "unknown".to_string()
                } else {
                    slot.name
                },
                arguments: slot.args,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_splits_events_and_ignores_keepalive() {
        let mut d = SseDecoder::new();
        assert!(d.push(b": ping\n\n").is_empty());
        let out = d.push(b"data: {\"a\":1}\n\n");
        assert_eq!(out, vec!["{\"a\":1}".to_string()]);
        // split across chunks
        assert!(d.push(b"data: {\"b\"").is_empty());
        let out = d.push(b":2}\n\ndata: [DONE]\n\n");
        assert_eq!(out, vec!["{\"b\":2}".to_string(), "[DONE]".to_string()]);
    }

    #[test]
    fn decoder_joins_multiline_data() {
        let mut d = SseDecoder::new();
        let out = d.push(b"data: line1\ndata: line2\n\n");
        assert_eq!(out, vec!["line1\nline2".to_string()]);
    }

    #[test]
    fn decoder_finish_flushes_unterminated_tail() {
        let mut d = SseDecoder::new();
        assert!(d.push(b"data: [DONE]").is_empty());
        assert_eq!(d.finish(), vec!["[DONE]".to_string()]);
        assert!(d.finish().is_empty());
    }

    #[test]
    fn empty_ids_are_not_new_tool_calls() {
        // 9router continuation frames send "id": "" — must not split the slot.
        let Frame::Delta(first) = parse_frame(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":""}}]}}]}"#,
        ) else {
            panic!("expected delta");
        };
        let Frame::Delta(second) = parse_frame(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"","function":{"arguments":"{\"path\":\"a\"}"}}]}}]}"#,
        ) else {
            panic!("expected delta");
        };
        assert_eq!(second.tool_calls[0].id, None);
        let mut acc = ToolCallAccumulator::default();
        for delta in [first, second] {
            for call in delta.tool_calls {
                acc.apply(call);
            }
        }
        let calls = acc.take();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read");
        assert_eq!(calls[0].arguments, "{\"path\":\"a\"}");
    }

    #[test]
    fn parse_content_reasoning_and_done() {
        assert_eq!(parse_frame("[DONE]"), Frame::Done);
        let Frame::Delta(d) = parse_frame(r#"{"choices":[{"delta":{"content":"hi "}}]}"#) else {
            panic!("expected delta");
        };
        assert_eq!(d.text, "hi ");
        let Frame::Delta(d) = parse_frame(r#"{"choices":[{"delta":{"reasoning_content":"hm"}}]}"#)
        else {
            panic!("expected delta");
        };
        assert_eq!(d.reasoning, "hm");
    }

    #[test]
    fn parse_content_array_shape() {
        let Frame::Delta(d) = parse_frame(
            r#"{"choices":[{"delta":{"content":[{"type":"text","text":"ab"},{"text":"cd"}]}}]}"#,
        ) else {
            panic!("expected delta");
        };
        assert_eq!(d.text, "abcd");
    }

    #[test]
    fn parse_error_frame() {
        assert_eq!(
            parse_frame(r#"{"error":{"message":"boom"}}"#),
            Frame::Error("boom".to_string())
        );
    }

    #[test]
    fn parse_tool_call_fragments() {
        let Frame::Delta(d) = parse_frame(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read","arguments":"{\"pa"}}]}}]}"#,
        ) else {
            panic!("expected delta");
        };
        assert_eq!(d.tool_calls.len(), 1);
        assert_eq!(d.tool_calls[0].index, 0);
        assert_eq!(d.tool_calls[0].name.as_deref(), Some("read"));
    }

    #[test]
    fn accumulator_joins_arguments_per_index() {
        let mut acc = ToolCallAccumulator::default();
        acc.apply(ToolCallDelta {
            index: 0,
            id: Some("c1".into()),
            name: Some("read".into()),
            arguments: Some("{\"pa".into()),
        });
        acc.apply(ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments: Some("th\":\"a\"}".into()),
        });
        let calls = acc.take();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read");
        assert_eq!(calls[0].arguments, "{\"path\":\"a\"}");
    }

    #[test]
    fn accumulator_keeps_parallel_indices_apart() {
        let mut acc = ToolCallAccumulator::default();
        for (index, name) in [(0, "read"), (1, "grep")] {
            acc.apply(ToolCallDelta {
                index,
                id: Some(format!("c{index}")),
                name: Some(name.into()),
                arguments: Some("{}".into()),
            });
        }
        let calls = acc.take();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "read");
        assert_eq!(calls[1].name, "grep");
    }

    #[test]
    fn accumulator_new_id_on_same_index_starts_new_slot() {
        let mut acc = ToolCallAccumulator::default();
        acc.apply(ToolCallDelta {
            index: 0,
            id: Some("c1".into()),
            name: Some("read".into()),
            arguments: Some("{}".into()),
        });
        acc.apply(ToolCallDelta {
            index: 0,
            id: Some("c2".into()),
            name: Some("grep".into()),
            arguments: Some("{}".into()),
        });
        let calls = acc.take();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].name, "grep");
        assert_eq!(calls[1].id, "c2");
    }

    #[test]
    fn accumulator_name_is_set_not_concatenated() {
        let mut acc = ToolCallAccumulator::default();
        acc.apply(ToolCallDelta {
            index: 0,
            id: Some("c1".into()),
            name: Some("rea".into()),
            arguments: None,
        });
        acc.apply(ToolCallDelta {
            index: 0,
            id: None,
            name: Some("read".into()),
            arguments: None,
        });
        let calls = acc.take();
        assert_eq!(calls[0].name, "read");
    }
}
