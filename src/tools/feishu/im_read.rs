use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct FeishuImReadTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuImReadTool {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            test_api_base: None,
        }
    }

    #[cfg(test)]
    fn with_api_base_for_test(mut self, api_base: impl Into<String>) -> Self {
        self.test_api_base = Some(api_base.into());
        self
    }

    fn build_client(&self, account: Option<&str>) -> anyhow::Result<FeishuToolClient> {
        let client = FeishuToolClient::from_config(Arc::clone(&self.config), account)?;
        Ok(match &self.test_api_base {
            Some(api_base) => client.with_api_base_for_test(api_base.clone()),
            None => client,
        })
    }
}

#[async_trait]
impl Tool for FeishuImReadTool {
    fn name(&self) -> &str {
        "feishu_im_read"
    }

    fn description(&self) -> &str {
        "Read Feishu IM message details for a known message ID."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "account": {
                    "type": "string",
                    "description": "Optional Feishu account name."
                },
                "message_id": {
                    "type": "string",
                    "description": "Feishu message ID to fetch"
                }
            },
            "required": ["message_id"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        async {
            let account = args.get("account").and_then(serde_json::Value::as_str);
            let message_id = args
                .get("message_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'message_id' parameter"))?;
            let client = self.build_client(account)?;
            let response = client
                .get_json(&format!("/im/v1/messages/{message_id}"))
                .await?;
            let output = json!({
                "account": client.account_name(),
                "message_id": message_id,
                "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
            });

            Ok(ToolResult {
                success: true,
                output: serde_json::to_string_pretty(&output)?,
                error: None,
            })
        }
        .await
        .map_err(|err| annotate_feishu_tool_error(self.name(), err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::LarkReceiveMode;
    use crate::config::{ChannelsConfig, FeishuConfig};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_config() -> Arc<Config> {
        Arc::new(Config {
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
        })
    }

    #[tokio::test]
    async fn read_message_fetches_im_message_detail() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v3/tenant_access_token/internal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "tenant_access_token": "tenant_token"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/im/v1/messages/om_123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": {
                    "message": {
                        "message_id": "om_123",
                        "msg_type": "text"
                    }
                }
            })))
            .mount(&server)
            .await;

        let tool = FeishuImReadTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({ "message_id": "om_123" }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("\"om_123\""));
        assert!(result.output.contains("\"msg_type\": \"text\""));
    }
}
