use super::traits::{Channel, ChannelMessage, SendMessage};
pub(crate) mod attachments;
pub(crate) mod cards;
pub(crate) mod delivery;
pub(crate) mod helpers;
pub(crate) mod inbound;
pub(crate) mod media;
pub(crate) mod message_builders;
pub(crate) mod outbound;
pub(crate) mod protocol;
pub(crate) mod runtime;
pub(crate) mod transport;

use self::attachments::{
    classify_lark_outgoing_attachments, parse_lark_attachment_markers,
    parse_lark_path_only_attachment, LarkAttachment, LarkAttachmentKind,
};
use self::cards::{
    build_lark_card_message_body, build_lark_reply_card_message_body, build_lark_streaming_card,
    next_lark_stream_flush_deadline, parse_lark_card_action_event,
    render_lark_card_action_event_content, LarkCardMessage, LarkCardPhase,
};
use self::helpers::{
    parse_post_content_details, random_lark_ack_reaction, should_respond_in_group,
    strip_at_placeholders,
};
use self::inbound::{
    build_lark_dispatch_context, build_lark_normalized_content, parse_lark_inbound_resources,
    parse_lark_text_content, render_lark_fallback_content, LarkInboundResourceKind,
    LarkParsedMessage,
};
use self::media::{
    content_type_from_response, extract_response_file_name, materialize_outbound_attachment,
    store_inbound_resource, store_inbound_resource_with_limit, LarkDownloadedResource,
    LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES,
};
use self::message_builders::{
    build_lark_audio_message_body, build_lark_file_message_body, build_lark_image_message_body,
    build_lark_reply_message_body, build_lark_text_message_body, build_lark_video_message_body,
};
use self::outbound::LarkOutboundRequest;
use self::protocol::{
    ensure_lark_send_success, extract_lark_response_code, extract_lark_token_ttl_seconds,
    is_lark_terminal_message_code, next_token_refresh_deadline, should_refresh_lark_tenant_token,
    should_refresh_last_recv, CachedTenantToken, LarkEvent, LarkHealthProbe, LarkMessage,
    LarkProbeStatus, MsgReceivePayload, PbFrame, PbHeader, WsClientConfig, WsEndpointResp,
    LARK_MESSAGE_UNAVAILABLE_TTL, WS_HEARTBEAT_TIMEOUT,
};
use futures_util::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio_tungstenite::tungstenite::Message as WsMsg;

const FEISHU_BASE_URL: &str = "https://open.feishu.cn/open-apis";
const FEISHU_WS_BASE_URL: &str = "https://open.feishu.cn";
const LARK_BASE_URL: &str = "https://open.larksuite.com/open-apis";
const LARK_WS_BASE_URL: &str = "https://open.larksuite.com";

const LARK_ACK_REACTIONS_ZH_CN: &[&str] = &[
    "OK", "JIAYI", "APPLAUSE", "THUMBSUP", "MUSCLE", "SMILE", "DONE",
];
const LARK_ACK_REACTIONS_ZH_TW: &[&str] = &[
    "OK",
    "JIAYI",
    "APPLAUSE",
    "THUMBSUP",
    "FINGERHEART",
    "SMILE",
    "DONE",
];
const LARK_ACK_REACTIONS_EN: &[&str] = &[
    "OK",
    "THUMBSUP",
    "THANKS",
    "MUSCLE",
    "FINGERHEART",
    "APPLAUSE",
    "SMILE",
    "DONE",
];
const LARK_ACK_REACTIONS_JA: &[&str] = &[
    "OK",
    "THUMBSUP",
    "THANKS",
    "MUSCLE",
    "FINGERHEART",
    "APPLAUSE",
    "SMILE",
    "DONE",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LarkAckLocale {
    ZhCn,
    ZhTw,
    En,
    Ja,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LarkPlatform {
    Lark,
    Feishu,
}

#[derive(Clone)]
struct LarkDraftSession {
    commands: mpsc::UnboundedSender<LarkDraftCommand>,
}

enum LarkDraftCommand {
    Update(String),
    Finalize {
        text: String,
        result_tx: oneshot::Sender<anyhow::Result<()>>,
    },
    Cancel {
        result_tx: oneshot::Sender<anyhow::Result<()>>,
    },
}

impl LarkPlatform {
    fn api_base(self) -> &'static str {
        match self {
            Self::Lark => LARK_BASE_URL,
            Self::Feishu => FEISHU_BASE_URL,
        }
    }

    fn ws_base(self) -> &'static str {
        match self {
            Self::Lark => LARK_WS_BASE_URL,
            Self::Feishu => FEISHU_WS_BASE_URL,
        }
    }

    fn locale_header(self) -> &'static str {
        match self {
            Self::Lark => "en",
            Self::Feishu => "zh",
        }
    }

    fn proxy_service_key(self) -> &'static str {
        match self {
            Self::Lark => "channel.lark",
            Self::Feishu => "channel.feishu",
        }
    }

    fn channel_name(self) -> &'static str {
        match self {
            Self::Lark => "lark",
            Self::Feishu => "feishu",
        }
    }
}

