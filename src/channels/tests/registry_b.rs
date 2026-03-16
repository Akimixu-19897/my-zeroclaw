#[tokio::test]
async fn classify_health_timeout() {
    let result = tokio::time::timeout(Duration::from_millis(1), async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        true
    })
    .await;
    let state = classify_health_result(&result);
    assert_eq!(state, ChannelHealthState::Timeout);
}

#[test]
fn collect_configured_channels_includes_mattermost_when_configured() {
    let mut config = Config::default();
    config.channels_config.mattermost = Some(crate::config::schema::MattermostConfig {
        url: "https://mattermost.example.com".to_string(),
        bot_token: "test-token".to_string(),
        channel_id: Some("channel-1".to_string()),
        allowed_users: vec![],
        thread_replies: Some(true),
        mention_only: Some(false),
    });

    let channels = collect_configured_channels(&config, "test");

    assert!(channels
        .iter()
        .any(|entry| entry.display_name == "Mattermost"));
    assert!(channels
        .iter()
        .any(|entry| entry.channel.name() == "mattermost"));
}

#[test]
fn collect_configured_channels_includes_named_wecom_accounts() {
    let mut config = Config::default();
    config.channels_config.wecom_accounts.insert(
        "ops".to_string(),
        crate::config::schema::WeComConfig {
            bot_id: "bot-ops".to_string(),
            secret: "secret-ops".to_string(),
            websocket_url: "wss://openws.work.weixin.qq.com".to_string(),
            allowed_users: vec!["*".to_string()],
        },
    );

    let channels = collect_configured_channels(&config, "test");

    assert!(channels
        .iter()
        .any(|entry| entry.display_name == "WeCom[ops]"));
    assert!(channels
        .iter()
        .any(|entry| entry.channel.name() == "wecom:ops"));
}

#[test]
fn collect_configured_channels_includes_named_feishu_accounts() {
    let mut config = Config::default();
    config.channels_config.feishu_accounts.insert(
        "ops".to_string(),
        crate::config::schema::FeishuConfig {
            app_id: "cli_ops".to_string(),
            app_secret: "secret".to_string(),
            enabled: None,
            encrypt_key: None,
            verification_token: None,
            allowed_users: vec!["*".to_string()],
            receive_mode: crate::config::schema::LarkReceiveMode::Websocket,
            port: None,
        },
    );

    let channels = collect_configured_channels(&config, "test");

    assert!(channels
        .iter()
        .any(|entry| entry.display_name == "Feishu[ops]"));
    assert!(channels
        .iter()
        .any(|entry| entry.channel.name() == "feishu:ops"));
}

#[test]
fn collect_configured_channels_excludes_disabled_feishu_accounts() {
    let mut config = Config::default();
    config.channels_config.feishu_accounts.insert(
        "ops".to_string(),
        crate::config::schema::FeishuConfig {
            app_id: "cli_ops".to_string(),
            app_secret: "secret".to_string(),
            enabled: Some(false),
            encrypt_key: None,
            verification_token: None,
            allowed_users: vec!["*".to_string()],
            receive_mode: crate::config::schema::LarkReceiveMode::Websocket,
            port: None,
        },
    );

    let channels = collect_configured_channels(&config, "test");

    assert!(!channels
        .iter()
        .any(|entry| entry.display_name == "Feishu[ops]"));
    assert!(!channels
        .iter()
        .any(|entry| entry.channel.name() == "feishu:ops"));
}

struct AlwaysFailChannel {
    name: &'static str,
    calls: Arc<AtomicUsize>,
}

struct BlockUntilClosedChannel {
    name: String,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Channel for AlwaysFailChannel {
    fn name(&self) -> &str {
        self.name
    }

    async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(
        &self,
        _tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
    ) -> anyhow::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("listen boom")
    }
}

#[async_trait::async_trait]
impl Channel for BlockUntilClosedChannel {
    fn name(&self) -> &str {
        &self.name
    }

    async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(
        &self,
        tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
    ) -> anyhow::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tx.closed().await;
        Ok(())
    }
}

#[tokio::test]
async fn supervised_listener_marks_error_and_restarts_on_failures() {
    let calls = Arc::new(AtomicUsize::new(0));
    let channel: Arc<dyn Channel> = Arc::new(AlwaysFailChannel {
        name: "test-supervised-fail",
        calls: Arc::clone(&calls),
    });

    let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(1);
    let handle = spawn_supervised_listener(channel, tx, 1, 1);

    tokio::time::sleep(Duration::from_millis(80)).await;
    drop(rx);
    handle.abort();
    let _ = handle.await;

    let snapshot = crate::health::snapshot_json();
    let component = &snapshot["components"]["channel:test-supervised-fail"];
    assert_eq!(component["status"], "error");
    assert!(component["restart_count"].as_u64().unwrap_or(0) >= 1);
    assert!(component["last_error"]
        .as_str()
        .unwrap_or("")
        .contains("listen boom"));
    assert!(calls.load(Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn supervised_listener_refreshes_health_while_running() {
    let calls = Arc::new(AtomicUsize::new(0));
    let channel_name = format!("test-supervised-heartbeat-{}", uuid::Uuid::new_v4());
    let component_name = format!("channel:{channel_name}");
    let channel: Arc<dyn Channel> = Arc::new(BlockUntilClosedChannel {
        name: channel_name,
        calls: Arc::clone(&calls),
    });

    let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(1);
    let handle = spawn_supervised_listener_with_health_interval(
        channel,
        tx,
        1,
        1,
        Duration::from_millis(20),
    );

    tokio::time::sleep(Duration::from_millis(35)).await;
    let first_last_ok = crate::health::snapshot_json()["components"][&component_name]["last_ok"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(!first_last_ok.is_empty());

    tokio::time::sleep(Duration::from_millis(70)).await;
    let second_last_ok = crate::health::snapshot_json()["components"][&component_name]["last_ok"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let first = chrono::DateTime::parse_from_rfc3339(&first_last_ok)
        .expect("last_ok should be valid RFC3339");
    let second = chrono::DateTime::parse_from_rfc3339(&second_last_ok)
        .expect("last_ok should be valid RFC3339");
    assert!(second > first, "expected periodic health heartbeat refresh");

    drop(rx);
    let join = tokio::time::timeout(Duration::from_secs(1), handle).await;
    assert!(join.is_ok(), "listener should stop after channel shutdown");
    assert!(calls.load(Ordering::SeqCst) >= 1);
}

