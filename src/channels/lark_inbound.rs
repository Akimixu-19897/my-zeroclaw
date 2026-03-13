use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LarkInboundResourceKind {
    Image,
    File,
    Audio,
    Video,
}

impl LarkInboundResourceKind {
    pub(crate) fn marker_label(self) -> &'static str {
        match self {
            Self::Image => "IMAGE",
            Self::File => "DOCUMENT",
            Self::Audio => "AUDIO",
            Self::Video => "VIDEO",
        }
    }

    pub(crate) fn placeholder(self) -> &'static str {
        match self {
            Self::Image => "<media:image>",
            Self::File => "<media:document>",
            Self::Audio => "<media:audio>",
            Self::Video => "<media:video>",
        }
    }

    pub(crate) fn resource_type(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::File | Self::Audio | Self::Video => "file",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LarkInboundResource {
    pub(crate) kind: LarkInboundResourceKind,
    pub(crate) file_key: String,
    pub(crate) file_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LarkParsedMessage {
    pub(crate) message_id: String,
    pub(crate) chat_id: String,
    pub(crate) sender_open_id: String,
    pub(crate) chat_type: String,
    pub(crate) message_type: String,
    pub(crate) create_time_secs: Option<u64>,
    pub(crate) root_id: Option<String>,
    pub(crate) parent_id: Option<String>,
    pub(crate) thread_id: Option<String>,
    pub(crate) mentions: Vec<Value>,
    pub(crate) text: String,
    pub(crate) post_mentioned_open_ids: Vec<String>,
    pub(crate) resources: Vec<LarkInboundResource>,
}

pub(crate) fn parse_lark_text_content(content_str: &str) -> Option<String> {
    serde_json::from_str::<Value>(content_str)
        .ok()
        .and_then(|v| {
            v.get("text")
                .and_then(|t| t.as_str())
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
        })
}

pub(crate) fn parse_lark_file_name(content: &Value) -> Option<String> {
    for key in ["file_name", "name", "title"] {
        if let Some(value) = content.get(key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub(crate) fn parse_lark_inbound_resources(
    message_type: &str,
    content_str: &str,
) -> Vec<LarkInboundResource> {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };

    let mut resources = Vec::new();
    let mut push_resource = |kind: LarkInboundResourceKind, key: Option<&str>| {
        if let Some(file_key) = key.map(str::trim).filter(|key| !key.is_empty()) {
            resources.push(LarkInboundResource {
                kind,
                file_key: file_key.to_string(),
                file_name: parse_lark_file_name(&parsed),
            });
        }
    };

    match message_type {
        "image" => push_resource(
            LarkInboundResourceKind::Image,
            parsed.get("image_key").and_then(Value::as_str),
        ),
        "file" => push_resource(
            LarkInboundResourceKind::File,
            parsed.get("file_key").and_then(Value::as_str),
        ),
        "audio" => push_resource(
            LarkInboundResourceKind::Audio,
            parsed.get("file_key").and_then(Value::as_str),
        ),
        "media" | "video" => push_resource(
            LarkInboundResourceKind::Video,
            parsed.get("file_key").and_then(Value::as_str),
        ),
        _ => {}
    }

    resources
}

pub(crate) fn render_lark_fallback_content(
    text: &str,
    resources: &[LarkInboundResource],
    interactive_fallback: Option<&str>,
) -> String {
    let mut blocks = Vec::new();

    for resource in resources {
        blocks.push(resource.kind.placeholder().to_string());
    }

    if let Some(fallback) = interactive_fallback
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        blocks.push(fallback.to_string());
    }

    let text = text.trim();
    if !text.is_empty() {
        blocks.push(text.to_string());
    }

    blocks.join("\n\n")
}
