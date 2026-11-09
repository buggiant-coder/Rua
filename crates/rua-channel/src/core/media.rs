use std::path::{Component, Path, PathBuf};

use base64::Engine;
use reqwest::Client;
use rua_core::RuaError;

use crate::core::content::OutgoingContentPart;

const MAX_MEDIA_SIZE: u64 = 50 * 1024 * 1024;
const DEFAULT_IMAGE_NAME: &str = "image.png";
const DEFAULT_FILE_NAME: &str = "file.bin";
const DEFAULT_AUDIO_NAME: &str = "audio.ogg";
const DEFAULT_VIDEO_NAME: &str = "video.mp4";

#[derive(Debug)]
pub struct ResolvedMedia {
    pub path: PathBuf,
    pub name: String,
}

pub async fn resolve_outgoing_media(
    part: &OutgoingContentPart,
    client: &Client,
) -> rua_core::Result<Option<ResolvedMedia>> {
    match part {
        OutgoingContentPart::Image { url, path, alt } => {
            let name_hint = alt
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_IMAGE_NAME);
            resolve_media_fields(url.as_deref(), path.as_deref(), name_hint, client).await
        }
        OutgoingContentPart::File { url, path, name } => {
            let name_hint = name
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_FILE_NAME);
            resolve_media_fields(url.as_deref(), path.as_deref(), name_hint, client).await
        }
        OutgoingContentPart::Audio { url, path, name } => {
            let name_hint = name
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_AUDIO_NAME);
            resolve_media_fields(url.as_deref(), path.as_deref(), name_hint, client).await
        }
        OutgoingContentPart::Video { url, path, name } => {
            let name_hint = name
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_VIDEO_NAME);
            resolve_media_fields(url.as_deref(), path.as_deref(), name_hint, client).await
        }
        _ => Ok(None),
    }
}

async fn resolve_media_fields(
    url: Option<&str>,
    path: Option<&Path>,
    name_hint: &str,
    client: &Client,
) -> rua_core::Result<Option<ResolvedMedia>> {
    let safe_name = sanitize_filename(name_hint);

    if let Some(url) = url {
        if url.starts_with("data:") {
            let data = parse_data_url(url)?;
            return decode_data_url(&data, &safe_name).map(Some);
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            return download_url(url, &safe_name, client).await.map(Some);
        }
        if url.starts_with("file://") {
            return resolve_file_url(url, &safe_name).map(Some);
        }
    }

    if let Some(p) = path {
        let safe = sanitize_path(p)?;
        validate_local_media_path(&safe)?;
        let name = safe
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| safe_name.clone());
        return Ok(Some(ResolvedMedia { path: safe, name }));
    }

    Ok(None)
}

pub fn sanitize_filename(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| DEFAULT_FILE_NAME.to_string());

    if base.is_empty() || base == "." || base == ".." {
        return DEFAULT_FILE_NAME.to_string();
    }

    base
}

fn sanitize_path(path: &Path) -> rua_core::Result<PathBuf> {
    let s = path.to_string_lossy();
    if s.contains("..") {
        return Err(RuaError::Security {
            message: format!("path traversal rejected: {s}"),
        });
    }
    if path.is_absolute() {
        return Err(RuaError::Security {
            message: format!("absolute path rejected: {s}"),
        });
    }
    Ok(path.to_path_buf())
}

fn validate_local_media_path(path: &Path) -> rua_core::Result<()> {
    let metadata = std::fs::metadata(path).map_err(|_| RuaError::Io {
        message: format!("local media file not found: {}", path.display()),
    })?;

    if !metadata.is_file() {
        return Err(RuaError::Validation {
            message: format!("local media path is not a file: {}", path.display()),
        });
    }

    let size = metadata.len();
    if size == 0 {
        return Err(RuaError::Validation {
            message: format!("local media file is empty: {}", path.display()),
        });
    }
    if size > MAX_MEDIA_SIZE {
        return Err(RuaError::Validation {
            message: format!("media too large: {size} bytes (max {MAX_MEDIA_SIZE})"),
        });
    }

    Ok(())
}

