use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct FeishuImMessageTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuImMessageTool {
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
impl Tool for FeishuImMessageTool {
    fn name(&self) -> &str {
        "feishu_im_message"
    }

    fn description(&self) -> &str {
        "Send, reply to, update, or delete Feishu IM messages with configured bot accounts."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["send_text", "reply_text", "update_text", "delete_message"],
                    "description": "IM message operation to perform"
                },
                "account": {
                    "type": "string",
                    "description": "Optional Feishu account name. Use `feishu` for the default account or `primary` / `feishu:primary` for a named account."
                },
                "receive_id": {
                    "type": "string",
                    "description": "Target chat ID for send_text"
                },
                "message_id": {
                    "type": "string",
                    "description": "Message ID for reply_text, update_text, or delete_message"
                },
                "text": {
                    "type": "string",
                    "description": "Text content to send, reply with, or update into the target message"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        async {
            let action = args
                .get("action")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?;
            let account = args.get("account").and_then(serde_json::Value::as_str);
            let client = self.build_client(account)?;

            let output = match action {
                "send_text" => {
                    let receive_id = args
                        .get("receive_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'receive_id' parameter"))?;
                    let text = args
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'text' parameter"))?;
                    let response = client
                        .post_json(
                            "/im/v1/messages?receive_id_type=chat_id",
                            &json!({
                                "receive_id": receive_id,
                                "msg_type": "text",
                                "content": json!({ "text": text }).to_string(),
                            }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "message_id": response.pointer("/data/message_id"),
                    })
                }
                "reply_text" => {
                    let message_id = args
                        .get("message_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'message_id' parameter"))?;
                    let text = args
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'text' parameter"))?;
                    let response = client
                        .post_json(
                            &format!("/im/v1/messages/{message_id}/reply"),
                            &json!({
                                "msg_type": "text",
                                "content": json!({ "text": text }).to_string(),
                            }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "message_id": response.pointer("/data/message_id"),
                    })
                }
                "update_text" => {
                    let message_id = args
                        .get("message_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'message_id' parameter"))?;
                    let text = args
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'text' parameter"))?;
                    client
                        .patch_json(
                            &format!("/im/v1/messages/{message_id}"),
                            &json!({
                                "msg_type": "text",
                                "content": json!({ "text": text }).to_string(),
                            }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "message_id": message_id,
                    })
                }
                "delete_message" => {
                    let message_id = args
                        .get("message_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'message_id' parameter"))?;
                    client
                        .delete(&format!("/im/v1/messages/{message_id}"))
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "message_id": message_id,
                        "deleted": true,
                    })
                }
                other => anyhow::bail!("Unsupported action: {other}"),
            };

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
    use wiremock::matchers::{body_json, method, path, query_param};
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

    async fn mock_token(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/auth/v3/tenant_access_token/internal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "tenant_access_token": "tenant_token"
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn send_text_posts_message_with_chat_receive_id() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("POST"))
            .and(path("/im/v1/messages"))
            .and(query_param("receive_id_type", "chat_id"))
            .and(body_json(json!({
                "receive_id": "oc_chat_1",
                "msg_type": "text",
                "content": "{\"text\":\"hello\"}"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "message_id": "om_123" }
            })))
            .mount(&server)
            .await;

        let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "send_text",
                "receive_id": "oc_chat_1",
                "text": "hello"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("\"message_id\": \"om_123\""));
    }

    #[tokio::test]
    async fn delete_message_calls_delete_endpoint() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("DELETE"))
            .and(path("/im/v1/messages/om_123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": {}
            })))
            .mount(&server)
            .await;

        let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "delete_message",
                "message_id": "om_123"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("\"deleted\": true"));
    }
}
