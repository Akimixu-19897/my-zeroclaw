use regex::Regex;

pub(crate) const SILENT_REPLY_TOKEN: &str = "NO_REPLY";

/// Returns true when the payload is exactly the channel silent-reply token.
pub(crate) fn is_silent_reply_text(message: &str) -> bool {
    message.trim() == SILENT_REPLY_TOKEN
}

/// Removes a trailing channel silent-reply token from mixed-content text.
///
/// Examples:
/// - `"Done.\n\nNO_REPLY"` -> `"Done."`
/// - `"[AUDIO:/tmp/x.ogg]\nNO_REPLY"` -> `"[AUDIO:/tmp/x.ogg]"`
pub(crate) fn strip_trailing_silent_reply_token(message: &str) -> String {
    static TRAILING_SILENT_REPLY_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?:^|\s+|\*+)NO_REPLY\s*$").expect("valid trailing NO_REPLY regex")
    });

    TRAILING_SILENT_REPLY_RE.replace(message, "").trim().to_string()
}

/// Strip tool-call XML tags from outgoing messages.
///
/// LLM responses may contain `<function_calls>`, `<function_call>`,
/// `<tool_call>`, `<toolcall>`, `<tool-call>`, `<tool>`, or `<invoke>`
/// blocks that are internal protocol and must not be forwarded to end
/// users on any channel.
pub(crate) fn strip_tool_call_tags(message: &str) -> String {
    const TOOL_CALL_OPEN_TAGS: [&str; 7] = [
        "<function_calls>",
        "<function_call>",
        "<tool_call>",
        "<toolcall>",
        "<tool-call>",
        "<tool>",
        "<invoke>",
    ];

    fn find_first_tag<'a>(haystack: &str, tags: &'a [&'a str]) -> Option<(usize, &'a str)> {
        tags.iter()
            .filter_map(|tag| haystack.find(tag).map(|idx| (idx, *tag)))
            .min_by_key(|(idx, _)| *idx)
    }

    fn matching_close_tag(open_tag: &str) -> Option<&'static str> {
        match open_tag {
            "<function_calls>" => Some("</function_calls>"),
            "<function_call>" => Some("</function_call>"),
            "<tool_call>" => Some("</tool_call>"),
            "<toolcall>" => Some("</toolcall>"),
            "<tool-call>" => Some("</tool-call>"),
            "<tool>" => Some("</tool>"),
            "<invoke>" => Some("</invoke>"),
            _ => None,
        }
    }

    fn extract_first_json_end(input: &str) -> Option<usize> {
        let trimmed = input.trim_start();
        let trim_offset = input.len().saturating_sub(trimmed.len());

        for (byte_idx, ch) in trimmed.char_indices() {
            if ch != '{' && ch != '[' {
                continue;
            }

            let slice = &trimmed[byte_idx..];
            let mut stream =
                serde_json::Deserializer::from_str(slice).into_iter::<serde_json::Value>();
            if let Some(Ok(_value)) = stream.next() {
                let consumed = stream.byte_offset();
                if consumed > 0 {
                    return Some(trim_offset + byte_idx + consumed);
                }
            }
        }

        None
    }

    fn strip_leading_close_tags(mut input: &str) -> &str {
        loop {
            let trimmed = input.trim_start();
            if !trimmed.starts_with("</") {
                return trimmed;
            }

            let Some(close_end) = trimmed.find('>') else {
                return "";
            };
            input = &trimmed[close_end + 1..];
        }
    }

    let mut kept_segments = Vec::new();
    let mut remaining = message;

    while let Some((start, open_tag)) = find_first_tag(remaining, &TOOL_CALL_OPEN_TAGS) {
        let before = &remaining[..start];
        if !before.is_empty() {
            kept_segments.push(before.to_string());
        }

        let Some(close_tag) = matching_close_tag(open_tag) else {
            break;
        };
        let after_open = &remaining[start + open_tag.len()..];

        if let Some(close_idx) = after_open.find(close_tag) {
            remaining = &after_open[close_idx + close_tag.len()..];
            continue;
        }

        if let Some(consumed_end) = extract_first_json_end(after_open) {
            remaining = strip_leading_close_tags(&after_open[consumed_end..]);
            continue;
        }

        kept_segments.push(remaining[start..].to_string());
        remaining = "";
        break;
    }

    if !remaining.is_empty() {
        kept_segments.push(remaining.to_string());
    }

    let mut result = kept_segments.concat();

    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }

    result.trim().to_string()
}

