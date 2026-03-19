use super::*;
use crate::channels::lark::cards::parse_lark_card_message;
use crate::channels::lark::helpers::{detect_lark_ack_locale, map_locale_tag};
use crate::channels::lark::inbound::{LarkInboundResource, LarkInboundResourceKind};
use crate::channels::lark::media::{
    store_inbound_resource, store_inbound_resource_with_limit,
    LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES,
};
use crate::channels::lark::message_builders::build_lark_post_content;
use crate::channels::lark::protocol::{LARK_DEFAULT_TOKEN_TTL, LARK_INVALID_ACCESS_TOKEN_CODE};
use crate::channels::traits::SendMessage;
use tempfile::TempDir;
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn with_bot_open_id(ch: LarkChannel, bot_open_id: &str) -> LarkChannel {
    ch.set_resolved_bot_open_id(Some(bot_open_id.to_string()));
    ch
}

fn make_channel() -> LarkChannel {
    with_bot_open_id(
        LarkChannel::new(
            "cli_test_app_id".into(),
            "test_app_secret".into(),
            "test_verification_token".into(),
            None,
            vec!["ou_testuser123".into()],
            true,
        ),
        "ou_bot",
    )
}

fn with_test_api_base(mut ch: LarkChannel, api_base: &str) -> LarkChannel {
    ch.api_base_override = Some(api_base.to_string());
    ch.ws_base_override = Some(api_base.to_string());
    ch
}

async fn with_cached_token(ch: LarkChannel, token: &str) -> LarkChannel {
    {
        let mut cached = ch.tenant_token.write().await;
        *cached = Some(CachedTenantToken {
            value: token.to_string(),
            refresh_after: Instant::now() + Duration::from_secs(300),
        });
    }
    ch
}

#[test]
fn lark_channel_name() {
    let ch = make_channel();
    assert_eq!(ch.name(), "lark");
}

#[test]
fn lark_ws_activity_refreshes_heartbeat_watchdog() {
    assert!(should_refresh_last_recv(&WsMsg::Binary(
        vec![1, 2, 3].into()
    )));
    assert!(should_refresh_last_recv(&WsMsg::Ping(vec![9, 9].into())));
    assert!(should_refresh_last_recv(&WsMsg::Pong(vec![8, 8].into())));
}

#[test]
fn lark_ws_non_activity_frames_do_not_refresh_heartbeat_watchdog() {
    assert!(!should_refresh_last_recv(&WsMsg::Text("hello".into())));
    assert!(!should_refresh_last_recv(&WsMsg::Close(None)));
}

#[test]
fn lark_group_response_requires_matching_bot_mention_when_ids_available() {
    let mentions = vec![serde_json::json!({
        "id": { "open_id": "ou_other" }
    })];
    assert!(!should_respond_in_group(
        true,
        Some("ou_bot"),
        &mentions,
        &[]
    ));

    let mentions = vec![serde_json::json!({
        "id": { "open_id": "ou_bot" }
    })];
    assert!(should_respond_in_group(
        true,
        Some("ou_bot"),
        &mentions,
        &[]
    ));
}

#[test]
fn lark_group_response_requires_resolved_open_id_when_mention_only_enabled() {
    let mentions = vec![serde_json::json!({
        "id": { "open_id": "ou_any" }
    })];
    assert!(!should_respond_in_group(true, None, &mentions, &[]));
}

#[test]
fn lark_group_response_allows_post_mentions_for_bot_open_id() {
    assert!(should_respond_in_group(
        true,
        Some("ou_bot"),
        &[],
        &[String::from("ou_bot")]
    ));
}

#[test]
fn lark_should_refresh_token_on_http_401() {
    let body = serde_json::json!({ "code": 0 });
    assert!(should_refresh_lark_tenant_token(
        reqwest::StatusCode::UNAUTHORIZED,
        &body
    ));
}

#[test]
fn lark_should_refresh_token_on_body_code_99991663() {
    let body = serde_json::json!({
        "code": LARK_INVALID_ACCESS_TOKEN_CODE,
        "msg": "Invalid access token for authorization."
    });
    assert!(should_refresh_lark_tenant_token(
        reqwest::StatusCode::OK,
        &body
    ));
}

#[test]
fn lark_should_not_refresh_token_on_success_body() {
    let body = serde_json::json!({ "code": 0, "msg": "ok" });
    assert!(!should_refresh_lark_tenant_token(
        reqwest::StatusCode::OK,
        &body
    ));
}

#[test]
fn lark_extract_token_ttl_seconds_supports_expire_and_expires_in() {
    let body_expire = serde_json::json!({ "expire": 7200 });
    let body_expires_in = serde_json::json!({ "expires_in": 3600 });
    let body_missing = serde_json::json!({});
    assert_eq!(extract_lark_token_ttl_seconds(&body_expire), 7200);
    assert_eq!(extract_lark_token_ttl_seconds(&body_expires_in), 3600);
    assert_eq!(
        extract_lark_token_ttl_seconds(&body_missing),
        LARK_DEFAULT_TOKEN_TTL.as_secs()
    );
}

#[test]
fn lark_next_token_refresh_deadline_reserves_refresh_skew() {
    let now = Instant::now();
    let regular = next_token_refresh_deadline(now, 7200);
    let short_ttl = next_token_refresh_deadline(now, 60);

    assert_eq!(regular.duration_since(now), Duration::from_secs(7080));
    assert_eq!(short_ttl.duration_since(now), Duration::from_secs(1));
}

#[test]
fn lark_ensure_send_success_rejects_non_zero_code() {
    let ok = serde_json::json!({ "code": 0 });
    let bad = serde_json::json!({ "code": 12345, "msg": "bad request" });

    assert!(ensure_lark_send_success(reqwest::StatusCode::OK, &ok, "test").is_ok());
    assert!(ensure_lark_send_success(reqwest::StatusCode::OK, &bad, "test").is_err());
}

#[test]
fn lark_user_allowed_exact() {
    let ch = make_channel();
    assert!(ch.is_user_allowed("ou_testuser123"));
    assert!(!ch.is_user_allowed("ou_other"));
}

#[test]
fn lark_user_allowed_wildcard() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    assert!(ch.is_user_allowed("ou_anyone"));
}

#[test]
fn lark_user_denied_empty() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec![],
        true,
    );
    assert!(!ch.is_user_allowed("ou_anyone"));
}

#[test]
fn lark_parse_challenge() {
    let ch = make_channel();
    let payload = serde_json::json!({
        "challenge": "abc123",
        "token": "test_verification_token",
        "type": "url_verification"
    });
    // Challenge payloads should not produce messages
    let msgs = ch.parse_event_payload(&payload);
    assert!(msgs.is_empty());
}

#[test]
fn lark_parse_valid_text_message() {
    let ch = make_channel();
    let payload = serde_json::json!({
        "header": {
            "event_type": "im.message.receive_v1"
        },
        "event": {
            "sender": {
                "sender_id": {
                    "open_id": "ou_testuser123"
                }
            },
            "message": {
                "message_type": "text",
                "content": "{\"text\":\"Hello ZeroClaw!\"}",
                "chat_id": "oc_chat123",
                "create_time": "1699999999000"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "Hello ZeroClaw!");
    assert_eq!(
        msgs[0].sender, "ou_testuser123",
        "DM sender should preserve official senderId semantics"
    );
    assert_eq!(msgs[0].reply_target, "oc_chat123");
    assert_eq!(msgs[0].channel, "lark");
    assert_eq!(msgs[0].timestamp, 1_699_999_999);
    let context = msgs[0]
        .context
        .as_ref()
        .expect("lark inbound context should be attached");
    assert_eq!(context.sender_id.as_deref(), Some("ou_testuser123"));
    assert_eq!(context.chat_id.as_deref(), Some("oc_chat123"));
    assert_eq!(context.chat_type.as_deref(), Some("p2p"));
    assert_eq!(context.content_type.as_deref(), Some("text"));
    assert_eq!(
        context.raw_content.as_deref(),
        Some("{\"text\":\"Hello ZeroClaw!\"}")
    );
    assert_eq!(
        context.origin_from.as_deref(),
        Some("feishu:ou_testuser123")
    );
    assert_eq!(context.origin_to.as_deref(), Some("user:ou_testuser123"));
    assert_eq!(context.envelope_from.as_deref(), Some("ou_testuser123"));
}

#[test]
fn lark_parse_group_text_message_keeps_chat_scoped_sender_for_shared_session() {
    let ch = with_bot_open_id(
        LarkChannel::new(
            "cli_test_app_id".into(),
            "test_app_secret".into(),
            "test_verification_token".into(),
            None,
            vec!["ou_testuser123".into()],
            false,
        ),
        "ou_bot",
    );
    let payload = serde_json::json!({
        "header": {
            "event_type": "im.message.receive_v1"
        },
        "event": {
            "sender": {
                "sender_id": {
                    "open_id": "ou_testuser123"
                }
            },
            "message": {
                "message_type": "text",
                "content": "{\"text\":\"Hello group\"}",
                "chat_id": "oc_group123",
                "chat_type": "group",
                "create_time": "1699999999000"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].sender, "oc_group123");
    assert_eq!(msgs[0].reply_target, "oc_group123");
    let context = msgs[0]
        .context
        .as_ref()
        .expect("group inbound context should be attached");
    assert_eq!(context.sender_id.as_deref(), Some("ou_testuser123"));
    assert_eq!(context.chat_id.as_deref(), Some("oc_group123"));
    assert_eq!(context.chat_type.as_deref(), Some("group"));
    assert_eq!(context.origin_to.as_deref(), Some("chat:oc_group123"));
    assert_eq!(
        context.envelope_from.as_deref(),
        Some("oc_group123:ou_testuser123")
    );
}

#[test]
fn lark_parse_unauthorized_user() {
    let ch = make_channel();
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_unauthorized" } },
            "message": {
                "message_type": "text",
                "content": "{\"text\":\"spam\"}",
                "chat_id": "oc_chat",
                "create_time": "1000"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert!(msgs.is_empty());
}

#[test]
fn lark_parse_non_text_message_produces_media_placeholder() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "image",
                "content": "{\"image_key\":\"img_v3_demo\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "<media:image>");
}

#[test]
fn lark_parse_location_message_builds_official_structured_content() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "location",
                "content": "{\"name\":\"ByteDance HQ\",\"latitude\":\"39.90\",\"longitude\":\"116.40\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].content,
        "<location name=\"ByteDance HQ\" coords=\"lat:39.90,lng:116.40\"/>"
    );
}

#[test]
fn lark_parse_system_message_renders_template_placeholders() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "system",
                "content": "{\"template\":\"{from_user} joined {to_chatters}\",\"from_user\":[\"Alice\"],\"to_chatters\":[\"Project Group\"]}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "Alice joined Project Group");
}

#[test]
fn lark_parse_unknown_message_type_falls_back_to_explicit_placeholder() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "mystery_type",
                "content": "{\"foo\":\"bar\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "[unsupported message]");
}

#[test]
fn lark_parse_share_user_message_builds_contact_card_content() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "share_user",
                "content": "{\"user_id\":\"ou_target\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "<contact_card id=\"ou_target\"/>");
}

