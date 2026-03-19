use super::*;

fn pick_uniform_index(len: usize) -> usize {
    debug_assert!(len > 0);
    let upper = len as u64;
    let reject_threshold = (u64::MAX / upper) * upper;

    loop {
        let value = rand::random::<u64>();
        if value < reject_threshold {
            return (value % upper) as usize;
        }
    }
}

fn random_from_pool(pool: &'static [&'static str]) -> &'static str {
    pool[pick_uniform_index(pool.len())]
}

fn lark_ack_pool(locale: LarkAckLocale) -> &'static [&'static str] {
    match locale {
        LarkAckLocale::ZhCn => LARK_ACK_REACTIONS_ZH_CN,
        LarkAckLocale::ZhTw => LARK_ACK_REACTIONS_ZH_TW,
        LarkAckLocale::En => LARK_ACK_REACTIONS_EN,
        LarkAckLocale::Ja => LARK_ACK_REACTIONS_JA,
    }
}

pub(super) fn map_locale_tag(tag: &str) -> Option<LarkAckLocale> {
    let normalized = tag.trim().to_ascii_lowercase().replace('-', "_");
    if normalized.is_empty() {
        return None;
    }

    if normalized.starts_with("ja") {
        return Some(LarkAckLocale::Ja);
    }
    if normalized.starts_with("en") {
        return Some(LarkAckLocale::En);
    }
    if normalized.contains("hant")
        || normalized.starts_with("zh_tw")
        || normalized.starts_with("zh_hk")
        || normalized.starts_with("zh_mo")
    {
        return Some(LarkAckLocale::ZhTw);
    }
    if normalized.starts_with("zh") {
        return Some(LarkAckLocale::ZhCn);
    }
    None
}

fn find_locale_hint(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for key in [
                "locale",
                "language",
                "lang",
                "i18n_locale",
                "user_locale",
                "locale_id",
            ] {
                if let Some(locale) = map.get(key).and_then(serde_json::Value::as_str) {
                    return Some(locale.to_string());
                }
            }

            for child in map.values() {
                if let Some(locale) = find_locale_hint(child) {
                    return Some(locale);
                }
            }
            None
        }
        serde_json::Value::Array(items) => {
            for child in items {
                if let Some(locale) = find_locale_hint(child) {
                    return Some(locale);
                }
            }
            None
        }
        _ => None,
    }
}

fn detect_locale_from_post_content(content: &str) -> Option<LarkAckLocale> {
    let parsed = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let obj = parsed.as_object()?;
    for key in obj.keys() {
        if let Some(locale) = map_locale_tag(key) {
            return Some(locale);
        }
    }
    None
}

fn is_japanese_kana(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3040..=0x309F |
        0x30A0..=0x30FF |
        0x31F0..=0x31FF
    )
}

fn is_cjk_han(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF |
        0x4E00..=0x9FFF
    )
}

fn is_traditional_only_han(ch: char) -> bool {
    matches!(
        ch,
        '奮' | '鬥'
            | '強'
            | '體'
            | '國'
            | '臺'
            | '萬'
            | '與'
            | '為'
            | '這'
            | '學'
            | '機'
            | '開'
            | '裡'
    )
}

fn is_simplified_only_han(ch: char) -> bool {
    matches!(
        ch,
        '奋' | '斗'
            | '强'
            | '体'
            | '国'
            | '台'
            | '万'
            | '与'
            | '为'
            | '这'
            | '学'
            | '机'
            | '开'
            | '里'
    )
}

fn detect_locale_from_text(text: &str) -> Option<LarkAckLocale> {
    if text.chars().any(is_japanese_kana) {
        return Some(LarkAckLocale::Ja);
    }
    if text.chars().any(is_traditional_only_han) {
        return Some(LarkAckLocale::ZhTw);
    }
    if text.chars().any(is_simplified_only_han) {
        return Some(LarkAckLocale::ZhCn);
    }
    if text.chars().any(is_cjk_han) {
        return Some(LarkAckLocale::ZhCn);
    }
    None
}

pub(super) fn detect_lark_ack_locale(
    payload: Option<&serde_json::Value>,
    fallback_text: &str,
) -> LarkAckLocale {
    if let Some(payload) = payload {
        if let Some(locale) = find_locale_hint(payload).and_then(|hint| map_locale_tag(&hint)) {
            return locale;
        }

        let message_content = payload
            .pointer("/message/content")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                payload
                    .pointer("/event/message/content")
                    .and_then(serde_json::Value::as_str)
            });

        if let Some(locale) = message_content.and_then(detect_locale_from_post_content) {
            return locale;
        }
    }

    detect_locale_from_text(fallback_text).unwrap_or(LarkAckLocale::En)
}

pub(super) fn random_lark_ack_reaction(
    payload: Option<&serde_json::Value>,
    fallback_text: &str,
) -> &'static str {
    let locale = detect_lark_ack_locale(payload, fallback_text);
    random_from_pool(lark_ack_pool(locale))
}

pub(super) struct ParsedPostContent {
    pub(super) text: String,
    pub(super) normalized_content: String,
    pub(super) mentioned_open_ids: Vec<String>,
}

