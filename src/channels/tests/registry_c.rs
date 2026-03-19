#[test]
fn maybe_restart_daemon_systemd_args_regression() {
    assert_eq!(
        SYSTEMD_STATUS_ARGS,
        ["--user", "is-active", "zeroclaw.service"]
    );
    assert_eq!(
        SYSTEMD_RESTART_ARGS,
        ["--user", "restart", "zeroclaw.service"]
    );
}

#[test]
fn maybe_restart_daemon_openrc_args_regression() {
    assert_eq!(OPENRC_STATUS_ARGS, ["zeroclaw", "status"]);
    assert_eq!(OPENRC_RESTART_ARGS, ["zeroclaw", "restart"]);
}

#[test]
fn normalize_merges_consecutive_user_turns() {
    let turns = vec![ChatMessage::user("hello"), ChatMessage::user("world")];
    let result = normalize_cached_channel_turns(turns);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "user");
    assert_eq!(result[0].content, "hello\n\nworld");
}

#[test]
fn normalize_preserves_strict_alternation() {
    let turns = vec![
        ChatMessage::user("hello"),
        ChatMessage::assistant("hi"),
        ChatMessage::user("bye"),
    ];
    let result = normalize_cached_channel_turns(turns);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].content, "hello");
    assert_eq!(result[1].content, "hi");
    assert_eq!(result[2].content, "bye");
}

#[test]
fn normalize_merges_multiple_consecutive_user_turns() {
    let turns = vec![
        ChatMessage::user("a"),
        ChatMessage::user("b"),
        ChatMessage::user("c"),
    ];
    let result = normalize_cached_channel_turns(turns);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "user");
    assert_eq!(result[0].content, "a\n\nb\n\nc");
}

#[test]
fn normalize_empty_input() {
    let result = normalize_cached_channel_turns(vec![]);
    assert!(result.is_empty());
}

#[test]
fn wecom_does_not_thread_tool_updates_via_message_id() {
    assert!(!should_forward_tool_events_as_thread_messages("wecom"));
    assert!(!should_forward_tool_events_as_thread_messages(
        "wecom:primary"
    ));

    let msg = traits::ChannelMessage {
        id: "wecom-msg-1".to_string(),
        sender: "alice".to_string(),
        reply_target: "user:alice".to_string(),
        content: "check cron".to_string(),
        channel: "wecom".to_string(),
        timestamp: 1,
        thread_ts: Some("req-original".to_string()),
        context: None,
    };

    assert_eq!(
        final_reply_thread_ts_after_tool_updates(&msg, true).as_deref(),
        Some("req-original")
    );

    let named_msg = traits::ChannelMessage {
        channel: "wecom:primary".to_string(),
        ..msg
    };

    assert_eq!(
        final_reply_thread_ts_after_tool_updates(&named_msg, true).as_deref(),
        Some("req-original")
    );
}

#[test]
fn feishu_and_lark_do_not_thread_tool_updates_via_message_id() {
    assert!(!should_forward_tool_events_as_thread_messages("feishu"));
    assert!(!should_forward_tool_events_as_thread_messages(
        "feishu:primary"
    ));
    assert!(!should_forward_tool_events_as_thread_messages("lark"));
    assert!(!should_forward_tool_events_as_thread_messages("lark:primary"));

    let msg = traits::ChannelMessage {
        id: "feishu-msg-1".to_string(),
        sender: "alice".to_string(),
        reply_target: "user:alice".to_string(),
        content: "send an image".to_string(),
        channel: "feishu".to_string(),
        timestamp: 1,
        thread_ts: Some("omt-original".to_string()),
        context: None,
    };

    assert_eq!(
        final_reply_thread_ts_after_tool_updates(&msg, true).as_deref(),
        None
    );

    let named_msg = traits::ChannelMessage {
        channel: "feishu:primary".to_string(),
        ..msg.clone()
    };

    assert_eq!(
        final_reply_thread_ts_after_tool_updates(&named_msg, true).as_deref(),
        None
    );

    let lark_msg = traits::ChannelMessage {
        channel: "lark:primary".to_string(),
        ..msg
    };

    assert_eq!(
        final_reply_thread_ts_after_tool_updates(&lark_msg, false).as_deref(),
        None
    );
}

