use super::inbound::{LarkInboundResource, LarkInboundResourceKind};
use anyhow::{Context, Result};
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES: usize = 30 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LarkDownloadedResource {
    pub(crate) kind: LarkInboundResourceKind,
    pub(crate) file_key: String,
    pub(crate) path: PathBuf,
}

pub(crate) fn infer_extension(
    content_type: Option<&str>,
    bytes: &[u8],
    fallback_name: Option<&str>,
    kind: LarkInboundResourceKind,
) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return "png";
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "jpg";
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return "gif";
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return "webp";
    }
    if bytes.starts_with(b"%PDF") {
        return "pdf";
    }

    if let Some(name) = fallback_name {
        if let Some(ext) = Path::new(name).extension().and_then(|ext| ext.to_str()) {
            let normalized = ext.to_ascii_lowercase();
            return match normalized.as_str() {
                "png" => "png",
                "jpg" | "jpeg" => "jpg",
                "gif" => "gif",
                "webp" => "webp",
                "pdf" => "pdf",
                "mp3" => "mp3",
                "mp4" => "mp4",
                "wav" => "wav",
                "txt" => "txt",
                "md" => "md",
                "csv" => "csv",
                "json" => "json",
                "doc" => "doc",
                "docx" => "docx",
                _ => default_extension_for_kind(kind),
            };
        }
    }

    if let Some(content_type) = content_type {
        let lower = content_type.to_ascii_lowercase();
        if lower.contains("png") {
            return "png";
        }
        if lower.contains("jpeg") || lower.contains("jpg") {
            return "jpg";
        }
        if lower.contains("gif") {
            return "gif";
        }
        if lower.contains("webp") {
            return "webp";
        }
        if lower.contains("pdf") {
            return "pdf";
        }
        if lower.contains("mpeg") || lower.contains("mp3") {
            return "mp3";
        }
        if lower.contains("mp4") {
            return "mp4";
        }
        if lower.contains("wav") {
            return "wav";
        }
        if lower.contains("json") {
            return "json";
        }
        if lower.contains("csv") {
            return "csv";
        }
        if lower.contains("markdown") {
            return "md";
        }
        if lower.contains("text/plain") {
            return "txt";
        }
    }

    default_extension_for_kind(kind)
}

fn default_extension_for_kind(kind: LarkInboundResourceKind) -> &'static str {
    match kind {
        LarkInboundResourceKind::Image => "png",
        LarkInboundResourceKind::Sticker => "png",
        LarkInboundResourceKind::Audio => "mp3",
        LarkInboundResourceKind::Video => "mp4",
        LarkInboundResourceKind::File => "bin",
    }
}

pub(crate) fn extract_response_file_name(response: &reqwest::Response) -> Option<String> {
    let disposition = response
        .headers()
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())?;
    let match_ = disposition
        .split(';')
        .find_map(|part| part.trim().strip_prefix("filename="))
        .or_else(|| {
            disposition
                .split(';')
                .find_map(|part| part.trim().strip_prefix("filename*=UTF-8''"))
        })?;
    let trimmed = match_.trim_matches('"').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) async fn store_inbound_resource(
    workspace_dir: &Path,
    channel_name: &str,
    account_id: &str,
    message_id: &str,
    resource: &LarkInboundResource,
    bytes: &[u8],
    content_type: Option<&str>,
    response_file_name: Option<&str>,
) -> Result<LarkDownloadedResource> {
    let account_segment = if account_id.is_empty() {
        "default"
    } else {
        account_id
    };
    let file_name_hint = response_file_name.or(resource.file_name.as_deref());
    let ext = infer_extension(content_type, bytes, file_name_hint, resource.kind);
    let prefix = match resource.kind {
        LarkInboundResourceKind::Image => "image",
        LarkInboundResourceKind::Sticker => "sticker",
        LarkInboundResourceKind::File => "file",
        LarkInboundResourceKind::Audio => "audio",
        LarkInboundResourceKind::Video => "video",
    };
    let output_dir = workspace_dir
        .join("channels")
        .join(channel_name)
        .join(account_segment)
        .join("inbound");
    tokio::fs::create_dir_all(&output_dir)
        .await
        .with_context(|| format!("create Lark inbound dir {}", output_dir.display()))?;
    let output_path = output_dir.join(format!(
        "{message_id}_{prefix}_{file_key}.{ext}",
        file_key = resource.file_key
    ));
    tokio::fs::write(&output_path, bytes)
        .await
        .with_context(|| format!("write Lark inbound resource {}", output_path.display()))?;
    Ok(LarkDownloadedResource {
        kind: resource.kind,
        file_key: resource.file_key.clone(),
        path: output_path,
    })
}

pub(crate) async fn store_inbound_resource_with_limit(
    workspace_dir: &Path,
    channel_name: &str,
    account_id: &str,
    message_id: &str,
    resource: &LarkInboundResource,
    bytes: &[u8],
    content_type: Option<&str>,
    response_file_name: Option<&str>,
    max_bytes: usize,
) -> Result<LarkDownloadedResource> {
    if bytes.len() > max_bytes {
        anyhow::bail!(
            "Lark inbound resource exceeds size limit: {} bytes > {} bytes",
            bytes.len(),
            max_bytes
        );
    }
    store_inbound_resource(
        workspace_dir,
        channel_name,
        account_id,
        message_id,
        resource,
        bytes,
        content_type,
        response_file_name,
    )
    .await
}

pub(crate) fn content_type_from_response(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

pub(crate) async fn materialize_outbound_attachment(
    client: &reqwest::Client,
    workspace_dir: Option<&Path>,
    channel_name: &str,
    account_id: &str,
    kind: LarkInboundResourceKind,
    target: &str,
) -> Result<PathBuf> {
    if let Some(path) = target.strip_prefix("file://") {
        let file_path = PathBuf::from(path);
        if !file_path.is_file() {
            anyhow::bail!("Lark outbound file URL is not a readable file: {target}");
        }
        return Ok(file_path);
    }

    let response = client
        .get(target)
        .send()
        .await
        .with_context(|| format!("download Lark outbound attachment {target}"))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("download Lark outbound attachment failed with status {status}");
    }

    let content_type = content_type_from_response(&response);
    let file_name = extract_response_file_name(&response);
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("read Lark outbound attachment body {target}"))?;

    let storage_root = workspace_dir
        .map(ToOwned::to_owned)
        .unwrap_or_else(std::env::temp_dir);
    let resource = LarkInboundResource {
        kind,
        file_key: format!("outbound_{}", current_timestamp_millis()),
        file_name,
    };
    let stored = store_inbound_resource(
        &storage_root,
        channel_name,
        account_id,
        "outbound",
        &resource,
        bytes.as_ref(),
        content_type.as_deref(),
        resource.file_name.as_deref(),
    )
    .await?;
    Ok(stored.path)
}

fn current_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
