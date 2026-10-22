//! Anthropic Messages API pieces: request conversion and stream parsing.
//!
//! Conversation history stays in the [OI]-compatible shape on disk; everything
//! is converted at request time so switching providers mid-session keeps the
//! same context.

use serde_json::{Value, json};

use crate::sse::ToolCall;

pub const API_VERSION: &str = "2023-06-01";
pub const DEFAULT_MAX_TOKENS: u32 = 8192;

pub fn messages_url(base_url: &str) -> String {
    format!("{}/messages", base_url.trim_end_matches('/'))
}

/// Flattens `content` whether it is a plain string or a block array.
fn content_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|part| {
                    part.get("text")
                        .and_then(Value::as_str)
                        .or_else(|| part.as_str())
                })
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
}

/// Stored [OI]-shaped messages -> (system prompt, Anthropic messages).
pub fn convert_messages(messages: &[Value]) -> (String, Vec<Value>) {
    let mut system = String::new();
    let mut out: Vec<Value> = Vec::new();
    let mut open_tool_results = false;
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        match role {
            "system" => {
                if let Some(text) = content_text(message.get("content")) {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&text);
                }
            }
            "user" => {
                open_tool_results = false;
                let text = content_text(message.get("content")).unwrap_or_default();
                out.push(json!({ "role": "user", "content": text }));
            }
            "assistant" => {
                open_tool_results = false;
                let mut blocks: Vec<Value> = Vec::new();
                if let Some(text) = content_text(message.get("content")).filter(|t| !t.is_empty()) {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        let id = call.get("id").and_then(Value::as_str).unwrap_or("");
                        let name = call
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if name.is_empty() {
                            continue;
                        }
                        let arguments = call
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}");
                        let input =
                            serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({}));
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input,
                        }));
                    }
                }
                if !blocks.is_empty() {
                    out.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
            "tool" => {
                let id = message
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let text = content_text(message.get("content")).unwrap_or_default();
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": text,
                });
                // Anthropic wants every result of one assistant turn in a single
                // user message, so append to the previous tool-result message.
                if open_tool_results
                    && let Some(blocks) = out
                        .last_mut()
                        .and_then(|last| last.get_mut("content"))
                        .and_then(Value::as_array_mut)
                {
                    blocks.push(block);
                    continue;
                }
                out.push(json!({ "role": "user", "content": [block] }));
                open_tool_results = true;
            }
            _ => {}
        }
    }
    (system, out)
}

/// [OI] function specs -> Anthropic `tools` array.
pub fn convert_tools(specs: &[Value]) -> Vec<Value> {
    specs
        .iter()
        .filter_map(|spec| {
            let function = spec.get("function")?;
            let name = function.get("name")?.as_str()?;
            let schema = function
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            Some(json!({
                "name": name,
                "description": function
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                "input_schema": schema,
            }))
        })
        .collect()
}

pub fn request_body(
    model: &str,
    system: &str,
    messages: &[Value],
    tools: &[Value],
    stream: bool,
) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": DEFAULT_MAX_TOKENS,
        "messages": messages,
        "stream": stream,
    });
    if !system.is_empty() {
        body["system"] = Value::String(system.to_string());
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
    }
    body
}

/// Non-streaming reply text (test connection / title suggestion).
pub fn reply_text(value: &Value) -> Option<String> {
    let parts = value.get("content")?.as_array()?;
    let text = parts
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    (!text.is_empty()).then_some(text)
}

/// Pulls a human-readable message out of an Anthropic error body.
pub fn error_message(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| text.trim().to_string())
}

