use crate::channels::traits;
use std::borrow::Cow;

pub(crate) fn conversation_memory_key(msg: &traits::ChannelMessage) -> String {
    match conversation_thread_scope(msg) {
        Some(thread_scope) => format!("{}_{}_{}_{}", msg.channel, thread_scope, msg.sender, msg.id),
        None => format!("{}_{}_{}", msg.channel, msg.sender, msg.id),
    }
}

pub(crate) fn conversation_history_key(msg: &traits::ChannelMessage) -> String {
    match conversation_thread_scope(msg) {
        Some(thread_scope) => format!("{}_{}_{}", msg.channel, thread_scope, msg.sender),
        None => format!("{}_{}", msg.channel, msg.sender),
    }
}

pub(crate) fn interruption_scope_key(msg: &traits::ChannelMessage) -> String {
    format!("{}_{}_{}", msg.channel, msg.reply_target, msg.sender)
}

pub(crate) fn channel_uses_lark_dispatch_context(channel_name: &str) -> bool {
    channel_name == "lark"
        || channel_name == "feishu"
        || channel_name.starts_with("lark:")
        || channel_name.starts_with("feishu:")
}

pub(crate) fn inbound_user_message_body(msg: &traits::ChannelMessage) -> Cow<'_, str> {
    let Some(context) = msg.context.as_ref() else {
        return Cow::Borrowed(msg.content.as_str());
    };
    if !channel_uses_lark_dispatch_context(&msg.channel) {
        return Cow::Borrowed(msg.content.as_str());
    };
    let Some(sender_id) = context
        .sender_id
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return Cow::Borrowed(msg.content.as_str());
    };
    let mut annotations = Vec::new();
    if let Some(content_type) = context
        .content_type
        .as_deref()
        .filter(|value| !value.is_empty() && *value != "text")
    {
        annotations.push(format!("content_type={content_type}"));
    }
    if let Some(parent_id) = context
        .parent_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        annotations.push(format!("parent_id={parent_id}"));
    }
    if let Some(thread_id) = context
        .thread_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        annotations.push(format!("thread_id={thread_id}"));
    }
    if let Some(chat_id) = context.chat_id.as_deref().filter(|value| !value.is_empty()) {
        annotations.push(format!("chat_id={chat_id}"));
    }

    let message = format!("{sender_id}: {}", msg.content);
    if annotations.is_empty() {
        Cow::Owned(message)
    } else {
        Cow::Owned(format!(
            "[LarkContext {}]\n{}",
            annotations.join(" "),
            message
        ))
    }
}

pub(crate) fn inbound_display_sender(msg: &traits::ChannelMessage) -> Cow<'_, str> {
    let Some(context) = msg.context.as_ref() else {
        return Cow::Borrowed(msg.sender.as_str());
    };
    if !channel_uses_lark_dispatch_context(&msg.channel) {
        return Cow::Borrowed(msg.sender.as_str());
    }

    match (
        context.chat_type.as_deref(),
        context.sender_id.as_deref(),
        context.chat_id.as_deref(),
    ) {
        (Some("group"), Some(sender_id), Some(chat_id)) => {
            Cow::Owned(format!("{sender_id} @ {chat_id}"))
        }
        (_, Some(sender_id), _) => Cow::Borrowed(sender_id),
        _ => Cow::Borrowed(msg.sender.as_str()),
    }
}

pub(crate) fn conversation_thread_scope(msg: &traits::ChannelMessage) -> Option<&str> {
    if channel_uses_lark_dispatch_context(&msg.channel) {
        return msg
            .context
            .as_ref()
            .and_then(|context| context.thread_id.as_deref())
            .filter(|value| !value.is_empty());
    }

    msg.thread_ts.as_deref()
}

pub(crate) fn should_forward_tool_events_as_thread_messages(channel_name: &str) -> bool {
    !(channel_name == "cli" || channel_name == "wecom" || channel_name.starts_with("wecom:"))
}

pub(crate) fn final_reply_thread_ts_after_tool_updates(
    msg: &traits::ChannelMessage,
    tools_used: bool,
) -> Option<String> {
    if tools_used && should_forward_tool_events_as_thread_messages(&msg.channel) {
        Some(msg.id.clone())
    } else {
        msg.thread_ts.clone()
    }
}
