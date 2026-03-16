use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LarkDispatchContext {
    pub(crate) sender_id: String,
    pub(crate) feishu_from: String,
    pub(crate) feishu_to: String,
    pub(crate) envelope_from: String,
    pub(crate) is_group: bool,
    pub(crate) is_thread: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LarkInboundResourceKind {
    Image,
    Sticker,
    File,
    Audio,
    Video,
}

impl LarkInboundResourceKind {
    pub(crate) fn marker_label(self) -> &'static str {
        match self {
            Self::Image => "IMAGE",
            Self::Sticker => "STICKER",
            Self::File => "DOCUMENT",
            Self::Audio => "AUDIO",
            Self::Video => "VIDEO",
        }
    }

    pub(crate) fn placeholder(self) -> &'static str {
        match self {
            Self::Image => "<media:image>",
            Self::Sticker => "<media:sticker>",
            Self::File => "<media:document>",
            Self::Audio => "<media:audio>",
            Self::Video => "<media:video>",
        }
    }

    pub(crate) fn resource_type(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Sticker => "file",
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
    pub(crate) raw_content: String,
    pub(crate) normalized_content: String,
    pub(crate) create_time_secs: Option<u64>,
    pub(crate) root_id: Option<String>,
    pub(crate) parent_id: Option<String>,
    pub(crate) thread_id: Option<String>,
    pub(crate) mentions: Vec<Value>,
    pub(crate) text: String,
    pub(crate) post_mentioned_open_ids: Vec<String>,
    pub(crate) resources: Vec<LarkInboundResource>,
}

pub(crate) fn build_lark_dispatch_context(parsed: &LarkParsedMessage) -> LarkDispatchContext {
    let is_group = parsed.chat_type == "group";
    let is_thread = is_group && parsed.thread_id.is_some();
    let sender_id = parsed.sender_open_id.clone();
    let feishu_from = format!("feishu:{sender_id}");
    let feishu_to = if is_group {
        format!("chat:{}", parsed.chat_id)
    } else {
        format!("user:{sender_id}")
    };
    let envelope_from = if is_group {
        format!("{}:{sender_id}", parsed.chat_id)
    } else {
        sender_id.clone()
    };

    LarkDispatchContext {
        sender_id,
        feishu_from,
        feishu_to,
        envelope_from,
        is_group,
        is_thread,
    }
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

pub(crate) fn build_lark_normalized_content(
    message_type: &str,
    content_str: &str,
    text_content: Option<&str>,
    resources: &[LarkInboundResource],
) -> String {
    match message_type {
        "text" => text_content.unwrap_or_default().trim().to_string(),
        "post" => text_content.unwrap_or_default().trim().to_string(),
        "location" => parse_lark_location_content(content_str),
        "system" => parse_lark_system_content(content_str),
        "hongbao" => parse_lark_hongbao_content(content_str),
        "sticker" => parse_lark_sticker_content(content_str),
        "folder" => parse_lark_folder_content(content_str),
        "share_chat" => parse_lark_share_chat_content(content_str),
        "share_user" => parse_lark_share_user_content(content_str),
        "todo" => parse_lark_todo_content(content_str),
        "vote" => parse_lark_vote_content(content_str),
        "share_calendar_event" => {
            parse_lark_calendar_content(content_str, "calendar_share", "[calendar event]")
        }
        "calendar" => {
            parse_lark_calendar_content(content_str, "calendar_invite", "[calendar event]")
        }
        "general_calendar" => {
            parse_lark_calendar_content(content_str, "calendar", "[calendar event]")
        }
        "video_chat" => parse_lark_video_chat_content(content_str),
        "image" => resources
            .first()
            .map(|resource| format!("![image]({})", resource.file_key))
            .unwrap_or_else(|| "[image]".to_string()),
        "file" => resources
            .first()
            .map(|resource| {
                let name_attr = resource
                    .file_name
                    .as_deref()
                    .filter(|name| !name.is_empty())
                    .map(|name| format!(" name=\"{name}\""))
                    .unwrap_or_default();
                format!("<file key=\"{}\"{name_attr}/>", resource.file_key)
            })
            .unwrap_or_else(|| "[file]".to_string()),
        "audio" => resources
            .first()
            .map(|resource| format!("<audio key=\"{}\"/>", resource.file_key))
            .unwrap_or_else(|| "[audio]".to_string()),
        "media" | "video" => resources
            .first()
            .map(|resource| {
                let name_attr = resource
                    .file_name
                    .as_deref()
                    .filter(|name| !name.is_empty())
                    .map(|name| format!(" name=\"{name}\""))
                    .unwrap_or_default();
                format!("<video key=\"{}\"{name_attr}/>", resource.file_key)
            })
            .unwrap_or_else(|| "[video]".to_string()),
        "interactive" => parse_lark_interactive_content(content_str),
        _ => parse_lark_unknown_content(content_str, text_content),
    }
}

fn parse_lark_sticker_content(content_str: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "[sticker]".to_string(),
    };
    let file_key = parsed
        .get("file_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match file_key {
        Some(file_key) => format!("<sticker key=\"{file_key}\"/>"),
        None => "[sticker]".to_string(),
    }
}

