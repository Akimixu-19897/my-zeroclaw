//! Channel subsystem for messaging platform integrations.
//!
//! This module provides the multi-channel messaging infrastructure that connects
//! ZeroClaw to external platforms. Each channel implements the [`Channel`] trait
//! defined in [`traits`], which provides a uniform interface for sending messages,
//! listening for incoming messages, health checking, and typing indicators.
//!
//! Channels are instantiated by [`start_channels`] based on the runtime configuration.
//! The subsystem manages per-sender conversation history, concurrent message processing
//! with configurable parallelism, and exponential-backoff reconnection for resilience.
//!
//! # Extension
//!
//! To add a new channel, implement [`Channel`] in a new submodule and wire it into
//! [`start_channels`]. See `AGENTS.md` §7.2 for the full change playbook.

pub mod clawdtalk;
pub mod cli;
pub mod dingtalk;
pub mod discord;
pub mod email_channel;
pub mod imessage;
pub mod irc;
#[cfg(feature = "channel-lark")]
pub mod lark;
pub mod linq;
#[cfg(feature = "channel-matrix")]
pub mod matrix;
pub mod mattermost;
pub mod nextcloud_talk;
pub mod nostr;
pub mod qq;
mod runtime;
pub mod signal;
pub mod slack;
pub mod telegram;
pub mod traits;
pub mod transcription;
pub mod tts;
pub mod wati;
pub mod wecom;
pub mod whatsapp;
#[cfg(feature = "whatsapp-web")]
pub mod whatsapp_storage;
#[cfg(feature = "whatsapp-web")]
pub mod whatsapp_web;

pub use clawdtalk::{ClawdTalkChannel, ClawdTalkConfig};
pub use cli::CliChannel;
pub use dingtalk::DingTalkChannel;
pub use discord::DiscordChannel;
pub use email_channel::EmailChannel;
pub use imessage::IMessageChannel;
pub use irc::IrcChannel;
#[cfg(feature = "channel-lark")]
pub use lark::LarkChannel;
pub use linq::LinqChannel;
#[cfg(feature = "channel-matrix")]
pub use matrix::MatrixChannel;
pub use mattermost::MattermostChannel;
pub use nextcloud_talk::NextcloudTalkChannel;
pub use nostr::NostrChannel;
pub use qq::QQChannel;
#[allow(unused_imports)]
pub(crate) use runtime::commands::handle_command;
#[cfg(test)]
#[allow(unused_imports)]
use runtime::lifecycle::{
    channel_health_detail_suffix, classify_health_result, ChannelHealthState,
};
pub use runtime::lifecycle::{doctor_channels, start_channels};
pub(crate) use runtime::processing::process_channel_message;
pub use runtime::prompt_builder::{build_system_prompt, build_system_prompt_with_mode};
#[cfg(test)]
#[allow(unused_imports)]
use runtime::registry::collect_configured_channels;
#[cfg(test)]
#[allow(unused_imports)]
use runtime::routing::should_skip_memory_context_entry;
pub(crate) use runtime::text::{normalize_channel_markdown_text, strip_tool_call_tags};
pub use signal::SignalChannel;
pub use slack::SlackChannel;
pub use telegram::TelegramChannel;
pub use traits::{Channel, SendMessage};
#[allow(unused_imports)]
pub use tts::{TtsManager, TtsProvider};
pub use wati::WatiChannel;
pub use wecom::WeComChannel;
pub use whatsapp::WhatsAppChannel;
#[cfg(feature = "whatsapp-web")]
pub use whatsapp_web::WhatsAppWebChannel;

use crate::agent::loop_::build_tool_instructions;
use crate::config::Config;
use crate::identity;
use crate::memory::{self, Memory};
use crate::observability::{self, Observer};
use crate::providers::{self, ChatMessage, Provider};
use crate::security::SecurityPolicy;
use crate::tools::{self, Tool};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use tokio_util::sync::CancellationToken;

use self::runtime::config::{
    config_file_stamp, resolved_default_model, resolved_default_provider,
    runtime_defaults_from_config,
};
#[cfg(test)]
#[allow(unused_imports)]
use self::runtime::processing_support::strip_isolated_tool_json_artifacts;
use self::runtime::routing::create_resilient_provider_nonblocking;

