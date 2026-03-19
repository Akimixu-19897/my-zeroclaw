use super::*;
use crate::channels::lark::helpers::parse_post_content;
use crate::channels::lark::message_builders::build_lark_post_content;
use crate::channels::lark::outbound::{normalize_lark_target, resolve_lark_receive_id_type};
use crate::channels::strip_tool_call_tags;
use crate::channels::traits::{ChannelMessageContext, SendMessage};
use async_trait::async_trait;

impl LarkChannel {
    async fn fetch_message_items_once(
        &self,
        message_id: &str,
        token: &str,
    ) -> anyhow::Result<(reqwest::StatusCode, serde_json::Value)> {
        let resp = self
            .http_client()
            .get(format!("{}/im/v1/messages/{message_id}", self.api_base()))
            .query(&[
                ("user_id_type", "open_id"),
                ("card_msg_content_type", "raw_card_content"),
            ])
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        let status = resp.status();
        let raw = resp.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|_| serde_json::json!({ "raw": raw }));
        Ok((status, parsed))
    }

    async fn fetch_message_items(&self, message_id: &str) -> anyhow::Result<serde_json::Value> {
        let token = self.get_tenant_access_token().await?;
        let (status, response) = self.fetch_message_items_once(message_id, &token).await?;
        let response = if should_refresh_lark_tenant_token(status, &response) {
            self.invalidate_token().await;
            let refreshed = self.get_tenant_access_token().await?;
            let (retry_status, retry_response) = self
                .fetch_message_items_once(message_id, &refreshed)
                .await?;
            if should_refresh_lark_tenant_token(retry_status, &retry_response) {
                anyhow::bail!(
                    "Lark message fetch failed after token refresh: status={retry_status}, body={retry_response}"
                );
            }
            retry_response
        } else {
            response
        };
        Ok(response)
    }

    fn parsed_sender_session_key(parsed: &LarkParsedMessage) -> String {
        if parsed.chat_type == "group" {
            parsed.chat_id.clone()
        } else {
            parsed.sender_open_id.clone()
        }
    }

    fn build_channel_message_context(parsed: &LarkParsedMessage) -> ChannelMessageContext {
        let dispatch = build_lark_dispatch_context(parsed);
        ChannelMessageContext {
            sender_id: Some(parsed.sender_open_id.clone()),
            chat_id: Some(parsed.chat_id.clone()),
            chat_type: Some(parsed.chat_type.clone()),
            content_type: Some(parsed.message_type.clone()),
            raw_content: Some(parsed.raw_content.clone()),
            root_id: parsed.root_id.clone(),
            parent_id: parsed.parent_id.clone(),
            thread_id: parsed.thread_id.clone(),
            origin_from: Some(dispatch.feishu_from),
            origin_to: Some(dispatch.feishu_to),
            envelope_from: Some(dispatch.envelope_from),
        }
    }

    async fn fetch_interactive_message_content(
        &self,
        message_id: &str,
    ) -> anyhow::Result<Option<String>> {
        let response = self.fetch_message_items(message_id).await?;

        if extract_lark_response_code(&response).unwrap_or(-1) != 0 {
            return Ok(None);
        }

        Ok(response
            .pointer("/data/items/0/body/content")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }

    async fn fetch_merge_forward_items(
        &self,
        message_id: &str,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let response = self.fetch_message_items(message_id).await?;
        if extract_lark_response_code(&response).unwrap_or(-1) != 0 {
            return Ok(Vec::new());
        }
        Ok(response
            .pointer("/data/items")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    fn build_merge_forward_item_content(item: &serde_json::Value) -> String {
        let msg_type = item
            .get("msg_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("text");
        let raw_content = item
            .pointer("/body/content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("{}");
        let resources = parse_lark_inbound_resources(msg_type, raw_content);
        let text = if msg_type == "text" {
            parse_lark_text_content(raw_content)
        } else {
            None
        };
        build_lark_normalized_content(msg_type, raw_content, text.as_deref(), &resources)
    }

    fn format_merge_forward_timestamp(create_time: Option<i64>) -> String {
        let Some(create_time) = create_time else {
            return "unknown".to_string();
        };
        let Some(timestamp) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(create_time)
        else {
            return "unknown".to_string();
        };
        timestamp
            .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).expect("valid offset"))
            .format("%Y-%m-%dT%H:%M:%S%:z")
            .to_string()
    }

    fn indent_lines(content: &str, prefix: &str) -> String {
        content
            .lines()
            .map(|line| format!("{prefix}{line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn build_merge_forward_children_map(
        items: &[serde_json::Value],
        root_message_id: &str,
    ) -> std::collections::HashMap<String, Vec<serde_json::Value>> {
        let mut children_map = std::collections::HashMap::<String, Vec<serde_json::Value>>::new();
        for item in items {
            let message_id = item
                .get("message_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let upper_message_id = item
                .get("upper_message_id")
                .and_then(serde_json::Value::as_str);
            if message_id == root_message_id && upper_message_id.is_none() {
                continue;
            }
            let parent_id = upper_message_id.unwrap_or(root_message_id).to_string();
            children_map
                .entry(parent_id)
                .or_default()
                .push(item.clone());
        }
        for children in children_map.values_mut() {
            children.sort_by_key(|item| {
                item.get("create_time")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or_default()
            });
        }
        children_map
    }

    fn format_merge_forward_subtree(
        parent_id: &str,
        children_map: &std::collections::HashMap<String, Vec<serde_json::Value>>,
        nested: bool,
    ) -> String {
        let Some(children) = children_map.get(parent_id) else {
            return "<forwarded_messages/>".to_string();
        };
        if children.is_empty() {
            return "<forwarded_messages/>".to_string();
        }

        let mut parts = Vec::new();
        for child in children {
            let msg_type = child
                .get("msg_type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("text");
            let content = if msg_type == "merge_forward" {
                let nested_id = child
                    .get("message_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if nested_id.is_empty() {
                    "<forwarded_messages/>".to_string()
                } else {
                    Self::format_merge_forward_subtree(nested_id, children_map, true)
                }
            } else {
                Self::build_merge_forward_item_content(child)
            };
            let sender = child
                .pointer("/sender/id")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("unknown");
            let timestamp = Self::format_merge_forward_timestamp(
                child
                    .get("create_time")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok()),
            );
            parts.push(format!(
                "[{timestamp}] {sender}:\n{}",
                Self::indent_lines(&content, "    ")
            ));
        }

        let body = if nested {
            Self::indent_lines(&parts.join("\n"), "    ")
        } else {
            parts.join("\n")
        };
        format!("<forwarded_messages>\n{body}\n</forwarded_messages>")
    }

    async fn fetch_merge_forward_content(&self, message_id: &str) -> anyhow::Result<String> {
        let items = self.fetch_merge_forward_items(message_id).await?;
        if items.is_empty() {
            return Ok("<forwarded_messages/>".to_string());
        }
        let children_map = Self::build_merge_forward_children_map(&items, message_id);
        Ok(Self::format_merge_forward_subtree(
            message_id,
            &children_map,
            false,
        ))
    }

    async fn fetch_parent_message_content(
        &self,
        parent_id: &str,
    ) -> anyhow::Result<Option<String>> {
        let response = self.fetch_message_items(parent_id).await?;
        if extract_lark_response_code(&response).unwrap_or(-1) != 0 {
            return Ok(None);
        }

        let Some(item) = response.pointer("/data/items/0") else {
            return Ok(None);
        };

        let msg_type = item
            .get("msg_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("text");

        if msg_type == "merge_forward" {
            let content = self.fetch_merge_forward_content(parent_id).await?;
            return Ok((!content.trim().is_empty()).then_some(content));
        }

        let raw_content = item
            .pointer("/body/content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("{}");
        let resources = parse_lark_inbound_resources(msg_type, raw_content);
        let text = match msg_type {
            "text" => parse_lark_text_content(raw_content),
            "post" => parse_post_content(raw_content),
            _ => None,
        };
        let content =
            build_lark_normalized_content(msg_type, raw_content, text.as_deref(), &resources);
        Ok((!content.trim().is_empty()).then_some(content))
    }

    fn parse_inbound_message_from_content(
        &self,
        sender_open_id: &str,
        message: &LarkMessage,
        content: &str,
    ) -> Option<LarkParsedMessage> {
        if sender_open_id.is_empty() {
            return None;
        }
        if !self.is_user_allowed(sender_open_id) {
            tracing::warn!("Lark: ignoring message from unauthorized user: {sender_open_id}");
            return None;
        }

        let resources = parse_lark_inbound_resources(&message.message_type, content);
        let (text, normalized_content, post_mentioned_open_ids) =
            match message.message_type.as_str() {
                "text" => {
                    let text = parse_lark_text_content(content)?;
                    let normalized =
                        build_lark_normalized_content("text", content, Some(&text), &resources);
                    (text, normalized, Vec::new())
                }
                "post" => {
                    let details = parse_post_content_details(content)?;
                    (
                        details.text,
                        details.normalized_content,
                        details.mentioned_open_ids,
                    )
                }
                "image" | "sticker" | "file" | "folder" | "audio" | "media" | "video" => (
                    String::new(),
                    build_lark_normalized_content(&message.message_type, content, None, &resources),
                    Vec::new(),
                ),
                "location"
                | "system"
                | "hongbao"
                | "share_chat"
                | "share_user"
                | "todo"
                | "vote"
                | "share_calendar_event"
                | "calendar"
                | "general_calendar"
                | "video_chat" => (
                    String::new(),
                    build_lark_normalized_content(&message.message_type, content, None, &resources),
                    Vec::new(),
                ),
                "interactive" => (
                    render_lark_fallback_content("", &resources, Some("<interactive card>")),
                    build_lark_normalized_content("interactive", content, None, &resources),
                    Vec::new(),
                ),
                _ => (
                    String::new(),
                    build_lark_normalized_content(&message.message_type, content, None, &resources),
                    Vec::new(),
                ),
            };

        let text = strip_at_placeholders(&text).trim().to_string();
        let bot_open_id = self.resolved_bot_open_id();
        if message.chat_type == "group"
            && !should_respond_in_group(
                self.mention_only,
                bot_open_id.as_deref(),
                &message.mentions,
                &post_mentioned_open_ids,
            )
        {
            return None;
        }

        if text.is_empty() && resources.is_empty() && normalized_content.trim().is_empty() {
            return None;
        }

        let create_time_secs = message
            .create_time
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .map(|ms| ms / 1000);

        Some(LarkParsedMessage {
            message_id: if message.message_id.is_empty() {
                format!(
                    "lark:{sender_open_id}:{}",
                    message.create_time.as_deref().unwrap_or("0")
                )
            } else {
                message.message_id.clone()
            },
            chat_id: if message.chat_id.is_empty() {
                sender_open_id.to_string()
            } else {
                message.chat_id.clone()
            },
            sender_open_id: sender_open_id.to_string(),
            chat_type: if message.chat_type.is_empty() {
                "p2p".to_string()
            } else {
                message.chat_type.clone()
            },
            message_type: message.message_type.clone(),
            raw_content: content.to_string(),
            normalized_content,
            create_time_secs,
            root_id: message.root_id.clone(),
            parent_id: message.parent_id.clone(),
            thread_id: message.thread_id.clone(),
            mentions: message.mentions.clone(),
            text,
            post_mentioned_open_ids,
            resources,
        })
    }

    async fn send_json_once(
        &self,
        url: &str,
        token: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<(reqwest::StatusCode, serde_json::Value)> {
        let resp = self
            .http_client()
            .post(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let raw = resp.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|_| serde_json::json!({ "raw": raw }));
        Ok((status, parsed))
    }

    async fn patch_message_once(
        &self,
        message_id: &str,
        token: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<(reqwest::StatusCode, serde_json::Value)> {
        let resp = self
            .http_client()
            .patch(self.message_patch_url(message_id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let raw = resp.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|_| serde_json::json!({ "raw": raw }));
        Ok((status, parsed))
    }

    async fn delete_message_once(
        &self,
        message_id: &str,
        token: &str,
    ) -> anyhow::Result<(reqwest::StatusCode, serde_json::Value)> {
        let resp = self
            .http_client()
            .delete(self.delete_message_url(message_id))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        let status = resp.status();
        let raw = resp.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|_| serde_json::json!({ "raw": raw }));
        Ok((status, parsed))
    }

    async fn send_json_message_with_response(
        &self,
        body: &serde_json::Value,
        reply_to_message_id: Option<&str>,
        recipient: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        let token = self.get_tenant_access_token().await?;
        let url = reply_to_message_id
            .map(|message_id| self.reply_message_url(message_id))
            .unwrap_or_else(|| {
                let receive_id_type =
                    resolve_lark_receive_id_type(recipient.unwrap_or_default().trim());
                self.send_message_url(receive_id_type)
            });
        let (status, response) = self.send_json_once(&url, &token, body).await?;

        if should_refresh_lark_tenant_token(status, &response) {
            self.invalidate_token().await;
            let new_token = self.get_tenant_access_token().await?;
            let (retry_status, retry_response) =
                self.send_json_once(&url, &new_token, body).await?;

            if should_refresh_lark_tenant_token(retry_status, &retry_response) {
                anyhow::bail!(
                    "Lark send failed after token refresh: status={retry_status}, body={retry_response}"
                );
            }

            ensure_lark_send_success(retry_status, &retry_response, "after token refresh")?;
            return Ok(retry_response);
        }

        ensure_lark_send_success(status, &response, "without token refresh")?;
        Ok(response)
    }

    pub(super) async fn patch_message_with_retry(
        &self,
        message_id: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<()> {
        if self.is_message_unavailable(message_id).await {
            return Ok(());
        }

        let token = self.get_tenant_access_token().await?;
        let (status, response) = self.patch_message_once(message_id, &token, body).await?;

        if should_refresh_lark_tenant_token(status, &response) {
            self.invalidate_token().await;
            let new_token = self.get_tenant_access_token().await?;
            let (retry_status, retry_response) = self
                .patch_message_once(message_id, &new_token, body)
                .await?;

            if should_refresh_lark_tenant_token(retry_status, &retry_response) {
                anyhow::bail!(
                    "Lark patch failed after token refresh: status={retry_status}, body={retry_response}"
                );
            }

            self.note_message_unavailable(message_id, &retry_response)
                .await;
            if self.is_message_unavailable(message_id).await {
                return Ok(());
            }
            ensure_lark_send_success(retry_status, &retry_response, "patch after token refresh")?;
            return Ok(());
        }

        self.note_message_unavailable(message_id, &response).await;
        if self.is_message_unavailable(message_id).await {
            return Ok(());
        }
        ensure_lark_send_success(status, &response, "patch without token refresh")?;
        Ok(())
    }

    pub(super) async fn delete_message_with_retry(&self, message_id: &str) -> anyhow::Result<()> {
        if self.is_message_unavailable(message_id).await {
            return Ok(());
        }

        let token = self.get_tenant_access_token().await?;
        let (status, response) = self.delete_message_once(message_id, &token).await?;

        if should_refresh_lark_tenant_token(status, &response) {
            self.invalidate_token().await;
            let new_token = self.get_tenant_access_token().await?;
            let (retry_status, retry_response) =
                self.delete_message_once(message_id, &new_token).await?;

            if should_refresh_lark_tenant_token(retry_status, &retry_response) {
                anyhow::bail!(
                    "Lark delete failed after token refresh: status={retry_status}, body={retry_response}"
                );
            }

            self.note_message_unavailable(message_id, &retry_response)
                .await;
            if self.is_message_unavailable(message_id).await {
                return Ok(());
            }
            ensure_lark_send_success(retry_status, &retry_response, "delete after token refresh")?;
            return Ok(());
        }

        self.note_message_unavailable(message_id, &response).await;
        if self.is_message_unavailable(message_id).await {
            return Ok(());
        }
        ensure_lark_send_success(status, &response, "delete without token refresh")?;
        Ok(())
    }

    async fn upload_image_once(
        &self,
        token: &str,
        image_path: &Path,
    ) -> anyhow::Result<(reqwest::StatusCode, serde_json::Value)> {
        let image_bytes = tokio::fs::read(image_path).await?;
        let file_name = image_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image");
        let mime = mime_guess::from_path(image_path).first_or_octet_stream();
        let image_part = reqwest::multipart::Part::bytes(image_bytes)
            .file_name(file_name.to_string())
            .mime_str(mime.as_ref())?;
        let form = reqwest::multipart::Form::new()
            .text("image_type", "message")
            .part("image", image_part);

        let resp = self
            .http_client()
            .post(self.upload_image_url())
            .header("Authorization", format!("Bearer {token}"))
            .multipart(form)
            .send()
            .await?;
        let status = resp.status();
        let raw = resp.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|_| serde_json::json!({ "raw": raw }));
        Ok((status, parsed))
    }

    async fn upload_file_once(
        &self,
        token: &str,
        file_path: &Path,
        file_type: &str,
    ) -> anyhow::Result<(reqwest::StatusCode, serde_json::Value)> {
        let file_bytes = tokio::fs::read(file_path).await?;
        let file_name = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file");
        let mime = mime_guess::from_path(file_path).first_or_octet_stream();
        let file_part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(file_name.to_string())
            .mime_str(mime.as_ref())?;
        let form = reqwest::multipart::Form::new()
            .text("file_type", file_type.to_string())
            .text("file_name", file_name.to_string())
            .part("file", file_part);

        let resp = self
            .http_client()
            .post(self.upload_file_url())
            .header("Authorization", format!("Bearer {token}"))
            .multipart(form)
            .send()
            .await?;
        let status = resp.status();
        let raw = resp.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|_| serde_json::json!({ "raw": raw }));
        Ok((status, parsed))
    }

    async fn upload_image_with_retry(&self, image_path: &Path) -> anyhow::Result<String> {
        let token = self.get_tenant_access_token().await?;
        let (status, response) = self.upload_image_once(&token, image_path).await?;

        if should_refresh_lark_tenant_token(status, &response) {
            self.invalidate_token().await;
            let refreshed_token = self.get_tenant_access_token().await?;
            let (retry_status, retry_response) =
                self.upload_image_once(&refreshed_token, image_path).await?;

            if should_refresh_lark_tenant_token(retry_status, &retry_response) {
                anyhow::bail!(
                    "Lark image upload failed after token refresh: status={retry_status}, body={retry_response}"
                );
            }

            ensure_lark_send_success(
                retry_status,
                &retry_response,
                "image upload after token refresh",
            )?;
            return retry_response
                .pointer("/data/image_key")
                .and_then(|value| value.as_str())
                .map(ToString::to_string)
                .ok_or_else(|| anyhow::anyhow!("missing image_key in Lark image upload response"));
        }

        ensure_lark_send_success(status, &response, "image upload without token refresh")?;
        response
            .pointer("/data/image_key")
            .and_then(|value| value.as_str())
            .map(ToString::to_string)
            .ok_or_else(|| anyhow::anyhow!("missing image_key in Lark image upload response"))
    }

    async fn upload_file_with_retry(
        &self,
        file_path: &Path,
        file_type: &str,
    ) -> anyhow::Result<String> {
        let token = self.get_tenant_access_token().await?;
        let (status, response) = self.upload_file_once(&token, file_path, file_type).await?;

        if should_refresh_lark_tenant_token(status, &response) {
            self.invalidate_token().await;
            let refreshed_token = self.get_tenant_access_token().await?;
            let (retry_status, retry_response) = self
                .upload_file_once(&refreshed_token, file_path, file_type)
                .await?;

            if should_refresh_lark_tenant_token(retry_status, &retry_response) {
                anyhow::bail!(
                    "Lark file upload failed after token refresh: status={retry_status}, body={retry_response}"
                );
            }

            ensure_lark_send_success(
                retry_status,
                &retry_response,
                "file upload after token refresh",
            )?;
            return retry_response
                .pointer("/data/file_key")
                .and_then(|value| value.as_str())
                .map(ToString::to_string)
                .ok_or_else(|| anyhow::anyhow!("missing file_key in Lark file upload response"));
        }

        ensure_lark_send_success(status, &response, "file upload without token refresh")?;
        response
            .pointer("/data/file_key")
            .and_then(|value| value.as_str())
            .map(ToString::to_string)
            .ok_or_else(|| anyhow::anyhow!("missing file_key in Lark file upload response"))
    }

    async fn send_json_message(
        &self,
        body: &serde_json::Value,
        reply_to_message_id: Option<&str>,
        recipient: Option<&str>,
    ) -> anyhow::Result<()> {
        let _ = self
            .send_json_message_with_response(body, reply_to_message_id, recipient)
            .await?;
        Ok(())
    }

    async fn send_text_message(
        &self,
        recipient: &str,
        content: &str,
        reply_to_message_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let body = if reply_to_message_id.is_some() {
            build_lark_reply_message_body("post", build_lark_post_content(content), true)
        } else {
            build_lark_text_message_body(recipient, content)
        };
        self.send_json_message(&body, reply_to_message_id, Some(recipient))
            .await
    }

    async fn note_message_unavailable(&self, message_id: &str, body: &serde_json::Value) {
        let Some(code) = extract_lark_response_code(body) else {
            return;
        };
        if !is_lark_terminal_message_code(code) || message_id.trim().is_empty() {
            return;
        }

        let mut cache = self.unavailable_message_ids.write().await;
        cache.retain(|_, (_, marked_at)| marked_at.elapsed() < LARK_MESSAGE_UNAVAILABLE_TTL);
        cache.insert(message_id.to_string(), (code, Instant::now()));
    }

    async fn is_message_unavailable(&self, message_id: &str) -> bool {
        if message_id.trim().is_empty() {
            return false;
        }

        let mut cache = self.unavailable_message_ids.write().await;
        cache.retain(|_, (_, marked_at)| marked_at.elapsed() < LARK_MESSAGE_UNAVAILABLE_TTL);
        cache.contains_key(message_id)
    }

    fn finalized_card_phase(text: &str) -> LarkCardPhase {
        if text.trim_start().starts_with("\u{26A0}\u{FE0F} Error:")
            || text.trim_start().starts_with("⚠️ Error:")
        {
            LarkCardPhase::Failed
        } else {
            LarkCardPhase::Completed
        }
    }

    fn build_final_card(text: &str) -> LarkCardMessage {
        if let Some(explicit_card) =
            LarkOutboundRequest::from_send_message(&SendMessage::new(text, ""), text).card
        {
            explicit_card
        } else {
            build_lark_streaming_card(Self::finalized_card_phase(text), text)
        }
    }

    fn should_resend_after_draft_finalize(
        recipient: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> bool {
        let message = SendMessage::new(text, recipient).in_thread(thread_ts.map(str::to_string));
        let outbound = LarkOutboundRequest::from_send_message(&message, text);
        outbound.has_local_attachments()
            || outbound.has_remote_attachments()
            || outbound.attachment_path().is_some()
    }

    async fn resend_after_draft_finalize(
        &self,
        recipient: &str,
        thread_ts: Option<&str>,
        message_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        if let Err(err) = self.delete_message_with_retry(message_id).await {
            tracing::warn!("lark: failed to delete draft {message_id} before resend: {err}");
        }

        <Self as Channel>::send(
            self,
            &SendMessage::new(text, recipient).in_thread(thread_ts.map(str::to_string)),
        )
        .await
    }

    async fn patch_streaming_draft(
        &self,
        message_id: &str,
        phase: LarkCardPhase,
        text: &str,
    ) -> anyhow::Result<()> {
        let card = build_lark_streaming_card(phase, text);
        let body = serde_json::json!({
            "content": card.content.to_string(),
        });
        self.patch_message_with_retry(message_id, &body).await
    }

    async fn patch_finalized_draft(&self, message_id: &str, text: &str) -> anyhow::Result<()> {
        let card = Self::build_final_card(text);
        let body = serde_json::json!({
            "content": card.content.to_string(),
        });
        self.patch_message_with_retry(message_id, &body).await
    }

    async fn download_message_resource(
        &self,
        message_id: &str,
        file_key: &str,
        resource_type: &str,
    ) -> anyhow::Result<(Vec<u8>, Option<String>, Option<String>)> {
        let build_url = || {
            format!(
                "{}/im/v1/messages/{message_id}/resources/{file_key}?type={resource_type}",
                self.api_base()
            )
        };

        let fetch_once = async |token: &str| -> anyhow::Result<reqwest::Response> {
            let response = self
                .http_client()
                .get(build_url())
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await?;
            Ok(response)
        };

        let token = self.get_tenant_access_token().await?;
        let mut response = fetch_once(&token).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.invalidate_token().await;
            let refreshed = self.get_tenant_access_token().await?;
            response = fetch_once(&refreshed).await?;
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Lark inbound resource download failed: message_id={message_id} file_key={file_key} type={resource_type} status={status} body={body}"
            );
        }

        let content_type = content_type_from_response(&response);
        let file_name = extract_response_file_name(&response);
        let bytes = response.bytes().await?.to_vec();
        Ok((bytes, content_type, file_name))
    }

    async fn materialize_inbound_resources(
        &self,
        parsed: &LarkParsedMessage,
    ) -> Vec<LarkDownloadedResource> {
        let Some(workspace_dir) = self.workspace_dir.as_deref() else {
            return Vec::new();
        };

        let mut downloaded = Vec::new();
        for resource in &parsed.resources {
            match self
                .download_message_resource(
                    &parsed.message_id,
                    &resource.file_key,
                    resource.kind.resource_type(),
                )
                .await
            {
                Ok((bytes, content_type, file_name)) => match store_inbound_resource_with_limit(
                    workspace_dir,
                    self.channel_name(),
                    self.account_id(),
                    &parsed.message_id,
                    resource,
                    &bytes,
                    content_type.as_deref(),
                    file_name.as_deref(),
                    LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES,
                )
                .await
                {
                    Ok(saved) => downloaded.push(saved),
                    Err(err) => tracing::warn!(
                        "Lark: failed to store inbound {} resource {}: {err}",
                        resource.kind.resource_type(),
                        resource.file_key
                    ),
                },
                Err(err) => tracing::warn!(
                    "Lark: failed to download inbound {} resource {}: {err}",
                    resource.kind.resource_type(),
                    resource.file_key
                ),
            }
        }

        downloaded
    }

    fn build_inbound_content(
        &self,
        parsed: &LarkParsedMessage,
        downloaded: &[LarkDownloadedResource],
    ) -> String {
        if downloaded.is_empty()
            && matches!(parsed.message_type.as_str(), "sticker" | "folder")
            && !parsed.normalized_content.trim().is_empty()
        {
            return parsed.normalized_content.trim().to_string();
        }

        let mut blocks = Vec::new();

        for resource in &parsed.resources {
            if let Some(saved) = downloaded
                .iter()
                .find(|item| item.file_key == resource.file_key)
            {
                blocks.push(format!(
                    "[{}:{}]",
                    resource.kind.marker_label(),
                    saved.path.display()
                ));
            } else {
                blocks.push(resource.kind.placeholder().to_string());
            }
        }

        let text = parsed.text.trim();
        if !text.is_empty() {
            blocks.push(text.to_string());
        } else if blocks.is_empty() {
            let normalized = parsed.normalized_content.trim();
            if !normalized.is_empty() {
                blocks.push(normalized.to_string());
            }
        }

        blocks.join("\n\n")
    }

    pub(super) fn parse_inbound_message(
        &self,
        sender_open_id: &str,
        message: &LarkMessage,
    ) -> Option<LarkParsedMessage> {
        self.parse_inbound_message_from_content(sender_open_id, message, &message.content)
    }

    pub(super) async fn parsed_to_channel_message(
        &self,
        parsed: LarkParsedMessage,
    ) -> Option<ChannelMessage> {
        let downloaded = self.materialize_inbound_resources(&parsed).await;
        let mut content = if parsed.resources.is_empty() {
            parsed.normalized_content.clone()
        } else {
            self.build_inbound_content(&parsed, &downloaded)
        };

        if let Some(parent_id) = parsed.parent_id.as_deref() {
            match self.fetch_parent_message_content(parent_id).await {
                Ok(Some(quoted)) => {
                    content = format!("[quoted message {parent_id}]\n{quoted}\n\n{content}");
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::debug!("Lark: failed to fetch quoted parent {}: {err}", parent_id);
                }
            }
        }

        if content.trim().is_empty() {
            return None;
        }

        let thread_ts = parsed.thread_id.clone();
        let context = Some(Self::build_channel_message_context(&parsed));
        Some(ChannelMessage {
            id: parsed.message_id.clone(),
            sender: Self::parsed_sender_session_key(&parsed),
            reply_target: parsed.chat_id,
            content,
            channel: self.channel_name().to_string(),
            timestamp: parsed.create_time_secs.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            }),
            thread_ts,
            context,
        })
    }

    pub(super) fn parse_card_action_event(
        &self,
        payload: &serde_json::Value,
    ) -> Vec<ChannelMessage> {
        let Some(event) = parse_lark_card_action_event(payload) else {
            return Vec::new();
        };
        let reply_target = event.chat_id.clone().unwrap_or_default();
        if reply_target.is_empty() {
            return Vec::new();
        }

        vec![ChannelMessage {
            id: event
                .message_id
                .clone()
                .unwrap_or_else(|| format!("lark-card-action:{reply_target}")),
            sender: event
                .operator_open_id
                .clone()
                .unwrap_or_else(|| reply_target.clone()),
            reply_target,
            content: render_lark_card_action_event_content(&event),
            channel: self.channel_name().to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            thread_ts: event.message_id,
            context: None,
        }]
    }

    pub fn parse_event_payload(&self, payload: &serde_json::Value) -> Vec<ChannelMessage> {
        let event_type = payload
            .pointer("/header/event_type")
            .and_then(|e| e.as_str())
            .unwrap_or("");
        if event_type == "card.action.trigger" {
            return self.parse_card_action_event(payload.get("event").unwrap_or(payload));
        }
        if event_type != "im.message.receive_v1" {
            return Vec::new();
        }

        let event = match payload.get("event") {
            Some(event) => event,
            None => return Vec::new(),
        };

        let sender_open_id = event
            .pointer("/sender/sender_id/open_id")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let message: LarkMessage = match serde_json::from_value(
            event
                .get("message")
                .cloned()
                .unwrap_or(serde_json::json!({})),
        ) {
            Ok(message) => message,
            Err(err) => {
                tracing::warn!("Lark: failed to parse event message payload: {err}");
                return Vec::new();
            }
        };

        let Some(parsed) = self.parse_inbound_message(sender_open_id, &message) else {
            return Vec::new();
        };
        let content = if parsed.resources.is_empty() {
            parsed.normalized_content.clone()
        } else {
            self.build_inbound_content(&parsed, &[])
        };
        if content.trim().is_empty() {
            return Vec::new();
        }

        let thread_ts = parsed.thread_id.clone();
        let context = Some(Self::build_channel_message_context(&parsed));
        vec![ChannelMessage {
            id: parsed.message_id.clone(),
            sender: Self::parsed_sender_session_key(&parsed),
            reply_target: parsed.chat_id,
            content,
            channel: self.channel_name().to_string(),
            timestamp: parsed.create_time_secs.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            }),
            thread_ts,
            context,
        }]
    }

    pub async fn parse_event_payload_async(
        &self,
        payload: &serde_json::Value,
    ) -> Vec<ChannelMessage> {
        let event_type = payload
            .pointer("/header/event_type")
            .and_then(|e| e.as_str())
            .unwrap_or("");
        if event_type == "card.action.trigger" {
            return self.parse_card_action_event(payload.get("event").unwrap_or(payload));
        }
        if event_type != "im.message.receive_v1" {
            return Vec::new();
        }

        let event = match payload.get("event") {
            Some(event) => event,
            None => return Vec::new(),
        };

        let sender_open_id = event
            .pointer("/sender/sender_id/open_id")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let message: LarkMessage = match serde_json::from_value(
            event
                .get("message")
                .cloned()
                .unwrap_or(serde_json::json!({})),
        ) {
            Ok(message) => message,
            Err(err) => {
                tracing::warn!("Lark: failed to parse event message payload: {err}");
                return Vec::new();
            }
        };

        let full_content = if message.message_type == "interactive" {
            self.fetch_interactive_message_content(&message.message_id)
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| message.content.clone())
        } else {
            message.content.clone()
        };

        let Some(mut parsed) =
            self.parse_inbound_message_from_content(sender_open_id, &message, &full_content)
        else {
            return Vec::new();
        };
        if message.message_type == "merge_forward" {
            parsed.normalized_content = self
                .fetch_merge_forward_content(&message.message_id)
                .await
                .unwrap_or_else(|_| "<forwarded_messages/>".to_string());
        }
        self.parsed_to_channel_message(parsed)
            .await
            .into_iter()
            .collect()
    }

    async fn send_attachment_path(
        &self,
        recipient: &str,
        path: &Path,
        kind: LarkAttachmentKind,
        reply_to_message_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let path = super::media_source::validate_explicit_local_media_path(
            path.to_path_buf(),
            self.workspace_dir.as_deref(),
        )?;
        let body = match kind {
            LarkAttachmentKind::Image => {
                let image_key = self.upload_image_with_retry(&path).await?;
                if reply_to_message_id.is_some() {
                    build_lark_reply_message_body(
                        "image",
                        serde_json::json!({ "image_key": image_key }),
                        true,
                    )
                } else {
                    build_lark_image_message_body(recipient, &image_key)
                }
            }
            LarkAttachmentKind::Document
            | LarkAttachmentKind::Audio
            | LarkAttachmentKind::Video => {
                let file_type = Self::detect_upload_file_type(&path, kind);
                let file_key = self.upload_file_with_retry(&path, file_type).await?;
                match (kind, reply_to_message_id) {
                    (LarkAttachmentKind::Document, Some(_)) => build_lark_reply_message_body(
                        "file",
                        serde_json::json!({ "file_key": file_key }),
                        true,
                    ),
                    (LarkAttachmentKind::Document, None) => {
                        build_lark_file_message_body(recipient, &file_key)
                    }
                    (LarkAttachmentKind::Audio, Some(_)) => build_lark_reply_message_body(
                        "audio",
                        serde_json::json!({ "file_key": file_key }),
                        true,
                    ),
                    (LarkAttachmentKind::Audio, None) => {
                        build_lark_audio_message_body(recipient, &file_key)
                    }
                    (LarkAttachmentKind::Video, Some(_)) => build_lark_reply_message_body(
                        "media",
                        serde_json::json!({ "file_key": file_key }),
                        true,
                    ),
                    (LarkAttachmentKind::Video, None) => {
                        build_lark_video_message_body(recipient, &file_key)
                    }
                    _ => unreachable!(),
                }
            }
        };

        self.send_json_message(&body, reply_to_message_id, Some(recipient))
            .await
    }

    async fn draft_session(&self, message_id: &str) -> Option<LarkDraftSession> {
        self.draft_sessions.read().await.get(message_id).cloned()
    }

    async fn remove_draft_session(&self, message_id: &str) {
        self.draft_sessions.write().await.remove(message_id);
    }

    async fn register_draft_session(
        &self,
        recipient: &str,
        message_id: &str,
        thread_ts: Option<&str>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        self.draft_sessions
            .write()
            .await
            .insert(message_id.to_string(), LarkDraftSession { commands: tx });

        let channel = self.clone();
        let recipient = recipient.to_string();
        let message_id = message_id.to_string();
        let thread_ts = thread_ts.map(str::to_string);
        tokio::spawn(async move {
            channel
                .run_draft_session_worker(recipient, message_id, thread_ts, rx)
                .await;
        });
    }

    async fn run_draft_session_worker(
        self,
        recipient: String,
        message_id: String,
        thread_ts: Option<String>,
        mut rx: mpsc::UnboundedReceiver<LarkDraftCommand>,
    ) {
        let mut pending_text: Option<String> = None;
        let mut next_flush_at: Option<Instant> = None;
        let mut last_flush_at = Instant::now();

        loop {
            if let Some(deadline) = next_flush_at {
                tokio::select! {
                    maybe_cmd = rx.recv() => {
                        let Some(cmd) = maybe_cmd else {
                            break;
                        };
                        match cmd {
                            LarkDraftCommand::Update(text) => {
                                pending_text = Some(text);
                                next_flush_at = Some(next_lark_stream_flush_deadline(last_flush_at, Instant::now()));
                            }
                            LarkDraftCommand::Finalize { text, result_tx } => {
                                let result = if Self::should_resend_after_draft_finalize(
                                    &recipient,
                                    thread_ts.as_deref(),
                                    &text,
                                ) {
                                    self.resend_after_draft_finalize(
                                        &recipient,
                                        thread_ts.as_deref(),
                                        &message_id,
                                        &text,
                                    ).await
                                } else {
                                    self.patch_finalized_draft(&message_id, &text).await
                                };
                                let _ = result_tx.send(result);
                                self.remove_draft_session(&message_id).await;
                                return;
                            }
                            LarkDraftCommand::Cancel { result_tx } => {
                                let result = self.delete_message_with_retry(&message_id).await;
                                let _ = result_tx.send(result);
                                self.remove_draft_session(&message_id).await;
                                return;
                            }
                        }
                    }
                    _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                        if let Some(text) = pending_text.take() {
                            if let Err(err) = self
                                .patch_streaming_draft(&message_id, LarkCardPhase::Generating, &text)
                                .await
                            {
                                tracing::warn!("lark: failed to patch draft {message_id}: {err}");
                            } else {
                                last_flush_at = Instant::now();
                            }
                        }
                        next_flush_at = None;
                    }
                }
            } else {
                let Some(cmd) = rx.recv().await else {
                    break;
                };
                match cmd {
                    LarkDraftCommand::Update(text) => {
                        pending_text = Some(text);
                        next_flush_at = Some(next_lark_stream_flush_deadline(
                            last_flush_at,
                            Instant::now(),
                        ));
                    }
                    LarkDraftCommand::Finalize { text, result_tx } => {
                        let result = if Self::should_resend_after_draft_finalize(
                            &recipient,
                            thread_ts.as_deref(),
                            &text,
                        ) {
                            self.resend_after_draft_finalize(
                                &recipient,
                                thread_ts.as_deref(),
                                &message_id,
                                &text,
                            )
                            .await
                        } else {
                            self.patch_finalized_draft(&message_id, &text).await
                        };
                        let _ = result_tx.send(result);
                        self.remove_draft_session(&message_id).await;
                        return;
                    }
                    LarkDraftCommand::Cancel { result_tx } => {
                        let result = self.delete_message_with_retry(&message_id).await;
                        let _ = result_tx.send(result);
                        self.remove_draft_session(&message_id).await;
                        return;
                    }
                }
            }
        }

        self.remove_draft_session(&message_id).await;
    }
}

#[async_trait]
impl Channel for LarkChannel {
    fn name(&self) -> &str {
        self.channel_name()
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        let raw_content = strip_tool_call_tags(&message.content);
        let outbound = LarkOutboundRequest::from_send_message(message, &raw_content);
        let reply_to_message_id = outbound.reply_message_id();
        let path_only_attachment = outbound.attachment_path();
        let path_only_text =
            path_only_attachment.is_some() && outbound.text.trim() == raw_content.trim();

        if !outbound.unresolved_markers.is_empty() {
            tracing::warn!(
                unresolved = ?outbound.unresolved_markers,
                "lark: unresolved attachment markers were sent as plain text"
            );
        }

        if let Some(card) = &outbound.card {
            let body = if reply_to_message_id.is_some() {
                build_lark_reply_card_message_body(card, true)
            } else {
                build_lark_card_message_body(&outbound.target, card)
            };
            return self
                .send_json_message(&body, reply_to_message_id, Some(&outbound.target))
                .await;
        }

        if !outbound.text.is_empty() && !path_only_text {
            self.send_text_message(&outbound.target, &outbound.text, reply_to_message_id)
                .await?;
        }

        for attachment in &outbound.local_attachments {
            let path = Path::new(&attachment.target);
            self.send_attachment_path(
                &outbound.target,
                path,
                attachment.kind,
                reply_to_message_id,
            )
            .await?;
        }

        for (kind, target) in &outbound.remote_attachments {
            let path = materialize_outbound_attachment(
                &self.http_client(),
                self.workspace_dir.as_deref(),
                self.media_max_bytes
                    .unwrap_or(LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES),
                self.media_local_roots.clone(),
                self.channel_name(),
                self.account_id(),
                Self::outbound_resource_kind(*kind),
                target,
            )
            .await?;
            self.send_attachment_path(&outbound.target, &path, *kind, reply_to_message_id)
                .await?;
        }

        if let Some((path, kind)) = path_only_attachment {
            self.send_attachment_path(&outbound.target, path, kind, reply_to_message_id)
                .await?;
        }

        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        use crate::config::schema::LarkReceiveMode;
        match self.receive_mode {
            LarkReceiveMode::Websocket => self.listen_ws(tx).await,
            LarkReceiveMode::Webhook => self.listen_http(tx).await,
        }
    }

    async fn health_check(&self) -> bool {
        self.probe_health().await.is_healthy()
    }

    fn supports_draft_updates(&self) -> bool {
        true
    }

    async fn send_draft(&self, message: &SendMessage) -> anyhow::Result<Option<String>> {
        let recipient =
            normalize_lark_target(&message.recipient).unwrap_or_else(|| message.recipient.clone());
        let card = build_lark_streaming_card(LarkCardPhase::Thinking, &message.content);
        let body = if message.thread_ts.is_some() {
            build_lark_reply_card_message_body(&card, true)
        } else {
            build_lark_card_message_body(&recipient, &card)
        };
        let response = self
            .send_json_message_with_response(&body, message.thread_ts.as_deref(), Some(&recipient))
            .await?;
        let message_id = response
            .pointer("/data/message_id")
            .and_then(|value| value.as_str())
            .map(str::to_string);

        if let Some(message_id) = message_id.as_deref() {
            self.register_draft_session(&recipient, message_id, message.thread_ts.as_deref())
                .await;
        }

        Ok(message_id)
    }

    async fn update_draft(
        &self,
        _recipient: &str,
        message_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        if let Some(session) = self.draft_session(message_id).await {
            session
                .commands
                .send(LarkDraftCommand::Update(text.to_string()))
                .map_err(|_| anyhow::anyhow!("lark draft session is no longer available"))?;
            return Ok(());
        }

        self.patch_streaming_draft(message_id, LarkCardPhase::Generating, text)
            .await
    }

    async fn finalize_draft(
        &self,
        recipient: &str,
        message_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        if let Some(session) = self.draft_session(message_id).await {
            let (result_tx, result_rx) = oneshot::channel();
            session
                .commands
                .send(LarkDraftCommand::Finalize {
                    text: text.to_string(),
                    result_tx,
                })
                .map_err(|_| anyhow::anyhow!("lark draft session is no longer available"))?;
            return result_rx
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("lark draft worker terminated")));
        }

        if Self::should_resend_after_draft_finalize(recipient, None, text) {
            return self
                .resend_after_draft_finalize(recipient, None, message_id, text)
                .await;
        }

        self.patch_finalized_draft(message_id, text).await
    }

    async fn cancel_draft(&self, _recipient: &str, message_id: &str) -> anyhow::Result<()> {
        if let Some(session) = self.draft_session(message_id).await {
            let (result_tx, result_rx) = oneshot::channel();
            session
                .commands
                .send(LarkDraftCommand::Cancel { result_tx })
                .map_err(|_| anyhow::anyhow!("lark draft session is no longer available"))?;
            return result_rx
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("lark draft worker terminated")));
        }

        self.delete_message_with_retry(message_id).await
    }

    async fn add_reaction(
        &self,
        _channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        let _ = self
            .execute_reaction_request("add reaction", |token| {
                self.post_message_reaction_with_token(message_id, token, emoji)
            })
            .await?;
        Ok(())
    }

    async fn remove_reaction(
        &self,
        _channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        for reaction_id in self.list_bot_reaction_ids(message_id, Some(emoji)).await? {
            let _ = self
                .execute_reaction_request("remove reaction", |token| {
                    self.delete_message_reaction_with_token(message_id, &reaction_id, token)
                })
                .await?;
        }
        Ok(())
    }
}
