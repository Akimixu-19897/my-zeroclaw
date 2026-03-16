use super::cards::{parse_lark_card_message, LarkCardMessage};
use super::{
    classify_lark_outgoing_attachments, parse_lark_attachment_markers,
    parse_lark_path_only_attachment, LarkAttachment, LarkAttachmentKind,
};
use crate::channels::traits::SendMessage;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LarkAttachmentSource {
    Local(PathBuf),
    RemoteUrl(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LarkOutboundRequest {
    pub(crate) target: String,
    pub(crate) text: String,
    pub(crate) card: Option<LarkCardMessage>,
    pub(crate) local_images: Vec<PathBuf>,
    pub(crate) local_documents: Vec<PathBuf>,
    pub(crate) remote_attachments: Vec<(LarkAttachmentKind, String)>,
    pub(crate) unresolved_markers: Vec<String>,
    pub(crate) path_only_attachment: Option<LarkAttachment>,
    pub(crate) thread_ts: Option<String>,
}

impl LarkOutboundRequest {
    pub(crate) fn from_send_message(message: &SendMessage, raw_content: &str) -> Self {
        if let Some(card) = parse_lark_card_message(raw_content) {
            return Self {
                target: message.recipient.clone(),
                text: String::new(),
                card: Some(card),
                local_images: Vec::new(),
                local_documents: Vec::new(),
                remote_attachments: Vec::new(),
                unresolved_markers: Vec::new(),
                path_only_attachment: None,
                thread_ts: message.thread_ts.clone(),
            };
        }

        let (cleaned_content, parsed_attachments) = parse_lark_attachment_markers(raw_content);
        let (local_images, local_documents, unresolved_markers) =
            classify_lark_outgoing_attachments(&parsed_attachments);
        let remote_attachments: Vec<(LarkAttachmentKind, String)> = parsed_attachments
            .iter()
            .filter_map(classify_remote_attachment)
            .collect();
        let unresolved_markers: Vec<String> = unresolved_markers
            .into_iter()
            .filter(|marker| {
                !remote_attachments
                    .iter()
                    .any(|(_, target)| marker.contains(target))
            })
            .collect();

        let mut text_segments = Vec::new();
        if !cleaned_content.is_empty() {
            text_segments.push(cleaned_content);
        }
        if !unresolved_markers.is_empty() {
            text_segments.extend(unresolved_markers.iter().cloned());
        }

        Self {
            target: message.recipient.clone(),
            text: text_segments.join("\n"),
            card: None,
            local_images,
            local_documents,
            remote_attachments,
            unresolved_markers,
            path_only_attachment: parse_lark_path_only_attachment(raw_content),
            thread_ts: message.thread_ts.clone(),
        }
    }

    pub(crate) fn has_local_attachments(&self) -> bool {
        !self.local_images.is_empty() || !self.local_documents.is_empty()
    }

    pub(crate) fn has_remote_attachments(&self) -> bool {
        !self.remote_attachments.is_empty()
    }

    pub(crate) fn attachment_path(&self) -> Option<(&Path, LarkAttachmentKind)> {
        let attachment = self.path_only_attachment.as_ref()?;
        Some((Path::new(&attachment.target), attachment.kind))
    }

    pub(crate) fn reply_message_id(&self) -> Option<&str> {
        self.thread_ts
            .as_deref()
            .filter(|value| !value.trim().is_empty())
    }
}

fn classify_remote_attachment(attachment: &LarkAttachment) -> Option<(LarkAttachmentKind, String)> {
    let target = attachment.target.trim();
    if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("file://")
    {
        return Some((attachment.kind, target.to_string()));
    }
    None
}
