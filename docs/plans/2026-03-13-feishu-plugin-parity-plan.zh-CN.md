# Feishu 插件完整能力对齐实施计划

> **给 Claude：** 必须使用 `superpowers:executing-plans` 子技能，按任务逐步执行本计划。

**目标：** 在 Rust 实现的 `zeroclaw` 中复刻官方 `@larksuite/openclaw-lark` 插件的功能集合，并为每个阶段设置明确的能力对齐检查点与可安全推进的分阶段交付路径。

**架构：** 保留现有原生 Rust channel 模型作为宿主运行时，然后在其上补齐三层 Feishu/Lark 能力对齐层：channel/runtime 行为层、富消息与媒体行为层、以及 tool/OAuth/管理能力层。优先复用现有扩展点 `src/channels/lark.rs`、`src/channels/mod.rs`、`src/config/schema.rs`、`src/tools/*`、`src/onboard/*`、`src/doctor/*`、`src/security/*`，而不是引入 Node bridge。

**技术栈：** Rust、Tokio、reqwest、serde/serde_json、现有 ZeroClaw channel/tool/runtime 抽象、Feishu/Lark Open Platform HTTP/WebSocket APIs。

---

## 范围基线

2026-03-13 实际审计到的官方参考制品：

- 运行时插件包 id：`@larksuite/openclaw-lark`
- 安装 / doctor CLI 包：`@larksuite/openclaw-lark-tools@1.0.21`
- 本地审计来源：`/Users/a80531901/Desktop/projects/openclaw-lark-tools`

重要修正：

- 当前下载到的公开制品，是官方 Node 插件的 onboarding/doctor CLI，而不是完整的消息运行时源码树。
- 因此，Phase 4.x 的能力对齐工作，必须把官方 CLI 作为安装、迁移、更新、诊断流程的事实基线。
- 消息层 / 运行时层的能力对齐，仍然以官方 `@larksuite/openclaw-lark` 行为为目标；但凡是超出当前已下载 CLI 之外的精确模块命名，在拿到运行时包源码前，都应视为“推断”而不是“已证实事实”。

官方主要模块分组：

- `src/channel/*`
- `src/core/*`
- `src/messaging/inbound/*`
- `src/messaging/outbound/*`
- `src/card/*`
- `src/tools/*`
- `src/commands/*`

本地 Rust 主要落点：

- `src/channels/lark.rs`
- `src/channels/mod.rs`
- `src/channels/traits.rs`
- `src/config/schema.rs`
- `src/tools/*`
- `src/onboard/*`
- `src/doctor/*`
- `src/security/*`

## 对齐矩阵

### A. Channel Core

官方模块：

- `src/channel/plugin.js`
- `src/channel/event-handlers.js`
- `src/channel/chat-queue.js`
- `src/channel/monitor.js`
- `src/channel/onboarding*.js`
- `src/core/accounts.js`
- `src/core/config-schema.js`
- `src/core/lark-client.js`
- `src/core/token-store.js`

本地映射：

- `src/channels/lark.rs`
- `src/channels/mod.rs`
- `src/config/schema.rs`
- `src/onboard/mod.rs`
- `src/onboard/wizard.rs`
- `src/doctor/mod.rs`

当前状态：

- 部分完成。
- 我们已经具备原生的 Lark/Feishu channel 配置、allowlist、webhook/websocket 接收模式、tenant token 刷新、具名 Feishu 账号和基础健康检查行为。
- 但还没有官方风格的账号目录管理、onboarding 迁移流程、channel monitor/diagnose 对齐，或插件风格的生命周期钩子。

优先级：P0

缺口总结：

- 没有与官方插件账号模型一致的显式多账号目录 / 运行时模型。
- 没有官方风格的 onboarding 状态机与迁移路径。
- 没有专门面向 Feishu 的 channel monitor/probe/doctor 对等实现。
- 没有一层清晰隔离 legacy 配置形态与当前配置形态的 config adapter。

### B. Messaging Inbound

官方模块：

- `src/messaging/inbound/parse.js`
- `src/messaging/inbound/handler.js`
- `src/messaging/inbound/dispatch*.js`
- `src/messaging/inbound/media-resolver.js`
- `src/messaging/inbound/policy.js`
- `src/messaging/inbound/permission.js`
- `src/messaging/inbound/reaction-handler.js`
- `src/messaging/inbound/user-name-cache.js`
- `src/messaging/converters/*`

本地映射：

- `src/channels/lark.rs`
- `src/channels/traits.rs`
- `src/agent/loop_.rs`

当前状态：

- 完成度较低。
- 我们能处理 text/post 接收、mention gating、websocket 模式下去重、以及自动 ack reaction。
- 但尚未解析大多数富消息类型。
- 也还不会下载入站图片 / 文件 / 音频 / 视频资源。
- 同时也没有保留官方风格的 message context 结构、dispatch context 和 permission gate 效果。

优先级：P0

缺口总结：

- 缺少 image、file、audio、video、system、sticker、location、share、vote、todo、merge-forward、interactive card 等转换器。
- 缺少结构化媒体 payload 注入能力。
- 缺少入站 reply/thread 元数据保真。
- 缺少 reaction 事件接入。
- 缺少更丰富的策略与权限约束模型。

### C. Messaging Outbound

官方模块：

