use super::*;
use crate::config::schema::LarkReceiveMode;
use crate::config::{ChannelsConfig, FeishuConfig};
use tempfile::TempDir;
use wiremock::matchers::{body_json, body_string_contains, method, path, query_param};
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

fn test_config_with_workspace(workspace_dir: &std::path::Path) -> Arc<Config> {
    let mut config = (*test_config()).clone();
    config.workspace_dir = workspace_dir.to_path_buf();
    Arc::new(config)
}

fn test_config_with_workspace_and_named_feishu_account(workspace_dir: &std::path::Path) -> Arc<Config> {
    let mut config = (*test_config_with_named_feishu_account()).clone();
    config.workspace_dir = workspace_dir.to_path_buf();
    Arc::new(config)
}

fn test_config_with_feishu_media_settings(
    media_max_mb: Option<usize>,
    media_local_roots: Vec<std::path::PathBuf>,
) -> Arc<Config> {
    let mut config = (*test_config()).clone();
    if let Some(feishu) = config.channels_config.feishu.as_mut() {
        feishu.media_max_mb = media_max_mb;
        feishu.media_local_roots = media_local_roots;
    }
    Arc::new(config)
}

fn test_config_with_named_feishu_account() -> Arc<Config> {
    let mut config = (*test_config()).clone();
    config.channels_config.feishu_accounts.insert(
        "primary".to_string(),
        FeishuConfig {
            app_id: "cli_named_primary".to_string(),
            app_secret: "named-secret".to_string(),
            enabled: None,
            encrypt_key: None,
            verification_token: None,
            allowed_users: vec!["*".to_string()],
            receive_mode: LarkReceiveMode::default(),
            port: None,
            media_max_mb: None,
            media_local_roots: Vec::new(),
        },
    );
    Arc::new(config)
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
async fn send_text_posts_message_with_chat_receive_id() {
    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_1",
            "msg_type": "post",
            "content": "{\"zh_cn\":{\"content\":[[{\"tag\":\"md\",\"text\":\"hello\"}]]}}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_123" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send_text",
            "receive_id": "oc_chat_1",
            "text": "hello"
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_123\""));
    assert!(!result.output.contains("\"assistant_reply\": \"NO_REPLY\""));
}

#[test]
fn tool_description_and_schema_encourage_direct_media_send_for_known_paths() {
    let tool = FeishuImMessageTool::new(test_config());
    let description = tool.description();
    assert!(description.contains("do not use search/list/glob tools first"));
    assert!(description.contains("local_pick=image"));
    assert!(description.contains("`.ogg` or `.opus`"));
    assert!(description.contains("exactly `NO_REPLY`"));

    let schema = tool.parameters_schema();
    let media_description = schema
        .pointer("/properties/media/description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    assert!(media_description.contains("exact local absolute path"));
    assert!(media_description.contains("search/list/glob tools"));
    assert!(media_description.contains("`.ogg` or `.opus`"));
    assert_eq!(
        schema.pointer("/properties/local_pick/description").and_then(serde_json::Value::as_str),
        Some("When no exact path is known, pick one suitable local attachment automatically. Search current channel inbound cache first, then fall back to the workspace root.")
    );
}

#[tokio::test]
async fn send_text_accepts_open_id_prefixed_target() {
    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "open_id"))
        .and(body_json(json!({
            "receive_id": "ou_user_1",
            "msg_type": "post",
            "content": "{\"zh_cn\":{\"content\":[[{\"tag\":\"md\",\"text\":\"hello\"}]]}}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_open_123" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send_text",
            "receive_id": "open_id:ou_user_1",
            "text": "hello"
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_open_123\""));
}

#[tokio::test]
async fn send_text_accepts_official_to_and_message_aliases() {
    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_1",
            "msg_type": "post",
            "content": "{\"zh_cn\":{\"content\":[[{\"tag\":\"md\",\"text\":\"hello alias\"}]]}}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_alias_123" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "message": "hello alias"
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_alias_123\""));
}

