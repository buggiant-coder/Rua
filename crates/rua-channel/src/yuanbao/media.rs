use std::collections::HashMap;
use std::path::Path;

use md5::{Digest, Md5};
use reqwest::Client;

use rua_core::{Result, RuaError};

pub const UPLOAD_INFO_PATH: &str = "/api/resource/genUploadInfo";
pub const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadResult {
    pub url: String,
    pub filename: String,
    pub size: usize,
    pub mime_type: String,
    pub uuid_hex: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
struct CosUploadConfig {
    upload_url: String,
    resource_url: String,
}

pub async fn download_and_upload_media(
    media_path: &str,
    client: &Client,
    api_domain: &str,
    auth_headers: &HashMap<String, String>,
) -> Result<UploadResult> {
    let (data, filename) = load_media_data(media_path, client).await?;
    if data.len() > MAX_UPLOAD_BYTES {
        return Err(RuaError::Validation {
            message: format!(
                "yuanbao media too large: {} bytes > {} bytes",
                data.len(),
                MAX_UPLOAD_BYTES
            ),
        });
    }
    let mime_type = guess_mime(&filename);
    let uuid_hex = md5_hex(&data);
    let (width, height) = if mime_type.starts_with("image/") {
        parse_image_size(&data)
    } else {
        (0, 0)
    };
    let config = get_upload_info(client, api_domain, auth_headers, &filename).await?;
    upload_to_cos(client, &config, &data, &mime_type).await?;

    Ok(UploadResult {
        url: config.resource_url,
        filename,
        size: data.len(),
        mime_type,
        uuid_hex,
        width,
        height,
    })
}

pub fn build_image_msg_body(result: &UploadResult) -> Vec<(String, serde_json::Value)> {
    vec![(
        "TIMImageElem".to_string(),
        serde_json::json!({
            "uuid": result.uuid_hex,
            "image_format": 255,
            "image_info_array": [{
                "type": 1,
                "size": result.size,
                "width": result.width,
                "height": result.height,
                "url": result.url,
            }],
        }),
    )]
}

pub fn build_file_msg_body(result: &UploadResult) -> Vec<(String, serde_json::Value)> {
    vec![(
        "TIMFileElem".to_string(),
        serde_json::json!({
            "uuid": result.uuid_hex,
            "file_name": result.filename,
            "file_size": result.size,
            "url": result.url,
        }),
    )]
}

async fn load_media_data(media_path: &str, client: &Client) -> Result<(Vec<u8>, String)> {
    if let Some(path) = resolve_local_path(media_path) {
        let data = tokio::fs::read(&path).await.map_err(|err| RuaError::Io {
            message: format!("read yuanbao media {}: {err}", path.display()),
        })?;
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("file")
            .to_string();
        return Ok((data, filename));
    }

    if media_path.starts_with("http://") || media_path.starts_with("https://") {
        let resp = client
            .get(media_path)
            .send()
            .await
            .map_err(|err| RuaError::Runtime {
                message: format!("download yuanbao media failed: {err}"),
            })?;
        if !resp.status().is_success() {
            return Err(RuaError::Runtime {
                message: format!("download yuanbao media HTTP {}", resp.status()),
            });
        }
        let filename = media_path
            .split('?')
            .next()
            .and_then(|clean| clean.rsplit('/').next())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("file")
            .to_string();
        let data = resp
            .bytes()
            .await
            .map_err(|err| RuaError::Runtime {
                message: format!("read yuanbao media response failed: {err}"),
            })?
            .to_vec();
        return Ok((data, filename));
    }

    Err(RuaError::Validation {
        message: format!("cannot resolve yuanbao media path: {media_path}"),
    })
}

async fn get_upload_info(
    client: &Client,
    api_domain: &str,
    auth_headers: &HashMap<String, String>,
    filename: &str,
) -> Result<CosUploadConfig> {
    let domain = api_domain
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let url = format!("http://{domain}{UPLOAD_INFO_PATH}");
    let mut request = client.post(&url);
    for (key, value) in auth_headers {
        request = request.header(key, value);
    }
    let resp = request
        .json(&serde_json::json!({
            "fileName": filename,
            "fileId": uuid::Uuid::new_v4().simple().to_string(),
            "docFrom": "localDoc",
            "docOpenId": "",
        }))
        .send()
        .await
        .map_err(|err| RuaError::Runtime {
            message: format!("yuanbao genUploadInfo failed: {err}"),
        })?;
    if !resp.status().is_success() {
        return Err(RuaError::Runtime {
            message: format!("yuanbao genUploadInfo HTTP {}", resp.status()),
        });
    }
    let mut body: serde_json::Value = resp.json().await.map_err(|err| RuaError::Runtime {
        message: format!("yuanbao genUploadInfo parse failed: {err}"),
    })?;
    if body.get("data").is_some_and(|value| value.is_object()) {
        body = body["data"].clone();
    }
    let upload_url = body
        .get("upload_full_url")
        .or_else(|| body.get("uploadFullUrl"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuaError::Runtime {
            message: format!("yuanbao genUploadInfo missing upload_full_url: {body}"),
        })?
        .to_string();
    let resource_url = body
        .get("resourceUrl")
        .or_else(|| body.get("resource_url"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&upload_url)
        .to_string();
    Ok(CosUploadConfig {
        upload_url,
        resource_url,
    })
}

async fn upload_to_cos(
    client: &Client,
    config: &CosUploadConfig,
    data: &[u8],
    mime_type: &str,
) -> Result<()> {
    let resp = client
        .put(&config.upload_url)
        .header(reqwest::header::CONTENT_TYPE, mime_type)
        .body(data.to_vec())
        .send()
        .await
        .map_err(|err| RuaError::Runtime {
            message: format!("yuanbao COS upload failed: {err}"),
        })?;
    if !resp.status().is_success() {
        return Err(RuaError::Runtime {
            message: format!("yuanbao COS upload HTTP {}", resp.status()),
        });
    }
    Ok(())
}

fn resolve_local_path(raw: &str) -> Option<std::path::PathBuf> {
    if raw.starts_with("file://") {
        return Some(std::path::PathBuf::from(raw.trim_start_matches("file://")));
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    None
}

fn guess_mime(filename: &str) -> String {
    match Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn md5_hex(data: &[u8]) -> String {
    let mut md5 = Md5::new();
    md5.update(data);
    format!("{:x}", md5.finalize())
}

fn parse_image_size(data: &[u8]) -> (u32, u32) {
    if data.len() >= 24 && data.starts_with(b"\x89PNG") {
        let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return (width, height);
    }
    (0, 0)
}