- `src/messaging/outbound/actions.js`
- `src/messaging/outbound/deliver.js`
- `src/messaging/outbound/media.js`
- `src/messaging/outbound/media-url-utils.js`
- `src/messaging/outbound/send.js`
- `src/messaging/outbound/reactions.js`
- `src/messaging/outbound/forward.js`
- `src/messaging/outbound/typing.js`
- `src/messaging/outbound/chat-manage.js`
- `src/core/targets.js`

本地映射：

- `src/channels/lark.rs`
- `src/channels/traits.rs`
- `src/channels/mod.rs`

当前状态：

- 部分完成。
- 我们支持纯文本发送、图片上传/发送、文件上传/发送，以及为消息确认添加 reaction。
- 我们会引导 agent 使用 `[IMAGE:]` 和 `[DOCUMENT:]` 标记。
- 但还没有统一的 text/card/media/reply/thread 发送动作模型。
- 也不支持官方风格的 URL/file URL/buffer 媒体输入、delete/unsend、列出/移除 reaction、forward、typing indicator 或 chat management 对齐。

优先级：P0

缺口总结：

- 没有统一的出站请求模型。
- 基于 marker 的附件流比官方媒体管线更窄。
- 媒体发送失败时缺少降级兜底行为。
- 缺少 receive_id 规范化与 thread 继承对齐。
- 缺少 audio/video/media 消息类型。
- 除 ack helper 之外，缺少完整的出站 reaction 管理。

### D. Cards And Rich Interaction

官方模块：

- `src/card/*`
- `src/messaging/converters/interactive/*`

本地映射：

- 目前还没有专门的 Lark card 子系统。
- 潜在落点：`src/channels/lark.rs`、新建 `src/channels/lark_cards.rs`、`src/agent/*`、以及 `src/channels/mod.rs` 中的 observer plumbing。

当前状态：

- 缺失。
- 本地 Lark 实现尚不支持 interactive cards、streaming cards、reply dispatchers、markdown/card 样式、不可用保护或 card reply modes。

优先级：P1

缺口总结：

- 没有 card builder 或 card payload 抽象。
- 没有渐进式回复的 stream update controller。
- 没有敏感操作的 confirmation-card UX。
- 没有 card 不可用时的优雅降级路径。

### E. Tool Surface: IM / Docs / Drive / Wiki / Bitable / Sheets / Calendar / Task / Search

官方模块：

- `src/tools/oapi/im/*`
- `src/tools/oapi/chat/*`
- `src/tools/oapi/common/*`
- `src/tools/oapi/drive/*`
- `src/tools/oapi/wiki/*`
- `src/tools/oapi/bitable/*`
- `src/tools/oapi/sheets/*`
- `src/tools/oapi/calendar/*`
- `src/tools/oapi/task/*`
- `src/tools/oapi/search/*`
- `src/tools/mcp/doc/*`

本地映射：

- `src/tools/*`
- `src/tools/schema.rs`
- `src/tools/traits.rs`
- `src/providers/*` 与 `src/agent/loop_.rs` 用于工具注册与调用

当前状态：

- 作为 Feishu 专属一等工具来说，大多原先缺失。
- ZeroClaw 核心本身已经有通用工具框架，所以宿主承载面是具备的。
- 但 Rust 代码树里原先没有 Feishu 专属 OAPI 工具族。

优先级：P1

缺口总结：

- 没有原生 Feishu docs/wiki/drive/search/calendar/task/bitable/sheets 工具。
- 没有 IM read/resource 工具对齐。
- 没有 MCP doc helper 对齐。
- 没有面向 Feishu 能力的 tool-scope bridge。

### F. Auth / OAuth / Scope / Security

官方模块：

- `src/tools/oauth*.js`
- `src/tools/auto-auth.js`
- `src/tools/onboarding-auth.js`
- `src/core/device-flow.js`
- `src/core/scope-manager.js`
- `src/core/tool-scopes.js`
- `src/core/app-scope-checker.js`
- `src/core/security-check.js`
- `src/core/owner-policy.js`
- `src/core/app-owner-fallback.js`
- `src/core/permission-url.js`

本地映射：

- `src/auth/*`
- `src/security/*`
- `src/tools/*`
- `src/onboard/*`

当前状态：

- 就 Feishu 专属能力对齐而言完成度较低。
- 我们已有通用 auth 与 security 基础设施，但还没有官方插件暴露出来的 Feishu 专属 scope 模型、OAuth UX、owner policy 与权限检查流程。

优先级：P1

缺口总结：

- 没有 Feishu OAuth/device-flow 用户链路。
- 没有 Feishu tool-scope 注册与 enforcement 层。
- 没有 owner-policy 对齐。
- 没有 permission URL / auth-error 指引流程。

### G. Commands / Ops / Diagnostics

官方模块：

- `src/commands/auth.js`
- `src/commands/doctor.js`
- `src/commands/diagnose.js`
- `src/channel/monitor.js`
- `src/channel/probe.js`

本地映射：

- `src/doctor/mod.rs`
- `src/main.rs`
- `src/health/mod.rs`
- `src/channels/lark.rs`

当前状态：

- 已有部分通用运行时诊断能力，但 Feishu 专属运维工具还未达到官方插件对等水平。

优先级：P2

缺口总结：

- 没有专门的 Feishu doctor/diagnose 命令集。
- 没有 scope/auth/media/config 专项健康探针。
- 没有与官方插件等价的面向操作员的排障路径。

