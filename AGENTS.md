# CLAUDE.md — ZeroClaw

## 命令

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

本地构建规则：

```bash
./cargo-build-clean.sh
```

- 在本仓库里执行本地 `cargo build` 时，默认必须使用 `./cargo-build-clean.sh`，不要直接运行裸 `cargo build`。
- 如果需要传递构建参数，也必须透传给脚本，例如 `./cargo-build-clean.sh --release`、`./cargo-build-clean.sh --features rag-pdf`。
- 该脚本会在构建成功后自动清理 `target/<profile>/incremental` 和 `target/<profile>/deps` 中最占空间的可回收产物，避免仓库体积持续暴涨。

完整的 PR 前验证（推荐）：

```bash
./dev/ci.sh all
```

如果只是文档改动：运行 Markdown lint 和链接完整性检查。若修改了 bootstrap 脚本：运行 `bash -n install.sh`。

## 项目概览

ZeroClaw 是一个以 Rust 为核心的一体化自治 Agent 运行时，重点关注性能、效率、稳定性、可扩展性、可持续性与安全性。

核心架构采用 trait 驱动的模块化设计。扩展能力时，应通过实现 trait 并在对应 factory 模块中注册来接入。

关键扩展点：

- `src/providers/traits.rs`（`Provider`）
- `src/channels/traits.rs`（`Channel`）
- `src/tools/traits.rs`（`Tool`）
- `src/memory/traits.rs`（`Memory`）
- `src/observability/traits.rs`（`Observer`）
- `src/runtime/traits.rs`（`RuntimeAdapter`）
- `src/peripherals/traits.rs`（`Peripheral`）—— 硬件板卡支持（STM32、RPi GPIO）

## 仓库结构

- `src/main.rs` —— CLI 入口与命令分发
- `src/lib.rs` —— 模块导出与共享命令枚举
- `src/config/` —— 配置 schema 与配置加载/合并
- `src/agent/` —— 编排主循环
- `src/gateway/` —— webhook / gateway 服务
- `src/security/` —— 安全策略、配对、密钥存储
- `src/memory/` —— markdown/sqlite memory 后端与 embedding/vector 合并
- `src/providers/` —— 模型 provider 及其容错包装
- `src/channels/` —— Telegram/Discord/Slack 等渠道接入
- `src/tools/` —— 工具执行面（shell、file、memory、browser）
- `src/peripherals/` —— 硬件外设（STM32、RPi GPIO）
- `src/runtime/` —— 运行时适配层（当前为 native）
- `docs/` —— 主题化文档（setup-guides、reference、ops、security、hardware、contributing、maintainers）
- `.github/` —— CI、模板、自动化工作流

## 风险分级

- **低风险**：仅文档 / chore / 测试改动
- **中风险**：大多数 `src/**` 行为改动，但不涉及边界或安全影响
- **高风险**：`src/security/**`、`src/runtime/**`、`src/gateway/**`、`src/tools/**`、`.github/workflows/**`、访问控制边界相关改动

如果不确定，按更高风险等级处理。

## 工作流程

1. **先读后写** —— 修改前必须先检查现有模块、factory 接线以及相邻测试。
2. **一个 PR 只解决一个问题** —— 不要把功能、重构、基础设施变更混在一起。
3. **最小化补丁** —— 不做预防式抽象；没有明确用途就不要新增配置项。
4. **按风险等级验证** —— 文档改动做轻量检查；代码改动做完整、相关的检查。
5. **记录影响面** —— 更新 PR 说明，写清行为变化、风险、副作用和回滚方式。
6. **保持队列卫生** —— 堆叠 PR 要声明 `Depends on #...`；替换旧 PR 要声明 `Supersedes #...`。

分支 / 提交 / PR 规则：

- 必须从非 `master` 分支开展工作。PR 目标分支必须是 `master`；不要直接推送到 `master`。
- 使用 conventional commit 标题。优先保持 PR 小而清晰（`size: XS/S/M`）。
- 完整遵循 `.github/pull_request_template.md`。
- 严禁提交 secrets、个人数据或真实身份信息（参见 `@docs/contributing/pr-discipline.md`）。

## 代码组织规则

- 单个源码文件原则上**不得超过 500 行**。
- 一旦文件接近或超过 500 行，应主动拆分，不要继续把新逻辑堆进同一个文件。
- 拆分时优先按模块职责拆，不要按随意的工具函数堆砌方式拆。
- 相同模块的文件必须放到同一个文件夹下面管理；不要把同一模块的多个文件散落在不同目录。
- 同一个文件夹下面不要混放大量不同模块的文件；只要已经出现明显的职责分层，就必须建立子目录分类。
- 新增目录时，目录名必须体现模块语义，而不是临时性或个人命名习惯。
- 重构目录结构时，优先保持“模块边界清晰”而不是“改动最少”。

## 反模式

- 不要为了小便利引入重量级依赖。
- 不要静默削弱安全策略或访问约束。
- 不要为了“以后可能会用”加入猜测性的配置或 feature flag。
- 不要把大规模纯格式化改动和功能改动混在一起。
- 不要顺手修改与当前任务无关的模块。
- 不要在没有明确说明的情况下绕过失败的检查。
- 不要在重构提交里隐藏行为变化副作用。
- 不要在测试数据、示例、文档或提交中包含个人身份信息或敏感信息。

## 关联参考

- `@docs/contributing/change-playbooks.md` —— 新增 provider、channel、tool、peripheral 的变更套路；安全 / gateway 变更边界
- `@docs/contributing/pr-discipline.md` —— 隐私规则、被替换 PR 的标注模板、交接模板
- `@docs/contributing/docs-contract.md` —— 文档系统契约、i18n 规则、locale 一致性要求
