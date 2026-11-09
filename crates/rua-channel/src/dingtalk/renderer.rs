use crate::core::content::OutgoingContentPart;
use crate::core::renderer::RenderStyle;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

const LONG_CODE_BLOCK_THRESHOLD: usize = 800;
const MARKDOWN_MAX_LENGTH: usize = 3500;

#[derive(Debug, Clone, PartialEq)]
pub enum DingTalkPayload {
    Markdown {
        title: String,
        text: String,
    },

    ActionCard {
        title: String,
        text: String,
        single_title: String,
        single_url: String,
    },

    Text {
        content: String,
    },

    Image {
        media_id: String,
    },
}

impl DingTalkPayload {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Markdown { title, text } => serde_json::json!({
                "msgtype": "markdown",
                "markdown": {
                    "title": title,
                    "text": text,
                }
            }),
            Self::ActionCard {
                title,
                text,
                single_title,
                single_url,
            } => serde_json::json!({
                "msgtype": "actionCard",
                "actionCard": {
                    "title": title,
                    "text": text,
                    "singleTitle": single_title,
                    "singleURL": single_url,
                }
            }),
            Self::Text { content } => serde_json::json!({
                "msgtype": "text",
                "text": {
                    "content": content,
                }
            }),
            Self::Image { media_id } => serde_json::json!({
                "msgtype": "image",
                "image": {
                    "media_id": media_id,
                }
            }),
        }
    }
}

pub struct MessageRenderer {
    pub style: RenderStyle,
    pub console_base_url: Option<String>,
}

impl MessageRenderer {
    pub fn new(style: RenderStyle) -> Self {
        Self {
            style,
            console_base_url: None,
        }
    }

    pub fn with_console_url(mut self, url: String) -> Self {
        self.console_base_url = Some(url);
        self
    }

    pub fn render(&self, parts: &[OutgoingContentPart]) -> Vec<DingTalkPayload> {
        let mut text_segments: Vec<String> = Vec::new();
        let image_payloads: Vec<DingTalkPayload> = Vec::new();

        for part in parts {
            match part {
                OutgoingContentPart::Text { text } | OutgoingContentPart::Markdown { text } => {
                    if !text.is_empty() {
                        text_segments.push(text.clone());
                    }
                }
                OutgoingContentPart::Refusal { text } => {
                    if !text.is_empty() {
                        text_segments.push(text.clone());
                    }
                }
                OutgoingContentPart::Tool { name, status, .. } => {
                    text_segments.push(format!("[Tool: {name} - {status:?}]"));
                }
                OutgoingContentPart::Image { url, alt, .. } => {
                    let label = alt.as_deref().unwrap_or("[Image]");
                    if let Some(u) = url {
                        text_segments.push(format!("![{label}]({u})"));
                    } else {
                        text_segments.push(label.to_string());
                    }
                }
                OutgoingContentPart::File { name, .. } => {
                    let label = name.as_deref().unwrap_or("file");
                    text_segments.push(format!("[File: {label}]"));
                }
                OutgoingContentPart::Audio { name, .. } => {
                    if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
                        text_segments.push(format!("[Audio: {label}]"));
                    } else {
                        text_segments.push("[Audio]".to_string());
                    }
                }
                OutgoingContentPart::Video { name, .. } => {
                    if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
                        text_segments.push(format!("[Video: {label}]"));
                    } else {
                        text_segments.push("[Video]".to_string());
                    }
                }
                OutgoingContentPart::Raw { value } => {
                    text_segments.push(value.to_string());
                }
                OutgoingContentPart::Thinking { .. } => {}
            }
        }

        let mut payloads = Vec::new();

        if !text_segments.is_empty() {
            let joined = text_segments.join("\n");
            let payload = self.render_text(&joined);
            payloads.push(payload);
        }

        payloads.extend(image_payloads);
        payloads
    }

    fn render_text(&self, text: &str) -> DingTalkPayload {
        if has_long_code_block(text) {
            return self.render_as_action_card(text);
        }

        let normalized = normalize_dingtalk_markdown(text);

        if normalized.len() > MARKDOWN_MAX_LENGTH {
            return DingTalkPayload::Text {
                content: normalized,
            };
        }

        let title = make_title(&normalized);
        DingTalkPayload::Markdown {
            title,
            text: normalized,
        }
    }

    fn render_as_action_card(&self, text: &str) -> DingTalkPayload {
        let summary = extract_summary(text);
        let normalized = normalize_dingtalk_markdown(&summary);
        let title = make_title(&normalized);

        let console_url = self.console_base_url.as_deref().unwrap_or("rua://console");

        DingTalkPayload::ActionCard {
            title: title.clone(),
            text: format!("{normalized}\n\n---\n\n> 完整内容包含长代码块，请在控制台查看。"),
            single_title: "查看完整内容".to_string(),
            single_url: console_url.to_string(),
        }
    }
}

static NUMBERED_LIST_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d+\.\s").unwrap());

static CODE_BLOCK_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?ms)^([ \t]*)(```[^\n]*\n.*?\n```)[ \t]*$").unwrap());

static TABLE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\|(.+\|)+\s*$").unwrap());

static TABLE_SEP_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\|[\s:-]+(\|[\s:-]+)+\|\s*$").unwrap());

static FENCED_BLOCK_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?ms)```[^\n]*\n(.*?)\n```").unwrap());

