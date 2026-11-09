use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use pin_project_lite::pin_project;
use tracing::debug;

use agent_types::{AgentEvent, ToolCall};

use crate::error::LlmError;
use crate::tag_parser::{TagParser, TagSegment};
use crate::traits::EventStream;

pin_project! {


    pub struct SanitizedStream {
        #[pin]
        inner: EventStream,
        tag_parser: TagParser,
        pending_events: Vec<Result<AgentEvent, LlmError>>,
        tc_counter: u32,
        saw_terminal: bool,
    }
}

impl SanitizedStream {
    pub fn new(inner: EventStream) -> Self {
        Self {
            inner,
            tag_parser: TagParser::new(),
            pending_events: Vec::new(),
            tc_counter: 0,
            saw_terminal: false,
        }
    }

    pub fn wrap(inner: EventStream) -> EventStream {
        Box::pin(Self::new(inner))
    }
}

impl Stream for SanitizedStream {
    type Item = Result<AgentEvent, LlmError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        if !this.pending_events.is_empty() {
            return Poll::Ready(Some(this.pending_events.remove(0)));
        }

        if *this.saw_terminal {
            return Poll::Ready(None);
        }

        match this.inner.poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => match event {
                AgentEvent::TextDelta { delta, .. } => {
                    this.tag_parser.push(&delta);
                    let segments = this.tag_parser.drain();
                    let consumed_structured_tag = this.tag_parser.take_consumed_structured_tag();
                    if segments.is_empty() {
                        if this.tag_parser.is_buffering_structured_segment() {
                            cx.waker().wake_by_ref();
                            return Poll::Pending;
                        }
                        if consumed_structured_tag {
                            cx.waker().wake_by_ref();
                            return Poll::Pending;
                        }
                        this.tag_parser.clear_buffer();
                        Poll::Ready(Some(Ok(AgentEvent::TextDelta {
                            delta,
                            metadata: serde_json::Value::Null,
                        })))
                    } else {
                        let mut events = convert_segments(&segments, *this.tc_counter);
                        *this.tc_counter += segments
                            .iter()
                            .filter(|s| matches!(s, TagSegment::ToolCall { .. }))
                            .count() as u32;
                        if events.is_empty() {
                            cx.waker().wake_by_ref();
                            Poll::Pending
                        } else {
                            let first = events.remove(0);
                            for e in events {
                                this.pending_events.push(Ok(e));
                            }
                            Poll::Ready(Some(Ok(first)))
                        }
                    }
                }
                AgentEvent::ToolCall { call } => {
                    let sanitized = sanitize_tool_call(call);
                    Poll::Ready(Some(Ok(AgentEvent::ToolCall { call: sanitized })))
                }
                AgentEvent::End { .. } | AgentEvent::Error { .. } => {
                    *this.saw_terminal = true;
                    let remaining = std::mem::take(this.tag_parser).finish();
                    let flushed = convert_segments(&remaining, *this.tc_counter);
                    for e in flushed {
                        this.pending_events.push(Ok(e));
                    }
                    this.pending_events.push(Ok(event));
                    Poll::Ready(Some(this.pending_events.remove(0)))
                }
                other => Poll::Ready(Some(Ok(other))),
            },
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => {
                if !*this.saw_terminal {
                    *this.saw_terminal = true;
                    let remaining = std::mem::take(this.tag_parser).finish();
                    let flushed = convert_segments(&remaining, *this.tc_counter);
                    for e in flushed {
                        this.pending_events.push(Ok(e));
                    }
                    if !this.pending_events.is_empty() {
                        return Poll::Ready(Some(this.pending_events.remove(0)));
                    }
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

fn convert_segments(segments: &[TagSegment], tc_offset: u32) -> Vec<AgentEvent> {
    TagParser::segments_to_events(segments, tc_offset)
}

pub fn sanitize_tool_call(mut call: ToolCall) -> ToolCall {
    if call.name.is_empty() {
        debug!("sanitize_tool_call: name was empty/null, replacing with \"\"");
        call.name = String::new();
    }
    if call.arguments.is_null() {
        if let Some(ref raw) = call.raw_arguments {
            if let Some(repaired) = try_repair_json(raw) {
                debug!(tool = %call.name, "repaired malformed tool call arguments");
                call.arguments = repaired;
            }
        }
    }
    call
}

pub fn try_repair_json(raw: &str) -> Option<serde_json::Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(v);
    }
    if let Some(v) = parse_json_prefix(trimmed) {
        return Some(v);
    }

    let mut repaired = trimmed.to_string();

    loop {
        let prev = repaired.clone();
        repaired = repaired.replace(",}", "}");
        repaired = repaired.replace(",]", "]");
        if repaired == prev {
            break;
        }
    }

    let single_count = repaired.chars().filter(|&c| c == '\'').count();
    let double_count = repaired.chars().filter(|&c| c == '"').count();
    if single_count > double_count {
        repaired = replace_single_quotes_with_double(&repaired);
    }

    serde_json::from_str(&repaired)
        .ok()
        .or_else(|| parse_json_prefix(&repaired))
}

fn parse_json_prefix(raw: &str) -> Option<serde_json::Value> {
    let mut stream = serde_json::Deserializer::from_str(raw).into_iter::<serde_json::Value>();
    stream.next()?.ok()
}

pub fn replace_single_quotes_with_double(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_double_quote = false;
    let mut prev_char = '\0';

    for ch in s.chars() {
        match ch {
            '"' if prev_char != '\\' => {
                in_double_quote = !in_double_quote;
                result.push(ch);
            }
            '\'' if !in_double_quote && prev_char != '\\' => {
                result.push('"');
            }
            _ => {
                result.push(ch);
            }
        }
        prev_char = ch;
    }
    result
}