fn resolve_file_url(url: &str, safe_name: &str) -> rua_core::Result<ResolvedMedia> {
    let mut raw_path = url.strip_prefix("file://").unwrap_or(url);
    if let Some(localhost_path) = raw_path.strip_prefix("localhost") {
        raw_path = localhost_path;
    }
    if raw_path.is_empty() || !raw_path.starts_with('/') {
        return Err(RuaError::Validation {
            message: format!("unsupported file URL: {url}"),
        });
    }

    let decoded = percent_decode_path(raw_path)?;
    let path = PathBuf::from(decoded);
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(RuaError::Security {
            message: format!("path traversal rejected: {raw_path}"),
        });
    }
    validate_local_media_path(&path)?;

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| safe_name.to_string());
    Ok(ResolvedMedia { path, name })
}

fn percent_decode_path(input: &str) -> rua_core::Result<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(RuaError::Validation {
                    message: format!("invalid percent encoding in file URL path: {input}"),
                });
            }
            let hi = hex_value(bytes[i + 1]).ok_or_else(|| RuaError::Validation {
                message: format!("invalid percent encoding in file URL path: {input}"),
            })?;
            let lo = hex_value(bytes[i + 2]).ok_or_else(|| RuaError::Validation {
                message: format!("invalid percent encoding in file URL path: {input}"),
            })?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(out).map_err(|e| RuaError::Validation {
        message: format!("file URL path is not valid UTF-8: {e}"),
    })
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

struct DataUrl {
    mime: String,
    data: Vec<u8>,
}

fn parse_data_url(url: &str) -> rua_core::Result<DataUrl> {
    let rest = url
        .strip_prefix("data:")
        .ok_or_else(|| RuaError::Validation {
            message: "unsupported data URL".to_string(),
        })?;
    let (header, encoded) = rest
        .split_once(";base64,")
        .ok_or_else(|| RuaError::Validation {
            message: "data URL must use ;base64,".to_string(),
        })?;
    let mime = if header.is_empty() {
        "application/octet-stream".to_string()
    } else {
        header.to_string()
    };
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|e| RuaError::Validation {
            message: format!("invalid data URL base64: {e}"),
        })?;
    Ok(DataUrl { mime, data })
}

fn decode_data_url(data_url: &DataUrl, name_hint: &str) -> rua_core::Result<ResolvedMedia> {
    let size = data_url.data.len() as u64;
    if size == 0 {
        return Err(RuaError::Validation {
            message: "decoded media file is empty".to_string(),
        });
    }
    if size > MAX_MEDIA_SIZE {
        return Err(RuaError::Validation {
            message: format!("media too large: {size} bytes (max {MAX_MEDIA_SIZE})"),
        });
    }

    let ext = mime_to_ext(&data_url.mime);
    let name = ensure_extension(name_hint, &ext);

    let dir = std::env::temp_dir().join("rua-media");
    std::fs::create_dir_all(&dir).map_err(|e| RuaError::Io {
        message: format!("failed to create temp dir: {e}"),
    })?;

    let file_path = dir.join(format!("{}-{}", uuid::Uuid::new_v4().as_simple(), name));
    std::fs::write(&file_path, &data_url.data).map_err(|e| RuaError::Io {
        message: format!("failed to write decoded media: {e}"),
    })?;

    Ok(ResolvedMedia {
        path: file_path,
        name,
    })
}

