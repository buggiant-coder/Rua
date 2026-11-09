use std::collections::HashMap;

use agent_types::provider_mapping::{OpenAiFunction, OpenAiToolCallDelta, OpenAiUsage};
use agent_types::{AgentEvent, FinishReason};
use serde_json::Value;
use tracing::debug;

use crate::error::is_retryable_provider_error;

pub fn extract_reasoning_tokens(usage: &Value) -> Option<u64> {
    usage
        .get("thinking_tokens")
        .and_then(|v| v.as_u64())
        .or_else(|| usage.get("reasoning_tokens").and_then(|v| v.as_u64()))
        .or_else(|| {
            usage
                .get("completion_tokens_details")
                .and_then(|d| d.get("reasoning_tokens"))
                .and_then(|v| v.as_u64())
        })
}

pub fn parse_openai_chunk(
    json_str: &str,
    tool_call_index_map: &mut HashMap<usize, String>,
) -> Result<(Vec<AgentEvent>, Option<FinishReason>), String> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid JSON: {e}"))?;

    let mut events = Vec::new();
    let mut finish = None;

    if let Some(error) = value.get("error") {
        let code = error
            .get("code")
            .or_else(|| error.get("type"))
            .and_then(|v| v.as_str());
        let message = error
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| error.as_str())
            .unwrap_or("OpenAI-compatible request failed");
        events.push(AgentEvent::Error {
            code: code.map(String::from),
            message: message.to_string(),
            retryable: is_retryable_provider_error(code, message),
            metadata: serde_json::Value::Null,
        });
        return Ok((events, finish));
    }

    if let Some(choices) = value.get("choices").and_then(|c| c.as_array()) {
        for choice in choices {
            let delta = choice
                .get("delta")
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            let reasoning_text = delta
                .get("reasoning_content")
                .and_then(|r| r.as_str())
                .or_else(|| delta.get("reasoning").and_then(|r| r.as_str()));
            if let Some(reasoning) = reasoning_text {
                if !reasoning.is_empty() {
                    events.push(AgentEvent::ThinkingDelta {
                        delta: reasoning.to_string(),
                        signature: None,
                        metadata: serde_json::Value::Null,
                    });
                }
            }

            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                if !content.is_empty() {
                    events.push(AgentEvent::TextDelta {
                        delta: content.to_string(),
                        metadata: serde_json::Value::Null,
                    });
                }
            }

            if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tool_calls {
                    let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                    let id = tc.get("id").and_then(|i| i.as_str()).map(String::from);
                    let function = tc.get("function").map(|f| OpenAiFunction {
                        name: f.get("name").and_then(|n| n.as_str()).map(String::from),
                        arguments: f.get("arguments").and_then(|a| {
                            if let Some(s) = a.as_str() {
                                Some(s.to_string())
                            } else if a.is_object() || a.is_array() {
                                Some(a.to_string())
                            } else {
                                None
                            }
                        }),
                    });

                    let tc_delta = OpenAiToolCallDelta {
                        index,
                        id,
                        function,
                        extra: Default::default(),
                    };
                    let tc_id = if let Some(ref real_id) = tc_delta.id {
                        tool_call_index_map.insert(tc_delta.index, real_id.clone());
                        real_id.clone()
                    } else if let Some(mapped_id) = tool_call_index_map.get(&tc_delta.index) {
                        mapped_id.clone()
                    } else {
                        format!("tc-{}", tc_delta.index)
                    };
                    let name = tc_delta.function.as_ref().and_then(|f| f.name.clone());
                    let args_delta = tc_delta
                        .function
                        .as_ref()
                        .and_then(|f| f.arguments.clone())
                        .unwrap_or_default();

                    let metadata = if let Some(extra_content) = tc.get("extra_content") {
                        serde_json::json!({ "extra_content": extra_content })
                    } else {
                        serde_json::Value::Null
                    };

                    debug!(
                        tool_call_id = %tc_id,
                        tool_name = ?name,
                        args_delta_len = args_delta.len(),
                        args_delta = %args_delta,
                        "event_parser: emitting ToolCallDelta"
                    );
                    events.push(AgentEvent::ToolCallDelta {
                        id: tc_id,
                        index,
                        name,
                        arguments_delta: args_delta,
                        metadata,
                    });
                }
            }

            if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                finish = Some(FinishReason::from_provider(fr));
            }
        }
    }

    if let Some(usage_val) = value.get("usage") {
        if let Ok(usage) = serde_json::from_value::<OpenAiUsage>(usage_val.clone()) {
            events.push(AgentEvent::Usage {
                usage: usage.to_token_usage(),
            });
        }
    }

    Ok((events, finish))
}

