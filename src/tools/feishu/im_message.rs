use super::common::{annotate_feishu_tool_error, FeishuToolClient};
#[path = "im_message_send.rs"]
mod send;

use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

const CHANNEL_CONTEXT_ARG_KEY: &str = "__channel_context";

pub struct FeishuImMessageTool {
    config: Arc<Config>,
    test_api_base: Option<String>,
}

impl FeishuImMessageTool {
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

    fn infer_account<'a>(&self, args: &'a serde_json::Value) -> Option<&'a str> {
        args.get("account")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                args.get(CHANNEL_CONTEXT_ARG_KEY)
                    .and_then(|context| context.get("current_channel_name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| {
                        *value == "feishu"
                            || value.starts_with("feishu:")
                            || *value == "lark"
                            || value.starts_with("lark:")
                    })
            })
    }
}

#[async_trait]
impl Tool for FeishuImMessageTool {
    fn name(&self) -> &str {
        "feishu_im_message"
    }

    fn description(&self) -> &str {
        "Send, reply to, update, or delete Feishu IM messages with configured bot accounts. \
         For current-chat delivery, prefer one direct send with `media`/`path` or `text`; do not \
         use search/list/glob tools first when the exact file path or URL is already known. If the \
         user only asked for any local test image/file, prefer `local_pick=image` or `local_pick=file` \
         to reuse the current chat cache/workspace in one call. If you already created or know an \
         exact local media path, send it with this tool and do not paste the path back as plain text. \
         Audio/video files are supported through the same `media`/`path` input. For Feishu voice-style \
         delivery, audio should be generated and saved as `.ogg` or `.opus` before sending; do not use \
         `mp3` or `m4a` when the intent is a voice message. When this tool successfully delivers into \
         the current Feishu chat, finish the turn with exactly `NO_REPLY` to avoid duplicate follow-up \
         messages or repeated sends."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["send", "send_text", "reply_text", "update_text", "delete_message"],
                    "description": "IM message operation to perform"
                },
                "account": {
                    "type": "string",
                    "description": "Optional Feishu account name. Use `feishu` for the default account or `primary` / `feishu:primary` for a named account."
                },
                "receive_id": {
                    "type": "string",
                    "description": "Target Feishu chat/user ID for send or send_text"
                },
                "message_id": {
                    "type": "string",
                    "description": "Message ID for reply_text, update_text, or delete_message"
                },
                "text": {
                    "type": "string",
                    "description": "Text content to send, reply with, or update into the target message"
                },
                "message": {
                    "type": "string",
                    "description": "Alias of text for unified send"
                },
                "to": {
                    "type": "string",
                    "description": "Alias of receive_id for unified send"
                },
                "media": {
                    "type": "string",
                    "description": "Media URL or exact local absolute path for unified send. Prefer passing the known path directly instead of rediscovering it with search/list/glob tools. For voice-style Feishu audio, prefer `.ogg` or `.opus` files."
                },
                "local_pick": {
                    "type": "string",
                    "enum": ["image", "file"],
                    "description": "When no exact path is known, pick one suitable local attachment automatically. Search current channel inbound cache first, then fall back to the workspace root."
                },
                "path": {
                    "type": "string",
                    "description": "Alias of media for unified send"
                },
                "filePath": {
                    "type": "string",
                    "description": "Alias of media for unified send"
                },
                "url": {
                    "type": "string",
                    "description": "Alias of media for unified send"
                },
                "fileName": {
                    "type": "string",
                    "description": "Optional upload file name override"
                },
                "name": {
                    "type": "string",
                    "description": "Alias of fileName for unified send"
                },
                "replyTo": {
                    "type": "string",
                    "description": "Optional message ID to reply to in unified send"
                },
                "card": {
                    "description": "Interactive card object or JSON string for unified send"
                },
            },
            "required": ["action"],
            "examples": [
                {
                    "action": "send",
                    "media": "/absolute/path/to/image.png"
                },
                {
                    "action": "send",
                    "message": "这里是报告",
                    "media": "/absolute/path/to/report.pdf"
                },
                {
                    "action": "send",
                    "media": "/absolute/path/to/voice.ogg"
                }
            ]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        async {
            let action = args
                .get("action")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?;
            let account = self.infer_account(&args);
            let client = self.build_client(account)?;

            let output = match action {
                "send" | "send_text" => {
                    send::deliver_feishu_send_action(
                        &client,
                        &args,
                        action,
                        self.config.workspace_dir.as_path(),
                    )
                    .await?
                }
                "reply_text" => {
                    let message_id = args
                        .get("message_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'message_id' parameter"))?;
                    let text = args
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'text' parameter"))?;
                    let response = client
                        .post_json(
                            &format!("/im/v1/messages/{message_id}/reply"),
                            &json!({
                                "msg_type": "post",
                                "content": crate::channels::lark::message_builders::build_lark_post_content(text).to_string(),
                            }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "message_id": response.pointer("/data/message_id"),
                    })
                }
                "update_text" => {
                    let message_id = args
                        .get("message_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'message_id' parameter"))?;
                    let text = args
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'text' parameter"))?;
                    client
                        .patch_json(
                            &format!("/im/v1/messages/{message_id}"),
                            &json!({
                                "msg_type": "post",
                                "content": crate::channels::lark::message_builders::build_lark_post_content(text).to_string(),
                            }),
                        )
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "message_id": message_id,
                    })
                }
                "delete_message" => {
                    let message_id = args
                        .get("message_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("Missing 'message_id' parameter"))?;
                    client
                        .delete(&format!("/im/v1/messages/{message_id}"))
                        .await?;
                    json!({
                        "account": client.account_name(),
                        "action": action,
                        "message_id": message_id,
                        "deleted": true,
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
#[path = "im_message_tests.rs"]
mod tests;
