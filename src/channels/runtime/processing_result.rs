use super::super::*;
use super::processing_approval::send_pending_approval_message;
use super::processing_support::{
    approval_card_thread, extract_tool_context_summary, parse_pending_channel_approval_request,
    sanitize_channel_response,
};
use super::routing::{
    append_sender_turn, compact_sender_history, is_context_window_overflow_error,
    rollback_orphan_user_turn,
};
use crate::agent::loop_::scrub_credentials;
use crate::observability::runtime_trace;
use crate::util::truncate_with_ellipsis;
use uuid::Uuid;

pub(super) enum LlmExecutionResult {
    Completed(Result<Result<String, anyhow::Error>, tokio::time::error::Elapsed>),
    Cancelled,
}

pub(super) struct ProcessingOutcomeContext {
    pub(super) ctx: Arc<ChannelRuntimeContext>,
    pub(super) msg: traits::ChannelMessage,
    pub(super) target_channel: Option<Arc<dyn Channel>>,
    pub(super) draft_message_id: Option<String>,
    pub(super) started_at: Instant,
    pub(super) history_key: String,
    pub(super) history_len_before_tools: usize,
    pub(super) history: Vec<ChatMessage>,
    pub(super) route: ChannelRouteSelection,
    pub(super) runtime_defaults: ChannelRuntimeDefaults,
    pub(super) cancellation_token: CancellationToken,
    pub(super) timeout_budget_secs: u64,
}

pub(super) async fn handle_llm_execution_result(
    outcome: ProcessingOutcomeContext,
    llm_result: LlmExecutionResult,
) {
    match llm_result {
        LlmExecutionResult::Cancelled => handle_cancelled(outcome).await,
        LlmExecutionResult::Completed(Ok(Ok(response))) => {
            handle_success(outcome, response).await;
        }
        LlmExecutionResult::Completed(Ok(Err(error))) => {
            handle_tool_loop_error(outcome, error).await;
        }
        LlmExecutionResult::Completed(Err(_)) => handle_timeout(outcome).await,
    }
}

async fn handle_cancelled(outcome: ProcessingOutcomeContext) {
    tracing::info!(
        channel = %outcome.msg.channel,
        sender = %outcome.msg.sender,
        "Cancelled in-flight channel request due to newer message"
    );
    runtime_trace::record_event(
        "channel_message_cancelled",
        Some(outcome.msg.channel.as_str()),
        Some(outcome.route.provider.as_str()),
        Some(outcome.route.model.as_str()),
        None,
        Some(false),
        Some("cancelled due to newer inbound message"),
        serde_json::json!({
            "sender": outcome.msg.sender,
            "elapsed_ms": outcome.started_at.elapsed().as_millis(),
        }),
    );
    cancel_draft_if_present(
        outcome.target_channel.as_ref(),
        &outcome.msg.reply_target,
        outcome.draft_message_id.as_deref(),
    )
    .await;
}

