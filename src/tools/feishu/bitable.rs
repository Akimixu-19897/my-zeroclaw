use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct FeishuBitableTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuBitableTool {
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
impl Tool for FeishuBitableTool {
    fn name(&self) -> &str {
        "feishu_bitable"
    }

    fn description(&self) -> &str {
        "Inspect and mutate Feishu Bitable apps, tables, fields, records, and views."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "get_app",
                        "list_tables",
                        "list_fields",
                        "list_views",
                        "list_records",
                        "create_record",
                        "update_record"
                    ],
                    "description": "Bitable operation to perform"
                },
                "account": { "type": "string", "description": "Optional Feishu account name." },
                "app_token": { "type": "string", "description": "Bitable app token" },
                "table_id": { "type": "string", "description": "Table ID for table-scoped actions" },
                "view_id": { "type": "string", "description": "Optional view ID for list_records" },
                "page_size": { "type": "integer", "description": "Optional page size for list actions" },
                "record_id": { "type": "string", "description": "Record ID for update_record" },
                "fields": { "type": "object", "description": "Record fields payload for create_record or update_record" }
            },
            "required": ["action", "app_token"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        async {
            let action = args
                .get("action")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?;
            let account = args.get("account").and_then(serde_json::Value::as_str);
            let app_token = args
                .get("app_token")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'app_token' parameter"))?;
            let client = self.build_client(account)?;
            let page_size = args
                .get("page_size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(50)
                .min(500);

            let output = match action {
                "get_app" => {
                    let response = client
                        .get_json(&format!("/bitable/v1/apps/{app_token}"))
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "app_token": app_token,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "list_tables" => {
                    let response = client
                        .get_json_with_query(
                            &format!("/bitable/v1/apps/{app_token}/tables"),
                            &[("page_size", page_size.to_string())],
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "app_token": app_token,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "list_fields" => {
                    let table_id = args
                        .get("table_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'table_id' parameter"))?;
                    let response = client
                        .get_json_with_query(
                            &format!("/bitable/v1/apps/{app_token}/tables/{table_id}/fields"),
                            &[("page_size", page_size.to_string())],
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "app_token": app_token,
                        "table_id": table_id,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "list_views" => {
                    let table_id = args
                        .get("table_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'table_id' parameter"))?;
                    let response = client
                        .get_json_with_query(
                            &format!("/bitable/v1/apps/{app_token}/tables/{table_id}/views"),
                            &[("page_size", page_size.to_string())],
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "app_token": app_token,
                        "table_id": table_id,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "list_records" => {
                    let table_id = args
                        .get("table_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'table_id' parameter"))?;
                    let view_id = args.get("view_id").and_then(serde_json::Value::as_str);
                    let mut query = vec![("page_size", page_size.to_string())];
                    if let Some(view_id) = view_id.filter(|value| !value.is_empty()) {
                        query.push(("view_id", view_id.to_string()));
                    }
                    let response = client
                        .get_json_with_query(
                            &format!("/bitable/v1/apps/{app_token}/tables/{table_id}/records"),
                            &query,
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "app_token": app_token,
                        "table_id": table_id,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "create_record" => {
                    let table_id = args
                        .get("table_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'table_id' parameter"))?;
                    let fields = args
                        .get("fields")
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Missing 'fields' parameter"))?;
                    let response = client
                        .post_json(
                            &format!("/bitable/v1/apps/{app_token}/tables/{table_id}/records"),
                            &json!({ "fields": fields }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "app_token": app_token,
                        "table_id": table_id,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "update_record" => {
                    let table_id = args
                        .get("table_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'table_id' parameter"))?;
                    let record_id = args
                        .get("record_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'record_id' parameter"))?;
                    let fields = args
                        .get("fields")
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Missing 'fields' parameter"))?;
                    let response = client
                        .patch_json(
                            &format!(
                            "/bitable/v1/apps/{app_token}/tables/{table_id}/records/{record_id}"
                        ),
                            &json!({ "fields": fields }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "app_token": app_token,
                        "table_id": table_id,
                        "record_id": record_id,
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
    async fn list_tables_reads_bitable_tables() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("GET"))
            .and(path("/bitable/v1/apps/app_123/tables"))
            .and(query_param("page_size", "20"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "items": [{ "table_id": "tbl_1", "name": "Tasks" }] }
            })))
            .mount(&server)
            .await;

        let tool = FeishuBitableTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "list_tables",
                "app_token": "app_123",
                "page_size": 20
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Tasks"));
    }

    #[tokio::test]
    async fn list_records_reads_bitable_records() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("GET"))
            .and(path("/bitable/v1/apps/app_123/tables/tbl_1/records"))
            .and(query_param("page_size", "50"))
            .and(query_param("view_id", "vew_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "items": [{ "record_id": "rec_1", "fields": { "Name": "Alpha" } }] }
            })))
            .mount(&server)
            .await;

        let tool = FeishuBitableTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "list_records",
                "app_token": "app_123",
                "table_id": "tbl_1",
                "view_id": "vew_1"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Alpha"));
    }

    #[tokio::test]
    async fn create_record_posts_fields_payload() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("POST"))
            .and(path("/bitable/v1/apps/app_123/tables/tbl_1/records"))
            .and(body_json(json!({
                "fields": { "Name": "Alpha", "Done": true }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "record": { "record_id": "rec_1" } }
            })))
            .mount(&server)
            .await;

        let tool = FeishuBitableTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "create_record",
                "app_token": "app_123",
                "table_id": "tbl_1",
                "fields": { "Name": "Alpha", "Done": true }
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("rec_1"));
    }
}
