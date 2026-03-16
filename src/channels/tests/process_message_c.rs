#[tokio::test]
async fn process_channel_message_reports_configured_max_tool_iterations_limit() {
    let channel_impl = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(IterativeToolProvider {
            required_tool_iterations: 20,
        }),
        default_provider: Arc::new("test-provider".to_string()),
        memory: Arc::new(NoopMemory),
        tools_registry: Arc::new(vec![Box::new(MockPriceTool)]),
        observer: Arc::new(NoopObserver),
        system_prompt: Arc::new("test-system-prompt".to_string()),
        model: Arc::new("test-model".to_string()),
        temperature: 0.0,
        auto_save_memory: false,
        max_tool_iterations: 3,
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
            id: "msg-iter-fail".to_string(),
            sender: "bob".to_string(),
            reply_target: "chat-iter-fail".to_string(),
            content: "Loop forever".to_string(),
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
    assert!(reply.starts_with("chat-iter-fail:"));
    assert!(reply.contains("⚠️ Error: Agent exceeded maximum tool iterations (3)"));
}

struct NoopMemory;

#[async_trait::async_trait]
impl Memory for NoopMemory {
    fn name(&self) -> &str {
        "noop"
    }

    async fn store(
        &self,
        _key: &str,
        _content: &str,
        _category: crate::memory::MemoryCategory,
        _session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn recall(
        &self,
        _query: &str,
        _limit: usize,
        _session_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::memory::MemoryEntry>> {
        Ok(Vec::new())
    }

    async fn get(&self, _key: &str) -> anyhow::Result<Option<crate::memory::MemoryEntry>> {
        Ok(None)
    }

    async fn list(
        &self,
        _category: Option<&crate::memory::MemoryCategory>,
        _session_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::memory::MemoryEntry>> {
        Ok(Vec::new())
    }

    async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn count(&self) -> anyhow::Result<usize> {
        Ok(0)
    }

    async fn health_check(&self) -> bool {
        true
    }
}

struct RecallMemory;

#[async_trait::async_trait]
impl Memory for RecallMemory {
    fn name(&self) -> &str {
        "recall-memory"
    }

    async fn store(
        &self,
        _key: &str,
        _content: &str,
        _category: crate::memory::MemoryCategory,
        _session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn recall(
        &self,
        _query: &str,
        _limit: usize,
        _session_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::memory::MemoryEntry>> {
        Ok(vec![crate::memory::MemoryEntry {
            id: "entry-1".to_string(),
            key: "memory_key_1".to_string(),
            content: "Age is 45".to_string(),
            category: crate::memory::MemoryCategory::Conversation,
            timestamp: "2026-02-20T00:00:00Z".to_string(),
            session_id: None,
            score: Some(0.9),
        }])
    }

    async fn get(&self, _key: &str) -> anyhow::Result<Option<crate::memory::MemoryEntry>> {
        Ok(None)
    }

    async fn list(
        &self,
        _category: Option<&crate::memory::MemoryCategory>,
        _session_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::memory::MemoryEntry>> {
        Ok(Vec::new())
    }

    async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn count(&self) -> anyhow::Result<usize> {
        Ok(1)
    }

    async fn health_check(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn message_dispatch_processes_messages_in_parallel() {
    let channel_impl = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name: Arc::new(channels_by_name),
        provider: Arc::new(SlowProvider {
            delay: Duration::from_millis(250),
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

    let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(4);
    tx.send(traits::ChannelMessage {
        id: "1".to_string(),
        sender: "alice".to_string(),
        reply_target: "alice".to_string(),
        content: "hello".to_string(),
        channel: "test-channel".to_string(),
        timestamp: 1,
        thread_ts: None,
        context: None,
    })
    .await
    .unwrap();
    tx.send(traits::ChannelMessage {
        id: "2".to_string(),
        sender: "bob".to_string(),
        reply_target: "bob".to_string(),
        content: "world".to_string(),
        channel: "test-channel".to_string(),
        timestamp: 2,
        thread_ts: None,
        context: None,
    })
    .await
    .unwrap();
    drop(tx);

    let started = Instant::now();
    run_message_dispatch_loop(rx, runtime_ctx, 2).await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_millis(430),
        "expected parallel dispatch (<430ms), got {:?}",
        elapsed
    );

    let sent_messages = channel_impl.sent_messages.lock().await;
    assert_eq!(sent_messages.len(), 2);
}

#[tokio::test]
async fn message_dispatch_interrupts_in_flight_telegram_request_and_preserves_context() {
    let channel_impl = Arc::new(TelegramRecordingChannel::default());
    let channel: Arc<dyn Channel> = channel_impl.clone();

    let mut channels_by_name = HashMap::new();
    channels_by_name.insert(channel.name().to_string(), channel);

    let provider_impl = Arc::new(DelayedHistoryCaptureProvider {
        delay: Duration::from_millis(250),
        calls: std::sync::Mutex::new(Vec::new()),
    });

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
            id: "msg-1".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat-1".to_string(),
            content: "forwarded content".to_string(),
            channel: "telegram".to_string(),
            timestamp: 1,
            thread_ts: None,
            context: None,
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        tx.send(traits::ChannelMessage {
            id: "msg-2".to_string(),
            sender: "alice".to_string(),
            reply_target: "chat-1".to_string(),
            content: "summarize this".to_string(),
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
    assert_eq!(sent_messages.len(), 1);
    assert!(sent_messages[0].starts_with("chat-1:"));
    assert!(sent_messages[0].contains("response-2"));
    drop(sent_messages);

    let calls = provider_impl
        .calls
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    assert_eq!(calls.len(), 2);
    let second_call = &calls[1];
    assert!(second_call
        .iter()
        .any(|(role, content)| { role == "user" && content.contains("forwarded content") }));
    assert!(second_call
        .iter()
        .any(|(role, content)| { role == "user" && content.contains("summarize this") }));
    assert!(
        !second_call.iter().any(|(role, _)| role == "assistant"),
        "cancelled turn should not persist an assistant response"
    );
}