#[tokio::test]
async fn send_text_inherits_thread_reply_context_when_target_is_omitted() {
    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages/om_parent_1/reply"))
        .and(body_json(json!({
            "msg_type": "post",
            "content": "{\"zh_cn\":{\"content\":[[{\"tag\":\"md\",\"text\":\"hello thread\"}]]}}",
            "reply_in_thread": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_reply_123" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "text": "hello thread",
            "__channel_context": {
                "current_channel_id": "chat:oc_chat_1",
                "current_message_id": "om_parent_1",
                "current_thread_ts": "omt_thread_1"
            }
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_reply_123\""));
    assert!(result.output.contains("\"assistant_reply\": \"NO_REPLY\""));
    assert!(result.output.contains("\"delivery_scope\": \"current_chat\""));
}

#[tokio::test]
async fn send_text_inherits_thread_reply_context_for_same_chat_alias_target() {
    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages/om_parent_same/reply"))
        .and(body_json(json!({
            "msg_type": "post",
            "content": "{\"zh_cn\":{\"content\":[[{\"tag\":\"md\",\"text\":\"hello same chat\"}]]}}",
            "reply_in_thread": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_same_123" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "text": "hello same chat",
            "__channel_context": {
                "current_channel_id": "chat:oc_chat_1",
                "current_message_id": "om_parent_same",
                "current_thread_ts": "omt_thread_1"
            }
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_same_123\""));
    assert!(result.output.contains("\"assistant_reply\": \"NO_REPLY\""));
    assert!(result.output.contains("\"delivery_scope\": \"current_chat\""));
}

#[tokio::test]
async fn send_text_reply_to_alias_overrides_inherited_current_message_id() {
    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages/om_override/reply"))
        .and(body_json(json!({
            "msg_type": "post",
            "content": "{\"zh_cn\":{\"content\":[[{\"tag\":\"md\",\"text\":\"override\"}]]}}",
            "reply_in_thread": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_override_result" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "text": "override",
            "replyTo": "om_override",
            "__channel_context": {
                "current_channel_id": "chat:oc_chat_1",
                "current_message_id": "om_parent_same",
                "current_thread_ts": "omt_thread_1"
            }
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result
        .output
        .contains("\"message_id\": \"om_override_result\""));
}

#[tokio::test]
async fn send_text_does_not_inherit_thread_reply_context_for_different_target() {
    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_2",
            "msg_type": "post",
            "content": "{\"zh_cn\":{\"content\":[[{\"tag\":\"md\",\"text\":\"other chat\"}]]}}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_other_chat" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_2",
            "text": "other chat",
            "__channel_context": {
                "current_channel_id": "chat:oc_chat_1",
                "current_message_id": "om_parent_same",
                "current_thread_ts": "omt_thread_1"
            }
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_other_chat\""));
}

#[tokio::test]
async fn send_text_inherits_named_feishu_account_from_current_channel_name() {
    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages/om_named_parent/reply"))
        .and(body_json(json!({
            "msg_type": "post",
            "content": "{\"zh_cn\":{\"content\":[[{\"tag\":\"md\",\"text\":\"named reply\"}]]}}",
            "reply_in_thread": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_named_reply" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config_with_named_feishu_account())
        .with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "text": "named reply",
            "__channel_context": {
                "current_channel_name": "feishu:primary",
                "current_channel_id": "chat:oc_chat_1",
                "current_message_id": "om_named_parent",
                "current_thread_ts": "omt_named_1"
            }
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_named_reply\""));
    assert!(result.output.contains("\"account\": \"feishu:primary\""));
}

#[tokio::test]
async fn send_card_accepts_json_string_alias() {
    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_card_123" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "card": "{\"schema\":\"2.0\",\"body\":{\"elements\":[{\"tag\":\"markdown\",\"content\":\"hello card\"}]}}"
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_card_123\""));
}

#[tokio::test]
async fn send_media_accepts_path_and_name_aliases() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("report.pdf");
    std::fs::write(&file_path, b"pdf-bytes").unwrap();

    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/files"))
        .and(body_string_contains("weather-report.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "file_key": "file_v3_123" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_1",
            "msg_type": "file",
            "content": "{\"file_key\":\"file_v3_123\"}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_file_123", "chat_id": "oc_chat_1" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "path": file_path.to_string_lossy().to_string(),
            "name": "weather-report.pdf"
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_file_123\""));
}

#[tokio::test]
async fn send_media_accepts_wrapped_remote_url_input() {
    let server = MockServer::start().await;
    let media_url = format!("{}/demo/image.png?sig=1#preview", server.uri());
    mock_token(&server).await;
    Mock::given(method("GET"))
        .and(path("/demo/image.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(b"\x89PNG\r\n\x1a\nfake".to_vec()),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/images"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "image_key": "img_v3_wrapped_1" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_1",
            "msg_type": "image",
            "content": "{\"image_key\":\"img_v3_wrapped_1\"}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_wrapped_media_1", "chat_id": "oc_chat_1" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "media": format!(" <\"{}\"> ", media_url)
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_wrapped_media_1\""));
}

#[tokio::test]
async fn send_media_accepts_explicit_absolute_path_outside_default_roots() {
    let repo_root = std::env::current_dir().unwrap();
    let outside = tempfile::tempdir_in(repo_root).unwrap();
    let file_path = outside.path().join("report.pdf");
    std::fs::write(&file_path, b"%PDF-1.4 fake").unwrap();

    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "file_key": "file_v3_explicit_path_1" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_1",
            "msg_type": "file",
            "content": "{\"file_key\":\"file_v3_explicit_path_1\"}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_explicit_path_1", "chat_id": "oc_chat_1" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "path": file_path.to_string_lossy().to_string(),
            "name": "report.pdf"
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_explicit_path_1\""));
}

#[tokio::test]
async fn send_media_honors_configured_local_roots_for_explicit_path() {
    let allowed_root = TempDir::new().unwrap();
    let outside_root = TempDir::new().unwrap();
    let file_path = outside_root.path().join("report.pdf");
    std::fs::write(&file_path, b"%PDF-1.4 fake").unwrap();

    let tool = FeishuImMessageTool::new(test_config_with_feishu_media_settings(
        None,
        vec![allowed_root.path().to_path_buf()],
    ));
    let err = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "path": file_path.to_string_lossy().to_string(),
        }))
        .await
        .expect_err("configured media_local_roots should restrict explicit local paths");

    assert!(err
        .to_string()
        .contains("Local media path is not under an allowed directory"));
}

