use super::super::*;
use super::config::{maybe_apply_runtime_config_update, runtime_defaults_snapshot};
use super::keys::{
    conversation_history_key, conversation_memory_key, final_reply_thread_ts_after_tool_updates,
    inbound_display_sender, inbound_user_message_body, outbound_thread_ts,
    should_forward_tool_events_as_thread_messages,
};
use super::notify::ChannelNotifyObserver;
use super::processing_approval::handle_pending_approval_action;
use super::processing_result::{
    handle_llm_execution_result, LlmExecutionResult, ProcessingOutcomeContext,
};
use super::processing_support::{log_worker_join_result, spawn_scoped_typing_task};
use super::prompt::{build_channel_system_prompt, normalize_cached_channel_turns};
use super::routing::{
    append_sender_turn, build_memory_context, get_or_create_provider, get_route_selection,
    handle_runtime_command_if_needed,
};
use crate::agent::loop_::run_tool_call_loop;
use crate::observability::runtime_trace;
use crate::util::truncate_with_ellipsis;
use serde_json::json;

struct ProcessingSupportHandles {
    draft_message_id: Option<String>,
    draft_updater: Option<tokio::task::JoinHandle<()>>,
    typing_cancellation: Option<CancellationToken>,
    typing_task: Option<tokio::task::JoinHandle<()>>,
    notify_observer: Option<Arc<ChannelNotifyObserver>>,
    notify_task: Option<tokio::task::JoinHandle<()>>,
}

pub(crate) async fn process_channel_message(
    ctx: Arc<ChannelRuntimeContext>,
    msg: traits::ChannelMessage,
    cancellation_token: CancellationToken,
) {
    if cancellation_token.is_cancelled() {
        return;
    }

    let Some((mut msg, target_channel)) =
        prepare_inbound_message(Arc::clone(&ctx), msg, cancellation_token.clone()).await
    else {
        return;
    };

    let history_key = conversation_history_key(&msg);
    let route = get_route_selection(ctx.as_ref(), &history_key);
    let runtime_defaults = runtime_defaults_snapshot(ctx.as_ref());
    let active_provider = match get_or_create_provider(ctx.as_ref(), &route.provider).await {
        Ok(provider) => provider,
        Err(err) => {
            send_provider_init_error(target_channel.as_ref(), &msg, &route.provider, &err).await;
            return;
        }
    };

    auto_save_inbound_memory(ctx.as_ref(), &msg).await;

    println!("  ⏳ Processing message...");
    let started_at = Instant::now();
    let mut history = build_message_history(ctx.as_ref(), &msg, &history_key).await;
    let supports = setup_processing_supports(
        Arc::clone(&ctx),
        target_channel.clone(),
        &msg,
        &route,
        &runtime_defaults,
        &mut history,
        active_provider,
        cancellation_token.clone(),
    )
    .await;

    let reaction_done_emoji = match &supports.0 {
        LlmExecutionResult::Completed(Ok(Ok(_))) => "\u{2705}",
        _ => "\u{26A0}\u{FE0F}",
    };

    let (llm_result, history_len_before_tools, timeout_budget_secs, mut support_handles) = supports;
    let tools_used = support_handles
        .notify_observer
        .as_ref()
        .is_some_and(|observer| observer.tools_used.load(Ordering::Relaxed));
    finalize_processing_supports(&mut msg, &mut support_handles, tools_used).await;

    handle_llm_execution_result(
        ProcessingOutcomeContext {
            ctx,
            msg: msg.clone(),
            target_channel: target_channel.clone(),
            draft_message_id: support_handles.draft_message_id.clone(),
            started_at,
            history_key,
            history_len_before_tools,
            history,
            route,
            runtime_defaults,
            cancellation_token,
            timeout_budget_secs,
        },
        llm_result,
    )
    .await;

    complete_message_reaction(target_channel.as_ref(), &msg, reaction_done_emoji).await;
}

