use crate::channels::{Channel, LarkChannel};
use crate::config::{Config, FeishuConfig};
use anyhow::Result;
use std::time::Duration;

#[derive(Debug, Clone)]
pub(crate) struct FeishuDiagnosticReport {
    pub(crate) summary: String,
    pub(crate) items: Vec<String>,
    targets: Vec<FeishuDiagnosticTarget>,
}

#[derive(Debug, Clone)]
struct FeishuDiagnosticTarget {
    display_name: String,
    account_name: String,
    source: FeishuConfigSource,
    config: FeishuConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeishuConfigSource {
    NativeDefault,
    NativeNamed,
    LegacyCompat,
}

pub(crate) fn collect_feishu_diagnostics(
    config: &Config,
    account: Option<&str>,
) -> Result<FeishuDiagnosticReport> {
    let targets = resolve_feishu_targets(config, account)?;
    let mut items = Vec::new();

    if targets.is_empty() {
        anyhow::bail!(
            "No Feishu configuration found. Configure [channels_config.feishu] or [channels_config.feishu_accounts.*]."
        );
    }

    for target in &targets {
        let source_label = match target.source {
            FeishuConfigSource::NativeDefault => "native-default",
            FeishuConfigSource::NativeNamed => "native-named",
            FeishuConfigSource::LegacyCompat => "legacy-lark-use-feishu",
        };
        items.push(format!(
            "{}: source={source_label}, receive_mode={}, allowed_users={}",
            target.display_name,
            receive_mode_label(&target.config),
            target.config.allowed_users.len()
        ));

        if target.source == FeishuConfigSource::LegacyCompat {
            items.push(format!(
                "{}: legacy compatibility path detected (`channels_config.lark.use_feishu=true`); migrate to `channels_config.feishu`.",
                target.display_name
            ));
        }

        if target.config.receive_mode == crate::config::schema::LarkReceiveMode::Webhook {
            if target
                .config
                .verification_token
                .as_deref()
                .is_none_or(str::is_empty)
            {
                items.push(format!(
                    "{}: webhook mode missing verification_token",
                    target.display_name
                ));
            }
            if target
                .config
                .encrypt_key
                .as_deref()
                .is_none_or(str::is_empty)
            {
                items.push(format!(
                    "{}: webhook mode missing encrypt_key",
                    target.display_name
                ));
            }
        }

        if target.config.allowed_users.is_empty() {
            items.push(format!(
                "{}: allowed_users is empty; all inbound traffic will be denied until users are added",
                target.display_name
            ));
        } else if target.config.allowed_users.iter().any(|entry| entry == "*") {
            items.push(format!(
                "{}: allowed_users contains `*`; review whether wildcard access is intended",
                target.display_name
            ));
        }
    }

    let summary = if targets.len() == 1 {
        format!("Feishu diagnostics for {}", targets[0].display_name)
    } else {
        format!(
            "Feishu diagnostics for {} account(s): {}",
            targets.len(),
            targets
                .iter()
                .map(|target| target.account_name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    Ok(FeishuDiagnosticReport {
        summary,
        items,
        targets,
    })
}

pub(crate) async fn run_feishu(config: &Config, account: Option<&str>) -> Result<()> {
    let report = collect_feishu_diagnostics(config, account)?;

    println!("🩺 ZeroClaw Doctor — Feishu");
    println!("{}", report.summary);
    println!();

    for item in &report.items {
        println!("  - {item}");
    }

    println!();
    println!("  Live health checks:");
    for target in &report.targets {
        let channel = build_feishu_channel(target, config);
        let result = tokio::time::timeout(Duration::from_secs(10), channel.health_check()).await;
        let status = match result {
            Ok(true) => "healthy",
            Ok(false) => {
                println!(
                    "  - {}: unhealthy (health check returned false)",
                    target.display_name
                );
                continue;
            }
            Err(_) => {
                println!("  - {}: timed out (>10s)", target.display_name);
                continue;
            }
        };
        println!("  - {}: {}", target.display_name, status);
    }

    Ok(())
}

fn resolve_feishu_targets(
    config: &Config,
    account: Option<&str>,
) -> Result<Vec<FeishuDiagnosticTarget>> {
    let requested =
        account.and_then(crate::config::ChannelsConfig::normalize_feishu_account_reference);
    let Some(requested) = requested else {
        return Ok(collect_all_feishu_targets(config));
    };

    if let Some((_, name, named)) = config
        .channels_config
        .resolve_feishu_account_reference(Some(&requested))
    {
        return Ok(vec![FeishuDiagnosticTarget {
            display_name: format!("Feishu[{name}]"),
            account_name: name.to_string(),
            source: if name == "default" {
                FeishuConfigSource::NativeDefault
            } else {
                FeishuConfigSource::NativeNamed
            },
            config: named.clone(),
        }]);
    }

    if requested.eq_ignore_ascii_case("default") || requested.eq_ignore_ascii_case("feishu") {
        if let Some(lark) = config
            .channels_config
            .lark
            .as_ref()
            .filter(|cfg| cfg.use_feishu)
        {
            return Ok(vec![FeishuDiagnosticTarget {
                display_name: "Feishu[legacy]".to_string(),
                account_name: "legacy".to_string(),
                source: FeishuConfigSource::LegacyCompat,
                config: FeishuConfig {
                    app_id: lark.app_id.clone(),
                    app_secret: lark.app_secret.clone(),
                    enabled: None,
                    encrypt_key: lark.encrypt_key.clone(),
                    verification_token: lark.verification_token.clone(),
                    allowed_users: lark.allowed_users.clone(),
                    receive_mode: lark.receive_mode.clone(),
                    port: lark.port,
                },
            }]);
        }
    }

    anyhow::bail!("Feishu account not found: {requested}");
}

fn collect_all_feishu_targets(config: &Config) -> Vec<FeishuDiagnosticTarget> {
    let mut targets = Vec::new();

    for account_id in config.channels_config.configured_feishu_account_ids() {
        let Some((_, name, feishu)) = config
            .channels_config
            .resolve_feishu_account_reference(Some(&account_id))
        else {
            continue;
        };
        targets.push(FeishuDiagnosticTarget {
            display_name: format!("Feishu[{name}]"),
            account_name: name.clone(),
            source: if name == "default" {
                FeishuConfigSource::NativeDefault
            } else {
                FeishuConfigSource::NativeNamed
            },
            config: feishu.clone(),
        });
    }

    if targets.is_empty() {
        if let Some(lark) = config
            .channels_config
            .lark
            .as_ref()
            .filter(|cfg| cfg.use_feishu)
        {
            targets.push(FeishuDiagnosticTarget {
                display_name: "Feishu[legacy]".to_string(),
                account_name: "legacy".to_string(),
                source: FeishuConfigSource::LegacyCompat,
                config: FeishuConfig {
                    app_id: lark.app_id.clone(),
                    app_secret: lark.app_secret.clone(),
                    enabled: None,
                    encrypt_key: lark.encrypt_key.clone(),
                    verification_token: lark.verification_token.clone(),
                    allowed_users: lark.allowed_users.clone(),
                    receive_mode: lark.receive_mode.clone(),
                    port: lark.port,
                },
            });
        }
    }

    targets
}

fn build_feishu_channel(target: &FeishuDiagnosticTarget, config: &Config) -> LarkChannel {
    let channel = match target.source {
        FeishuConfigSource::NativeDefault | FeishuConfigSource::LegacyCompat => {
            LarkChannel::from_feishu_config(&target.config)
        }
        FeishuConfigSource::NativeNamed => LarkChannel::from_named_feishu_config(
            format!("feishu:{}", target.account_name),
            &target.config,
        ),
    };

    channel.with_workspace_dir(Some(config.workspace_dir.clone()))
}

fn receive_mode_label(config: &FeishuConfig) -> &'static str {
    match config.receive_mode {
        crate::config::schema::LarkReceiveMode::Websocket => "websocket",
        crate::config::schema::LarkReceiveMode::Webhook => "webhook",
    }
}

#[cfg(test)]
mod tests {
    use super::collect_feishu_diagnostics;
    use crate::config::{schema::LarkReceiveMode, Config, FeishuConfig, LarkConfig};

    #[test]
    fn collect_feishu_diagnostics_supports_default_aliases_and_wildcard_warning() {
        let mut config = Config::default();
        config.channels_config.feishu = Some(FeishuConfig {
            app_id: "cli_default".into(),
            app_secret: "secret".into(),
            enabled: None,
            encrypt_key: Some("encrypt".into()),
            verification_token: Some("verify".into()),
            allowed_users: vec!["*".into()],
            receive_mode: LarkReceiveMode::Websocket,
            port: None,
        });

        for alias in ["default", "feishu"] {
            let report = collect_feishu_diagnostics(&config, Some(alias)).expect("report");

            assert!(report.summary.contains("Feishu[default]"));
            assert!(report
                .items
                .iter()
                .any(|item| item.contains("allowed_users contains `*`")));
        }
    }

    #[test]
    fn collect_feishu_diagnostics_errors_when_no_feishu_config_exists() {
        let config = Config::default();
        let err = collect_feishu_diagnostics(&config, None).expect_err("missing config");

        assert!(err.to_string().contains("No Feishu configuration found"));
    }

    #[test]
    fn collect_feishu_diagnostics_reports_legacy_compat_config() {
        let mut config = Config::default();
        config.channels_config.lark = Some(LarkConfig {
            app_id: "cli_legacy".into(),
            app_secret: "secret".into(),
            verification_token: Some("verify".into()),
            encrypt_key: Some("encrypt".into()),
            allowed_users: vec!["ou_1".into()],
            mention_only: false,
            use_feishu: true,
            receive_mode: LarkReceiveMode::Webhook,
            port: Some(8080),
        });

        let report = collect_feishu_diagnostics(&config, None).expect("report");

        assert!(report.summary.contains("legacy"));
        assert!(report
            .items
            .iter()
            .any(|item| item.contains("lark.use_feishu=true")));
    }

    #[test]
    fn collect_feishu_diagnostics_reports_named_account_and_webhook_gaps() {
        let mut config = Config::default();
        config.channels_config.feishu_accounts.insert(
            "ops".into(),
            FeishuConfig {
                app_id: "cli_ops".into(),
                app_secret: "ops-secret".into(),
                enabled: None,
                encrypt_key: None,
                verification_token: None,
                allowed_users: vec!["ou_ops".into()],
                receive_mode: LarkReceiveMode::Webhook,
                port: Some(8081),
            },
        );

        let report = collect_feishu_diagnostics(&config, Some("ops")).expect("report");

        assert!(report.summary.contains("ops"));
        assert!(report
            .items
            .iter()
            .any(|item| item.contains("verification_token")));
        assert!(report.items.iter().any(|item| item.contains("encrypt_key")));
    }

    #[test]
    fn collect_feishu_diagnostics_normalizes_prefixed_named_account_lookup() {
        let mut config = Config::default();
        config.channels_config.feishu_accounts.insert(
            "ops".into(),
            FeishuConfig {
                app_id: "cli_ops".into(),
                app_secret: "ops-secret".into(),
                enabled: None,
                encrypt_key: Some("encrypt".into()),
                verification_token: Some("verify".into()),
                allowed_users: vec!["ou_ops".into()],
                receive_mode: LarkReceiveMode::Websocket,
                port: None,
            },
        );

        let report = collect_feishu_diagnostics(&config, Some(" Feishu:ops ")).expect("report");

        assert!(report.summary.contains("ops"));
        assert!(report
            .items
            .iter()
            .any(|item| item.contains("source=native-named")));
    }
}