fn unwrap_post_locale<'a>(parsed: &'a serde_json::Value) -> Option<&'a serde_json::Value> {
    if parsed.get("title").is_some() || parsed.get("content").is_some() {
        return Some(parsed);
    }

    parsed
        .get("zh_cn")
        .or_else(|| parsed.get("en_us"))
        .or_else(|| {
            parsed
                .as_object()
                .and_then(|m| m.values().find(|v| v.is_object()))
        })
}

pub(super) fn parse_post_content_details(content: &str) -> Option<ParsedPostContent> {
    let parsed = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let locale = unwrap_post_locale(&parsed)?;

    let mut text = String::new();
    let mut normalized_lines = Vec::new();
    let mut mentioned_open_ids = Vec::new();

    if let Some(title) = locale
        .get("title")
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())
    {
        text.push_str(title);
        text.push_str("\n\n");
        normalized_lines.push(format!("**{title}**"));
        normalized_lines.push(String::new());
    }

    if let Some(paragraphs) = locale.get("content").and_then(|c| c.as_array()) {
        for para in paragraphs {
            if let Some(elements) = para.as_array() {
                let mut normalized_line = String::new();
                for el in elements {
                    match el.get("tag").and_then(|t| t.as_str()).unwrap_or("") {
                        "text" => {
                            if let Some(t) = el.get("text").and_then(|t| t.as_str()) {
                                text.push_str(t);
                                normalized_line.push_str(t);
                            }
                        }
                        "a" => {
                            let label = el
                                .get("text")
                                .and_then(|t| t.as_str())
                                .filter(|s| !s.is_empty())
                                .or_else(|| el.get("href").and_then(|h| h.as_str()))
                                .unwrap_or("");
                            let href = el.get("href").and_then(|h| h.as_str()).unwrap_or("");
                            text.push_str(label);
                            if href.is_empty() {
                                normalized_line.push_str(label);
                            } else {
                                normalized_line.push_str(&format!("[{label}]({href})"));
                            }
                        }
                        "at" => {
                            let n = el
                                .get("user_name")
                                .and_then(|n| n.as_str())
                                .or_else(|| el.get("user_id").and_then(|i| i.as_str()))
                                .unwrap_or("user");
                            text.push('@');
                            text.push_str(n);
                            normalized_line.push('@');
                            normalized_line.push_str(n);
                            if let Some(open_id) = el
                                .get("user_id")
                                .and_then(|i| i.as_str())
                                .map(str::trim)
                                .filter(|id| !id.is_empty())
                            {
                                mentioned_open_ids.push(open_id.to_string());
                            }
                        }
                        "img" => {
                            if let Some(image_key) = el.get("image_key").and_then(|v| v.as_str()) {
                                normalized_line.push_str(&format!("![image]({image_key})"));
                            }
                        }
                        "media" => {
                            if let Some(file_key) = el.get("file_key").and_then(|v| v.as_str()) {
                                normalized_line.push_str(&format!("<file key=\"{file_key}\"/>"));
                            }
                        }
                        _ => {}
                    }
                }
                text.push('\n');
                normalized_lines.push(normalized_line);
            }
        }
    }

    let result = text.trim().to_string();
    let normalized_content = normalized_lines.join("\n").trim().to_string();
    if result.is_empty() && normalized_content.is_empty() {
        None
    } else {
        Some(ParsedPostContent {
            text: result,
            normalized_content,
            mentioned_open_ids,
        })
    }
}

pub(super) fn parse_post_content(content: &str) -> Option<String> {
    parse_post_content_details(content).map(|details| details.text)
}

pub(super) fn strip_at_placeholders(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch == '@' {
            let rest: String = chars.clone().map(|(_, c)| c).collect();
            if let Some(after) = rest.strip_prefix("_user_") {
                let skip =
                    "_user_".len() + after.chars().take_while(|c| c.is_ascii_digit()).count();
                for _ in 0..=skip {
                    chars.next();
                }
                if chars.peek().map(|(_, c)| *c == ' ').unwrap_or(false) {
                    chars.next();
                }
                continue;
            }
        }
        result.push(ch);
    }
    result
}

fn mention_matches_bot_open_id(mention: &serde_json::Value, bot_open_id: &str) -> bool {
    mention
        .pointer("/id/open_id")
        .or_else(|| mention.pointer("/open_id"))
        .and_then(|v| v.as_str())
        .is_some_and(|value| value == bot_open_id)
}

pub(super) fn should_respond_in_group(
    mention_only: bool,
    bot_open_id: Option<&str>,
    mentions: &[serde_json::Value],
    post_mentioned_open_ids: &[String],
) -> bool {
    if !mention_only {
        return true;
    }
    let Some(bot_open_id) = bot_open_id.filter(|id| !id.is_empty()) else {
        return false;
    };
    if mentions.is_empty() && post_mentioned_open_ids.is_empty() {
        return false;
    }
    mentions
        .iter()
        .any(|mention| mention_matches_bot_open_id(mention, bot_open_id))
        || post_mentioned_open_ids
            .iter()
            .any(|id| id.as_str() == bot_open_id)
}
