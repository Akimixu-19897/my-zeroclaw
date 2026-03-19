fn approval_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[tokio::test]
async fn process_channel_message_executes_tool_calls_instead_of_sending_raw_json() {
    let channel_impl = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(ToolCallingProvider),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![Box::new(MockPriceTool)]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("test-system-prompt".to_string()),
        model: Arc::new("test-model".to_string()),
        temperature: 0.0,
        auto_save_memory: false,
        max_tool_iterations: 10,
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
        non_cli_excluded_tools: Arc::new(Vec::new()),
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        model_routes: Arc::new(Vec::new()),
    });

    process_channel_message(
        runtime_ctx,
        traits::ChannelMessage {
            id: "msg-1".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat-42".to_string(),
            content: "What is the BTC price now?".to_string(),
            channel: "test-channel".to_string(),
            timestamp: 1,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let sent_messages = channel_impl.sent_messages.lock().await;
    assert!(!sent_messages.is_empty());
    let reply = sent_messages.last().unwrap();
    assert!(reply.starts_with("chat-42:"));
    assert!(reply.contains("BTC is currently around"));
    assert!(!reply.contains("\"tool_calls\""));
    assert!(!reply.contains("mock_price"));
}

#[tokio::test]
async fn process_channel_message_replies_flat_in_feishu_even_when_inbound_has_thread() {
    let channel_impl = Arc::new(RecordingFeishuChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(DummyProvider),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("test-system-prompt".to_string()),
        model: Arc::new("test-model".to_string()),
        temperature: 0.0,
        auto_save_memory: false,
        max_tool_iterations: 10,
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
        non_cli_excluded_tools: Arc::new(Vec::new()),
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        model_routes: Arc::new(Vec::new()),
    });

    process_channel_message(
        runtime_ctx,
        traits::ChannelMessage {
            id: "om_root_1".to_string(),
            sender: "alice".to_string(),
            reply_target: "oc_chat_1".to_string(),
            content: "hello".to_string(),
            channel: "feishu".to_string(),
            timestamp: 1,
            thread_ts: Some("om_root_1".to_string()),
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let sent = channel_impl.sent_messages.lock().await;
    assert_eq!(sent.as_slice(), ["oc_chat_1:ok"]);
    drop(sent);

    let sent_threads = channel_impl.sent_threads.lock().await;
    assert_eq!(sent_threads.as_slice(), &[None]);
}

#[tokio::test]
async fn process_channel_message_sends_feishu_confirmation_card_for_pending_approval() {
    let _guard = approval_test_lock().lock().await;
    clear_pending_channel_approvals();
    let channel_impl = Arc::new(RecordingFeishuChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(ApprovalRequestProvider),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![Box::new(ApprovalAwareTool)]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("test-system-prompt".to_string()),
        model: Arc::new("test-model".to_string()),
        temperature: 0.0,
        auto_save_memory: false,
        max_tool_iterations: 10,
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
        non_cli_excluded_tools: Arc::new(Vec::new()),
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        model_routes: Arc::new(Vec::new()),
    });

    process_channel_message(
        runtime_ctx,
        traits::ChannelMessage {
            id: "approval-msg-1".to_string(),
            sender: "alice".to_string(),
            reply_target: "oc_chat_1".to_string(),
            content: "Run the shell command".to_string(),
            channel: "feishu".to_string(),
            timestamp: 1,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let sent = channel_impl.sent_messages.lock().await;
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("```lark-card"));
    assert!(sent[0].contains("Confirmation Required"));
    assert!(sent[0].contains("confirm_write"));
    drop(sent);

    let sent_threads = channel_impl.sent_threads.lock().await;
    assert_eq!(sent_threads.as_slice(), &[None]);
}

#[tokio::test]
async fn process_channel_message_confirm_action_executes_pending_approval() {
    let _guard = approval_test_lock().lock().await;
    clear_pending_channel_approvals();
    let operation_id = format!("op-confirm-{}", uuid::Uuid::new_v4());
    let channel_impl = Arc::new(RecordingFeishuChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(ApprovalSummaryProvider),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![Box::new(ApprovalAwareTool)]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("test-system-prompt".to_string()),
        model: Arc::new("test-model".to_string()),
        temperature: 0.0,
        auto_save_memory: false,
        max_tool_iterations: 10,
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
        non_cli_excluded_tools: Arc::new(Vec::new()),
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        model_routes: Arc::new(Vec::new()),
    });

    insert_pending_channel_approval(PendingChannelApproval {
        operation_id: operation_id.clone(),
        tool_name: "shell".to_string(),
        arguments: serde_json::json!({ "command": "touch /tmp/approval-test" }),
        reason: "Command requires explicit approval (approved=true): medium-risk operation"
            .to_string(),
        preview: "{\n  \"command\": \"touch /tmp/approval-test\"\n}".to_string(),
        reply_target: "oc_chat_1".to_string(),
        thread_ts: Some("om_card_1".to_string()),
        user_message: "Run the shell command".to_string(),
        provider: "test-provider".to_string(),
        model: "test-model".to_string(),
        created_at: Instant::now(),
    });

    process_channel_message(
            runtime_ctx,
            traits::ChannelMessage {
                id: "om_card_1".to_string(),
                sender: "ou_user_1".to_string(),
                reply_target: "oc_chat_1".to_string(),
                content: format!(
                    "```lark-card-action\n{{\"type\":\"lark_card_action\",\"action\":\"confirm_write\",\"operation_id\":\"{}\",\"message_id\":\"om_card_1\",\"chat_id\":\"oc_chat_1\",\"operator_open_id\":\"ou_user_1\",\"value\":{{}}}}\n```",
                    operation_id
                ),
                channel: "feishu".to_string(),
                timestamp: 2,
                thread_ts: Some("om_card_1".to_string()),
                context: None,
            },
            CancellationToken::new(),
        )
        .await;

    let sent = channel_impl.sent_messages.lock().await;
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].contains("approval-complete")
            || sent[0].contains("approved:touch /tmp/approval-test"),
        "unexpected approval completion payload: {}",
        sent[0]
    );
    drop(sent);

    let sent_threads = channel_impl.sent_threads.lock().await;
    assert_eq!(sent_threads.as_slice(), &[None]);
    assert!(get_pending_channel_approval(&operation_id).is_none());
}