async fn prepare_inbound_message(
    ctx: Arc<ChannelRuntimeContext>,
    msg: traits::ChannelMessage,
    cancellation_token: CancellationToken,
) -> Option<(traits::ChannelMessage, Option<Arc<dyn Channel>>)> {
    if cancellation_token.is_cancelled() {
        return None;
    }

    let display_sender = inbound_display_sender(&msg);
    println!(
        "  💬 [{}] from {}: {}",
        msg.channel,
        display_sender,
        truncate_with_ellipsis(&msg.content, 80)
    );
    runtime_trace::record_event(
        "channel_message_inbound",
        Some(msg.channel.as_str()),
        None,
        None,
        None,
        None,
        None,
        serde_json::json!({
            "sender": msg.sender,
            "display_sender": display_sender,
            "message_id": msg.id,
            "reply_target": msg.reply_target,
            "context": msg.context,
            "content_preview": truncate_with_ellipsis(&msg.content, 160),
        }),
    );

    let msg = if let Some(hooks) = &ctx.hooks {
        match hooks.run_on_message_received(msg).await {
            crate::hooks::HookResult::Cancel(reason) => {
                tracing::info!(%reason, "incoming message dropped by hook");
                return None;
            }
            crate::hooks::HookResult::Continue(modified) => modified,
        }
    } else {
        msg
    };

    let target_channel = ctx.channels_by_name.get(&msg.channel).cloned();
    if handle_pending_approval_action(ctx.as_ref(), &msg, target_channel.as_ref()).await {
        return None;
    }
    if let Err(err) = maybe_apply_runtime_config_update(ctx.as_ref()).await {
        tracing::warn!("Failed to apply runtime config update: {err}");
    }
    if handle_runtime_command_if_needed(ctx.as_ref(), &msg, target_channel.as_ref()).await {
        return None;
    }

    Some((msg, target_channel))
}

async fn auto_save_inbound_memory(ctx: &ChannelRuntimeContext, msg: &traits::ChannelMessage) {
    let inbound_body = inbound_user_message_body(msg);
    if ctx.auto_save_memory && inbound_body.chars().count() >= AUTOSAVE_MIN_MESSAGE_CHARS {
        let autosave_key = conversation_memory_key(msg);
        let _ = ctx
            .memory
            .store(
                &autosave_key,
                inbound_body.as_ref(),
                crate::memory::MemoryCategory::Conversation,
                None,
            )
            .await;
    }
}

async fn build_message_history(
    ctx: &ChannelRuntimeContext,
    msg: &traits::ChannelMessage,
    history_key: &str,
) -> Vec<ChatMessage> {
    let inbound_body = inbound_user_message_body(msg);
    let had_prior_history = ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(history_key)
        .is_some_and(|turns| !turns.is_empty());

    append_sender_turn(ctx, history_key, ChatMessage::user(inbound_body.as_ref()));

    let prior_turns_raw = ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(history_key)
        .cloned()
        .unwrap_or_default();
    let mut prior_turns = normalize_cached_channel_turns(prior_turns_raw);

    if !had_prior_history {
        let memory_context =
            build_memory_context(ctx.memory.as_ref(), &msg.content, ctx.min_relevance_score).await;
        if let Some(last_turn) = prior_turns.last_mut() {
            if last_turn.role == "user" && !memory_context.is_empty() {
                last_turn.content = format!("{memory_context}{}", inbound_body.as_ref());
            }
        }
    }

    let system_prompt =
        build_channel_system_prompt(ctx.system_prompt.as_str(), &msg.channel, &msg.reply_target);
    let mut history = vec![ChatMessage::system(system_prompt)];
    history.extend(prior_turns);
    history
}