## 建议的交付阶段

### Phase 1: Lark Channel Core Parity

目标：

- 稳定的多账号配置模型
- 入站/出站统一消息管线
- 结构化消息上下文
- 官方风格的 target/reply/thread 规范化

优先修改文件：

- `src/channels/lark.rs`
- `src/channels/mod.rs`
- `src/config/schema.rs`

退出标准：

- 文本、post、图片、文件、音视频占位符、reply/thread 路由、reaction 管理，以及接收/发送规范化，都能通过同一套一致的 Rust channel surface 工作。

### Phase 2: Rich Media And Card Parity

目标：

- 入站媒体下载
- 出站媒体源规范化
- streaming cards
- interactive cards
- graceful degrade 行为

可能涉及文件：

- `src/channels/lark.rs`
- 新建 `src/channels/lark_cards.rs`
- 新建 `src/channels/lark_media.rs`
- `src/channels/traits.rs`

退出标准：

- 用户可以直接发送或接收富媒体与基于卡片的交互，不再依赖临时脚本。

### Phase 3: Feishu Native Tool Surface

目标：

- IM read/resource 工具
- docs/wiki/drive/bitable/sheets/calendar/task/search 工具

可能涉及文件：

- 新建 `src/tools/feishu_*`
- `src/tools/mod.rs`
- `src/tools/schema.rs`

退出标准：

- Rust 版 `zeroclaw` 暴露出一套 Feishu 原生工具族，在覆盖面上大致接近官方插件。

### Phase 4: Auth / Scope / Onboarding / Diagnostics

目标：

- OAuth/device flow
- scope 管理
- onboarding 与迁移
- doctor/diagnose 对齐

可能涉及文件：

- `src/auth/*`
- `src/security/*`
- `src/onboard/*`
- `src/doctor/mod.rs`
- `src/main.rs`

退出标准：

- Feishu 的接入、认证修复、权限检查与运维诊断，都能在产品内部完成，而不是依赖手工改代码。

## 验证清单

- 每个阶段都要有 code-to-spec 的能力对齐检查清单。
- 每个阶段都要补 Rust 测试，覆盖：
  - happy path
  - auth refresh path
  - unsupported payload path
  - retry / reconnect path
  - permission denied path
- 在所有差异彻底关闭前，始终维护一节“官方行为差异”说明。

## 初始缺口统计

- P0 缺口：Channel Core、Messaging Inbound、Messaging Outbound
- P1 缺口：Cards/Rich Interaction、Feishu Tool Surface、Auth/Scope/Security
- P2 缺口：Commands/Ops/Diagnostics

## 官方行为差异

以下差异在当前阶段仍被视为已知未闭合项，继续保留，直到实现与官方运行时源码完全对齐：

- Feishu OAuth 目前仅实现 authorization-code / redirect 流，尚未补 device flow。
- Scope 目前以错误提示和静态注册为主，尚未补官方风格的 preflight app-scope introspection。
- Feishu 工具族已具备最小可用覆盖面，但在 Docs、Drive、Bitable、Sheets、Calendar、Task、Search 细节上仍未达到官方全部动作宽度。
- Card/streaming/interactive 已经贯通基础链路，但更细粒度的降级策略和更丰富的控制器状态仍在收口。
- 专门的 Feishu doctor / diagnose 命令面仍需持续补充 live probe、配置修复建议和更多 operator-facing 细节。

## 建议的立即下一步任务

先为 **Phase 1: Lark Channel Core Parity** 拆出详细子计划，因为后续所有阶段都依赖统一的 channel/runtime/message 模型。

## 可执行任务拆解

### Phase 1.1: Multi-Account Runtime Model

状态：已完成，默认/具名账号身份、显式 `enabled` 语义、账号级缓存隔离与健康命名已对齐并完成回归验证。

**文件：**

- 修改：`src/channels/lark.rs`
- 修改：`src/config/schema.rs`
- 修改：`src/channels/mod.rs`
- 测试：`src/channels/lark.rs`

**Step 1：审计当前账号相关字段与构造器**

运行：

```bash
rg -n "from_feishu_config|from_named_feishu_config|feishu_accounts|app_id|app_secret|channel_name" src/channels/lark.rs src/channels/mod.rs src/config/schema.rs
```

预期：

- 当前已经有具名 Feishu 账号支持，但运行时行为仍主要是 per-channel-instance，而不是账号目录驱动。

**Step 2：增加显式内部账号身份模型**

实现：

- 在 Lark channel runtime 内加入稳定的 `account_id` 字段。
- 明确区分平台类型（`lark` vs `feishu`）和运行时账号身份。
- 提供“默认账号”与“具名账号”的辅助方法。

**Step 3：为账号身份行为补测试**

新增测试：

- 默认 Feishu 账号命名
- 具名 Feishu 账号命名
- 具名账号路由一致性
- 不会意外从一个账号回退到另一个账号

**Step 4：为账号级资源增加运行时辅助方法**

实现：

- account-scoped token cache keys
- account-scoped bot open_id cache keys
- account-scoped dedupe/heartbeat naming

**Step 5：验证**

运行：

```bash
cargo test channels::lark::tests:: --lib
```

### Phase 1.2: Unified Inbound Message Context

状态：已完成，内部 parsed message 模型、channel surface 透传、以及 runtime 消费链路已对齐并完成回归验证。