/// Lark/Feishu channel.
///
/// Supports two receive modes (configured via `receive_mode` in config):
/// - **`websocket`** (default): persistent WSS long-connection; no public URL needed.
/// - **`webhook`**: HTTP callback server; requires a public HTTPS endpoint.
#[derive(Clone)]
pub struct LarkChannel {
    name_override: Option<String>,
    account_id: String,
    app_id: String,
    app_secret: String,
    verification_token: String,
    port: Option<u16>,
    allowed_users: Vec<String>,
    /// Bot open_id resolved at runtime via `/bot/v3/info`.
    resolved_bot_open_id: Arc<StdRwLock<Option<String>>>,
    mention_only: bool,
    /// When true, use Feishu (CN) endpoints; when false, use Lark (international).
    use_feishu: bool,
    platform: LarkPlatform,
    /// How to receive events: WebSocket long-connection or HTTP webhook.
    receive_mode: crate::config::schema::LarkReceiveMode,
    /// Cached tenant access token
    tenant_token: Arc<RwLock<Option<CachedTenantToken>>>,
    /// Dedup set: WS message_ids seen in last ~30 min to prevent double-dispatch
    ws_seen_ids: Arc<RwLock<HashMap<String, Instant>>>,
    unavailable_message_ids: Arc<RwLock<HashMap<String, (i64, Instant)>>>,
    draft_sessions: Arc<RwLock<HashMap<String, LarkDraftSession>>>,
    api_base_override: Option<String>,
    ws_base_override: Option<String>,
    workspace_dir: Option<PathBuf>,
}

impl LarkChannel {
    pub(crate) fn normalize_account_reference(raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }

        for prefix in ["feishu:", "lark:"] {
            if trimmed.len() > prefix.len() && trimmed[..prefix.len()].eq_ignore_ascii_case(prefix)
            {
                let account = trimmed[prefix.len()..].trim();
                return (!account.is_empty()).then(|| account.to_string());
            }
        }

