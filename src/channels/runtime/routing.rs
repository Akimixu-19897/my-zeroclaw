use super::super::*;
use super::config::runtime_defaults_snapshot;
use super::keys::conversation_history_key;
use super::prompt::{
    normalize_cached_channel_turns, parse_runtime_command, resolve_provider_alias,
};
use crate::util::truncate_with_ellipsis;

pub(crate) fn default_route_selection(ctx: &ChannelRuntimeContext) -> ChannelRouteSelection {
    let defaults = runtime_defaults_snapshot(ctx);
    ChannelRouteSelection {
        provider: defaults.default_provider,
        model: defaults.model,
    }
}

pub(crate) fn get_route_selection(
    ctx: &ChannelRuntimeContext,
    sender_key: &str,
) -> ChannelRouteSelection {
    ctx.route_overrides
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(sender_key)
        .cloned()
        .unwrap_or_else(|| default_route_selection(ctx))
}

pub(crate) fn set_route_selection(
    ctx: &ChannelRuntimeContext,
    sender_key: &str,
    next: ChannelRouteSelection,
) {
    let default_route = default_route_selection(ctx);
    let mut routes = ctx
        .route_overrides
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if next == default_route {
        routes.remove(sender_key);
    } else {
        routes.insert(sender_key.to_string(), next);
    }
}

pub(crate) fn clear_sender_history(ctx: &ChannelRuntimeContext, sender_key: &str) {
    ctx.conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(sender_key);
}

pub(crate) fn compact_sender_history(ctx: &ChannelRuntimeContext, sender_key: &str) -> bool {
    let mut histories = ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let Some(turns) = histories.get_mut(sender_key) else {
        return false;
    };

    if turns.is_empty() {
        return false;
    }

    let keep_from = turns
        .len()
        .saturating_sub(CHANNEL_HISTORY_COMPACT_KEEP_MESSAGES);
    let mut compacted = normalize_cached_channel_turns(turns[keep_from..].to_vec());

    for turn in &mut compacted {
        if turn.content.chars().count() > CHANNEL_HISTORY_COMPACT_CONTENT_CHARS {
            turn.content =
                truncate_with_ellipsis(&turn.content, CHANNEL_HISTORY_COMPACT_CONTENT_CHARS);
        }
    }

    if compacted.is_empty() {
        turns.clear();
        return false;
    }

    *turns = compacted;
    true
}

pub(crate) fn append_sender_turn(ctx: &ChannelRuntimeContext, sender_key: &str, turn: ChatMessage) {
    let mut histories = ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let turns = histories.entry(sender_key.to_string()).or_default();
    turns.push(turn);
    while turns.len() > MAX_CHANNEL_HISTORY {
        turns.remove(0);
    }
}

pub(crate) fn rollback_orphan_user_turn(
    ctx: &ChannelRuntimeContext,
    sender_key: &str,
    expected_content: &str,
) -> bool {
    let mut histories = ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(turns) = histories.get_mut(sender_key) else {
        return false;
    };

    let should_pop = turns
        .last()
        .is_some_and(|turn| turn.role == "user" && turn.content == expected_content);
    if !should_pop {
        return false;
    }

    turns.pop();
    if turns.is_empty() {
        histories.remove(sender_key);
    }
    true
}

pub(crate) fn should_skip_memory_context_entry(key: &str, content: &str) -> bool {
    if memory::is_assistant_autosave_key(key) {
        return true;
    }

    if key.trim().to_ascii_lowercase().ends_with("_history") {
        return true;
    }

    content.chars().count() > MEMORY_CONTEXT_MAX_CHARS
}

pub(crate) fn is_context_window_overflow_error(err: &anyhow::Error) -> bool {
    let lower = err.to_string().to_lowercase();
    [
        "exceeds the context window",
        "context window of this model",
        "maximum context length",
        "context length exceeded",
        "too many tokens",
        "token limit exceeded",
        "prompt is too long",
        "input is too long",
    ]
    .iter()
    .any(|hint| lower.contains(hint))
}