**文件：**

- 修改：`src/channels/lark.rs`
- 修改：`src/channels/traits.rs`
- 测试：`src/channels/lark.rs`

**Step 1：定义目标入站对齐结构**

实现一个内部 message parse result，包含：

- message id
- chat id
- sender id
- chat type
- root id
- parent id
- thread id
- content type
- raw content
- normalized text
- attachments/resources

**Step 2：把当前 text/post 解析重构进新 parse result**

目标：

- 把现有零散的 text/post 解析移动到一条统一的内部 parse pipeline 中
- 在为 image/file/audio/video/interactive 扩展做准备的同时，保留当前行为

**Step 3：为 text/post 对齐保真补测试**

测试：

- DM text
- group text with mention rules
- post message with mention extraction
- malformed content JSON

**Step 4：保留 thread 元数据**

实现：

- 保留 `root_id`、`parent_id`、`thread_id`
- 即使 agent surface 暂时没用到全部字段，也要先透传进内部 message 对象

**Step 5：验证**

运行：

```bash
cargo test lark_parse_ --lib
```

补充验证：

```bash
cargo test channels::lark::tests:: --lib
cargo test process_feishu_group_message_history_preserves_sender_identity_from_context --lib
cargo test process_feishu_threaded_media_message_preserves_lark_context_metadata --lib
cargo test feishu_reply_root_id_without_thread_id_does_not_split_session_key --lib
```

### Phase 1.3: Inbound Rich Message Converters

状态：已完成，官方 `@larksuite/openclaw-lark` converter 清单已覆盖并补齐关键格式细节。

说明：

- 本阶段按计划文档目标已经通过。
- 但这里的“完成”不等于与官方源码实现逐分支、逐字符 `100%` 等价。
- 当前实现已经覆盖主路径与大部分 `interactive` concise 行为，足以满足本阶段对齐目标。
- 若后续要追求“与官方完全一模一样”，仍需继续做官方 `interactive` converter 的逐 payload 对拍与边角 case 收口。

**文件：**

- 修改：`src/channels/lark.rs`
- 新建：`src/channels/lark_inbound.rs`
- 测试：`src/channels/lark.rs`

已完成项：

- 已从 `lark.rs` 拆出 converter helpers，并迁入 `src/channels/lark/inbound.rs`
- 已覆盖官方 converter 清单：
  - text
  - post
  - image
  - file
  - audio
  - video / media
  - sticker
  - folder
  - interactive
  - share_chat
  - share_user
  - location
  - merge_forward
  - system
  - hongbao
  - share_calendar_event
  - calendar
  - general_calendar
  - video_chat
  - todo
  - vote
  - unknown fallback
- 已实现 resource descriptor 提取：
  - `image_key` / `file_key`
  - `file_name`
  - `post` 内嵌图片/文件资源
  - `sticker` 资源类型单独建模
- 已对齐关键格式细节：
  - `share_chat` / `share_user` 空 id 仍保留 `id=""`
  - `calendar` / `todo` / `video_chat` 毫秒时间戳转北京时间
  - `interactive` 使用 `<card title="...">`
  - `interactive` 的非法 `json_card` 返回明确占位
  - `merge_forward` 支持异步展开、递归嵌套和时间戳格式化
- 已补齐回归测试，覆盖新增 converter 行为与边界情况

**验证**

运行：

```bash
cargo test channels::lark::tests:: --lib
```

### Phase 1.4: Inbound Media Download And Local Persistence

状态：已完成，入站媒体下载、落盘与失败路径已具备测试证明。

**文件：**

- 修改：`src/channels/lark.rs`
- 新建：`src/channels/lark_media.rs`
- 修改：`src/channels/mod.rs`
- 测试：`src/channels/lark.rs`

已完成项：

- 已实现基于 `message_id + file_key + type` 的资源下载
- 已支持在存在 workspace 时，把入站媒体落到：
  - `workspace/channels/<channel>/<account>/inbound/...`
- 已保留文件名线索，并通过响应头 / MIME / 文件头推断扩展名
- 已增加大小保护：
  - 超过默认 30 MB 的入站资源拒绝落盘
- 已把本地路径回填进入站消息内容：
  - 图片 -> `[IMAGE:/abs/path]`
  - 文件 -> `[DOCUMENT:/abs/path]`
  - 音频 -> `[AUDIO:/abs/path]`
  - 视频 -> `[VIDEO:/abs/path]`
- 已补测试：
  - image download success
  - file download success
  - oversized payload failure path

**验证**

运行：

```bash
cargo test channels::lark::tests:: --lib
```

### Phase 1.5: Unified Outbound Request Model

状态：进行中，内部出站请求规范化已经落地，同时保留了当前 marker/path 行为。

**文件：**

- 修改：`src/channels/lark.rs`
- 新建：`src/channels/lark_outbound.rs`
- 测试：`src/channels/lark.rs`

**Step 1：定义内部出站请求结构体**

字段：

- target
- text
- card
- media inputs
- file name
- reply_to_message_id
- reply_in_thread
- account_id

**Step 2：重构当前 `Channel::send` 逻辑，先构建出站请求**

目标：

- 保留当前 marker 支持
- 所有发送行为都先走统一规范化函数

**Step 3：为请求规范化补测试**

测试：

