use crate::config::{build_runtime_proxy_client_with_timeouts, Config, FeishuConfig};
use crate::security::feishu_tool_scope_requirements;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

const FEISHU_OPEN_API_BASE: &str = "https://open.feishu.cn/open-apis";
const FEISHU_TIMEOUT_SECS: u64 = 30;
const FEISHU_CONNECT_TIMEOUT_SECS: u64 = 10;

#[derive(Clone)]
pub(crate) struct FeishuToolClient {
    account_name: String,
    app_id: String,
    app_secret: String,
    api_base: String,
    http_client: reqwest::Client,
}

impl FeishuToolClient {
    pub(crate) fn from_config(config: Arc<Config>, account: Option<&str>) -> anyhow::Result<Self> {
        let (account_name, account_config) = resolve_feishu_account(config.as_ref(), account)?;
        Ok(Self {
            account_name,
            app_id: account_config.app_id,
            app_secret: account_config.app_secret,
            api_base: FEISHU_OPEN_API_BASE.to_string(),
            http_client: build_runtime_proxy_client_with_timeouts(
                "channel.feishu",
                FEISHU_TIMEOUT_SECS,
                FEISHU_CONNECT_TIMEOUT_SECS,
            ),
        })
    }

    #[allow(dead_code)]
    pub(crate) fn with_api_base_for_test(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = api_base.into();
        self
    }

    pub(crate) fn account_name(&self) -> &str {
        &self.account_name
    }

    pub(crate) fn api_base(&self) -> &str {
        &self.api_base
    }

