# 上游 ZeroClaw 更新审阅

## 目的

这份文档用于审阅 `zeroclaw-labs/zeroclaw` 上游在我们当前同步基线之后新增的提交，帮助判断：

- 上游最近更新了什么
- 哪些改动适合并到我们当前这份 `zeroclaw`
- 哪些改动会和我们本地 Feishu / WeCom / channel runtime 改造产生明显冲突
- 后续推荐采用 `cherry-pick / 手工对齐 / 暂不引入` 哪种方式

## 审阅范围

- 我们本地当前 HEAD：
  - `2518bff` `feat: 收口飞书通道重构并默认启用 PDF 解析`
- 我们上次明确带有“合并 upstream master”语义的本地基线：
  - `b2cee71` `Merge upstream master into local fork`
- 该 merge commit 的 upstream parent：
  - `8d7abb7` `fix(config): accept openai-* aliases for wire_api config (#3191)`
- 上游当前 `master`：
  - `220745e` `ci: add API token guidance to dashboard reauth notice (#3640)`

结论：

- 这次不是“小量增量”，而是一次中等偏大的上游前进。
- 基于 GitHub compare 页面可确认：
  - `ahead_by = 305`
  - `files changed = 422`
- 这意味着不适合直接无脑 merge。
- 更适合的策略是：
  - 先按主题分组审阅
  - 对低冲突修复类改动做选择性吸收
  - 对高冲突架构类改动单独开一轮对齐

## 重要说明

由于当前本地网络环境下：

- `git fetch upstream` 可见远端 refs，但实际对象抓取不稳定
- GitHub API 原始 compare JSON / patch 在本机命令行侧存在连接重置问题

所以这份文档采用的是：

- 本地 git 历史证据
- GitHub compare 页面统计
- GitHub 提交页最近提交
- `ls-remote` 可见的上游活跃分支名

来做主题级审阅和合并建议。

也就是说：

- 这份文档足够支持“决策”
- 但还不是 305 条 commit 的逐条 diff 级审阅清单
- 如果后续你要，我建议下一轮单独做“逐条 appendix”

## 总体判断

上游这 305 个 commit，按价值和合并风险看，大致分成 5 类：

1. 修复类，适合优先吸收
2. 安全/策略类，值得审慎吸收
3. 通道/runtime 行为类，价值高但和我们本地改动冲突也高
4. 新集成/新能力类，功能多，但大多数和我们当前目标无直接关系
5. CI / dashboard / release / docs 类，对本地使用价值有限，可延后

我的总体建议不是直接 merge upstream `master`，而是：

- 第一批优先吸收：
  - panic / timeout / tool failure / daemon restart / pairing 修复
- 第二批选择性吸收：
  - runtime / channel / security 中和我们目标一致的部分
- 第三批暂缓：
  - 新产品面能力、大型 dashboard / session / terminal UI / workspace 架构改造

## 最近可确认的上游提交

以下是我从 GitHub 提交页直接确认到的最近几条：

- `220745e` `ci: add API token guidance to dashboard reauth notice (#3640)`
- `31cc6bf` `fix(daemon): restart current daemon after openclaw update (#3643)`
- `7e11333` `chore: stage and commit workspace changes after openclaw update (#3636)`
- `7b981b1` `feat(config): add default heartbeat interval and timeout (#3637)`
- `4a5bc5f` `fix(ci): gate X posts on canonical release events only (#3635)`

这些提交本身说明：

- 上游最近在持续处理 daemon/update/release/dashboard 提示体验
- 同时在推进更稳定的默认 heartbeat 行为

## 从远端分支名可确认的主题

`ls-remote` 能看到的上游活跃/近期主题分支包括：

- `fix/3460-context-window-exceeded`
- `work-issues/3533-fix-utf8-slice-panic`
- `work-issues/3544-fix-codex-sse-buffering`
- `work-issues/3628-surface-tool-failures-in-chat`
- `work-issues/3262-channel-proxy-support`
- `work-issues/3486-fix-matrix-image-marker`
- `work-issues/3477-fix-matrix-channel-key`
- `work-issues/3487-channel-approval-manager`
- `issue-2494-feishu-secret-roundtrip`
- `issue-2487-channel-ack-schema-v2`
- `feat/default-enable-web-tools`
- `feature/interactive-session-state`
- `feature/terminal-ui`
- `work/multi-client-workspaces`
- `feat/google-workspace-cli`
- `feat/microsoft365`
- `feat/stt-multi-provider`
- `feat/openvpn-tunnel`
- `feat/hardware-rpi-aardvark-gpio`
- `feat/verifiable-intent`

这些分支名已经足够说明，上游这一段时间的工作重点主要在：

- runtime 稳定性
- channel / approval / pairing / daemon 行为
- Web / dashboard / session 体验
- 多工作区 / 多客户端方向
- 新集成能力扩张

## 主题级审阅与合并建议

