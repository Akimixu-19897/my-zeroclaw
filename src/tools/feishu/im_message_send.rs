use super::super::common::FeishuToolClient;
use crate::channels::lark::media::LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES;
use crate::channels::lark::media_source::{
    build_outbound_media_load_options, build_root_scoped_sandbox_media_load_options,
    load_outbound_media, parse_media_source_input, validate_explicit_local_media_path,
    validate_remote_media_url, LoadedOutboundMedia, LocalPathPolicy, NormalizedMediaSource,
    OutboundMediaLoadOptions,
};
use crate::channels::lark::message_builders::build_lark_post_content;
use crate::channels::lark::outbound::{normalize_lark_target, resolve_lark_receive_id_type};
use directories::UserDirs;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(super) const CHANNEL_CONTEXT_ARG_KEY: &str = "__channel_context";

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct FeishuSendToolContext {
    current_channel_name: Option<String>,
    current_channel_id: Option<String>,
    current_message_id: Option<String>,
    current_thread_ts: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalPickKind {
    Image,
    File,
}

pub(super) struct FeishuSendParams {
    receive_id: Option<String>,
    text: String,
    reply_to_message_id: Option<String>,
    reply_in_thread: bool,
    targets_current_chat: bool,
    media_source: Option<String>,
    local_pick: Option<LocalPickKind>,
    file_name: Option<String>,
    card: Option<Value>,
    media_load_options: Option<OutboundMediaLoadOptions>,
}

pub(super) async fn deliver_feishu_send_action(
    client: &FeishuToolClient,
    args: &Value,
    action: &str,
    workspace_dir: &Path,
) -> anyhow::Result<Value> {
    let params = parse_send_params(args, workspace_dir, client)?;
    if params.text.trim().is_empty() && params.media_source.is_none() && params.card.is_none() {
        anyhow::bail!("send requires at least one of: message, card, or media.");
    }

    if !params.text.trim().is_empty() && (params.media_source.is_some() || params.card.is_some()) {
        send_post_text(client, &params).await?;
    }

    if let Some(card) = params.card.as_ref() {
        let response = send_interactive_card(client, &params, card.clone()).await?;
        return Ok(build_send_output(
            client,
            action,
            &response,
            None,
            params.targets_current_chat,
        ));
    }

    if let Some(media_source) = params.media_source.as_deref() {
        return deliver_media(client, action, &params, media_source).await;
    }

    let response = send_post_text(client, &params).await?;
    Ok(build_send_output(
        client,
        action,
        &response,
        None,
        params.targets_current_chat,
    ))
}

fn parse_send_params(
    args: &Value,
    workspace_dir: &Path,
    client: &FeishuToolClient,
) -> anyhow::Result<FeishuSendParams> {
    let explicit_receive_id = first_string(args, &["receive_id", "to"])
        .map(|value| {
            normalize_lark_target(value)
                .ok_or_else(|| anyhow::anyhow!("Invalid 'receive_id' parameter"))
        })
        .transpose()?;
    let text = first_string(args, &["text", "message"])
        .unwrap_or_default()
        .to_string();
    let explicit_reply_to =
        first_string(args, &["replyTo", "reply_to", "message_id"]).map(str::to_string);
    let media_source = first_string(args, &["media", "path", "filePath", "url"])
        .map(|raw| normalize_media_source_input(raw, Some(workspace_dir)))
        .transpose()?;
    let local_pick = first_string(args, &["local_pick", "pick"])
        .map(parse_local_pick_kind)
        .transpose()?;
    let file_name = first_string(args, &["fileName", "name"]).map(str::to_string);
    let card = parse_card_param(args.get("card"))?;

    let tool_context = args
        .get(CHANNEL_CONTEXT_ARG_KEY)
        .cloned()
        .map(serde_json::from_value::<FeishuSendToolContext>)
        .transpose()?
        .unwrap_or_default();
    let context_channel_id = tool_context
        .current_channel_id
        .as_deref()
        .and_then(normalize_lark_target);
    let same_chat = explicit_receive_id.is_none()
        || matches!(
            (explicit_receive_id.as_deref(), context_channel_id.as_deref()),
            (Some(explicit), Some(current)) if explicit == current
        );
    let targets_current_chat = context_channel_id.is_some()
        && (explicit_receive_id.is_none() || explicit_receive_id == context_channel_id);
    let reply_in_thread = same_chat
        && tool_context
            .current_thread_ts
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    let reply_to_message_id = explicit_reply_to.or_else(|| {
        if reply_in_thread {
            tool_context
                .current_message_id
                .clone()
                .filter(|value| !value.trim().is_empty())
        } else {
            None
        }
    });
    let media_source = media_source.or_else(|| {
        local_pick.and_then(|kind| pick_local_media_path(workspace_dir, &tool_context, kind))
    });
    let media_load_options = media_source
        .as_deref()
        .map(|source| build_media_load_options(source, workspace_dir, local_pick, client))
        .transpose()?;

    Ok(FeishuSendParams {
        receive_id: explicit_receive_id.or(context_channel_id),
        text,
        reply_to_message_id,
        reply_in_thread,
        targets_current_chat,
        media_source,
        local_pick,
        file_name,
        card,
        media_load_options,
    })
}

async fn send_post_text(
    client: &FeishuToolClient,
    params: &FeishuSendParams,
) -> anyhow::Result<Value> {
    send_im_message(
        client,
        params,
        "post",
        build_lark_post_content(&params.text).to_string(),
    )
    .await
}

async fn send_interactive_card(
    client: &FeishuToolClient,
    params: &FeishuSendParams,
    card: Value,
) -> anyhow::Result<Value> {
    send_im_message(client, params, "interactive", card.to_string()).await
}

async fn send_im_message(
    client: &FeishuToolClient,
    params: &FeishuSendParams,
    msg_type: &str,
    content: String,
) -> anyhow::Result<Value> {
    if let Some(message_id) = params.reply_to_message_id.as_deref() {
        client
            .post_json(
                &format!("/im/v1/messages/{message_id}/reply"),
                &json!({
                    "msg_type": msg_type,
                    "content": content,
                    "reply_in_thread": params.reply_in_thread,
                }),
            )
            .await
    } else {
        let receive_id = params
            .receive_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Missing 'receive_id' parameter"))?;
        let receive_id_type = resolve_lark_receive_id_type(receive_id);
        client
            .post_json(
                &format!("/im/v1/messages?receive_id_type={receive_id_type}"),
                &json!({
                    "receive_id": receive_id,
                    "msg_type": msg_type,
                    "content": content,
                }),
            )
            .await
    }
}

async fn deliver_media(
    client: &FeishuToolClient,
    action: &str,
    params: &FeishuSendParams,
    media_source: &str,
) -> anyhow::Result<Value> {
    let response = upload_and_send_media(client, params, media_source).await?;
    Ok(build_send_output(
        client,
        action,
        &response,
        None,
        params.targets_current_chat,
    ))
}

async fn upload_and_send_media(
    client: &FeishuToolClient,
    params: &FeishuSendParams,
    media_source: &str,
) -> anyhow::Result<Value> {
    let payload = resolve_media_payload(
        media_source,
        params.file_name.as_deref(),
        params.media_load_options.as_ref(),
    )
    .await?;
    if is_image_file_name(&payload.file_name) {
        let response: Value = client
            .upload_named_bytes(
                "/im/v1/images",
                "image",
                &payload.file_name,
                payload
                    .mime_type
                    .as_deref()
                    .unwrap_or("application/octet-stream"),
                payload.bytes,
                &[("image_type", "message")],
            )
            .await?;
        let image_key = response
            .pointer("/data/image_key")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing image_key in Feishu image upload response"))?;
        return send_im_message(
            client,
            params,
            "image",
            json!({ "image_key": image_key }).to_string(),
        )
        .await;
    }

    let file_type = detect_file_type(&payload.file_name);
    let duration = match file_type {
        "opus" => parse_ogg_opus_duration_ms(&payload.bytes),
        "mp4" => parse_mp4_duration_ms(&payload.bytes),
        _ => None,
    };
    let duration_string = duration.map(|value| value.to_string());
    let mut fields = vec![
        ("file_type", file_type),
        ("file_name", payload.file_name.as_str()),
    ];
    if let Some(duration) = duration_string.as_deref() {
        fields.push(("duration", duration));
    }
    let response: Value = client
        .upload_named_bytes(
            "/im/v1/files",
            "file",
            &payload.file_name,
            payload
                .mime_type
                .as_deref()
                .unwrap_or("application/octet-stream"),
            payload.bytes,
            &fields,
        )
        .await?;
    let file_key = response
        .pointer("/data/file_key")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing file_key in Feishu file upload response"))?;
    let msg_type = match file_type {
        "opus" => "audio",
        "mp4" => "media",
        _ => "file",
    };
    send_im_message(
        client,
        params,
        msg_type,
        json!({ "file_key": file_key }).to_string(),
    )
    .await
}

fn build_send_output(
    client: &FeishuToolClient,
    action: &str,
    response: &Value,
    warning: Option<String>,
    targets_current_chat: bool,
) -> Value {
    let mut output = json!({
        "account": client.account_name(),
        "action": action,
        "message_id": response.pointer("/data/message_id"),
        "chat_id": response.pointer("/data/chat_id"),
    });
    if targets_current_chat {
        output["assistant_reply"] = Value::String("NO_REPLY".to_string());
        output["delivery_scope"] = Value::String("current_chat".to_string());
        output["note"] = Value::String(
            "Message already delivered to the current Feishu chat. End the turn with exactly NO_REPLY."
                .to_string(),
        );
    }
    if let Some(warning) = warning {
        output["warning"] = Value::String(warning);
    }
    output
}

#[derive(Debug, Clone)]
struct MediaPayload {
    bytes: Vec<u8>,
    file_name: String,
    mime_type: Option<String>,
}

async fn resolve_media_payload(
    media_source: &str,
    override_file_name: Option<&str>,
    load_options: Option<&OutboundMediaLoadOptions>,
) -> anyhow::Result<MediaPayload> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut default_load_options = None;
    match load_outbound_media(
        &client,
        media_source,
        load_options.unwrap_or_else(|| {
            default_load_options.get_or_insert_with(|| {
                build_outbound_media_load_options(
                    LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES,
                    None,
                    None,
                    LocalPathPolicy::ExplicitPathAllowed,
                )
            })
        }),
    )
    .await?
    {
        LoadedOutboundMedia::Downloaded(remote) => {
            let file_name = override_file_name
                .map(str::to_string)
                .or_else(|| remote.file_name.clone())
                .or_else(|| derive_file_name_from_url(media_source))
                .unwrap_or_else(|| "file".to_string());
            Ok(MediaPayload {
                bytes: remote.bytes,
                file_name,
                mime_type: remote.content_type,
            })
        }
        LoadedOutboundMedia::LocalPath(path) => {
            let file_name = override_file_name
                .map(str::to_string)
                .or_else(|| {
                    path.file_name()
                        .and_then(|value| value.to_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "file".to_string());
            let mime_type = mime_guess::from_path(&path).first_raw().map(str::to_string);
            Ok(MediaPayload {
                bytes: std::fs::read(&path)?,
                file_name,
                mime_type,
            })
        }
        LoadedOutboundMedia::InMemory(local) => {
            let file_name = override_file_name
                .map(str::to_string)
                .or_else(|| {
                    local.path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "file".to_string());
            let mime_type = mime_guess::from_path(&local.path).first_raw().map(str::to_string);
            Ok(MediaPayload {
                bytes: local.bytes,
                file_name,
                mime_type,
            })
        }
    }
}

fn build_media_load_options(
    media_source: &str,
    workspace_dir: &Path,
    local_pick: Option<LocalPickKind>,
    client: &FeishuToolClient,
) -> anyhow::Result<OutboundMediaLoadOptions> {
    let max_bytes = client
        .media_max_bytes()
        .unwrap_or(LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES);
    if local_pick.is_some() {
        let path = Path::new(media_source);
        let root = path.parent().unwrap_or(workspace_dir);
        return Ok(build_root_scoped_sandbox_media_load_options(
            max_bytes,
            root,
        ));
    }

    Ok(build_outbound_media_load_options(
        max_bytes,
        Some(workspace_dir),
        client.media_local_roots().map(|roots| roots.to_vec()),
        if client.media_local_roots().is_some() {
            LocalPathPolicy::DefaultRoots
        } else {
            LocalPathPolicy::ExplicitPathAllowed
        },
    ))
}

fn derive_file_name_from_url(raw: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(raw).ok()?;
    let last = parsed.path_segments()?.next_back()?;
    (!last.trim().is_empty()).then(|| last.to_string())
}

fn normalize_media_source_input(
    raw: &str,
    workspace_dir: Option<&Path>,
) -> anyhow::Result<String> {
    match parse_media_source_input(raw)? {
        NormalizedMediaSource::LocalPath(path) => Ok(
            validate_explicit_local_media_path(path, workspace_dir)?
                .display()
                .to_string(),
        ),
        NormalizedMediaSource::RemoteUrl(url) => validate_remote_media_url(&url),
    }
}

fn is_image_file_name(file_name: &str) -> bool {
    matches!(
        extension(file_name).as_deref(),
        Some("jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "ico" | "tiff" | "tif" | "heic")
    )
}

fn detect_file_type(file_name: &str) -> &'static str {
    match extension(file_name).as_deref() {
        Some("opus" | "ogg") => "opus",
        Some("mp4" | "mov" | "avi" | "mkv" | "webm") => "mp4",
        Some("pdf") => "pdf",
        Some("doc" | "docx") => "doc",
        Some("xls" | "xlsx" | "csv") => "xls",
        Some("ppt" | "pptx") => "ppt",
        _ => "stream",
    }
}

fn parse_ogg_opus_duration_ms(buffer: &[u8]) -> Option<u64> {
    const OGGS: &[u8; 4] = b"OggS";
    let offset = buffer.windows(OGGS.len()).rposition(|window| window == OGGS)?;
    let granule_offset = offset + 6;
    let granule_bytes: [u8; 8] = buffer.get(granule_offset..granule_offset + 8)?.try_into().ok()?;
    let granule = u64::from_le_bytes(granule_bytes);
    (granule > 0).then_some(granule.div_ceil(48_000) * 1_000)
}

fn parse_mp4_duration_ms(buffer: &[u8]) -> Option<u64> {
    let (moov_start, moov_end) = find_mp4_box(buffer, 0, buffer.len(), b"moov")?;
    let (mvhd_start, _) = find_mp4_box(buffer, moov_start, moov_end, b"mvhd")?;
    let version = *buffer.get(mvhd_start)?;

    let (timescale, duration) = if version == 0 {
        let timescale = u32::from_be_bytes(buffer.get(mvhd_start + 12..mvhd_start + 16)?.try_into().ok()?);
        let duration = u32::from_be_bytes(buffer.get(mvhd_start + 16..mvhd_start + 20)?.try_into().ok()?);
        (u64::from(timescale), u64::from(duration))
    } else {
        let timescale = u32::from_be_bytes(buffer.get(mvhd_start + 20..mvhd_start + 24)?.try_into().ok()?);
        let hi = u32::from_be_bytes(buffer.get(mvhd_start + 24..mvhd_start + 28)?.try_into().ok()?);
        let lo = u32::from_be_bytes(buffer.get(mvhd_start + 28..mvhd_start + 32)?.try_into().ok()?);
        (u64::from(timescale), (u64::from(hi) << 32) | u64::from(lo))
    };

    (timescale > 0 && duration > 0).then_some(duration.saturating_mul(1_000) / timescale)
}

fn find_mp4_box(buffer: &[u8], start: usize, end: usize, box_type: &[u8; 4]) -> Option<(usize, usize)> {
    let mut offset = start;
    while offset.checked_add(8)? <= end {
        let size = u32::from_be_bytes(buffer.get(offset..offset + 4)?.try_into().ok()?);
        let current_type = buffer.get(offset + 4..offset + 8)?;

        let (box_end, data_start) = match size {
            0 => (end, offset + 8),
            1 => {
                let hi = u32::from_be_bytes(buffer.get(offset + 8..offset + 12)?.try_into().ok()?);
                let lo = u32::from_be_bytes(buffer.get(offset + 12..offset + 16)?.try_into().ok()?);
                let box_end_u64 =
                    (offset as u64).saturating_add((u64::from(hi) << 32) | u64::from(lo));
                let box_end = usize::try_from(box_end_u64).ok()?;
                (box_end.min(end), offset + 16)
            }
            size if size >= 8 => {
                let box_end = offset.checked_add(usize::try_from(size).ok()?)?;
                (box_end.min(end), offset + 8)
            }
            _ => return None,
        };

        if current_type == box_type {
            return Some((data_start, box_end));
        }
        if box_end <= offset {
            return None;
        }
        offset = box_end;
    }
    None
}

fn extension(file_name: &str) -> Option<String> {
    Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
}

fn parse_card_param(raw: Option<&Value>) -> anyhow::Result<Option<Value>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    if raw.is_object() {
        return Ok(Some(raw.clone()));
    }
    if let Some(value) = raw.as_str() {
        let trimmed = value.trim();
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                if parsed.is_object() {
                    return Ok(Some(parsed));
                }
            }
        }
        return Ok(None);
    }
    Ok(None)
}

