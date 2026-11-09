use matrix_sdk::room::reply::{EnforceThread, Reply};
use matrix_sdk::ruma::events::room::message::{AddMentions, ReplyWithinThread};
use serde_json::{json, Value};

use super::types::MatrixOptions;

#[derive(Clone, Default)]
pub(super) struct MatrixReply {
    pub thread_root: Option<String>,
    pub options: MatrixOptions,
}

impl MatrixReply {
    pub fn from_metadata(metadata: &Value, options: MatrixOptions) -> Self {
        Self {
            thread_root: metadata["matrix_thread_root_event_id"]
                .as_str()
                .map(str::to_owned),
            options,
        }
    }

    pub fn decorate(&self, mut event: Value) -> Value {
        if let Some(content) = event.get_mut("m.new_content") {
            *content = self.decorate(content.clone());
            return event;
        }
        if let Some(root) = &self.thread_root {
            event["m.relates_to"] = json!({
                "rel_type": "m.thread", "event_id": root,
                "is_falling_back": true, "m.in_reply_to": { "event_id": root },
            });
        }
        let body = event["body"].as_str().unwrap_or_default().to_string();
        let regex = regex::Regex::new(r"@[a-zA-Z0-9._=+/\-]+:[a-zA-Z0-9.\-]+(?::\d+)?")
            .expect("constant Matrix ID regex");
        let mut ids = Vec::new();
        for value in regex.find_iter(&body) {
            if !ids.contains(&value.as_str()) {
                ids.push(value.as_str());
            }
        }
        if self.options.outbound_structured_mentions && !ids.is_empty() {
            event["m.mentions"] = json!({ "user_ids": ids });
        }
        if self.options.mention_pill_in_body && !ids.is_empty() {
            let mut html = escape_html(&body);
            for id in ids {
                html = html.replace(
                    &escape_html(id),
                    &format!(
                        "<a href=\"https://matrix.to/#/{}\">{}</a>",
                        super::channel::percent_encode_path_segment(id),
                        escape_html(id),
                    ),
                );
            }
            event["format"] = json!("org.matrix.custom.html");
            event["formatted_body"] = json!(html.replace('\n', "<br>"));
        }
        event
    }

    pub fn attachment_reply(&self) -> Option<Reply> {
        Some(Reply {
            event_id: self.thread_root.as_ref()?.as_str().try_into().ok()?,
            enforce_thread: EnforceThread::Threaded(ReplyWithinThread::No),
            add_mentions: AddMentions::No,
        })
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
