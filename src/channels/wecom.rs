use super::traits::{Channel, ChannelMessage, SendMessage};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMsg;
const DEFAULT_PING_INTERVAL_SECS: u64 = 20;
const PROACTIVE_SEND_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
struct WeComOutboundEnvelope {
    payload: String,
    ack_req_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecipientTarget {
    chat_id: Option<String>,
    user_id: Option<String>,
    chat_type: Option<i32>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct WeComOutboundFrame<T> {
    cmd: String,
    headers: WeComHeaders,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<T>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
struct WeComHeaders {
    req_id: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct WeComSubscribeBody {
    bot_id: String,
    secret: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct WeComReplyStreamBody {
    msgtype: &'static str,
    stream: WeComStreamContent,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct WeComStreamContent {
    id: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    finish: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    content: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct WeComSendMarkdownBody {
    chatid: String,
    msgtype: &'static str,
    markdown: WeComTextContent,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct WeComTextContent {
    content: String,
}

#[derive(Debug, Deserialize)]
struct WeComInboundFrame {
    cmd: Option<String>,
    headers: Option<WeComHeaders>,
    body: Option<WeComInboundBody>,
    errcode: Option<i32>,
    errmsg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WeComInboundBody {
    msgid: Option<String>,
    chatid: Option<String>,
    chattype: Option<String>,
    from: Option<WeComInboundFrom>,
    msgtype: Option<String>,
    text: Option<WeComInboundText>,
}

#[derive(Debug, Deserialize)]
struct WeComInboundFrom {
    userid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WeComInboundText {
    content: Option<String>,
}

pub struct WeComChannel {
    name: String,
    bot_id: String,
    secret: String,
    websocket_url: String,
    allowed_users: Vec<String>,
    outbound_tx: mpsc::UnboundedSender<WeComOutboundEnvelope>,
    outbound_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<WeComOutboundEnvelope>>>,
}

impl WeComChannel {
    pub fn new(
        name: String,
        bot_id: String,
        secret: String,
        websocket_url: String,
        allowed_users: Vec<String>,
    ) -> Self {
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        Self {
            name,
            bot_id,
            secret,
            websocket_url,
            allowed_users,
            outbound_tx,
            outbound_rx: Arc::new(tokio::sync::Mutex::new(outbound_rx)),
        }
    }

    pub fn from_config(config: &crate::config::schema::WeComConfig) -> Self {
        Self::from_named_config("wecom".to_string(), config)
    }

    pub fn from_named_config(name: String, config: &crate::config::schema::WeComConfig) -> Self {
        Self::new(
            name,
            config.bot_id.clone(),
            config.secret.clone(),
            config.websocket_url.clone(),
            config.allowed_users.clone(),
        )
    }

    fn build_subscribe_frame(bot_id: &str, secret: &str) -> WeComOutboundFrame<WeComSubscribeBody> {
        WeComOutboundFrame {
            cmd: "aibot_subscribe".to_string(),
            headers: WeComHeaders {
                req_id: new_req_id("aibot_subscribe"),
            },
            body: Some(WeComSubscribeBody {
                bot_id: bot_id.to_string(),
                secret: secret.to_string(),
            }),
        }
    }

    fn build_ping_frame() -> WeComOutboundFrame<()> {
        WeComOutboundFrame {
            cmd: "ping".to_string(),
            headers: WeComHeaders {
                req_id: new_req_id("ping"),
            },
            body: None,
        }
    }

    fn build_send_frame(
        content: &str,
        recipient: &str,
        thread_ts: Option<&str>,
    ) -> Result<WeComOutboundEnvelope> {
        if let Some(req_id) = thread_ts {
            let frame = WeComOutboundFrame {
                cmd: "aibot_respond_msg".to_string(),
                headers: WeComHeaders {
                    req_id: req_id.to_string(),
                },
                body: Some(WeComReplyStreamBody {
                    msgtype: "stream",
                    stream: WeComStreamContent {
                        id: req_id.to_string(),
                        finish: true,
                        content: content.to_string(),
                    },
                }),
            };
            return Ok(WeComOutboundEnvelope {
                payload: serde_json::to_string(&frame).context("serialize WeCom respond frame")?,
                ack_req_id: Some(req_id.to_string()),
            });
        }

        let target = Self::parse_recipient_target(recipient)?;
        if target.user_id.is_some() {
            anyhow::bail!(
                "WeCom proactive send requires chat:<chatid> or group:<chatid>; user:<userid> is only valid for replying to inbound messages"
            );
        }
        let chat_id = target
            .chat_id
            .or(target.user_id)
            .ok_or_else(|| anyhow::anyhow!("WeCom proactive send requires a chat target"))?;
        let frame = WeComOutboundFrame {
            cmd: "aibot_send_msg".to_string(),
            headers: WeComHeaders {
                req_id: new_req_id("aibot_send_msg"),
            },
            body: Some(WeComSendMarkdownBody {
                chatid: chat_id,
                msgtype: "markdown",
                markdown: WeComTextContent {
                    content: content.to_string(),
                },
            }),
        };
        Ok(WeComOutboundEnvelope {
            payload: serde_json::to_string(&frame).context("serialize WeCom send frame")?,
            ack_req_id: None,
        })
    }

    async fn send_proactive_message(&self, message: &SendMessage) -> Result<()> {
        let envelope = Self::build_send_frame(&message.content, &message.recipient, None)?;
        let outbound_req_id = Self::extract_req_id(&envelope.payload)?;

        let (stream, _) = connect_async(self.websocket_url.as_str())
            .await
            .with_context(|| format!("connect WeCom websocket {}", self.websocket_url))?;
        let (mut write, mut read) = stream.split();

        let subscribe_frame = Self::build_subscribe_frame(&self.bot_id, &self.secret);
        let subscribe_req_id = subscribe_frame.headers.req_id.clone();
        let subscribe_payload =
            serde_json::to_string(&subscribe_frame).context("serialize WeCom subscribe frame")?;
        write.send(WsMsg::Text(subscribe_payload.into())).await?;
        Self::await_ack(&mut read, &subscribe_req_id, "subscribe").await?;

        write.send(WsMsg::Text(envelope.payload.into())).await?;
        Self::await_ack(&mut read, &outbound_req_id, "send").await?;
        Ok(())
    }

    fn extract_req_id(payload: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct ReqIdEnvelope {
            headers: WeComHeaders,
        }

        let frame: ReqIdEnvelope =
            serde_json::from_str(payload).context("parse WeCom outbound payload req_id")?;
        Ok(frame.headers.req_id)
    }

    async fn await_ack<S>(read: &mut S, req_id: &str, phase: &str) -> Result<()>
    where
        S: futures_util::Stream<
                Item = std::result::Result<WsMsg, tokio_tungstenite::tungstenite::Error>,
            > + Unpin,
    {
        timeout(Duration::from_secs(PROACTIVE_SEND_TIMEOUT_SECS), async {
            loop {
                let incoming = read
                    .next()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("WeCom websocket closed before {phase} ack"))??;

                let frame = match incoming {
                    WsMsg::Text(text) => serde_json::from_str::<WeComInboundFrame>(text.as_ref())
                        .context("parse WeCom ack frame")?,
                    WsMsg::Binary(bytes) => serde_json::from_slice::<WeComInboundFrame>(&bytes)
                        .context("parse WeCom binary ack frame")?,
                    WsMsg::Ping(_) | WsMsg::Pong(_) | WsMsg::Frame(_) => continue,
                    WsMsg::Close(_) => {
                        anyhow::bail!("WeCom websocket closed before {phase} ack")
                    }
                };

                let incoming_req_id = frame
                    .headers
                    .as_ref()
                    .map(|headers| headers.req_id.as_str())
                    .unwrap_or_default();
                if incoming_req_id != req_id {
                    continue;
                }

                match frame.errcode {
                    Some(0) => return Ok(()),
                    Some(code) => {
                        let errmsg = frame.errmsg.unwrap_or_else(|| "unknown WeCom error".to_string());
                        anyhow::bail!("WeCom {phase} failed ({code}): {errmsg}");
                    }
                    None => continue,
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("WeCom {phase} timed out waiting for ack"))?
    }

    fn parse_recipient_target(recipient: &str) -> Result<RecipientTarget> {
        let trimmed = recipient.trim();
        if trimmed.is_empty() {
            anyhow::bail!("WeCom recipient is empty");
        }

        if let Some(chat_id) = trimmed.strip_prefix("group:") {
            return Ok(RecipientTarget {
                chat_id: Some(chat_id.to_string()),
                user_id: None,
                chat_type: Some(2),
            });
        }
        if let Some(chat_id) = trimmed.strip_prefix("chat:") {
            return Ok(RecipientTarget {
                chat_id: Some(chat_id.to_string()),
                user_id: None,
                chat_type: Some(1),
            });
        }
        if let Some(user_id) = trimmed.strip_prefix("user:") {
            return Ok(RecipientTarget {
                chat_id: None,
                user_id: Some(user_id.to_string()),
                chat_type: None,
            });
        }

        Ok(RecipientTarget {
            chat_id: Some(trimmed.to_string()),
            user_id: None,
            chat_type: Some(1),
        })
    }

    fn parse_inbound_message(
        frame: WeComInboundFrame,
        channel_name: &str,
    ) -> Option<ChannelMessage> {
        if frame.errcode.is_some() {
            return None;
        }
        let cmd = frame.cmd.as_deref()?;
        if cmd != "aibot_msg_callback" && cmd != "aibot_callback" {
            return None;
        }

        let headers = frame.headers?;
        let body = frame.body?;
        if body.msgtype.as_deref()? != "text" {
            return None;
        }

        let chat_id = body.chatid.clone();
        let sender = body.from?.userid?;
        let content = body.text?.content?;
        let reply_target = match body.chattype.as_deref() {
            Some("group") => format!("group:{}", chat_id?),
            _ => chat_id
                .map(|chat_id| format!("chat:{chat_id}"))
                .unwrap_or_else(|| format!("user:{sender}")),
        };

        Some(ChannelMessage {
            id: body.msgid.unwrap_or_else(|| new_req_id("msg")),
            sender,
            reply_target,
            content,
            channel: channel_name.to_string(),
            timestamp: current_timestamp_secs(),
            thread_ts: Some(headers.req_id),
        })
    }

    fn allows_sender(&self, sender: &str) -> bool {
        self.allowed_users.is_empty()
            || self.allowed_users.iter().any(|allowed| {
                allowed == "*"
                    || allowed.eq_ignore_ascii_case(sender)
                    || allowed == &format!("user:{sender}")
            })
    }

    fn enqueue_outbound(
        pending_replies: &mut HashMap<String, VecDeque<String>>,
        in_flight_replies: &mut HashMap<String, String>,
        envelope: WeComOutboundEnvelope,
    ) -> Option<String> {
        match envelope.ack_req_id {
            Some(reply_req_id) => {
                let queue = pending_replies.entry(reply_req_id.clone()).or_default();
                queue.push_back(envelope.payload);
                if in_flight_replies.contains_key(&reply_req_id) {
                    None
                } else {
                    let next = queue.pop_front()?;
                    in_flight_replies.insert(reply_req_id, next.clone());
                    Some(next)
                }
            }
            None => Some(envelope.payload),
        }
    }

    fn release_reply_ack(
        pending_replies: &mut HashMap<String, VecDeque<String>>,
        in_flight_replies: &mut HashMap<String, String>,
        req_id: &str,
    ) -> Option<String> {
        in_flight_replies.remove(req_id)?;
        let queue = pending_replies.get_mut(req_id)?;
        let next = queue.pop_front();
        match next {
            Some(payload) => {
                in_flight_replies.insert(req_id.to_string(), payload.clone());
                Some(payload)
            }
            None => {
                pending_replies.remove(req_id);
                None
            }
        }
    }
}

#[async_trait]
impl Channel for WeComChannel {
    fn name(&self) -> &str {
        &self.name
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        if message.thread_ts.is_none() {
            return self.send_proactive_message(message).await;
        }

        let envelope = Self::build_send_frame(
            &message.content,
            &message.recipient,
            message.thread_ts.as_deref(),
        )?;
        self.outbound_tx
            .send(envelope)
            .map_err(|_| anyhow::anyhow!("WeCom outbound queue is closed"))?;
        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> Result<()> {
        let mut outbound_rx = self
            .outbound_rx
            .try_lock()
            .map_err(|_| anyhow::anyhow!("WeCom listen already started"))?;

        loop {
            tracing::info!("WeCom: connecting to {}", self.websocket_url);
            let (stream, _) = connect_async(self.websocket_url.as_str())
                .await
                .with_context(|| format!("connect WeCom websocket {}", self.websocket_url))?;
            let (mut write, mut read) = stream.split();
            tracing::info!("WeCom: websocket connected");

            let subscribe =
                serde_json::to_string(&Self::build_subscribe_frame(&self.bot_id, &self.secret))?;
            write.send(WsMsg::Text(subscribe.into())).await?;
            tracing::info!("WeCom: subscribe frame sent");

            let mut ping = tokio::time::interval(Duration::from_secs(DEFAULT_PING_INTERVAL_SECS));
            ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut pending_replies: HashMap<String, VecDeque<String>> = HashMap::new();
            let mut in_flight_replies: HashMap<String, String> = HashMap::new();

            loop {
                tokio::select! {
                    maybe_outbound = outbound_rx.recv() => {
                        match maybe_outbound {
                            Some(envelope) => {
                                if let Some(payload) = Self::enqueue_outbound(
                                    &mut pending_replies,
                                    &mut in_flight_replies,
                                    envelope,
                                ) {
                                    write.send(WsMsg::Text(payload.into())).await?;
                                }
                            }
                            None => return Ok(()),
                        }
                    }
                    _ = ping.tick() => {
                        let payload = serde_json::to_string(&Self::build_ping_frame())?;
                        write.send(WsMsg::Text(payload.into())).await?;
                    }
                    incoming = read.next() => {
                        match incoming {
                            Some(Ok(WsMsg::Text(text))) => {
                                match serde_json::from_str::<WeComInboundFrame>(text.as_ref()) {
                                    Ok(frame) => {
                                        let ack_req_id = frame.headers.as_ref().map(|h| h.req_id.clone());
                                        if frame.errcode == Some(0) {
                                            tracing::debug!(
                                                "WeCom: received ack req_id={}",
                                                frame.headers.as_ref().map(|h| h.req_id.as_str()).unwrap_or("")
                                            );
                                        } else if let Some(errcode) = frame.errcode {
                                            tracing::warn!(
                                                "WeCom: received error errcode={} errmsg={} req_id={}",
                                                errcode,
                                                frame.errmsg.as_deref().unwrap_or(""),
                                                frame.headers.as_ref().map(|h| h.req_id.as_str()).unwrap_or("")
                                            );
                                        }
                                        if let Some(req_id) = ack_req_id.as_deref() {
                                            if let Some(next_payload) = Self::release_reply_ack(
                                                &mut pending_replies,
                                                &mut in_flight_replies,
                                                req_id,
                                            ) {
                                                write.send(WsMsg::Text(next_payload.into())).await?;
                                            }
                                        }
                                        if let Some(message) = Self::parse_inbound_message(frame, self.name()) {
                                            if !self.allows_sender(&message.sender) {
                                                tracing::warn!("WeCom: ignoring {} (not in allowed_users)", message.sender);
                                                continue;
                                            }
                                            tx.send(message)
                                                .await
                                                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                        }
                                    }
                                    Err(err) => tracing::debug!("WeCom: failed to parse inbound frame: {err}"),
                                }
                            }
                            Some(Ok(WsMsg::Binary(bytes))) => {
                                if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                                    if let Ok(frame) = serde_json::from_str::<WeComInboundFrame>(&text) {
                                        if let Some(message) = Self::parse_inbound_message(frame, self.name()) {
                                            if !self.allows_sender(&message.sender) {
                                                tracing::warn!("WeCom: ignoring {} (not in allowed_users)", message.sender);
                                                continue;
                                            }
                                            tx.send(message)
                                                .await
                                                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                        }
                                    }
                                }
                            }
                            Some(Ok(WsMsg::Ping(payload))) => {
                                write.send(WsMsg::Pong(payload)).await?;
                            }
                            Some(Ok(WsMsg::Pong(_))) => {}
                            Some(Ok(WsMsg::Frame(_))) => {}
                            Some(Ok(WsMsg::Close(_))) => break,
                            Some(Err(err)) => {
                                tracing::warn!("WeCom: websocket read failed: {err}");
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn health_check(&self) -> bool {
        !self.bot_id.trim().is_empty() && !self.secret.trim().is_empty()
    }
}

fn new_req_id(prefix: &str) -> String {
    format!(
        "{}_{}_{}",
        prefix,
        current_timestamp_millis(),
        random_suffix()
    )
}

fn current_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn random_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{:08x}", nanos)
}

fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    #[test]
    fn build_subscribe_frame_contains_bot_credentials() {
        let frame = WeComChannel::build_subscribe_frame("bot-1", "secret-1");
        assert_eq!(frame.cmd, "aibot_subscribe");
        let body = frame.body.expect("subscribe body");
        assert_eq!(body.bot_id, "bot-1");
        assert_eq!(body.secret, "secret-1");
        assert!(!frame.headers.req_id.is_empty());
    }

    #[test]
    fn parse_inbound_group_text_message_uses_group_reply_target_and_req_id() {
        let frame: WeComInboundFrame = serde_json::from_str(
            r#"{
              "cmd":"aibot_callback",
              "headers":{"req_id":"req-123"},
              "body":{
                "msgid":"msg-1",
                "chatid":"chat-1",
                "chattype":"group",
                "from":{"userid":"zhangsan"},
                "msgtype":"text",
                "text":{"content":"hello"}
              }
            }"#,
        )
        .unwrap();

        let message = WeComChannel::parse_inbound_message(frame, "wecom").expect("parsed message");
        assert_eq!(message.id, "msg-1");
        assert_eq!(message.sender, "zhangsan");
        assert_eq!(message.reply_target, "group:chat-1");
        assert_eq!(message.content, "hello");
        assert_eq!(message.thread_ts.as_deref(), Some("req-123"));
    }

    #[test]
    fn parse_inbound_direct_text_message_uses_chat_reply_target_and_req_id() {
        let frame: WeComInboundFrame = serde_json::from_str(
            r#"{
              "cmd":"aibot_callback",
              "headers":{"req_id":"req-direct"},
              "body":{
                "msgid":"msg-direct",
                "chatid":"chat-direct-1",
                "chattype":"single",
                "from":{"userid":"zhangsan"},
                "msgtype":"text",
                "text":{"content":"hello"}
              }
            }"#,
        )
        .unwrap();

        let message = WeComChannel::parse_inbound_message(frame, "wecom").expect("parsed message");
        assert_eq!(message.reply_target, "chat:chat-direct-1");
        assert_eq!(message.thread_ts.as_deref(), Some("req-direct"));
    }

    #[test]
    fn build_send_frame_uses_respond_msg_for_replies() {
        let payload =
            WeComChannel::build_send_frame("pong", "group:chat-1", Some("req-abc")).unwrap();
        assert_eq!(payload.ack_req_id.as_deref(), Some("req-abc"));
        let value: serde_json::Value = serde_json::from_str(&payload.payload).unwrap();
        assert_eq!(value["cmd"], "aibot_respond_msg");
        assert_eq!(value["headers"]["req_id"], "req-abc");
        assert_eq!(value["body"]["msgtype"], "stream");
        assert_eq!(value["body"]["stream"]["id"], "req-abc");
        assert_eq!(value["body"]["stream"]["finish"], true);
        assert_eq!(value["body"]["stream"]["content"], "pong");
        assert!(value["body"].get("text").is_none());
    }

    #[test]
    fn enqueue_outbound_serializes_replies_per_req_id() {
        let mut pending = HashMap::new();
        let mut inflight = HashMap::new();

        let first = WeComOutboundEnvelope {
            payload: "first".to_string(),
            ack_req_id: Some("req-abc".to_string()),
        };
        let second = WeComOutboundEnvelope {
            payload: "second".to_string(),
            ack_req_id: Some("req-abc".to_string()),
        };

        assert_eq!(
            WeComChannel::enqueue_outbound(&mut pending, &mut inflight, first),
            Some("first".to_string())
        );
        assert_eq!(
            WeComChannel::enqueue_outbound(&mut pending, &mut inflight, second),
            None
        );
        assert_eq!(
            WeComChannel::release_reply_ack(&mut pending, &mut inflight, "req-abc"),
            Some("second".to_string())
        );
        assert_eq!(
            WeComChannel::release_reply_ack(&mut pending, &mut inflight, "req-abc"),
            None
        );
    }

    #[test]
    fn parse_inbound_message_supports_legacy_callback_cmd() {
        let frame: WeComInboundFrame = serde_json::from_str(
            r#"{
              "cmd":"aibot_msg_callback",
              "headers":{"req_id":"req-legacy"},
              "body":{
                "msgid":"msg-legacy",
                "chatid":"chat-legacy",
                "chattype":"group",
                "from":{"userid":"lisi"},
                "msgtype":"text",
                "text":{"content":"legacy"}
              }
            }"#,
        )
        .unwrap();

        let message =
            WeComChannel::parse_inbound_message(frame, "wecom:ops").expect("parsed legacy message");
        assert_eq!(message.id, "msg-legacy");
        assert_eq!(message.channel, "wecom:ops");
        assert_eq!(message.thread_ts.as_deref(), Some("req-legacy"));
    }

    #[test]
    fn build_send_frame_uses_send_msg_for_proactive_group_messages() {
        let payload = WeComChannel::build_send_frame("pong", "group:chat-1", None).unwrap();
        assert!(payload.ack_req_id.is_none());
        let value: serde_json::Value = serde_json::from_str(&payload.payload).unwrap();
        assert_eq!(value["cmd"], "aibot_send_msg");
        assert_eq!(value["body"]["chatid"], "chat-1");
        assert_eq!(value["body"]["msgtype"], "markdown");
        assert_eq!(value["body"]["markdown"]["content"], "pong");
    }

    #[test]
    fn build_send_frame_uses_userid_as_chatid_for_proactive_direct_messages() {
        let err = WeComChannel::build_send_frame("pong", "user:alice", None).unwrap_err();
        assert!(err
            .to_string()
            .contains("WeCom proactive send requires chat:<chatid> or group:<chatid>"));
    }

    #[tokio::test]
    async fn proactive_send_connects_subscribes_and_waits_for_ack() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();

            let subscribe = ws.next().await.unwrap().unwrap();
            let subscribe_text = match subscribe {
                WsMsg::Text(text) => text,
                other => panic!("unexpected subscribe frame: {other:?}"),
            };
            let subscribe_json: serde_json::Value =
                serde_json::from_str(subscribe_text.as_ref()).unwrap();
            assert_eq!(subscribe_json["cmd"], "aibot_subscribe");
            let subscribe_req_id = subscribe_json["headers"]["req_id"]
                .as_str()
                .unwrap()
                .to_string();
            ws.send(WsMsg::Text(
                serde_json::json!({
                    "headers": {"req_id": subscribe_req_id},
                    "errcode": 0
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();

            let outbound = ws.next().await.unwrap().unwrap();
            let outbound_text = match outbound {
                WsMsg::Text(text) => text,
                other => panic!("unexpected outbound frame: {other:?}"),
            };
            let outbound_json: serde_json::Value =
                serde_json::from_str(outbound_text.as_ref()).unwrap();
            assert_eq!(outbound_json["cmd"], "aibot_send_msg");
            assert_eq!(outbound_json["body"]["chatid"], "chat-direct-1");
            assert_eq!(outbound_json["body"]["markdown"]["content"], "hello");
            let outbound_req_id = outbound_json["headers"]["req_id"]
                .as_str()
                .unwrap()
                .to_string();
            ws.send(WsMsg::Text(
                serde_json::json!({
                    "headers": {"req_id": outbound_req_id},
                    "errcode": 0
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        });

        let channel = WeComChannel::new(
            "wecom".into(),
            "bot-1".into(),
            "secret-1".into(),
            format!("ws://{addr}"),
            vec!["*".into()],
        );

        channel
            .send(&SendMessage::new("hello", "chat:chat-direct-1"))
            .await
            .unwrap();

        server.await.unwrap();
    }

    #[tokio::test]
    async fn listen_can_be_restarted_after_connection_failure() {
        let channel = WeComChannel::new(
            "wecom:test".into(),
            "bot-1".into(),
            "secret-1".into(),
            "ws://127.0.0.1:9".into(),
            vec!["*".into()],
        );
        let (tx, _rx) = tokio::sync::mpsc::channel(1);

        let first = channel.listen(tx.clone()).await.unwrap_err();
        assert!(first.to_string().contains("connect WeCom websocket"));

        let second = channel.listen(tx).await.unwrap_err();
        assert!(
            second.to_string().contains("connect WeCom websocket"),
            "listen restart should retry websocket connect, got: {second}"
        );
    }

    #[test]
    fn parse_recipient_target_supports_user_and_group_prefixes() {
        let group = WeComChannel::parse_recipient_target("group:chat-1").unwrap();
        assert_eq!(group.chat_id.as_deref(), Some("chat-1"));
        assert_eq!(group.chat_type, Some(2));

        let user = WeComChannel::parse_recipient_target("user:alice").unwrap();
        assert_eq!(user.user_id.as_deref(), Some("alice"));
        assert_eq!(user.chat_id, None);
    }

    #[test]
    fn new_req_id_uses_sdk_like_prefix_format() {
        let req_id = new_req_id("aibot_send_msg");
        assert!(req_id.starts_with("aibot_send_msg_"));
        assert!(req_id.split('_').count() >= 4);
    }
}
