#[test]
fn extract_tool_context_summary_collects_alias_and_native_tool_calls() {
    let history = vec![
        ChatMessage::system("sys"),
        ChatMessage::assistant(
            r#"<toolcall>
{"name":"shell","arguments":{"command":"date"}}
</toolcall>"#,
        ),
        ChatMessage::assistant(
            r#"{"content":null,"tool_calls":[{"id":"1","name":"web_search","arguments":"{}"}]}"#,
        ),
    ];

    let summary = extract_tool_context_summary(&history, 1);
    assert_eq!(summary, "[Used tools: shell, web_search]");
}

#[test]
fn build_channel_system_prompt_includes_feishu_attachment_marker_guidance() {
    let prompt = build_channel_system_prompt("base", "feishu", "oc_chat123");

    assert!(
        prompt.contains("When responding on Feishu:"),
        "feishu channel instructions should be embedded into the system prompt"
    );
    assert!(
        prompt.contains("[IMAGE:<absolute-path-or-url>]"),
        "feishu image marker guidance should be present"
    );
    assert!(
        prompt.contains("[DOCUMENT:<absolute-path>]"),
        "feishu document marker guidance should be present"
    );
    assert!(
        prompt.contains("Do not write ad-hoc upload scripts"),
        "agent should be explicitly told not to create workaround scripts"
    );
}

#[test]
fn build_channel_system_prompt_includes_named_feishu_attachment_marker_guidance() {
    let prompt = build_channel_system_prompt("base", "feishu:primary", "oc_chat123");

    assert!(
        prompt.contains("When responding on Feishu:"),
        "named feishu channels should still receive feishu attachment guidance"
    );
    assert!(prompt.contains("[DOCUMENT:<absolute-path>]"));
}

#[test]
fn extract_tool_context_summary_collects_prompt_mode_tool_result_names() {
    let history = vec![
        ChatMessage::system("sys"),
        ChatMessage::assistant("Using markdown tool call fence"),
        ChatMessage::user(
            r#"[Tool results]
<tool_result name="http_request">
{"status":200}
</tool_result>
<tool_result name="shell">
Mon Feb 20
</tool_result>"#,
        ),
    ];

    let summary = extract_tool_context_summary(&history, 1);
    assert_eq!(summary, "[Used tools: http_request, shell]");
}

#[test]
fn extract_tool_context_summary_respects_start_index() {
    let history = vec![
        ChatMessage::assistant(
            r#"<tool_call>
{"name":"stale_tool","arguments":{}}
</tool_call>"#,
        ),
        ChatMessage::assistant(
            r#"<tool_call>
{"name":"fresh_tool","arguments":{}}
</tool_call>"#,
        ),
    ];

    let summary = extract_tool_context_summary(&history, 1);
    assert_eq!(summary, "[Used tools: fresh_tool]");
}

#[test]
fn strip_isolated_tool_json_artifacts_removes_tool_calls_and_results() {
    let mut known_tools = HashSet::new();
    known_tools.insert("schedule".to_string());

    let input = r#"{"name":"schedule","parameters":{"action":"create","message":"test"}}
{"name":"schedule","parameters":{"action":"cancel","task_id":"test"}}
Let me create the reminder properly:
{"name":"schedule","parameters":{"action":"create","message":"Go to sleep"}}
{"result":{"task_id":"abc","status":"scheduled"}}
Done reminder set for 1:38 AM."#;

    let result = strip_isolated_tool_json_artifacts(input, &known_tools);
    let normalized = result
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        normalized,
        "Let me create the reminder properly:\nDone reminder set for 1:38 AM."
    );
}

#[test]
fn strip_isolated_tool_json_artifacts_preserves_non_tool_json() {
    let mut known_tools = HashSet::new();
    known_tools.insert("shell".to_string());

    let input = r#"{"name":"profile","parameters":{"timezone":"UTC"}}
This is an example JSON object for profile settings."#;

    let result = strip_isolated_tool_json_artifacts(input, &known_tools);
    assert_eq!(result, input);
}

// ── AIEOS Identity Tests (Issue #168) ─────────────────────────

#[test]
fn aieos_identity_from_file() {
    use crate::config::IdentityConfig;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let identity_path = tmp.path().join("aieos_identity.json");

    // Write AIEOS identity file
    let aieos_json = r#"{
            "identity": {
                "names": {"first": "Nova", "nickname": "Nov"},
                "bio": "A helpful AI assistant.",
                "origin": "Silicon Valley"
            },
            "psychology": {
                "mbti": "INTJ",
                "moral_compass": ["Be helpful", "Do no harm"]
            },
            "linguistics": {
                "style": "concise",
                "formality": "casual"
            }
        }"#;
    std::fs::write(&identity_path, aieos_json).unwrap();

    // Create identity config pointing to the file
    let config = IdentityConfig {
        format: "aieos".into(),
        aieos_path: Some("aieos_identity.json".into()),
        aieos_inline: None,
    };

    let prompt = build_system_prompt(tmp.path(), "model", &[], &[], Some(&config), None);

    // Should contain AIEOS sections
    assert!(prompt.contains("## Identity"));
    assert!(prompt.contains("**Name:** Nova"));
    assert!(prompt.contains("**Nickname:** Nov"));
    assert!(prompt.contains("**Bio:** A helpful AI assistant."));
    assert!(prompt.contains("**Origin:** Silicon Valley"));

    assert!(prompt.contains("## Personality"));
    assert!(prompt.contains("**MBTI:** INTJ"));
    assert!(prompt.contains("**Moral Compass:**"));
    assert!(prompt.contains("- Be helpful"));

    assert!(prompt.contains("## Communication Style"));
    assert!(prompt.contains("**Style:** concise"));
    assert!(prompt.contains("**Formality Level:** casual"));

    // Should NOT contain OpenClaw bootstrap file headers
    assert!(!prompt.contains("### SOUL.md"));
    assert!(!prompt.contains("### IDENTITY.md"));
    assert!(!prompt.contains("[File not found"));
}

