use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct FeishuDocCreateTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuDocCreateTool {
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
impl Tool for FeishuDocCreateTool {
    fn name(&self) -> &str {
        "feishu_doc_create"
    }

    fn description(&self) -> &str {
        "Create a Feishu Docx document, optionally inside a target folder."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "account": { "type": "string", "description": "Optional Feishu account name." },
                "title": { "type": "string", "description": "Document title" },
                "folder_token": {
                    "type": "string",
                    "description": "Optional parent folder token for the new document"
                }
            },
            "required": ["title"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        async {
            let account = args.get("account").and_then(serde_json::Value::as_str);
            let title = args
                .get("title")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'title' parameter"))?;
            let folder_token = args.get("folder_token").and_then(serde_json::Value::as_str);

            let client = self.build_client(account)?;
            let mut body = json!({ "title": title });
            if let Some(folder_token) = folder_token.filter(|value| !value.is_empty()) {
                body["folder_token"] = json!(folder_token);
            }

            let response = client.post_json("/docx/v1/documents", &body).await?;
            let output = json!({
                "account": client.account_name(),
                "title": title,
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
    async fn create_doc_posts_docx_create_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v3/tenant_access_token/internal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "tenant_access_token": "tenant_token"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/docx/v1/documents"))
            .and(body_json(json!({
                "title": "Project Plan",
                "folder_token": "fld_parent_1"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": {
                    "document": {
                        "document_id": "doccn123",
                        "title": "Project Plan"
                    }
                }
            })))
            .mount(&server)
            .await;

        let tool = FeishuDocCreateTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "title": "Project Plan",
                "folder_token": "fld_parent_1"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("doccn123"));
    }
}