#[tokio::test]
async fn process_channel_message_telegram_does_not_persist_tool_summary_prefix() {
    let channel_impl = Arc::new(TelegramRecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(ToolCallingProvider),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![Box::new(MockPriceTool)]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("test-system-prompt".to_string()),
        model: Arc::new("test-model".to_string()),
        temperature: 0.0,
        auto_save_memory: false,
        max_tool_iterations: 10,
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
        non_cli_excluded_tools: Arc::new(Vec::new()),
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        model_routes: Arc::new(Vec::new()),
    });

    process_channel_message(
        runtime_ctx.clone(),
        traits::ChannelMessage {
            id: "msg-telegram-tool-1".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat-telegram".to_string(),
            content: "What is the BTC price now?".to_string(),
            channel: "telegram".to_string(),
            timestamp: 1,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let sent_messages = channel_impl.sent_messages.lock().await;
    assert!(!sent_messages.is_empty());
    let reply = sent_messages.last().unwrap();
    assert!(reply.contains("BTC is currently around"));

    let histories = runtime_ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let turns = histories
        .get("telegram_alice")
        .expect("telegram history should be stored");
    let assistant_turn = turns
        .iter()
        .rev()
        .find(|turn| turn.role == "assistant")
        .expect("assistant turn should be present");
    assert!(
        !assistant_turn.content.contains("[Used tools:"),
        "telegram history should not persist tool-summary prefix"
    );
}

#[tokio::test]
async fn process_channel_message_strips_unexecuted_tool_json_artifacts_from_reply() {
    let channel_impl = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(RawToolArtifactProvider),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![Box::new(MockPriceTool)]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("test-system-prompt".to_string()),
        model: Arc::new("test-model".to_string()),
        temperature: 0.0,
        auto_save_memory: false,
        max_tool_iterations: 10,
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
        runtime_ctx,
        traits::ChannelMessage {
            id: "msg-raw-json".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat-raw".to_string(),
            content: "What is the BTC price now?".to_string(),
            channel: "test-channel".to_string(),
            timestamp: 3,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let sent_messages = channel_impl.sent_messages.lock().await;
    assert_eq!(sent_messages.len(), 1);
    assert!(sent_messages[0].starts_with("chat-raw:"));
    assert!(sent_messages[0].contains("BTC is currently around"));
    assert!(!sent_messages[0].contains("\"name\":\"mock_price\""));
    assert!(!sent_messages[0].contains("\"result\""));
}

#[tokio::test]
async fn process_channel_message_executes_tool_calls_with_alias_tags() {
    let channel_impl = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(ToolCallingAliasProvider),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![Box::new(MockPriceTool)]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("test-system-prompt".to_string()),
        model: Arc::new("test-model".to_string()),
        temperature: 0.0,
        auto_save_memory: false,
        max_tool_iterations: 10,
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
        runtime_ctx,
        traits::ChannelMessage {
            id: "msg-2".to_string(),
            sender: "bob".to_string(),
            reply_target: "chat-84".to_string(),
            content: "What is the BTC price now?".to_string(),
            channel: "test-channel".to_string(),
            timestamp: 2,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let sent_messages = channel_impl.sent_messages.lock().await;
    assert!(!sent_messages.is_empty());
    let reply = sent_messages.last().unwrap();
    assert!(reply.starts_with("chat-84:"));
    assert!(reply.contains("alias-tag flow resolved"));
    assert!(!reply.contains("<toolcall>"));
    assert!(!reply.contains("mock_price"));
}
