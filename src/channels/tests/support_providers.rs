pub(super) struct SlowProvider {
    pub(super) delay: Duration,
}

#[async_trait::async_trait]
impl Provider for SlowProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        tokio::time::sleep(self.delay).await;
        Ok(format!("echo: {message}"))
    }
}

pub(super) struct ToolCallingProvider;

pub(super) struct ApprovalRequestProvider;

pub(super) struct ApprovalSummaryProvider;

pub(super) struct ApprovalAwareTool;

fn tool_call_payload() -> String {
    r#"<tool_call>
{"name":"mock_price","arguments":{"symbol":"BTC"}}
</tool_call>"#
        .to_string()
}

fn tool_call_payload_with_alias_tag() -> String {
    r#"<toolcall>
{"name":"mock_price","arguments":{"symbol":"BTC"}}
</toolcall>"#
        .to_string()
}

#[async_trait::async_trait]
impl Provider for ToolCallingProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok(tool_call_payload())
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        let has_tool_results = messages
            .iter()
            .any(|msg| msg.role == "user" && msg.content.contains("[Tool results]"));
        if has_tool_results {
            Ok("BTC is currently around $65,000 based on latest tool output.".to_string())
        } else {
            Ok(tool_call_payload())
        }
    }
}

#[async_trait::async_trait]
impl Provider for ApprovalRequestProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok(
                "```zeroclaw-approval\n{\"tool_name\":\"shell\",\"arguments\":{\"command\":\"touch /tmp/approval-test\"},\"reason\":\"Command requires explicit approval (approved=true): medium-risk operation\",\"preview\":\"{\\n  \\\"command\\\": \\\"touch /tmp/approval-test\\\"\\n}\"}\n```"
                    .to_string(),
            )
    }
}

#[async_trait::async_trait]
impl Provider for ApprovalSummaryProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok("approval-complete".to_string())
    }
}

#[async_trait::async_trait]
impl Tool for ApprovalAwareTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Test approval-aware shell tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "approved": { "type": "boolean" }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        if args.get("approved").and_then(serde_json::Value::as_bool) != Some(true) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(
                    "Command requires explicit approval (approved=true): medium-risk operation"
                        .to_string(),
                ),
            });
        }

        Ok(ToolResult {
            success: true,
            output: format!(
                "approved:{}",
                args.get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            ),
            error: None,
        })
    }
}

pub(super) struct ToolCallingAliasProvider;

#[async_trait::async_trait]
impl Provider for ToolCallingAliasProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok(tool_call_payload_with_alias_tag())
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        let has_tool_results = messages
            .iter()
            .any(|msg| msg.role == "user" && msg.content.contains("[Tool results]"));
        if has_tool_results {
            Ok("BTC alias-tag flow resolved to final text output.".to_string())
        } else {
            Ok(tool_call_payload_with_alias_tag())
        }
    }
}

pub(super) struct RawToolArtifactProvider;

#[async_trait::async_trait]
impl Provider for RawToolArtifactProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok("fallback".to_string())
    }

    async fn chat_with_history(
        &self,
        _messages: &[ChatMessage],
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok(r#"{"name":"mock_price","parameters":{"symbol":"BTC"}}
{"result":{"symbol":"BTC","price_usd":65000}}
BTC is currently around $65,000 based on latest tool output."#
            .to_string())
    }
}

pub(super) struct IterativeToolProvider {
    pub(super) required_tool_iterations: usize,
}

impl IterativeToolProvider {
    fn completed_tool_iterations(messages: &[ChatMessage]) -> usize {
        messages
            .iter()
            .filter(|msg| msg.role == "user" && msg.content.contains("[Tool results]"))
            .count()
    }
}

#[async_trait::async_trait]
impl Provider for IterativeToolProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok(tool_call_payload())
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        let completed_iterations = Self::completed_tool_iterations(messages);
        if completed_iterations >= self.required_tool_iterations {
            Ok(format!(
                "Completed after {completed_iterations} tool iterations."
            ))
        } else {
            Ok(tool_call_payload())
        }
    }
}

#[derive(Default)]
pub(super) struct HistoryCaptureProvider {
    pub(super) calls: std::sync::Mutex<Vec<Vec<(String, String)>>>,
}

#[async_trait::async_trait]
impl Provider for HistoryCaptureProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok("fallback".to_string())
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        let snapshot = messages
            .iter()
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect::<Vec<_>>();
        let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        calls.push(snapshot);
        Ok(format!("response-{}", calls.len()))
    }
}

pub(super) struct DelayedHistoryCaptureProvider {
    pub(super) delay: Duration,
    pub(super) calls: std::sync::Mutex<Vec<Vec<(String, String)>>>,
}

#[async_trait::async_trait]
impl Provider for DelayedHistoryCaptureProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok("fallback".to_string())
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        let snapshot = messages
            .iter()
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect::<Vec<_>>();
        let call_index = {
            let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
            calls.push(snapshot);
            calls.len()
        };
        tokio::time::sleep(self.delay).await;
        Ok(format!("response-{call_index}"))
    }
}

pub(super) struct MockPriceTool;

#[derive(Default)]
pub(super) struct ModelCaptureProvider {
    pub(super) call_count: AtomicUsize,
    pub(super) models: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Provider for ModelCaptureProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok("fallback".to_string())
    }

    async fn chat_with_history(
        &self,
        _messages: &[ChatMessage],
        model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.models
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(model.to_string());
        Ok("ok".to_string())
    }
}

#[async_trait::async_trait]
impl Tool for MockPriceTool {
    fn name(&self) -> &str {
        "mock_price"
    }

    fn description(&self) -> &str {
        "Return a mocked BTC price"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string" }
            },
            "required": ["symbol"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let symbol = args.get("symbol").and_then(serde_json::Value::as_str);
        if symbol != Some("BTC") {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("unexpected symbol".to_string()),
            });
        }

        Ok(ToolResult {
            success: true,
            output: r#"{"symbol":"BTC","price_usd":65000}"#.to_string(),
            error: None,
        })
    }
}
