use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct FeishuDocFetchTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuDocFetchTool {
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
impl Tool for FeishuDocFetchTool {
    fn name(&self) -> &str {
        "feishu_doc_fetch"
    }

    fn description(&self) -> &str {
        "Fetch Feishu Docx metadata and, optionally, document block structure."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "account": { "type": "string", "description": "Optional Feishu account name." },
                "document_id": { "type": "string", "description": "Docx document ID" },
                "include_blocks": {
                    "type": "boolean",
                    "description": "When true, also fetch the document blocks"
                },
                "include_raw_content": {
                    "type": "boolean",
                    "description": "When true, also fetch the document raw content"
                },
                "page_size": {
                    "type": "integer",
                    "description": "Optional block page size when include_blocks=true"
                }
            },
            "required": ["document_id"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        async {
            let account = args.get("account").and_then(serde_json::Value::as_str);
            let document_id = args
                .get("document_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'document_id' parameter"))?;
            let include_blocks = args
                .get("include_blocks")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let include_raw_content = args
                .get("include_raw_content")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let page_size = args
                .get("page_size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(200)
                .min(500);

            let client = self.build_client(account)?;
            let document = client
                .get_json(&format!("/docx/v1/documents/{document_id}"))
                .await?;

            let blocks = if include_blocks {
                Some(
                    client
                        .get_json_with_query(
                            &format!("/docx/v1/documents/{document_id}/blocks"),
                            &[("page_size", page_size.to_string())],
                        )
                        .await?
                        .get("data")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
            } else {
                None
            };
            let raw_content = if include_raw_content {
                Some(
                    client
                        .get_json(&format!("/docx/v1/documents/{document_id}/raw_content"))
                        .await?
                        .get("data")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
            } else {
                None
            };

            let output = json!({
                "account": client.account_name(),
                "document_id": document_id,
                "document": document.get("data").cloned().unwrap_or(serde_json::Value::Null),
                "blocks": blocks,
                "raw_content": raw_content,
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
    use wiremock::matchers::{method, path, query_param};
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
    async fn fetch_doc_reads_metadata_and_blocks() {
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
            .and(path("/docx/v1/documents/doccn123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": {
                    "document": { "document_id": "doccn123", "title": "Project Plan" }
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/docx/v1/documents/doccn123/blocks"))
            .and(query_param("page_size", "50"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "items": [{ "block_id": "blk1", "block_type": 2 }] }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/docx/v1/documents/doccn123/raw_content"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "content": "# Project Plan" }
            })))
            .mount(&server)
            .await;

        let tool = FeishuDocFetchTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "document_id": "doccn123",
                "include_blocks": true,
                "include_raw_content": true,
                "page_size": 50
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Project Plan"));
        assert!(result.output.contains("blk1"));
        assert!(result.output.contains("# Project Plan"));
    }
}
