use super::traits::{Channel, ChannelMessage, SendMessage};
use aes::Aes256;
use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::Engine as _;
use cbc::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
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
    #[serde(skip_serializing_if = "Vec::is_empty", rename = "msg_item")]
    msg_item: Vec<WeComReplyMsgItem>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct WeComReplyMsgItem {
    msgtype: &'static str,
    image: WeComReplyImageContent,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct WeComReplyImageContent {
    base64: String,
    md5: String,
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
    image: Option<WeComInboundImage>,
    mixed: Option<WeComInboundMixed>,
    quote: Option<WeComInboundQuote>,
}

#[derive(Debug, Deserialize)]
struct WeComInboundFrom {
    userid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WeComInboundText {
    content: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct WeComInboundImage {
    url: Option<String>,
    aeskey: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WeComInboundMixed {
    #[serde(rename = "msg_item")]
    msg_item: Option<Vec<WeComInboundMixedItem>>,
}

#[derive(Debug, Deserialize)]
struct WeComInboundMixedItem {
    msgtype: Option<String>,
    text: Option<WeComInboundText>,
    image: Option<WeComInboundImage>,
}

#[derive(Debug, Deserialize)]
struct WeComInboundQuote {
    msgtype: Option<String>,
    text: Option<WeComInboundText>,
    image: Option<WeComInboundImage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WeComAttachment {
    target: String,
}

#[derive(Debug, Clone)]
struct WeComInboundImageRef {
    url: String,
    aeskey: Option<String>,
}

type Aes256CbcDecryptor = cbc::Decryptor<Aes256>;

pub struct WeComChannel {
    name: String,
    bot_id: String,
    secret: String,
    websocket_url: String,
    allowed_users: Vec<String>,
    workspace_dir: Option<PathBuf>,
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
        workspace_dir: Option<PathBuf>,
    ) -> Self {
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        Self {
            name,
            bot_id,
            secret,
            websocket_url,
            allowed_users,
            workspace_dir,
            outbound_tx,
            outbound_rx: Arc::new(tokio::sync::Mutex::new(outbound_rx)),
        }
    }

    pub fn from_config(config: &crate::config::schema::WeComConfig) -> Self {
        Self::from_named_config("wecom".to_string(), config)
    }

    pub fn from_named_config(name: String, config: &crate::config::schema::WeComConfig) -> Self {
        Self::from_named_config_with_workspace(name, config, None)
    }

    pub fn from_config_with_workspace(
        config: &crate::config::schema::WeComConfig,
        workspace_dir: PathBuf,
    ) -> Self {
        Self::from_named_config_with_workspace("wecom".to_string(), config, Some(workspace_dir))
    }

    pub fn from_named_config_with_workspace(
        name: String,
        config: &crate::config::schema::WeComConfig,
        workspace_dir: Option<PathBuf>,
    ) -> Self {
        Self::new(
            name,
            config.bot_id.clone(),
            config.secret.clone(),
            config.websocket_url.clone(),
            config.allowed_users.clone(),
            workspace_dir,
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
        let raw_content = super::strip_tool_call_tags(content);
        let (cleaned_content, local_images, unresolved_markers) =
            parse_wecom_attachment_markers(&raw_content);
        let rendered_content = compose_outbound_text_content(&cleaned_content, &unresolved_markers);

        if let Some(req_id) = thread_ts {
            let msg_item = build_reply_msg_items(&local_images)?;
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
                        content: rendered_content,
                        msg_item,
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
        if !local_images.is_empty() {
            anyhow::bail!(
                "WeCom proactive send does not support image attachments; use a reply context"
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
                    content: rendered_content,
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
                let incoming = read.next().await.ok_or_else(|| {
                    anyhow::anyhow!("WeCom websocket closed before {phase} ack")
                })??;

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
                        let errmsg = frame
                            .errmsg
                            .unwrap_or_else(|| "unknown WeCom error".to_string());
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
            context: None,
        })
    }

    async fn parse_inbound_message_with_workspace(
        frame: WeComInboundFrame,
        channel_name: &str,
        workspace_dir: Option<&Path>,
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
        let msgtype = body.msgtype.as_deref()?;
        if !matches!(msgtype, "text" | "image" | "mixed") {
            return None;
        }

        let chat_id = body.chatid.clone();
        let sender = body.from.as_ref()?.userid.clone()?;
        let content = match render_inbound_content(&body, workspace_dir).await {
            Ok(Some(content)) => content,
            Ok(None) => return None,
            Err(err) => {
                tracing::warn!("WeCom: failed to resolve inbound content: {err}");
                return None;
            }
        };
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
            context: None,
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

fn parse_wecom_attachment_markers(message: &str) -> (String, Vec<PathBuf>, Vec<String>) {
    let mut cleaned = String::with_capacity(message.len());
    let mut attachments = Vec::new();
    let mut unresolved = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel_start) = message[cursor..].find('[') {
        let start = cursor + rel_start;
        cleaned.push_str(&message[cursor..start]);

        let Some(rel_end) = message[start..].find(']') else {
            cleaned.push_str(&message[start..]);
            cursor = message.len();
            break;
        };
        let end = start + rel_end;
        let marker_text = &message[start + 1..end];
        let parsed = marker_text.split_once(':').and_then(|(kind, target)| {
            if !matches!(kind.trim().to_ascii_uppercase().as_str(), "IMAGE" | "PHOTO") {
                return None;
            }
            let target = target.trim();
            if target.is_empty() {
                return None;
            }
            Some(WeComAttachment {
                target: target.to_string(),
            })
        });

        if let Some(attachment) = parsed {
            let path = Path::new(&attachment.target);
            if path.exists() && path.is_file() {
                attachments.push(path.to_path_buf());
            } else {
                unresolved.push(format!("[IMAGE:{}]", attachment.target));
            }
        } else {
            cleaned.push_str(&message[start..=end]);
        }

        cursor = end + 1;
    }

    if cursor < message.len() {
        cleaned.push_str(&message[cursor..]);
    }

    (cleaned.trim().to_string(), attachments, unresolved)
}

fn compose_outbound_text_content(content: &str, unresolved_markers: &[String]) -> String {
    let mut parts = Vec::new();
    if !content.trim().is_empty() {
        parts.push(content.trim().to_string());
    }
    if !unresolved_markers.is_empty() {
        parts.push(unresolved_markers.join("\n"));
    }
    parts.join("\n")
}

fn build_reply_msg_items(image_paths: &[PathBuf]) -> Result<Vec<WeComReplyMsgItem>> {
    image_paths
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path)
                .with_context(|| format!("read WeCom reply image {}", path.display()))?;
            Ok(WeComReplyMsgItem {
                msgtype: "image",
                image: WeComReplyImageContent {
                    base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    md5: format!("{:x}", md5::compute(&bytes)),
                },
            })
        })
        .collect()
}

fn collect_inbound_parts(body: &WeComInboundBody) -> (Vec<String>, Vec<WeComInboundImageRef>) {
    let mut text_parts = Vec::new();
    let mut image_refs = Vec::new();

    if body.msgtype.as_deref() == Some("mixed") {
        if let Some(items) = body
            .mixed
            .as_ref()
            .and_then(|mixed| mixed.msg_item.as_ref())
        {
            for item in items {
                match item.msgtype.as_deref() {
                    Some("text") => {
                        if let Some(content) =
                            item.text.as_ref().and_then(|text| text.content.as_ref())
                        {
                            let trimmed = content.trim();
                            if !trimmed.is_empty() {
                                text_parts.push(trimmed.to_string());
                            }
                        }
                    }
                    Some("image") => {
                        if let Some(url) = item.image.as_ref().and_then(|image| image.url.as_ref())
                        {
                            image_refs.push(WeComInboundImageRef {
                                url: url.clone(),
                                aeskey: item.image.as_ref().and_then(|image| image.aeskey.clone()),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
    } else {
        if let Some(content) = body.text.as_ref().and_then(|text| text.content.as_ref()) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                text_parts.push(trimmed.to_string());
            }
        }
        if let Some(url) = body.image.as_ref().and_then(|image| image.url.as_ref()) {
            image_refs.push(WeComInboundImageRef {
                url: url.clone(),
                aeskey: body.image.as_ref().and_then(|image| image.aeskey.clone()),
            });
        }
    }

    if let Some(quote) = &body.quote {
        if quote.msgtype.as_deref() == Some("text") {
            if let Some(content) = quote.text.as_ref().and_then(|text| text.content.as_ref()) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    text_parts.push(trimmed.to_string());
                }
            }
        }
        if quote.msgtype.as_deref() == Some("image") {
            if let Some(url) = quote.image.as_ref().and_then(|image| image.url.as_ref()) {
                image_refs.push(WeComInboundImageRef {
                    url: url.clone(),
                    aeskey: quote.image.as_ref().and_then(|image| image.aeskey.clone()),
                });
            }
        }
    }

    (text_parts, image_refs)
}

async fn render_inbound_content(
    body: &WeComInboundBody,
    workspace_dir: Option<&Path>,
) -> Result<Option<String>> {
    let (text_parts, image_refs) = collect_inbound_parts(body);
    let mut blocks = Vec::new();

    for image in image_refs {
        let marker = match workspace_dir {
            Some(workspace_dir) => match download_and_store_image(&image, workspace_dir).await {
                Ok(path) => format!("[IMAGE:{}]", path.display()),
                Err(err) => {
                    tracing::warn!("WeCom: image download failed for {}: {err}", image.url);
                    format!("[IMAGE:{}]", image.url)
                }
            },
            None => format!("[IMAGE:{}]", image.url),
        };
        blocks.push(marker);
    }

    for text in text_parts {
        blocks.push(text);
    }

    if blocks.is_empty() {
        return Ok(None);
    }

    Ok(Some(blocks.join("\n\n")))
}

async fn download_and_store_image(
    image: &WeComInboundImageRef,
    workspace_dir: &Path,
) -> Result<PathBuf> {
    let response = reqwest::get(&image.url)
        .await
        .with_context(|| format!("download WeCom image {}", image.url))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("download WeCom image failed with status {status}");
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("read WeCom image body {}", image.url))?;

    let decrypted = if let Some(aeskey) = image.aeskey.as_deref() {
        decrypt_wecom_file(bytes.as_ref(), aeskey)?
    } else {
        bytes.to_vec()
    };

    let ext =
        detect_image_extension(content_type.as_deref(), &decrypted, &image.url).unwrap_or("png");
    let file_name = format!(
        "wecom_{}_{}.{}",
        current_timestamp_millis(),
        random_suffix(),
        ext
    );
    let output_dir = workspace_dir.join("channels").join("wecom").join("inbound");
    tokio::fs::create_dir_all(&output_dir)
        .await
        .with_context(|| format!("create WeCom inbound image dir {}", output_dir.display()))?;
    let output_path = output_dir.join(file_name);
    tokio::fs::write(&output_path, &decrypted)
        .await
        .with_context(|| format!("write WeCom inbound image {}", output_path.display()))?;
    Ok(output_path)
}

fn decrypt_wecom_file(encrypted: &[u8], aeskey: &str) -> Result<Vec<u8>> {
    let key = base64::engine::general_purpose::STANDARD
        .decode(aeskey)
        .context("decode WeCom aeskey")?;
    if key.len() != 32 {
        anyhow::bail!(
            "invalid WeCom aeskey length: expected 32 bytes, got {}",
            key.len()
        );
    }
    let iv = &key[..16];
    let mut buffer = encrypted.to_vec();
    let decrypted = Aes256CbcDecryptor::new_from_slices(&key, iv)
        .context("initialize WeCom AES-256-CBC decryptor")?
        .decrypt_padded_mut::<NoPadding>(&mut buffer)
        .context("decrypt WeCom image payload")?;
    strip_pkcs7_padding_32(decrypted)
}

fn strip_pkcs7_padding_32(bytes: &[u8]) -> Result<Vec<u8>> {
    let Some(&pad_len) = bytes.last() else {
        anyhow::bail!("empty decrypted WeCom payload");
    };
    let pad_len = usize::from(pad_len);
    if pad_len == 0 || pad_len > 32 || pad_len > bytes.len() {
        anyhow::bail!("invalid WeCom PKCS#7 padding length: {pad_len}");
    }
    if !bytes[bytes.len() - pad_len..]
        .iter()
        .all(|byte| usize::from(*byte) == pad_len)
    {
        anyhow::bail!("invalid WeCom PKCS#7 padding bytes");
    }
    Ok(bytes[..bytes.len() - pad_len].to_vec())
}

fn detect_image_extension(
    content_type: Option<&str>,
    bytes: &[u8],
    url: &str,
) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("jpg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    if bytes.starts_with(b"BM") {
        return Some("bmp");
    }

    if let Some(content_type) = content_type {
        match content_type.split(';').next().unwrap_or_default().trim() {
            "image/png" => return Some("png"),
            "image/jpeg" => return Some("jpg"),
            "image/gif" => return Some("gif"),
            "image/webp" => return Some("webp"),
            "image/bmp" => return Some("bmp"),
            _ => {}
        }
    }

    Path::new(url)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| match ext.to_ascii_lowercase().as_str() {
            "png" => "png",
            "jpg" | "jpeg" => "jpg",
            "gif" => "gif",
            "webp" => "webp",
            "bmp" => "bmp",
            _ => "png",
        })
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
                                        if let Some(message) = Self::parse_inbound_message_with_workspace(
                                            frame,
                                            self.name(),
                                            self.workspace_dir.as_deref(),
                                        ).await {
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
                                        if let Some(message) = Self::parse_inbound_message_with_workspace(
                                            frame,
                                            self.name(),
                                            self.workspace_dir.as_deref(),
                                        ).await {
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
    use std::path::Path;
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
            None,
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
            None,
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

    fn wecom_image_test_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    #[tokio::test]
    async fn parse_inbound_image_message_downloads_to_workspace() {
        let _guard = wecom_image_test_lock().lock().await;
        let image_bytes = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aS1cAAAAASUVORK5CYII=")
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
                image_bytes.len()
            );
            use tokio::io::AsyncWriteExt;
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(&image_bytes).await.unwrap();
        });

        let workspace = tempfile::tempdir().unwrap();
        let frame: WeComInboundFrame = serde_json::from_str(&format!(
            r#"{{
              "cmd":"aibot_callback",
              "headers":{{"req_id":"req-image"}},
              "body":{{
                "msgid":"msg-image",
                "chatid":"chat-image",
                "chattype":"single",
                "from":{{"userid":"zhangsan"}},
                "msgtype":"image",
                "image":{{"url":"http://{addr}/image.png"}}
              }}
            }}"#
        ))
        .unwrap();

        let message = WeComChannel::parse_inbound_message_with_workspace(
            frame,
            "wecom",
            Some(workspace.path()),
        )
        .await
        .expect("parsed message");

        assert_eq!(message.sender, "zhangsan");
        assert_eq!(message.reply_target, "chat:chat-image");
        assert!(message.content.starts_with("[IMAGE:"));
        let image_path = message
            .content
            .strip_prefix("[IMAGE:")
            .and_then(|value| value.strip_suffix(']'))
            .unwrap();
        assert!(
            Path::new(image_path).exists(),
            "downloaded image missing: {image_path}"
        );

        server.await.unwrap();
    }

    #[tokio::test]
    async fn parse_inbound_mixed_message_combines_images_and_text() {
        let _guard = wecom_image_test_lock().lock().await;
        let image_bytes = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aS1cAAAAASUVORK5CYII=")
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
                image_bytes.len()
            );
            use tokio::io::AsyncWriteExt;
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(&image_bytes).await.unwrap();
        });

        let workspace = tempfile::tempdir().unwrap();
        let frame: WeComInboundFrame = serde_json::from_str(&format!(
            r#"{{
              "cmd":"aibot_callback",
              "headers":{{"req_id":"req-mixed"}},
              "body":{{
                "msgid":"msg-mixed",
                "chatid":"group-1",
                "chattype":"group",
                "from":{{"userid":"lisi"}},
                "msgtype":"mixed",
                "mixed":{{
                  "msg_item":[
                    {{"msgtype":"image","image":{{"url":"http://{addr}/mix.png"}}}},
                    {{"msgtype":"text","text":{{"content":"请描述这张图"}}}}
                  ]
                }}
              }}
            }}"#
        ))
        .unwrap();

        let message = WeComChannel::parse_inbound_message_with_workspace(
            frame,
            "wecom",
            Some(workspace.path()),
        )
        .await
        .expect("parsed mixed message");

        assert_eq!(message.reply_target, "group:group-1");
        assert!(message.content.contains("[IMAGE:"));
        assert!(message.content.contains("请描述这张图"));

        server.await.unwrap();
    }

    #[test]
    fn build_send_frame_reply_converts_image_marker_to_msg_item() {
        let workspace = tempfile::tempdir().unwrap();
        let image_path = workspace.path().join("reply.png");
        let image_bytes = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aS1cAAAAASUVORK5CYII=")
            .unwrap();
        std::fs::write(&image_path, image_bytes).unwrap();

        let payload = WeComChannel::build_send_frame(
            &format!("已处理\n[IMAGE:{}]", image_path.display()),
            "group:chat-1",
            Some("req-image-reply"),
        )
        .unwrap();

        let value: serde_json::Value = serde_json::from_str(&payload.payload).unwrap();
        assert_eq!(value["cmd"], "aibot_respond_msg");
        assert_eq!(value["body"]["stream"]["content"], "已处理");
        assert_eq!(value["body"]["stream"]["finish"], true);
        assert_eq!(value["body"]["stream"]["msg_item"][0]["msgtype"], "image");
        assert!(value["body"]["stream"]["msg_item"][0]["image"]["base64"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        assert!(value["body"]["stream"]["msg_item"][0]["image"]["md5"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
    }
}
