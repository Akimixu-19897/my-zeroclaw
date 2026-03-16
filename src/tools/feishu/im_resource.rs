use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

pub struct FeishuImResourceTool {
    config: Arc<Config>,
    workspace_dir: PathBuf,
    test_api_base: Option<String>,
}

impl FeishuImResourceTool {
    pub fn new(config: Arc<Config>, workspace_dir: PathBuf) -> Self {
        Self {
            config,
            workspace_dir,
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
impl Tool for FeishuImResourceTool {
    fn name(&self) -> &str {
        "feishu_im_resource"
    }

    fn description(&self) -> &str {
        "Download a Feishu IM message resource and save it into the workspace."
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
                    "description": "Source message ID"
                },
                "file_key": {
                    "type": "string",
                    "description": "Feishu file key from the inbound message payload"
                },
                "resource_type": {
                    "type": "string",
                    "enum": ["image", "file", "audio", "video"],
                    "description": "Resource type expected by Feishu resource API"
                }
            },
            "required": ["message_id", "file_key", "resource_type"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        async {
            let account = args.get("account").and_then(serde_json::Value::as_str);
            let message_id = args
                .get("message_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'message_id' parameter"))?;
            let file_key = args
                .get("file_key")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'file_key' parameter"))?;
            let resource_type = args
                .get("resource_type")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'resource_type' parameter"))?;

            let client = self.build_client(account)?;
            let (bytes, content_type, file_name) = client
                .download_bytes(&format!(
                    "/im/v1/messages/{message_id}/resources/{file_key}?type={resource_type}"
                ))
                .await?;

            let downloads_dir = self.workspace_dir.join("feishu_downloads");
            std::fs::create_dir_all(&downloads_dir)?;
            let file_name = file_name.unwrap_or_else(|| {
                format!(
                    "{}_{}_{}",
                    message_id,
                    resource_type,
                    Uuid::new_v4().simple()
                )
            });
            let file_path = downloads_dir.join(file_name);
            std::fs::write(&file_path, &bytes)?;

            let output = json!({
                "account": client.account_name(),
                "message_id": message_id,
                "file_key": file_key,
                "resource_type": resource_type,
                "content_type": content_type,
                "saved_path": file_path,
                "size_bytes": bytes.len(),
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
    use tempfile::TempDir;
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
                }),
                ..ChannelsConfig::default()
            },
            ..Config::default()
        })
    }

    #[tokio::test]
    async fn resource_download_saves_file_into_workspace() {
        let tmp = TempDir::new().unwrap();
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
            .and(path("/im/v1/messages/om_123/resources/file_456"))
            .and(query_param("type", "file"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("content-type", "application/pdf")
                    .append_header("content-disposition", "attachment; filename=\"report.pdf\"")
                    .set_body_bytes(b"pdf-bytes".to_vec()),
            )
            .mount(&server)
            .await;

        let tool = FeishuImResourceTool::new(test_config(), tmp.path().to_path_buf())
            .with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "message_id": "om_123",
                "file_key": "file_456",
                "resource_type": "file"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("report.pdf"));
        let saved_path = tmp.path().join("feishu_downloads").join("report.pdf");
        assert_eq!(std::fs::read(saved_path).unwrap(), b"pdf-bytes");
    }
}
