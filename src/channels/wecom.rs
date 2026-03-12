use super::traits::{Channel, ChannelMessage, SendMessage};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMsg;
use uuid::Uuid;

const DEFAULT_PING_INTERVAL_SECS: u64 = 20;

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
    outbound_tx: mpsc::UnboundedSender<String>,
    outbound_rx: Arc<StdMutex<Option<mpsc::UnboundedReceiver<String>>>>,
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
            outbound_rx: Arc::new(StdMutex::new(Some(outbound_rx))),
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
                req_id: new_req_id(),
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
                req_id: new_req_id(),
            },
            body: None,
        }
    }

    fn build_send_frame(content: &str, recipient: &str, thread_ts: Option<&str>) -> Result<String> {
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
            return serde_json::to_string(&frame).context("serialize WeCom respond frame");
        }

        let target = Self::parse_recipient_target(recipient)?;
        let chat_id = target
            .chat_id
            .or(target.user_id)
            .ok_or_else(|| anyhow::anyhow!("WeCom proactive send requires a chat target"))?;
        let frame = WeComOutboundFrame {
            cmd: "aibot_send_msg".to_string(),
            headers: WeComHeaders {
                req_id: new_req_id(),
            },
            body: Some(WeComSendMarkdownBody {
                chatid: chat_id,
                msgtype: "markdown",
                markdown: WeComTextContent {
                    content: content.to_string(),
                },
            }),
        };
        serde_json::to_string(&frame).context("serialize WeCom send frame")
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

        let sender = body.from?.userid?;
        let content = body.text?.content?;
        let reply_target = match body.chattype.as_deref() {
            Some("group") => format!("group:{}", body.chatid?),
            _ => format!("user:{sender}"),
        };

        Some(ChannelMessage {
            id: body.msgid.unwrap_or_else(new_req_id),
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
}

#[async_trait]
impl Channel for WeComChannel {
    fn name(&self) -> &str {
        &self.name
    }

    async fn send(&self, message: &SendMessage) -> Result<()> {
        let payload = Self::build_send_frame(
            &message.content,
            &message.recipient,
            message.thread_ts.as_deref(),
        )?;
        self.outbound_tx
            .send(payload)
            .map_err(|_| anyhow::anyhow!("WeCom outbound queue is closed"))?;
        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> Result<()> {
        let mut outbound_rx = self
            .outbound_rx
            .lock()
            .map_err(|_| anyhow::anyhow!("WeCom outbound receiver lock poisoned"))?
            .take()
            .ok_or_else(|| anyhow::anyhow!("WeCom listen already started"))?;

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

            loop {
                tokio::select! {
                    maybe_outbound = outbound_rx.recv() => {
                        match maybe_outbound {
                            Some(payload) => {
                                write.send(WsMsg::Text(payload.into())).await?;
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

fn new_req_id() -> String {
    Uuid::new_v4().to_string()
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
    fn build_send_frame_uses_respond_msg_for_replies() {
        let payload =
            WeComChannel::build_send_frame("pong", "group:chat-1", Some("req-abc")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["cmd"], "aibot_respond_msg");
        assert_eq!(value["headers"]["req_id"], "req-abc");
        assert_eq!(value["body"]["msgtype"], "stream");
        assert_eq!(value["body"]["stream"]["id"], "req-abc");
        assert_eq!(value["body"]["stream"]["finish"], true);
        assert_eq!(value["body"]["stream"]["content"], "pong");
        assert!(value["body"].get("text").is_none());
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
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["cmd"], "aibot_send_msg");
        assert_eq!(value["body"]["chatid"], "chat-1");
        assert_eq!(value["body"]["msgtype"], "markdown");
        assert_eq!(value["body"]["markdown"]["content"], "pong");
    }

    #[test]
    fn build_send_frame_uses_userid_as_chatid_for_proactive_direct_messages() {
        let payload = WeComChannel::build_send_frame("pong", "user:alice", None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["cmd"], "aibot_send_msg");
        assert_eq!(value["body"]["chatid"], "alice");
        assert_eq!(value["body"]["msgtype"], "markdown");
        assert_eq!(value["body"]["markdown"]["content"], "pong");
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
}