/// Normalize markdown text for channel delivery.
///
/// This is channel-agnostic text preprocessing shared by outbound adapters.
/// It intentionally avoids transport-specific envelope logic such as Lark
/// `post` payloads or platform receive-id routing.
pub(crate) fn normalize_channel_markdown_text(message: &str) -> String {
    let with_mentions = normalize_at_mentions(message);
    let with_tables = convert_markdown_tables_to_bullets(&with_mentions);
    optimize_markdown_style(&with_tables, 1)
}

fn normalize_at_mentions(content: &str) -> String {
    let bytes = content.as_bytes();
    let mut index = 0usize;
    let mut output = String::with_capacity(content.len());

    while let Some(relative) = content[index..].find("<at ") {
        let start = index + relative;
        output.push_str(&content[index..start]);

        let Some(close_relative) = content[start..].find('>') else {
            output.push_str(&content[start..]);
            return output;
        };
        let end = start + close_relative + 1;
        let tag = &content[start..end];
        output.push_str(&normalize_single_at_tag(tag));
        index = end;
    }

    if index < bytes.len() {
        output.push_str(&content[index..]);
    }

    output
}

fn normalize_single_at_tag(tag: &str) -> String {
    for key in ["user_id", "open_id", "id"] {
        let Some(value) = extract_at_attribute(tag, key) else {
            continue;
        };
        let quoted = format!(r#"{key}="{value}""#);
        if tag.contains(&quoted) {
            return tag.replacen(&quoted, &format!(r#"user_id="{value}""#), 1);
        }

        let bare = format!("{key}={value}");
        return tag.replacen(&bare, &format!(r#"user_id="{value}""#), 1);
    }

    tag.to_string()
}

fn extract_at_attribute<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let pattern = format!("{key}=");
    let start = tag.find(&pattern)? + pattern.len();
    let tail = &tag[start..];
    if let Some(rest) = tail.strip_prefix('"') {
        let end = rest.find('"')?;
        Some(&rest[..end])
    } else {
        let end = tail.find([' ', '>']).unwrap_or(tail.len());
        Some(&tail[..end])
    }
}

fn convert_markdown_tables_to_bullets(content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let mut index = 0usize;
    let mut result = Vec::new();

    while index < lines.len() {
        if let Some((consumed, rendered)) = try_convert_markdown_table(&lines[index..]) {
            result.push(rendered);
            index += consumed;
            continue;
        }
        result.push(lines[index].to_string());
        index += 1;
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_reply_text_matches_exact_token() {
        assert!(is_silent_reply_text("NO_REPLY"));
        assert!(is_silent_reply_text("  NO_REPLY  "));
        assert!(!is_silent_reply_text("NO_REPLY later"));
    }

    #[test]
    fn strip_trailing_silent_reply_token_removes_suffix_only() {
        assert_eq!(strip_trailing_silent_reply_token("Done.\n\nNO_REPLY"), "Done.");
        assert_eq!(
            strip_trailing_silent_reply_token("[AUDIO:/tmp/test.ogg]\nNO_REPLY"),
            "[AUDIO:/tmp/test.ogg]"
        );
        assert_eq!(
            strip_trailing_silent_reply_token("NO_REPLY -- keep me"),
            "NO_REPLY -- keep me"
        );
    }
}

fn try_convert_markdown_table(lines: &[&str]) -> Option<(usize, String)> {
    if lines.len() < 2 {
        return None;
    }

    let header_line = lines[0].trim();
    let divider_line = lines[1].trim();
    if !looks_like_markdown_table_row(header_line)
        || !looks_like_markdown_table_divider(divider_line)
    {
        return None;
    }

    let headers = parse_markdown_table_row(header_line);
    if headers.is_empty() {
        return None;
    }

    let mut rows = Vec::new();
    let mut consumed = 2usize;
    while consumed < lines.len() {
        let row_line = lines[consumed].trim();
        if !looks_like_markdown_table_row(row_line) {
            break;
        }
        rows.push(parse_markdown_table_row(row_line));
        consumed += 1;
    }

    if rows.is_empty() {
        return None;
    }

    Some((consumed, render_markdown_table_as_bullets(&headers, &rows)))
}

fn looks_like_markdown_table_row(line: &str) -> bool {
    line.starts_with('|') && line.ends_with('|') && line.matches('|').count() >= 2
}

fn looks_like_markdown_table_divider(line: &str) -> bool {
    looks_like_markdown_table_row(line)
        && parse_markdown_table_row(line)
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
}

fn parse_markdown_table_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn render_markdown_table_as_bullets(headers: &[String], rows: &[Vec<String>]) -> String {
    let use_first_col_as_label = headers.len() > 1 && !rows.is_empty();
    let mut out = String::new();

    for row in rows {
        if use_first_col_as_label {
            if let Some(label) = row.first().filter(|value| !value.trim().is_empty()) {
                out.push_str("**");
                out.push_str(label.trim());
                out.push_str("**\n");
            }

            for column_index in 1..row.len() {
                append_table_bullet_line(
                    &mut out,
                    headers.get(column_index),
                    row.get(column_index),
                    column_index,
                );
            }
            out.push('\n');
            continue;
        }

        for column_index in 0..row.len() {
            append_table_bullet_line(
                &mut out,
                headers.get(column_index),
                row.get(column_index),
                column_index,
            );
        }
        out.push('\n');
    }

    out.trim_end().to_string()
}

fn append_table_bullet_line(
    out: &mut String,
    header: Option<&String>,
    value: Option<&String>,
    column_index: usize,
) {
    let value = value.map(|item| item.trim()).unwrap_or_default();
    if value.is_empty() {
        return;
    }

    out.push_str("• ");
    if let Some(header) = header
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
    {
        out.push_str(header);
        out.push_str(": ");
    } else {
        out.push_str(&format!("Column {column_index}: "));
    }
    out.push_str(value);
    out.push('\n');
}

fn optimize_markdown_style(content: &str, card_version: u8) -> String {
    let code_block_re = Regex::new(r"```[\s\S]*?```").expect("valid code block regex");
    let h1_to_h3_re = Regex::new(r"(?m)^#{1,3} ").expect("valid heading detector");
    let h2_to_h6_line_re = Regex::new(r"(?m)^#{2,6} (.+)$").expect("valid h2-h6 regex");
    let h1_line_re = Regex::new(r"(?m)^# (.+)$").expect("valid h1 regex");
    let extra_blank_lines_re = Regex::new(r"\n{3,}").expect("valid blank line regex");

    let code_blocks = code_block_re
        .find_iter(content)
        .map(|m| m.as_str().to_string())
        .collect::<Vec<_>>();
    let mut placeholder_index = 0usize;
    let mut protected = code_block_re
        .replace_all(content, |_caps: &regex::Captures| {
            let index = placeholder_index;
            placeholder_index += 1;
            format!("___CB_{index}___")
        })
        .into_owned();

    if h1_to_h3_re.is_match(content) {
        protected = h2_to_h6_line_re
            .replace_all(&protected, "##### $1")
            .into_owned();
        protected = h1_line_re.replace_all(&protected, "#### $1").into_owned();
    }

    for (index, block) in code_blocks.iter().enumerate() {
        let replacement = if card_version >= 2 {
            format!("\n<br>\n{block}\n<br>\n")
        } else {
            block.clone()
        };
        protected = protected.replace(&format!("___CB_{index}___"), &replacement);
    }

    let collapsed = extra_blank_lines_re
        .replace_all(&protected, "\n\n")
        .into_owned();
    strip_invalid_image_keys(&collapsed)
}

fn strip_invalid_image_keys(content: &str) -> String {
    if !content.contains("![") {
        return content.to_string();
    }

    Regex::new(r"!\[([^\]]*)\]\(([^)\s]+)\)")
        .expect("valid markdown image regex")
        .replace_all(content, |caps: &regex::Captures| {
            let full = caps.get(0).map(|m| m.as_str()).unwrap_or_default();
            let value = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
            if value.starts_with("img_")
                || value.starts_with("http://")
                || value.starts_with("https://")
            {
                full.to_string()
            } else {
                value.to_string()
            }
        })
        .into_owned()
}
