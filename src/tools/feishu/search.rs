use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct FeishuSearchTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuSearchTool {
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
impl Tool for FeishuSearchTool {
    fn name(&self) -> &str {
        "feishu_search"
    }

    fn description(&self) -> &str {
        "Search Feishu Docs and Wiki content through the native Search v2 surface."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "account": { "type": "string", "description": "Optional Feishu account name." },
                "query": { "type": "string", "description": "Search query text" },
                "page_size": { "type": "integer", "description": "Optional page size" }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        async {
            let account = args.get("account").and_then(serde_json::Value::as_str);
            let query = args
                .get("query")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'query' parameter"))?;
            let page_size = args
                .get("page_size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(10)
                .min(100);
            let client = self.build_client(account)?;
            let response = client
                .post_json(
                    "/search/v2/doc_wiki/search",
                    &json!({
                        "query": query,
                        "page_size": page_size
                    }),
                )
                .await?;

            let output = json!({
                "account": client.account_name(),
                "query": query,
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
    async fn search_posts_doc_wiki_query() {
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
            .and(path("/search/v2/doc_wiki/search"))
            .and(body_json(json!({
                "query": "runbook",
                "page_size": 5
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "items": [{ "title": "Ops Runbook", "url": "https://example.com/doc" }] }
            })))
            .mount(&server)
            .await;

        let tool = FeishuSearchTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "query": "runbook",
                "page_size": 5
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Ops Runbook"));
    }
}