- text only
- text + image marker
- path-only image
- text + file marker
- unresolved marker fallback

**Step 4：验证**

运行：

```bash
cargo test lark_parse_attachment_ --lib
```

### Phase 1.6: Target / Reply / Thread Normalization

状态：进行中，`thread_ts` 现在已经能流入原生 Lark reply-in-thread 发送行为，不再在 channel 边界被忽略。

**文件：**

- 修改：`src/channels/lark.rs`
- 修改：`src/channels/mod.rs`
- 测试：`src/channels/lark.rs`

**Step 1：增加 target normalization helper**

实现：

- normalize target id
- 如有需要，推断 receive id 风格
- 区分 chat target 与 reply target

**Step 2：把 reply 和 thread 字段接入出站流程**

实现：

- reply-to message id send path
- thread reply send path
- 若 channel context 可提供，则继承 current-thread

**Step 3：补测试**

测试：

- normal chat send
- reply send
- thread reply send
- named Feishu account reply path

**Step 4：验证**

运行：

```bash
cargo test lark_build_ --lib
```

### Phase 1.7: Outbound Media Source Normalization

状态：进行中，marker 解析现在已接受 `file://` 与远程 `http(s)` 附件，并在发送前将它们物化为可上传本地文件。

**文件：**

- 修改：`src/channels/lark.rs`
- 修改：`src/channels/lark_media.rs`
- 测试：`src/channels/lark.rs`

**Step 1：增加媒体输入变体**

支持：

- absolute local path
- `file://` URL
- remote URL

**Step 2：增加安全的本地路径校验**

实现：

- 仅允许 approved roots 或显式允许的本地路径
- 拒绝含糊不清的相对路径

**Step 3：增加媒体上传前的远程抓取路径**

实现：

- remote URL download
- file name derivation
- MIME/type inference before upload

**Step 4：补测试**

测试：

- local path media
- file URL media
- remote URL media
- invalid URL / unsupported scheme

**Step 5：验证**

运行：

```bash
cargo test channels::lark::tests:: --lib
```

### Phase 1.8: Outbound Media Message Types

状态：进行中，出站附件分类现在已覆盖 image/file/audio/video，并将 video 映射为 `media`、audio 映射为 `audio` 消息类型。

**文件：**

- 修改：`src/channels/lark.rs`
- 修改：`src/channels/lark_media.rs`
- 测试：`src/channels/lark.rs`

**Step 1：按媒体类型拆分发送 helper**

实现：

- send image
- send file
- send audio
- send video

**Step 2：按媒体分类拆分上传 helper**

实现：

- image upload
- file upload
- 为 audio/video/general file 做 file type mapping

**Step 3：补测试**

测试：

- image payload uses `image_key`
- file payload uses `file_key`
- audio/video classification routes correctly

**Step 4：验证**

运行：

```bash
cargo test lark_build_ --lib
```

### Phase 1.9: Reaction Management Parity

状态：已完成。trait 级 add/remove reaction 支持现已作为 Lark ack reactions 的底层能力，并已补齐 bot-owned reaction lookup/delete 对齐测试。

**文件：**

- 修改：`src/channels/lark.rs`
- 修改：`src/channels/traits.rs`
- 测试：`src/channels/lark.rs`

**Step 1：将 reaction helpers 从 ack-only 提升为通用能力**

实现：

- add reaction
- remove reaction
- 如删除流程需要，可选的 reaction listing primitive

**Step 2：为 Lark 接上 trait 级 reaction 支持**

目标：

- Lark 通过通用 channel reaction 方法工作，而不是继续用私有的 ack-only 行为

**Step 3：将基于 locale 的 ack reaction 保留为策略层**

执行：

- 保留当前 ack UX
- 但把它移动到通用 reaction 能力之上

**Step 4：补测试**

测试：

- add reaction success path
- remove reaction success path
- ack reaction still works

**Step 5：验证**

运行：

```bash
cargo test lark_reaction_ --lib
```

### Phase 1.10: Channel-Level Health / Probe / Diagnostics

状态：进行中，Lark/Feishu probes 现在会把 config/token/transport/bot-identity 状态写进 health snapshots 与 doctor summaries；下一步是把同样的细节扩展到更广泛的 operator-facing diagnostics。

**文件：**

- 修改：`src/channels/lark.rs`
- 修改：`src/doctor/mod.rs`
- 修改：`src/health/mod.rs`
- 测试：`src/channels/lark.rs`

**Step 1：增加 Feishu/Lark 专属 probe helpers**

检查：

- token fetch
- websocket connectivity
- configured account completeness
- bot identity resolution

**Step 2：通过 doctor/health surface 暴露 probe 结果**

目标：

- 操作员能看清故障是 auth、config、transport 还是 permission 相关

**Step 3：补测试**

测试：

- missing app id/app secret
- invalid token path
- bot identity unresolved

**Step 4：验证**

运行：

```bash
cargo test channels::lark::tests:: --lib
```

### Phase 2.1: Card Payload Abstraction

状态：进行中，原生 card payload 解析和 interactive send/reply body 已经通过显式 fenced `lark-card` / `feishu-card` payload 落地；streaming/update shell 留待后续阶段。

**文件：**

- 新建：`src/channels/lark_cards.rs`
- 修改：`src/channels/lark.rs`
- 测试：`src/channels/lark.rs`

**Step 1：引入内部 card model**