#[tokio::test]
async fn send_media_accepts_file_url_input() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("report.pdf");
    std::fs::write(&file_path, b"%PDF-1.4 fake").unwrap();
    let file_url = reqwest::Url::from_file_path(&file_path)
        .expect("temp file should convert into file:// URL")
        .to_string();

    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "file_key": "file_v3_file_url_1" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_1",
            "msg_type": "file",
            "content": "{\"file_key\":\"file_v3_file_url_1\"}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_file_url_1", "chat_id": "oc_chat_1" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "media": file_url
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_file_url_1\""));
}

#[tokio::test]
async fn send_media_accepts_media_prefixed_remote_url_input() {
    let server = MockServer::start().await;
    let media_url = format!("{}/demo/image.png?sig=1", server.uri());
    mock_token(&server).await;
    Mock::given(method("GET"))
        .and(path("/demo/image.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(b"\x89PNG\r\n\x1a\nfake".to_vec()),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/images"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "image_key": "img_v3_media_prefix_1" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_1",
            "msg_type": "image",
            "content": "{\"image_key\":\"img_v3_media_prefix_1\"}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_media_prefix_1", "chat_id": "oc_chat_1" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "media": format!(" MEDIA : {} ", media_url)
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_media_prefix_1\""));
}

#[tokio::test]
async fn send_media_rejects_remote_content_length_over_limit() {
    let server = MockServer::start().await;
    let media_url = format!("{}/demo/huge.bin", server.uri());
    let oversized = vec![b'x'; crate::channels::lark::media::LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES + 1];
    mock_token(&server).await;
    Mock::given(method("GET"))
        .and(path("/demo/huge.bin"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/octet-stream")
                .set_body_bytes(oversized),
        )
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let err = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "media": media_url
        }))
        .await
        .expect_err("oversized remote media should be rejected");

    assert!(err.to_string().contains("remote media exceeds size limit"));
}

