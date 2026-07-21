//! Provider-specific request body builders for reasoning round-trip.
//!
//! - Chat Completions: plaintext `<think>` in content (Phase 1)
//! - OpenAI Responses: `reasoning` items with `encrypted_content` (Phase 2)
//! - Anthropic Messages: `thinking` blocks with `signature` (Phase 2)

use crate::config::ApiMode;
use crate::model_capabilities::lookup_capabilities;
use crate::types::{ImageAttachment, Message, ReasoningState, Role, ToolDefinition};
use base64::Engine;
use serde_json::{json, Value};

/// Build the JSON body for the resolved API mode.
pub fn build_provider_body(
    api_mode: ApiMode,
    model_id: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    stream: bool,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    thinking_enabled: bool,
    reasoning_effort: Option<&str>,
) -> Value {
    match api_mode {
        ApiMode::ChatCompletions => {
            build_chat_completions_body(model_id, messages, tools, stream, temperature, max_tokens)
        }
        ApiMode::Responses => build_responses_body(
            model_id,
            messages,
            tools,
            stream,
            temperature,
            max_tokens,
            thinking_enabled,
            reasoning_effort,
        ),
        ApiMode::AnthropicMessages => build_anthropic_messages_body(
            model_id,
            messages,
            tools,
            stream,
            temperature,
            max_tokens,
            thinking_enabled,
        ),
    }
}

/// Endpoint path relative to `base_url` (which may already include `/v1`).
pub fn endpoint_for(api_mode: ApiMode, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    match api_mode {
        ApiMode::ChatCompletions => format!("{base}/chat/completions"),
        ApiMode::Responses => {
            if base.ends_with("/responses") {
                base.to_string()
            } else {
                format!("{base}/responses")
            }
        }
        ApiMode::AnthropicMessages => {
            // Anthropic base is typically https://api.anthropic.com — messages at /v1/messages
            if base.contains("/messages") {
                base.to_string()
            } else if base.ends_with("/v1") {
                format!("{base}/messages")
            } else {
                format!("{base}/v1/messages")
            }
        }
    }
}

fn build_chat_completions_body(
    model_id: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    stream: bool,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
) -> Value {
    let supports_images = lookup_capabilities(model_id).supports_images;
    let api_messages = if supports_images && messages.iter().any(message_has_images) {
        Value::Array(
            messages
                .iter()
                .map(|msg| chat_completions_message_value(msg, true))
                .collect(),
        )
    } else {
        // Message.reasoning / images are skip_serializing — content already has <think> tags.
        json!(messages)
    };
    let mut body = json!({
        "model": model_id,
        "messages": api_messages,
        "stream": stream,
    });
    if let Some(temp) = temperature {
        body["temperature"] = json!(temp);
    }
    if let Some(max) = max_tokens {
        body["max_tokens"] = json!(max);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
    }
    body
}

fn message_has_images(msg: &Message) -> bool {
    msg.images
        .as_ref()
        .is_some_and(|images| !images.is_empty())
}

fn chat_completions_message_value(msg: &Message, include_images: bool) -> Value {
    let mut value = json!({
        "role": msg.role.to_string(),
    });
    if let Some(ref tool_calls) = msg.tool_calls {
        value["tool_calls"] = json!(tool_calls);
    }
    if let Some(ref tool_call_id) = msg.tool_call_id {
        value["tool_call_id"] = json!(tool_call_id);
    }
    if let Some(ref name) = msg.name {
        value["name"] = json!(name);
    }

    if include_images && msg.role == Role::User && message_has_images(msg) {
        value["content"] = user_content_chat_completions(msg);
    } else if let Some(content) = msg.content.as_deref() {
        value["content"] = json!(content);
    } else {
        value["content"] = Value::Null;
    }
    value
}

fn user_content_chat_completions(msg: &Message) -> Value {
    let mut parts: Vec<Value> = Vec::new();
    let text = msg.content.as_deref().unwrap_or("");
    if !text.is_empty() {
        parts.push(json!({
            "type": "text",
            "text": text,
        }));
    }
    if let Some(ref images) = msg.images {
        for image in images {
            if let Some(url) = image_data_url(image) {
                parts.push(json!({
                    "type": "image_url",
                    "image_url": { "url": url },
                }));
            }
        }
    }
    if parts.is_empty() {
        json!("")
    } else {
        Value::Array(parts)
    }
}

