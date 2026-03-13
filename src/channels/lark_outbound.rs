use super::lark::{
    classify_lark_outgoing_attachments, parse_lark_attachment_markers,
    parse_lark_path_only_attachment, LarkAttachment, LarkAttachmentKind,
};
use super::traits::SendMessage;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LarkOutboundRequest {
    pub(crate) target: String,
    pub(crate) text: String,
    pub(crate) local_images: Vec<PathBuf>,
    pub(crate) local_documents: Vec<PathBuf>,
    pub(crate) unresolved_markers: Vec<String>,
    pub(crate) path_only_attachment: Option<LarkAttachment>,
    pub(crate) thread_ts: Option<String>,
}

impl LarkOutboundRequest {
    pub(crate) fn from_send_message(message: &SendMessage, raw_content: &str) -> Self {
        let (cleaned_content, parsed_attachments) = parse_lark_attachment_markers(raw_content);
        let (local_images, local_documents, unresolved_markers) =
            classify_lark_outgoing_attachments(&parsed_attachments);

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
            local_images,
            local_documents,
            unresolved_markers,
            path_only_attachment: parse_lark_path_only_attachment(raw_content),
            thread_ts: message.thread_ts.clone(),
        }
    }

    pub(crate) fn has_local_attachments(&self) -> bool {
        !self.local_images.is_empty() || !self.local_documents.is_empty()
    }

    pub(crate) fn attachment_path(&self) -> Option<(&Path, LarkAttachmentKind)> {
        let attachment = self.path_only_attachment.as_ref()?;
        Some((Path::new(&attachment.target), attachment.kind))
    }
}
