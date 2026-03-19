use super::super::*;
use crate::observability::traits::{ObserverEvent, ObserverMetric};
use crate::util::truncate_with_ellipsis;

/// Observer wrapper that forwards tool-call events to a channel sender
/// for real-time threaded notifications.
pub(crate) struct ChannelNotifyObserver {
    pub(crate) inner: Arc<dyn Observer>,
    pub(crate) tx: tokio::sync::mpsc::UnboundedSender<String>,
    pub(crate) tools_used: AtomicBool,
}

impl Observer for ChannelNotifyObserver {
    fn record_event(&self, event: &ObserverEvent) {
        if let ObserverEvent::ToolCallStart { tool, arguments } = event {
            self.tools_used.store(true, Ordering::Relaxed);
            let detail = arguments
                .as_deref()
                .filter(|args| !args.is_empty())
                .map(format_tool_call_detail)
                .unwrap_or_default();
            let _ = self.tx.send(format!("\u{1F527} `{tool}`{detail}"));
        }
        self.inner.record_event(event);
    }

    fn record_metric(&self, metric: &ObserverMetric) {
        self.inner.record_metric(metric);
    }

    fn flush(&self) {
        self.inner.flush();
    }

    fn name(&self) -> &str {
        "channel-notify"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn format_tool_call_detail(args: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(args) {
        Ok(value) => format_tool_call_detail_from_json(value),
        Err(_) => format!(": {}", truncate_with_ellipsis(args, 120)),
    }
}

fn format_tool_call_detail_from_json(value: serde_json::Value) -> String {
    let sanitized = strip_internal_tool_context(value);
    if let Some(cmd) = sanitized.get("command").and_then(|c| c.as_str()) {
        return format!(": `{}`", truncate_with_ellipsis(cmd, 200));
    }
    if let Some(query) = sanitized.get("query").and_then(|c| c.as_str()) {
        return format!(": {}", truncate_with_ellipsis(query, 200));
    }
    if let Some(path) = sanitized.get("path").and_then(|c| c.as_str()) {
        return format!(": {}", truncate_with_ellipsis(path, 200));
    }
    if let Some(url) = sanitized.get("url").and_then(|c| c.as_str()) {
        return format!(": {}", truncate_with_ellipsis(url, 200));
    }
    if sanitized
        .as_object()
        .is_some_and(|object| object.is_empty())
    {
        return String::new();
    }

    format!(": {}", truncate_with_ellipsis(&sanitized.to_string(), 120))
}

fn strip_internal_tool_context(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(object) = value.as_object_mut() {
        object.remove("__channel_context");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tool_call_detail_hides_internal_channel_context() {
        let detail = format_tool_call_detail(
            r#"{"query":"hello","__channel_context":{"current_channel_id":"user:ou_test"}}"#,
        );

        assert_eq!(detail, ": hello");
        assert!(!detail.contains("__channel_context"));
    }

    #[test]
    fn format_tool_call_detail_truncates_multibyte_text_safely() {
        let detail = format_tool_call_detail(
            &serde_json::json!({
                "message": "发送一条消息到当前飞书会话".repeat(40)
            })
            .to_string(),
        );

        assert!(detail.starts_with(": "));
        assert!(detail.ends_with("..."));
    }
}
