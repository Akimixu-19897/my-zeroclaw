use super::super::*;
use crate::observability::traits::{ObserverEvent, ObserverMetric};

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
            let detail = match arguments {
                Some(args) if !args.is_empty() => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(args) {
                        if let Some(cmd) = v.get("command").and_then(|c| c.as_str()) {
                            format!(": `{}`", if cmd.len() > 200 { &cmd[..200] } else { cmd })
                        } else if let Some(q) = v.get("query").and_then(|c| c.as_str()) {
                            format!(": {}", if q.len() > 200 { &q[..200] } else { q })
                        } else if let Some(p) = v.get("path").and_then(|c| c.as_str()) {
                            format!(": {p}")
                        } else if let Some(u) = v.get("url").and_then(|c| c.as_str()) {
                            format!(": {u}")
                        } else {
                            let s = args.to_string();
                            if s.len() > 120 {
                                format!(": {}…", &s[..120])
                            } else {
                                format!(": {s}")
                            }
                        }
                    } else {
                        let s = args.to_string();
                        if s.len() > 120 {
                            format!(": {}…", &s[..120])
                        } else {
                            format!(": {s}")
                        }
                    }
                }
                _ => String::new(),
            };
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