fn parse_lark_hongbao_content(content_str: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "<hongbao/>".to_string(),
    };
    let text_attr = parsed
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!(" text=\"{value}\""))
        .unwrap_or_default();
    format!("<hongbao{text_attr}/>")
}

fn parse_lark_folder_content(content_str: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "[folder]".to_string(),
    };
    let file_key = parsed
        .get("file_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(file_key) = file_key else {
        return "[folder]".to_string();
    };
    let name_attr = parse_lark_file_name(&parsed)
        .map(|file_name| format!(" name=\"{file_name}\""))
        .unwrap_or_default();
    format!("<folder key=\"{file_key}\"{name_attr}/>")
}

fn parse_lark_location_content(content_str: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "<location/>".to_string(),
    };
    let name_attr = parsed
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!(" name=\"{}\"", value.trim()))
        .unwrap_or_default();
    let latitude = parsed.get("latitude").and_then(value_to_trimmed_string);
    let longitude = parsed.get("longitude").and_then(value_to_trimmed_string);
    let coords_attr = match (latitude, longitude) {
        (Some(lat), Some(lng)) => format!(" coords=\"lat:{lat},lng:{lng}\""),
        _ => String::new(),
    };
    format!("<location{name_attr}{coords_attr}/>")
}

fn parse_lark_system_content(content_str: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "[system message]".to_string(),
    };
    let Some(template) = parsed.get("template").and_then(Value::as_str) else {
        return "[system message]".to_string();
    };
    let replacements = [
        (
            "{from_user}",
            join_string_array(parsed.get("from_user")).unwrap_or_default(),
        ),
        (
            "{to_chatters}",
            join_string_array(parsed.get("to_chatters")).unwrap_or_default(),
        ),
        (
            "{divider_text}",
            parsed
                .pointer("/divider_text/text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
    ];
    let mut content = template.to_string();
    for (placeholder, value) in replacements {
        content = content.replace(placeholder, value.trim());
    }
    let trimmed = content.trim();
    if trimmed.is_empty() {
        "[system message]".to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_lark_share_chat_content(content_str: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "<group_card id=\"\"/>".to_string(),
    };
    let chat_id = parsed
        .get("chat_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("<group_card id=\"{chat_id}\"/>")
}

fn parse_lark_share_user_content(content_str: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "<contact_card id=\"\"/>".to_string(),
    };
    let user_id = parsed
        .get("user_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("<contact_card id=\"{user_id}\"/>")
}

fn parse_lark_todo_content(content_str: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "<todo>\n[todo]\n</todo>".to_string(),
    };
    let mut parts = Vec::new();
    let title = parsed
        .pointer("/summary/title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let body: String = parsed
        .pointer("/summary/content")
        .and_then(Value::as_array)
        .map(|content| extract_lark_plain_text_from_post_blocks(content))
        .unwrap_or_default();
    let full_title = [title, body.trim()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !full_title.is_empty() {
        parts.push(full_title);
    }
    if let Some(due) = parsed.get("due_time").and_then(value_to_lark_datetime) {
        parts.push(format!("Due: {due}"));
    }
    let inner = if parts.is_empty() {
        "[todo]".to_string()
    } else {
        parts.join("\n")
    };
    format!("<todo>\n{inner}\n</todo>")
}

fn parse_lark_vote_content(content_str: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "<vote>\n[vote]\n</vote>".to_string(),
    };
    let mut parts = Vec::new();
    if let Some(topic) = parsed.get("topic").and_then(Value::as_str) {
        let trimmed = topic.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_string());
        }
    }
    if let Some(options) = parsed.get("options").and_then(Value::as_array) {
        for option in options {
            if let Some(option_text) = option
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                parts.push(format!("• {option_text}"));
            }
        }
    }
    let inner = if parts.is_empty() {
        "[vote]".to_string()
    } else {
        parts.join("\n")
    };
    format!("<vote>\n{inner}\n</vote>")
}

fn parse_lark_calendar_content(content_str: &str, tag: &str, fallback: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return format!("<{tag}>{fallback}</{tag}>"),
    };
    let mut parts = Vec::new();
    if let Some(summary) = parsed.get("summary").and_then(Value::as_str) {
        let trimmed = summary.trim();
        if !trimmed.is_empty() {
            parts.push(format!("📅 {trimmed}"));
        }
    }
    let start = parsed.get("start_time").and_then(value_to_lark_datetime);
    let end = parsed.get("end_time").and_then(value_to_lark_datetime);
    match (start, end) {
        (Some(start), Some(end)) => parts.push(format!("🕙 {start} ~ {end}")),
        (Some(start), None) => parts.push(format!("🕙 {start}")),
        _ => {}
    }
    let inner = if parts.is_empty() {
        fallback.to_string()
    } else {
        parts.join("\n")
    };
    format!("<{tag}>{inner}</{tag}>")
}