支持：

- static card payload
- reply card payload
- streaming/updatable card shell

**Step 2：增加 cards 的出站发送路径**

目标：

- card send 不再建立在 text message 假设之上

**Step 3：补测试**

测试：

- simple card payload serialization
- reply card routing

### Phase 2.2: Streaming Card Controller

状态：进行中

已实现：

- thinking / generating / completed / failed 的显式 draft phase 映射
- 带节流 patch flushing 的 channel-level draft session worker
- finalize path：当最终输出是 image/file/audio/video 时，从 draft card 切回原生媒体发送
- 为节流更新与附件 finalize resend 提供回归测试

**文件：**

- 修改：`src/channels/lark_cards.rs`
- 修改：`src/channels/mod.rs`
- 修改：`src/agent/loop_.rs`

**Step 1：定义 streaming 状态机**

状态：

- thinking
- generating
- completed
- failed

**Step 2：把 streaming updates 接到 agent reply 生命周期**

目标：

- 对于长耗时回复，在支持的平台上实现原地更新

**Step 3：增加测试 / 模拟钩子**

剩余：

- card patch/delete 失败时，对 unavailable-message guard 做对齐
- 在当前 IM patch mode 之外，补更丰富的 controller transition 与 fallback heuristics
- 如果 Rust runtime 未来引入 CardKit transport，则补 CardKit 对齐

### Phase 2.3: Interactive Card Replies And Confirmation Flow

状态：进行中

子任务：

1. 增加原生 confirmation-card builder，对齐 confirm / reject / preview buttons。
2. 将 `card.action.trigger` callbacks 解析为结构化 channel events。
3. 把 callback events 接回现有 channel message pipeline，让 agent/tool 层可消费。
4. 让敏感操作工具输出并消费 confirmation-card contract。

**文件：**

- 修改：`src/channels/lark_cards.rs`
- 修改：`src/channels/lark.rs`
- 在需要 confirmation 语义的地方修改：`src/tools/*`

**Step 1：定义 interactive callback payload handling**

已实现：

- 原生 confirmation-card payload builder，支持 confirm / reject / preview actions。
- `card.action.trigger` callbacks 会被解析为结构化 fenced channel messages。

**Step 2：将 interactive responses 映射为 channel events**

已实现：

- Lark/Feishu callback events 现已进入现有 channel message pipeline。
- Channel runtime 会在正常 LLM 处理前识别 `lark-card-action` messages。

**Step 3：为敏感操作增加 confirmation-card 流程**

已实现：

- 当非 CLI 工具调用因显式审批要求而失败时，tool loop 现在会发出结构化 `zeroclaw-approval` contract。
- Channel runtime 会保存 pending approvals、发送 Lark confirmation cards，并消费 `confirm_write` / `reject_write` / `preview_write`。
- 确认后的动作会以 `approved=true` 重新执行，并在可用时通过当前 provider 把结果摘要回会话。

剩余：

- 超出当前 explicit-approval error path 之外，还要把 confirmation contract 拓展为更一等的工具语义，以覆盖官方插件更丰富的行为。

### Phase 2.4: Unavailable / Degrade Guard

状态：已完成

**文件：**

- 修改：`src/channels/lark_cards.rs`
- 修改：`src/channels/lark.rs`

**Step 1：检测 card 不可用状态**

已实现：

- patch/delete 重试现在会检测 recalled/deleted cards 等终态消息，并把消息缓存为 unavailable。

**Step 2：回退到纯文本，同时保留任务结果**

已实现：

- 一旦 card/message 进入终态失败，后续 patch/delete 会直接短路，从而让 streaming/final delivery 能安全降级，避免重复噪声报错。

### Phase 3.1: Feishu IM Tool Family

状态：进行中

**文件：**

- 新建：`src/tools/feishu_im_read.rs`
- 新建：`src/tools/feishu_im_message.rs`
- 新建：`src/tools/feishu_im_resource.rs`
- 修改：`src/tools/mod.rs`
- 修改：`src/tools/schema.rs`

**Step 1：增加 read-message 能力**

已实现：

- 新增可复用的 Feishu OAPI client 基础设施，覆盖 account resolution、tenant token 获取、JSON requests 和 resource download。
- 新增 `feishu_im_read`，按 message ID 获取消息详情。

**Step 2：增加 send/manage message 能力**

已实现：

- 新增 `feishu_im_message`，支持 `send_text`、`reply_text`、`update_text`、`delete_message`。
- 当配置了 Feishu 账号时，会将 IM tools 注册进主工具注册表。

**Step 3：增加消息资源能力**

已实现：

- 新增 `feishu_im_resource`，用于下载消息资源并持久化到 workspace。

剩余：

- 从当前偏 text 的 IM surface 继续扩展到更丰富的媒体 / 原生动作对齐，以及更广泛的账号 / 区域覆盖。

### Phase 3.2: Docs / Wiki / Drive Tool Family

状态：进行中，最小可用原生 Docs/Wiki/Drive 工具族已落地并注册。

**文件：**

- 新建：`src/tools/feishu_doc_create.rs`
- 新建：`src/tools/feishu_doc_fetch.rs`
- 新建：`src/tools/feishu_doc_update.rs`
- 新建：`src/tools/feishu_drive_file.rs`
- 新建：`src/tools/feishu_wiki_space.rs`
- 修改：`src/tools/mod.rs`