/// Per-sender conversation history for channel messages.
type ConversationHistoryMap = Arc<Mutex<HashMap<String, Vec<ChatMessage>>>>;
/// Maximum history messages to keep per sender.
const MAX_CHANNEL_HISTORY: usize = 50;
/// Minimum user-message length (in chars) for auto-save to memory.
/// Messages shorter than this (e.g. "ok", "thanks") are not stored,
/// reducing noise in memory recall.
const AUTOSAVE_MIN_MESSAGE_CHARS: usize = 20;

/// Maximum characters per injected workspace file (matches `OpenClaw` default).
const BOOTSTRAP_MAX_CHARS: usize = 20_000;

const DEFAULT_CHANNEL_INITIAL_BACKOFF_SECS: u64 = 2;
const DEFAULT_CHANNEL_MAX_BACKOFF_SECS: u64 = 60;
const MIN_CHANNEL_MESSAGE_TIMEOUT_SECS: u64 = 30;
/// Default timeout for processing a single channel message (LLM + tools).
/// Used as fallback when not configured in channels_config.message_timeout_secs.
const CHANNEL_MESSAGE_TIMEOUT_SECS: u64 = 300;
/// Cap timeout scaling so large max_tool_iterations values do not create unbounded waits.
const CHANNEL_MESSAGE_TIMEOUT_SCALE_CAP: u64 = 4;
const CHANNEL_PARALLELISM_PER_CHANNEL: usize = 4;
const CHANNEL_MIN_IN_FLIGHT_MESSAGES: usize = 8;
const CHANNEL_MAX_IN_FLIGHT_MESSAGES: usize = 64;
const CHANNEL_TYPING_REFRESH_INTERVAL_SECS: u64 = 4;
const CHANNEL_HEALTH_HEARTBEAT_SECS: u64 = 30;
const MODEL_CACHE_FILE: &str = "models_cache.json";
const MODEL_CACHE_PREVIEW_LIMIT: usize = 10;
const MEMORY_CONTEXT_MAX_ENTRIES: usize = 4;
const MEMORY_CONTEXT_ENTRY_MAX_CHARS: usize = 800;
const MEMORY_CONTEXT_MAX_CHARS: usize = 4_000;
const CHANNEL_HISTORY_COMPACT_KEEP_MESSAGES: usize = 12;
const CHANNEL_HISTORY_COMPACT_CONTENT_CHARS: usize = 600;
/// Guardrail for hook-modified outbound channel content.
const CHANNEL_HOOK_MAX_OUTBOUND_CHARS: usize = 20_000;
const CHANNEL_APPROVAL_REQUEST_FENCE: &str = "zeroclaw-approval";
const LARK_CARD_ACTION_FENCE: &str = "lark-card-action";
const CHANNEL_PENDING_APPROVAL_TTL: Duration = Duration::from_secs(60 * 60);

type ProviderCacheMap = Arc<Mutex<HashMap<String, Arc<dyn Provider>>>>;
type RouteSelectionMap = Arc<Mutex<HashMap<String, ChannelRouteSelection>>>;

#[derive(Debug, Clone)]
struct PendingChannelApproval {
    operation_id: String,
    tool_name: String,
    arguments: serde_json::Value,
    reason: String,
    preview: String,
    reply_target: String,
    thread_ts: Option<String>,
    user_message: String,
    provider: String,
    model: String,
    created_at: Instant,
}

#[derive(Debug, Clone)]
struct PendingChannelApprovalRequest {
    tool_name: String,
    arguments: serde_json::Value,
    reason: String,
    preview: String,
}

#[derive(Debug, Clone)]
struct ParsedChannelCardAction {
    action: String,
    operation_id: Option<String>,
}

fn effective_channel_message_timeout_secs(configured: u64) -> u64 {
    configured.max(MIN_CHANNEL_MESSAGE_TIMEOUT_SECS)
}