fn user_content_responses(msg: &Message, include_images: bool) -> Value {
    if !include_images || !message_has_images(msg) {
        return json!(msg.content.as_deref().unwrap_or(""));
    }
    let mut parts: Vec<Value> = Vec::new();
    let text = msg.content.as_deref().unwrap_or("");
    if !text.is_empty() {
        parts.push(json!({
            "type": "input_text",
            "text": text,
        }));
    }
    if let Some(ref images) = msg.images {
        for image in images {
            if let Some(url) = image_data_url(image) {
                parts.push(json!({
                    "type": "input_image",
                    "image_url": url,
                }));
            }
        }
    }
    if parts.is_empty() {
        json!("")
    } else {
        Value::Array(parts)
    }
}

fn user_content_anthropic(msg: &Message, include_images: bool) -> Value {
    if !include_images || !message_has_images(msg) {
        return json!(msg.content.as_deref().unwrap_or(""));
    }
    let mut parts: Vec<Value> = Vec::new();
    let text = msg.content.as_deref().unwrap_or("");
    if !text.is_empty() {
        parts.push(json!({
            "type": "text",
            "text": text,
        }));
    }
    if let Some(ref images) = msg.images {
        for image in images {
            if let Some((media_type, data)) = image_base64_parts(image) {
                parts.push(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    }
                }));
            }
        }
    }
    if parts.is_empty() {
        json!("")
    } else {
        Value::Array(parts)
    }
}

fn image_base64_parts(image: &ImageAttachment) -> Option<(String, String)> {
    let bytes = std::fs::read(&image.path).ok()?;
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some((image.mime_type.clone(), data))
}

fn image_data_url(image: &ImageAttachment) -> Option<String> {
    let (mime, data) = image_base64_parts(image)?;
    Some(format!("data:{mime};base64,{data}"))
}

/// OpenAI Responses API: flatten history into `input` items, round-tripping
/// encrypted reasoning blobs unchanged.
fn build_responses_body(
    model_id: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    stream: bool,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    thinking_enabled: bool,
    reasoning_effort: Option<&str>,
) -> Value {
    let include_images = lookup_capabilities(model_id).supports_images;
    let mut input: Vec<Value> = Vec::new();
    for msg in messages {
        match msg.role {
            Role::System => {
                if let Some(content) = msg.content.as_deref() {
                    if !content.is_empty() {
                        input.push(json!({
                            "role": "system",
                            "content": content,
                        }));
                    }
                }
            }
            Role::User => {
                if message_has_images(msg) || msg.content.as_ref().is_some_and(|c| !c.is_empty()) {
                    input.push(json!({
                        "role": "user",
                        "content": user_content_responses(msg, include_images),
                    }));
                }
            }
            Role::Assistant => {
                // Opaque reasoning item first (Codex / Responses requirement in tool loops).
                let mut has_opaque_reasoning = false;
                if let Some(ref reasoning) = msg.reasoning {
                    if let Some(ref blob) = reasoning.encrypted_content {
                        if !blob.is_empty() {
                            has_opaque_reasoning = true;
                            let mut item = json!({
                                "type": "reasoning",
                                "encrypted_content": blob,
                            });
                            if let Some(ref summary) = reasoning.summary {
                                item["summary"] = json!([{ "type": "summary_text", "text": summary }]);
                            }
                            input.push(item);
                        }
                    }
                }
                // Responses-compatible providers may return plaintext thinking
                // without an opaque blob. Preserve that active-loop state as
                // assistant history rather than silently dropping it.
                if !has_opaque_reasoning
                    && let Some(text) = msg
                        .reasoning
                        .as_ref()
                        .and_then(|reasoning| reasoning.text.as_deref())
                    && !text.is_empty()
                {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": format!("<think>{text}</think>"),
                        }],
                    }));
                }
                if let Some(ref calls) = msg.tool_calls {
                    for tc in calls {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.function.name,
                            "arguments": tc.function.arguments,
                        }));
                    }
                }
                if let Some(content) = msg.content.as_deref() {
                    let visible = crate::hygiene::strip_thinking_in_content(content);
                    if !visible.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{ "type": "output_text", "text": visible }],
                        }));
                    }
                }
            }
            Role::Tool => {
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": msg.tool_call_id.as_deref().unwrap_or(""),
                    "output": msg.content.as_deref().unwrap_or(""),
                }));
            }
        }
    }

    let mut body = json!({
        "model": model_id,
        "input": input,
        "stream": stream,
    });
    if thinking_enabled || reasoning_effort.is_some() {
        let mut reasoning = json!({ "summary": "auto" });
        if let Some(effort) = reasoning_effort {
            reasoning["effort"] = json!(effort);
        }
        body["reasoning"] = reasoning;
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    if let Some(temp) = temperature {
        body["temperature"] = json!(temp);
    }
    if let Some(max) = max_tokens {
        body["max_output_tokens"] = json!(max);
    }
    if !tools.is_empty() {
        let mapped: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.function.name,
                    "description": t.function.description,
                    "parameters": t.function.parameters,
                })
            })
            .collect();
        body["tools"] = json!(mapped);
    }
    body
}