**Step 1：文档 CRUD 对齐**

已实现：

- 新增 `feishu_doc_create`，支持创建 Docx 并可选指定父目录。
- 新增 `feishu_doc_fetch`，支持文档元数据、block tree 与 raw-content 获取。
- 新增 `feishu_doc_update`，支持 `update_title` 与 `delete_document`。

**Step 2：Drive 文件访问对齐**

已实现：

- 新增 `feishu_drive_file`，支持 `upload_file`、`get_file_meta`、`download_file`。
- Drive 下载默认也会持久化到 workspace 级存储，与 ZeroClaw 其他资源下载模式保持一致。

**Step 3：Wiki 导航对齐**

已实现：

- 新增 `feishu_wiki_space`，支持 `list_spaces`、`get_node`、`list_nodes`。
- 所有新增 Docs/Wiki/Drive 工具都已在 Feishu 账号可用时注册进主工具表。

剩余：

- 将 doc mutations 从 title/delete 扩展到更丰富的 block/content 编辑对齐。
- 若要推进到官方更完整覆盖面，则继续扩展 drive 的 folder/list/export/share 等操作。
- 用真实 Feishu app 校验当前 endpoint 集合，并记录仍然存在的官方行为差异。

### Phase 3.3: Bitable / Sheets Tool Family

状态：进行中，最小可用原生 Bitable 与 Sheets 工具族已落地并注册。

**文件：**

- 新建：`src/tools/feishu_bitable_*.rs`
- 新建：`src/tools/feishu_sheets_*.rs`
- 修改：`src/tools/mod.rs`

**Step 1：Bitable app/table/field/record/view 能力面**

已实现：

- 新增 `feishu_bitable`，支持 `get_app`、`list_tables`、`list_fields`、`list_views`、`list_records`、`create_record`、`update_record`。
- 当配置了 Feishu 账号时，Bitable 工具会注册进主 Feishu 原生工具族。

**Step 2：Sheets 读写能力面**

已实现：

- 新增 `feishu_sheets`，支持 `get_meta`、`read_range`、`write_range`。
- 扩展共享 Feishu client，支持带鉴权的 `PUT` JSON 请求，用于 Sheets 值写入。

剩余：

- 继续把 Bitable 对齐扩展到 field/view mutations、batch record operations、以及更丰富的 filtering/sorting 语义。
- 如果需要更接近官方覆盖面，继续把 Sheets 扩展到创建 sheet、append/insert、以及 formatting 级操作。
- 用真实 Feishu app 校验精确的 Bitable/Sheets endpoint 集合，并记录行为差异。

### Phase 3.4: Calendar / Task / Search Tool Family

状态：进行中，最小可用原生 Calendar / Task / Search 工具族已落地并注册。

**文件：**

- 新建：`src/tools/feishu_calendar_*.rs`
- 新建：`src/tools/feishu_task_*.rs`
- 新建：`src/tools/feishu_search_*.rs`
- 修改：`src/tools/mod.rs`

**Step 1：Calendar CRUD 与参会人管理**

已实现：

- 新增 `feishu_calendar`，支持 `list_calendars`、`list_events`、`create_event`、`update_event`。
- Calendar 事件处理现已为 Feishu 原生工具面提供基础排期 CRUD 路径。

**Step 2：Task / tasklist / comment / subtask 对齐**

已实现：

- 新增 `feishu_task`，支持 `list_tasklists`、`list_tasks`、`get_task`、`create_task`、`update_task`。
- 这已经覆盖核心 task/tasklist 读写闭环，尽管 comments 与 subtasks 仍未完成。

**Step 3：Search 对齐**

已实现：

- 新增 `feishu_search`，基于 Search v2 的 doc/wiki 搜索面。
- Calendar / Task / Search 已与其他 Feishu 原生工具一起注册。

剩余：

- 将 calendar 支持扩展到 attendee management 与更广泛的 event mutation 对齐。
- 将 task 支持扩展到 comments、subtasks 与更丰富的 tasklist 语义。
- 如果要达到更广的官方插件覆盖面，则把 search 扩展到当前 doc/wiki 查询面之外。

### Phase 4.1: OAuth And Device Flow

状态：进行中，最小可用 Feishu OAuth authorization-code 流程现已接入原生 auth 命令。

**文件：**

- 新建：`src/auth/feishu_oauth.rs`
- 修改：`src/auth/mod.rs`
- 修改：`src/onboard/*`
- 修改：`src/main.rs`

**Step 1：实现 Feishu 专属 OAuth/device flow client**

已实现：

- 新增原生 `feishu_oauth` 模块，覆盖 authorization URL 生成、loopback callback 捕获、redirect-code 解析、token exchange 与 refresh-token 支持。

**Step 2：增加面向用户的 onboarding 流程**

已实现：

- `zeroclaw auth login --provider feishu`
- `zeroclaw auth paste-redirect --provider feishu`
- `zeroclaw auth refresh --provider feishu`

这些命令现在与 OpenAI/Gemini 已使用的 pending-login 持久化模型保持一致。

**Step 3：增加 auth recovery 路径**

已实现：

- 已通过 `AuthService` 增加 Feishu OAuth profile 持久化与自动刷新。

剩余：