### 1. 稳定性修复

代表主题：

- `fix/3460-context-window-exceeded`
- `work-issues/3533-fix-utf8-slice-panic`
- `work-issues/3544-fix-codex-sse-buffering`
- `work-issues/3628-surface-tool-failures-in-chat`
- `fix(daemon): restart current daemon after openclaw update (#3643)`

这类改动的价值：

- 直接提升线上稳定性
- 减少 agent/runtime 崩溃、超时、假死、错误不可见问题
- 和我们当前“本地常驻 daemon + 飞书会话实时交互”的使用方式高度相关

是否适合并入：

- 适合

建议方式：

- 优先做 `cherry-pick` 级审阅
- 对涉及 `src/agent/loop_.rs`、`src/channels/runtime/*` 的改动不要直接机械套
- 先对照我们本地已做修复，避免重复引入

和我们当前代码的冲突风险：

- 中到高

原因：

- 我们已经大幅改过：
  - `src/agent/loop_.rs`
  - `src/channels/runtime/*`
  - Feishu/Lark 发送与 thread 语义
- 例如 `utf8 slice panic` 这一类，我们本地已经自己修过，不能直接覆盖

结论：

- 推荐吸收，但要手工对齐，不要整段覆盖

### 2. 安全/策略/权限边界修复

代表主题：

- `work-issues/3563-fix-cron-add-nl-security`
- `work-issues/3567-allow-commands-bypass-high-risk`
- `work-issues/3568-http-request-private-hosts`
- `issue-3082-allowed-roots-direct-paths`
- `feat/verifiable-intent`

这类改动的价值：

- 直接影响工具调用边界和高风险操作策略
- 对 daemon 常驻、本地文件访问、网络请求能力很关键

是否适合并入：

- 原则上适合

建议方式：

- 必须逐项人工审阅
- 尤其是涉及：
  - 文件系统 allowed roots
  - `http_request` 私网访问
  - 高风险命令绕过策略
  - cron / 自然语言调度安全

和我们当前代码的冲突风险：

- 中

原因：

- 这类改动主要在安全边界，不一定直接撞上 Feishu/Lark 重构
- 但会影响我们本地 agent 的默认行为，必须确认不会误伤当前工作流

结论：

- 值得并，但应该作为单独一批安全补丁审阅

### 3. Channel / Runtime 行为改造

代表主题：

- `issue-2487-channel-ack-schema-v2`
- `work-issues/3487-channel-approval-manager`
- `work-issues/3262-channel-proxy-support`
- `fix/dashboard-pairing-code`
- `work-issues/3628-surface-tool-failures-in-chat`

这类改动的价值：

- 和我们当前场景最相关
- 可能改善：
  - channel ack / typing / draft
  - approval manager
  - pairing 交互
  - 代理支持
  - tool failure 在会话里可见性

是否适合并入：

- 有价值，但冲突很高

建议方式：

- 不建议直接 merge
- 建议逐功能比对
- 尤其要把以下文件作为冲突高危区域：
  - `src/channels/runtime/processing.rs`
  - `src/channels/runtime/processing_result.rs`
  - `src/channels/runtime/notify.rs`
  - `src/channels/runtime/keys.rs`
  - `src/agent/loop_.rs`

和我们当前代码的冲突风险：

- 很高

原因：

- 我们最近正好在这些位置做了大量改动：
  - 飞书/Lark 默认平铺回复
  - 不再刷话题窗口
  - 工具消息降噪
  - 本地测试附件优先吃当前会话缓存

结论：

- 值得参考
- 但适合“看着源码手工移植”，不适合直接并

### 4. Feishu / Lark 相关

代表主题：

- `issue-2494-feishu-secret-roundtrip`
- `channel approval manager`
- 以及 runtime / channel 系列中与 Lark/Feishu 相关的统一处理

这类改动的价值：

- 和我们本地当前的重点完全重合

是否适合并入：

- 有选择地参考

建议方式：

- 不建议整块并
- 因为我们本地已经做了比上游更深的飞书定制：
  - 图片/文件发送
  - 混合消息识别
  - reply/topic 平铺化
  - 当前会话缓存目录优先本地附件发送

和我们当前代码的冲突风险：

- 极高

原因：

- 我们已经重构了：
  - `src/channels/lark/*`
  - `src/tools/feishu/*`
  - `src/channels/runtime/*`

结论：

- 只适合逐点对照
- 不适合直接拉上游整段替换

### 5. Dashboard / Session / 多工作区 / Terminal UI

代表主题：

- `feature/interactive-session-state`
- `feature/terminal-ui`
- `work/multi-client-workspaces`
- `web-dashboard`
- `web-electric-dashboard`
- `fix/dashboard-pairing-code`
- `work-issues/3011-fix-dashboard-ws-protocols`

这类改动的价值：