/// Anthropic Messages API: system separate; thinking+signature blocks round-tripped.
fn build_anthropic_messages_body(
    model_id: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    stream: bool,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    thinking_enabled: bool,
) -> Value {
    let include_images = lookup_capabilities(model_id).supports_images;
    let mut system_parts: Vec<String> = Vec::new();
    let mut api_messages: Vec<Value> = Vec::new();

    for msg in messages {
        match msg.role {
            Role::System => {
                if let Some(c) = msg.content.as_deref() {
                    if !c.is_empty() {
                        system_parts.push(c.to_string());
                    }
                }
            }
            Role::User => {
                if message_has_images(msg) || msg.content.as_ref().is_some_and(|c| !c.is_empty()) {
                    api_messages.push(json!({
                        "role": "user",
                        "content": user_content_anthropic(msg, include_images),
                    }));
                }
            }
            Role::Assistant => {
                let mut blocks: Vec<Value> = Vec::new();
                if let Some(ref reasoning) = msg.reasoning {
                    if reasoning.has_opaque_blob() || reasoning.text.is_some() {
                        let mut block = json!({
                            "type": "thinking",
                            "thinking": reasoning.text.as_deref().unwrap_or(""),
                        });
                        if let Some(ref sig) = reasoning.signature {
                            block["signature"] = json!(sig);
                        }
                        blocks.push(block);
                    }
                } else if let Some(content) = msg.content.as_deref() {
                    // Compat: extract <think> into a thinking block without signature.
                    if let Some((think, _)) = extract_think_tag(content) {
                        blocks.push(json!({
                            "type": "thinking",
                            "thinking": think,
                        }));
                    }
                }
                if let Some(content) = msg.content.as_deref() {
                    let visible = crate::hygiene::strip_thinking_in_content(content);
                    if !visible.is_empty() {
                        blocks.push(json!({
                            "type": "text",
                            "text": visible,
                        }));
                    }
                }
                if let Some(ref calls) = msg.tool_calls {
                    for tc in calls {
                        let input: Value = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or_else(|_| json!({}));
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": input,
                        }));
                    }
                }
                if !blocks.is_empty() {
                    api_messages.push(json!({
                        "role": "assistant",
                        "content": blocks,
                    }));
                }
            }
            Role::Tool => {
                api_messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": msg.tool_call_id.as_deref().unwrap_or(""),
                        "content": msg.content.as_deref().unwrap_or(""),
                    }],
                }));
            }
        }
    }

    let mut body = json!({
        "model": model_id,
        "messages": api_messages,
        "max_tokens": max_tokens.unwrap_or(8192),
        "stream": stream,
    });
    if !system_parts.is_empty() {
        body["system"] = json!(system_parts.join("\n\n"));
    }
    if let Some(temp) = temperature {
        body["temperature"] = json!(temp);
    }
    if thinking_enabled {
        body["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": 10000,
        });
    }
    if !tools.is_empty() {
        let mapped: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.function.name,
                    "description": t.function.description,
                    "input_schema": t.function.parameters,
                })
            })
            .collect();
        body["tools"] = json!(mapped);
    }
    body
}

fn extract_think_tag(content: &str) -> Option<(String, String)> {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let start = content.find(OPEN)?;
    let after = &content[start + OPEN.len()..];
    let end = after.find(CLOSE)?;
    let think = after[..end].to_string();
    let rest = after[end + CLOSE.len()..].trim_start_matches('\n').to_string();
    Some((think, rest))
}