        Some(trimmed.to_string())
    }

    fn outbound_resource_kind(kind: LarkAttachmentKind) -> LarkInboundResourceKind {
        match kind {
            LarkAttachmentKind::Image => LarkInboundResourceKind::Image,
            LarkAttachmentKind::Document => LarkInboundResourceKind::File,
            LarkAttachmentKind::Audio => LarkInboundResourceKind::Audio,
            LarkAttachmentKind::Video => LarkInboundResourceKind::Video,
        }
    }

    fn detect_upload_file_type(path: &Path, kind: LarkAttachmentKind) -> &'static str {
        match kind {
            LarkAttachmentKind::Audio => "opus",
            LarkAttachmentKind::Video => "mp4",
            LarkAttachmentKind::Document | LarkAttachmentKind::Image => match path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.to_ascii_lowercase())
                .as_deref()
            {
                Some("pdf") => "pdf",
                Some("doc") | Some("docx") => "doc",
                Some("xls") | Some("xlsx") | Some("csv") => "xls",
                Some("ppt") | Some("pptx") => "ppt",
                _ => "stream",
            },
        }
    }

    pub fn new(
        app_id: String,
        app_secret: String,
        verification_token: String,
        port: Option<u16>,
        allowed_users: Vec<String>,
        mention_only: bool,
    ) -> Self {
        Self::new_with_platform(
            app_id,
            app_secret,
            verification_token,
            port,
            allowed_users,
            mention_only,
            LarkPlatform::Lark,
        )
    }

    fn new_with_platform(
        app_id: String,
        app_secret: String,
        verification_token: String,
        port: Option<u16>,
        allowed_users: Vec<String>,
        mention_only: bool,
        platform: LarkPlatform,
    ) -> Self {
        Self {
            name_override: None,
            account_id: "default".to_string(),
            app_id,
            app_secret,
            verification_token,
            port,
            allowed_users,
            resolved_bot_open_id: Arc::new(StdRwLock::new(None)),
            mention_only,
            use_feishu: matches!(platform, LarkPlatform::Feishu),
            platform,
            receive_mode: crate::config::schema::LarkReceiveMode::default(),
            tenant_token: Arc::new(RwLock::new(None)),
            ws_seen_ids: Arc::new(RwLock::new(HashMap::new())),
            unavailable_message_ids: Arc::new(RwLock::new(HashMap::new())),
            draft_sessions: Arc::new(RwLock::new(HashMap::new())),
            api_base_override: None,
            ws_base_override: None,
            workspace_dir: None,
        }
    }

    /// Build from `LarkConfig` using legacy compatibility:
    /// when `use_feishu=true`, this instance routes to Feishu endpoints.
    pub fn from_config(config: &crate::config::schema::LarkConfig) -> Self {
        let platform = if config.use_feishu {
            LarkPlatform::Feishu
        } else {
            LarkPlatform::Lark
        };
        let mut ch = Self::new_with_platform(
            config.app_id.clone(),
            config.app_secret.clone(),
            config.verification_token.clone().unwrap_or_default(),
            config.port,
            config.allowed_users.clone(),
            config.mention_only,
            platform,
        );
        ch.receive_mode = config.receive_mode.clone();
        ch
    }

    pub fn from_lark_config(config: &crate::config::schema::LarkConfig) -> Self {
        let mut ch = Self::new_with_platform(
            config.app_id.clone(),
            config.app_secret.clone(),
            config.verification_token.clone().unwrap_or_default(),
            config.port,
            config.allowed_users.clone(),
            config.mention_only,
            LarkPlatform::Lark,
        );
        ch.receive_mode = config.receive_mode.clone();
        ch
    }

    pub fn from_feishu_config(config: &crate::config::schema::FeishuConfig) -> Self {
        let mut ch = Self::new_with_platform(
            config.app_id.clone(),
            config.app_secret.clone(),
            config.verification_token.clone().unwrap_or_default(),
            config.port,
            config.allowed_users.clone(),
            false,
            LarkPlatform::Feishu,
        );
        ch.receive_mode = config.receive_mode.clone();
        ch
    }

    pub fn from_named_feishu_config(
        name: String,
        config: &crate::config::schema::FeishuConfig,
    ) -> Self {
        let mut ch = Self::from_feishu_config(config);
        let account_id = Self::normalize_account_reference(&name).unwrap_or(name);
        ch.account_id = account_id.clone();
        ch.name_override = Some(format!("feishu:{account_id}"));
        ch
    }

    pub fn with_workspace_dir(mut self, workspace_dir: Option<PathBuf>) -> Self {
        self.workspace_dir = workspace_dir;
        self
    }

    fn account_id(&self) -> &str {
        &self.account_id
    }

    fn health_component_name(&self) -> String {
        format!("channel:{}", self.channel_name())
    }

    fn receive_mode_label(&self) -> &'static str {
        match self.receive_mode {
            crate::config::schema::LarkReceiveMode::Websocket => "websocket",
            crate::config::schema::LarkReceiveMode::Webhook => "webhook",
        }
    }

    fn http_client(&self) -> reqwest::Client {
        crate::config::build_runtime_proxy_client(self.platform.proxy_service_key())
    }

    fn channel_name(&self) -> &str {
        self.name_override
            .as_deref()
            .unwrap_or_else(|| self.platform.channel_name())
    }

    fn api_base(&self) -> &str {
        self.api_base_override
            .as_deref()
            .unwrap_or_else(|| self.platform.api_base())
    }

    fn ws_base(&self) -> &str {
        self.ws_base_override
            .as_deref()
            .unwrap_or_else(|| self.platform.ws_base())
    }

    fn tenant_access_token_url(&self) -> String {
        format!("{}/auth/v3/tenant_access_token/internal", self.api_base())
    }

    fn bot_info_url(&self) -> String {
        format!("{}/bot/v3/info", self.api_base())
    }

    fn send_message_url(&self) -> String {
        format!("{}/im/v1/messages?receive_id_type=chat_id", self.api_base())
    }

    fn reply_message_url(&self, message_id: &str) -> String {
        format!("{}/im/v1/messages/{message_id}/reply", self.api_base())
    }

    fn upload_image_url(&self) -> String {
        format!("{}/im/v1/images", self.api_base())
    }

    fn upload_file_url(&self) -> String {
        format!("{}/im/v1/files", self.api_base())
    }

    fn message_reaction_url(&self, message_id: &str) -> String {
        format!("{}/im/v1/messages/{message_id}/reactions", self.api_base())
    }

    fn message_patch_url(&self, message_id: &str) -> String {
        format!("{}/im/v1/messages/{message_id}", self.api_base())
    }

    fn delete_message_reaction_url(&self, message_id: &str, reaction_id: &str) -> String {
        format!(
            "{}/im/v1/messages/{message_id}/reactions/{reaction_id}",
            self.api_base()
        )
    }

    fn delete_message_url(&self, message_id: &str) -> String {
        format!("{}/im/v1/messages/{message_id}", self.api_base())
    }

    fn resolved_bot_open_id(&self) -> Option<String> {
        self.resolved_bot_open_id
            .read()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn set_resolved_bot_open_id(&self, open_id: Option<String>) {
        if let Ok(mut guard) = self.resolved_bot_open_id.write() {
            *guard = open_id;
        }
    }

    /// Check if a user open_id is allowed
    fn is_user_allowed(&self, open_id: &str) -> bool {
        self.allowed_users.iter().any(|u| u == "*" || u == open_id)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WS helper functions
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