#[test]
fn lark_parse_todo_message_builds_structured_todo_content() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "todo",
                "content": "{\"summary\":{\"title\":\"发布版本\",\"content\":[[{\"text\":\"检查 changelog\"}]]},\"due_time\":\"2026-03-16T10:00:00Z\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].content,
        "<todo>\n发布版本\n检查 changelog\nDue: 2026-03-16T10:00:00Z\n</todo>"
    );
}

#[test]
fn lark_parse_calendar_message_builds_structured_calendar_content() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "calendar",
                "content": "{\"summary\":\"项目例会\",\"start_time\":\"2026-03-16T10:00:00Z\",\"end_time\":\"2026-03-16T11:00:00Z\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].content,
        "<calendar_invite>📅 项目例会\n🕙 2026-03-16T10:00:00Z ~ 2026-03-16T11:00:00Z</calendar_invite>"
    );
}

#[test]
fn lark_parse_sticker_message_builds_structured_sticker_content() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "sticker",
                "content": "{\"file_key\":\"stk_v3_demo\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "<sticker key=\"stk_v3_demo\"/>");
}

#[test]
fn lark_parse_folder_message_builds_structured_folder_content() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "folder",
                "content": "{\"file_key\":\"fld_v3_demo\",\"file_name\":\"设计资料\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].content,
        "<folder key=\"fld_v3_demo\" name=\"设计资料\"/>"
    );
}

#[test]
fn lark_parse_hongbao_message_builds_structured_red_packet_content() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "hongbao",
                "content": "{\"text\":\"恭喜发财\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "<hongbao text=\"恭喜发财\"/>");
}

#[test]
fn lark_parse_share_chat_message_builds_group_card_content() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "share_chat",
                "content": "{\"chat_id\":\"oc_target_group\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "<group_card id=\"oc_target_group\"/>");
}

#[test]
fn lark_parse_share_chat_message_preserves_empty_group_id_shape() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "share_chat",
                "content": "{}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "<group_card id=\"\"/>");
}

#[test]
fn lark_parse_share_user_message_preserves_empty_contact_id_shape() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "share_user",
                "content": "{}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "<contact_card id=\"\"/>");
}

#[test]
fn lark_parse_vote_message_builds_structured_vote_content() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "vote",
                "content": "{\"topic\":\"午饭吃什么\",\"options\":[\"饺子\",\"面条\"]}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].content,
        "<vote>\n午饭吃什么\n• 饺子\n• 面条\n</vote>"
    );
}

#[test]
fn lark_parse_video_chat_message_builds_structured_meeting_content() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "video_chat",
                "content": "{\"topic\":\"项目同步会\",\"start_time\":1704067200000}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].content,
        "<meeting>📹 项目同步会\n🕙 2024-01-01 08:00</meeting>"
    );
}

#[test]
fn lark_parse_todo_message_formats_millis_due_time_like_official_converter() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "todo",
                "content": "{\"summary\":{\"title\":\"发布版本\",\"content\":[[{\"text\":\"检查 changelog\"}]]},\"due_time\":1704067200000}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].content,
        "<todo>\n发布版本\n检查 changelog\nDue: 2024-01-01 08:00\n</todo>"
    );
}

#[test]
fn lark_parse_calendar_message_formats_millis_window_like_official_converter() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "calendar",
                "content": "{\"summary\":\"项目例会\",\"start_time\":1704067200000,\"end_time\":1704070800000}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].content,
        "<calendar_invite>📅 项目例会\n🕙 2024-01-01 08:00 ~ 2024-01-01 09:00</calendar_invite>"
    );
}

#[test]
fn lark_parse_attachment_markers_extracts_local_images() {
    let (cleaned, attachments) =
        parse_lark_attachment_markers("先看图 [IMAGE:/tmp/snap.png] 然后回复");

    assert_eq!(cleaned, "先看图  然后回复");
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].kind, LarkAttachmentKind::Image);
    assert_eq!(attachments[0].target, "/tmp/snap.png");
}

#[test]
fn lark_parse_attachment_markers_extracts_local_documents() {
    let (cleaned, attachments) =
        parse_lark_attachment_markers("请发送 [DOCUMENT:/tmp/spec.pdf] 给对方");

    assert_eq!(cleaned, "请发送  给对方");
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].kind, LarkAttachmentKind::Document);
    assert_eq!(attachments[0].target, "/tmp/spec.pdf");
}

#[test]
fn lark_parse_attachment_markers_extracts_audio_and_video() {
    let (_, attachments) =
        parse_lark_attachment_markers("发语音 [AUDIO:/tmp/demo.ogg] 和视频 [VIDEO:/tmp/demo.mp4]");

    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].kind, LarkAttachmentKind::Audio);
    assert_eq!(attachments[1].kind, LarkAttachmentKind::Video);
}

#[test]
fn lark_parse_attachment_markers_treats_voice_as_audio() {
    let (cleaned, attachments) = parse_lark_attachment_markers("[VOICE:/tmp/demo.ogg]");

    assert!(cleaned.is_empty());
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].kind, LarkAttachmentKind::Audio);
    assert_eq!(attachments[0].target, "/tmp/demo.ogg");
}

#[test]
fn lark_parse_inbound_image_resource_extracts_file_key() {
    let resources = parse_lark_inbound_resources("image", "{\"image_key\":\"img_v3_demo\"}");

    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].file_key, "img_v3_demo");
    assert_eq!(
        resources[0].kind.resource_type(),
        "image",
        "image resources should map to message resource type=image"
    );
}

#[test]
fn lark_parse_inbound_file_resource_extracts_file_name() {
    let resources = parse_lark_inbound_resources(
        "file",
        "{\"file_key\":\"file_v3_demo\",\"file_name\":\"report.pdf\"}",
    );

    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].file_key, "file_v3_demo");
    assert_eq!(resources[0].file_name.as_deref(), Some("report.pdf"));
}

#[tokio::test]
async fn lark_store_inbound_resource_writes_to_account_scoped_path() {
    let workspace = TempDir::new().unwrap();
    let resource = LarkInboundResource {
        kind: LarkInboundResourceKind::Image,
        file_key: "img_v3_demo".to_string(),
        file_name: Some("photo.png".to_string()),
    };

    let stored = store_inbound_resource(
        workspace.path(),
        "feishu",
        "primary",
        "om_123",
        &resource,
        b"\x89PNG\r\n\x1a\nfake",
        Some("image/png"),
        Some("photo.png"),
    )
    .await
    .expect("resource should be written");

    assert_eq!(stored.kind, LarkInboundResourceKind::Image);
    assert!(
        stored.path.exists(),
        "stored path missing: {}",
        stored.path.display()
    );
    assert!(
        stored
            .path
            .to_string_lossy()
            .contains("/channels/feishu/primary/inbound/"),
        "stored path should be account-scoped: {}",
        stored.path.display()
    );
}

#[tokio::test]
async fn lark_store_inbound_resource_rejects_oversized_payload() {
    let workspace = TempDir::new().unwrap();
    let resource = LarkInboundResource {
        kind: LarkInboundResourceKind::File,
        file_key: "file_v3_big".to_string(),
        file_name: Some("big.bin".to_string()),
    };

    let err = store_inbound_resource_with_limit(
        workspace.path(),
        "feishu",
        "primary",
        "om_big",
        &resource,
        &[0_u8; 8],
        Some("application/octet-stream"),
        Some("big.bin"),
        4,
    )
    .await
    .expect_err("oversized resource should fail");

    assert!(
        err.to_string().contains("exceeds size limit"),
        "unexpected error: {err}"
    );
}

#[test]
fn lark_parse_path_only_attachment_detects_image_file() {
    let dir = TempDir::new().unwrap();
    let image_path = dir.path().join("screen.png");
    std::fs::write(&image_path, b"fake-png").unwrap();

    let parsed = parse_lark_path_only_attachment(image_path.to_string_lossy().as_ref())
        .expect("expected image attachment");

    assert_eq!(parsed.kind, LarkAttachmentKind::Image);
    assert_eq!(parsed.target, image_path.to_string_lossy());
}

#[test]
fn lark_parse_path_only_attachment_detects_document_file() {
    let dir = TempDir::new().unwrap();
    let doc_path = dir.path().join("spec.pdf");
    std::fs::write(&doc_path, b"%PDF-1.4").unwrap();

    let parsed = parse_lark_path_only_attachment(doc_path.to_string_lossy().as_ref())
        .expect("expected document attachment");

    assert_eq!(parsed.kind, LarkAttachmentKind::Document);
    assert_eq!(parsed.target, doc_path.to_string_lossy());
}

#[test]
fn lark_outbound_request_collects_local_attachments_and_text() {
    let dir = TempDir::new().unwrap();
    let image_path = dir.path().join("screen.png");
    let doc_path = dir.path().join("spec.pdf");
    std::fs::write(&image_path, b"fake-png").unwrap();
    std::fs::write(&doc_path, b"%PDF-1.4").unwrap();

    let content = format!(
        "请先看图 [IMAGE:{}] 再读文件 [DOCUMENT:{}]",
        image_path.display(),
        doc_path.display()
    );
    let message = SendMessage::new(content.clone(), "oc_chat123");
    let outbound = LarkOutboundRequest::from_send_message(&message, &content);

    assert_eq!(outbound.target, "oc_chat123");
    assert_eq!(outbound.local_attachments.len(), 2);
    assert_eq!(outbound.local_attachments[0].kind, LarkAttachmentKind::Image);
    assert_eq!(outbound.local_attachments[1].kind, LarkAttachmentKind::Document);
    assert!(
        outbound.text.contains("请先看图"),
        "text segments should preserve surrounding text"
    );
}

#[test]
fn lark_outbound_request_voice_marker_does_not_leave_text() {
    let tmp = TempDir::new().unwrap();
    let audio_path = tmp.path().join("voice.ogg");
    std::fs::write(&audio_path, b"fake audio").unwrap();

    let raw = format!("[VOICE:{}]", audio_path.display());
    let msg = SendMessage::new(raw.clone(), "chat:oc_chat_1");
    let outbound = LarkOutboundRequest::from_send_message(&msg, &raw);

    assert!(outbound.text.is_empty(), "unexpected leftover text: {}", outbound.text);
    assert_eq!(outbound.local_attachments.len(), 1);
    assert_eq!(outbound.local_attachments[0].kind, LarkAttachmentKind::Audio);
    assert_eq!(
        outbound.local_attachments[0].target,
        audio_path.display().to_string()
    );
}

