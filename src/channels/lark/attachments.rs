use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LarkAttachmentKind {
    Image,
    Document,
    Audio,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LarkAttachment {
    pub(crate) kind: LarkAttachmentKind,
    pub(crate) target: String,
}

impl LarkAttachmentKind {
    fn from_marker(marker: &str) -> Option<Self> {
        match marker.trim().to_ascii_uppercase().as_str() {
            "IMAGE" => Some(Self::Image),
            "DOCUMENT" | "FILE" => Some(Self::Document),
            "AUDIO" | "VOICE" => Some(Self::Audio),
            "VIDEO" => Some(Self::Video),
            _ => None,
        }
    }
}

fn infer_lark_attachment_kind_from_target(target: &str) -> Option<LarkAttachmentKind> {
    let normalized = target
        .split('?')
        .next()
        .unwrap_or(target)
        .split('#')
        .next()
        .unwrap_or(target);

    let extension = Path::new(normalized)
        .extension()
        .and_then(|ext| ext.to_str())?
        .to_ascii_lowercase();

    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Some(LarkAttachmentKind::Image),
        "mp3" | "wav" | "ogg" | "opus" | "m4a" => Some(LarkAttachmentKind::Audio),
        "mp4" | "mov" | "avi" | "mkv" | "webm" => Some(LarkAttachmentKind::Video),
        "pdf" | "txt" | "md" | "csv" | "json" | "zip" | "tar" | "gz" | "doc" | "docx" | "xls"
        | "xlsx" | "ppt" | "pptx" => Some(LarkAttachmentKind::Document),
        _ => None,
    }
}

pub(crate) fn parse_lark_path_only_attachment(message: &str) -> Option<LarkAttachment> {
    let trimmed = message.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }

    let candidate = trimmed.trim_matches(|c| matches!(c, '`' | '"' | '\''));
    if candidate.chars().any(char::is_whitespace) {
        return None;
    }

    let candidate = candidate.strip_prefix("file://").unwrap_or(candidate);
    let kind = infer_lark_attachment_kind_from_target(candidate)?;

    if !Path::new(candidate).exists() {
        return None;
    }

    Some(LarkAttachment {
        kind,
        target: candidate.to_string(),
    })
}

pub(crate) fn parse_lark_attachment_markers(message: &str) -> (String, Vec<LarkAttachment>) {
    let mut cleaned = String::with_capacity(message.len());
    let mut attachments = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel_start) = message[cursor..].find('[') {
        let start = cursor + rel_start;
        cleaned.push_str(&message[cursor..start]);

        let Some(rel_end) = message[start..].find(']') else {
            cleaned.push_str(&message[start..]);
            cursor = message.len();
            break;
        };
        let end = start + rel_end;
        let marker_text = &message[start + 1..end];

        let parsed = marker_text.split_once(':').and_then(|(kind, target)| {
            let kind = LarkAttachmentKind::from_marker(kind)?;
            let target = target.trim();
            if target.is_empty() {
                return None;
            }
            Some(LarkAttachment {
                kind,
                target: target.to_string(),
            })
        });

        if let Some(attachment) = parsed {
            attachments.push(attachment);
        } else {
            cleaned.push_str(&message[start..=end]);
        }

        cursor = end + 1;
    }

    if cursor < message.len() {
        cleaned.push_str(&message[cursor..]);
    }

    (cleaned.trim().to_string(), attachments)
}

pub(crate) fn classify_lark_outgoing_attachments(
    attachments: &[LarkAttachment],
) -> (Vec<LarkAttachment>, Vec<String>) {
    let mut local_attachments = Vec::new();
    let mut unresolved_markers = Vec::new();

    for attachment in attachments {
        let target = attachment.target.trim();
        let path = Path::new(target);
        if path.exists() && path.is_file() {
            local_attachments.push(LarkAttachment {
                kind: attachment.kind,
                target: target.to_string(),
            });
            continue;
        }

        unresolved_markers.push(match attachment.kind {
            LarkAttachmentKind::Image => format!("[IMAGE:{target}]"),
            LarkAttachmentKind::Document => format!("[DOCUMENT:{target}]"),
            LarkAttachmentKind::Audio => format!("[AUDIO:{target}]"),
            LarkAttachmentKind::Video => format!("[VIDEO:{target}]"),
        });
    }

    (local_attachments, unresolved_markers)
}