fn parse_lark_video_chat_content(content_str: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "<meeting>\n[video chat]\n</meeting>".to_string(),
    };
    let mut parts = Vec::new();
    if let Some(topic) = parsed.get("topic").and_then(Value::as_str) {
        let trimmed = topic.trim();
        if !trimmed.is_empty() {
            parts.push(format!("📹 {trimmed}"));
        }
    }
    if let Some(start_time) = parsed.get("start_time").and_then(value_to_lark_datetime) {
        parts.push(format!("🕙 {start_time}"));
    }
    let inner = if parts.is_empty() {
        "[video chat]".to_string()
    } else {
        parts.join("\n")
    };
    format!("<meeting>{inner}</meeting>")
}

fn parse_lark_unknown_content(content_str: &str, text_content: Option<&str>) -> String {
    if let Some(text) = text_content
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return text.to_string();
    }
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "[unsupported message]".to_string(),
    };
    if let Some(text) = parsed.get("text").and_then(Value::as_str).map(str::trim) {
        if !text.is_empty() {
            return text.to_string();
        }
    }
    "[unsupported message]".to_string()
}

fn extract_lark_plain_text_from_post_blocks(content: &[Value]) -> String {
    let mut lines = Vec::new();
    for paragraph in content {
        let Some(elements) = paragraph.as_array() else {
            continue;
        };
        let mut line = String::new();
        for element in elements {
            if let Some(text) = element.get("text").and_then(Value::as_str) {
                line.push_str(text);
            }
        }
        if !line.trim().is_empty() {
            lines.push(line.trim().to_string());
        }
    }
    lines.join("\n")
}

fn join_string_array(value: Option<&Value>) -> Option<String> {
    let array = value?.as_array()?;
    let joined = array
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

fn value_to_trimmed_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn value_to_lark_datetime(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => format_lark_millis_datetime(number.as_i64()?),
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            if let Ok(ms) = trimmed.parse::<i64>() {
                return format_lark_millis_datetime(ms);
            }
            Some(trimmed.to_string())
        }
        _ => None,
    }
}

fn format_lark_millis_datetime(ms: i64) -> Option<String> {
    let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)?;
    Some(
        timestamp
            .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600)?)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
    )
}

fn parse_lark_interactive_content(content_str: &str) -> String {
    let parsed = match serde_json::from_str::<Value>(content_str) {
        Ok(value) => value,
        Err(_) => return "[interactive card]".to_string(),
    };
    let attachment = parsed
        .get("json_attachment")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());

    if let Some(json_card) = parsed.get("json_card").and_then(Value::as_str) {
        return match serde_json::from_str::<Value>(json_card) {
            Ok(card) => render_lark_card_content(&card, attachment.as_ref()),
            Err(_) => "<card>\n[无法解析卡片内容]\n</card>".to_string(),
        };
    }

    if parsed.get("json_card").is_some() {
        return "<card>\n[无法解析卡片内容]\n</card>".to_string();
    }

    render_lark_legacy_card_content(&parsed)
}

fn render_lark_card_content(card: &Value, attachment: Option<&Value>) -> String {
    let title = card
        .pointer("/header/title/content")
        .and_then(Value::as_str)
        .or_else(|| {
            card.pointer("/header/property/title/content")
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let body_elements = card
        .pointer("/body/property/elements")
        .and_then(Value::as_array)
        .or_else(|| card.pointer("/body/elements").and_then(Value::as_array))
        .or_else(|| card.get("elements").and_then(Value::as_array));
    let body = body_elements
        .map(|elements| render_lark_card_elements(elements, attachment, 0))
        .unwrap_or_default();
    let body = body
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty() && Some(*line) != title.as_deref())
        .collect::<Vec<_>>()
        .join("\n");

    if body.is_empty() && title.is_none() {
        "[interactive card]".to_string()
    } else if let Some(title) = title {
        if body.is_empty() {
            format!("<card title=\"{title}\">\n</card>")
        } else {
            format!("<card title=\"{title}\">\n{body}\n</card>")
        }
    } else {
        format!("<card>\n{body}\n</card>")
    }
}

fn render_lark_legacy_card_content(card: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(title) = card
        .pointer("/header/title/content")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("**{}**", title.trim()));
    }
    if let Some(elements) = card
        .get("elements")
        .and_then(Value::as_array)
        .or_else(|| card.pointer("/body/elements").and_then(Value::as_array))
    {
        lines.extend(
            render_lark_card_elements(elements, None, 0)
                .lines()
                .map(ToOwned::to_owned),
        );
    }
    if lines.is_empty() {
        "[interactive card]".to_string()
    } else {
        lines.join("\n")
    }
}

