# my-zeroclaw

这是基于上游 [zeroclaw-labs/zeroclaw](https://github.com/zeroclaw-labs/zeroclaw) 克隆后做的定制版。

上游原本那套通用介绍、安装说明、架构说明、完整配置参考、运维文档、硬件文档和贡献流程都还在上游仓库里，没必要在这里再抄一遍。

需要看原版资料，直接去这里：

- 上游仓库：[zeroclaw-labs/zeroclaw](https://github.com/zeroclaw-labs/zeroclaw)
- 上游英文 README：[README.en.md](https://github.com/zeroclaw-labs/zeroclaw/blob/master/README.en.md)
- 上游中文 README：[README.md](https://github.com/zeroclaw-labs/zeroclaw/blob/master/README.md)
- 上游文档目录：[docs/README.en.md](https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/README.en.md)
- 上游 channel 参考：[docs/reference/api/channels-reference.md](https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/reference/api/channels-reference.md)
- 上游 config 参考：[docs/reference/api/config-reference.md](https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/reference/api/config-reference.md)

## 这个 fork 改了什么

这份仓库不是“原版镜像”，而是偏本地自用和即时可用的魔改版，核心改动集中在通道、权限、身份和实际投递链路。

- 默认对接 OpenAI 兼容接口，当前本机配置可直接走 `https://right.codes/codex/v1`
- 飞书通道已打通，并支持 WebSocket 模式
- 企业微信智能机器人长连接通道已补上
- 飞书支持多账户：`[channels_config.feishu_accounts.<name>]`
- 企业微信支持多账户：`[channels_config.wecom_accounts.<name>]`
- 飞书和企业微信允许同时配置、同时运行
- 定时任务投递支持 `delivery.account`
- 企业微信定时投递支持区分单聊和群聊：`user:<userid>` / `group:<chatid>`
- 心跳投递支持命名账户：`feishu:<name>` / `wecom:<name>`
- 企业微信回消息链路已按长连接场景补齐，不再沿用不适配的旧配置查找逻辑
- 飞书图片发送能力已补
- 本地身份文件和人格设定已改成自定义风格，不走原版默认人格
- 本地安全/自治配置已偏放开，适合你这台机器的使用方式

## 当前建议的配置写法

即便你现在每个平台只有一个账号，也建议直接写成多账户结构，后续扩容不用迁移格式。

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

后续新增第二个账号时，直接继续加：

```toml
[channels_config.feishu_accounts.ops]
app_id = "cli_ops"
app_secret = "xxx"

[channels_config.wecom_accounts.ops]
bot_id = "aib_ops"
secret = "xxx"
```

## 这个 fork 现在更适合什么

- 本机长期跑 daemon
- 飞书和企业微信同时接入
- 多机器人账号并行
- 定时任务往不同聊天目标投递
- 高权限、本机自动化、少限制使用

## 不在这里重复维护的内容

下面这些内容以后默认以上游为准，这个仓库不重复写大而全版本：

- 通用安装教程
- 原版全部命令说明
- 原版全部 provider 说明
- 原版全部 memory / tool / runtime 说明
- 原版完整架构图和贡献规范

如果后续这个 fork 再加新能力，README 只记录“和上游相比新增了什么”，不再回退成百科全书。
