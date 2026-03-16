use super::super::*;
use super::processing_support::{
    approval_card_thread, approved_tool_args, channel_supports_lark_cards, find_tool_for_channel,
    parse_lark_card_action_content, summarize_tool_args_for_approval,
};
use super::routing::get_or_create_provider;

pub(crate) async fn send_pending_approval_message(
    channel: &dyn Channel,
    msg: &traits::ChannelMessage,
    approval: &PendingChannelApproval,
) -> anyhow::Result<()> {
    if channel_supports_lark_cards(channel.name()) {
        #[cfg(feature = "channel-lark")]
        {
            let card = crate::channels::lark::cards::build_lark_confirmation_card(
                &approval.operation_id,
                &summarize_tool_args_for_approval(&approval.tool_name, &approval.reason),
                None,
            );
            let content = format!("```lark-card\n{}\n```", card.content);
            return channel
                .send(
                    &SendMessage::new(content, &msg.reply_target)
                        .in_thread(approval_card_thread(msg)),
                )
                .await;
        }
    }

    let fallback = format!(
        "Approval required for `{}`.\nReason: {}\n\nReply `approve {}` to continue.\n\n{}",
        approval.tool_name, approval.reason, approval.operation_id, approval.preview
    );
    channel
        .send(&SendMessage::new(fallback, &msg.reply_target).in_thread(approval_card_thread(msg)))
        .await
}

pub(crate) async fn render_pending_approval_tool_result(
    ctx: &ChannelRuntimeContext,
    approval: &PendingChannelApproval,
    result: &tools::ToolResult,
) -> String {
    let tool_result_text = if result.success {
        if result.output.trim().is_empty() {
            format!("`{}` completed successfully.", approval.tool_name)
        } else {
            result.output.clone()
        }
    } else {
        let err = result
            .error
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("unknown tool error");
        format!("Error: {err}")
    };

    let provider = match get_or_create_provider(ctx, &approval.provider).await {
        Ok(provider) => provider,
        Err(_) => return tool_result_text,
    };

    let prompt = format!(
        "The user previously asked:\n{}\n\nA sensitive tool call required approval.\nTool: {}\nArguments: {}\n\nThe operator approved the action and the tool returned:\n{}\n\nWrite the final user-facing reply. Be concise, mention failures plainly, and do not ask for another approval.",
        approval.user_message,
        approval.tool_name,
        approval.arguments,
        tool_result_text,
    );

    provider
        .chat_with_system(
            Some(
                "You are ZeroClaw completing a previously approved tool action. Respond with the final answer for the user, not internal workflow notes.",
            ),
            &prompt,
            &approval.model,
            ctx.temperature,
        )
        .await
        .map(|reply| {
            let trimmed = reply.trim();
            if trimmed.is_empty() {
                tool_result_text.clone()
            } else {
                trimmed.to_string()
            }
        })
        .unwrap_or(tool_result_text)
}

pub(crate) async fn handle_pending_approval_action(
    ctx: &ChannelRuntimeContext,
    msg: &traits::ChannelMessage,
    target_channel: Option<&Arc<dyn Channel>>,
) -> bool {
    let Some(action) = parse_lark_card_action_content(&msg.content) else {
        return false;
    };
    let Some(operation_id) = action.operation_id.as_deref() else {
        return false;
    };
    let Some(channel) = target_channel else {
        return true;
    };
    let Some(approval) = get_pending_channel_approval(operation_id) else {
        let _ = channel
            .send(
                &SendMessage::new(
                    "This approval request is no longer available. Please retry the original action.",
                    &msg.reply_target,
                )
                .in_thread(msg.thread_ts.clone()),
            )
            .await;
        return true;
    };

    match action.action.as_str() {
        "preview_write" => {
            let _ = channel
                .send(
                    &SendMessage::new(approval.preview.clone(), &approval.reply_target)
                        .in_thread(approval.thread_ts.clone()),
                )
                .await;
        }
        "reject_write" => {
            let _ = take_pending_channel_approval(operation_id);
            let _ = channel
                .send(
                    &SendMessage::new(
                        format!("Rejected `{}`. No action was taken.", approval.tool_name),
                        &approval.reply_target,
                    )
                    .in_thread(approval.thread_ts.clone()),
                )
                .await;
        }
        "confirm_write" => {
            let Some(approval) = take_pending_channel_approval(operation_id) else {
                return true;
            };
            let Some(tool) =
                find_tool_for_channel(ctx.tools_registry.as_ref(), &approval.tool_name)
            else {
                let _ = channel
                    .send(
                        &SendMessage::new(
                            format!(
                                "Approval target `{}` is no longer available.",
                                approval.tool_name
                            ),
                            &approval.reply_target,
                        )
                        .in_thread(approval.thread_ts.clone()),
                    )
                    .await;
                return true;
            };
            let result = match tool.execute(approved_tool_args(&approval.arguments)).await {
                Ok(result) => result,
                Err(err) => tools::ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(err.to_string()),
                },
            };
            let rendered = render_pending_approval_tool_result(ctx, &approval, &result).await;
            let _ = channel
                .send(
                    &SendMessage::new(rendered, &approval.reply_target)
                        .in_thread(approval.thread_ts.clone()),
                )
                .await;
        }
        _ => {}
    }

    true
}
