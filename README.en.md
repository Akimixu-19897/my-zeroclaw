# my-zeroclaw

This repository is a customized fork of [zeroclaw-labs/zeroclaw](https://github.com/zeroclaw-labs/zeroclaw).

The original ZeroClaw project already maintains the broad product overview, installation guides, architecture notes, full configuration reference, operations docs, hardware docs, and contribution workflow. Duplicating all of that here is noise.

Use upstream for the original material:

- Upstream repository: [zeroclaw-labs/zeroclaw](https://github.com/zeroclaw-labs/zeroclaw)
- Upstream English README: [README.en.md](https://github.com/zeroclaw-labs/zeroclaw/blob/master/README.en.md)
- Upstream Chinese README: [README.md](https://github.com/zeroclaw-labs/zeroclaw/blob/master/README.md)
- Upstream docs hub: [docs/README.en.md](https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/README.en.md)
- Upstream channels reference: [docs/reference/api/channels-reference.md](https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/reference/api/channels-reference.md)
- Upstream config reference: [docs/reference/api/config-reference.md](https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/reference/api/config-reference.md)

## What this fork changes

This is not a mirror. It is a pragmatic local-use fork focused on channel integrations, runtime permissions, identity tuning, and message delivery behavior.

- Default OpenAI-compatible endpoint wiring for `https://right.codes/codex/v1`
- Feishu channel support with WebSocket mode
- WeCom intelligent bot long-connection channel support
- Multi-account Feishu support via `[channels_config.feishu_accounts.<name>]`
- Multi-account WeCom support via `[channels_config.wecom_accounts.<name>]`
- Feishu and WeCom can be configured and run at the same time
- Cron delivery supports `delivery.account`
- WeCom cron delivery uses conversation targets: `chat:<chatid>` / `group:<chatid>`
- Heartbeat delivery supports named accounts: `feishu:<name>` / `wecom:<name>`
- WeCom reply delivery path adjusted for long-connection runtime behavior
- Feishu image upload/send support added
- Local identity/persona files customized away from upstream defaults
- Local autonomy/security config tuned for high-permission workstation use

## Recommended config shape

Even if you currently have only one account per platform, keep the config in multi-account form so future expansion does not require migration.

```toml
[channels_config.feishu_accounts.primary]
app_id = "cli_xxx"
app_secret = "xxx"
allowed_users = ["*"]
receive_mode = "websocket"

[channels_config.wecom_accounts.primary]
bot_id = "aib_xxx"
secret = "xxx"
websocket_url = "wss://openws.work.weixin.qq.com"
allowed_users = ["*"]
```

Add more accounts later like this:

```toml
[channels_config.feishu_accounts.ops]
app_id = "cli_ops"
app_secret = "xxx"

[channels_config.wecom_accounts.ops]
bot_id = "aib_ops"
secret = "xxx"
```

## What this fork is for

- Running a long-lived local daemon
- Feishu and WeCom together
- Multiple bot accounts in parallel
- Cron delivery to different chat targets
- High-permission workstation automation

## What stays upstream

This fork does not try to keep a second full copy of upstream documentation for:

- General installation
- Full command reference
- Full provider reference
- Complete memory/tool/runtime documentation
- Full architecture and contribution docs

This README should only describe what is materially different in this fork.