#[tokio::test]
async fn send_media_uses_configured_media_max_mb_limit() {
    let server = MockServer::start().await;
    let media_url = format!("{}/demo/too-large.bin", server.uri());
    let oversized = vec![b'x'; 1024 * 1024 + 1];
    mock_token(&server).await;
    Mock::given(method("GET"))
        .and(path("/demo/too-large.bin"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/octet-stream")
                .set_body_bytes(oversized),
        )
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config_with_feishu_media_settings(Some(1), Vec::new()))
        .with_api_base_for_test(server.uri());
    let err = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "media": media_url
        }))
        .await
        .expect_err("configured media_max_mb should cap remote media loading");

    assert!(err.to_string().contains("remote media exceeds size limit"));
}

#[tokio::test]
async fn send_media_rejects_relative_local_path() {
    let tool = FeishuImMessageTool::new(test_config());
    let err = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "path": "./relative/report.pdf"
        }))
        .await
        .expect_err("relative paths should be rejected");

    assert!(err
        .to_string()
        .contains("expected an absolute local path, file:// URL, or http(s) URL"));
}

#[tokio::test]
async fn send_media_rejects_unsupported_url_scheme() {
    let tool = FeishuImMessageTool::new(test_config());
    let err = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "url": "ftp://example.com/demo.png"
        }))
        .await
        .expect_err("unsupported schemes should be rejected");

    assert!(err
        .to_string()
        .contains("Unsupported media URL scheme: ftp"));
}

#[tokio::test]
async fn send_media_local_pick_prefers_current_channel_inbound_cache() {
    let workspace = TempDir::new().unwrap();
    let inbound_dir = workspace
        .path()
        .join("channels/feishu:primary/primary/inbound");
    std::fs::create_dir_all(&inbound_dir).unwrap();
    let preferred = inbound_dir.join("picked.png");
    std::fs::write(&preferred, b"\x89PNG\r\n\x1a\nfake").unwrap();

    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/images"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "image_key": "img_v3_pick_1" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_1",
            "msg_type": "image",
            "content": "{\"image_key\":\"img_v3_pick_1\"}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_pick_1" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config_with_workspace_and_named_feishu_account(
        workspace.path(),
    ))
        .with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "local_pick": "image",
            "__channel_context": {
                "current_channel_name": "feishu:primary",
                "current_channel_id": "chat:oc_chat_1"
            }
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_pick_1\""));
}

#[tokio::test]
async fn send_media_local_pick_falls_back_to_workspace_root() {
    let workspace = TempDir::new().unwrap();
    let report = workspace.path().join("report.pdf");
    std::fs::write(&report, b"%PDF-1.4 fake").unwrap();

    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "file_key": "file_v3_pick_1" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_1",
            "msg_type": "file",
            "content": "{\"file_key\":\"file_v3_pick_1\"}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_pick_file_1" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config_with_workspace_and_named_feishu_account(
        workspace.path(),
    ))
        .with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "local_pick": "file",
            "__channel_context": {
                "current_channel_name": "feishu:primary",
                "current_channel_id": "chat:oc_chat_1"
            }
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_pick_file_1\""));
}

#[cfg(unix)]
#[tokio::test]
async fn send_media_local_pick_rejects_symlink_escaping_current_inbound_root() {
    use std::os::unix::fs::symlink;

    let workspace = TempDir::new().unwrap();
    let inbound_dir = workspace
        .path()
        .join("channels/feishu:primary/primary/inbound");
    std::fs::create_dir_all(&inbound_dir).unwrap();

    let outside_root = TempDir::new().unwrap();
    let outside_file = outside_root.path().join("escaped.png");
    std::fs::write(&outside_file, b"\x89PNG\r\n\x1a\nfake").unwrap();
    let inbound_link = inbound_dir.join("picked.png");
    symlink(&outside_file, &inbound_link).unwrap();

    let tool = FeishuImMessageTool::new(test_config_with_workspace_and_named_feishu_account(
        workspace.path(),
    ));
    let err = tool
        .execute(json!({
            "action": "send",
            "local_pick": "image",
            "__channel_context": {
                "current_channel_name": "feishu:primary",
                "current_channel_id": "chat:oc_chat_1"
            }
        }))
        .await
        .expect_err("symlink escaping inbound root should be rejected");

    assert!(err
        .to_string()
        .contains("Local media path is not under an allowed directory"));
}