#[test]
fn lark_outbound_request_preserves_local_audio_and_video_attachment_kinds() {
    let dir = TempDir::new().unwrap();
    let audio_path = dir.path().join("voice.ogg");
    let video_path = dir.path().join("clip.mp4");
    std::fs::write(&audio_path, b"fake-ogg").unwrap();
    std::fs::write(&video_path, b"fake-mp4").unwrap();

    let content = format!(
        "发语音 [AUDIO:{}] 再发视频 [VIDEO:{}]",
        audio_path.display(),
        video_path.display()
    );
    let message = SendMessage::new(content.clone(), "oc_chat123");
    let outbound = LarkOutboundRequest::from_send_message(&message, &content);

    assert_eq!(outbound.local_attachments.len(), 2);
    assert_eq!(outbound.local_attachments[0].kind, LarkAttachmentKind::Audio);
    assert_eq!(outbound.local_attachments[1].kind, LarkAttachmentKind::Video);
}

#[test]
fn lark_outbound_request_detects_path_only_attachment() {
    let dir = TempDir::new().unwrap();
    let image_path = dir.path().join("cover.png");
    std::fs::write(&image_path, b"fake-png").unwrap();

    let raw = image_path.to_string_lossy().to_string();
    let message = SendMessage::new(raw.clone(), "oc_chat123");
    let outbound = LarkOutboundRequest::from_send_message(&message, &raw);

    let (path, kind) = outbound
        .attachment_path()
        .expect("path-only attachment should be detected");
    assert_eq!(kind, LarkAttachmentKind::Image);
    assert_eq!(path, image_path.as_path());
}

#[test]
fn lark_outbound_request_preserves_thread_reply_target() {
    let message = SendMessage::new("hello", "oc_chat123").in_thread(Some("om_root_1".into()));
    let outbound = LarkOutboundRequest::from_send_message(&message, "hello");

    assert_eq!(outbound.reply_message_id(), Some("om_root_1"));
}

#[test]
fn lark_outbound_request_normalizes_prefixed_targets_like_official_plugin() {
    let cases = [
        ("chat:oc_chat123", "oc_chat123"),
        ("user:ou_user123", "ou_user123"),
        ("open_id:ou_user123", "ou_user123"),
        ("feishu:ou_user123", "ou_user123"),
        ("oc_chat123", "oc_chat123"),
        ("ou_user123", "ou_user123"),
    ];

    for (raw_target, expected_target) in cases {
        let message = SendMessage::new("hello", raw_target);
        let outbound = LarkOutboundRequest::from_send_message(&message, "hello");
        assert_eq!(outbound.target, expected_target, "raw target={raw_target}");
    }
}

#[test]
fn lark_outbound_request_collects_remote_attachment_markers() {
    let content = "看这个 [IMAGE:https://example.com/demo.png]";
    let message = SendMessage::new(content, "oc_chat123");
    let outbound = LarkOutboundRequest::from_send_message(&message, content);

    assert_eq!(outbound.remote_attachments.len(), 1);
    assert!(outbound.unresolved_markers.is_empty());
}

#[test]
fn lark_parse_card_message_from_fenced_block() {
    let raw = r#"```lark-card
{"config":{"wide_screen_mode":true},"elements":[{"tag":"markdown","content":"hello"}]}
```"#;

    let card = parse_lark_card_message(raw).expect("card payload should parse");

    assert_eq!(card.content["config"]["wide_screen_mode"], true);
    assert_eq!(card.content["elements"][0]["tag"], "markdown");
}

#[test]
fn lark_parse_card_message_from_fenced_block_with_bare_elements() {
    let raw = r#"```feishu-card
{"elements":[{"tag":"markdown","content":"hello"}]}
```"#;

    let card = parse_lark_card_message(raw).expect("bare elements card should parse");

    assert_eq!(card.content["elements"][0]["tag"], "markdown");
    assert_eq!(card.content["elements"][0]["content"], "hello");
}

#[test]
fn lark_parse_card_message_from_raw_json_text() {
    let raw = r#"{"schema":"2.0","body":{"elements":[{"tag":"markdown","content":"hello"}]}}"#;

    let card = parse_lark_card_message(raw).expect("raw card json should parse");

    assert_eq!(card.content["schema"], "2.0");
    assert_eq!(card.content["body"]["elements"][0]["tag"], "markdown");
}

#[test]
fn lark_parse_card_message_unwraps_interactive_wrapper_like_official_plugin() {
    let raw = r#"{"msg_type":"interactive","card":{"schema":"2.0","body":{"elements":[{"tag":"markdown","content":"wrapped"}]}}}"#;

    let card = parse_lark_card_message(raw).expect("wrapped interactive card should parse");

    assert_eq!(card.content["schema"], "2.0");
    assert_eq!(card.content["body"]["elements"][0]["content"], "wrapped");
}

#[test]
fn lark_build_image_message_body_uses_image_key() {
    let body = build_lark_image_message_body("oc_chat123", "img_v3_key");

    assert_eq!(body["receive_id"], "oc_chat123");
    assert_eq!(body["msg_type"], "image");
    assert_eq!(
        body["content"],
        serde_json::json!({ "image_key": "img_v3_key" }).to_string()
    );
}

#[test]
fn lark_build_file_message_body_uses_file_key() {
    let body = build_lark_file_message_body("oc_chat123", "file_v3_key");

    assert_eq!(body["receive_id"], "oc_chat123");
    assert_eq!(body["msg_type"], "file");
    assert_eq!(
        body["content"],
        serde_json::json!({ "file_key": "file_v3_key" }).to_string()
    );
}

#[test]
fn lark_build_card_message_body_uses_interactive_type() {
    let card = parse_lark_card_message(
        r#"```feishu-card
{"elements":[{"tag":"markdown","content":"hello card"}]}
```"#,
    )
    .expect("card payload should parse");
    let body = build_lark_card_message_body("oc_chat123", &card);

    assert_eq!(body["receive_id"], "oc_chat123");
    assert_eq!(body["msg_type"], "interactive");
    assert_eq!(
        body["content"],
        serde_json::json!({"elements":[{"tag":"markdown","content":"hello card"}]}).to_string()
    );
}

#[test]
fn lark_build_text_message_body_uses_post_payload_like_official_plugin() {
    let body = build_lark_text_message_body("oc_chat123", "hello **world**");
    let content: serde_json::Value =
        serde_json::from_str(body["content"].as_str().expect("content string")).unwrap();

    assert_eq!(body["receive_id"], "oc_chat123");
    assert_eq!(body["msg_type"], "post");
    assert_eq!(
        content,
        serde_json::json!({
            "zh_cn": {
                "content": [[{ "tag": "md", "text": "hello **world**" }]]
            }
        })
    );
}

#[test]
fn lark_build_post_content_normalizes_common_at_mention_forms_like_official_plugin() {
    let content = build_lark_post_content(
        r#"<at id=all></at> <at open_id="ou_user1"></at> <at user_id=ou_user2>张三</at>"#,
    );

    assert_eq!(
        content,
        serde_json::json!({
            "zh_cn": {
                "content": [[{
                    "tag": "md",
                    "text": r#"<at user_id="all"></at> <at user_id="ou_user1"></at> <at user_id="ou_user2">张三</at>"#,
                }]]
            }
        })
    );
}

#[test]
fn lark_build_post_content_applies_official_markdown_heading_downgrade() {
    let content = build_lark_post_content("# Title\n## Section\nplain");

    assert_eq!(
        content,
        serde_json::json!({
            "zh_cn": {
                "content": [[{
                    "tag": "md",
                    "text": "#### Title\n##### Section\nplain",
                }]]
            }
        })
    );
}

#[test]
fn lark_build_post_content_strips_invalid_markdown_image_keys_like_official_plugin() {
    let content = build_lark_post_content(
        "![bad](local/path.png) ![good](img_v3_demo) ![url](https://example.com/a.png)",
    );

    assert_eq!(
        content,
        serde_json::json!({
            "zh_cn": {
                "content": [[{
                    "tag": "md",
                    "text": "local/path.png ![good](img_v3_demo) ![url](https://example.com/a.png)",
                }]]
            }
        })
    );
}

#[test]
fn lark_build_post_content_converts_simple_markdown_table_to_bullets_like_official_plugin() {
    let content =
        build_lark_post_content("| Name | Value |\n|------|-------|\n| A | 1 |\n| B | 2 |");

    assert_eq!(
        content,
        serde_json::json!({
            "zh_cn": {
                "content": [[{
                    "tag": "md",
                    "text": "**A**\n• Value: 1\n\n**B**\n• Value: 2",
                }]]
            }
        })
    );
}

#[test]
fn lark_build_post_content_converts_multi_column_markdown_table_to_bullets_like_official_plugin() {
    let content = build_lark_post_content(
        "| Feature | SQLite | Postgres |\n|---------|--------|----------|\n| Speed | Fast | Medium |\n| Scale | Small | Large |",
    );

    assert_eq!(
        content,
        serde_json::json!({
            "zh_cn": {
                "content": [[{
                    "tag": "md",
                    "text": "**Speed**\n• SQLite: Fast\n• Postgres: Medium\n\n**Scale**\n• SQLite: Small\n• Postgres: Large",
                }]]
            }
        })
    );
}

#[test]
fn lark_build_post_content_preserves_inline_styles_and_links_inside_markdown_table_cells() {
    let content = build_lark_post_content(
        "| Name | Value |\n|------|-------|\n| _Row_ | [Link](https://example.com) |",
    );

    assert_eq!(
        content,
        serde_json::json!({
            "zh_cn": {
                "content": [[{
                    "tag": "md",
                    "text": "**_Row_**\n• Value: [Link](https://example.com)",
                }]]
            }
        })
    );
}

#[test]
fn lark_build_post_content_handles_empty_markdown_table_cells_like_official_plugin() {
    let content = build_lark_post_content("| Name | Value |\n|------|-------|\n| A | |\n| B | 2 |");

    assert_eq!(
        content,
        serde_json::json!({
            "zh_cn": {
                "content": [[{
                    "tag": "md",
                    "text": "**A**\n\n**B**\n• Value: 2",
                }]]
            }
        })
    );
}

#[test]
fn lark_build_post_content_preserves_fenced_code_blocks_while_normalizing_text() {
    let content =
        build_lark_post_content("# Title\n```md\n# not-a-heading\n![bad](local.png)\n```");

    assert_eq!(
        content,
        serde_json::json!({
            "zh_cn": {
                "content": [[{
                    "tag": "md",
                    "text": "#### Title\n```md\n# not-a-heading\nlocal.png\n```",
                }]]
            }
        })
    );
}