- Feishu 的 device-code flow 仍未实现，当前仅支持 browser/redirect 模式。
- 当前使用的 Feishu OAuth endpoints 仍需用真实 app 验证，因为现有实现仍主要是基于公开 API 资料推断，而不是在本仓库完成端到端实测。

### Phase 4.2: Scope Management And Permission Checks

状态：进行中，scope registry 与 permission-hint plumbing 已经为原生 Feishu 工具族落地。

**文件：**

- 新建：`src/security/feishu_scopes.rs`
- 修改：`src/security/mod.rs`
- 修改：`src/tools/*`

**Step 1：定义 Feishu scope registry**

已实现：

- 新增中心化 Feishu scope registry，将每个原生 Feishu 工具映射到其预期 OpenAPI scopes。

**Step 2：将工具映射到 scope requirements**

已实现：

- 已为 IM、Docs、Drive、Wiki、Bitable、Sheets、Calendar、Task、Search 工具注册明确 scope 集。

**Step 3：增加 preflight permission checks**

已实现：

- 当 Feishu 工具失败且看起来像 permission/scope 错误时，现在会附带针对工具的 scope 指引，便于操作员立即知道该授予什么权限。

剩余：

- 在接入 Feishu OAuth/app-scope introspection 后，从“出错时提示”升级到真正的 preflight permission introspection。

### Phase 4.3: Owner Policy And Safe Defaults

状态：进行中，owner-policy 评估与诊断现已在 Lark/Feishu channels 中对外可见。

**文件：**

- 新建：`src/security/feishu_owner_policy.rs`
- 修改：`src/security/mod.rs`
- 修改：`src/channels/lark.rs`

**Step 1：定义 owner/user policy 规则**

已实现：

- 新增专用 owner-policy evaluator，用于 Feishu/Lark channels，覆盖 empty allowlists、wildcard allowlists 与 mention-only group gating。

**Step 2：执行 DM/group 安全默认值**

已实现：

- Health probes 与 doctor summaries 现在会显示 owner-policy disposition，让 wildcard/no-mention 这类高风险配置能直接在诊断里暴露出来。

剩余：

- 决定评审建议中的 owner-policy 场景，究竟应升级为强约束，还是继续保留为面向操作员可见的诊断提示。

### Phase 4.4: Onboarding Migration Parity

状态：已完成。

已交付：

- onboarding 新接入 Feishu 时，直接写入原生 `channels_config.feishu`，不再继续生成 legacy `lark.use_feishu` 配置。
- channels repair wizard 现在会在进入交互修复前，先尝试把 legacy Feishu 兼容配置迁移到原生 `feishu` 节点。
- 迁移 helper 具备保守策略：若原生 `channels_config.feishu` 已存在，则保留 legacy `lark.use_feishu`，避免误覆盖用户现有配置。
- 已补回归测试，覆盖“legacy -> native 迁移”和“已有 native 配置时不覆盖”两条核心路径。

**文件：**

- 修改：`src/onboard/mod.rs`
- 修改：`src/onboard/wizard.rs`
- 修改：`src/config/schema.rs`

**Step 1：增加 Feishu 专属 onboarding prompts**

**Step 2：增加 legacy-to-current migration helpers**

### Phase 5.1: Doctor / Diagnose Commands

状态：已完成。

已交付：

- 新增 `zeroclaw doctor feishu [--account <name>]` 子命令，提供 Feishu/Lark 专项诊断入口。
- 支持识别三类配置来源：原生默认账号、原生具名账号、legacy `lark.use_feishu` 兼容路径。
- webhook 模式下会检查 `verification_token` / `encrypt_key` 缺失。
- 会提示 `allowed_users` 为空或包含 `*` 这类高风险配置。
- 会执行 live health check，并按账号打印健康状态。
- 通用 `doctor` 结果中也已加入 Feishu/Lark 相关诊断增强，包括外部 Node 插件并存冲突提示，以及 daemon channel probe 的 Feishu/Lark 细节摘要。

**文件：**

- 修改：`src/doctor/mod.rs`
- 修改：`src/main.rs`
- 新建：`src/doctor/feishu.rs`

**Step 1：增加 Feishu doctor checks**

**Step 2：增加 Feishu diagnose output**

### Phase 5.2: Parity Test Matrix

状态：已完成。

已交付：

- 新增独立 `tests/feishu_parity.rs` 作为 Phase 对齐矩阵入口。
- 测试会检查关键 Feishu/Lark 能力模块是否都已落到 Rust 代码树中。
- 测试会检查计划文档中是否保留“官方行为差异 / 有意差异”追踪段，避免对齐过程失去剩余差异记录。
- 测试会检查 `Phase 4.4 / 5.1 / 5.2` 三个阶段标题仍保留在计划文档中，保证计划与实现之间的可追踪性。

**文件：**

- 新建：`tests/feishu_parity/*.rs` 或扩展现有 channel tests

**Step 1：建立按功能维度划分的对齐检查清单**

**Step 2：按阶段补齐回归测试**

**Step 3：追踪剩余的有意差异**

当前仍保留为“有意差异 / 后续再做”的内容，不属于本阶段未完成项：

- 暂未把 `doctor feishu` 做成与官方 CLI 完全一致的文案和输出格式，只保证行为覆盖与运维可读性。
- parity test matrix 当前以“模块存在 + 计划追踪 + 关键行为回归”为主，尚未扩展成逐能力的端到端 fixture 矩阵。