fn render_lark_card_elements(
    elements: &[Value],
    attachment: Option<&Value>,
    depth: usize,
) -> String {
    elements
        .iter()
        .filter_map(|element| render_lark_card_element(element, attachment, depth))
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_lark_card_element(
    element: &Value,
    attachment: Option<&Value>,
    depth: usize,
) -> Option<String> {
    let object = element.as_object()?;
    let tag = object
        .get("tag")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let prop = card_extract_property(element);
    let rendered = match tag {
        "plain_text" | "text" => card_convert_plain_text(prop),
        "markdown" | "lark_md" => card_convert_markdown(prop, attachment),
        "markdown_v1" => card_convert_markdown_v1(element, prop, attachment),
        "div" => card_convert_div(prop, attachment),
        "note" => card_convert_note(prop, attachment),
        "hr" => "---".to_string(),
        "br" => "\n".to_string(),
        "column_set" => card_convert_column_set(prop, attachment, depth),
        "column" => card_convert_column(prop, attachment, depth),
        "person" => card_convert_person(prop, attachment),
        "person_v1" => card_convert_person(prop, attachment),
        "person_list" => card_convert_person_list(prop),
        "avatar" => "👤".to_string(),
        "at" => card_convert_at(prop, attachment),
        "at_all" => "@所有人".to_string(),
        "button" => card_convert_button(prop, attachment),
        "actions" | "action" => card_convert_actions(prop, attachment),
        "overflow" => card_convert_overflow(prop),
        "select_static" => card_convert_select(prop, false),
        "multi_select_static" | "multi_select_person" => card_convert_select(prop, true),
        "select_person" => card_convert_select(prop, false),
        "select_img" => card_convert_select_img(prop),
        "input" => card_convert_input(prop),
        "date_picker" => card_convert_date_picker(prop, "date"),
        "picker_time" => card_convert_date_picker(prop, "time"),
        "picker_datetime" => card_convert_date_picker(prop, "datetime"),
        "checker" => card_convert_checker(prop),
        "img" | "image" => card_convert_image(prop, attachment),
        "img_combination" => card_convert_img_combination(prop),
        "table" => card_convert_table(prop),
        "chart" => card_convert_chart(prop),
        "audio" => "🎵 音频".to_string(),
        "video" => "🎬 视频".to_string(),
        "collapsible_panel" => card_convert_collapsible_panel(prop, attachment),
        "form" => card_convert_form(prop, attachment),
        "interactive_container" => card_convert_interactive_container(prop, attachment),
        "text_tag" => card_convert_text_tag(prop),
        "number_tag" => card_convert_number_tag(prop),
        "link" => card_convert_link(prop),
        "emoji" => card_convert_emoji(prop),
        "local_datetime" => card_convert_local_datetime(prop),
        "list" => card_convert_list(prop, attachment),
        "blockquote" => card_convert_blockquote(prop, attachment),
        "code_block" => card_convert_code_block(prop),
        "code_span" => format!("`{}`", card_extract_string(prop, "content")),
        "heading" => card_convert_heading(prop, attachment),
        "fallback_text" => card_convert_fallback_text(prop, attachment),
        "repeat" => prop
            .get("elements")
            .and_then(Value::as_array)
            .map(|elements| render_lark_card_elements(elements, attachment, depth))
            .unwrap_or_default(),
        "card_header" | "custom_icon" | "standard_icon" => String::new(),
        _ => card_convert_unknown(prop, tag, attachment, depth),
    };
    let trimmed = rendered.trim_matches('\n').to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn card_extract_property<'a>(element: &'a Value) -> &'a Value {
    element.get("property").unwrap_or(element)
}

fn card_extract_text_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Object(map) => {
            if let Some(property) = map.get("property") {
                return card_extract_text_content(property);
            }
            if let Some(i18n) = map.get("i18nContent").and_then(Value::as_object) {
                for locale in ["zh_cn", "en_us", "ja_jp"] {
                    if let Some(text) = i18n.get(locale).and_then(Value::as_str) {
                        if !text.is_empty() {
                            return text.to_string();
                        }
                    }
                }
            }
            if let Some(content) = map.get("content").and_then(Value::as_str) {
                return content.to_string();
            }
            if let Some(elements) = map.get("elements").and_then(Value::as_array) {
                return elements
                    .iter()
                    .map(card_extract_text_content)
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join("");
            }
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                return text.to_string();
            }
            String::new()
        }
        _ => String::new(),
    }
}

fn card_extract_string(prop: &Value, key: &str) -> String {
    prop.get(key)
        .map(card_extract_text_content)
        .unwrap_or_default()
}

