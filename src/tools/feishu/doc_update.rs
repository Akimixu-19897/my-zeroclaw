use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct FeishuDocUpdateTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuDocUpdateTool {
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
impl Tool for FeishuDocUpdateTool {
    fn name(&self) -> &str {
        "feishu_doc_update"
    }

    fn description(&self) -> &str {
        "Update or delete a Feishu Docx document."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["update_title", "delete_document"],
                    "description": "Document mutation to perform. Defaults to update_title."
                },
                "account": { "type": "string", "description": "Optional Feishu account name." },
                "document_id": { "type": "string", "description": "Docx document ID" },
                "title": { "type": "string", "description": "New document title" }
            },
            "required": ["document_id"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        async {
            let action = args
                .get("action")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("update_title");
            let account = args.get("account").and_then(serde_json::Value::as_str);
            let document_id = args
                .get("document_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'document_id' parameter"))?;

            let client = self.build_client(account)?;
            let output = match action {
                "update_title" => {
                    let title = args
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'title' parameter"))?;
                    client
                        .patch_json(
                            &format!("/docx/v1/documents/{document_id}"),
                            &json!({ "title": title }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "document_id": document_id,
                        "title": title,
                        "updated": true,
                    })
                }
                "delete_document" => {
                    client
                        .delete(&format!("/docx/v1/documents/{document_id}"))
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "document_id": document_id,
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
    use wiremock::matchers::{body_json, method, path};
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
                    media_max_mb: None,
                    media_local_roots: Vec::new(),
                }),
                ..ChannelsConfig::default()
            },
            ..Config::default()
        })
    }

    #[tokio::test]
    async fn update_doc_title_patches_document() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v3/tenant_access_token/internal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "tenant_access_token": "tenant_token"
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/docx/v1/documents/doccn123"))
            .and(body_json(json!({ "title": "Renamed Doc" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": {}
            })))
            .mount(&server)
            .await;

        let tool = FeishuDocUpdateTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "update_title",
                "document_id": "doccn123",
                "title": "Renamed Doc"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("\"updated\": true"));
    }

    #[tokio::test]
    async fn delete_document_calls_delete_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v3/tenant_access_token/internal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "tenant_access_token": "tenant_token"
            })))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/docx/v1/documents/doccn123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": {}
            })))
            .mount(&server)
            .await;

        let tool = FeishuDocUpdateTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "delete_document",
                "document_id": "doccn123"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("\"deleted\": true"));
    }
}