pub(crate) fn load_cached_model_preview(workspace_dir: &Path, provider_name: &str) -> Vec<String> {
    let cache_path = workspace_dir.join("state").join(MODEL_CACHE_FILE);
    let Ok(raw) = std::fs::read_to_string(cache_path) else {
        return Vec::new();
    };
    let Ok(state) = serde_json::from_str::<ModelCacheState>(&raw) else {
        return Vec::new();
    };

    state
        .entries
        .into_iter()
        .find(|entry| entry.provider == provider_name)
        .map(|entry| {
            entry
                .models
                .into_iter()
                .take(MODEL_CACHE_PREVIEW_LIMIT)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(crate) async fn get_or_create_provider(
    ctx: &ChannelRuntimeContext,
    provider_name: &str,
) -> anyhow::Result<Arc<dyn Provider>> {
    if let Some(existing) = ctx
        .provider_cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(provider_name)
        .cloned()
    {
        return Ok(existing);
    }

    if provider_name == ctx.default_provider.as_str() {
        return Ok(Arc::clone(&ctx.provider));
    }

    let defaults = runtime_defaults_snapshot(ctx);
    let api_url = if provider_name == defaults.default_provider.as_str() {
        defaults.api_url.as_deref()
    } else {
        None
    };

    let provider = create_resilient_provider_nonblocking(
        provider_name,
        ctx.api_key.clone(),
        api_url.map(ToString::to_string),
        ctx.reliability.as_ref().clone(),
        ctx.provider_runtime_options.clone(),
    )
    .await?;
    let provider: Arc<dyn Provider> = Arc::from(provider);

    if let Err(err) = provider.warmup().await {
        tracing::warn!(provider = provider_name, "Provider warmup failed: {err}");
    }

    let mut cache = ctx.provider_cache.lock().unwrap_or_else(|e| e.into_inner());
    let cached = cache
        .entry(provider_name.to_string())
        .or_insert_with(|| Arc::clone(&provider));
    Ok(Arc::clone(cached))
}

pub(crate) async fn create_resilient_provider_nonblocking(
    provider_name: &str,
    api_key: Option<String>,
    api_url: Option<String>,
    reliability: crate::config::ReliabilityConfig,
    provider_runtime_options: providers::ProviderRuntimeOptions,
) -> anyhow::Result<Box<dyn Provider>> {
    let provider_name = provider_name.to_string();
    tokio::task::spawn_blocking(move || {
        providers::create_resilient_provider_with_options(
            &provider_name,
            api_key.as_deref(),
            api_url.as_deref(),
            &reliability,
            &provider_runtime_options,
        )
    })
    .await
    .context("failed to join provider initialization task")?
}

pub(crate) fn build_models_help_response(
    current: &ChannelRouteSelection,
    workspace_dir: &Path,
    model_routes: &[crate::config::ModelRouteConfig],
) -> String {
    let mut response = String::new();
    let _ = writeln!(
        response,
        "Current provider: `{}`\nCurrent model: `{}`",
        current.provider, current.model
    );
    response.push_str("\nSwitch model with `/model <model-id>` or `/model <hint>`.\n");

    if !model_routes.is_empty() {
        response.push_str("\nConfigured model routes:\n");
        for route in model_routes {
            let _ = writeln!(
                response,
                "  `{}` → {} ({})",
                route.hint, route.model, route.provider
            );
        }
    }

    let cached_models = load_cached_model_preview(workspace_dir, &current.provider);
    if cached_models.is_empty() {
        let _ = writeln!(
            response,
            "\nNo cached model list found for `{}`. Ask the operator to run `zeroclaw models refresh --provider {}`.",
            current.provider, current.provider
        );
    } else {
        let _ = writeln!(
            response,
            "\nCached model IDs (top {}):",
            cached_models.len()
        );
        for model in cached_models {
            let _ = writeln!(response, "- `{model}`");
        }
    }

    response
}

pub(crate) fn build_providers_help_response(current: &ChannelRouteSelection) -> String {
    let mut response = String::new();
    let _ = writeln!(
        response,
        "Current provider: `{}`\nCurrent model: `{}`",
        current.provider, current.model
    );
    response.push_str("\nSwitch provider with `/models <provider>`.\n");
    response.push_str("Switch model with `/model <model-id>`.\n\n");
    response.push_str("Available providers:\n");
    for provider in providers::list_providers() {
        if provider.aliases.is_empty() {
            let _ = writeln!(response, "- {}", provider.name);
        } else {
            let _ = writeln!(
                response,
                "- {} (aliases: {})",
                provider.name,
                provider.aliases.join(", ")
            );
        }
    }
    response
}

pub(crate) async fn handle_runtime_command_if_needed(
    ctx: &ChannelRuntimeContext,
    msg: &traits::ChannelMessage,
    target_channel: Option<&Arc<dyn Channel>>,
) -> bool {
    let Some(command) = parse_runtime_command(&msg.channel, &msg.content) else {
        return false;
    };

    let Some(channel) = target_channel else {
        return true;
    };

    let sender_key = conversation_history_key(msg);
    let mut current = get_route_selection(ctx, &sender_key);

    let response = match command {
        ChannelRuntimeCommand::ShowProviders => build_providers_help_response(&current),
        ChannelRuntimeCommand::SetProvider(raw_provider) => {
            match resolve_provider_alias(&raw_provider) {
                Some(provider_name) => match get_or_create_provider(ctx, &provider_name).await {
                    Ok(_) => {
                        if provider_name != current.provider {
                            current.provider = provider_name.clone();
                            set_route_selection(ctx, &sender_key, current.clone());
                        }

                        format!(
                            "Provider switched to `{provider_name}` for this sender session. Current model is `{}`.\nUse `/model <model-id>` to set a provider-compatible model.",
                            current.model
                        )
                    }
                    Err(err) => {
                        let safe_err = providers::sanitize_api_error(&err.to_string());
                        format!(
                            "Failed to initialize provider `{provider_name}`. Route unchanged.\nDetails: {safe_err}"
                        )
                    }
                },
                None => format!(
                    "Unknown provider `{raw_provider}`. Use `/models` to list valid providers."
                ),
            }
        }
        ChannelRuntimeCommand::ShowModel => {
            build_models_help_response(&current, ctx.workspace_dir.as_path(), &ctx.model_routes)
        }
        ChannelRuntimeCommand::SetModel(raw_model) => {
            let model = raw_model.trim().trim_matches('`').to_string();
            if model.is_empty() {
                "Model ID cannot be empty. Use `/model <model-id>`.".to_string()
            } else {
                // Resolve provider+model from model_routes (match by model name or hint)
                if let Some(route) = ctx.model_routes.iter().find(|r| {
                    r.model.eq_ignore_ascii_case(&model) || r.hint.eq_ignore_ascii_case(&model)
                }) {
                    current.provider = route.provider.clone();
                    current.model = route.model.clone();
                } else {
                    current.model = model.clone();
                }
                set_route_selection(ctx, &sender_key, current.clone());

                format!(
                    "Model switched to `{}` (provider: `{}`). Context preserved.",
                    current.model, current.provider
                )
            }
        }
        ChannelRuntimeCommand::NewSession => {
            clear_sender_history(ctx, &sender_key);
            "Conversation history cleared. Starting fresh.".to_string()
        }
    };

    if let Err(err) = channel
        .send(&SendMessage::new(response, &msg.reply_target).in_thread(msg.thread_ts.clone()))
        .await
    {
        tracing::warn!(
            "Failed to send runtime command response on {}: {err}",
            channel.name()
        );
    }

    true
}

pub(crate) async fn build_memory_context(
    mem: &dyn Memory,
    user_msg: &str,
    min_relevance_score: f64,
) -> String {
    let mut context = String::new();

    if let Ok(entries) = mem.recall(user_msg, 5, None).await {
        let mut included = 0usize;
        let mut used_chars = 0usize;

        for entry in entries.iter().filter(|e| match e.score {
            Some(score) => score >= min_relevance_score,
            None => true,
        }) {
            if included >= MEMORY_CONTEXT_MAX_ENTRIES {
                break;
            }

            if should_skip_memory_context_entry(&entry.key, &entry.content) {
                continue;
            }

            let content = if entry.content.chars().count() > MEMORY_CONTEXT_ENTRY_MAX_CHARS {
                truncate_with_ellipsis(&entry.content, MEMORY_CONTEXT_ENTRY_MAX_CHARS)
            } else {
                entry.content.clone()
            };

            let line = format!("- {}: {}\n", entry.key, content);
            let line_chars = line.chars().count();
            if used_chars + line_chars > MEMORY_CONTEXT_MAX_CHARS {
                break;
            }

            if included == 0 {
                context.push_str("[Memory context]\n");
            }

            context.push_str(&line);
            used_chars += line_chars;
            included += 1;
        }

        if included > 0 {
            context.push('\n');
        }
    }

    context
}
