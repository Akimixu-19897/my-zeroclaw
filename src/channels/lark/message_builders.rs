use crate::channels::normalize_channel_markdown_text;

pub(crate) fn build_lark_text_message_body(recipient: &str, content: &str) -> serde_json::Value {
    serde_json::json!({
        "receive_id": recipient,
        "msg_type": "post",
        "content": build_lark_post_content(content).to_string(),
    })
}

pub(crate) fn build_lark_image_message_body(recipient: &str, image_key: &str) -> serde_json::Value {
    serde_json::json!({
        "receive_id": recipient,
        "msg_type": "image",
        "content": serde_json::json!({ "image_key": image_key }).to_string(),
    })
}

pub(crate) fn build_lark_file_message_body(recipient: &str, file_key: &str) -> serde_json::Value {
    serde_json::json!({
        "receive_id": recipient,
        "msg_type": "file",
        "content": serde_json::json!({ "file_key": file_key }).to_string(),
    })
}

pub(crate) fn build_lark_audio_message_body(recipient: &str, file_key: &str) -> serde_json::Value {
    serde_json::json!({
        "receive_id": recipient,
        "msg_type": "audio",
        "content": serde_json::json!({ "file_key": file_key }).to_string(),
    })
}

pub(crate) fn build_lark_video_message_body(recipient: &str, file_key: &str) -> serde_json::Value {
    serde_json::json!({
        "receive_id": recipient,
        "msg_type": "media",
        "content": serde_json::json!({ "file_key": file_key }).to_string(),
    })
}

pub(crate) fn build_lark_reply_message_body(
    msg_type: &str,
    content: serde_json::Value,
    reply_in_thread: bool,
) -> serde_json::Value {
    serde_json::json!({
        "msg_type": msg_type,
        "content": content.to_string(),
        "reply_in_thread": reply_in_thread,
    })
}

pub(crate) fn build_lark_post_content(content: &str) -> serde_json::Value {
    let normalized = normalize_channel_markdown_text(content);
    serde_json::json!({
        "zh_cn": {
            "content": [[{
                "tag": "md",
                "text": normalized,
            }]]
        }
    })
}