#[test]
fn lark_build_reply_message_body_sets_reply_in_thread() {
    let body = build_lark_reply_message_body(
        "post",
        serde_json::json!({
            "zh_cn": {
                "content": [[{ "tag": "md", "text": "hello thread" }]]
            }
        }),
        true,
    );

    assert_eq!(body["msg_type"], "post");
    let content: serde_json::Value =
        serde_json::from_str(body["content"].as_str().expect("content string")).unwrap();
    assert_eq!(
        content,
        serde_json::json!({
            "zh_cn": {
                "content": [[{ "tag": "md", "text": "hello thread" }]]
            }
        })
    );
    assert_eq!(body["reply_in_thread"], true);
}

#[test]
fn lark_build_reply_card_message_body_sets_interactive_reply() {
    let card = parse_lark_card_message(
        r#"```lark-card
{"elements":[{"tag":"markdown","content":"thread card"}]}
```"#,
    )
    .expect("card payload should parse");
    let body = build_lark_reply_card_message_body(&card, true);

    assert_eq!(body["msg_type"], "interactive");
    assert_eq!(body["reply_in_thread"], true);
    assert_eq!(
        body["content"],
        serde_json::json!({"elements":[{"tag":"markdown","content":"thread card"}]}).to_string()
    );
}

#[test]
fn lark_build_streaming_cards_reflect_state_labels() {
    use crate::channels::lark::cards::{build_lark_streaming_card, LarkCardPhase};

    let thinking = build_lark_streaming_card(LarkCardPhase::Thinking, "");
    let generating = build_lark_streaming_card(LarkCardPhase::Generating, "partial answer");
    let completed = build_lark_streaming_card(LarkCardPhase::Completed, "final answer");
    let failed = build_lark_streaming_card(LarkCardPhase::Failed, "something failed");

    assert_eq!(thinking.content["header"]["title"]["content"], "Thinking");
    assert_eq!(
        generating.content["header"]["title"]["content"],
        "Generating"
    );
    assert_eq!(completed.content["header"]["title"]["content"], "Completed");
    assert_eq!(failed.content["header"]["title"]["content"], "Failed");
    assert_eq!(
        generating.content["body"]["elements"][0]["content"],
        "partial answer"
    );
}

#[test]
fn lark_build_confirmation_card_contains_expected_actions() {
    let card = crate::channels::lark::cards::build_lark_confirmation_card(
        "op_123",
        "Need confirmation before writing",
        Some("file.txt"),
    );

    assert_eq!(
        card.content["header"]["title"]["content"],
        "Confirmation Required"
    );
    assert_eq!(
        card.content["body"]["elements"][4]["actions"][0]["value"]["action"],
        "confirm_write"
    );
    assert_eq!(
        card.content["body"]["elements"][4]["actions"][1]["value"]["action"],
        "reject_write"
    );
    assert_eq!(
        card.content["body"]["elements"][2]["text"]["content"],
        "**Preview:**\nfile.txt"
    );
}

#[test]
fn lark_build_audio_and_video_message_bodies_use_expected_types() {
    let audio = build_lark_audio_message_body("oc_chat123", "file_audio_key");
    let video = build_lark_video_message_body("oc_chat123", "file_video_key");

    assert_eq!(audio["msg_type"], "audio");
    assert_eq!(
        audio["content"],
        serde_json::json!({ "file_key": "file_audio_key" }).to_string()
    );
    assert_eq!(video["msg_type"], "media");
    assert_eq!(
        video["content"],
        serde_json::json!({ "file_key": "file_video_key" }).to_string()
    );
}

#[tokio::test]
async fn lark_materialize_outbound_attachment_accepts_file_url() {
    let workspace = TempDir::new().unwrap();
    let local_file = workspace.path().join("demo.png");
    std::fs::write(&local_file, b"fake-png").unwrap();
    let client = reqwest::Client::new();
    let target = format!("file://{}", local_file.display());

    let resolved = materialize_outbound_attachment(
        &client,
        Some(workspace.path()),
        LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES,
        None,
        "feishu",
        "primary",
        LarkInboundResourceKind::Image,
        &target,
    )
    .await
    .expect("file URL should resolve to local path");

    assert_eq!(resolved, std::fs::canonicalize(local_file).unwrap());
}

#[tokio::test]
async fn lark_materialize_outbound_attachment_rejects_remote_content_length_over_limit() {
    let server = MockServer::start().await;
    let oversized = vec![b'x'; LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES + 1];
    Mock::given(method("GET"))
        .and(path("/demo/huge.bin"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/octet-stream")
                .set_body_bytes(oversized),
        )
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let err = materialize_outbound_attachment(
        &client,
        None,
        LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES,
        None,
        "feishu",
        "primary",
        LarkInboundResourceKind::File,
        &format!("{}/demo/huge.bin", server.uri()),
    )
    .await
    .expect_err("oversized remote media should be rejected");

    let rendered = format!("{err:#}");
    assert!(rendered.contains("size limit"), "unexpected error: {rendered}");
}

#[tokio::test]
async fn lark_materialize_outbound_attachment_honors_configured_local_roots() {
    let allowed_root = TempDir::new().unwrap();
    let outside_root = TempDir::new().unwrap();
    let outside_file = outside_root.path().join("demo.png");
    std::fs::write(&outside_file, b"fake-png").unwrap();
    let client = reqwest::Client::new();
    let target = format!("file://{}", outside_file.display());

    let err = materialize_outbound_attachment(
        &client,
        None,
        LARK_DEFAULT_INBOUND_MEDIA_MAX_BYTES,
        Some(vec![allowed_root.path().to_path_buf()]),
        "feishu",
        "primary",
        LarkInboundResourceKind::Image,
        &target,
    )
    .await
    .expect_err("configured media_local_roots should restrict outbound attachment paths");

    assert!(err
        .to_string()
        .contains("Local media path is not under an allowed directory"));
}

#[test]
fn lark_parse_empty_text_skipped() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "text",
                "content": "{\"text\":\"\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert!(msgs.is_empty());
}

#[test]
fn lark_parse_wrong_event_type() {
    let ch = make_channel();
    let payload = serde_json::json!({
        "header": { "event_type": "im.chat.disbanded_v1" },
        "event": {}
    });

    let msgs = ch.parse_event_payload(&payload);
    assert!(msgs.is_empty());
}

#[test]
fn lark_parse_card_action_event_maps_to_channel_message() {
    let ch = make_channel();
    let payload = serde_json::json!({
        "header": { "event_type": "card.action.trigger" },
        "event": {
            "open_chat_id": "oc_chat123",
            "open_message_id": "om_card_1",
            "operator": {
                "open_id": "ou_user_1"
            },
            "action": {
                "value": {
                    "action": "confirm_write",
                    "operation_id": "op_123"
                }
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);

    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].reply_target, "oc_chat123");
    assert_eq!(msgs[0].sender, "ou_user_1");
    assert_eq!(msgs[0].thread_ts.as_deref(), Some("om_card_1"));
    assert!(msgs[0].content.contains("confirm_write"));
    assert!(msgs[0].content.contains("op_123"));
}

#[test]
fn lark_parse_missing_sender() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "message": {
                "message_type": "text",
                "content": "{\"text\":\"hello\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert!(msgs.is_empty());
}

#[test]
fn lark_parse_unicode_message() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "text",
                "content": "{\"text\":\"Hello world 🌍\"}",
                "chat_id": "oc_chat",
                "create_time": "1000"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "Hello world 🌍");
}

#[test]
fn lark_parse_missing_event() {
    let ch = make_channel();
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert!(msgs.is_empty());
}

#[test]
fn lark_parse_invalid_content_json() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "text",
                "content": "not valid json",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert!(msgs.is_empty());
}

#[test]
fn lark_config_serde() {
    use crate::config::schema::{LarkConfig, LarkReceiveMode};
    let lc = LarkConfig {
        app_id: "cli_app123".into(),
        app_secret: "secret456".into(),
        encrypt_key: None,
        verification_token: Some("vtoken789".into()),
        allowed_users: vec!["ou_user1".into(), "ou_user2".into()],
        mention_only: false,
        use_feishu: false,
        receive_mode: LarkReceiveMode::default(),
        port: None,
        media_max_mb: None,
        media_local_roots: Vec::new(),
    };
    let json = serde_json::to_string(&lc).unwrap();
    let parsed: LarkConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.app_id, "cli_app123");
    assert_eq!(parsed.app_secret, "secret456");
    assert_eq!(parsed.verification_token.as_deref(), Some("vtoken789"));
    assert_eq!(parsed.allowed_users.len(), 2);
}

#[test]
fn lark_config_toml_roundtrip() {
    use crate::config::schema::{LarkConfig, LarkReceiveMode};
    let lc = LarkConfig {
        app_id: "app".into(),
        app_secret: "secret".into(),
        encrypt_key: None,
        verification_token: Some("tok".into()),
        allowed_users: vec!["*".into()],
        mention_only: false,
        use_feishu: false,
        receive_mode: LarkReceiveMode::Webhook,
        port: Some(9898),
        media_max_mb: None,
        media_local_roots: Vec::new(),
    };
    let toml_str = toml::to_string(&lc).unwrap();
    let parsed: LarkConfig = toml::from_str(&toml_str).unwrap();
    assert_eq!(parsed.app_id, "app");
    assert_eq!(parsed.verification_token.as_deref(), Some("tok"));
    assert_eq!(parsed.allowed_users, vec!["*"]);
}

#[test]
fn lark_config_defaults_optional_fields() {
    use crate::config::schema::{LarkConfig, LarkReceiveMode};
    let json = r#"{"app_id":"a","app_secret":"s"}"#;
    let parsed: LarkConfig = serde_json::from_str(json).unwrap();
    assert!(parsed.verification_token.is_none());
    assert!(parsed.allowed_users.is_empty());
    assert!(!parsed.mention_only);
    assert_eq!(parsed.receive_mode, LarkReceiveMode::Websocket);
    assert!(parsed.port.is_none());
}

#[test]
fn lark_from_config_preserves_mode_and_region() {
    use crate::config::schema::{LarkConfig, LarkReceiveMode};

    let cfg = LarkConfig {
        app_id: "cli_app123".into(),
        app_secret: "secret456".into(),
        encrypt_key: None,
        verification_token: Some("vtoken789".into()),
        allowed_users: vec!["*".into()],
        mention_only: false,
        use_feishu: false,
        receive_mode: LarkReceiveMode::Webhook,
        port: Some(9898),
        media_max_mb: None,
        media_local_roots: Vec::new(),
    };

    let ch = LarkChannel::from_config(&cfg);

    assert_eq!(ch.api_base(), LARK_BASE_URL);
    assert_eq!(ch.ws_base(), LARK_WS_BASE_URL);
    assert_eq!(ch.receive_mode, LarkReceiveMode::Webhook);
    assert_eq!(ch.port, Some(9898));
}

#[test]
fn lark_from_lark_config_ignores_legacy_feishu_flag() {
    use crate::config::schema::{LarkConfig, LarkReceiveMode};

    let cfg = LarkConfig {
        app_id: "cli_app123".into(),
        app_secret: "secret456".into(),
        encrypt_key: None,
        verification_token: Some("vtoken789".into()),
        allowed_users: vec!["*".into()],
        mention_only: false,
        use_feishu: true,
        receive_mode: LarkReceiveMode::Webhook,
        port: Some(9898),
        media_max_mb: None,
        media_local_roots: Vec::new(),
    };

    let ch = LarkChannel::from_lark_config(&cfg);

    assert_eq!(ch.api_base(), LARK_BASE_URL);
    assert_eq!(ch.ws_base(), LARK_WS_BASE_URL);
    assert_eq!(ch.name(), "lark");
}

#[test]
fn lark_from_feishu_config_sets_feishu_platform() {
    use crate::config::schema::{FeishuConfig, LarkReceiveMode};

    let cfg = FeishuConfig {
        app_id: "cli_feishu_app123".into(),
        app_secret: "secret456".into(),
        enabled: None,
        encrypt_key: None,
        verification_token: Some("vtoken789".into()),
        allowed_users: vec!["*".into()],
        receive_mode: LarkReceiveMode::Webhook,
        port: Some(9898),
        media_max_mb: None,
        media_local_roots: Vec::new(),
    };

    let ch = LarkChannel::from_feishu_config(&cfg);

    assert_eq!(ch.api_base(), FEISHU_BASE_URL);
    assert_eq!(ch.ws_base(), FEISHU_WS_BASE_URL);
    assert_eq!(ch.name(), "feishu");
    assert_eq!(ch.account_id(), "default");
}

#[test]
fn lark_from_named_feishu_config_sets_account_identity() {
    use crate::config::schema::{FeishuConfig, LarkReceiveMode};

    let cfg = FeishuConfig {
        app_id: "cli_feishu_app123".into(),
        app_secret: "secret456".into(),
        enabled: None,
        encrypt_key: None,
        verification_token: Some("vtoken789".into()),
        allowed_users: vec!["*".into()],
        receive_mode: LarkReceiveMode::Webhook,
        port: Some(9898),
        media_max_mb: None,
        media_local_roots: Vec::new(),
    };

    let ch = LarkChannel::from_named_feishu_config("feishu:primary".into(), &cfg);

    assert_eq!(ch.name(), "feishu:primary");
    assert_eq!(ch.account_id(), "primary");
}

#[test]
fn lark_from_named_feishu_config_normalizes_prefixed_account_identity() {
    use crate::config::schema::{FeishuConfig, LarkReceiveMode};

    let cfg = FeishuConfig {
        app_id: "cli_feishu_app123".into(),
        app_secret: "secret456".into(),
        enabled: None,
        encrypt_key: None,
        verification_token: Some("vtoken789".into()),
        allowed_users: vec!["*".into()],
        receive_mode: LarkReceiveMode::Webhook,
        port: Some(9898),
        media_max_mb: None,
        media_local_roots: Vec::new(),
    };

    let ch = LarkChannel::from_named_feishu_config(" Feishu:Primary ".into(), &cfg);

    assert_eq!(ch.name(), "feishu:Primary");
    assert_eq!(ch.account_id(), "Primary");
}

#[test]
fn lark_named_feishu_health_component_name_is_account_scoped() {
    use crate::config::schema::{FeishuConfig, LarkReceiveMode};

    let cfg = FeishuConfig {
        app_id: "cli_feishu_app123".into(),
        app_secret: "secret456".into(),
        enabled: None,
        encrypt_key: None,
        verification_token: Some("vtoken789".into()),
        allowed_users: vec!["*".into()],
        receive_mode: LarkReceiveMode::Webhook,
        port: Some(9898),
        media_max_mb: None,
        media_local_roots: Vec::new(),
    };

    let ch = LarkChannel::from_named_feishu_config("feishu:ops".into(), &cfg);

    assert_eq!(ch.health_component_name(), "channel:feishu:ops");
}

#[tokio::test]
async fn lark_named_feishu_instances_do_not_share_runtime_caches() {
    use crate::config::schema::{FeishuConfig, LarkReceiveMode};

    let cfg = FeishuConfig {
        app_id: "cli_feishu_app123".into(),
        app_secret: "secret456".into(),
        enabled: None,
        encrypt_key: None,
        verification_token: Some("vtoken789".into()),
        allowed_users: vec!["*".into()],
        receive_mode: LarkReceiveMode::Webhook,
        port: Some(9898),
        media_max_mb: None,
        media_local_roots: Vec::new(),
    };

    let primary = LarkChannel::from_named_feishu_config("feishu:primary".into(), &cfg);
    let ops = LarkChannel::from_named_feishu_config("feishu:ops".into(), &cfg);

    primary.set_resolved_bot_open_id(Some("ou_primary_bot".into()));
    {
        let mut cached = primary.tenant_token.write().await;
        *cached = Some(CachedTenantToken {
            value: "tenant_primary".into(),
            refresh_after: Instant::now() + Duration::from_secs(300),
        });
    }

    assert_eq!(
        primary.resolved_bot_open_id().as_deref(),
        Some("ou_primary_bot")
    );
    assert_eq!(ops.resolved_bot_open_id(), None);

    let primary_token = primary
        .tenant_token
        .read()
        .await
        .as_ref()
        .map(|token| token.value.clone());
    let ops_token = ops
        .tenant_token
        .read()
        .await
        .as_ref()
        .map(|token| token.value.clone());

    assert_eq!(primary_token.as_deref(), Some("tenant_primary"));
    assert_eq!(ops_token, None);
}

#[test]
fn lark_parse_inbound_message_preserves_thread_metadata_and_raw_content() {
    let ch = with_bot_open_id(
        LarkChannel::new(
            "cli_app123".into(),
            "secret456".into(),
            "token789".into(),
            None,
            vec!["ou_user".into()],
            false,
        ),
        "ou_bot",
    );
    let message = LarkMessage {
        message_id: "om_thread_1".into(),
        root_id: Some("om_root_1".into()),
        parent_id: Some("om_parent_1".into()),
        thread_id: Some("omt_thread_1".into()),
        create_time: Some("1700000000000".into()),
        chat_id: "oc_chat_1".into(),
        chat_type: "group".into(),
        message_type: "text".into(),
        content: "{\"text\":\"hello thread\"}".into(),
        mentions: Vec::new(),
    };

    let parsed = ch
        .parse_inbound_message("ou_user", &message)
        .expect("parsed inbound message");

    assert_eq!(parsed.root_id.as_deref(), Some("om_root_1"));
    assert_eq!(parsed.parent_id.as_deref(), Some("om_parent_1"));
    assert_eq!(parsed.thread_id.as_deref(), Some("omt_thread_1"));
    assert_eq!(parsed.raw_content, "{\"text\":\"hello thread\"}");
    assert_eq!(parsed.text, "hello thread");
    assert_eq!(parsed.normalized_content, "hello thread");
}

#[test]
fn lark_parse_event_payload_reply_without_thread_id_does_not_set_thread_ts() {
    let ch = with_bot_open_id(
        LarkChannel::new(
            "cli_app123".into(),
            "secret456".into(),
            "token789".into(),
            None,
            vec!["ou_user".into()],
            false,
        ),
        "ou_bot",
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_id": "om_reply_1",
                "root_id": "om_root_1",
                "parent_id": "om_parent_1",
                "message_type": "text",
                "content": "{\"text\":\"reply text\"}",
                "chat_id": "oc_chat_1",
                "chat_type": "group",
                "create_time": "1700000000000",
                "mentions": [{
                    "id": { "open_id": "ou_bot" }
                }]
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);

    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].thread_ts, None);
    assert_eq!(
        msgs[0]
            .context
            .as_ref()
            .and_then(|context| context.root_id.as_deref()),
        Some("om_root_1")
    );
}

#[test]
fn lark_parse_inbound_image_message_builds_official_normalized_content() {
    let ch = with_bot_open_id(
        LarkChannel::new(
            "cli_app123".into(),
            "secret456".into(),
            "token789".into(),
            None,
            vec!["ou_user".into()],
            false,
        ),
        "ou_bot",
    );
    let message = LarkMessage {
        message_id: "om_image_1".into(),
        root_id: None,
        parent_id: None,
        thread_id: None,
        create_time: Some("1700000000000".into()),
        chat_id: "oc_chat_1".into(),
        chat_type: "p2p".into(),
        message_type: "image".into(),
        content: "{\"image_key\":\"img_v3_123\"}".into(),
        mentions: Vec::new(),
    };

    let parsed = ch
        .parse_inbound_message("ou_user", &message)
        .expect("image should parse");

    assert_eq!(parsed.text, "");
    assert_eq!(parsed.normalized_content, "![image](img_v3_123)");
    assert_eq!(parsed.resources.len(), 1);
    assert_eq!(parsed.resources[0].file_key, "img_v3_123");
}

#[test]
fn lark_parse_inbound_post_message_builds_official_markdown_content() {
    let ch = with_bot_open_id(
        LarkChannel::new(
            "cli_app123".into(),
            "secret456".into(),
            "token789".into(),
            None,
            vec!["ou_user".into()],
            false,
        ),
        "ou_bot",
    );
    let message = LarkMessage {
        message_id: "om_post_1".into(),
        root_id: None,
        parent_id: None,
        thread_id: None,
        create_time: Some("1700000000000".into()),
        chat_id: "oc_chat_1".into(),
        chat_type: "p2p".into(),
        message_type: "post".into(),
        content: "{\"zh_cn\":{\"title\":\"日报\",\"content\":[[{\"tag\":\"text\",\"text\":\"查看 \"},{\"tag\":\"a\",\"text\":\"文档\",\"href\":\"https://example.com\"}],[{\"tag\":\"img\",\"image_key\":\"img_v3_post\"}]]}}".into(),
        mentions: Vec::new(),
    };

    let parsed = ch
        .parse_inbound_message("ou_user", &message)
        .expect("post should parse");

    assert_eq!(parsed.text, "日报\n\n查看 文档");
    assert_eq!(
        parsed.normalized_content,
        "**日报**\n\n查看 [文档](https://example.com)\n![image](img_v3_post)"
    );
    assert_eq!(parsed.resources.len(), 1);
    assert_eq!(parsed.resources[0].file_key, "img_v3_post");
}

#[test]
fn lark_parse_inbound_flat_post_message_builds_markdown_content() {
    let ch = with_bot_open_id(
        LarkChannel::new(
            "cli_app123".into(),
            "secret456".into(),
            "token789".into(),
            None,
            vec!["ou_user".into()],
            false,
        ),
        "ou_bot",
    );
    let message = LarkMessage {
        message_id: "om_post_flat_1".into(),
        root_id: None,
        parent_id: None,
        thread_id: None,
        create_time: Some("1700000000000".into()),
        chat_id: "oc_chat_1".into(),
        chat_type: "p2p".into(),
        message_type: "post".into(),
        content: "{\"title\":\"日报\",\"content\":[[{\"tag\":\"text\",\"text\":\"查看 \"},{\"tag\":\"img\",\"image_key\":\"img_v3_post\"}]]}".into(),
        mentions: Vec::new(),
    };

    let parsed = ch
        .parse_inbound_message("ou_user", &message)
        .expect("flat post should parse");

    assert_eq!(parsed.text, "日报\n\n查看");
    assert_eq!(
        parsed.normalized_content,
        "**日报**\n\n查看 ![image](img_v3_post)"
    );
    assert_eq!(parsed.resources.len(), 1);
    assert_eq!(parsed.resources[0].file_key, "img_v3_post");
}

#[tokio::test]
async fn lark_parse_event_payload_async_fetches_full_interactive_card_content() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth/v3/tenant_access_token/internal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "tenant_access_token": "tenant_token"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/bot/v3/info"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "bot": { "open_id": "ou_bot" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/im/v1/messages/om_interactive_1"))
        .and(query_param("user_id_type", "open_id"))
        .and(query_param("card_msg_content_type", "raw_card_content"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "items": [{
                    "body": {
                        "content": "{\"json_card\":\"{\\\"header\\\":{\\\"title\\\":{\\\"content\\\":\\\"审批\\\"}},\\\"elements\\\":[{\\\"tag\\\":\\\"markdown\\\",\\\"content\\\":\\\"请确认\\\"}]}\"}"
                    }
                }]
            }
        })))
        .mount(&server)
        .await;

    let mut ch = LarkChannel::from_feishu_config(&crate::config::schema::FeishuConfig {
        app_id: "cli_app123".into(),
        app_secret: "secret456".into(),
        enabled: None,
        encrypt_key: None,
        verification_token: Some("token789".into()),
        allowed_users: vec!["*".into()],
        receive_mode: crate::config::schema::LarkReceiveMode::Websocket,
        port: None,
        media_max_mb: None,
        media_local_roots: Vec::new(),
    });
    ch.api_base_override = Some(server.uri());

    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_id": "om_interactive_1",
                "message_type": "interactive",
                "content": "{\"type\":\"card\"}",
                "chat_id": "oc_chat_1",
                "chat_type": "p2p",
                "create_time": "1700000000000",
                "mentions": []
            }
        }
    });

    let msgs = ch.parse_event_payload_async(&payload).await;

    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "<card title=\"审批\">\n请确认\n</card>");
}

#[tokio::test]
async fn lark_parse_event_payload_async_downloads_image_resource_to_workspace() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/im/v1/messages/om_image_1/resources/img_v3_demo"))
        .and(query_param("type", "image"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "image/png")
                .append_header("content-disposition", "attachment; filename=\"photo.png\"")
                .set_body_raw(b"\x89PNG\r\n\x1a\nfake".to_vec(), "image/png"),
        )
        .mount(&server)
        .await;

    let workspace = TempDir::new().unwrap();
    let ch = with_cached_token(
        with_test_api_base(
            LarkChannel::new(
                "cli_app123".into(),
                "secret456".into(),
                "token789".into(),
                None,
                vec!["*".into()],
                true,
            )
            .with_workspace_dir(Some(workspace.path().to_path_buf())),
            &server.uri(),
        ),
        "tenant_token",
    )
    .await;

    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_id": "om_image_1",
                "message_type": "image",
                "content": "{\"image_key\":\"img_v3_demo\"}",
                "chat_id": "oc_chat_1",
                "chat_type": "p2p",
                "create_time": "1700000000000",
                "mentions": []
            }
        }
    });

    let msgs = ch.parse_event_payload_async(&payload).await;

    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].content.starts_with("[IMAGE:"));
    let path = msgs[0]
        .content
        .trim_start_matches("[IMAGE:")
        .trim_end_matches(']');
    assert!(
        std::path::Path::new(path).exists(),
        "downloaded image missing: {path}"
    );
}

#[tokio::test]
async fn lark_parse_event_payload_async_downloads_file_resource_to_workspace() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/im/v1/messages/om_file_1/resources/file_v3_demo"))
        .and(query_param("type", "file"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "application/pdf")
                .append_header("content-disposition", "attachment; filename=\"report.pdf\"")
                .set_body_raw(b"%PDF-1.4 demo".to_vec(), "application/pdf"),
        )
        .mount(&server)
        .await;

    let workspace = TempDir::new().unwrap();
    let ch = with_cached_token(
        with_test_api_base(
            LarkChannel::new(
                "cli_app123".into(),
                "secret456".into(),
                "token789".into(),
                None,
                vec!["*".into()],
                true,
            )
            .with_workspace_dir(Some(workspace.path().to_path_buf())),
            &server.uri(),
        ),
        "tenant_token",
    )
    .await;

    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_id": "om_file_1",
                "message_type": "file",
                "content": "{\"file_key\":\"file_v3_demo\",\"file_name\":\"report.pdf\"}",
                "chat_id": "oc_chat_1",
                "chat_type": "p2p",
                "create_time": "1700000000000",
                "mentions": []
            }
        }
    });

    let msgs = ch.parse_event_payload_async(&payload).await;

    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].content.starts_with("[DOCUMENT:"));
    let path = msgs[0]
        .content
        .trim_start_matches("[DOCUMENT:")
        .trim_end_matches(']');
    assert!(
        std::path::Path::new(path).exists(),
        "downloaded file missing: {path}"
    );
}

#[tokio::test]
async fn lark_parse_event_payload_async_includes_quoted_parent_message_content() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/im/v1/messages/om_parent_1"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "items": [{
                    "message_id": "om_parent_1",
                    "msg_type": "text",
                    "body": { "content": "{\"text\":\"原始消息\"}" }
                }]
            }
        })))
        .mount(&server)
        .await;

    let ch = with_cached_token(
        with_test_api_base(
            LarkChannel::new(
                "cli_app123".into(),
                "secret456".into(),
                "token789".into(),
                None,
                vec!["*".into()],
                true,
            ),
            &server.uri(),
        ),
        "tenant_token",
    )
    .await;

    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_id": "om_reply_1",
                "message_type": "text",
                "content": "{\"text\":\"这是回复内容\"}",
                "parent_id": "om_parent_1",
                "chat_id": "oc_chat_1",
                "chat_type": "p2p",
                "create_time": "1700000000000",
                "mentions": []
            }
        }
    });

    let msgs = ch.parse_event_payload_async(&payload).await;

    assert_eq!(msgs.len(), 1);
    assert!(
        msgs[0]
            .content
            .contains("[quoted message om_parent_1]\n原始消息"),
        "quoted content missing: {}",
        msgs[0].content
    );
    assert!(msgs[0].content.contains("这是回复内容"));
}

#[test]
fn lark_parse_interactive_message_reports_invalid_json_card_like_official_converter() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "interactive",
                "content": "{\"json_card\":\"{bad json\"}",
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "<card>\n[无法解析卡片内容]\n</card>");
}

#[test]
fn lark_parse_interactive_message_converts_official_concise_card_elements() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "interactive",
                "content": serde_json::json!({
                    "json_card": serde_json::json!({
                        "header": { "title": { "content": "复杂卡片" } },
                        "elements": [
                            { "tag": "note", "elements": [{ "tag": "plain_text", "content": "提示信息" }] },
                            { "tag": "actions", "actions": [
                                { "tag": "button", "text": { "content": "打开文档" }, "actions": [{ "type": "open_url", "action": { "url": "https://example.com/doc" } }] },
                                { "tag": "button", "text": { "content": "禁用按钮" }, "disabled": true }
                            ]},
                            { "tag": "select_static", "options": [
                                { "text": { "content": "选项A" }, "value": "a" },
                                { "text": { "content": "选项B" }, "value": "b" }
                            ], "initialOption": "a" },
                            { "tag": "input", "label": { "content": "备注" }, "placeholder": { "content": "请输入" } },
                            { "tag": "date_picker", "initialDate": "2026-03-16" },
                            { "tag": "checker", "checked": true, "text": { "content": "已确认" } },
                            { "tag": "link", "content": "更多信息", "url": { "url": "https://example.com/more" } },
                            { "tag": "emoji", "key": "OK" },
                            { "tag": "list", "items": [
                                { "type": "ul", "level": 0, "elements": [{ "tag": "text", "content": "第一项" }] },
                                { "type": "ol", "level": 1, "order": 2, "elements": [{ "tag": "text", "content": "第二项" }] }
                            ]},
                            { "tag": "blockquote", "content": "引用内容" },
                            { "tag": "code_block", "language": "rust", "contents": [
                                { "contents": [{ "content": "fn main() {}" }] }
                            ]},
                            { "tag": "heading", "level": 2, "content": "章节标题" },
                            { "tag": "interactive_container", "actions": [{ "type": "open_url", "action": { "url": "https://example.com/click" } }], "elements": [
                                { "tag": "markdown", "content": "点我跳转" }
                            ]},
                            { "tag": "collapsible_panel", "expanded": true, "header": { "title": { "content": "展开详情" } }, "elements": [
                                { "tag": "plain_text", "content": "内部说明" }
                            ]},
                            { "tag": "form", "elements": [
                                { "tag": "plain_text", "content": "表单内容" }
                            ]},
                            { "tag": "img", "title": { "content": "封面图" } },
                            { "tag": "audio", "fileID": "audio_key_1" },
                            { "tag": "video", "fileID": "video_key_1" }
                        ]
                    }).to_string()
                }).to_string(),
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].content,
        "<card title=\"复杂卡片\">\n📝 提示信息\n[打开文档](https://example.com/doc) [禁用按钮 ✗]\n{✓选项A / 选项B}\n备注: 请输入_____\n📅 2026-03-16\n[x] 已确认\n[更多信息](https://example.com/more)\n👌\n- 第一项\n  2. 第二项\n> 引用内容\n```rust\nfn main() {}\n```\n## 章节标题\n<clickable url=\"https://example.com/click\">\n点我跳转\n</clickable>\n▼ 展开详情\n    内部说明\n▲\n<form>\n表单内容\n</form>\n🖼️ 封面图\n🎵 音频\n🎬 视频\n</card>"
    );
}

#[test]
fn lark_parse_interactive_message_converts_attachment_backed_card_elements() {
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "interactive",
                "content": serde_json::json!({
                    "json_card": serde_json::json!({
                        "header": { "property": { "title": { "content": "附件卡片" } } },
                        "body": { "property": { "elements": [
                            { "tag": "person", "userID": "ou_person_1" },
                            { "tag": "at", "userID": "ou_at_1" },
                            { "tag": "local_datetime", "milliseconds": 1704067200000i64 },
                            { "tag": "number_tag", "text": { "content": "42" }, "url": { "url": "https://example.com/42" } },
                            { "tag": "text_tag", "text": { "content": "标签" } },
                            { "tag": "overflow", "options": [
                                { "text": { "content": "操作A" } },
                                { "text": { "content": "操作B" } }
                            ]},
                            { "tag": "img_combination", "imgList": [
                                { "imageID": "img1" },
                                { "imageID": "img2" }
                            ]},
                            { "tag": "fallback_text", "text": { "content": "兜底内容" } }
                        ]}}
                    }).to_string(),
                    "json_attachment": serde_json::json!({
                        "persons": {
                            "ou_person_1": { "content": "张三" }
                        },
                        "at_users": {
                            "ou_at_1": { "content": "李四", "user_id": "u_123" }
                        }
                    }).to_string()
                }).to_string(),
                "chat_id": "oc_chat"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].content,
        "<card title=\"附件卡片\">\n@张三\n@李四\n2024-01-01T00:00:00Z\n[42](https://example.com/42)\n「标签」\n⋮ 操作A, 操作B\n🖼️ 2张图片\n兜底内容\n</card>"
    );
}

#[tokio::test]
async fn lark_parse_event_payload_async_expands_merge_forward_messages() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/im/v1/messages/om_merge_forward_1"))
        .and(query_param("user_id_type", "open_id"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "items": [
                    {
                        "message_id": "om_sub_text_1",
                        "msg_type": "text",
                        "create_time": "1704067200000",
                        "sender": { "id": "ou_alice" },
                        "body": { "content": "{\"text\":\"第一条\"}" }
                    },
                    {
                        "message_id": "om_nested_forward_1",
                        "msg_type": "merge_forward",
                        "create_time": "1704067260000",
                        "sender": { "id": "ou_bob" },
                        "body": { "content": "{}" }
                    },
                    {
                        "message_id": "om_sub_share_user_1",
                        "upper_message_id": "om_nested_forward_1",
                        "msg_type": "share_user",
                        "create_time": "1704067320000",
                        "sender": { "id": "ou_cindy" },
                        "body": { "content": "{\"user_id\":\"ou_target\"}" }
                    }
                ]
            }
        })))
        .mount(&server)
        .await;

    let ch = with_cached_token(
        with_test_api_base(
            LarkChannel::new(
                "cli_app123".into(),
                "secret456".into(),
                "token789".into(),
                None,
                vec!["*".into()],
                true,
            ),
            &server.uri(),
        ),
        "tenant_token",
    )
    .await;

    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_id": "om_merge_forward_1",
                "message_type": "merge_forward",
                "content": "{}",
                "chat_id": "oc_chat_1",
                "chat_type": "p2p",
                "create_time": "1704067000000",
                "mentions": []
            }
        }
    });

    let msgs = ch.parse_event_payload_async(&payload).await;

    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].content,
        "<forwarded_messages>\n[2024-01-01T08:00:00+08:00] ou_alice:\n    第一条\n[2024-01-01T08:01:00+08:00] ou_bob:\n    <forwarded_messages>\n        [2024-01-01T08:02:00+08:00] ou_cindy:\n            <contact_card id=\"ou_target\"/>\n    </forwarded_messages>\n</forwarded_messages>"
    );
}

#[test]
fn lark_builds_dispatch_context_like_official_runtime() {
    let parsed = LarkParsedMessage {
        message_id: "om_thread_1".into(),
        chat_id: "oc_chat_1".into(),
        sender_open_id: "ou_user_1".into(),
        chat_type: "group".into(),
        message_type: "text".into(),
        raw_content: "{\"text\":\"hello\"}".into(),
        normalized_content: "hello".into(),
        create_time_secs: Some(1_700_000_000),
        root_id: Some("om_root_1".into()),
        parent_id: Some("om_parent_1".into()),
        thread_id: Some("omt_thread_1".into()),
        mentions: Vec::new(),
        text: "hello".into(),
        post_mentioned_open_ids: Vec::new(),
        resources: Vec::new(),
    };

    let dispatch = build_lark_dispatch_context(&parsed);

    assert!(dispatch.is_group);
    assert!(dispatch.is_thread);
    assert_eq!(dispatch.sender_id, "ou_user_1");
    assert_eq!(dispatch.feishu_from, "feishu:ou_user_1");
    assert_eq!(dispatch.feishu_to, "chat:oc_chat_1");
    assert_eq!(dispatch.envelope_from, "oc_chat_1:ou_user_1");
}

#[test]
fn lark_parse_fallback_sender_to_open_id() {
    // When chat_id is missing, sender should fall back to open_id
    let ch = LarkChannel::new(
        "id".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );
    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "text",
                "content": "{\"text\":\"hello\"}",
                "create_time": "1000"
            }
        }
    });

    let msgs = ch.parse_event_payload(&payload);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].sender, "ou_user");
}

#[test]
fn lark_parse_group_message_requires_bot_mention_when_enabled() {
    let ch = with_bot_open_id(
        LarkChannel::new(
            "cli_app123".into(),
            "secret".into(),
            "token".into(),
            None,
            vec!["*".into()],
            true,
        ),
        "ou_bot_123",
    );

    let no_mention_payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "text",
                "content": "{\"text\":\"hello\"}",
                "chat_type": "group",
                "chat_id": "oc_chat",
                "mentions": []
            }
        }
    });
    assert!(ch.parse_event_payload(&no_mention_payload).is_empty());

    let wrong_mention_payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "text",
                "content": "{\"text\":\"hello\"}",
                "chat_type": "group",
                "chat_id": "oc_chat",
                "mentions": [{ "id": { "open_id": "ou_other" } }]
            }
        }
    });
    assert!(ch.parse_event_payload(&wrong_mention_payload).is_empty());

    let bot_mention_payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "text",
                "content": "{\"text\":\"hello\"}",
                "chat_type": "group",
                "chat_id": "oc_chat",
                "mentions": [{ "id": { "open_id": "ou_bot_123" } }]
            }
        }
    });
    assert_eq!(ch.parse_event_payload(&bot_mention_payload).len(), 1);
}

#[test]
fn lark_parse_group_post_message_accepts_at_when_top_level_mentions_empty() {
    let ch = with_bot_open_id(
        LarkChannel::new(
            "cli_app123".into(),
            "secret".into(),
            "token".into(),
            None,
            vec!["*".into()],
            true,
        ),
        "ou_bot_123",
    );

    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "post",
                "chat_type": "group",
                "chat_id": "oc_chat",
                "mentions": [],
                "content": "{\"zh_cn\":{\"title\":\"\",\"content\":[[{\"tag\":\"at\",\"user_id\":\"ou_bot_123\",\"user_name\":\"Bot\"},{\"tag\":\"text\",\"text\":\" hi\"}]]}}"
            }
        }
    });

    assert_eq!(ch.parse_event_payload(&payload).len(), 1);
}

#[test]
fn lark_parse_group_message_allows_without_mention_when_disabled() {
    let ch = LarkChannel::new(
        "cli_app123".into(),
        "secret".into(),
        "token".into(),
        None,
        vec!["*".into()],
        false,
    );

    let payload = serde_json::json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_type": "text",
                "content": "{\"text\":\"hello\"}",
                "chat_type": "group",
                "chat_id": "oc_chat",
                "mentions": []
            }
        }
    });

    assert_eq!(ch.parse_event_payload(&payload).len(), 1);
}

#[test]
fn lark_reaction_url_matches_region() {
    let ch_lark = make_channel();
    assert_eq!(
        ch_lark.message_reaction_url("om_test_message_id"),
        "https://open.larksuite.com/open-apis/im/v1/messages/om_test_message_id/reactions"
    );

    let feishu_cfg = crate::config::schema::FeishuConfig {
        app_id: "cli_app123".into(),
        app_secret: "secret456".into(),
        enabled: None,
        encrypt_key: None,
        verification_token: Some("vtoken789".into()),
        allowed_users: vec!["*".into()],
        receive_mode: crate::config::schema::LarkReceiveMode::Webhook,
        port: Some(9898),
        media_max_mb: None,
        media_local_roots: Vec::new(),
    };
    let ch_feishu = LarkChannel::from_feishu_config(&feishu_cfg);
    assert_eq!(
        ch_feishu.message_reaction_url("om_test_message_id"),
        "https://open.feishu.cn/open-apis/im/v1/messages/om_test_message_id/reactions"
    );
}

#[test]
fn lark_reaction_locale_explicit_language_tags() {
    assert_eq!(map_locale_tag("zh-CN"), Some(LarkAckLocale::ZhCn));
    assert_eq!(map_locale_tag("zh_TW"), Some(LarkAckLocale::ZhTw));
    assert_eq!(map_locale_tag("zh-Hant"), Some(LarkAckLocale::ZhTw));
    assert_eq!(map_locale_tag("en-US"), Some(LarkAckLocale::En));
    assert_eq!(map_locale_tag("ja-JP"), Some(LarkAckLocale::Ja));
    assert_eq!(map_locale_tag("fr-FR"), None);
}

#[test]
fn lark_reaction_locale_prefers_explicit_payload_locale() {
    let payload = serde_json::json!({
        "sender": {
            "locale": "ja-JP"
        },
        "message": {
            "content": "{\"text\":\"hello\"}"
        }
    });
    assert_eq!(
        detect_lark_ack_locale(Some(&payload), "你好，世界"),
        LarkAckLocale::Ja
    );
}

#[test]
fn lark_reaction_locale_unsupported_payload_falls_back_to_text_script() {
    let payload = serde_json::json!({
        "sender": {
            "locale": "fr-FR"
        },
        "message": {
            "content": "{\"text\":\"頑張れ\"}"
        }
    });
    assert_eq!(
        detect_lark_ack_locale(Some(&payload), "頑張ってください"),
        LarkAckLocale::Ja
    );
}

#[test]
fn lark_reaction_locale_detects_simplified_and_traditional_text() {
    assert_eq!(
        detect_lark_ack_locale(None, "继续奋斗，今天很强"),
        LarkAckLocale::ZhCn
    );
    assert_eq!(
        detect_lark_ack_locale(None, "繼續奮鬥，今天很強"),
        LarkAckLocale::ZhTw
    );
}

#[test]
fn lark_reaction_locale_defaults_to_english_for_unsupported_text() {
    assert_eq!(
        detect_lark_ack_locale(None, "Bonjour tout le monde"),
        LarkAckLocale::En
    );
}

#[test]
fn random_lark_ack_reaction_respects_detected_locale_pool() {
    let payload = serde_json::json!({
        "sender": {
            "locale": "zh-CN"
        }
    });
    let selected = random_lark_ack_reaction(Some(&payload), "hello");
    assert!(LARK_ACK_REACTIONS_ZH_CN.contains(&selected));

    let payload = serde_json::json!({
        "sender": {
            "locale": "zh-TW"
        }
    });
    let selected = random_lark_ack_reaction(Some(&payload), "hello");
    assert!(LARK_ACK_REACTIONS_ZH_TW.contains(&selected));

    let payload = serde_json::json!({
        "sender": {
            "locale": "en-US"
        }
    });
    let selected = random_lark_ack_reaction(Some(&payload), "hello");
    assert!(LARK_ACK_REACTIONS_EN.contains(&selected));

    let payload = serde_json::json!({
        "sender": {
            "locale": "ja-JP"
        }
    });
    let selected = random_lark_ack_reaction(Some(&payload), "hello");
    assert!(LARK_ACK_REACTIONS_JA.contains(&selected));
}

#[tokio::test]
async fn lark_reaction_add_reaction_success_path() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages/om_message_1/reactions"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "reaction_id": "reaction_123"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel
        .add_reaction("oc_chat_1", "om_message_1", "THUMBSUP")
        .await
        .expect("add reaction should succeed");
}

#[tokio::test]
async fn lark_reaction_remove_reaction_success_path() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("GET"))
        .and(path("/im/v1/messages/om_message_1/reactions"))
        .and(header("authorization", "Bearer tenant_token"))
        .and(query_param("reaction_type", "OK"))
        .and(query_param("page_size", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "items": [
                    {
                        "reaction_id": "reaction_bot_1",
                        "reaction_type": { "emoji_type": "OK" },
                        "operator": { "operator_type": "app", "operator_id": "cli_test_app_id" }
                    },
                    {
                        "reaction_id": "reaction_user_1",
                        "reaction_type": { "emoji_type": "OK" },
                        "operator": { "operator_type": "user", "operator_id": "ou_user" }
                    }
                ],
                "page_token": "",
                "has_more": false
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("DELETE"))
        .and(path(
            "/im/v1/messages/om_message_1/reactions/reaction_bot_1",
        ))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel
        .remove_reaction("oc_chat_1", "om_message_1", "OK")
        .await
        .expect("remove reaction should delete bot-owned reactions");
}

#[tokio::test]
async fn lark_reaction_ack_uses_trait_level_add_reaction() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages/om_message_1/reactions"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "reaction_id": "reaction_ack_1"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel.try_add_ack_reaction("om_message_1", "OK").await;
}

#[tokio::test]
async fn lark_health_probe_reports_missing_credentials() {
    let channel = LarkChannel::new(
        "".into(),
        "".into(),
        "token".into(),
        None,
        vec!["*".into()],
        true,
    );

    let probe = channel.probe_health().await;

    assert_eq!(probe.config_status, LarkProbeStatus::Error);
    assert_eq!(probe.token_status, LarkProbeStatus::Skipped);
    assert!(probe.summary.contains("missing app_id or app_secret"));
}

#[tokio::test]
async fn lark_health_probe_reports_invalid_token_path() {
    let server = MockServer::start().await;
    let channel = with_test_api_base(make_channel(), &server.uri());

    Mock::given(method("POST"))
        .and(path("/auth/v3/tenant_access_token/internal"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "code": 99991663,
            "msg": "invalid tenant token"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let probe = channel.probe_health().await;

    assert_eq!(probe.config_status, LarkProbeStatus::Ok);
    assert_eq!(probe.token_status, LarkProbeStatus::Error);
    assert_eq!(probe.transport_status, LarkProbeStatus::Skipped);
    assert!(probe.summary.contains("tenant token fetch failed"));
}

#[tokio::test]
async fn lark_health_probe_reports_unresolved_bot_identity() {
    let server = MockServer::start().await;
    let channel = with_test_api_base(make_channel(), &server.uri());

    Mock::given(method("POST"))
        .and(path("/auth/v3/tenant_access_token/internal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "tenant_access_token": "tenant_token",
            "expire": 7200
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/callback/ws/endpoint"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "URL": "wss://example.com/ws",
                "ClientConfig": {
                    "PingInterval": 30
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/bot/v3/info"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let probe = channel.probe_health().await;

    assert_eq!(probe.token_status, LarkProbeStatus::Ok);
    assert_eq!(probe.transport_status, LarkProbeStatus::Ok);
    assert_eq!(probe.bot_identity_status, LarkProbeStatus::Error);
    assert!(probe.summary.contains("bot open_id missing"));
}

#[tokio::test]
async fn lark_send_draft_sends_interactive_card_and_returns_message_id() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "message_id": "om_draft_1"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let message_id = channel
        .send_draft(&SendMessage::new("draft body", "oc_chat123"))
        .await
        .expect("draft send should succeed");

    assert_eq!(message_id.as_deref(), Some("om_draft_1"));
}

#[tokio::test]
async fn lark_send_text_to_open_id_target_uses_open_id_receive_id_type_and_post_payload() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "open_id"))
        .and(header("authorization", "Bearer tenant_token"))
        .and(body_string_contains(r#""receive_id":"ou_user123""#))
        .and(body_string_contains(r#""msg_type":"post""#))
        .and(body_string_contains(r#"\"tag\":\"md\""#))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "message_id": "om_text_open_id_1"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel
        .send(&SendMessage::new("hello open id", "open_id:ou_user123"))
        .await
        .expect("send should succeed");
}

#[tokio::test]
async fn lark_send_text_to_chat_prefixed_target_uses_chat_id_receive_id_type() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(header("authorization", "Bearer tenant_token"))
        .and(body_string_contains(r#""receive_id":"oc_chat123""#))
        .and(body_string_contains(r#""msg_type":"post""#))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "message_id": "om_text_chat_1"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel
        .send(&SendMessage::new("hello chat", "chat:oc_chat123"))
        .await
        .expect("send should succeed");
}

#[tokio::test]
async fn lark_send_explicit_absolute_path_attachment_outside_default_roots_succeeds() {
    let repo_root = std::env::current_dir().unwrap();
    let outside = tempfile::tempdir_in(repo_root).unwrap();
    let file_path = outside.path().join("report.pdf");
    std::fs::write(&file_path, b"%PDF-1.4 fake").unwrap();

    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("POST"))
        .and(path("/im/v1/files"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "file_key": "file_explicit_path_1"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(header("authorization", "Bearer tenant_token"))
        .and(body_string_contains(r#""receive_id":"oc_chat123""#))
        .and(body_string_contains(r#""msg_type":"file""#))
        .and(body_string_contains(r#"\"file_key\":\"file_explicit_path_1\""#))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "message_id": "om_file_explicit_1"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel
        .send(&SendMessage::new(
            format!("[DOCUMENT:{}]", file_path.display()),
            "chat:oc_chat123",
        ))
        .await
        .expect("send should succeed");
}

#[tokio::test]
async fn lark_send_raw_card_json_routes_to_interactive_message_like_official_plugin() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(header("authorization", "Bearer tenant_token"))
        .and(body_string_contains(r#""msg_type":"interactive""#))
        .and(body_string_contains(r#"\"schema\":\"2.0\""#))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "message_id": "om_card_1"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel
        .send(&SendMessage::new(
            r#"{"schema":"2.0","body":{"elements":[{"tag":"markdown","content":"card body"}]}}"#,
            "oc_chat123",
        ))
        .await
        .expect("card send should succeed");
}

#[tokio::test]
async fn lark_update_draft_patches_interactive_card_message() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("PATCH"))
        .and(path("/im/v1/messages/om_draft_1"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {}
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel
        .update_draft("oc_chat123", "om_draft_1", "updated body")
        .await
        .expect("draft update should succeed");
}

#[tokio::test]
async fn lark_update_draft_throttles_and_flushes_latest_content() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "message_id": "om_draft_1"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("PATCH"))
        .and(path("/im/v1/messages/om_draft_1"))
        .and(header("authorization", "Bearer tenant_token"))
        .and(body_string_contains("second update"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let draft_id = channel
        .send_draft(&SendMessage::new("draft body", "oc_chat123"))
        .await
        .expect("draft send should succeed")
        .expect("draft message id should exist");

    channel
        .update_draft("oc_chat123", &draft_id, "first update")
        .await
        .expect("first draft update should succeed");
    channel
        .update_draft("oc_chat123", &draft_id, "second update")
        .await
        .expect("second draft update should succeed");

    tokio::time::sleep(std::time::Duration::from_millis(1_900)).await;
}

#[tokio::test]
async fn lark_finalize_draft_resends_attachment_instead_of_patching_marker_text() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;
    let workspace = TempDir::new().unwrap();
    let image_path = workspace.path().join("draft-final.png");
    std::fs::write(&image_path, b"fake-png-bytes").unwrap();

    Mock::given(method("DELETE"))
        .and(path("/im/v1/messages/om_draft_1"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/im/v1/images"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "image_key": "img_v3_draft_final"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .and(query_param("receive_id_type", "chat_id"))
        .and(header("authorization", "Bearer tenant_token"))
        .and(body_string_contains("img_v3_draft_final"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "data": {
                "message_id": "om_final_1"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel
        .finalize_draft(
            "oc_chat123",
            "om_draft_1",
            image_path.to_string_lossy().as_ref(),
        )
        .await
        .expect("draft finalize should resend attachment");
}

#[tokio::test]
async fn lark_cancel_draft_deletes_message() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("DELETE"))
        .and(path("/im/v1/messages/om_draft_1"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel
        .cancel_draft("oc_chat123", "om_draft_1")
        .await
        .expect("draft cancel should succeed");
}

#[tokio::test]
async fn lark_patch_message_short_circuits_after_terminal_message_code() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("PATCH"))
        .and(path("/im/v1/messages/om_draft_1"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 230011,
            "msg": "message recalled"
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel
        .patch_message_with_retry("om_draft_1", &serde_json::json!({"content":"{}"}))
        .await
        .expect("terminal message code should degrade to no-op");

    channel
        .patch_message_with_retry("om_draft_1", &serde_json::json!({"content":"{}"}))
        .await
        .expect("subsequent patch should short-circuit");
}

#[tokio::test]
async fn lark_delete_message_short_circuits_after_terminal_message_code() {
    let server = MockServer::start().await;
    let channel = with_cached_token(
        with_test_api_base(make_channel(), &server.uri()),
        "tenant_token",
    )
    .await;

    Mock::given(method("DELETE"))
        .and(path("/im/v1/messages/om_draft_1"))
        .and(header("authorization", "Bearer tenant_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 231003,
            "msg": "message deleted"
        })))
        .expect(1)
        .mount(&server)
        .await;

    channel
        .delete_message_with_retry("om_draft_1")
        .await
        .expect("terminal delete code should degrade to no-op");

    channel
        .delete_message_with_retry("om_draft_1")
        .await
        .expect("subsequent delete should short-circuit");
}
