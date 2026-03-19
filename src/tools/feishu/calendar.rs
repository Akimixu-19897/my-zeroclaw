use super::common::{annotate_feishu_tool_error, FeishuToolClient};
use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct FeishuCalendarTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuCalendarTool {
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
impl Tool for FeishuCalendarTool {
    fn name(&self) -> &str {
        "feishu_calendar"
    }

    fn description(&self) -> &str {
        "Inspect calendars and create or update Feishu calendar events."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list_calendars", "list_events", "create_event", "update_event"],
                    "description": "Calendar operation to perform"
                },
                "account": { "type": "string", "description": "Optional Feishu account name." },
                "calendar_id": { "type": "string", "description": "Calendar ID for event-scoped operations" },
                "event_id": { "type": "string", "description": "Event ID for update_event" },
                "page_size": { "type": "integer", "description": "Optional page size for list actions" },
                "summary": { "type": "string", "description": "Event title" },
                "description": { "type": "string", "description": "Event description" },
                "start_time": { "type": "string", "description": "RFC3339 event start time" },
                "end_time": { "type": "string", "description": "RFC3339 event end time" }
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
                "list_calendars" => {
                    let response = client
                        .get_json_with_query(
                            "/calendar/v4/calendars",
                            &[("page_size", page_size.to_string())],
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "list_events" => {
                    let calendar_id = args
                        .get("calendar_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'calendar_id' parameter"))?;
                    let response = client
                        .get_json_with_query(
                            &format!("/calendar/v4/calendars/{calendar_id}/events"),
                            &[("page_size", page_size.to_string())],
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "calendar_id": calendar_id,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "create_event" => {
                    let calendar_id = args
                        .get("calendar_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'calendar_id' parameter"))?;
                    let summary = args
                        .get("summary")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'summary' parameter"))?;
                    let start_time = args
                        .get("start_time")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'start_time' parameter"))?;
                    let end_time = args
                        .get("end_time")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'end_time' parameter"))?;
                    let description = args
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    let response = client
                        .post_json(
                            &format!("/calendar/v4/calendars/{calendar_id}/events"),
                            &json!({
                                "summary": summary,
                                "description": description,
                                "start_time": { "timestamp": start_time },
                                "end_time": { "timestamp": end_time }
                            }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "calendar_id": calendar_id,
                        "data": response.get("data").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
                "update_event" => {
                    let calendar_id = args
                        .get("calendar_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'calendar_id' parameter"))?;
                    let event_id = args
                        .get("event_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'event_id' parameter"))?;
                    let summary = args
                        .get("summary")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'summary' parameter"))?;
                    let response = client
                        .patch_json(
                            &format!("/calendar/v4/calendars/{calendar_id}/events/{event_id}"),
                            &json!({ "summary": summary }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "calendar_id": calendar_id,
                        "event_id": event_id,
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
    async fn list_calendars_reads_calendar_list() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("GET"))
            .and(path("/calendar/v4/calendars"))
            .and(query_param("page_size", "20"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "items": [{ "calendar_id": "cal_1", "summary": "Team" }] }
            })))
            .mount(&server)
            .await;

        let tool = FeishuCalendarTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "list_calendars",
                "page_size": 20
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Team"));
    }

    #[tokio::test]
    async fn create_event_posts_calendar_payload() {
        let server = MockServer::start().await;
        mock_token(&server).await;
        Mock::given(method("POST"))
            .and(path("/calendar/v4/calendars/cal_1/events"))
            .and(body_json(json!({
                "summary": "Weekly Sync",
                "description": "Planning",
                "start_time": { "timestamp": "2026-03-13T10:00:00+08:00" },
                "end_time": { "timestamp": "2026-03-13T11:00:00+08:00" }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "data": { "event": { "event_id": "evt_1" } }
            })))
            .mount(&server)
            .await;

        let tool = FeishuCalendarTool::new(test_config()).with_api_base_for_test(server.uri());
        let result = tool
            .execute(json!({
                "action": "create_event",
                "calendar_id": "cal_1",
                "summary": "Weekly Sync",
                "description": "Planning",
                "start_time": "2026-03-13T10:00:00+08:00",
                "end_time": "2026-03-13T11:00:00+08:00"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("evt_1"));
    }
}
