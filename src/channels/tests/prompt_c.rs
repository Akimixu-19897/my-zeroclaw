#[tokio::test]
async fn process_feishu_threaded_media_message_preserves_lark_context_metadata() {
    let channel_impl = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let provider_impl = Arc::new(HistoryCaptureProvider::default());
    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: provider_impl.clone(),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("test-system-prompt".to_string()),
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
        runtime_ctx,
        traits::ChannelMessage {
            id: "om_media_1".to_string(),
            sender: "oc_chat_thread".to_string(),
            reply_target: "oc_chat_thread".to_string(),
            content: "received image attachment".to_string(),
            channel: "feishu".to_string(),
            timestamp: 1,
            thread_ts: Some("omt_thread_1".to_string()),
            context: Some(traits::ChannelMessageContext {
                sender_id: Some("ou_user_media".to_string()),
                chat_id: Some("oc_chat_thread".to_string()),
                chat_type: Some("group".to_string()),
                content_type: Some("image".to_string()),
                raw_content: Some("{\"image_key\":\"img_v3_demo\"}".to_string()),
                root_id: Some("om_root_1".to_string()),
                parent_id: Some("om_parent_1".to_string()),
                thread_id: Some("omt_thread_1".to_string()),
                origin_from: Some("feishu:ou_user_media".to_string()),
                origin_to: Some("chat:oc_chat_thread".to_string()),
                envelope_from: Some("oc_chat_thread:ou_user_media".to_string()),
            }),
        },
        CancellationToken::new(),
    )
    .await;

    let calls = provider_impl
        .calls
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    assert_eq!(calls.len(), 1);
    let first_call = &calls[0];
    assert!(first_call.iter().any(|(role, content)| {
        role == "user"
            && content.contains("content_type=image")
            && content.contains("parent_id=om_parent_1")
            && content.contains("thread_id=omt_thread_1")
    }));
}

#[tokio::test]
async fn process_channel_message_enriches_current_turn_without_persisting_context() {
    let channel_impl = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let provider_impl = Arc::new(HistoryCaptureProvider::default());
    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: provider_impl.clone(),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(RecallMemory),
        tools_registry: Arc::new(vec![]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("test-system-prompt".to_string()),
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
        runtime_ctx.clone(),
        traits::ChannelMessage {
            id: "msg-ctx-1".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat-ctx".to_string(),
            content: "hello".to_string(),
            channel: "test-channel".to_string(),
            timestamp: 1,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let calls = provider_impl
        .calls
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].len(), 2);
    assert_eq!(calls[0][1].0, "user");
    assert!(calls[0][1].1.contains("[Memory context]"));
    assert!(calls[0][1].1.contains("Age is 45"));
    assert!(calls[0][1].1.contains("hello"));

    let histories = runtime_ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let turns = histories
        .get("test-channel_alice")
        .expect("history should be stored for sender");
    assert_eq!(turns[0].role, "user");
    assert_eq!(turns[0].content, "hello");
    assert!(!turns[0].content.contains("[Memory context]"));
}

#[tokio::test]
async fn process_channel_message_telegram_keeps_system_instruction_at_top_only() {
    let channel_impl = Arc::new(TelegramRecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let provider_impl = Arc::new(HistoryCaptureProvider::default());
    let mut histories = HashMap::new();
    histories.insert(
        "telegram_alice".to_string(),
        vec![
            ChatMessage::assistant("stale assistant"),
            ChatMessage::user("earlier user question"),
            ChatMessage::assistant("earlier assistant reply"),
        ],
    );

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: provider_impl.clone(),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("test-system-prompt".to_string()),
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
        runtime_ctx.clone(),
        traits::ChannelMessage {
            id: "tg-msg-1".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat-telegram".to_string(),
            content: "hello".to_string(),
            channel: "telegram".to_string(),
            timestamp: 1,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let calls = provider_impl
        .calls
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].len(), 4);

    let roles = calls[0]
        .iter()
        .map(|(role, _)| role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, vec!["system", "user", "assistant", "user"]);
    assert!(
        calls[0][0].1.contains("When responding on Telegram:"),
        "telegram channel instructions should be embedded into the system prompt"
    );
    assert!(
        calls[0][0].1.contains("For media attachments use markers:"),
        "telegram media marker guidance should live in the system prompt"
    );
    assert!(!calls[0].iter().skip(1).any(|(role, _)| role == "system"));
}