- 对产品化体验有帮助
- 但和你当前最关心的本地飞书机器人交互主线不是同一个优先级

是否适合并入：

- 暂不建议现在合

原因：

- 体量大
- 容易把当前本地 fork 拉进另一条产品路线
- 和我们最近的目标“飞书官方能力对齐”不是同一阶段任务

结论：

- 可以后置
- 如果未来要做多客户端、多工作区、dashboard 配对体验，再单开专题评估

### 6. 新集成能力

代表主题：

- `feat/google-workspace-cli`
- `feat/microsoft365`
- `feat/stt-multi-provider`
- `feat/openvpn-tunnel`
- `feat/hardware-rpi-aardvark-gpio`

这类改动的价值：

- 能力扩张很明显
- 但都不是你当前这份 fork 的核心阻塞点

是否适合并入：

- 现在不建议

原因：

- 新依赖、新配置、新维护面
- 会增加整体复杂度
- 对飞书机器人链路没有立竿见影的收益

结论：

- 暂缓

### 7. CI / Release / Docs

代表主题：

- `ci: add API token guidance to dashboard reauth notice (#3640)`
- `fix(ci): gate X posts on canonical release events only (#3635)`
- `chore/ci-docs-cleanup`
- `fix/release-*`

这类改动的价值：

- 对上游发布流程有帮助
- 对你当前本地 daemon 使用价值不高

是否适合并入：

- 大部分不需要

结论：

- 暂不优先

## 推荐合并优先级

### A 级：建议优先挑着吸收

- `fix/3460-context-window-exceeded`
- `work-issues/3533-fix-utf8-slice-panic`
- `work-issues/3544-fix-codex-sse-buffering`
- `work-issues/3628-surface-tool-failures-in-chat`
- `fix(daemon): restart current daemon after openclaw update (#3643)`
- `fix/dashboard-pairing-code`

原因：

- 这些都偏修复类
- 直接提升稳定性或错误可见性
- 符合你现在的使用场景

### B 级：值得看，但要人工对齐

- `issue-2487-channel-ack-schema-v2`
- `work-issues/3487-channel-approval-manager`
- `work-issues/3262-channel-proxy-support`
- `issue-2494-feishu-secret-roundtrip`
- `work-issues/3563-fix-cron-add-nl-security`
- `work-issues/3568-http-request-private-hosts`

原因：

- 很可能有价值
- 但和我们当前 channel/runtime/security 改动交叉很多

### C 级：建议暂缓

- `feature/interactive-session-state`
- `feature/terminal-ui`
- `work/multi-client-workspaces`
- `feat/google-workspace-cli`
- `feat/microsoft365`
- `feat/stt-multi-provider`
- `feat/openvpn-tunnel`
- `feat/hardware-rpi-aardvark-gpio`
- 各类 dashboard / electric UI / release / 社媒自动化提交

原因：

- 不是当前 fork 的核心目标
- 合入收益低于风险

## 和我们当前本地 fork 的冲突热区

以下区域如果吸收上游改动，极可能冲突：

- `src/agent/loop_.rs`
- `src/channels/runtime/*`
- `src/channels/lark/*`
- `src/tools/feishu/*`
- `docs/plans/2026-03-13-feishu-plugin-parity-plan.zh-CN.md`
- `AGENTS.md`

冲突原因：

- 我们本地已经深改这些区域
- 上游近期也正好在 runtime / channel / pairing / dashboard / tool failure 上持续迭代

因此建议：

- 不要直接 `merge upstream/master`
- 采用“专题式吸收”

## 建议的实际操作顺序

### 第一步：先吸收修复类

优先检查并手工对齐：

- context window exceeded
- utf8 slice panic
- codex SSE buffering
- tool failure surface
- daemon restart / pairing code 修复

### 第二步：再看安全边界

- cron 安全
- private host HTTP 限制
- allowed roots / direct paths

### 第三步：最后再看 channel/runtime 改造

- ack schema
- approval manager
- channel proxy support
- Feishu secret roundtrip

## 我的结论

如果目标是：

- 保持你现在这份本地 fork 可用
- 不破坏我们刚完成的飞书/Lark 定制
- 同时吃到上游近期真正有价值的修复

那最合理的策略是：

- 不直接 merge upstream/master
- 先挑 A 级修复类改动手工吸收
- B 级改动逐专题比对
- C 级全部后置

一句话结论：

- 上游最近确实更新很多
- 但这是一波“适合挑着拿，不适合整锅端”的增量

## 参考来源

- 上游仓库：
  - https://github.com/zeroclaw-labs/zeroclaw
- 上游 compare 统计：
  - https://api.github.com/repos/zeroclaw-labs/zeroclaw/compare/8d7abb73e74bdf65f37fb7640215154c243bd236...220745e217399421d4e9120ceedb1717c8d0e72e
- 上游提交页：
  - https://github.com/zeroclaw-labs/zeroclaw/commits/master