#[derive(Default)]
struct Slot {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
pub struct StreamAcc {
    pub text: String,
    pub thinking: String,
    pub calls: Vec<ToolCall>,
    pub done: bool,
    pub error: Option<String>,
    slots: Vec<Option<Slot>>,
}

impl StreamAcc {
    fn slot(&mut self, index: usize) -> &mut Slot {
        if self.slots.len() <= index {
            self.slots.resize_with(index + 1, || None);
        }
        self.slots[index].get_or_insert_with(Slot::default)
    }
}

/// Feeds one SSE payload. Returns live `(text, thinking)` deltas to forward.
pub fn apply_event(acc: &mut StreamAcc, payload: &str) -> (String, String) {
    let Ok(value) = serde_json::from_str::<Value>(payload.trim()) else {
        return (String::new(), String::new());
    };
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
    let mut text = String::new();
    let mut thinking = String::new();
    match kind {
        "content_block_start" => {
            let block = value.get("content_block").cloned().unwrap_or(Value::Null);
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    let slot = acc.slot(index);
                    slot.id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    slot.name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                }
                Some("text") => {
                    text = block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                }
                Some("thinking") => {
                    thinking = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                }
                _ => {}
            }
        }
        "content_block_delta" => {
            let delta = value.get("delta").cloned().unwrap_or(Value::Null);
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => {
                    text = delta
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                }
                Some("thinking_delta") => {
                    thinking = delta
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                }
                Some("input_json_delta") => {
                    if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                        acc.slot(index).arguments.push_str(partial);
                    }
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            if let Some(Some(mut slot)) = acc.slots.get_mut(index).map(Option::take)
                && !slot.name.is_empty()
            {
                if slot.id.is_empty() {
                    slot.id = format!("toolu_{}", acc.calls.len());
                }
                if slot.arguments.trim().is_empty() {
                    slot.arguments = "{}".to_string();
                }
                acc.calls.push(ToolCall {
                    id: slot.id,
                    name: slot.name,
                    arguments: slot.arguments,
                });
            }
        }
        "message_stop" => acc.done = true,
        "error" => acc.error = Some(error_message(&value.to_string())),
        _ => {}
    }
    acc.text.push_str(&text);
    acc.thinking.push_str(&thinking);
    (text, thinking)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_system_tool_calls_and_results() {
        let messages = vec![
            json!({"role":"system","content":"be brief"}),
            json!({"role":"user","content":"list files"}),
            json!({"role":"assistant","content":"sure","tool_calls":[
                {"id":"c1","type":"function","function":{"name":"list","arguments":"{\"path\":\".\"}"}},
                {"id":"c2","type":"function","function":{"name":"read","arguments":"not json"}}
            ]}),
            json!({"role":"tool","tool_call_id":"c1","content":"a.txt"}),
            json!({"role":"tool","tool_call_id":"c2","content":"boom"}),
            json!({"role":"user","content":"thanks"}),
        ];
        let (system, out) = convert_messages(&messages);
        assert_eq!(system, "be brief");
        assert_eq!(out.len(), 4);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[1]["content"][0]["type"], "text");
        assert_eq!(out[1]["content"][1]["type"], "tool_use");
        assert_eq!(out[1]["content"][1]["input"]["path"], ".");
        assert_eq!(out[1]["content"][2]["input"], json!({}));
        // Both tool results share one user message.
        assert_eq!(out[2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(out[2]["content"][1]["tool_use_id"], "c2");
        assert_eq!(out[3]["content"], "thanks");
    }

    #[test]
    fn converts_tool_specs_to_input_schema() {
        let specs = vec![json!({
            "type":"function",
            "function":{"name":"read","description":"Read a file",
                        "parameters":{"type":"object","properties":{"path":{"type":"string"}}}}
        })];
        let tools = convert_tools(&specs);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "read");
        assert_eq!(
            tools[0]["input_schema"]["properties"]["path"]["type"],
            "string"
        );
    }

    #[test]
    fn accumulates_text_thinking_and_streamed_tool_arguments() {
        let frames = [
            r#"{"type":"message_start","message":{}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"halo "}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"bos"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"read"}}"#,
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"a.txt\"}"}}"#,
            r#"{"type":"content_block_stop","index":2}"#,
            r#"{"type":"message_stop"}"#,
        ];
        let mut acc = StreamAcc::default();
        for frame in frames {
            apply_event(&mut acc, frame);
        }
        assert!(acc.done);
        assert_eq!(acc.text, "halo bos");
        assert_eq!(acc.thinking, "hmm");
        assert_eq!(acc.calls.len(), 1);
        assert_eq!(acc.calls[0].id, "toolu_1");
        assert_eq!(acc.calls[0].name, "read");
        assert_eq!(acc.calls[0].arguments, r#"{"path":"a.txt"}"#);
    }

    #[test]
    fn error_event_surfaces_provider_message() {
        let mut acc = StreamAcc::default();
        apply_event(
            &mut acc,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
        );
        assert_eq!(acc.error.as_deref(), Some("busy"));
        assert_eq!(
            error_message(r#"{"error":{"message":"nope"}}"#),
            "nope".to_string()
        );
    }

    #[test]
    fn non_streaming_reply_text_joins_text_blocks() {
        let value = json!({"content":[
            {"type":"thinking","thinking":"x"},
            {"type":"text","text":"OK"}
        ]});
        assert_eq!(reply_text(&value).as_deref(), Some("OK"));
    }
}
