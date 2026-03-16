use super::*;

// Feishu WebSocket long-connection: pbbp2.proto frame codec

#[derive(Clone, PartialEq, prost::Message)]
pub(super) struct PbHeader {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

/// Feishu WS frame (pbbp2.proto).
/// method=0 -> CONTROL (ping/pong)  method=1 -> DATA (events)
#[derive(Clone, PartialEq, prost::Message)]
pub(super) struct PbFrame {
    #[prost(uint64, tag = "1")]
    pub seq_id: u64,
    #[prost(uint64, tag = "2")]
    pub log_id: u64,
    #[prost(int32, tag = "3")]
    pub service: i32,
    #[prost(int32, tag = "4")]
    pub method: i32,
    #[prost(message, repeated, tag = "5")]
    pub headers: Vec<PbHeader>,
    #[prost(bytes = "vec", optional, tag = "8")]
    pub payload: Option<Vec<u8>>,
}

impl PbFrame {
    pub(super) fn header_value<'a>(&'a self, key: &str) -> &'a str {
        self.headers
            .iter()
            .find(|h| h.key == key)
            .map(|h| h.value.as_str())
            .unwrap_or("")
    }
}

#[derive(Debug, serde::Deserialize, Default, Clone)]
pub(super) struct WsClientConfig {
    #[serde(rename = "PingInterval")]
    pub(super) ping_interval: Option<u64>,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct WsEndpointResp {
    pub(super) code: i32,
    #[serde(default)]
    pub(super) msg: Option<String>,
    #[serde(default)]
    pub(super) data: Option<WsEndpoint>,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct WsEndpoint {
    #[serde(rename = "URL")]
    pub(super) url: String,
    #[serde(rename = "ClientConfig")]
    pub(super) client_config: Option<WsClientConfig>,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct LarkEvent {
    pub(super) header: LarkEventHeader,
    pub(super) event: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct LarkEventHeader {
    pub(super) event_type: String,
    #[allow(dead_code)]
    pub(super) event_id: String,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct MsgReceivePayload {
    pub(super) sender: LarkSender,
    pub(super) message: LarkMessage,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct LarkSender {
    pub(super) sender_id: LarkSenderId,
    #[serde(default)]
    pub(super) sender_type: String,
}

#[derive(Debug, serde::Deserialize, Default)]
pub(super) struct LarkSenderId {
    pub(super) open_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct LarkMessage {
    #[serde(default)]
    pub(super) message_id: String,
    #[serde(default)]
    pub(super) chat_id: String,
    #[serde(default)]
    pub(super) chat_type: String,
    #[serde(default)]
    pub(super) message_type: String,
    #[serde(default)]
    pub(super) content: String,
    #[serde(default)]
    pub(super) mentions: Vec<serde_json::Value>,
    #[serde(default)]
    pub(super) root_id: Option<String>,
    #[serde(default)]
    pub(super) parent_id: Option<String>,
    #[serde(default)]
    pub(super) thread_id: Option<String>,
    #[serde(default)]
    pub(super) create_time: Option<String>,
}

/// Heartbeat timeout for WS connection; must be larger than ping_interval.
pub(super) const WS_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(300);
/// Refresh tenant token this many seconds before expiry.
pub(super) const LARK_TOKEN_REFRESH_SKEW: Duration = Duration::from_secs(120);
/// Fallback tenant token TTL when `expire`/`expires_in` is absent.
pub(super) const LARK_DEFAULT_TOKEN_TTL: Duration = Duration::from_secs(7200);
/// Feishu/Lark API business code for expired/invalid tenant access token.
pub(super) const LARK_INVALID_ACCESS_TOKEN_CODE: i64 = 99_991_663;
/// Feishu/Lark API business code for recalled message.
pub(super) const LARK_MESSAGE_RECALLED_CODE: i64 = 230_011;
/// Feishu/Lark API business code for deleted message.
pub(super) const LARK_MESSAGE_DELETED_CODE: i64 = 231_003;
/// Cache TTL for messages known to be unavailable.
pub(super) const LARK_MESSAGE_UNAVAILABLE_TTL: Duration = Duration::from_secs(30 * 60);

pub(super) fn should_refresh_last_recv(msg: &WsMsg) -> bool {
    matches!(msg, WsMsg::Binary(_) | WsMsg::Ping(_) | WsMsg::Pong(_))
}

#[derive(Debug, Clone)]
pub(super) struct CachedTenantToken {
    pub(super) value: String,
    pub(super) refresh_after: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LarkProbeStatus {
    Ok,
    Error,
    Skipped,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(super) struct LarkHealthProbe {
    pub(super) probe_kind: &'static str,
    pub(super) platform: &'static str,
    pub(super) account_id: String,
    pub(super) receive_mode: &'static str,
    pub(super) owner_policy_disposition: &'static str,
    pub(super) owner_policy_summary: String,
    pub(super) config_status: LarkProbeStatus,
    pub(super) token_status: LarkProbeStatus,
    pub(super) transport_status: LarkProbeStatus,
    pub(super) bot_identity_status: LarkProbeStatus,
    pub(super) summary: String,
}

impl LarkHealthProbe {
    pub(super) fn is_healthy(&self) -> bool {
        self.config_status == LarkProbeStatus::Ok
            && self.token_status == LarkProbeStatus::Ok
            && self.transport_status != LarkProbeStatus::Error
            && self.bot_identity_status != LarkProbeStatus::Error
    }
}

pub(super) fn extract_lark_response_code(body: &serde_json::Value) -> Option<i64> {
    body.get("code").and_then(|c| c.as_i64())
}

fn is_lark_invalid_access_token(body: &serde_json::Value) -> bool {
    extract_lark_response_code(body) == Some(LARK_INVALID_ACCESS_TOKEN_CODE)
}

pub(super) fn should_refresh_lark_tenant_token(
    status: reqwest::StatusCode,
    body: &serde_json::Value,
) -> bool {
    status == reqwest::StatusCode::UNAUTHORIZED || is_lark_invalid_access_token(body)
}

pub(super) fn is_lark_terminal_message_code(code: i64) -> bool {
    matches!(code, LARK_MESSAGE_RECALLED_CODE | LARK_MESSAGE_DELETED_CODE)
}

pub(super) fn extract_lark_token_ttl_seconds(body: &serde_json::Value) -> u64 {
    let ttl = body
        .get("expire")
        .or_else(|| body.get("expires_in"))
        .and_then(|v| v.as_u64())
        .or_else(|| {
            body.get("expire")
                .or_else(|| body.get("expires_in"))
                .and_then(|v| v.as_i64())
                .and_then(|v| u64::try_from(v).ok())
        })
        .unwrap_or(LARK_DEFAULT_TOKEN_TTL.as_secs());
    ttl.max(1)
}

pub(super) fn next_token_refresh_deadline(now: Instant, ttl_seconds: u64) -> Instant {
    let ttl = Duration::from_secs(ttl_seconds.max(1));
    let refresh_in = ttl
        .checked_sub(LARK_TOKEN_REFRESH_SKEW)
        .unwrap_or(Duration::from_secs(1));
    now + refresh_in
}

pub(super) fn ensure_lark_send_success(
    status: reqwest::StatusCode,
    body: &serde_json::Value,
    context: &str,
) -> anyhow::Result<()> {
    if !status.is_success() {
        anyhow::bail!("Lark send failed {context}: status={status}, body={body}");
    }

    let code = extract_lark_response_code(body).unwrap_or(0);
    if code != 0 {
        anyhow::bail!("Lark send failed {context}: code={code}, body={body}");
    }

    Ok(())
}