#[tokio::test]
async fn send_media_upload_failure_returns_error() {
    let server = MockServer::start().await;
    let media_url = format!("{}/demo/fail.pdf", server.uri());
    mock_token(&server).await;
    Mock::given(method("GET"))
        .and(path("/demo/fail.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"pdf-bytes".to_vec()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 999,
            "msg": "upload failed"
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let err = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "url": media_url
        }))
        .await
        .unwrap_err();

    assert!(err.to_string().contains("upload failed"));
}

#[tokio::test]
async fn send_media_routes_ogg_to_audio_message_with_duration() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("voice.ogg");
    let mut bytes = vec![0_u8; 64];
    bytes[0..4].copy_from_slice(b"OggS");
    bytes[6..14].copy_from_slice(&48_000_u64.to_le_bytes());
    std::fs::write(&file_path, bytes).unwrap();

    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "file_key": "file_v3_audio_1" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_1",
            "msg_type": "audio",
            "content": "{\"file_key\":\"file_v3_audio_1\"}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_audio_1", "chat_id": "oc_chat_1" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "path": file_path.to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_audio_1\""));

    let requests = server.received_requests().await.unwrap();
    let upload_request = requests
        .iter()
        .find(|request| request.url.path() == "/im/v1/files")
        .expect("upload request should be recorded");
    let upload_body = String::from_utf8_lossy(&upload_request.body);
    assert!(upload_body.contains("name=\"file_type\""));
    assert!(upload_body.contains("opus"));
    assert!(upload_body.contains("name=\"duration\""));
    assert!(upload_body.contains("1000"));
}

#[tokio::test]
async fn send_media_routes_mp4_to_media_message_with_duration() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("clip.mp4");
    std::fs::write(&file_path, build_test_mp4_with_duration(1000, 3000)).unwrap();

    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/im/v1/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "file_key": "file_v3_video_1" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(body_json(json!({
            "receive_id": "oc_chat_1",
            "msg_type": "media",
            "content": "{\"file_key\":\"file_v3_video_1\"}"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": { "message_id": "om_video_1", "chat_id": "oc_chat_1" }
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "send",
            "to": "chat:oc_chat_1",
            "path": file_path.to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"message_id\": \"om_video_1\""));

    let requests = server.received_requests().await.unwrap();
    let upload_request = requests
        .iter()
        .find(|request| request.url.path() == "/im/v1/files")
        .expect("upload request should be recorded");
    let upload_body = String::from_utf8_lossy(&upload_request.body);
    assert!(upload_body.contains("name=\"file_type\""));
    assert!(upload_body.contains("mp4"));
    assert!(upload_body.contains("name=\"duration\""));
    assert!(upload_body.contains("3000"));
}

fn build_test_mp4_with_duration(timescale: u32, duration: u32) -> Vec<u8> {
    let mut mvhd = Vec::new();
    mvhd.push(0);
    mvhd.extend_from_slice(&[0, 0, 0]);
    mvhd.extend_from_slice(&[0; 8]);
    mvhd.extend_from_slice(&timescale.to_be_bytes());
    mvhd.extend_from_slice(&duration.to_be_bytes());

    let mut mvhd_box = Vec::new();
    mvhd_box.extend_from_slice(&(8_u32 + u32::try_from(mvhd.len()).unwrap()).to_be_bytes());
    mvhd_box.extend_from_slice(b"mvhd");
    mvhd_box.extend_from_slice(&mvhd);

    let mut moov_box = Vec::new();
    moov_box.extend_from_slice(&(8_u32 + u32::try_from(mvhd_box.len()).unwrap()).to_be_bytes());
    moov_box.extend_from_slice(b"moov");
    moov_box.extend_from_slice(&mvhd_box);
    moov_box
}

#[tokio::test]
async fn delete_message_calls_delete_endpoint() {
    let server = MockServer::start().await;
    mock_token(&server).await;
    Mock::given(method("DELETE"))
        .and(path("/im/v1/messages/om_123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": {}
        })))
        .mount(&server)
        .await;

    let tool = FeishuImMessageTool::new(test_config()).with_api_base_for_test(server.uri());
    let result = tool
        .execute(json!({
            "action": "delete_message",
            "message_id": "om_123"
        }))
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.contains("\"deleted\": true"));
}