pub fn parse_anthropic_event(
    event_type: &str,
    data: &str,
) -> Result<(Vec<AgentEvent>, Option<FinishReason>), String> {
    let value: serde_json::Value =
        serde_json::from_str(data).map_err(|e| format!("invalid JSON: {e}"))?;

    let mut events = Vec::new();
    let mut finish = None;

    match event_type {
        "message_start" => {
            if let Some(msg) = value.get("message") {
                if let Some(usage) = msg.get("usage") {
                    if let (Some(input), Some(output)) = (
                        usage.get("input_tokens").and_then(|t| t.as_u64()),
                        usage.get("output_tokens").and_then(|t| t.as_u64()),
                    ) {
                        events.push(AgentEvent::Usage {
                            usage: agent_types::TokenUsage {
                                prompt_tokens: input,
                                completion_tokens: output,
                                reasoning_tokens: extract_reasoning_tokens(usage),
                                cached_tokens: usage
                                    .get("cache_read_input_tokens")
                                    .and_then(|c| c.as_u64()),
                                cache_write_tokens: usage
                                    .get("cache_creation_input_tokens")
                                    .and_then(|c| c.as_u64()),
                                total_tokens: None,
                            },
                        });
                    }
                }
            }
        }
        "content_block_start" => {
            if let Some(cb) = value.get("content_block") {
                let block_type = cb.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match block_type {
                    "thinking" => {
                        if let Some(text) = cb.get("thinking").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                events.push(AgentEvent::ThinkingDelta {
                                    delta: text.to_string(),
                                    signature: None,
                                    metadata: serde_json::Value::Null,
                                });
                            }
                        }
                    }
                    "text" => {
                        if let Some(text) = cb.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                events.push(AgentEvent::TextDelta {
                                    delta: text.to_string(),
                                    metadata: serde_json::Value::Null,
                                });
                            }
                        }
                    }
                    "tool_use" => {
                        let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                        let tool_id = cb.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let tool_name = cb.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        debug!(
                            anthropic_index = index,
                            tool_call_id = %tool_id,
                            tool_name = %tool_name,
                            "parse_anthropic_event: content_block_start tool_use"
                        );
                    }
                    _ => {}
                }
            }
        }
        "content_block_delta" => {
            if let Some(delta) = value.get("delta") {
                let delta_type = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match delta_type {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                events.push(AgentEvent::TextDelta {
                                    delta: text.to_string(),
                                    metadata: serde_json::Value::Null,
                                });
                            }
                        }
                    }
                    "thinking_delta" => {
                        if let Some(thinking) = delta.get("thinking").and_then(|t| t.as_str()) {
                            if !thinking.is_empty() {
                                events.push(AgentEvent::ThinkingDelta {
                                    delta: thinking.to_string(),
                                    signature: None,
                                    metadata: serde_json::Value::Null,
                                });
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(json_delta) = delta.get("partial_json").and_then(|j| j.as_str())
                        {
                            let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                            debug!(
                                anthropic_index = index,
                                delta_len = json_delta.len(),
                                delta = %json_delta,
                                "parse_anthropic_event: input_json_delta"
                            );
                            events.push(AgentEvent::ToolCallDelta {
                                id: format!("anthropic-tc-{index}"),
                                index: index as usize,
                                name: None,
                                arguments_delta: json_delta.to_string(),
                                metadata: serde_json::Value::Null,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        "content_block_stop" => {
            if let Some(cb) = value.get("content_block") {
                if let Some(sig) = cb.get("signature").and_then(|s| s.as_str()) {
                    events.push(AgentEvent::ThinkingDelta {
                        delta: String::new(),
                        signature: Some(sig.to_string()),
                        metadata: serde_json::Value::Null,
                    });
                }
            }
        }
        "message_delta" => {
            if let Some(delta) = value.get("delta") {
                if let Some(reason) = delta.get("stop_reason").and_then(|r| r.as_str()) {
                    finish = Some(FinishReason::from_provider(reason));
                }
            }
            if let Some(usage) = value.get("usage") {
                let input = usage
                    .get("input_tokens")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0);
                let output = usage
                    .get("output_tokens")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0);
                events.push(AgentEvent::Usage {
                    usage: agent_types::TokenUsage {
                        prompt_tokens: input,
                        completion_tokens: output,
                        reasoning_tokens: extract_reasoning_tokens(usage),
                        cached_tokens: usage
                            .get("cache_read_input_tokens")
                            .and_then(|c| c.as_u64()),
                        cache_write_tokens: usage
                            .get("cache_creation_input_tokens")
                            .and_then(|c| c.as_u64()),
                        total_tokens: None,
                    },
                });
            }
        }
        "message_stop" if finish.is_none() => {
            finish = Some(FinishReason::Stop);
        }
        "message_stop" => {}
        "error" => {
            let msg = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .or_else(|| value.get("message").and_then(|m| m.as_str()))
                .unwrap_or("unknown error");
            let code = value
                .get("error")
                .and_then(|e| e.get("type"))
                .and_then(|t| t.as_str());
            events.push(AgentEvent::Error {
                code: code.map(String::from),
                message: msg.to_string(),
                retryable: is_retryable_provider_error(code, msg),
                metadata: serde_json::Value::Null,
            });
        }
        _ => {}
    }

    Ok((events, finish))
}

pub fn parse_gemini_chunk(
    json_str: &str,
) -> Result<(Vec<AgentEvent>, Option<FinishReason>), String> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid JSON: {e}"))?;

    let mut events = Vec::new();
    let mut finish = None;

    if value.get("type").and_then(|v| v.as_str()) == Some("error")
        || value.get("error").is_some_and(|error| error.is_object())
    {
        let error = value.get("error").unwrap_or(&value);
        let code = error
            .get("type")
            .or_else(|| error.get("code"))
            .and_then(|v| v.as_str());
        let message = error
            .get("message")
            .or_else(|| value.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("Gemini request failed");
        events.push(AgentEvent::Error {
            code: code.map(String::from),
            message: message.to_string(),
            retryable: is_retryable_provider_error(code, message),
            metadata: serde_json::Value::Null,
        });
        return Ok((events, finish));
    }

    if let Some(candidates) = value.get("candidates").and_then(|c| c.as_array()) {
        for candidate in candidates {
            if let Some(parts) = candidate
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            let is_thought = part
                                .get("thought")
                                .and_then(|t| t.as_bool())
                                .unwrap_or(false);
                            if is_thought {
                                events.push(AgentEvent::ThinkingDelta {
                                    delta: text.to_string(),
                                    signature: part
                                        .get("thoughtSignature")
                                        .and_then(|s| s.as_str())
                                        .map(String::from),
                                    metadata: serde_json::Value::Null,
                                });
                            } else {
                                events.push(AgentEvent::TextDelta {
                                    delta: text.to_string(),
                                    metadata: serde_json::Value::Null,
                                });
                            }
                        }
                    }
                    if let Some(fc) = part.get("functionCall") {
                        let name = fc
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args = fc
                            .get("args")
                            .cloned()
                            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                        let provider_call_id =
                            fc.get("id").and_then(|id| id.as_str()).map(str::to_string);
                        let call_id = provider_call_id
                            .clone()
                            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                        let mut metadata = serde_json::Map::new();
                        if let Some(signature) =
                            part.get("thoughtSignature").and_then(|s| s.as_str())
                        {
                            metadata.insert(
                                "extra_content".into(),
                                serde_json::json!({"thought_signature": signature}),
                            );
                        }
                        debug!(
                            tool_call_id = %call_id,
                            tool_name = %name,
                            arguments = %args,
                            "parse_gemini_chunk: emitting ToolCall"
                        );
                        events.push(AgentEvent::ToolCall {
                            call: agent_types::ToolCall {
                                id: call_id,
                                name,
                                arguments: args,
                                raw_arguments: None,
                                provider_call_id,
                                metadata,
                            },
                        });
                    }
                }
            }

            if let Some(reason) = candidate.get("finishReason").and_then(|r| r.as_str()) {
                finish = Some(FinishReason::from_provider(reason));
            }
        }
    }

    if let Some(usage_meta) = value.get("usageMetadata") {
        let prompt = usage_meta
            .get("promptTokenCount")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let completion = usage_meta
            .get("candidatesTokenCount")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let total = usage_meta.get("totalTokenCount").and_then(|t| t.as_u64());
        let cached = usage_meta
            .get("cachedContentTokenCount")
            .and_then(|t| t.as_u64());

        events.push(AgentEvent::Usage {
            usage: agent_types::TokenUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                reasoning_tokens: None,
                cached_tokens: cached,
                cache_write_tokens: None,
                total_tokens: total,
            },
        });
    }

    Ok((events, finish))
}