async fn handle_success(outcome: ProcessingOutcomeContext, response: String) {
    let mut outbound_response = response;
    if let Some(hooks) = &outcome.ctx.hooks {
        match hooks
            .run_on_message_sending(
                outcome.msg.channel.clone(),
                outcome.msg.reply_target.clone(),
                outbound_response.clone(),
            )
            .await
        {
            crate::hooks::HookResult::Cancel(reason) => {
                tracing::info!(%reason, "outgoing message suppressed by hook");
                return;
            }
            crate::hooks::HookResult::Continue((hook_channel, hook_recipient, mut modified)) => {
                if hook_channel != outcome.msg.channel || hook_recipient != outcome.msg.reply_target
                {
                    tracing::warn!(
                        from_channel = %outcome.msg.channel,
                        from_recipient = %outcome.msg.reply_target,
                        to_channel = %hook_channel,
                        to_recipient = %hook_recipient,
                        "on_message_sending attempted to rewrite channel routing; only content mutation is applied"
                    );
                }

                let modified_len = modified.chars().count();
                if modified_len > CHANNEL_HOOK_MAX_OUTBOUND_CHARS {
                    tracing::warn!(
                        limit = CHANNEL_HOOK_MAX_OUTBOUND_CHARS,
                        attempted = modified_len,
                        "hook-modified outbound content exceeded limit; truncating"
                    );
                    modified = truncate_with_ellipsis(&modified, CHANNEL_HOOK_MAX_OUTBOUND_CHARS);
                }

                if modified != outbound_response {
                    tracing::info!(
                        channel = %outcome.msg.channel,
                        sender = %outcome.msg.sender,
                        before_len = outbound_response.chars().count(),
                        after_len = modified.chars().count(),
                        "outgoing message content modified by hook"
                    );
                }

                outbound_response = modified;
            }
        }
    }

    if let Some(request) = parse_pending_channel_approval_request(&outbound_response) {
        let operation_id = Uuid::new_v4().to_string();
        let approval = PendingChannelApproval {
            operation_id: operation_id.clone(),
            tool_name: request.tool_name.clone(),
            arguments: request.arguments.clone(),
            reason: request.reason.clone(),
            preview: request.preview.clone(),
            reply_target: outcome.msg.reply_target.clone(),
            thread_ts: approval_card_thread(&outcome.msg),
            user_message: outcome.msg.content.clone(),
            provider: outcome.route.provider.clone(),
            model: outcome.route.model.clone(),
            created_at: Instant::now(),
        };
        insert_pending_channel_approval(approval.clone());

        let history_response = format!(
            "[Pending approval]\nTool: {}\nReason: {}",
            approval.tool_name, approval.reason
        );
        append_sender_turn(
            outcome.ctx.as_ref(),
            &outcome.history_key,
            ChatMessage::assistant(&history_response),
        );

        println!(
            "  🤖 Reply ({}ms): approval requested for {}",
            outcome.started_at.elapsed().as_millis(),
            approval.tool_name
        );

        if let Some(channel) = outcome.target_channel.as_ref() {
            cancel_draft_if_present(
                Some(channel),
                &outcome.msg.reply_target,
                outcome.draft_message_id.as_deref(),
            )
            .await;
            if let Err(err) =
                send_pending_approval_message(channel.as_ref(), &outcome.msg, &approval).await
            {
                tracing::warn!("Failed to send pending approval card: {err}");
            }
        }
        return;
    }

    let sanitized_response =
        sanitize_channel_response(&outbound_response, outcome.ctx.tools_registry.as_ref());
    let delivered_response = if sanitized_response.is_empty()
        && !outbound_response.trim().is_empty()
    {
        "I encountered malformed tool-call output and could not produce a safe reply. Please try again.".to_string()
    } else {
        sanitized_response
    };
    runtime_trace::record_event(
        "channel_message_outbound",
        Some(outcome.msg.channel.as_str()),
        Some(outcome.route.provider.as_str()),
        Some(outcome.route.model.as_str()),
        None,
        Some(true),
        None,
        serde_json::json!({
            "sender": outcome.msg.sender,
            "elapsed_ms": outcome.started_at.elapsed().as_millis(),
            "response": scrub_credentials(&delivered_response),
        }),
    );

    let tool_summary =
        extract_tool_context_summary(&outcome.history, outcome.history_len_before_tools);
    let history_response = if tool_summary.is_empty() || outcome.msg.channel == "telegram" {
        delivered_response.clone()
    } else {
        format!("{tool_summary}\n{delivered_response}")
    };

    append_sender_turn(
        outcome.ctx.as_ref(),
        &outcome.history_key,
        ChatMessage::assistant(&history_response),
    );
    println!(
        "  🤖 Reply ({}ms): {}",
        outcome.started_at.elapsed().as_millis(),
        truncate_with_ellipsis(&delivered_response, 80)
    );

    if let Some(channel) = outcome.target_channel.as_ref() {
        if let Some(draft_id) = outcome.draft_message_id.as_deref() {
            if let Err(error) = channel
                .finalize_draft(&outcome.msg.reply_target, draft_id, &delivered_response)
                .await
            {
                tracing::warn!("Failed to finalize draft: {error}; sending as new message");
                let _ = channel
                    .send(
                        &SendMessage::new(&delivered_response, &outcome.msg.reply_target)
                            .in_thread(outcome.msg.thread_ts.clone()),
                    )
                    .await;
            }
        } else if let Err(error) = channel
            .send(
                &SendMessage::new(delivered_response, &outcome.msg.reply_target)
                    .in_thread(outcome.msg.thread_ts.clone()),
            )
            .await
        {
            eprintln!("  ❌ Failed to reply on {}: {error}", channel.name());
        }
    }
}