async fn download_url(
    url: &str,
    name_hint: &str,
    client: &Client,
) -> rua_core::Result<ResolvedMedia> {
    let resp = client.get(url).send().await.map_err(|e| RuaError::Io {
        message: format!("media download failed: {e}"),
    })?;

    if !resp.status().is_success() {
        return Err(RuaError::Io {
            message: format!("media download returned status {}", resp.status()),
        });
    }

    if let Some(len) = resp.content_length() {
        if len > MAX_MEDIA_SIZE {
            return Err(RuaError::Validation {
                message: format!("media too large: {len} bytes (max {MAX_MEDIA_SIZE})"),
            });
        }
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let content_disposition_name = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(content_disposition_filename);

    let bytes = resp.bytes().await.map_err(|e| RuaError::Io {
        message: format!("media download body read failed: {e}"),
    })?;

    if bytes.is_empty() {
        return Err(RuaError::Validation {
            message: "downloaded media file is empty".to_string(),
        });
    }
    if bytes.len() as u64 > MAX_MEDIA_SIZE {
        return Err(RuaError::Validation {
            message: format!(
                "media too large: {} bytes (max {MAX_MEDIA_SIZE})",
                bytes.len()
            ),
        });
    }

    let dir = std::env::temp_dir().join("rua-media");
    std::fs::create_dir_all(&dir).map_err(|e| RuaError::Io {
        message: format!("failed to create temp dir: {e}"),
    })?;

    let mut name = content_disposition_name.unwrap_or_else(|| url_to_filename(url, name_hint));
    name = repair_file_extension(name, content_type.as_deref(), &bytes);
    let file_path = dir.join(format!("{}-{}", uuid::Uuid::new_v4().as_simple(), name));
    std::fs::write(&file_path, &bytes).map_err(|e| RuaError::Io {
        message: format!("failed to write downloaded media: {e}"),
    })?;

    Ok(ResolvedMedia {
        path: file_path,
        name,
    })
}

fn url_to_filename(url: &str, fallback: &str) -> String {
    url.rsplit('/')
        .next()
        .and_then(|seg| seg.split('?').next())
        .filter(|s| !s.is_empty() && s.len() < 200)
        .map(sanitize_filename)
        .unwrap_or_else(|| fallback.to_string())
}

fn content_disposition_filename(header: &str) -> Option<String> {
    let mut fallback = None;
    for part in header.split(';') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        if value.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case("filename*") {
            if let Some(decoded) = decode_rfc5987_filename(value) {
                return Some(decoded);
            }
        } else if key.eq_ignore_ascii_case("filename") {
            fallback = Some(sanitize_filename(value));
        }
    }
    fallback
}

fn decode_rfc5987_filename(value: &str) -> Option<String> {
    let (charset, encoded) = value.split_once("''")?;
    if !charset.eq_ignore_ascii_case("utf-8") {
        return None;
    }
    percent_decode_path(encoded)
        .ok()
        .map(|decoded| sanitize_filename(&decoded))
}

fn mime_to_ext(mime: &str) -> String {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/wav" => "wav",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "application/pdf" => "pdf",
        _ => "bin",
    }
    .to_string()
}

fn ensure_extension(name: &str, default_ext: &str) -> String {
    if Path::new(name).extension().is_some() {
        name.to_string()
    } else {
        format!("{name}.{default_ext}")
    }
}

fn repair_file_extension(name: String, content_type: Option<&str>, bytes: &[u8]) -> String {
    let path = Path::new(&name);
    if path.extension().and_then(|ext| ext.to_str()) != Some("file") {
        return name;
    }

    let ext = content_type
        .map(mime_to_ext)
        .filter(|ext| ext != "bin")
        .or_else(|| guess_ext_from_magic(bytes));

    let Some(ext) = ext else {
        return name;
    };

    let mut repaired = PathBuf::from(name);
    repaired.set_extension(ext);
    repaired
        .file_name()
        .map(|file_name| file_name.to_string_lossy().into_owned())
        .unwrap_or_else(|| DEFAULT_FILE_NAME.to_string())
}

fn guess_ext_from_magic(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(b"%PDF") {
        return Some("pdf".into());
    }
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Some("zip".into());
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("png".into());
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("jpg".into());
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif".into());
    }
    if bytes.starts_with(b"\xd0\xcf\x11\xe0") {
        return Some("doc".into());
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("webp".into());
    }
    None
}
