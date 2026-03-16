use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

pub struct FeishuDriveFileTool {
    config: Arc<Config>,
    workspace_dir: PathBuf,
    test_api_base: Option<String>,
}

impl FeishuDriveFileTool {
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
impl Tool for FeishuDriveFileTool {
    fn name(&self) -> &str {
        "feishu_drive_file"
    }

    fn description(&self) -> &str {
        "Upload a local file to Feishu Drive or fetch metadata for an existing Drive file."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["upload_file", "get_file_meta", "download_file"],
                    "description": "Drive file operation"
                },
                "account": { "type": "string", "description": "Optional Feishu account name." },
                "file_path": { "type": "string", "description": "Absolute local file path for upload_file" },
                "destination_path": {
                    "type": "string",
                    "description": "Optional absolute local path for download_file"
                },
                "parent_type": {
                    "type": "string",
                    "description": "Feishu Drive parent type for upload_file, e.g. explorer"
                },
                "parent_node": {
                    "type": "string",
                    "description": "Feishu Drive parent node token for upload_file"
                },
                "file_name": {
                    "type": "string",
                    "description": "Optional override name for upload_file"
                },
                "file_token": { "type": "string", "description": "Drive file token for get_file_meta" }
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
                "upload_file" => {
                    let file_path = args
                        .get("file_path")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;
                    let parent_type = args
                        .get("parent_type")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'parent_type' parameter"))?;
                    let parent_node = args
                        .get("parent_node")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'parent_node' parameter"))?;
                    let file_path_buf = PathBuf::from(file_path);
                    if !file_path_buf.is_absolute() || !file_path_buf.is_file() {
                        anyhow::bail!("'file_path' must be an existing absolute file path");
                    }
                    let file_name = args
                        .get("file_name")
                        .and_then(serde_json::Value::as_str)
                        .or_else(|| file_path_buf.file_name().and_then(|value| value.to_str()))
                        .ok_or_else(|| anyhow::anyhow!("Unable to determine file name"))?;
                    let response = client
                        .upload_file(
                            "/drive/v1/files/upload_all",
                            &file_path_buf,
                            &[
                                ("file_name", file_name),
                                ("parent_type", parent_type),
                                ("parent_node", parent_node),
                            ],
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "get_file_meta" => {
                    let file_token = args
                        .get("file_token")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'file_token' parameter"))?;
                    let response = client
                        .get_json(&format!("/drive/v1/files/{file_token}"))
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "download_file" => {
                    let file_token = args
                        .get("file_token")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'file_token' parameter"))?;
                    let (bytes, content_type, file_name) = client
                        .download_bytes(&format!("/drive/v1/files/{file_token}/download"))
                        .await?;
                    let destination_path = if let Some(path) = args
                        .get("destination_path")
                        .and_then(serde_json::Value::as_str)
                    {
                        let path = PathBuf::from(path);
                        if !path.is_absolute() {
                            anyhow::bail!("'destination_path' must be an absolute path");
                        }
                        path
                    } else {
                        let downloads_dir = self.workspace_dir.join("feishu_drive_downloads");
                        std::fs::create_dir_all(&downloads_dir)?;
                        downloads_dir.join(file_name.unwrap_or_else(|| {
                            format!("{}_{}", file_token, Uuid::new_v4().simple())
                        }))
                    };
                    if let Some(parent_dir) = destination_path.parent() {
                        std::fs::create_dir_all(parent_dir)?;
                    }
                    std::fs::write(&destination_path, &bytes)?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "file_token": file_token,
                        "content_type": content_type,
                        "saved_path": destination_path,
                        "size_bytes": bytes.len(),
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
    use tempfile::TempDir;
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
    async fn get_file_meta_reads_drive_file() {
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
            .and(path("/drive/v1/files/file_123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "file": { "name": "report.pdf", "token": "file_123" } }
            })))
            .mount(&server)
            .await;

        let tool = FeishuDriveFileTool::new(test_config(), std::env::temp_dir())
            .with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "get_file_meta",
                "file_token": "file_123"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("report.pdf"));
    }

    #[tokio::test]
    async fn upload_file_posts_multipart_request() {
        let tmp = TempDir::new().unwrap();
        let local_file = tmp.path().join("report.txt");
        std::fs::write(&local_file, b"hello drive").unwrap();
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
            .and(path("/drive/v1/files/upload_all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": {
                    "file_token": "file_uploaded_1",
                    "name": "report.txt"
                }
            })))
            .mount(&server)
            .await;

        let tool = FeishuDriveFileTool::new(test_config(), tmp.path().to_path_buf())
            .with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "upload_file",
                "file_path": local_file,
                "parent_type": "explorer",
                "parent_node": "fld_parent_1"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("file_uploaded_1"));
    }

    #[tokio::test]
    async fn download_file_saves_drive_file_into_workspace() {
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
            .and(path("/drive/v1/files/file_123/download"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("content-type", "application/pdf")
                    .append_header("content-disposition", "attachment; filename=\"report.pdf\"")
                    .set_body_bytes(b"drive-file".to_vec()),
            )
            .mount(&server)
            .await;

        let tool = FeishuDriveFileTool::new(test_config(), tmp.path().to_path_buf())
            .with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "download_file",
                "file_token": "file_123"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("report.pdf"));
        let saved_path = tmp.path().join("feishu_drive_downloads").join("report.pdf");
        assert_eq!(std::fs::read(saved_path).unwrap(), b"drive-file");
    }
}
