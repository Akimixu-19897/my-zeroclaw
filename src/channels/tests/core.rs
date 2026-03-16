
fn make_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    // Create minimal workspace files
    std::fs::write(tmp.path().join("SOUL.md"), "# Soul\nBe helpful.").unwrap();
    std::fs::write(tmp.path().join("IDENTITY.md"), "# Identity\nName: ZeroClaw").unwrap();
    std::fs::write(tmp.path().join("USER.md"), "# User\nName: Test User").unwrap();
    std::fs::write(
        tmp.path().join("AGENTS.md"),
        "# Agents\nFollow instructions.",
    )
    .unwrap();
    std::fs::write(tmp.path().join("TOOLS.md"), "# Tools\nUse shell carefully.").unwrap();
    std::fs::write(
        tmp.path().join("HEARTBEAT.md"),
        "# Heartbeat\nCheck status.",
    )
    .unwrap();
    std::fs::write(tmp.path().join("MEMORY.md"), "# Memory\nUser likes Rust.").unwrap();
    tmp
}

#[test]
fn effective_channel_message_timeout_secs_clamps_to_minimum() {
    assert_eq!(
        effective_channel_message_timeout_secs(0),
        MIN_CHANNEL_MESSAGE_TIMEOUT_SECS
    );
    assert_eq!(
        effective_channel_message_timeout_secs(15),
        MIN_CHANNEL_MESSAGE_TIMEOUT_SECS
    );
    assert_eq!(effective_channel_message_timeout_secs(300), 300);
}

#[test]
fn channel_message_timeout_budget_scales_with_tool_iterations() {
    assert_eq!(channel_message_timeout_budget_secs(300, 1), 300);
    assert_eq!(channel_message_timeout_budget_secs(300, 2), 600);
    assert_eq!(channel_message_timeout_budget_secs(300, 3), 900);
}

#[test]
fn channel_message_timeout_budget_uses_safe_defaults_and_cap() {
    // 0 iterations falls back to 1x timeout budget.
    assert_eq!(channel_message_timeout_budget_secs(300, 0), 300);
    // Large iteration counts are capped to avoid runaway waits.
    assert_eq!(
        channel_message_timeout_budget_secs(300, 10),
        300 * CHANNEL_MESSAGE_TIMEOUT_SCALE_CAP
    );
}

#[test]
fn context_window_overflow_error_detector_matches_known_messages() {
    let overflow_err = anyhow::anyhow!(
        "OpenAI Codex stream error: Your input exceeds the context window of this model."
    );
    assert!(is_context_window_overflow_error(&overflow_err));

    let other_err = anyhow::anyhow!("OpenAI Codex API error (502 Bad Gateway): error code: 502");
    assert!(!is_context_window_overflow_error(&other_err));
}

#[test]
fn memory_context_skip_rules_exclude_history_blobs() {
    assert!(should_skip_memory_context_entry(
        "telegram_123_history",
        r#"[{"role":"user"}]"#
    ));
    assert!(should_skip_memory_context_entry(
        "assistant_resp_legacy",
        "fabricated memory"
    ));
    assert!(!should_skip_memory_context_entry("telegram_123_45", "hi"));
}

#[test]
fn normalize_cached_channel_turns_merges_consecutive_user_turns() {
    let turns = vec![
        ChatMessage::user("forwarded content"),
        ChatMessage::user("summarize this"),
    ];

    let normalized = normalize_cached_channel_turns(turns);
    assert_eq!(normalized.len(), 1);
    assert_eq!(normalized[0].role, "user");
    assert!(normalized[0].content.contains("forwarded content"));
    assert!(normalized[0].content.contains("summarize this"));
}

#[test]
fn normalize_cached_channel_turns_merges_consecutive_assistant_turns() {
    let turns = vec![
        ChatMessage::user("first user"),
        ChatMessage::assistant("assistant part 1"),
        ChatMessage::assistant("assistant part 2"),
        ChatMessage::user("next user"),
    ];

    let normalized = normalize_cached_channel_turns(turns);
    assert_eq!(normalized.len(), 3);
    assert_eq!(normalized[0].role, "user");
    assert_eq!(normalized[1].role, "assistant");
    assert_eq!(normalized[2].role, "user");
    assert!(normalized[1].content.contains("assistant part 1"));
    assert!(normalized[1].content.contains("assistant part 2"));
}

/// Verify that an orphan user turn followed by a failure-marker assistant
/// turn normalizes correctly, so the LLM sees the failed request as closed
/// and does not re-execute it on the next user message.
#[test]
fn normalize_preserves_failure_marker_after_orphan_user_turn() {
    let turns = vec![
        ChatMessage::user("download something from GitHub"),
        ChatMessage::assistant("[Task failed — not continuing this request]"),
        ChatMessage::user("what is WAL?"),
    ];

    let normalized = normalize_cached_channel_turns(turns);
    assert_eq!(normalized.len(), 3);
    assert_eq!(normalized[0].role, "user");
    assert_eq!(normalized[1].role, "assistant");
    assert!(normalized[1].content.contains("Task failed"));
    assert_eq!(normalized[2].role, "user");
    assert_eq!(normalized[2].content, "what is WAL?");
}

/// Same as above but for the timeout variant.
#[test]
fn normalize_preserves_timeout_marker_after_orphan_user_turn() {
    let turns = vec![
        ChatMessage::user("run a long task"),
        ChatMessage::assistant("[Task timed out — not continuing this request]"),
        ChatMessage::user("next question"),
    ];

    let normalized = normalize_cached_channel_turns(turns);
    assert_eq!(normalized.len(), 3);
    assert_eq!(normalized[1].role, "assistant");
    assert!(normalized[1].content.contains("Task timed out"));
    assert_eq!(normalized[2].content, "next question");
}

