#[derive(Default)]
pub(super) struct RecordingChannel {
    pub(super) sent_messages: tokio::sync::Mutex<Vec<String>>,
    pub(super) start_typing_calls: AtomicUsize,
    pub(super) stop_typing_calls: AtomicUsize,
    pub(super) reactions_added: tokio::sync::Mutex<Vec<(String, String, String)>>,
    pub(super) reactions_removed: tokio::sync::Mutex<Vec<(String, String, String)>>,
}

#[derive(Default)]
pub(super) struct RecordingFeishuChannel {
    pub(super) sent_messages: tokio::sync::Mutex<Vec<String>>,
    pub(super) sent_threads: tokio::sync::Mutex<Vec<Option<String>>>,
}

#[derive(Default)]
pub(super) struct TelegramRecordingChannel {
    pub(super) sent_messages: tokio::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Channel for TelegramRecordingChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        self.sent_messages
            .lock()
            .await
            .push(format!("{}:{}", message.recipient, message.content));
        Ok(())
    }

    async fn listen(
        &self,
        _tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn start_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn stop_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl Channel for RecordingChannel {
    fn name(&self) -> &str {
        "test-channel"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        self.sent_messages
            .lock()
            .await
            .push(format!("{}:{}", message.recipient, message.content));
        Ok(())
    }

    async fn listen(
        &self,
        _tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn start_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        self.start_typing_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn stop_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        self.stop_typing_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn add_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        self.reactions_added.lock().await.push((
            channel_id.to_string(),
            message_id.to_string(),
            emoji.to_string(),
        ));
        Ok(())
    }

    async fn remove_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        self.reactions_removed.lock().await.push((
            channel_id.to_string(),
            message_id.to_string(),
            emoji.to_string(),
        ));
        Ok(())
    }
}

#[async_trait::async_trait]
impl Channel for RecordingFeishuChannel {
    fn name(&self) -> &str {
        "feishu"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        self.sent_messages
            .lock()
            .await
            .push(format!("{}:{}", message.recipient, message.content));
        self.sent_threads.lock().await.push(message.thread_ts.clone());
        Ok(())
    }

    async fn listen(
        &self,
        _tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}
