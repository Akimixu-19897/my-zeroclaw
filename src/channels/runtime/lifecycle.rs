use super::super::*;
use super::dispatch::run_message_dispatch_loop;
use super::processing_support::{compute_max_in_flight_messages, spawn_supervised_listener};
use super::registry::{collect_configured_channels, ConfiguredChannel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelHealthState {
    Healthy,
    Unhealthy,
    Timeout,
}

pub(crate) fn channel_health_detail_suffix(component_name: &str) -> String {
    let snapshot = crate::health::snapshot_json();
    let Some(component) = snapshot
        .get("components")
        .and_then(serde_json::Value::as_object)
        .and_then(|components| components.get(component_name))
    else {
        return String::new();
    };

    let Some(details) = component.get("details") else {
        return String::new();
    };

    let probe_kind = details
        .get("probe_kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    if probe_kind != "lark_channel" {
        return String::new();
    }

    let account = details
        .get("account_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("default");
    let token = details
        .get("token_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let transport = details
        .get("transport_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let bot = details
        .get("bot_identity_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let receive_mode = details
        .get("receive_mode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");

    format!(
        " [account={account}, mode={receive_mode}, token={token}, transport={transport}, bot={bot}]"
    )
}

pub(crate) fn classify_health_result(
    result: &std::result::Result<bool, tokio::time::error::Elapsed>,
) -> ChannelHealthState {
    match result {
        Ok(true) => ChannelHealthState::Healthy,
        Ok(false) => ChannelHealthState::Unhealthy,
        Err(_) => ChannelHealthState::Timeout,
    }
}

pub async fn doctor_channels(config: Config) -> Result<()> {
    let mut channels = collect_configured_channels(&config, "health check");

    if let Some(ref ns) = config.channels_config.nostr {
        channels.push(ConfiguredChannel {
            display_name: "Nostr".to_string(),
            channel: Arc::new(
                NostrChannel::new(&ns.private_key, ns.relays.clone(), &ns.allowed_pubkeys).await?,
            ),
        });
    }

    if channels.is_empty() {
        println!("No real-time channels configured. Run `zeroclaw onboard` first.");
        return Ok(());
    }

    println!("🩺 ZeroClaw Channel Doctor");
    println!();

    let mut healthy = 0_u32;
    let mut unhealthy = 0_u32;
    let mut timeout = 0_u32;

    for configured in channels {
        let component_name = format!("channel:{}", configured.channel.name());
        let result =
            tokio::time::timeout(Duration::from_secs(10), configured.channel.health_check()).await;
        let state = classify_health_result(&result);
        let detail_suffix = channel_health_detail_suffix(&component_name);

        match state {
            ChannelHealthState::Healthy => {
                healthy += 1;
                println!(
                    "  ✅ {:<9} healthy{}",
                    configured.display_name, detail_suffix
                );
            }
            ChannelHealthState::Unhealthy => {
                unhealthy += 1;
                println!(
                    "  ❌ {:<9} unhealthy (auth/config/network){}",
                    configured.display_name, detail_suffix
                );
            }
            ChannelHealthState::Timeout => {
                timeout += 1;
                println!(
                    "  ⏱️  {:<9} timed out (>10s){}",
                    configured.display_name, detail_suffix
                );
            }
        }
    }

    if config.channels_config.webhook.is_some() {
        println!("  ℹ️  Webhook   check via `zeroclaw gateway` then GET /health");
    }

    println!();
    println!("Summary: {healthy} healthy, {unhealthy} unhealthy, {timeout} timed out");
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub async fn start_channels(config: Config) -> Result<()> {
    let provider_name = resolved_default_provider(&config);
    let provider_runtime_options = providers::ProviderRuntimeOptions {
        auth_profile_override: None,
        provider_api_url: config.api_url.clone(),
        zeroclaw_dir: config.config_path.parent().map(std::path::PathBuf::from),
        secrets_encrypt: config.secrets.encrypt,
        reasoning_enabled: config.runtime.reasoning_enabled,
    };
    let provider: Arc<dyn Provider> = Arc::from(
        create_resilient_provider_nonblocking(
            &provider_name,
            config.api_key.clone(),
            config.api_url.clone(),
            config.reliability.clone(),
            provider_runtime_options.clone(),
        )
        .await?,
    );

    if let Err(e) = provider.warmup().await {
        tracing::warn!("Provider warmup failed (non-fatal): {e}");
    }

    let initial_stamp = config_file_stamp(&config.config_path).await;
    {
        let mut store = runtime_config_store()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        store.insert(
            config.config_path.clone(),
            RuntimeConfigState {
                defaults: runtime_defaults_from_config(&config),
                last_applied_stamp: initial_stamp,
            },
        );
    }

    let observer: Arc<dyn Observer> =
        Arc::from(observability::create_observer(&config.observability));
    let runtime: Arc<dyn crate::runtime::RuntimeAdapter> =
        Arc::from(crate::runtime::create_runtime(&config.runtime)?);
    let security = Arc::new(SecurityPolicy::from_config(
        &config.autonomy,
        &config.workspace_dir,
    ));
    let model = resolved_default_model(&config);
    let temperature = config.default_temperature;
    let mem: Arc<dyn Memory> = Arc::from(memory::create_memory_with_storage_and_routes(
        &config.memory,
        &config.embedding_routes,
        Some(&config.storage.provider.config),
        &config.workspace_dir,
        config.api_key.as_deref(),
    )?);
    let (composio_key, composio_entity_id) = if config.composio.enabled {
        (
            config.composio.api_key.as_deref(),
            Some(config.composio.entity_id.as_str()),
        )
    } else {
        (None, None)
    };
    let workspace = config.workspace_dir.clone();
    let tools_registry = Arc::new(tools::all_tools_with_runtime(
        Arc::new(config.clone()),
        &security,
        runtime,
        Arc::clone(&mem),
        composio_key,
        composio_entity_id,
        &config.browser,
        &config.http_request,
        &config.web_fetch,
        &workspace,
        &config.agents,
        config.api_key.as_deref(),
        &config,
    ));

    let skills = crate::skills::load_skills_with_config(&workspace, &config);

    let mut tool_descs: Vec<(&str, &str)> = vec![
        (
            "shell",
            "Execute terminal commands. Use when: running local checks, build/test commands, diagnostics. Don't use when: a safer dedicated tool exists, or command is destructive without approval.",
        ),
        (
            "file_read",
            "Read file contents. Use when: inspecting project files, configs, logs. Don't use when: a targeted search is enough.",
        ),
        (
            "file_write",
            "Write file contents. Use when: applying focused edits, scaffolding files, updating docs/code. Don't use when: side effects are unclear or file ownership is uncertain.",
        ),
        (
            "memory_store",
            "Save to memory. Use when: preserving durable preferences, decisions, key context. Don't use when: information is transient/noisy/sensitive without need.",
        ),
        (
            "memory_recall",
            "Search memory. Use when: retrieving prior decisions, user preferences, historical context. Don't use when: answer is already in current context.",
        ),
        (
            "memory_forget",
            "Delete a memory entry. Use when: memory is incorrect/stale or explicitly requested for removal. Don't use when: impact is uncertain.",
        ),
    ];

    if config.browser.enabled {
        tool_descs.push((
            "browser_open",
            "Open approved HTTPS URLs in system browser (allowlist-only, no scraping)",
        ));
    }
    if config.composio.enabled {
        tool_descs.push((
            "composio",
            "Execute actions on 1000+ apps via Composio (Gmail, Notion, GitHub, Slack, etc.). Use action='list' to discover actions, 'list_accounts' to retrieve connected account IDs, 'execute' to run (optionally with connected_account_id), and 'connect' for OAuth.",
        ));
    }
    tool_descs.push((
        "schedule",
        "Manage scheduled tasks (create/list/get/cancel/pause/resume). Supports recurring cron and one-shot delays.",
    ));
    tool_descs.push((
        "pushover",
        "Send a Pushover notification to your device. Requires PUSHOVER_TOKEN and PUSHOVER_USER_KEY in .env file.",
    ));
    if !config.agents.is_empty() {
        tool_descs.push((
            "delegate",
            "Delegate a subtask to a specialized agent. Use when: a task benefits from a different model (e.g. fast summarization, deep reasoning, code generation). The sub-agent runs a single prompt and returns its response.",
        ));
    }

    let excluded = &config.autonomy.non_cli_excluded_tools;
    if !excluded.is_empty() {
        tool_descs.retain(|(name, _)| !excluded.iter().any(|ex| ex == name));
    }

    let bootstrap_max_chars = if config.agent.compact_context {
        Some(6000)
    } else {
        None
    };
    let native_tools = provider.supports_native_tools();
    let mut system_prompt = build_system_prompt_with_mode(
        &workspace,
        &model,
        &tool_descs,
        &skills,
        Some(&config.identity),
        bootstrap_max_chars,
        native_tools,
        config.skills.prompt_injection_mode,
    );
    if !native_tools {
        system_prompt.push_str(&build_tool_instructions(tools_registry.as_ref()));
    }

    if !skills.is_empty() {
        println!(
            "  🧩 Skills:   {}",
            skills
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let mut channels: Vec<Arc<dyn Channel>> =
        collect_configured_channels(&config, "runtime startup")
            .into_iter()
            .map(|configured| configured.channel)
            .collect();

    if let Some(ref ns) = config.channels_config.nostr {
        channels.push(Arc::new(
            NostrChannel::new(&ns.private_key, ns.relays.clone(), &ns.allowed_pubkeys).await?,
        ));
    }
    if channels.is_empty() {
        println!("No channels configured. Run `zeroclaw onboard` to set up channels.");
        return Ok(());
    }

    println!("🦀 ZeroClaw Channel Server");
    println!("  🤖 Model:    {model}");
    let effective_backend = memory::effective_memory_backend_name(
        &config.memory.backend,
        Some(&config.storage.provider.config),
    );
    println!(
        "  🧠 Memory:   {} (auto-save: {})",
        effective_backend,
        if config.memory.auto_save { "on" } else { "off" }
    );
    println!(
        "  📡 Channels: {}",
        channels
            .iter()
            .map(|c| c.name())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!();
    println!("  Listening for messages... (Ctrl+C to stop)");
    println!();

    crate::health::mark_component_ok("channels");

    let initial_backoff_secs = config
        .reliability
        .channel_initial_backoff_secs
        .max(DEFAULT_CHANNEL_INITIAL_BACKOFF_SECS);
    let max_backoff_secs = config
        .reliability
        .channel_max_backoff_secs
        .max(DEFAULT_CHANNEL_MAX_BACKOFF_SECS);

    let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(100);

    let mut handles = Vec::new();
    for ch in &channels {
        handles.push(spawn_supervised_listener(
            ch.clone(),
            tx.clone(),
            initial_backoff_secs,
            max_backoff_secs,
        ));
    }
    drop(tx);

    let channels_by_name = Arc::new(
        channels
            .iter()
            .map(|ch| (ch.name().to_string(), Arc::clone(ch)))
            .collect::<HashMap<_, _>>(),
    );
    let max_in_flight_messages = compute_max_in_flight_messages(channels.len());

    println!("  🚦 In-flight message limit: {max_in_flight_messages}");

    let mut provider_cache_seed: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    provider_cache_seed.insert(provider_name.clone(), Arc::clone(&provider));
    let message_timeout_secs =
        effective_channel_message_timeout_secs(config.channels_config.message_timeout_secs);
    let interrupt_on_new_message = config
        .channels_config
        .telegram
        .as_ref()
        .is_some_and(|tg| tg.interrupt_on_new_message);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name,
        provider: Arc::clone(&provider),
        default_provider: Arc::new(provider_name),
        memory: Arc::clone(&mem),
        tools_registry: Arc::clone(&tools_registry),
        observer,
        system_prompt: Arc::new(system_prompt),
        model: Arc::new(model.clone()),
        temperature,
        auto_save_memory: config.memory.auto_save,
        max_tool_iterations: config.agent.max_tool_iterations,
        min_relevance_score: config.memory.min_relevance_score,
        conversation_histories: Arc::new(Mutex::new(HashMap::new())),
        provider_cache: Arc::new(Mutex::new(provider_cache_seed)),
        route_overrides: Arc::new(Mutex::new(HashMap::new())),
        api_key: config.api_key.clone(),
        api_url: config.api_url.clone(),
        reliability: Arc::new(config.reliability.clone()),
        provider_runtime_options,
        workspace_dir: Arc::new(config.workspace_dir.clone()),
        message_timeout_secs,
        interrupt_on_new_message,
        multimodal: config.multimodal.clone(),
        hooks: if config.hooks.enabled {
            let mut runner = crate::hooks::HookRunner::new();
            if config.hooks.builtin.command_logger {
                runner.register(Box::new(crate::hooks::builtin::CommandLoggerHook::new()));
            }
            Some(Arc::new(runner))
        } else {
            None
        },
        non_cli_excluded_tools: Arc::new(config.autonomy.non_cli_excluded_tools.clone()),
        model_routes: Arc::new(config.model_routes.clone()),
    });

    run_message_dispatch_loop(rx, runtime_ctx, max_in_flight_messages).await;

    for h in handles {
        let _ = h.await;
    }

    Ok(())
}