#[test]
fn compact_sender_history_keeps_recent_truncated_messages() {
    let mut histories = HashMap::new();
    let sender = "telegram_u1".to_string();
    histories.insert(
        sender.clone(),
        (0..20)
            .map(|idx| {
                let content = format!("msg-{idx}-{}", "x".repeat(700));
                if idx % 2 == 0 {
                    ChatMessage::user(content)
                } else {
                    ChatMessage::assistant(content)
                }
            })
            .collect::<Vec<_>>(),
    );

    let ctx = ChannelRuntimeContext {
        channels_by_name: Arc::new(HashMap::new()),
        provider: Arc::new(DummyProvider),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("system".to_string()),
        model: Arc::new("test-model".to_string()),
        temperature: 0.0,
        auto_save_memory: false,
        max_tool_iterations: 5,
        min_relevance_score: 0.0,
        conversation_histories: Arc::new(Mutex::new(histories)),
        provider_cache: Arc::new(Mutex::new(HashMap::new())),
        route_overrides: Arc::new(Mutex::new(HashMap::new())),
        api_key: None,
        api_url: None,
        reliability: Arc::new(crate::config::ReliabilityConfig::default()),
        interrupt_on_new_message: false,
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        provider_runtime_options: providers::ProviderRuntimeOptions::default(),
        workspace_dir: Arc::new(std::env::temp_dir()),
        message_timeout_secs: CHANNEL_MESSAGE_TIMEOUT_SECS,
        non_cli_excluded_tools: Arc::new(Vec::new()),
        model_routes: Arc::new(Vec::new()),
    };

    assert!(compact_sender_history(&ctx, &sender));

    let histories = ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let kept = histories
        .get(&sender)
        .expect("sender history should remain");
    assert_eq!(kept.len(), CHANNEL_HISTORY_COMPACT_KEEP_MESSAGES);
    assert!(kept.iter().all(|turn| {
        let len = turn.content.chars().count();
        len <= CHANNEL_HISTORY_COMPACT_CONTENT_CHARS
            || (len <= CHANNEL_HISTORY_COMPACT_CONTENT_CHARS + 3 && turn.content.ends_with("..."))
    }));
}

#[test]
fn append_sender_turn_stores_single_turn_per_call() {
    let sender = "telegram_u2".to_string();
    let ctx = ChannelRuntimeContext {
        channels_by_name: Arc::new(HashMap::new()),
        provider: Arc::new(DummyProvider),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("system".to_string()),
        model: Arc::new("test-model".to_string()),
        temperature: 0.0,
        auto_save_memory: false,
        max_tool_iterations: 5,
        min_relevance_score: 0.0,
        conversation_histories: Arc::new(Mutex::new(HashMap::new())),
        provider_cache: Arc::new(Mutex::new(HashMap::new())),
        route_overrides: Arc::new(Mutex::new(HashMap::new())),
        api_key: None,
        api_url: None,
        reliability: Arc::new(crate::config::ReliabilityConfig::default()),
        interrupt_on_new_message: false,
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        provider_runtime_options: providers::ProviderRuntimeOptions::default(),
        workspace_dir: Arc::new(std::env::temp_dir()),
        message_timeout_secs: CHANNEL_MESSAGE_TIMEOUT_SECS,
        non_cli_excluded_tools: Arc::new(Vec::new()),
        model_routes: Arc::new(Vec::new()),
    };

    append_sender_turn(&ctx, &sender, ChatMessage::user("hello"));

    let histories = ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let turns = histories.get(&sender).expect("sender history should exist");
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].role, "user");
    assert_eq!(turns[0].content, "hello");
}

#[test]
fn rollback_orphan_user_turn_removes_only_latest_matching_user_turn() {
    let sender = "telegram_u3".to_string();
    let mut histories = HashMap::new();
    histories.insert(
        sender.clone(),
        vec![
            ChatMessage::user("first"),
            ChatMessage::assistant("ok"),
            ChatMessage::user("pending"),
        ],
    );
    let ctx = ChannelRuntimeContext {
        channels_by_name: Arc::new(HashMap::new()),
        provider: Arc::new(DummyProvider),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("system".to_string()),
        model: Arc::new("test-model".to_string()),
        temperature: 0.0,
        auto_save_memory: false,
        max_tool_iterations: 5,
        min_relevance_score: 0.0,
        conversation_histories: Arc::new(Mutex::new(histories)),
        provider_cache: Arc::new(Mutex::new(HashMap::new())),
        route_overrides: Arc::new(Mutex::new(HashMap::new())),
        api_key: None,
        api_url: None,
        reliability: Arc::new(crate::config::ReliabilityConfig::default()),
        interrupt_on_new_message: false,
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        provider_runtime_options: providers::ProviderRuntimeOptions::default(),
        workspace_dir: Arc::new(std::env::temp_dir()),
        message_timeout_secs: CHANNEL_MESSAGE_TIMEOUT_SECS,
        non_cli_excluded_tools: Arc::new(Vec::new()),
        model_routes: Arc::new(Vec::new()),
    };

    assert!(rollback_orphan_user_turn(&ctx, &sender, "pending"));

    let histories = ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let turns = histories
        .get(&sender)
        .expect("sender history should remain");
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].content, "first");
    assert_eq!(turns[1].content, "ok");
}

struct DummyProvider;

#[async_trait::async_trait]
impl Provider for DummyProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok("ok".to_string())
    }
}
