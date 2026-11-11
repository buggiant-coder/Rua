use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_DISPLAY_MEDIA_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaOrigin {
    pub device_id: String,
    pub src: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<PathBuf>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayMedia {
    pub media_ref: String,
    pub mime_type: String,
    pub byte_size: u64,
    pub sha256: String,
    pub origin: MediaOrigin,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

struct ImageSource {
    range: Range<usize>,
    src: String,
    markdown_alt: Option<String>,
    markdown_title: String,
}

/// One cache belongs to one Turn, so repeated incremental writes retain identity.
#[derive(Default)]
pub struct DisplayMediaArchive {
    imported: HashMap<String, DisplayMedia>,
}

impl DisplayMediaArchive {
    pub fn store_image_bytes(
        storage_root: &Path,
        bytes: &[u8],
        mime: &str,
        device_id: &str,
        source: &str,
    ) -> io::Result<DisplayMedia> {
        if bytes.is_empty()
            || bytes.len() as u64 > MAX_DISPLAY_MEDIA_BYTES
            || !mime.starts_with("image/")
        {
            return Err(io::Error::other("invalid display image bytes or MIME type"));
        }
        let id = uuid::Uuid::new_v4();
        let item = DisplayMedia {
            media_ref: format!("media/{id}"),
            mime_type: mime.into(),
            byte_size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            origin: MediaOrigin {
                device_id: device_id.into(),
                src: source.into(),
                base_dir: None,
                extra: Default::default(),
            },
            extra: Default::default(),
        };
        let directory = storage_root.join("media");
        fs::create_dir_all(&directory)?;
        atomic_write(
            &directory.join(format!(".{id}.media.json")),
            &serde_json::to_vec(&item)?,
        )?;
        atomic_write(&directory.join(id.to_string()), bytes)?;
        Ok(item)
    }

    pub fn rewrite(
        &mut self,
        text: &str,
        storage_root: &Path,
        base_dir: &Path,
        device_id: &str,
        resolve: impl Fn(&Path) -> io::Result<PathBuf>,
    ) -> io::Result<(String, Vec<DisplayMedia>)> {
        let mut output = text.to_string();
        let mut media = Vec::new();
        let mut replacements = Vec::new();
        for image in image_sources(text) {
            if !is_local_source(&image.src) {
                continue;
            }
            if device_id.is_empty() || device_id == "unknown" {
                return Err(io::Error::other("display image source Device is unknown"));
            }
            let item = if let Some(item) = self.imported.get(&image.src) {
                item.clone()
            } else {
                let raw_path = image.src.strip_prefix("file://").unwrap_or(&image.src);
                let path = resolve(Path::new(raw_path))?;
                let mut file = File::open(&path)?;
                let metadata = file.metadata()?;
                if !metadata.is_file()
                    || metadata.len() == 0
                    || metadata.len() > MAX_DISPLAY_MEDIA_BYTES
                {
                    return Err(io::Error::other(
                        "display image must be a nonempty regular file of at most 50 MiB",
                    ));
                }
                let mime = mime_guess::from_path(&path)
                    .first_or_octet_stream()
                    .to_string();
                if !mime.starts_with("image/") {
                    return Err(io::Error::other(
                        "display image path is not a supported image",
                    ));
                }
                let mut bytes = Vec::new();
                (&mut file)
                    .take(MAX_DISPLAY_MEDIA_BYTES + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_DISPLAY_MEDIA_BYTES {
                    return Err(io::Error::other("display image exceeds 50 MiB"));
                }
                let id = uuid::Uuid::new_v4();
                let item = DisplayMedia {
                    media_ref: format!("media/{id}"),
                    mime_type: mime,
                    byte_size: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                    origin: MediaOrigin {
                        device_id: device_id.into(),
                        src: image.src.clone(),
                        base_dir: (!Path::new(raw_path).is_absolute()
                            && !raw_path.starts_with('~'))
                        .then(|| base_dir.to_path_buf()),
                        extra: Default::default(),
                    },
                    extra: Default::default(),
                };
                let directory = storage_root.join("media");
                fs::create_dir_all(&directory)?;
                atomic_write(
                    &directory.join(format!(".{id}.media.json")),
                    &serde_json::to_vec(&item)?,
                )?;
                atomic_write(&directory.join(id.to_string()), &bytes)?;
                self.imported.insert(image.src.clone(), item.clone());
                item
            };
            let replacement = match image.markdown_alt {
                Some(alt) => {
                    let title = if image.markdown_title.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " \"{}\"",
                            image
                                .markdown_title
                                .replace('\\', "\\\\")
                                .replace('"', "\\\"")
                        )
                    };
                    format!("![{alt}]({}{title})", item.media_ref)
                }
                None => format!("\"{}\"", item.media_ref),
            };
            replacements.push((image.range, replacement));
            if !media
                .iter()
                .any(|entry: &DisplayMedia| entry.media_ref == item.media_ref)
            {
                media.push(item);
            }
        }
        for (range, replacement) in replacements.into_iter().rev() {
            output.replace_range(range, &replacement);
        }
        Ok((output, media))
    }
}

pub fn managed_image_refs(text: &str) -> Vec<String> {
    image_sources(text)
        .into_iter()
        .filter_map(|image| {
            image.src.strip_prefix("media/")?;
            Some(image.src)
        })
        .collect()
}

fn is_local_source(src: &str) -> bool {
    !src.is_empty()
        && !src.starts_with("media/")
        && !["http:", "https:", "data:", "blob:"]
            .iter()
            .any(|prefix| src.to_ascii_lowercase().starts_with(prefix))
}

fn image_sources(text: &str) -> Vec<ImageSource> {
    let mut images = Vec::new();
    let mut excluded = Vec::new();
    let mut code_start = None;
    let mut image: Option<(Range<usize>, String, String, String)> = None;
    for (event, range) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code_start = Some(range.start),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(start) = code_start.take() {
                    excluded.push(start..range.end);
                }
            }
            Event::Code(_) | Event::Html(_) | Event::InlineHtml(_) => excluded.push(range),
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                image = Some((
                    range,
                    dest_url.to_string(),
                    String::new(),
                    title.to_string(),
                ))
            }
            Event::Text(value) => {
                if let Some((_, _, alt, _)) = &mut image {
                    alt.push_str(&value);
                }
            }
            Event::End(TagEnd::Image) => {
                if let Some((mut span, src, alt, title)) = image.take() {
                    span.end = range.end;
                    images.push(ImageSource {
                        range: span,
                        src,
                        markdown_title: title,
                        markdown_alt: Some(
                            alt.replace('\\', "\\\\")
                                .replace('[', "\\[")
                                .replace(']', "\\]"),
                        ),
                    });
                }
            }
            _ => {}
        }
    }
    let mut offset = 0;
    let mut figure: Option<(usize, Vec<ImageSource>)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        let blocked = excluded
            .iter()
            .any(|range| range.contains(&(offset + line.len() - line.trim_start().len())));
        if !blocked && (trimmed == "@figure" || trimmed.starts_with("@figure ")) {
            figure = Some((offset, Vec::new()));
        } else if !blocked && trimmed == "@end" {
            if let Some((start, items)) = figure.take() {
                images.retain(|image| {
                    image.range.start < start || image.range.start >= offset + line.len()
                });
                images.extend(items);
            }
        } else if !blocked {
            if let Some((_, items)) = &mut figure {
                if let Some(body) = trimmed.strip_prefix("- ") {
                    let body_offset = offset + line.find(body).unwrap_or(0);
                    if let Some((range, src)) = figure_src(body) {
                        items.push(ImageSource {
                            range: body_offset + range.start..body_offset + range.end,
                            src,
                            markdown_alt: None,
                            markdown_title: String::new(),
                        });
                    }
                }
            }
        }
        offset += line.len();
    }
    images.sort_by_key(|image| image.range.start);
    images
}

fn figure_src(body: &str) -> Option<(Range<usize>, String)> {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || b"_-".contains(&bytes[i])) {
            i += 1;
        }
        if i == key_start {
            break;
        }
        let key = &body[key_start..i];
        if bytes.get(i) != Some(&b'=') {
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            continue;
        }
        i += 1;
        let start = i;
        let value = if bytes.get(i) == Some(&b'"') {
            i += 1;
            let mut decoded = String::new();
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                }
                let c = body[i..].chars().next()?;
                decoded.push(c);
                i += c.len_utf8();
            }
            if i == bytes.len() {
                return None;
            }
            i += 1;
            decoded
        } else {
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            body[start..i].to_string()
        };
        if key == "src" {
            return Some((start..i, value));
        }
    }
    None
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("media path has no parent"))?;
    let temp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        rua_core::fs::sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
