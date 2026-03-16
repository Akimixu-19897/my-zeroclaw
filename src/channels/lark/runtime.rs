use super::*;
use crate::security::{evaluate_feishu_owner_policy, FeishuOwnerPolicyDisposition};

impl LarkChannel {
    pub(super) async fn post_message_reaction_with_token(
        &self,
        message_id: &str,
        token: String,
        emoji_type: &str,
    ) -> anyhow::Result<reqwest::Response> {
        let url = self.message_reaction_url(message_id);
        let body = serde_json::json!({
            "reaction_type": {
                "emoji_type": emoji_type
            }
        });

        let response = self
            .http_client()
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;

        Ok(response)
    }

    pub(super) async fn list_message_reactions_with_token(
        &self,
        message_id: &str,
        token: String,
        emoji_type: Option<&str>,
        page_token: Option<&str>,
    ) -> anyhow::Result<reqwest::Response> {
        let url = self.message_reaction_url(message_id);
        let mut request = self
            .http_client()
            .get(&url)
            .header("Authorization", format!("Bearer {token}"));
        let mut query_params = vec![("page_size", "50".to_string())];

        if let Some(emoji_type) = emoji_type.filter(|value| !value.is_empty()) {
            query_params.push(("reaction_type", emoji_type.to_string()));
        }
        if let Some(page_token) = page_token.filter(|value| !value.is_empty()) {
            query_params.push(("page_token", page_token.to_string()));
        }

        request = request.query(&query_params);
        Ok(request.send().await?)
    }

    pub(super) async fn delete_message_reaction_with_token(
        &self,
        message_id: &str,
        reaction_id: &str,
        token: String,
    ) -> anyhow::Result<reqwest::Response> {
        let response = self
            .http_client()
            .delete(self.delete_message_reaction_url(message_id, reaction_id))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        Ok(response)
    }

    pub(super) async fn execute_reaction_request<F, Fut>(
        &self,
        operation_name: &str,
        mut request: F,
    ) -> anyhow::Result<serde_json::Value>
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<reqwest::Response>>,
    {
        let mut token = self.get_tenant_access_token().await?;
        let mut retried = false;

        loop {
            let response = request(token.clone()).await?;
            let status = response.status();
            let body: serde_json::Value = response.json().await?;

            if should_refresh_lark_tenant_token(status, &body) && !retried {
                self.invalidate_token().await;
                token = self.get_tenant_access_token().await?;
                retried = true;
                continue;
            }

            ensure_lark_send_success(status, &body, operation_name)?;
            return Ok(body);
        }
    }

    pub(super) async fn list_bot_reaction_ids(
        &self,
        message_id: &str,
        emoji_type: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        let mut reaction_ids = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let current_page_token = page_token.clone();
            let body = self
                .execute_reaction_request("list reaction", |token| {
                    self.list_message_reactions_with_token(
                        message_id,
                        token,
                        emoji_type,
                        current_page_token.as_deref(),
                    )
                })
                .await?;

            let Some(data) = body.get("data") else {
                break;
            };

            if let Some(items) = data.get("items").and_then(|value| value.as_array()) {
                reaction_ids.extend(items.iter().filter_map(|item| {
                    let operator_type = item
                        .pointer("/operator/operator_type")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default();
                    if operator_type != "app" {
                        return None;
                    }

                    item.get("reaction_id")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                }));
            }

            let has_more = data
                .get("has_more")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            page_token = data
                .get("page_token")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string);