pub fn parse_ollama_chunk(
    json_str: &str,
) -> Result<(Vec<AgentEvent>, Option<FinishReason>), String> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid JSON: {e}"))?;

    let mut events = Vec::new();
    let mut finish = None;

    if let Some(error) = value.get("error") {
        events.push(AgentEvent::Error {
            code: None,
            message: error
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| error.as_str())
                .unwrap_or("Ollama request failed")
                .to_string(),
            retryable: false,
            metadata: serde_json::Value::Null,
        });
        return Ok((events, finish));
    }

    if let Some(msg) = value.get("message") {
        if let Some(thinking) = msg.get("thinking").and_then(|t| t.as_str()) {
            if !thinking.is_empty() {
                events.push(AgentEvent::ThinkingDelta {
                    delta: thinking.to_string(),
                    signature: None,
                    metadata: serde_json::Value::Null,
                });
            }
        }

        if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
            if !content.is_empty() {
                events.push(AgentEvent::TextDelta {
                    delta: content.to_string(),
                    metadata: serde_json::Value::Null,
                });
            }
        }

        if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
            for (idx, tc) in tool_calls.iter().enumerate() {
                if let Some(function) = tc.get("function") {
                    let name = function
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments = function
                        .get("arguments")
                        .cloned()
                        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                    let raw_arguments = Some(arguments.to_string());
                    let tool_id = format!("{idx}_{name}");
                    debug!(
                        tool_call_id = %tool_id,
                        tool_name = %name,
                        arguments = %arguments,
                        "parse_ollama_chunk: emitting ToolCall"
                    );

                    events.push(AgentEvent::ToolCall {
                        call: agent_types::ToolCall {
                            id: tool_id,
                            name,
                            arguments,
                            raw_arguments,
                            provider_call_id: None,
                            metadata: serde_json::Map::new(),
                        },
                    });
                }
            }
        }
    }

    if let Some(done) = value.get("done").and_then(|d| d.as_bool()) {
        if done {
            finish = Some(FinishReason::Stop);

            let prompt = value
                .get("prompt_eval_count")
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            let completion = value
                .get("eval_count")
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            if prompt > 0 || completion > 0 {
                events.push(AgentEvent::Usage {
                    usage: agent_types::TokenUsage {
                        prompt_tokens: prompt,
                        completion_tokens: completion,
                        reasoning_tokens: None,
                        cached_tokens: None,
                        cache_write_tokens: None,
                        total_tokens: None,
                    },
                });
            }
        }
    }

    Ok((events, finish))
}
