use super::lark_inbound::{LarkInboundResource, LarkInboundResourceKind};
use anyhow::{Context, Result};
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use std::path::{Path, PathBuf};

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

pub(crate) fn content_type_from_response(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}