            if !has_more || page_token.is_none() {
                break;
            }
        }

        Ok(reaction_ids)
    }

    pub(super) async fn try_add_ack_reaction(&self, message_id: &str, emoji_type: &str) {
        if message_id.is_empty() {
            return;
        }

        if let Err(err) = self.add_reaction("", message_id, emoji_type).await {
            tracing::warn!("Lark: failed to add reaction for {message_id}: {err}");
        }
    }

    pub(super) async fn get_tenant_access_token(&self) -> anyhow::Result<String> {
        {
            let cached = self.tenant_token.read().await;
            if let Some(ref token) = *cached {
                if Instant::now() < token.refresh_after {
                    return Ok(token.value.clone());
                }
            }
        }

        let url = self.tenant_access_token_url();
        let body = serde_json::json!({
            "app_id": self.app_id,
            "app_secret": self.app_secret,
        });

        let resp = self.http_client().post(&url).json(&body).send().await?;
        let status = resp.status();
        let data: serde_json::Value = resp.json().await?;

        if !status.is_success() {
            anyhow::bail!("Lark tenant_access_token request failed: status={status}, body={data}");
        }

        let code = data.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = data
                .get("msg")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Lark tenant_access_token failed: {msg}");
        }

        let token = data
            .get("tenant_access_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing tenant_access_token in response"))?
            .to_string();

        let ttl_seconds = extract_lark_token_ttl_seconds(&data);
        let refresh_after = next_token_refresh_deadline(Instant::now(), ttl_seconds);

        {
            let mut cached = self.tenant_token.write().await;
            *cached = Some(CachedTenantToken {
                value: token.clone(),
                refresh_after,
            });
        }

        Ok(token)
    }

    pub(super) async fn invalidate_token(&self) {
        let mut cached = self.tenant_token.write().await;
        *cached = None;
    }

    async fn fetch_bot_open_id_with_token(
        &self,
        token: &str,
    ) -> anyhow::Result<(reqwest::StatusCode, serde_json::Value)> {
        let resp = self
            .http_client()
            .get(self.bot_info_url())
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        let status = resp.status();
        let body = resp
            .json::<serde_json::Value>()
            .await
            .unwrap_or_else(|_| serde_json::json!({}));
        Ok((status, body))
    }

    async fn refresh_bot_open_id(&self) -> anyhow::Result<Option<String>> {
        let token = self.get_tenant_access_token().await?;
        let (status, body) = self.fetch_bot_open_id_with_token(&token).await?;

        let body = if should_refresh_lark_tenant_token(status, &body) {
            self.invalidate_token().await;
            let refreshed = self.get_tenant_access_token().await?;
            let (retry_status, retry_body) = self.fetch_bot_open_id_with_token(&refreshed).await?;
            if !retry_status.is_success() {
                anyhow::bail!(
                    "Lark bot info request failed after token refresh: status={retry_status}, body={retry_body}"
                );
            }
            retry_body
        } else {
            if !status.is_success() {
                anyhow::bail!("Lark bot info request failed: status={status}, body={body}");
            }
            body
        };

        let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code != 0 {
            anyhow::bail!("Lark bot info failed: code={code}, body={body}");
        }

        let bot_open_id = body
            .pointer("/bot/open_id")
            .or_else(|| body.pointer("/data/bot/open_id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned);

        self.set_resolved_bot_open_id(bot_open_id.clone());
        Ok(bot_open_id)
    }

    pub(super) async fn probe_health(&self) -> LarkHealthProbe {
        let platform = self.platform.channel_name();
        let receive_mode = self.receive_mode_label();
        let owner_policy = evaluate_feishu_owner_policy(&self.allowed_users, self.mention_only);
        let mut probe = LarkHealthProbe {
            probe_kind: "lark_channel",
            platform,
            account_id: self.account_id().to_string(),
            receive_mode,
            owner_policy_disposition: match owner_policy.disposition {
                FeishuOwnerPolicyDisposition::SafeDefault => "safe_default",
                FeishuOwnerPolicyDisposition::Restricted => "restricted",
                FeishuOwnerPolicyDisposition::ReviewRecommended => "review_recommended",
            },
            owner_policy_summary: owner_policy.summary,
            config_status: LarkProbeStatus::Ok,
            token_status: LarkProbeStatus::Skipped,
            transport_status: LarkProbeStatus::Skipped,
            bot_identity_status: LarkProbeStatus::Skipped,
            summary: "ok".to_string(),
        };

        if self.app_id.trim().is_empty() || self.app_secret.trim().is_empty() {
            probe.config_status = LarkProbeStatus::Error;
            probe.summary = "missing app_id or app_secret".to_string();
            return probe;
        }

        match self.get_tenant_access_token().await {
            Ok(_) => probe.token_status = LarkProbeStatus::Ok,
            Err(err) => {
                probe.token_status = LarkProbeStatus::Error;
                probe.summary = format!("tenant token fetch failed: {err}");
                return probe;
            }
        }

        probe.transport_status = match self.receive_mode {
            crate::config::schema::LarkReceiveMode::Websocket => {
                match self.get_ws_endpoint().await {
                    Ok(_) => LarkProbeStatus::Ok,
                    Err(err) => {
                        probe.summary = format!("websocket endpoint probe failed: {err}");
                        LarkProbeStatus::Error
                    }
                }
            }
            crate::config::schema::LarkReceiveMode::Webhook => {
                if self.port.is_some() {
                    LarkProbeStatus::Ok
                } else {
                    probe.summary = "webhook mode requires port".to_string();
                    LarkProbeStatus::Error
                }
            }
        };

        match self.refresh_bot_open_id().await {
            Ok(Some(_)) => probe.bot_identity_status = LarkProbeStatus::Ok,
            Ok(None) => {
                probe.bot_identity_status = LarkProbeStatus::Error;
                probe.summary = "bot open_id missing from bot info response".to_string();
            }
            Err(err) => {
                probe.bot_identity_status = LarkProbeStatus::Error;
                if probe.summary == "ok" {
                    probe.summary = format!("bot identity probe failed: {err}");
                }
            }
        }

        if probe.summary == "ok" && !probe.is_healthy() {
            probe.summary = "probe reported unhealthy state".to_string();
        }

        probe
    }

    pub(super) async fn ensure_bot_open_id(&self) {
        if !self.mention_only || self.resolved_bot_open_id().is_some() {
            return;
        }

        match self.refresh_bot_open_id().await {
            Ok(Some(open_id)) => tracing::info!("Lark: resolved bot open_id: {open_id}"),
            Ok(None) => {
                tracing::warn!(
                    "Lark: bot open_id missing from /bot/v3/info response; mention_only group messages will be ignored"
                );
            }
            Err(err) => {
                tracing::warn!(
                    "Lark: failed to resolve bot open_id: {err}; mention_only group messages will be ignored"
                );
            }
        }
    }
}