fn ensure_list_spacing(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::with_capacity(lines.len());

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let is_numbered = NUMBERED_LIST_RE.is_match(trimmed);
        if is_numbered && i > 0 {
            let prev = lines[i - 1];
            let prev_empty = prev.trim().is_empty();
            let prev_numbered = NUMBERED_LIST_RE.is_match(prev.trim_start());
            if !prev_empty && !prev_numbered {
                out.push("");
            }
        }
        out.push(line);
    }

    out.join("\n")
}

fn dedent_code_blocks(text: &str) -> String {
    CODE_BLOCK_RE
        .replace_all(text, |caps: &regex::Captures| {
            let indent = caps.get(1).map_or("", |m| m.as_str());
            let block = caps.get(2).map_or("", |m| m.as_str());

            if indent.is_empty() {
                return block.to_string();
            }

            let n = indent.len();
            block
                .split('\n')
                .map(|ln| if ln.starts_with(indent) { &ln[n..] } else { ln })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .into_owned()
}

fn tables_to_lists(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut result = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        if i + 1 < lines.len() && TABLE_RE.is_match(lines[i]) && TABLE_SEP_RE.is_match(lines[i + 1])
        {
            let headers: Vec<&str> = lines[i]
                .trim()
                .trim_matches('|')
                .split('|')
                .map(|s| s.trim())
                .collect();

            i += 2;

            while i < lines.len() && TABLE_RE.is_match(lines[i]) {
                let cells: Vec<&str> = lines[i]
                    .trim()
                    .trim_matches('|')
                    .split('|')
                    .map(|s| s.trim())
                    .collect();

                let mut parts = Vec::new();
                for (j, cell) in cells.iter().enumerate() {
                    if !cell.is_empty() {
                        let label = headers.get(j).copied().unwrap_or("?");
                        parts.push(format!("**{label}**: {cell}"));
                    }
                }
                if !parts.is_empty() {
                    result.push(format!("- {}", parts.join(" | ")));
                }
                i += 1;
            }
            continue;
        }

        result.push(lines[i].to_string());
        i += 1;
    }

    result.join("\n")
}

pub fn normalize_dingtalk_markdown(text: &str) -> String {
    let text = ensure_list_spacing(text);
    let text = dedent_code_blocks(&text);
    tables_to_lists(&text)
}

fn has_long_code_block(text: &str) -> bool {
    for caps in FENCED_BLOCK_RE.captures_iter(text) {
        if let Some(content) = caps.get(1) {
            if content.as_str().len() > LONG_CODE_BLOCK_THRESHOLD {
                return true;
            }
        }
    }
    false
}

fn make_title(text: &str) -> String {
    let preview: String = text.chars().take(10).collect();
    format!("\u{1f4ac}{preview}...")
}

fn extract_summary(text: &str) -> String {
    FENCED_BLOCK_RE
        .replace_all(text, |caps: &regex::Captures| {
            let content = caps.get(1).map_or("", |m| m.as_str());
            if content.len() > LONG_CODE_BLOCK_THRESHOLD {
                "```\n[... 代码已省略，请在控制台查看 ...]\n```".to_string()
            } else {
                caps.get(0).map_or("", |m| m.as_str()).to_string()
            }
        })
        .into_owned()
}

pub async fn upload_media(
    client: &reqwest::Client,
    access_token: &str,
    data: &[u8],
    media_type: &str,
    filename: Option<&str>,
) -> rua_core::Result<String> {
    upload_media_with_base_url(
        client,
        "https://oapi.dingtalk.com",
        access_token,
        data,
        media_type,
        filename,
    )
    .await
}

pub async fn upload_media_with_base_url(
    client: &reqwest::Client,
    base_url: &str,
    access_token: &str,
    data: &[u8],
    media_type: &str,
    filename: Option<&str>,
) -> rua_core::Result<String> {
    let name = filename.unwrap_or(match media_type {
        "image" => "upload.jpg",
        _ => "upload.bin",
    });

    let url = format!(
        "{}/media/upload?access_token={access_token}&type={media_type}",
        base_url.trim_end_matches('/')
    );

    let part = reqwest::multipart::Part::bytes(data.to_vec())
        .file_name(name.to_string())
        .mime_str(
            mime_guess::from_path(name)
                .first_raw()
                .unwrap_or("application/octet-stream"),
        )
        .map_err(|e| rua_core::RuaError::Runtime {
            message: format!("mime error: {e}"),
        })?;

    let form = reqwest::multipart::Form::new().part("media", part);

    let resp = client
        .post(&url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| rua_core::RuaError::Runtime {
            message: format!("upload_media request failed: {e}"),
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(rua_core::RuaError::Runtime {
            message: format!("upload_media HTTP {status}: {body}"),
        });
    }

    let result: Value = resp.json().await.map_err(|e| rua_core::RuaError::Runtime {
        message: format!("upload_media parse response: {e}"),
    })?;

    let errcode = result.get("errcode").and_then(|v| v.as_i64()).unwrap_or(0);
    if errcode != 0 {
        return Err(rua_core::RuaError::Runtime {
            message: format!("upload_media errcode={errcode}: {result}"),
        });
    }

    result
        .get("media_id")
        .or_else(|| result.get("mediaId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| rua_core::RuaError::Runtime {
            message: format!("upload_media: no media_id in response: {result}"),
        })
}