async fn handle_tool_loop_error(outcome: ProcessingOutcomeContext, error: anyhow::Error) {
    if crate::agent::loop_::is_tool_loop_cancelled(&error)
        || outcome.cancellation_token.is_cancelled()
    {
        tracing::info!(
            channel = %outcome.msg.channel,
            sender = %outcome.msg.sender,
            "Cancelled in-flight channel request due to newer message"
        );
        runtime_trace::record_event(
            "channel_message_cancelled",
            Some(outcome.msg.channel.as_str()),
            Some(outcome.route.provider.as_str()),
            Some(outcome.route.model.as_str()),
            None,
            Some(false),
            Some("cancelled during tool-call loop"),
            serde_json::json!({
                "sender": outcome.msg.sender,
                "elapsed_ms": outcome.started_at.elapsed().as_millis(),
            }),
        );
        cancel_draft_if_present(
            outcome.target_channel.as_ref(),
            &outcome.msg.reply_target,
            outcome.draft_message_id.as_deref(),
        )
        .await;
        return;
    }

    if is_context_window_overflow_error(&error) {
        let compacted = compact_sender_history(outcome.ctx.as_ref(), &outcome.history_key);
        let error_text = if compacted {
            "⚠️ Context window exceeded for this conversation. I compacted recent history and kept the latest context. Please resend your last message."
        } else {
            "⚠️ Context window exceeded for this conversation. Please resend your last message."
        };
        eprintln!(
            "  ⚠️ Context window exceeded after {}ms; sender history compacted={}",
            outcome.started_at.elapsed().as_millis(),
            compacted
        );
        runtime_trace::record_event(
            "channel_message_error",
            Some(outcome.msg.channel.as_str()),
            Some(outcome.route.provider.as_str()),
            Some(outcome.route.model.as_str()),
            None,
            Some(false),
            Some("context window exceeded"),
            serde_json::json!({
                "sender": outcome.msg.sender,
                "elapsed_ms": outcome.started_at.elapsed().as_millis(),
                "history_compacted": compacted,
            }),
        );
        deliver_error_text(
            outcome.target_channel.as_ref(),
            &outcome.msg,
            outcome.draft_message_id.as_deref(),
            error_text,
        )
        .await;
        return;
    }

    eprintln!(
        "  ❌ LLM error after {}ms: {error}",
        outcome.started_at.elapsed().as_millis()
    );
    let safe_error = providers::sanitize_api_error(&error.to_string());
    runtime_trace::record_event(
        "channel_message_error",
        Some(outcome.msg.channel.as_str()),
        Some(outcome.route.provider.as_str()),
        Some(outcome.route.model.as_str()),
        None,
        Some(false),
        Some(&safe_error),
        serde_json::json!({
            "sender": outcome.msg.sender,
            "elapsed_ms": outcome.started_at.elapsed().as_millis(),
        }),
    );
    let should_rollback_user_turn = error
        .downcast_ref::<providers::ProviderCapabilityError>()
        .is_some_and(|capability| capability.capability.eq_ignore_ascii_case("vision"));
    let rolled_back = should_rollback_user_turn
        && rollback_orphan_user_turn(
            outcome.ctx.as_ref(),
            &outcome.history_key,
            &outcome.msg.content,
        );

    if !rolled_back {
        append_sender_turn(
            outcome.ctx.as_ref(),
            &outcome.history_key,
            ChatMessage::assistant("[Task failed — not continuing this request]"),
        );
    }
    let error_text = format!("⚠️ Error: {error}");
    deliver_error_text(
        outcome.target_channel.as_ref(),
        &outcome.msg,
        outcome.draft_message_id.as_deref(),
        &error_text,
    )
    .await;
}

async fn handle_timeout(outcome: ProcessingOutcomeContext) {
    let timeout_msg = format!(
        "LLM response timed out after {}s (base={}s, max_tool_iterations={})",
        outcome.timeout_budget_secs,
        outcome.ctx.message_timeout_secs,
        outcome.ctx.max_tool_iterations
    );
    runtime_trace::record_event(
        "channel_message_timeout",
        Some(outcome.msg.channel.as_str()),
        Some(outcome.route.provider.as_str()),
        Some(outcome.route.model.as_str()),
        None,
        Some(false),
        Some(&timeout_msg),
        serde_json::json!({
            "sender": outcome.msg.sender,
            "elapsed_ms": outcome.started_at.elapsed().as_millis(),
        }),
    );
    eprintln!(
        "  ❌ {} (elapsed: {}ms)",
        timeout_msg,
        outcome.started_at.elapsed().as_millis()
    );
    append_sender_turn(
        outcome.ctx.as_ref(),
        &outcome.history_key,
        ChatMessage::assistant("[Task timed out — not continuing this request]"),
    );
    let error_text = "⚠️ Request timed out while waiting for the model. Please try again.";
    deliver_error_text(
        outcome.target_channel.as_ref(),
        &outcome.msg,
        outcome.draft_message_id.as_deref(),
        error_text,
    )
    .await;
}

async fn cancel_draft_if_present(
    channel: Option<&Arc<dyn Channel>>,
    reply_target: &str,
    draft_message_id: Option<&str>,
) {
    if let (Some(channel), Some(draft_id)) = (channel, draft_message_id) {
        if let Err(error) = channel.cancel_draft(reply_target, draft_id).await {
            tracing::debug!("Failed to cancel draft on {}: {error}", channel.name());
        }
    }
}

async fn deliver_error_text(
    channel: Option<&Arc<dyn Channel>>,
    msg: &traits::ChannelMessage,
    draft_message_id: Option<&str>,
    text: &str,
) {
    let Some(channel) = channel else {
        return;
    };

    if let Some(draft_id) = draft_message_id {
        let _ = channel
            .finalize_draft(&msg.reply_target, draft_id, text)
            .await;
    } else {
        let _ = channel
            .send(&SendMessage::new(text, &msg.reply_target).in_thread(msg.thread_ts.clone()))
            .await;
    }
}