/// Attach reasoning parsed from a provider response onto an assistant message.
pub fn attach_reasoning_from_response(msg: &mut Message, reasoning: ReasoningState) {
    if !reasoning.is_empty() {
        msg.reasoning = Some(reasoning);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionCall, ToolCall};

    #[test]
    fn responses_body_round_trips_encrypted_blob() {
        let blob = "enc-blob-xyz";
        let mut assistant = Message::assistant_with_tools(
            "calling",
            vec![ToolCall {
                id: "call_1".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "shell".into(),
                    arguments: "{}".into(),
                },
            }],
        );
        assistant.reasoning = Some(ReasoningState {
            encrypted_content: Some(blob.into()),
            summary: Some("plan".into()),
            ..Default::default()
        });
        let msgs = vec![
            Message::user("do it"),
            assistant,
            Message::tool("call_1".into(), "ok".into(), Some("shell".into())),
        ];
        let body = build_responses_body(
            "o3",
            &msgs,
            &[],
            true,
            None,
            Some(1024),
            true,
            Some("high"),
        );
        let input = body["input"].as_array().unwrap();
        let reasoning_item = input
            .iter()
            .find(|i| i.get("type").and_then(|t| t.as_str()) == Some("reasoning"))
            .expect("reasoning item");
        assert_eq!(
            reasoning_item["encrypted_content"].as_str(),
            Some(blob)
        );
        assert!(body["include"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("reasoning.encrypted_content")));
    }

    #[test]
    fn responses_body_keeps_plaintext_reasoning_without_an_opaque_blob() {
        let mut assistant = Message::assistant_with_tools(
            "<think>inspect before calling</think>",
            vec![ToolCall {
                id: "call_1".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "shell".into(),
                    arguments: "{}".into(),
                },
            }],
        );
        assistant.reasoning = Some(ReasoningState::from_text("inspect before calling"));

        let body = build_responses_body(
            "responses-compatible-model",
            &[Message::user("do it"), assistant],
            &[],
            true,
            None,
            Some(1_024),
            true,
            Some("high"),
        );

        let serialized = serde_json::to_string(&body["input"]).unwrap();
        assert!(serialized.contains("inspect before calling"));
        assert!(serialized.contains("function_call"));
    }

    #[test]
    fn anthropic_body_round_trips_signature() {
        let mut assistant = Message::assistant("answer");
        assistant.reasoning = Some(ReasoningState {
            text: Some("step 1".into()),
            signature: Some("sig-abc".into()),
            ..Default::default()
        });
        let msgs = vec![Message::system("sys"), Message::user("q"), assistant];
        let body = build_anthropic_messages_body("claude-sonnet-4", &msgs, &[], false, None, Some(2048), true);
        assert_eq!(body["system"].as_str(), Some("sys"));
        let content = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], "sig-abc");
        assert_eq!(content[0]["thinking"], "step 1");
        assert_eq!(content[1]["type"], "text");
    }

    #[test]
    fn endpoint_paths() {
        assert!(endpoint_for(ApiMode::ChatCompletions, "https://api.openai.com/v1")
            .ends_with("/chat/completions"));
        assert!(endpoint_for(ApiMode::Responses, "https://api.openai.com/v1")
            .ends_with("/responses"));
        assert!(endpoint_for(ApiMode::AnthropicMessages, "https://api.anthropic.com")
            .ends_with("/v1/messages"));
    }

    #[test]
    fn chat_completions_omits_reasoning_field() {
        let mut msg = Message::assistant("<think>r</think>\nhi");
        msg.reasoning = Some(ReasoningState::from_text("r"));
        let body = build_chat_completions_body("deepseek-chat", &[msg], &[], true, None, None);
        let ser = serde_json::to_string(&body["messages"][0]).unwrap();
        assert!(!ser.contains("encrypted_content"));
        assert!(ser.contains("<think>"));
    }

    #[test]
    fn chat_completions_keeps_stream_retry_thinking_in_assistant_content() {
        let mut messages = vec![Message::user("do it")];
        crate::hygiene::inject_stream_retry_hint(
            &mut messages,
            "partial reasoning sentinel",
            "partial answer",
        );

        let body = build_chat_completions_body(
            "chat-model",
            &messages,
            &[],
            true,
            None,
            Some(1_024),
        );
        let serialized = serde_json::to_string(&body["messages"]).unwrap();
        assert!(serialized.contains("<think>partial reasoning sentinel</think>"));
        assert!(serialized.contains("partial answer"));
    }

    #[test]
    fn api_mode_infer() {
        assert_eq!(
            ApiMode::infer("https://api.anthropic.com", "claude-sonnet"),
            ApiMode::AnthropicMessages
        );
        assert_eq!(
            ApiMode::infer("https://api.openai.com/v1", "o3-mini"),
            ApiMode::Responses
        );
        assert_eq!(
            ApiMode::infer("https://api.deepseek.com", "deepseek-chat"),
            ApiMode::ChatCompletions
        );
    }
}
