use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct FeishuWikiSpaceTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuWikiSpaceTool {
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
impl Tool for FeishuWikiSpaceTool {
    fn name(&self) -> &str {
        "feishu_wiki_space"
    }

    fn description(&self) -> &str {
        "List Feishu Wiki spaces or inspect a node within a specific space."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list_spaces", "get_node", "list_nodes"],
                    "description": "Wiki space operation"
                },
                "account": { "type": "string", "description": "Optional Feishu account name." },
                "space_id": { "type": "string", "description": "Wiki space ID for get_node" },
                "node_token": { "type": "string", "description": "Node token for get_node" },
                "parent_node_token": {
                    "type": "string",
                    "description": "Optional parent node token for list_nodes"
                },
                "page_size": { "type": "integer", "description": "Optional page size for list_spaces or list_nodes" }
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
                "list_spaces" => {
                    let page_size = args
                        .get("page_size")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(50)
                        .min(200);
                    let response = client
                        .get_json_with_query(
                            "/wiki/v2/spaces",
                            &[("page_size", page_size.to_string())],
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "get_node" => {
                    let space_id = args
                        .get("space_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'space_id' parameter"))?;
                    let node_token = args
                        .get("node_token")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'node_token' parameter"))?;
                    let response = client
                        .get_json(&format!("/wiki/v2/spaces/{space_id}/nodes/{node_token}"))
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "list_nodes" => {
                    let space_id = args
                        .get("space_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'space_id' parameter"))?;
                    let page_size = args
                        .get("page_size")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(50)
                        .min(200);
                    let parent_node_token = args
                        .get("parent_node_token")
                        .and_then(serde_json::Value::as_str);
                    let mut query = vec![("page_size", page_size.to_string())];
                    if let Some(parent_node_token) =
                        parent_node_token.filter(|value| !value.is_empty())
                    {
                        query.push(("parent_node_token", parent_node_token.to_string()));
                    }
                    let response = client
                        .get_json_with_query(&format!("/wiki/v2/spaces/{space_id}/nodes"), &query)
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
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
    async fn list_spaces_reads_wiki_space_list() {
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
            .and(path("/wiki/v2/spaces"))
            .and(query_param("page_size", "20"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "items": [{ "space_id": "space_1", "name": "Engineering Wiki" }] }
            })))
            .mount(&server)
            .await;

        let tool = FeishuWikiSpaceTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "list_spaces",
                "page_size": 20
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Engineering Wiki"));
    }

    #[tokio::test]
    async fn get_node_reads_single_wiki_node() {
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
            .and(path("/wiki/v2/spaces/space_1/nodes/node_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "node": { "node_token": "node_1", "title": "Runbook" } }
            })))
            .mount(&server)
            .await;

        let tool = FeishuWikiSpaceTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "get_node",
                "space_id": "space_1",
                "node_token": "node_1"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Runbook"));
    }

    #[tokio::test]
    async fn list_nodes_reads_child_nodes_in_space() {
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
            .and(path("/wiki/v2/spaces/space_1/nodes"))
            .and(query_param("page_size", "20"))
            .and(query_param("parent_node_token", "root_node"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "items": [{ "node_token": "node_2", "title": "Spec" }] }
            })))
            .mount(&server)
            .await;

        let tool = FeishuWikiSpaceTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "list_nodes",
                "space_id": "space_1",
                "parent_node_token": "root_node",
                "page_size": 20
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Spec"));
    }
}