fn channel_message_timeout_budget_secs(
    message_timeout_secs: u64,
    max_tool_iterations: usize,
) -> u64 {
    let iterations = max_tool_iterations.max(1) as u64;
    let scale = iterations.min(CHANNEL_MESSAGE_TIMEOUT_SCALE_CAP);
    message_timeout_secs.saturating_mul(scale)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChannelRouteSelection {
    provider: String,
    model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChannelRuntimeCommand {
    ShowProviders,
    SetProvider(String),
    ShowModel,
    SetModel(String),
    NewSession,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ModelCacheState {
    entries: Vec<ModelCacheEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ModelCacheEntry {
    provider: String,
    models: Vec<String>,
}

#[derive(Debug, Clone)]
struct ChannelRuntimeDefaults {
    default_provider: String,
    model: String,
    temperature: f64,
    api_key: Option<String>,
    api_url: Option<String>,
    reliability: crate::config::ReliabilityConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConfigFileStamp {
    modified: SystemTime,
    len: u64,
}

#[derive(Debug, Clone)]
struct RuntimeConfigState {
    defaults: ChannelRuntimeDefaults,
    last_applied_stamp: Option<ConfigFileStamp>,
}

fn runtime_config_store() -> &'static Mutex<HashMap<PathBuf, RuntimeConfigState>> {
    static STORE: OnceLock<Mutex<HashMap<PathBuf, RuntimeConfigState>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pending_channel_approvals() -> &'static Mutex<HashMap<String, PendingChannelApproval>> {
    static STORE: OnceLock<Mutex<HashMap<String, PendingChannelApproval>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn prune_expired_pending_channel_approvals(now: Instant) {
    let mut store = pending_channel_approvals()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    store.retain(|_, approval| {
        now.saturating_duration_since(approval.created_at) < CHANNEL_PENDING_APPROVAL_TTL
    });
}

fn insert_pending_channel_approval(approval: PendingChannelApproval) {
    prune_expired_pending_channel_approvals(Instant::now());
    pending_channel_approvals()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(approval.operation_id.clone(), approval);
}

fn get_pending_channel_approval(operation_id: &str) -> Option<PendingChannelApproval> {
    prune_expired_pending_channel_approvals(Instant::now());
    pending_channel_approvals()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(operation_id)
        .cloned()
}

fn take_pending_channel_approval(operation_id: &str) -> Option<PendingChannelApproval> {
    prune_expired_pending_channel_approvals(Instant::now());
    pending_channel_approvals()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(operation_id)
}

#[cfg(test)]
fn clear_pending_channel_approvals() {
    pending_channel_approvals()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

const SYSTEMD_STATUS_ARGS: [&str; 3] = ["--user", "is-active", "zeroclaw.service"];
const SYSTEMD_RESTART_ARGS: [&str; 3] = ["--user", "restart", "zeroclaw.service"];
const OPENRC_STATUS_ARGS: [&str; 2] = ["zeroclaw", "status"];
const OPENRC_RESTART_ARGS: [&str; 2] = ["zeroclaw", "restart"];

#[derive(Clone)]
pub(crate) struct ChannelRuntimeContext {
    channels_by_name: Arc<HashMap<String, Arc<dyn Channel>>>,
    provider: Arc<dyn Provider>,
    default_provider: Arc<String>,
    memory: Arc<dyn Memory>,
    tools_registry: Arc<Vec<Box<dyn Tool>>>,
    observer: Arc<dyn Observer>,
    system_prompt: Arc<String>,
    model: Arc<String>,
    temperature: f64,
    auto_save_memory: bool,
    max_tool_iterations: usize,
    min_relevance_score: f64,
    conversation_histories: ConversationHistoryMap,
    provider_cache: ProviderCacheMap,
    route_overrides: RouteSelectionMap,
    api_key: Option<String>,
    api_url: Option<String>,
    reliability: Arc<crate::config::ReliabilityConfig>,
    provider_runtime_options: providers::ProviderRuntimeOptions,
    workspace_dir: Arc<PathBuf>,
    message_timeout_secs: u64,
    interrupt_on_new_message: bool,
    multimodal: crate::config::MultimodalConfig,
    hooks: Option<Arc<crate::hooks::HookRunner>>,
    non_cli_excluded_tools: Arc<Vec<String>>,
    model_routes: Arc<Vec<crate::config::ModelRouteConfig>>,
}

#[derive(Clone)]
struct InFlightSenderTaskState {
    task_id: u64,
    cancellation: CancellationToken,
    completion: Arc<InFlightTaskCompletion>,
}

struct InFlightTaskCompletion {
    done: AtomicBool,
    notify: tokio::sync::Notify,
}

impl InFlightTaskCompletion {
    fn new() -> Self {
        Self {
            done: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn mark_done(&self) {
        self.done.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn wait(&self) {
        if self.done.load(Ordering::Acquire) {
            return;
        }
        self.notify.notified().await;
    }
}

#[cfg(test)]
mod tests;
