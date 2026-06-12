use crate::types::StreamEvent;
use anyhow::Result;
use futures::stream::{self, Stream};
use reqwest::Response;
use serde_json::Value;

pub struct SseParser;

impl SseParser {
    pub fn parse_stream(response: Response) -> impl Stream<Item = Result<StreamEvent>> {
        stream::unfold(
            (response, String::new(), false),
            |(mut resp, mut buffer, mut done)| async move {
                if done {
                    return None;
                }

                loop {
                    if let Some(line_end) = buffer.find('\n') {
                        let line = buffer[..line_end].trim().to_string();
                        buffer = buffer[line_end + 1..].to_string();

                        if line.is_empty() {
                            continue;
                        }

                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                done = true;
                                return Some((Ok(StreamEvent::Done), (resp, buffer, done)));
                            }

                            match parse_sse_data(data) {
                                Ok(event) => {
                                    return Some((Ok(event), (resp, buffer, done)));
                                }
                                Err(_) => continue,
                            }
                        }
                        continue;
                    }

                    match resp.chunk().await {
                        Ok(Some(chunk)) => {
                            let text = String::from_utf8_lossy(&chunk);
                            buffer.push_str(&text);
                        }
                        Ok(None) => {
                            done = true;
                            return Some((Ok(StreamEvent::Done), (resp, buffer, done)));
                        }
                        Err(e) => {
                            done = true;
                            return Some((
                                Err(anyhow::anyhow!("stream error: {e}")),
                                (resp, buffer, done),
                            ));
                        }
                    }
                }
            },
        )
    }
}

fn parse_sse_data(data: &str) -> Result<StreamEvent> {
    let v: Value = serde_json::from_str(data)?;

    let choices = v["choices"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no choices array"))?;

    if choices.is_empty() {
        return Ok(StreamEvent::Done);
    }

    let choice = &choices[0];
    let delta = &choice["delta"];

    if let Some(finish_reason) = choice["finish_reason"].as_str()
        && (finish_reason == "stop" || finish_reason == "tool_calls")
    {
        return Ok(StreamEvent::Done);
    }

    if let Some(tool_calls) = delta["tool_calls"].as_array()
        && let Some(tc) = tool_calls.first()
    {
        let index = tc["index"].as_u64().unwrap_or(0) as usize;
        return Ok(StreamEvent::ToolCallDelta {
            index,
            id: tc["id"].as_str().map(|s| s.to_string()),
            function_name: tc["function"]["name"].as_str().map(|s| s.to_string()),
            arguments_delta: tc["function"]["arguments"].as_str().map(|s| s.to_string()),
        });
    }

    if let Some(thinking) = delta["reasoning_content"].as_str()
        && !thinking.is_empty()
    {
        return Ok(StreamEvent::ThinkingDelta(thinking.to_string()));
    }

    if let Some(content) = delta["content"].as_str()
        && !content.is_empty()
    {
        return Ok(StreamEvent::TextDelta(content.to_string()));
    }

    Ok(StreamEvent::TextDelta(String::new()))
}

pub struct ToolCallAccumulator {
    calls: std::collections::HashMap<usize, PartialToolCall>,
}

#[derive(Default)]
struct PartialToolCall {
    id: Option<String>,
    function_name: Option<String>,
    arguments: String,
}

impl Default for ToolCallAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolCallAccumulator {
    pub fn new() -> Self {
        Self {
            calls: std::collections::HashMap::new(),
        }
    }

    pub fn push(&mut self, event: StreamEvent) {
        if let StreamEvent::ToolCallDelta {
            index,
            id,
            function_name,
            arguments_delta,
        } = event
        {
            let entry = self.calls.entry(index).or_default();
            if let Some(id) = id {
                entry.id = Some(id);
            }
            if let Some(name) = function_name {
                entry.function_name = Some(name);
            }
            if let Some(delta) = arguments_delta {
                entry.arguments.push_str(&delta);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    pub fn into_tool_calls(self) -> Vec<crate::types::ToolCall> {
        let mut sorted: Vec<_> = self.calls.into_iter().collect();
        sorted.sort_by_key(|(i, _)| *i);

        sorted
            .into_iter()
            .filter_map(|(_, partial)| {
                Some(crate::types::ToolCall {
                    id: partial
                        .id
                        .unwrap_or_else(|| format!("call_{}", uuid::Uuid::new_v4())),
                    call_type: "function".to_string(),
                    function: crate::types::FunctionCall {
                        name: partial.function_name?,
                        arguments: partial.arguments,
                    },
                })
            })
            .collect()
    }
}
