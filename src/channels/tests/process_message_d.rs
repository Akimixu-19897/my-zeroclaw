#[tokio::test]
async fn message_dispatch_interrupt_scope_is_same_sender_same_chat() {
    let channel_impl = Arc::new(TelegramRecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(SlowProvider {
            delay: Duration::from_millis(180),
        }),
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
        interrupt_on_new_message: true,
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        non_cli_excluded_tools: Arc::new(Vec::new()),
        model_routes: Arc::new(Vec::new()),
    });

    let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(8);
    let send_task = tokio::spawn(async move {
        tx.send(traits::ChannelMessage {
            id: "msg-a".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat-1".to_string(),
            content: "first chat".to_string(),
            channel: "telegram".to_string(),
            timestamp: 1,
            thread_ts: None,
            context: None,
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(traits::ChannelMessage {
            id: "msg-b".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat-2".to_string(),
            content: "second chat".to_string(),
            channel: "telegram".to_string(),
            timestamp: 2,
            thread_ts: None,
            context: None,
        })
        .await
        .unwrap();
    });

    run_message_dispatch_loop(rx, runtime_ctx, 4).await;
    send_task.await.unwrap();

    let sent_messages = channel_impl.sent_messages.lock().await;
    assert_eq!(sent_messages.len(), 2);
    assert!(sent_messages.iter().any(|msg| msg.starts_with("chat-1:")));
    assert!(sent_messages.iter().any(|msg| msg.starts_with("chat-2:")));
}

#[tokio::test]
async fn process_channel_message_cancels_scoped_typing_task() {
    let channel_impl = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(SlowProvider {
            delay: Duration::from_millis(20),
        }),
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
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        non_cli_excluded_tools: Arc::new(Vec::new()),
        model_routes: Arc::new(Vec::new()),
    });

    process_channel_message(
        runtime_ctx,
        traits::ChannelMessage {
            id: "typing-msg".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat-typing".to_string(),
            content: "hello".to_string(),
            channel: "test-channel".to_string(),
            timestamp: 1,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let starts = channel_impl.start_typing_calls.load(Ordering::SeqCst);
    let stops = channel_impl.stop_typing_calls.load(Ordering::SeqCst);
    assert_eq!(starts, 1, "start_typing should be called once");
    assert_eq!(stops, 1, "stop_typing should be called once");
}

#[tokio::test]
async fn process_channel_message_adds_and_swaps_reactions() {
    let channel_impl = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(SlowProvider {
            delay: Duration::from_millis(5),
        }),
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
        multimodal: crate::config::MultimodalConfig::default(),
        hooks: None,
        non_cli_excluded_tools: Arc::new(Vec::new()),
        model_routes: Arc::new(Vec::new()),
    });

    process_channel_message(
        runtime_ctx,
        traits::ChannelMessage {
            id: "react-msg".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat-react".to_string(),
            content: "hello".to_string(),
            channel: "test-channel".to_string(),
            timestamp: 1,
            thread_ts: None,
            context: None,
        },
        CancellationToken::new(),
    )
    .await;

    let added = channel_impl.reactions_added.lock().await;
    assert!(
        added.len() >= 2,
        "expected at least 2 reactions added (\u{1F440} then \u{2705}), got {}",
        added.len()
    );
    assert_eq!(added[0].2, "\u{1F440}", "first reaction should be eyes");
    assert_eq!(
        added.last().unwrap().2,
        "\u{2705}",
        "last reaction should be checkmark"
    );

    let removed = channel_impl.reactions_removed.lock().await;
    assert_eq!(removed.len(), 1, "eyes reaction should be removed once");
    assert_eq!(removed[0].2, "\u{1F440}");
}