fn card_convert_plain_text(prop: &Value) -> String {
    let mut content = card_extract_string(prop, "content");
    if content.is_empty() {
        return String::new();
    }
    let attrs = prop
        .pointer("/textStyle/attributes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if attrs
        .iter()
        .any(|attr| attr.as_str() == Some("strikethrough"))
    {
        content = format!("~~{content}~~");
    }
    if attrs.iter().any(|attr| attr.as_str() == Some("italic")) {
        content = format!("*{content}*");
    }
    if attrs.iter().any(|attr| attr.as_str() == Some("bold")) {
        content = format!("**{content}**");
    }
    content
}

fn card_convert_markdown(prop: &Value, attachment: Option<&Value>) -> String {
    if let Some(elements) = prop.get("elements").and_then(Value::as_array) {
        return elements
            .iter()
            .filter_map(|element| render_lark_card_element(element, attachment, 0))
            .collect::<Vec<_>>()
            .join("");
    }
    card_extract_string(prop, "content")
}

fn card_convert_markdown_v1(element: &Value, prop: &Value, attachment: Option<&Value>) -> String {
    if let Some(elements) = prop.get("elements").and_then(Value::as_array) {
        return elements
            .iter()
            .filter_map(|item| render_lark_card_element(item, attachment, 0))
            .collect::<Vec<_>>()
            .join("");
    }
    if let Some(fallback) = element.get("fallback") {
        if let Some(rendered) = render_lark_card_element(fallback, attachment, 0) {
            return rendered;
        }
    }
    card_extract_string(prop, "content")
}

fn card_convert_div(prop: &Value, attachment: Option<&Value>) -> String {
    let mut results = Vec::new();
    if let Some(text) = prop.get("text").filter(|value| value.is_object()) {
        let rendered = render_lark_card_element(text, attachment, 0).unwrap_or_default();
        if !rendered.is_empty() {
            results.push(rendered);
        }
    }
    if let Some(fields) = prop.get("fields").and_then(Value::as_array) {
        let field_texts = fields
            .iter()
            .filter_map(|field| field.get("text"))
            .filter_map(|text| render_lark_card_element(text, attachment, 0))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>();
        if !field_texts.is_empty() {
            results.push(field_texts.join("\n"));
        }
    }
    if let Some(extra) = prop.get("extra") {
        if let Some(rendered) = render_lark_card_element(extra, attachment, 0) {
            if !rendered.is_empty() {
                results.push(rendered);
            }
        }
    }
    results.join("\n")
}

fn card_convert_note(prop: &Value, attachment: Option<&Value>) -> String {
    let Some(elements) = prop.get("elements").and_then(Value::as_array) else {
        return String::new();
    };
    let texts = elements
        .iter()
        .filter_map(|element| render_lark_card_element(element, attachment, 0))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    if texts.is_empty() {
        String::new()
    } else {
        format!("📝 {}", texts.join(" "))
    }
}

fn card_convert_link(prop: &Value) -> String {
    let content = card_extract_string(prop, "content");
    let content = if content.is_empty() {
        "链接".to_string()
    } else {
        content
    };
    let url = prop
        .pointer("/url/url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if url.is_empty() {
        content
    } else {
        format!("[{content}]({url})")
    }
}

fn card_convert_emoji(prop: &Value) -> String {
    match prop.get("key").and_then(Value::as_str).unwrap_or_default() {
        "OK" => "👌",
        "THUMBSUP" => "👍",
        "SMILE" => "😊",
        "HEART" => "❤️",
        "CLAP" => "👏",
        "FIRE" => "🔥",
        "PARTY" => "🎉",
        "THINK" => "🤔",
        other => return format!(":{other}:"),
    }
    .to_string()
}

fn card_convert_local_datetime(prop: &Value) -> String {
    if let Some(milliseconds) = prop.get("milliseconds") {
        if let Some(rendered) = card_format_milliseconds_iso8601(milliseconds) {
            return rendered;
        }
    }
    card_extract_string(prop, "fallbackText")
}

fn card_convert_list(prop: &Value, attachment: Option<&Value>) -> String {
    let Some(items) = prop.get("items").and_then(Value::as_array) else {
        return String::new();
    };
    let mut lines = Vec::new();
    for item in items {
        let level = item.get("level").and_then(Value::as_u64).unwrap_or(0) as usize;
        let indent = "  ".repeat(level);
        let marker = match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "ol" => format!(
                "{}.",
                item.get("order").and_then(Value::as_u64).unwrap_or(0)
            ),
            _ => "-".to_string(),
        };
        let content = item
            .get("elements")
            .and_then(Value::as_array)
            .map(|elements| {
                elements
                    .iter()
                    .filter_map(|element| render_lark_card_element(element, attachment, 0))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        if !content.is_empty() {
            lines.push(format!("{indent}{marker} {content}"));
        }
    }
    lines.join("\n")
}

fn card_convert_blockquote(prop: &Value, attachment: Option<&Value>) -> String {
    let content = if let Some(content) = prop.get("content").and_then(Value::as_str) {
        content.to_string()
    } else {
        prop.get("elements")
            .and_then(Value::as_array)
            .map(|elements| {
                elements
                    .iter()
                    .filter_map(|element| render_lark_card_element(element, attachment, 0))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default()
    };
    if content.is_empty() {
        String::new()
    } else {
        content
            .lines()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn card_convert_code_block(prop: &Value) -> String {
    let language = prop
        .get("language")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("plaintext");
    let mut code = String::new();
    if let Some(lines) = prop.get("contents").and_then(Value::as_array) {
        for line in lines {
            if let Some(contents) = line.get("contents").and_then(Value::as_array) {
                for content in contents {
                    if let Some(text) = content.get("content").and_then(Value::as_str) {
                        code.push_str(text);
                    }
                }
            }
        }
    }
    format!("```{language}\n{code}\n```")
}

fn card_convert_heading(prop: &Value, attachment: Option<&Value>) -> String {
    let level = prop
        .get("level")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 6) as usize;
    let content = if let Some(content) = prop.get("content").and_then(Value::as_str) {
        content.to_string()
    } else {
        prop.get("elements")
            .and_then(Value::as_array)
            .map(|elements| {
                elements
                    .iter()
                    .filter_map(|element| render_lark_card_element(element, attachment, 0))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default()
    };
    format!("{} {}", "#".repeat(level), content)
}

fn card_convert_text_tag(prop: &Value) -> String {
    let text = prop
        .get("text")
        .map(card_extract_text_content)
        .unwrap_or_default();
    if text.is_empty() {
        String::new()
    } else {
        format!("「{text}」")
    }
}

fn card_convert_number_tag(prop: &Value) -> String {
    let text = prop
        .get("text")
        .map(card_extract_text_content)
        .unwrap_or_default();
    let url = prop
        .pointer("/url/url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if text.is_empty() {
        String::new()
    } else if url.is_empty() {
        text
    } else {
        format!("[{text}]({url})")
    }
}

fn card_convert_unknown(
    prop: &Value,
    _tag: &str,
    attachment: Option<&Value>,
    depth: usize,
) -> String {
    for key in ["content", "text", "title", "label", "placeholder"] {
        if let Some(value) = prop.get(key) {
            let text = card_extract_text_content(value);
            if !text.is_empty() {
                return text;
            }
        }
    }
    if let Some(elements) = prop.get("elements").and_then(Value::as_array) {
        return render_lark_card_elements(elements, attachment, depth);
    }
    "[未知内容]".to_string()
}

fn card_convert_column_set(prop: &Value, attachment: Option<&Value>, depth: usize) -> String {
    prop.get("columns")
        .and_then(Value::as_array)
        .map(|columns| {
            columns
                .iter()
                .filter_map(|column| render_lark_card_element(column, attachment, depth + 1))
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
}

fn card_convert_column(prop: &Value, attachment: Option<&Value>, depth: usize) -> String {
    prop.get("elements")
        .and_then(Value::as_array)
        .map(|elements| render_lark_card_elements(elements, attachment, depth))
        .unwrap_or_default()
}

fn card_convert_form(prop: &Value, attachment: Option<&Value>) -> String {
    let body = prop
        .get("elements")
        .and_then(Value::as_array)
        .map(|elements| render_lark_card_elements(elements, attachment, 0))
        .unwrap_or_default();
    format!("<form>\n{body}\n</form>")
}

fn card_convert_collapsible_panel(prop: &Value, attachment: Option<&Value>) -> String {
    let title = prop
        .pointer("/header/title")
        .map(card_extract_text_content)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "详情".to_string());
    let expanded = prop
        .get("expanded")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !expanded {
        return format!("▶ {title}");
    }
    let content = prop
        .get("elements")
        .and_then(Value::as_array)
        .map(|elements| render_lark_card_elements(elements, attachment, 1))
        .unwrap_or_default();
    let indented = content
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("▼ {title}\n{indented}\n▲")
}

fn card_convert_interactive_container(prop: &Value, attachment: Option<&Value>) -> String {
    let url = prop
        .get("actions")
        .and_then(Value::as_array)
        .and_then(|actions| actions.first())
        .and_then(|action| {
            action
                .get("type")
                .and_then(Value::as_str)
                .filter(|t| *t == "open_url")
                .map(|_| action)
        })
        .and_then(|action| action.pointer("/action/url"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let body = prop
        .get("elements")
        .and_then(Value::as_array)
        .map(|elements| render_lark_card_elements(elements, attachment, 0))
        .unwrap_or_default();
    if url.is_empty() {
        format!("<clickable>\n{body}\n</clickable>")
    } else {
        format!(
            "<clickable url=\"{}\">\n{body}\n</clickable>",
            card_escape_attr(url)
        )
    }
}

fn card_convert_button(prop: &Value, _attachment: Option<&Value>) -> String {
    let text = prop
        .get("text")
        .map(card_extract_text_content)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "按钮".to_string());
    let disabled = prop
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Some(url) = prop
        .get("actions")
        .and_then(Value::as_array)
        .and_then(|actions| {
            actions.iter().find_map(|action| {
                (action.get("type").and_then(Value::as_str) == Some("open_url"))
                    .then(|| action.pointer("/action/url").and_then(Value::as_str))
                    .flatten()
            })
        })
    {
        return format!("[{text}]({url})");
    }
    if disabled {
        format!("[{text} ✗]")
    } else {
        format!("[{text}]")
    }
}

fn card_convert_actions(prop: &Value, attachment: Option<&Value>) -> String {
    prop.get("actions")
        .and_then(Value::as_array)
        .map(|actions| {
            actions
                .iter()
                .filter_map(|action| render_lark_card_element(action, attachment, 0))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn card_convert_select(prop: &Value, is_multi: bool) -> String {
    let options = prop
        .get("options")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut selected = std::collections::HashSet::new();
    if is_multi {
        if let Some(values) = prop.get("selectedValues").and_then(Value::as_array) {
            for value in values {
                if let Some(value) = value.as_str() {
                    selected.insert(value.to_string());
                }
            }
        }
    } else if let Some(value) = prop.get("initialOption").and_then(Value::as_str) {
        selected.insert(value.to_string());
    }

    let mut option_texts = Vec::new();
    let mut has_selected = false;
    for option in options {
        let value = option
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut text = option
            .get("text")
            .map(card_extract_text_content)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| value.clone());
        if text.is_empty() {
            continue;
        }
        if selected.contains(&value) {
            text = format!("✓{text}");
            has_selected = true;
        }
        option_texts.push(text);
    }
    if option_texts.is_empty() {
        let placeholder = prop
            .get("placeholder")
            .map(card_extract_text_content)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "请选择".to_string());
        option_texts.push(format!("{placeholder} ▼"));
    } else if !has_selected {
        if let Some(last) = option_texts.last_mut() {
            last.push_str(" ▼");
        }
    }
    format!("{{{}}}", option_texts.join(" / "))
}

fn card_convert_select_img(prop: &Value) -> String {
    let Some(options) = prop.get("options").and_then(Value::as_array) else {
        return String::new();
    };
    let selected = prop
        .get("selectedValues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(ToOwned::to_owned))
        .collect::<std::collections::HashSet<_>>();
    let mut texts = Vec::new();
    for (idx, option) in options.iter().enumerate() {
        let value = option
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let prefix = if selected.contains(value) { "✓" } else { "" };
        texts.push(format!("{prefix}🖼️图{}", idx + 1));
    }
    format!("{{{}}}", texts.join(" / "))
}

fn card_convert_input(prop: &Value) -> String {
    let label = prop
        .get("label")
        .map(card_extract_text_content)
        .unwrap_or_default();
    let default_value = prop
        .get("defaultValue")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let placeholder = prop
        .get("placeholder")
        .map(card_extract_text_content)
        .unwrap_or_default();
    let mut result = if !default_value.is_empty() {
        format!("{default_value}___")
    } else if !placeholder.is_empty() {
        format!("{placeholder}_____")
    } else {
        "_____".to_string()
    };
    if prop.get("inputType").and_then(Value::as_str) == Some("multiline_text") {
        result = result.replace("_____", "...");
    }
    if label.is_empty() {
        result
    } else {
        format!("{label}: {result}")
    }
}

fn card_convert_date_picker(prop: &Value, picker_type: &str) -> String {
    let (emoji, value) = match picker_type {
        "time" => ("🕐", prop.get("initialTime")),
        "datetime" => ("📅", prop.get("initialDatetime")),
        _ => ("📅", prop.get("initialDate")),
    };
    let mut rendered = value
        .and_then(card_normalize_time_format)
        .unwrap_or_default();
    if rendered.is_empty() {
        rendered = prop
            .get("placeholder")
            .map(card_extract_text_content)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "选择".to_string());
    }
    format!("{emoji} {rendered}")
}

fn card_convert_checker(prop: &Value) -> String {
    let checked = prop
        .get("checked")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let text = prop
        .get("text")
        .map(card_extract_text_content)
        .unwrap_or_default();
    format!("{} {text}", if checked { "[x]" } else { "[ ]" })
}

fn card_convert_overflow(prop: &Value) -> String {
    let Some(options) = prop.get("options").and_then(Value::as_array) else {
        return String::new();
    };
    let texts = options
        .iter()
        .map(|option| {
            option
                .get("text")
                .map(card_extract_text_content)
                .unwrap_or_default()
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    format!("⋮ {}", texts.join(", "))
}

fn card_convert_person(prop: &Value, attachment: Option<&Value>) -> String {
    let user_id = prop
        .get("userID")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if user_id.is_empty() {
        return String::new();
    }
    if let Some(person_name) = attachment
        .and_then(|attachment| attachment.pointer(&format!("/persons/{user_id}/content")))
        .and_then(Value::as_str)
    {
        return format!("@{person_name}");
    }
    let notation = prop
        .get("notation")
        .map(card_extract_text_content)
        .filter(|value| !value.is_empty());
    notation
        .map(|name| format!("@{name}"))
        .unwrap_or_else(|| format!("@{user_id}"))
}

fn card_convert_person_list(prop: &Value) -> String {
    let Some(persons) = prop.get("persons").and_then(Value::as_array) else {
        return String::new();
    };
    std::iter::repeat("@用户")
        .take(persons.len())
        .collect::<Vec<_>>()
        .join(", ")
}

fn card_convert_at(prop: &Value, attachment: Option<&Value>) -> String {
    let user_id = prop
        .get("userID")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if user_id.is_empty() {
        return String::new();
    }
    if let Some(name) = attachment
        .and_then(|attachment| attachment.pointer(&format!("/at_users/{user_id}/content")))
        .and_then(Value::as_str)
    {
        return format!("@{name}");
    }
    format!("@{user_id}")
}

fn card_convert_image(prop: &Value, attachment: Option<&Value>) -> String {
    let mut alt = prop
        .get("alt")
        .map(card_extract_text_content)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "图片".to_string());
    if let Some(title) = prop
        .get("title")
        .map(card_extract_text_content)
        .filter(|value| !value.is_empty())
    {
        alt = title;
    }
    let result = format!("🖼️ {alt}");
    if let Some(image_id) = prop.get("imageID").and_then(Value::as_str) {
        if let Some(token) = attachment
            .and_then(|attachment| attachment.pointer(&format!("/images/{image_id}/token")))
            .and_then(Value::as_str)
        {
            let _ = token;
        }
    }
    result
}

fn card_convert_img_combination(prop: &Value) -> String {
    let count = prop
        .get("imgList")
        .and_then(Value::as_array)
        .map(|items| items.len())
        .unwrap_or(0);
    if count == 0 {
        String::new()
    } else {
        format!("🖼️ {count}张图片")
    }
}

fn card_convert_chart(prop: &Value) -> String {
    let chart_spec = prop.get("chartSpec").and_then(Value::as_object);
    let mut title = chart_spec
        .and_then(|spec| spec.get("title"))
        .and_then(Value::as_object)
        .and_then(|title| title.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("图表")
        .to_string();
    if let Some(chart_type) = chart_spec
        .and_then(|spec| spec.get("type"))
        .and_then(Value::as_str)
    {
        let type_name = match chart_type {
            "bar" => "柱状图",
            "line" => "折线图",
            "pie" => "饼图",
            "area" => "面积图",
            "radar" => "雷达图",
            "scatter" => "散点图",
            _ => "",
        };
        title.push_str(type_name);
    }
    format!("📊 {title}")
}

fn card_convert_table(prop: &Value) -> String {
    let Some(columns) = prop.get("columns").and_then(Value::as_array) else {
        return String::new();
    };
    let col_names = columns
        .iter()
        .map(|column| {
            column
                .get("displayName")
                .and_then(Value::as_str)
                .or_else(|| column.get("name").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string()
        })
        .collect::<Vec<_>>();
    let col_keys = columns
        .iter()
        .map(|column| {
            column
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect::<Vec<_>>();
    let mut lines = vec![
        format!("| {} |", col_names.join(" | ")),
        format!(
            "|{}",
            col_names.iter().map(|_| "------|").collect::<String>()
        ),
    ];
    if let Some(rows) = prop.get("rows").and_then(Value::as_array) {
        for row in rows {
            let cells = col_keys
                .iter()
                .map(|key| {
                    row.get(key)
                        .and_then(|cell| cell.get("data"))
                        .map(card_extract_table_cell_value)
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>();
            lines.push(format!("| {} |", cells.join(" | ")));
        }
    }
    lines.join("\n")
}

fn card_extract_table_cell_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .map(|text| format!("「{text}」"))
            .collect::<Vec<_>>()
            .join(" "),
        Value::Object(_) => card_extract_text_content(value),
        _ => String::new(),
    }
}

fn card_convert_fallback_text(prop: &Value, attachment: Option<&Value>) -> String {
    if let Some(text) = prop.get("text") {
        let rendered = card_extract_text_content(text);
        if !rendered.is_empty() {
            return rendered;
        }
    }
    prop.get("elements")
        .and_then(Value::as_array)
        .map(|elements| {
            elements
                .iter()
                .filter_map(|element| render_lark_card_element(element, attachment, 0))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn card_escape_attr(value: &str) -> String {
    value.replace('"', "\\\"").replace('\n', "\\n")
}

fn card_format_milliseconds_iso8601(value: &Value) -> Option<String> {
    let ms = match value {
        Value::String(raw) => raw.parse::<i64>().ok()?,
        Value::Number(number) => number.as_i64()?,
        _ => return None,
    };
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)?;
    Some(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn card_normalize_time_format(value: &Value) -> Option<String> {
    let input = match value {
        Value::String(raw) => raw.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    if input.is_empty() {
        return None;
    }
    if let Ok(num) = input.parse::<i64>() {
        if input.len() >= 13 {
            return chrono::DateTime::<chrono::Utc>::from_timestamp_millis(num)
                .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        }
        if input.len() >= 10 {
            return chrono::DateTime::<chrono::Utc>::from_timestamp(num, 0)
                .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        }
    }
    Some(input)
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
        "sticker" => push_resource(
            LarkInboundResourceKind::Sticker,
            parsed.get("file_key").and_then(Value::as_str),
        ),
        "file" => push_resource(
            LarkInboundResourceKind::File,
            parsed.get("file_key").and_then(Value::as_str),
        ),
        "folder" => push_resource(
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
        "post" => {
            if let Some(locale) =
                parsed
                    .get("zh_cn")
                    .or_else(|| parsed.get("en_us"))
                    .or_else(|| {
                        parsed
                            .as_object()
                            .and_then(|items| items.values().find(|value| value.is_object()))
                    })
            {
                if let Some(paragraphs) = locale.get("content").and_then(Value::as_array) {
                    for paragraph in paragraphs {
                        if let Some(elements) = paragraph.as_array() {
                            for element in elements {
                                match element
                                    .get("tag")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                {
                                    "img" => push_resource(
                                        LarkInboundResourceKind::Image,
                                        element.get("image_key").and_then(Value::as_str),
                                    ),
                                    "media" => push_resource(
                                        LarkInboundResourceKind::File,
                                        element.get("file_key").and_then(Value::as_str),
                                    ),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
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