fn first_string<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn parse_local_pick_kind(value: &str) -> anyhow::Result<LocalPickKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "image" => Ok(LocalPickKind::Image),
        "file" => Ok(LocalPickKind::File),
        other => anyhow::bail!("Unsupported local_pick value: {other}"),
    }
}

fn pick_local_media_path(
    workspace_dir: &Path,
    context: &FeishuSendToolContext,
    kind: LocalPickKind,
) -> Option<String> {
    let mut roots = Vec::new();
    if let Some(channel_name) = context
        .current_channel_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let account_id = channel_name
            .split_once(':')
            .map(|(_, account)| account)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("default");
        roots.push(
            workspace_dir
                .join("channels")
                .join(channel_name)
                .join(account_id)
                .join("inbound"),
        );
    }
    roots.push(workspace_dir.to_path_buf());
    if let Some(home) = UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) {
        let fallback_root = home.join(".zeroclaw").join("workspace");
        if fallback_root != workspace_dir {
            roots.push(fallback_root);
        }
    }

    find_first_matching_file(&roots, kind).map(|path| path.to_string_lossy().into_owned())
}

fn find_first_matching_file(roots: &[PathBuf], kind: LocalPickKind) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for root in roots {
        if !root.exists() {
            continue;
        }
        let mut queue = VecDeque::from([(root.clone(), 0usize)]);
        while let Some((dir, depth)) = queue.pop_front() {
            let entries = std::fs::read_dir(&dir).ok()?;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if depth < 4 {
                        queue.push_back((path, depth + 1));
                    }
                    continue;
                }
                if !matches_local_pick(&path, kind) {
                    continue;
                }
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|meta| meta.modified().ok())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                match &best {
                    Some((best_time, _)) if modified <= *best_time => {}
                    _ => best = Some((modified, path)),
                }
            }
            if best.is_some() && root.ends_with("inbound") {
                break;
            }
        }
        if best.is_some() {
            break;
        }
    }

    best.map(|(_, path)| path)
}

fn matches_local_pick(path: &Path, kind: LocalPickKind) -> bool {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    match (kind, ext.as_deref()) {
        (LocalPickKind::Image, Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "heic")) => {
            true
        }
        (LocalPickKind::File, Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "heic")) => {
            false
        }
        (LocalPickKind::File, Some(_)) => true,
        _ => false,
    }
}
