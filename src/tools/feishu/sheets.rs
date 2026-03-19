use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct FeishuSheetsTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuSheetsTool {
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
impl Tool for FeishuSheetsTool {
    fn name(&self) -> &str {
        "feishu_sheets"
    }

    fn description(&self) -> &str {
        "Read spreadsheet metadata and read/write values in Feishu Sheets."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["get_meta", "read_range", "write_range"],
                    "description": "Sheets operation to perform"
                },
                "account": { "type": "string", "description": "Optional Feishu account name." },
                "spreadsheet_token": { "type": "string", "description": "Spreadsheet token" },
                "range": { "type": "string", "description": "A1-style range such as Sheet1!A1:B2" },
                "values": { "type": "array", "description": "2D array of cell values for write_range" }
            },
            "required": ["action", "spreadsheet_token"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        async {
            let action = args
                .get("action")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?;
            let account = args.get("account").and_then(serde_json::Value::as_str);
            let spreadsheet_token = args
                .get("spreadsheet_token")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'spreadsheet_token' parameter"))?;
            let client = self.build_client(account)?;

            let output = match action {
                "get_meta" => {
                    let response = client
                        .get_json(&format!(
                            "/sheets/v3/spreadsheets/{spreadsheet_token}/sheets/query"
                        ))
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "spreadsheet_token": spreadsheet_token,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "read_range" => {
                    let range = args
                        .get("range")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'range' parameter"))?;
                    let response = client
                        .get_json(&format!(
                            "/sheets/v2/spreadsheets/{spreadsheet_token}/values/{range}"
                        ))
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "spreadsheet_token": spreadsheet_token,
                        "range": range,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "write_range" => {
                    let range = args
                        .get("range")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'range' parameter"))?;
                    let values = args
                        .get("values")
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Missing 'values' parameter"))?;
                    let response = client
                        .put_json(
                            &format!("/sheets/v2/spreadsheets/{spreadsheet_token}/values"),
                            &json!({
                                "valueRange": {
                                    "range": range,
                                    "values": values
                                }
                            }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "spreadsheet_token": spreadsheet_token,
                        "range": range,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
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
    async fn get_meta_reads_sheet_listing() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("GET"))
            .and(path("/sheets/v3/spreadsheets/sht_123/sheets/query"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "sheets": [{ "sheet_id": "sheet_1", "title": "Sheet1" }] }
            })))
            .mount(&server)
            .await;

        let tool = FeishuSheetsTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "get_meta",
                "spreadsheet_token": "sht_123"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Sheet1"));
    }

    #[tokio::test]
    async fn read_range_fetches_cell_values() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("GET"))
            .and(path("/sheets/v2/spreadsheets/sht_123/values/Sheet1!A1:B2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "valueRange": { "range": "Sheet1!A1:B2", "values": [["A", "B"]] } }
            })))
            .mount(&server)
            .await;

        let tool = FeishuSheetsTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "read_range",
                "spreadsheet_token": "sht_123",
                "range": "Sheet1!A1:B2"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("\"A\""));
    }

    #[tokio::test]
    async fn write_range_puts_value_range_payload() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("PUT"))
            .and(path("/sheets/v2/spreadsheets/sht_123/values"))
            .and(body_json(json!({
                "valueRange": {
                    "range": "Sheet1!A1:B2",
                    "values": [["A", "B"], ["1", "2"]]
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "updatedCells": 4 }
            })))
            .mount(&server)
            .await;

        let tool = FeishuSheetsTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "write_range",
                "spreadsheet_token": "sht_123",
                "range": "Sheet1!A1:B2",
                "values": [["A", "B"], ["1", "2"]]
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("updatedCells"));
    }
}