    pub(crate) async fn tenant_access_token(&self) -> anyhow::Result<String> {
        let response = self
            .http_client
            .post(format!(
                "{}/auth/v3/tenant_access_token/internal",
                self.api_base
            ))
            .json(&serde_json::json!({
                "app_id": self.app_id,
                "app_secret": self.app_secret,
            }))
            .send()
            .await?;
        let status = response.status();
        let payload = response.text().await?;
        if !status.is_success() {
            anyhow::bail!(
                "Feishu tenant_access_token request failed: status={status}, body={payload}"
            );
        }
        let json: Value = serde_json::from_str(&payload)?;
        let code = json.get("code").and_then(Value::as_i64).unwrap_or_default();
        if code != 0 {
            let msg = json
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            anyhow::bail!("Feishu tenant_access_token failed: {msg}");
        }
        json.get("tenant_access_token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("missing tenant_access_token in response"))
    }

    pub(crate) async fn get_json(&self, path: &str) -> anyhow::Result<Value> {
        let token = self.tenant_access_token().await?;
        let response = self
            .http_client
            .get(format!("{}{}", self.api_base, path))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        parse_json_response(response).await
    }

    pub(crate) async fn get_json_with_query(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> anyhow::Result<Value> {
        let token = self.tenant_access_token().await?;
        let response = self
            .http_client
            .get(format!("{}{}", self.api_base, path))
            .query(query)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        parse_json_response(response).await
    }

    pub(crate) async fn post_json(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let token = self.tenant_access_token().await?;
        let response = self
            .http_client
            .post(format!("{}{}", self.api_base, path))
            .header("Authorization", format!("Bearer {token}"))
            .json(body)
            .send()
            .await?;
        parse_json_response(response).await
    }

    pub(crate) async fn patch_json(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let token = self.tenant_access_token().await?;
        let response = self
            .http_client
            .patch(format!("{}{}", self.api_base, path))
            .header("Authorization", format!("Bearer {token}"))
            .json(body)
            .send()
            .await?;
        parse_json_response(response).await
    }

    pub(crate) async fn put_json(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let token = self.tenant_access_token().await?;
        let response = self
            .http_client
            .put(format!("{}{}", self.api_base, path))
            .header("Authorization", format!("Bearer {token}"))
            .json(body)
            .send()
            .await?;
        parse_json_response(response).await
    }

    pub(crate) async fn delete(&self, path: &str) -> anyhow::Result<Value> {
        let token = self.tenant_access_token().await?;
        let response = self
            .http_client
            .delete(format!("{}{}", self.api_base, path))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        parse_json_response(response).await
    }

    pub(crate) async fn post_multipart(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> anyhow::Result<Value> {
        let token = self.tenant_access_token().await?;
        let response = self
            .http_client
            .post(format!("{}{}", self.api_base, path))
            .header("Authorization", format!("Bearer {token}"))
            .multipart(form)
            .send()
            .await?;
        parse_json_response(response).await
    }

    pub(crate) async fn download_bytes(
        &self,
        path: &str,
    ) -> anyhow::Result<(Vec<u8>, Option<String>, Option<String>)> {
        let token = self.tenant_access_token().await?;
        let response = self
            .http_client
            .get(format!("{}{}", self.api_base, path))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Feishu download failed: status={status}, body={body}");
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let file_name = response
            .headers()
            .get(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_disposition_filename);
        let bytes = response.bytes().await?.to_vec();
        Ok((bytes, content_type, file_name))
    }

    pub(crate) async fn upload_file(
        &self,
        path: &str,
        file_path: &Path,
        text_fields: &[(&str, &str)],
    ) -> anyhow::Result<Value> {
        let file_name = file_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("file");
        let mime = mime_guess::from_path(file_path).first_or_octet_stream();
        let file_bytes = std::fs::read(file_path)?;
        let file_part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(file_name.to_string())
            .mime_str(mime.as_ref())?;

        let mut form = reqwest::multipart::Form::new().part("file", file_part);
        for (key, value) in text_fields {
            form = form.text((*key).to_string(), (*value).to_string());
        }

        self.post_multipart(path, form).await
    }
}

fn resolve_feishu_account(
    config: &Config,
    account: Option<&str>,
) -> anyhow::Result<(String, FeishuConfig)> {
    let resolved = config
        .channels_config
        .resolve_feishu_account_profile(account);
    if !resolved.enabled || !resolved.configured {
        anyhow::bail!("No matching Feishu account is configured");
    }

    let cfg = resolved
        .config
        .ok_or_else(|| anyhow::anyhow!("No matching Feishu account is configured"))?;
    Ok((resolved.channel_name, cfg.clone()))
}

fn parse_content_disposition_filename(header: &str) -> Option<String> {
    for part in header.split(';') {
        let trimmed = part.trim();
        if let Some(file_name) = trimmed.strip_prefix("filename=") {
            return Some(file_name.trim_matches('"').to_string());
        }
    }
    None
}

pub(crate) fn annotate_feishu_tool_error(tool_name: &str, err: anyhow::Error) -> anyhow::Error {
    let Some(requirements) = feishu_tool_scope_requirements(tool_name) else {
        return err;
    };

    let message = err.to_string();
    let normalized = message.to_ascii_lowercase();
    let looks_like_scope_issue = normalized.contains("permission")
        || normalized.contains("scope")
        || normalized.contains("forbidden")
        || normalized.contains("no permission")
        || normalized.contains("access denied");

    if !looks_like_scope_issue {
        return err;
    }

    anyhow::anyhow!(
        "{}\nRequired Feishu scopes for `{}`: {}",
        message,
        tool_name,
        requirements.scopes.join(", ")
    )
}

async fn parse_json_response(response: reqwest::Response) -> anyhow::Result<Value> {
    let status = response.status();
    let payload = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("Feishu API request failed: status={status}, body={payload}");
    }
    let json: Value = serde_json::from_str(&payload)?;
    let code = json.get("code").and_then(Value::as_i64).unwrap_or_default();
    if code != 0 {
        let msg = json
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        anyhow::bail!("Feishu API returned code {code}: {msg}");
    }
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::LarkReceiveMode;
    use crate::config::ChannelsConfig;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn base_config() -> Config {
        Config {
            channels_config: ChannelsConfig {
                feishu: Some(FeishuConfig {
                    app_id: "cli_test_app".to_string(),
                    app_secret: "secret".to_string(),
                    enabled: None,
                    encrypt_key: None,
                    verification_token: None,
                    allowed_users: vec!["*".to_string()],
                    receive_mode: LarkReceiveMode::default(),
                    port: None,
                }),
                ..ChannelsConfig::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn resolves_default_feishu_account() {
        let client = FeishuToolClient::from_config(Arc::new(base_config()), None).unwrap();
        assert_eq!(client.account_name(), "feishu");
    }

    #[test]
    fn resolves_default_feishu_account_before_named_accounts() {
        let mut config = base_config();
        config.channels_config.feishu_accounts.insert(
            "ops".to_string(),
            FeishuConfig {
                app_id: "cli_ops".to_string(),
                app_secret: "ops-secret".to_string(),
                enabled: None,
                encrypt_key: None,
                verification_token: None,
                allowed_users: vec!["*".to_string()],
                receive_mode: LarkReceiveMode::default(),
                port: None,
            },
        );

        let client = FeishuToolClient::from_config(Arc::new(config), None).unwrap();
        assert_eq!(client.account_name(), "feishu");
    }

    #[test]
    fn resolves_prefixed_named_feishu_account() {
        let mut config = Config {
            channels_config: ChannelsConfig::default(),
            ..Config::default()
        };
        config.channels_config.feishu_accounts.insert(
            "ops".to_string(),
            FeishuConfig {
                app_id: "cli_ops".to_string(),
                app_secret: "ops-secret".to_string(),
                enabled: None,
                encrypt_key: None,
                verification_token: None,
                allowed_users: vec!["*".to_string()],
                receive_mode: LarkReceiveMode::default(),
                port: None,
            },
        );

        let client = FeishuToolClient::from_config(Arc::new(config), Some(" Feishu:OPS ")).unwrap();
        assert_eq!(client.account_name(), "feishu:ops");
    }

    #[test]
    fn rejects_unconfigured_feishu_account() {
        let mut config = Config {
            channels_config: ChannelsConfig::default(),
            ..Config::default()
        };
        config.channels_config.feishu = Some(FeishuConfig {
            app_id: "".to_string(),
            app_secret: "".to_string(),
            enabled: None,
            encrypt_key: None,
            verification_token: None,
            allowed_users: vec!["*".to_string()],
            receive_mode: LarkReceiveMode::default(),
            port: None,
        });

        let err = FeishuToolClient::from_config(Arc::new(config), None)
            .err()
            .expect("unconfigured account should fail");
        assert!(err
            .to_string()
            .contains("No matching Feishu account is configured"));
    }

    #[test]
    fn rejects_unknown_feishu_account_without_falling_back() {
        let mut config = base_config();
        config.channels_config.feishu_accounts.insert(
            "ops".to_string(),
            FeishuConfig {
                app_id: "cli_ops".to_string(),
                app_secret: "ops-secret".to_string(),
                enabled: None,
                encrypt_key: None,
                verification_token: None,
                allowed_users: vec!["*".to_string()],
                receive_mode: LarkReceiveMode::default(),
                port: None,
            },
        );

        let err = FeishuToolClient::from_config(Arc::new(config), Some("sales"))
            .err()
            .expect("unknown account should fail");
        assert!(err
            .to_string()
            .contains("No matching Feishu account is configured"));
    }

    #[tokio::test]
    async fn tenant_access_token_uses_internal_token_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v3/tenant_access_token/internal"))
            .and(body_json(serde_json::json!({
                "app_id": "cli_test_app",
                "app_secret": "secret",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "tenant_access_token": "tenant_token"
            })))
            .mount(&server)
            .await;

        let client = FeishuToolClient::from_config(Arc::new(base_config()), None)
            .unwrap()
            .with_api_base_for_test(server.uri());

        let token = client.tenant_access_token().await.unwrap();
        assert_eq!(token, "tenant_token");
    }

    #[test]
    fn annotate_feishu_tool_error_adds_scope_hint_for_permission_failures() {
        let err = annotate_feishu_tool_error(
            "feishu_doc_create",
            anyhow::anyhow!("Feishu API returned code 99991677: permission denied"),
        );
        let rendered = err.to_string();
        assert!(rendered.contains("docs:document"));
        assert!(rendered.contains("drive:drive"));
    }
}