#[test]
fn aieos_identity_from_inline() {
    use crate::config::IdentityConfig;

    let config = IdentityConfig {
        format: "aieos".into(),
        aieos_path: None,
        aieos_inline: Some(r#"{"identity":{"names":{"first":"Claw"}}}"#.into()),
    };

    let prompt = build_system_prompt(
        std::env::temp_dir().as_path(),
        "model",
        &[],
        &[],
        Some(&config),
        None,
    );

    assert!(prompt.contains("**Name:** Claw"));
    assert!(prompt.contains("## Identity"));
}

#[test]
fn aieos_fallback_to_openclaw_on_parse_error() {
    use crate::config::IdentityConfig;

    let config = IdentityConfig {
        format: "aieos".into(),
        aieos_path: Some("nonexistent.json".into()),
        aieos_inline: None,
    };

    let ws = make_workspace();
    let prompt = build_system_prompt(ws.path(), "model", &[], &[], Some(&config), None);

    // Should fall back to OpenClaw format when AIEOS file is not found
    // (Error is logged to stderr with filename, not included in prompt)
    assert!(prompt.contains("### SOUL.md"));
}

#[test]
fn aieos_empty_uses_openclaw() {
    use crate::config::IdentityConfig;

    // Format is "aieos" but neither path nor inline is set
    let config = IdentityConfig {
        format: "aieos".into(),
        aieos_path: None,
        aieos_inline: None,
    };

    let ws = make_workspace();
    let prompt = build_system_prompt(ws.path(), "model", &[], &[], Some(&config), None);

    // Should use OpenClaw format (not configured for AIEOS)
    assert!(prompt.contains("### SOUL.md"));
    assert!(prompt.contains("Be helpful"));
}

#[test]
fn openclaw_format_uses_bootstrap_files() {
    use crate::config::IdentityConfig;

    let config = IdentityConfig {
        format: "openclaw".into(),
        aieos_path: Some("identity.json".into()),
        aieos_inline: None,
    };

    let ws = make_workspace();
    let prompt = build_system_prompt(ws.path(), "model", &[], &[], Some(&config), None);

    // Should use OpenClaw format even if aieos_path is set
    assert!(prompt.contains("### SOUL.md"));
    assert!(prompt.contains("Be helpful"));
    assert!(!prompt.contains("## Identity"));
}

#[test]
fn none_identity_config_uses_openclaw() {
    let ws = make_workspace();
    // Pass None for identity config
    let prompt = build_system_prompt(ws.path(), "model", &[], &[], None, None);

    // Should use OpenClaw format
    assert!(prompt.contains("### SOUL.md"));
    assert!(prompt.contains("Be helpful"));
}

#[test]
fn classify_health_ok_true() {
    let state = classify_health_result(&Ok(true));
    assert_eq!(state, ChannelHealthState::Healthy);
}

#[test]
fn channel_health_detail_suffix_formats_lark_probe_details() {
    let component_name = format!("channel:test-detail-{}", uuid::Uuid::new_v4());
    crate::health::set_component_details(
        &component_name,
        serde_json::json!({
            "probe_kind": "lark_channel",
            "account_id": "primary",
            "receive_mode": "websocket",
            "token_status": "ok",
            "transport_status": "error",
            "bot_identity_status": "ok"
        }),
    );

    let suffix = channel_health_detail_suffix(&component_name);

    assert!(suffix.contains("account=primary"));
    assert!(suffix.contains("mode=websocket"));
    assert!(suffix.contains("token=ok"));
    assert!(suffix.contains("transport=error"));
    assert!(suffix.contains("bot=ok"));
}

#[test]
fn channel_health_detail_suffix_ignores_non_lark_probe_details() {
    let component_name = format!("channel:test-detail-{}", uuid::Uuid::new_v4());
    crate::health::set_component_details(
        &component_name,
        serde_json::json!({
            "probe_kind": "other_probe",
            "status": "ok"
        }),
    );

    assert!(channel_health_detail_suffix(&component_name).is_empty());
}

#[test]
fn classify_health_ok_false() {
    let state = classify_health_result(&Ok(false));
    assert_eq!(state, ChannelHealthState::Unhealthy);
}