#[allow(clippy::too_many_arguments)]
async fn setup_processing_supports(
    ctx: Arc<ChannelRuntimeContext>,
    target_channel: Option<Arc<dyn Channel>>,
    msg: &traits::ChannelMessage,
    route: &ChannelRouteSelection,
    runtime_defaults: &ChannelRuntimeDefaults,
    history: &mut Vec<ChatMessage>,
    active_provider: Arc<dyn Provider>,
    cancellation_token: CancellationToken,
) -> (LlmExecutionResult, usize, u64, ProcessingSupportHandles) {
    let use_streaming = target_channel
        .as_ref()
        .is_some_and(|channel| channel.supports_draft_updates());
    tracing::debug!(
        channel = %msg.channel,
        has_target_channel = target_channel.is_some(),
        use_streaming,
        supports_draft = target_channel.as_ref().map_or(false, |channel| channel.supports_draft_updates()),
        "Draft streaming decision"
    );

    let (delta_tx, delta_rx) = if use_streaming {
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let draft_message_id = create_draft_message(target_channel.as_ref(), msg, use_streaming).await;
    let draft_updater = spawn_draft_updater(
        delta_rx,
        draft_message_id.as_deref(),
        target_channel.as_ref(),
        msg,
    );

    acknowledge_message(target_channel.as_ref(), msg).await;

    let typing_cancellation = target_channel.as_ref().map(|_| CancellationToken::new());
    let typing_task = match (target_channel.as_ref(), typing_cancellation.as_ref()) {
        (Some(channel), Some(token)) => Some(spawn_scoped_typing_task(
            Arc::clone(channel),
            msg.reply_target.clone(),
            token.clone(),
        )),
        _ => None,
    };

    let (notify_observer, notify_task) =
        create_notify_runtime(Arc::clone(&ctx.observer), target_channel.clone(), msg);

    let history_len_before_tools = history.len();
    let timeout_budget_secs =
        channel_message_timeout_budget_secs(ctx.message_timeout_secs, ctx.max_tool_iterations);
    let llm_result = tokio::select! {
        () = cancellation_token.cancelled() => LlmExecutionResult::Cancelled,
        result = tokio::time::timeout(
            Duration::from_secs(timeout_budget_secs),
            run_tool_call_loop(
                active_provider.as_ref(),
                history,
                ctx.tools_registry.as_ref(),
                notify_observer.as_ref() as &dyn Observer,
                route.provider.as_str(),
                route.model.as_str(),
                runtime_defaults.temperature,
                true,
                None,
                msg.channel.as_str(),
                &ctx.multimodal,
                ctx.max_tool_iterations,
                Some(cancellation_token.clone()),
                delta_tx,
                ctx.hooks.as_deref(),
                if msg.channel == "cli" {
                    &[] as &[String]
                } else {
                    ctx.non_cli_excluded_tools.as_ref()
                },
                channel_tool_execution_context(&msg),
            ),
        ) => LlmExecutionResult::Completed(result),
    };

    (
        llm_result,
        history_len_before_tools,
        timeout_budget_secs,
        ProcessingSupportHandles {
            draft_message_id,
            draft_updater,
            typing_cancellation,
            typing_task,
            notify_observer: Some(notify_observer),
            notify_task,
        },
    )
}

fn channel_tool_execution_context(msg: &traits::ChannelMessage) -> Option<serde_json::Value> {
    let current_channel_id = msg
        .context
        .as_ref()
        .and_then(|context| context.origin_to.clone())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| (!msg.reply_target.trim().is_empty()).then(|| msg.reply_target.clone()));
    let current_message_id = (!msg.id.trim().is_empty()).then(|| msg.id.clone());
    let current_thread_ts = msg
        .context
        .as_ref()
        .and_then(|context| context.thread_id.clone())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| msg.thread_ts.clone());

    if current_channel_id.is_none() && current_message_id.is_none() && current_thread_ts.is_none() {
        return None;
    }

    Some(json!({
        "current_channel_name": msg.channel,
        "current_channel_id": current_channel_id,
        "current_message_id": current_message_id,
        "current_thread_ts": current_thread_ts,
    }))
}

async fn create_draft_message(
    target_channel: Option<&Arc<dyn Channel>>,
    msg: &traits::ChannelMessage,
    use_streaming: bool,
) -> Option<String> {
    if !use_streaming {
        return None;
    }

    let channel = target_channel?;
    match channel
        .send_draft(
            &SendMessage::new("...", &msg.reply_target)
                .in_thread(outbound_thread_ts(&msg.channel, msg.thread_ts.clone())),
        )
        .await
    {
        Ok(id) => id,
        Err(error) => {
            tracing::debug!("Failed to send draft on {}: {error}", channel.name());
            None
        }
    }
}

