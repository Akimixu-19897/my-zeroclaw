use std::time::{Duration, Instant};

use serde_json::Value;

pub(crate) const LARK_CARD_PATCH_THROTTLE: Duration = Duration::from_millis(1_500);
pub(crate) const LARK_CARD_LONG_GAP_THRESHOLD: Duration = Duration::from_millis(2_000);
pub(crate) const LARK_CARD_BATCH_AFTER_GAP: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LarkCardPhase {
    Thinking,
    Generating,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LarkCardMessage {
    pub(crate) content: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LarkCardActionEvent {
    pub(crate) action: String,
    pub(crate) operation_id: Option<String>,
    pub(crate) message_id: Option<String>,
    pub(crate) chat_id: Option<String>,
    pub(crate) operator_open_id: Option<String>,
    pub(crate) value: Value,
}

impl LarkCardMessage {
    pub(crate) fn new(content: Value) -> Self {
        Self { content }
    }
}

pub(crate) fn parse_lark_card_message(raw_content: &str) -> Option<LarkCardMessage> {
    let trimmed = raw_content.trim();
    let body = extract_fenced_block(trimmed, "lark-card")
        .or_else(|| extract_fenced_block(trimmed, "feishu-card"))?;
    let content = serde_json::from_str::<Value>(body).ok()?;
    Some(LarkCardMessage::new(content))
}

pub(crate) fn build_lark_card_message_body(
    recipient: &str,
    card: &LarkCardMessage,
) -> serde_json::Value {
    serde_json::json!({
        "receive_id": recipient,
        "msg_type": "interactive",
        "content": card.content.to_string(),
    })
}

pub(crate) fn build_lark_reply_card_message_body(
    card: &LarkCardMessage,
    reply_in_thread: bool,
) -> serde_json::Value {
    serde_json::json!({
        "msg_type": "interactive",
        "content": card.content.to_string(),
        "reply_in_thread": reply_in_thread,
    })
}

pub(crate) fn build_lark_markdown_card(text: &str) -> LarkCardMessage {
    build_lark_streaming_card(LarkCardPhase::Completed, text)
}

pub(crate) fn build_lark_confirmation_card(
    operation_id: &str,
    operation_description: &str,
    preview: Option<&str>,
) -> LarkCardMessage {
    let mut actions = vec![
        serde_json::json!({
            "tag": "button",
            "text": { "tag": "plain_text", "content": "Confirm" },
            "type": "primary",
            "value": {
                "action": "confirm_write",
                "operation_id": operation_id,
            }
        }),
        serde_json::json!({
            "tag": "button",
            "text": { "tag": "plain_text", "content": "Reject" },
            "type": "danger",
            "value": {
                "action": "reject_write",
                "operation_id": operation_id,
            }
        }),
    ];

    if preview.is_none() {
        actions.push(serde_json::json!({
            "tag": "button",
            "text": { "tag": "plain_text", "content": "Preview" },
            "type": "default",
            "value": {
                "action": "preview_write",
                "operation_id": operation_id,
            }
        }));
    }

    let mut elements = vec![serde_json::json!({
        "tag": "div",
        "text": {
            "tag": "lark_md",
            "content": operation_description,
        }
    })];

    if let Some(preview) = preview.filter(|value| !value.trim().is_empty()) {
        elements.push(serde_json::json!({ "tag": "hr" }));
        elements.push(serde_json::json!({
            "tag": "div",
            "text": {
                "tag": "lark_md",
                "content": format!("**Preview:**\n{preview}"),
            }
        }));
    }

    elements.push(serde_json::json!({ "tag": "hr" }));
    elements.push(serde_json::json!({
        "tag": "action",
        "actions": actions,
    }));

    LarkCardMessage::new(serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true,
            "update_multi": true,
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": "Confirmation Required",
            },
            "template": "orange",
        },
        "body": {
            "elements": elements,
        }
    }))
}

pub(crate) fn build_lark_streaming_card(phase: LarkCardPhase, text: &str) -> LarkCardMessage {
    let content = if text.trim().is_empty() { "..." } else { text };
    let title = match phase {
        LarkCardPhase::Thinking => "Thinking",
        LarkCardPhase::Generating => "Generating",
        LarkCardPhase::Completed => "Completed",
        LarkCardPhase::Failed => "Failed",
    };

    LarkCardMessage::new(serde_json::json!({
        "schema": "2.0",
        "header": {
            "title": {
                "tag": "plain_text",
                "content": title,
            }
        },
        "config": {
            "wide_screen_mode": true,
            "update_multi": true,
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": content,
                }
            ]
        }
    }))
}

pub(crate) fn next_lark_stream_flush_deadline(last_flush_at: Instant, now: Instant) -> Instant {
    let elapsed = now.saturating_duration_since(last_flush_at);
    if elapsed >= LARK_CARD_PATCH_THROTTLE {
        if elapsed > LARK_CARD_LONG_GAP_THRESHOLD {
            now + LARK_CARD_BATCH_AFTER_GAP
        } else {
            now
        }
    } else {
        last_flush_at + LARK_CARD_PATCH_THROTTLE
    }
}

pub(crate) fn parse_lark_card_action_event(payload: &Value) -> Option<LarkCardActionEvent> {
    let action_value = payload.get("action")?.get("value")?.clone();
    let action = action_value
        .get("action")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())?
        .to_string();

    let operation_id = action_value
        .get("operation_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);

    let message_id = payload
        .get("open_message_id")
        .or_else(|| payload.get("message_id"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);

    let chat_id = payload
        .get("open_chat_id")
        .or_else(|| payload.get("chat_id"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);

    let operator_open_id = payload
        .pointer("/operator/open_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);

    Some(LarkCardActionEvent {
        action,
        operation_id,
        message_id,
        chat_id,
        operator_open_id,
        value: action_value,
    })
}

pub(crate) fn render_lark_card_action_event_content(event: &LarkCardActionEvent) -> String {
    let body = serde_json::json!({
        "type": "lark_card_action",
        "action": event.action,
        "operation_id": event.operation_id,
        "message_id": event.message_id,
        "chat_id": event.chat_id,
        "operator_open_id": event.operator_open_id,
        "value": event.value,
    });

    format!("```lark-card-action\n{}\n```", body)
}

fn extract_fenced_block<'a>(content: &'a str, language: &str) -> Option<&'a str> {
    let prefix = format!("```{language}");
    let body = content.strip_prefix(&prefix)?;
    let body = body
        .strip_prefix('\n')
        .or_else(|| body.strip_prefix("\r\n"))?;
    body.strip_suffix("```").map(str::trim)
}
