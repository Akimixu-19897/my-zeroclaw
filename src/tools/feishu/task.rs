use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct FeishuTaskTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuTaskTool {
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
impl Tool for FeishuTaskTool {
    fn name(&self) -> &str {
        "feishu_task"
    }

    fn description(&self) -> &str {
        "List, create, inspect, and update Feishu tasks and tasklists."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list_tasklists", "list_tasks", "get_task", "create_task", "update_task"],
                    "description": "Task operation to perform"
                },
                "account": { "type": "string", "description": "Optional Feishu account name." },
                "page_size": { "type": "integer", "description": "Optional page size for list actions" },
                "task_guid": { "type": "string", "description": "Task GUID for get_task or update_task" },
                "summary": { "type": "string", "description": "Task title" },
                "description": { "type": "string", "description": "Task description" },
                "due": { "type": "string", "description": "Optional due timestamp" }
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
            let page_size = args
                .get("page_size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(50)
                .min(500);

            let output = match action {
                "list_tasklists" => {
                    let response = client
                        .get_json_with_query(
                            "/task/v2/tasklists",
                            &[("page_size", page_size.to_string())],
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "list_tasks" => {
                    let response = client
                        .get_json_with_query(
                            "/task/v2/tasks",
                            &[("page_size", page_size.to_string())],
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "get_task" => {
                    let task_guid = args
                        .get("task_guid")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'task_guid' parameter"))?;
                    let response = client
                        .get_json(&format!("/task/v2/tasks/{task_guid}"))
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "task_guid": task_guid,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "create_task" => {
                    let summary = args
                        .get("summary")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'summary' parameter"))?;
                    let description = args
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    let due = args.get("due").and_then(serde_json::Value::as_str);
                    let mut body = json!({
                        "summary": summary,
                        "description": description,
                    });
                    if let Some(due) = due.filter(|value| !value.is_empty()) {
                        body["due"] = json!(due);
                    }
                    let response = client.post_json("/task/v2/tasks", &body).await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "update_task" => {
                    let task_guid = args
                        .get("task_guid")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'task_guid' parameter"))?;
                    let summary = args
                        .get("summary")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'summary' parameter"))?;
                    let response = client
                        .patch_json(
                            &format!("/task/v2/tasks/{task_guid}"),
                            &json!({ "summary": summary }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "task_guid": task_guid,
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
    async fn list_tasklists_reads_tasklist_collection() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("GET"))
            .and(path("/task/v2/tasklists"))
            .and(query_param("page_size", "20"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "items": [{ "guid": "tl_1", "summary": "Roadmap" }] }
            })))
            .mount(&server)
            .await;

        let tool = FeishuTaskTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "list_tasklists",
                "page_size": 20
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Roadmap"));
    }

    #[tokio::test]
    async fn create_task_posts_summary_and_description() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("POST"))
            .and(path("/task/v2/tasks"))
            .and(body_json(json!({
                "summary": "Follow up",
                "description": "Call customer",
                "due": "2026-03-14T10:00:00+08:00"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "task": { "guid": "task_1" } }
            })))
            .mount(&server)
            .await;

        let tool = FeishuTaskTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "create_task",
                "summary": "Follow up",
                "description": "Call customer",
                "due": "2026-03-14T10:00:00+08:00"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("task_1"));
    }
}