fn spawn_draft_updater(
    delta_rx: Option<tokio::sync::mpsc::Receiver<String>>,
    draft_message_id: Option<&str>,
    target_channel: Option<&Arc<dyn Channel>>,
    msg: &traits::ChannelMessage,
) -> Option<tokio::task::JoinHandle<()>> {
    let (Some(mut rx), Some(draft_id), Some(channel)) =
        (delta_rx, draft_message_id, target_channel)
    else {
        return None;
    };
    let channel = Arc::clone(channel);
    let reply_target = msg.reply_target.clone();
    let draft_id = draft_id.to_string();
    Some(tokio::spawn(async move {
        let mut accumulated = String::new();
        while let Some(delta) = rx.recv().await {
            if delta == crate::agent::loop_::DRAFT_CLEAR_SENTINEL {
                accumulated.clear();
                continue;
            }
            accumulated.push_str(&delta);
            if let Err(error) = channel
                .update_draft(&reply_target, &draft_id, &accumulated)
                .await
            {
                tracing::debug!("Draft update failed: {error}");
            }
        }
    }))
}

async fn acknowledge_message(
    target_channel: Option<&Arc<dyn Channel>>,
    msg: &traits::ChannelMessage,
) {
    if let Some(channel) = target_channel {
        if let Err(error) = channel
            .add_reaction(&msg.reply_target, &msg.id, "\u{1F440}")
            .await
        {
            tracing::debug!("Failed to add reaction: {error}");
        }
    }
}

fn create_notify_runtime(
    observer: Arc<dyn Observer>,
    target_channel: Option<Arc<dyn Channel>>,
    msg: &traits::ChannelMessage,
) -> (
    Arc<ChannelNotifyObserver>,
    Option<tokio::task::JoinHandle<()>>,
) {
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let notify_observer: Arc<ChannelNotifyObserver> = Arc::new(ChannelNotifyObserver {
        inner: observer,
        tx: notify_tx,
        tools_used: AtomicBool::new(false),
    });
    let notify_channel = target_channel;
    let notify_reply_target = msg.reply_target.clone();
    let notify_thread_root = msg.id.clone();
    let notify_task = if !should_forward_tool_events_as_thread_messages(&msg.channel) {
        Some(tokio::spawn(async move {
            while notify_rx.recv().await.is_some() {}
        }))
    } else {
        Some(tokio::spawn(async move {
            let thread_ts = Some(notify_thread_root);
            while let Some(text) = notify_rx.recv().await {
                if let Some(ref channel) = notify_channel {
                    let _ = channel
                        .send(
                            &SendMessage::new(&text, &notify_reply_target)
                                .in_thread(thread_ts.clone()),
                        )
                        .await;
                }
            }
        }))
    };

    (notify_observer, notify_task)
}

async fn finalize_processing_supports(
    msg: &mut traits::ChannelMessage,
    support_handles: &mut ProcessingSupportHandles,
    tools_used: bool,
) {
    if let Some(handle) = support_handles.draft_updater.take() {
        let _ = handle.await;
    }

    msg.thread_ts = final_reply_thread_ts_after_tool_updates(msg, tools_used);
    let notify_observer = support_handles.notify_observer.take();
    drop(notify_observer);
    if let Some(handle) = support_handles.notify_task.take() {
        let _ = handle.await;
    }

    if let Some(token) = support_handles.typing_cancellation.as_ref() {
        token.cancel();
    }
    if let Some(handle) = support_handles.typing_task.take() {
        log_worker_join_result(handle.await);
    }
}

async fn complete_message_reaction(
    target_channel: Option<&Arc<dyn Channel>>,
    msg: &traits::ChannelMessage,
    reaction_done_emoji: &str,
) {
    if let Some(channel) = target_channel {
        let _ = channel
            .remove_reaction(&msg.reply_target, &msg.id, "\u{1F440}")
            .await;
        let _ = channel
            .add_reaction(&msg.reply_target, &msg.id, reaction_done_emoji)
            .await;
    }
}

async fn send_provider_init_error(
    target_channel: Option<&Arc<dyn Channel>>,
    msg: &traits::ChannelMessage,
    provider_name: &str,
    error: &anyhow::Error,
) {
    let safe_error = providers::sanitize_api_error(&error.to_string());
    let message = format!(
        "⚠️ Failed to initialize provider `{provider_name}`. Please run `/models` to choose another provider.\nDetails: {safe_error}",
    );
    if let Some(channel) = target_channel {
        let _ = channel
            .send(
                &SendMessage::new(message, &msg.reply_target)
                    .in_thread(outbound_thread_ts(&msg.channel, msg.thread_ts.clone())),
            )
            .await;
    }
}