#[test]
fn slack_keeps_tool_update_threading_via_message_id() {
    assert!(should_forward_tool_events_as_thread_messages("slack"));

    let msg = traits::ChannelMessage {
        id: "slack-root-1".to_string(),
        sender: "alice".to_string(),
        reply_target: "C123".to_string(),
        content: "check cron".to_string(),
        channel: "slack".to_string(),
        timestamp: 1,
        thread_ts: Some("123.456".to_string()),
        context: None,
    };

    assert_eq!(
        final_reply_thread_ts_after_tool_updates(&msg, true).as_deref(),
        Some("slack-root-1")
    );
}

// ── E2E: photo [IMAGE:] marker rejected by non-vision provider ───

/// End-to-end test: a photo attachment message (containing `[IMAGE:]`
/// marker) sent through `process_channel_message` with a non-vision
/// provider must produce a `"⚠️ Error: …does not support vision"` reply
/// on the recording channel — no real Telegram or LLM API required.
#[tokio::test]
async fn e2e_photo_attachment_rejected_by_non_vision_provider() {
    let channel_impl = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    // DummyProvider has default capabilities (vision: false).
    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(DummyProvider),
        default_provider: Arc::new("dummy".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("You are a helpful assistant.".to_string()),
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
        provider_runtime_options: providers::ProviderRuntimeOptions::default(),
        workspace_dir: Arc::new(std::env::temp_dir()),
        message_timeout_secs: CHANNEL_MESSAGE_TIMEOUT_SECS,
        interrupt_on_new_message: false,
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        non_cli_excluded_tools: Arc::new(Vec::new()),
        model_routes: Arc::new(Vec::new()),
    });

    // Simulate a photo attachment message with [IMAGE:] marker.
    process_channel_message(
        runtime_ctx,
        traits::ChannelMessage {
            id: "msg-photo-1".to_string(),
            sender: "zeroclaw_user".to_string(),
            reply_target: "chat-photo".to_string(),
            content: "[IMAGE:/tmp/workspace/photo_99_1.jpg]\n\nWhat is this?".to_string(),
            channel: "test-channel".to_string(),
            timestamp: 1,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let sent = channel_impl.sent_messages.lock().await;
    assert_eq!(sent.len(), 1, "expected exactly one reply message");
    assert!(
        sent[0].contains("does not support vision"),
        "reply must mention vision capability error, got: {}",
        sent[0]
    );
    assert!(
        sent[0].contains("⚠️ Error"),
        "reply must start with error prefix, got: {}",
        sent[0]
    );
}

#[tokio::test]
async fn e2e_failed_vision_turn_does_not_poison_follow_up_text_turn() {
    let channel_impl = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(DummyProvider),
        default_provider: Arc::new("dummy".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("You are a helpful assistant.".to_string()),
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
        provider_runtime_options: providers::ProviderRuntimeOptions::default(),
        workspace_dir: Arc::new(std::env::temp_dir()),
        message_timeout_secs: CHANNEL_MESSAGE_TIMEOUT_SECS,
        interrupt_on_new_message: false,
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        non_cli_excluded_tools: Arc::new(Vec::new()),
        model_routes: Arc::new(Vec::new()),
    });

    process_channel_message(
        Arc::clone(&runtime_ctx),
        traits::ChannelMessage {
            id: "msg-photo-1".to_string(),
            sender: "zeroclaw_user".to_string(),
            reply_target: "chat-photo".to_string(),
            content: "[IMAGE:/tmp/workspace/photo_99_1.jpg]\n\nWhat is this?".to_string(),
            channel: "test-channel".to_string(),
            timestamp: 1,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    process_channel_message(
        Arc::clone(&runtime_ctx),
        traits::ChannelMessage {
            id: "msg-text-2".to_string(),
            sender: "zeroclaw_user".to_string(),
            reply_target: "chat-photo".to_string(),
            content: "What is WAL?".to_string(),
            channel: "test-channel".to_string(),
            timestamp: 2,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let sent = channel_impl.sent_messages.lock().await;
    assert_eq!(sent.len(), 2, "expected one error and one successful reply");
    assert!(
        sent[0].contains("does not support vision"),
        "first reply must mention vision capability error, got: {}",
        sent[0]
    );
    assert!(
        sent[1].ends_with(":ok"),
        "second reply should succeed for text-only turn, got: {}",
        sent[1]
    );
    drop(sent);

    let histories = runtime_ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let turns = histories
        .get("test-channel_zeroclaw_user")
        .expect("history should exist for sender");
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].role, "user");
    assert_eq!(turns[0].content, "What is WAL?");
    assert_eq!(turns[1].role, "assistant");
    assert_eq!(turns[1].content, "ok");
    assert!(
        turns.iter().all(|turn| !turn.content.contains("[IMAGE:")),
        "failed vision turn must not persist image marker content"
    );
}
