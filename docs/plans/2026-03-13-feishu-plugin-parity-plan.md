# Feishu Plugin Full Parity Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Recreate the official `@larksuite/openclaw-lark` plugin feature set inside Rust-based `zeroclaw` with explicit parity checkpoints and rollout-safe phases.

**Architecture:** Keep the existing native Rust channel model as the host runtime, then add a Feishu/Lark parity layer in three strata: channel/runtime behavior, rich messaging/media behavior, and tool/OAuth/admin surfaces. Reuse existing `src/channels/lark.rs`, `src/channels/mod.rs`, `src/config/schema.rs`, `src/tools/*`, `src/onboard/*`, `src/doctor/*`, and `src/security/*` extension points instead of introducing a Node bridge.

**Tech Stack:** Rust, Tokio, reqwest, serde/serde_json, existing ZeroClaw channel/tool/runtime abstractions, Feishu/Lark Open Platform HTTP/WebSocket APIs.

---

## Scope Baseline

Official reference package examined: `@larksuite/openclaw-lark@2026.3.12`

Primary official module groups:

- `src/channel/*`
- `src/core/*`
- `src/messaging/inbound/*`
- `src/messaging/outbound/*`
- `src/card/*`
- `src/tools/*`
- `src/commands/*`

Primary local Rust landing zones:

- `src/channels/lark.rs`
- `src/channels/mod.rs`
- `src/channels/traits.rs`
- `src/config/schema.rs`
- `src/tools/*`
- `src/onboard/*`
- `src/doctor/*`
- `src/security/*`

## Parity Matrix

### A. Channel Core

Official modules:

- `src/channel/plugin.js`
- `src/channel/event-handlers.js`
- `src/channel/chat-queue.js`
- `src/channel/monitor.js`
- `src/channel/onboarding*.js`
- `src/core/accounts.js`
- `src/core/config-schema.js`
- `src/core/lark-client.js`
- `src/core/token-store.js`

Local mapping:

- `src/channels/lark.rs`
- `src/channels/mod.rs`
- `src/config/schema.rs`
- `src/onboard/mod.rs`
- `src/onboard/wizard.rs`
- `src/doctor/mod.rs`

Current status:

- Partial.
- We already have native Lark/Feishu channel config, allowlist, webhook/websocket receive mode, tenant token refresh, named Feishu accounts, and basic health behavior.
- We do not have official-style account directory management, onboarding migration flow, channel monitor/diagnose parity, or plugin-style lifecycle hooks.

Priority: P0

Gap summary:

- No explicit multi-account directory/runtime model matching official plugin accounts.
- No official-style onboarding state machine and migration path.
- No channel monitor/probe/doctor equivalent dedicated to Feishu.
- No config adapter layer that isolates legacy vs current config shape cleanly.

### B. Messaging Inbound

Official modules:

- `src/messaging/inbound/parse.js`
- `src/messaging/inbound/handler.js`
- `src/messaging/inbound/dispatch*.js`
- `src/messaging/inbound/media-resolver.js`
- `src/messaging/inbound/policy.js`
- `src/messaging/inbound/permission.js`
- `src/messaging/inbound/reaction-handler.js`
- `src/messaging/inbound/user-name-cache.js`
- `src/messaging/converters/*`

Local mapping:

- `src/channels/lark.rs`
- `src/channels/traits.rs`
- `src/agent/loop_.rs`

Current status:

- Low.
- We handle text/post receive, mention gating, dedupe in websocket mode, and auto-ack reactions.
- We do not parse most rich message kinds.
- We do not download inbound image/file/audio/video resources.
- We do not preserve official-style message context structure, dispatch context, or permission gate effects.

Priority: P0

Gap summary:

- Missing converters for image, file, audio, video, system, sticker, location, share, vote, todo, merge-forward, interactive card.
- Missing structured media payload injection.
- Missing inbound reply/thread metadata handling parity.
- Missing reaction event ingestion.
- Missing richer policy and permission enforcement model.

### C. Messaging Outbound

Official modules:

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

Local mapping:

- `src/channels/lark.rs`
- `src/channels/traits.rs`
- `src/channels/mod.rs`

Current status:

- Partial.
- We support plain text sends, image upload/send, file upload/send, and add-reaction for message acknowledgements.
- We guide agents to use `[IMAGE:]` and `[DOCUMENT:]` markers.
- We do not have a unified send action for text/card/media/reply/thread.
- We do not support official-style URL/file URL/buffer media sources, delete/unsend, list/remove reactions parity, forward, typing indicators, or chat management parity.

Priority: P0

Gap summary:

- No unified outbound request model.
- Marker-based attachment flow is narrower than official media pipeline.
- Missing failure fallback behavior for media sends.
- Missing receive_id normalization and thread inheritance parity.
- Missing audio/video/media msg types.
- Missing outbound reaction management beyond ack helper.

### D. Cards And Rich Interaction

Official modules:

- `src/card/*`
- `src/messaging/converters/interactive/*`

Local mapping:

- No dedicated Lark card subsystem yet.
- Possible landing zones: `src/channels/lark.rs`, new `src/channels/lark_cards.rs`, `src/agent/*`, observer plumbing in `src/channels/mod.rs`.

Current status:

- Missing.
- Local Lark implementation does not support interactive cards, streaming cards, reply dispatchers, markdown/card styling, unavailable guards, or card reply modes.

Priority: P1

Gap summary:

- No card builder or card payload abstraction.
- No stream update controller for progressive responses.
- No confirmation-card UX for sensitive operations.
- No graceful degrade path when cards are unavailable.

### E. Tool Surface: IM / Docs / Drive / Wiki / Bitable / Sheets / Calendar / Task / Search

Official modules:

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

Local mapping:

- `src/tools/*`
- `src/tools/schema.rs`
- `src/tools/traits.rs`
- `src/providers/*` and `src/agent/loop_.rs` for tool registration/use

Current status:

- Mostly missing as Feishu-specific first-class tools.
- Core ZeroClaw already has a generic tool framework, so the host surface exists.
- There is no Feishu-specific OAPI tool family in the Rust tree today.

Priority: P1

Gap summary:

- No native Feishu docs/wiki/drive/search/calendar/task/bitable/sheets tools.
- No IM read/resource tool parity.
- No MCP doc helper parity.
- No tool-scope bridge for Feishu capabilities.

### F. Auth / OAuth / Scope / Security

Official modules:

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

Local mapping:

- `src/auth/*`
- `src/security/*`
- `src/tools/*`
- `src/onboard/*`

Current status:

- Low for Feishu-specific parity.
- We have generic auth and security infrastructure, but not the Feishu-specific scope model, OAuth UX, owner policy, or permission introspection flow exposed by the official plugin.

Priority: P1

Gap summary:

- No Feishu OAuth/device-flow user journey.
- No Feishu tool-scope registration and enforcement layer.
- No owner-policy parity.
- No permission URL / auth-error guidance flow.

### G. Commands / Ops / Diagnostics

Official modules:

- `src/commands/auth.js`
- `src/commands/doctor.js`
- `src/commands/diagnose.js`
- `src/channel/monitor.js`
- `src/channel/probe.js`

Local mapping:

- `src/doctor/mod.rs`
- `src/main.rs`
- `src/health/mod.rs`
- `src/channels/lark.rs`

Current status:

- Partial generic runtime diagnostics exist, but Feishu-specific operational tooling is not at official plugin parity.

Priority: P2

Gap summary:

- No dedicated Feishu doctor/diagnose command suite.
- No scope/auth/media/config-specific health probes.
- No operator-facing troubleshooting path equivalent to official plugin.

## Recommended Delivery Phases

### Phase 1: Lark Channel Core Parity

Target:

- Stable multi-account config model
- inbound/outbound unified message pipeline
- structured message context
- official-style target/reply/thread normalization

Files to modify first:

- `src/channels/lark.rs`
- `src/channels/mod.rs`
- `src/config/schema.rs`

Exit criteria:

- Text, post, image, file, audio/video placeholders, reply/thread routing, reaction management, and receive/send normalization all work through one coherent Rust channel surface.

### Phase 2: Rich Media And Card Parity

Target:

- inbound media download
- outbound media source normalization
- streaming cards
- interactive cards
- graceful degrade behavior

Likely files:

- `src/channels/lark.rs`
- new `src/channels/lark_cards.rs`
- new `src/channels/lark_media.rs`
- `src/channels/traits.rs`

Exit criteria:

- A user can send or receive rich media and card-based interactions without temporary scripts.

### Phase 3: Feishu Native Tool Surface

Target:

- IM read/resource tools
- docs/wiki/drive/bitable/sheets/calendar/task/search tools

Likely files:

- new `src/tools/feishu_*`
- `src/tools/mod.rs`
- `src/tools/schema.rs`

Exit criteria:

- Rust `zeroclaw` exposes a Feishu-native tool family roughly matching official plugin breadth.

### Phase 4: Auth / Scope / Onboarding / Diagnostics

Target:

- OAuth/device flow
- scope management
- onboarding and migration
- doctor/diagnose parity

Likely files:

- `src/auth/*`
- `src/security/*`
- `src/onboard/*`
- `src/doctor/mod.rs`
- `src/main.rs`

Exit criteria:

- Feishu setup, auth repair, permission inspection, and operator diagnostics can be handled in-product without manual code surgery.

## Verification Checklist

- Code-to-spec parity checklist exists per phase.
- For each phase, add Rust tests covering:
  - happy path
  - auth refresh path
  - unsupported payload path
  - retry / reconnect path
  - permission denied path
- Maintain a running “official behavior differences” section until fully closed.

## Initial Gap Count

- P0 gaps: Channel Core, Messaging Inbound, Messaging Outbound
- P1 gaps: Cards/Rich Interaction, Feishu Tool Surface, Auth/Scope/Security
- P2 gaps: Commands/Ops/Diagnostics

## Recommended Immediate Next Task

Build a detailed sub-plan for **Phase 1: Lark Channel Core Parity**, because every later phase depends on a unified channel/runtime/message model.

## Executable Task Breakdown

### Phase 1.1: Multi-Account Runtime Model

Status: In progress, base runtime identity model landed.

**Files:**

- Modify: `src/channels/lark.rs`
- Modify: `src/config/schema.rs`
- Modify: `src/channels/mod.rs`
- Test: `src/channels/lark.rs`

**Step 1: Audit current account-related fields and constructors**

Run:

```bash
rg -n "from_feishu_config|from_named_feishu_config|feishu_accounts|app_id|app_secret|channel_name" src/channels/lark.rs src/channels/mod.rs src/config/schema.rs
```

Expected:

- Existing named Feishu account support is present but runtime behavior is mostly per-channel-instance, not account-directory driven.

**Step 2: Add an explicit internal account identity model**

Implement:

- Stable `account_id` field inside the Lark channel runtime.
- Clear distinction between platform kind (`lark` vs `feishu`) and runtime account identity.
- Helper methods for “default account” vs “named account”.

**Step 3: Write tests for account identity behavior**

Add tests for:

- default Feishu account naming
- named Feishu account naming
- named account routing consistency
- no accidental fallback from one account to another

**Step 4: Add runtime helpers for account-scoped resources**

Implement:

- account-scoped token cache keys
- account-scoped bot open_id cache keys
- account-scoped dedupe/heartbeat naming

**Step 5: Verify**

Run:

```bash
cargo test channels::lark::tests:: --lib
```

### Phase 1.2: Unified Inbound Message Context

Status: In progress, internal parsed message model landed.

**Files:**

- Modify: `src/channels/lark.rs`
- Modify: `src/channels/traits.rs`
- Test: `src/channels/lark.rs`

**Step 1: Define the target inbound parity shape**

Implement an internal message parse result carrying:

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

**Step 2: Refactor current text/post parsing into the new parse result**

Goal:

- move existing ad hoc text/post parsing into one internal parse pipeline
- preserve current behavior while preparing for image/file/audio/video/interactive additions

**Step 3: Add tests for text/post parity preservation**

Test:

- DM text
- group text with mention rules
- post message with mention extraction
- malformed content JSON

**Step 4: Thread metadata preservation**

Implement:

- retain `root_id`, `parent_id`, `thread_id`
- pass through to the internal message object even if the agent surface does not use all fields yet

**Step 5: Verify**

Run:

```bash
cargo test lark_parse_ --lib
```

### Phase 1.3: Inbound Rich Message Converters

Status: In progress, image/file/audio/video placeholder/resource extraction landed.

**Files:**

- Modify: `src/channels/lark.rs`
- Create: `src/channels/lark_inbound.rs`
- Test: `src/channels/lark.rs`

**Step 1: Extract converter helpers from `lark.rs`**

Create converter-oriented helpers for:

- text
- post
- image
- file
- audio
- video
- interactive
- unsupported fallback

**Step 2: Implement image/file resource descriptor extraction**

Goal:

- parse `image_key` / `file_key` and related metadata from inbound message payloads
- do not download yet in this task, only normalize into resource descriptors

**Step 3: Implement placeholder policy**

Add a consistent placeholder mapping:

- image -> `<media:image>`
- file -> `<media:document>`
- audio -> `<media:audio>`
- video -> `<media:video>`

**Step 4: Add tests for non-text message ingestion**

Test:

- image event no longer skipped
- file event no longer skipped
- unsupported type remains controlled and explicit

**Step 5: Verify**

Run:

```bash
cargo test lark_parse_non_text --lib
```

### Phase 1.4: Inbound Media Download And Local Persistence

Status: In progress, webhook/WS path can now materialize inbound media into workspace-scoped channel storage when workspace storage is available.

**Files:**

- Modify: `src/channels/lark.rs`
- Create: `src/channels/lark_media.rs`
- Modify: `src/channels/mod.rs`
- Test: `src/channels/lark.rs`

**Step 1: Introduce Feishu message resource download helpers**

Implement download functions for:

- message image resource
- message file resource

using message id + resource key + resource type.

**Step 2: Add media save path policy**

Implement:

- dedicated inbound media storage layout
- size guardrails
- MIME detection fallback
- file name preservation when available

**Step 3: Attach local media paths to inbound message data**

Minimum acceptable parity:

- inject `[IMAGE:/abs/path]` and `[DOCUMENT:/abs/path]` into normalized content

Preferred parity:

- preserve structured media metadata alongside text

**Step 4: Add tests for inbound media save behavior**

Test:

- image download success
- file download success
- oversized or invalid media failure path

**Step 5: Verify**

Run:

```bash
cargo test channels::lark::tests:: --lib
```

### Phase 1.5: Unified Outbound Request Model

Status: In progress, internal outbound request normalization landed while preserving current marker/path behavior.

**Files:**

- Modify: `src/channels/lark.rs`
- Create: `src/channels/lark_outbound.rs`
- Test: `src/channels/lark.rs`

**Step 1: Define an internal outbound request struct**

Fields:

- target
- text
- card
- media inputs
- file name
- reply_to_message_id
- reply_in_thread
- account_id

**Step 2: Refactor current `Channel::send` logic to build the outbound request**

Goal:

- keep current marker support
- route all send behavior through one normalization function

**Step 3: Add tests for request normalization**

Test:

- text only
- text + image marker
- path-only image
- text + file marker
- unresolved marker fallback

**Step 4: Verify**

Run:

```bash
cargo test lark_parse_attachment_ --lib
```

### Phase 1.6: Target / Reply / Thread Normalization

**Files:**

- Modify: `src/channels/lark.rs`
- Modify: `src/channels/mod.rs`
- Test: `src/channels/lark.rs`

**Step 1: Add target normalization helper**

Implement:

- normalize target id
- infer receive id style if needed
- separate chat target from reply target

**Step 2: Add reply and thread fields to outbound flow**

Implement:

- reply-to message id send path
- thread reply send path
- current-thread inheritance where available from channel context

**Step 3: Add tests**

Test:

- normal chat send
- reply send
- thread reply send
- named Feishu account reply path

**Step 4: Verify**

Run:

```bash
cargo test lark_build_ --lib
```

### Phase 1.7: Outbound Media Source Normalization

**Files:**

- Modify: `src/channels/lark.rs`
- Modify: `src/channels/lark_media.rs`
- Test: `src/channels/lark.rs`

**Step 1: Add media input variants**

Support:

- absolute local path
- `file://` URL
- remote URL

**Step 2: Add safe local path validation**

Implement:

- only approved roots or explicitly allowed local paths
- reject ambiguous relative paths

**Step 3: Add remote fetch path for media uploads**

Implement:

- remote URL download
- file name derivation
- MIME/type inference before upload

**Step 4: Add tests**

Test:

- local path media
- file URL media
- remote URL media
- invalid URL / unsupported scheme

**Step 5: Verify**

Run:

```bash
cargo test channels::lark::tests:: --lib
```

### Phase 1.8: Outbound Media Message Types

**Files:**

- Modify: `src/channels/lark.rs`
- Modify: `src/channels/lark_media.rs`
- Test: `src/channels/lark.rs`

**Step 1: Split send helpers by media type**

Implement:

- send image
- send file
- send audio
- send video

**Step 2: Split upload helpers by media classification**

Implement:

- image upload
- file upload
- file type mapping for audio/video/general file

**Step 3: Add tests**

Test:

- image payload uses `image_key`
- file payload uses `file_key`
- audio/video classification routes correctly

**Step 4: Verify**

Run:

```bash
cargo test lark_build_ --lib
```

### Phase 1.9: Reaction Management Parity

**Files:**

- Modify: `src/channels/lark.rs`
- Modify: `src/channels/traits.rs`
- Test: `src/channels/lark.rs`

**Step 1: Promote reaction helpers beyond ack-only**

Implement:

- add reaction
- remove reaction
- optional reaction listing primitive if needed for removal flow

**Step 2: Wire trait-level reaction support for Lark**

Goal:

- Lark should use the generic channel reaction methods instead of private ack-only behavior

**Step 3: Keep locale-based ack reaction as a policy layer**

Do:

- preserve current ack UX
- move it on top of the generic reaction capability

**Step 4: Add tests**

Test:

- add reaction success path
- remove reaction success path
- ack reaction still works

**Step 5: Verify**

Run:

```bash
cargo test lark_reaction_ --lib
```

### Phase 1.10: Channel-Level Health / Probe / Diagnostics

**Files:**

- Modify: `src/channels/lark.rs`
- Modify: `src/doctor/mod.rs`
- Modify: `src/health/mod.rs`
- Test: `src/channels/lark.rs`

**Step 1: Add Feishu/Lark-specific probe helpers**

Check:

- token fetch
- websocket connectivity
- configured account completeness
- bot identity resolution

**Step 2: Expose probe results through doctor/health surfaces**

Goal:

- operator can see whether failure is auth, config, transport, or permission related

**Step 3: Add tests**

Test:

- missing app id/app secret
- invalid token path
- bot identity unresolved

**Step 4: Verify**

Run:

```bash
cargo test channels::lark::tests:: --lib
```

### Phase 2.1: Card Payload Abstraction

**Files:**

- Create: `src/channels/lark_cards.rs`
- Modify: `src/channels/lark.rs`
- Test: `src/channels/lark.rs`

**Step 1: Introduce internal card model**

Support:

- static card payload
- reply card payload
- streaming/updatable card shell

**Step 2: Add outbound send path for cards**

Goal:

- card send no longer piggybacks on text message assumptions

**Step 3: Add tests**

Test:

- simple card payload serialization
- reply card routing

### Phase 2.2: Streaming Card Controller

**Files:**

- Modify: `src/channels/lark_cards.rs`
- Modify: `src/channels/mod.rs`
- Modify: `src/agent/loop_.rs`

**Step 1: Define streaming state machine**

States:

- thinking
- generating
- completed
- failed

**Step 2: Attach streaming updates to agent reply lifecycle**

Goal:

- long-running replies update in place where supported

**Step 3: Add tests / simulation hooks**

### Phase 2.3: Interactive Card Replies And Confirmation Flow

**Files:**

- Modify: `src/channels/lark_cards.rs`
- Modify: `src/channels/lark.rs`
- Modify: `src/tools/*` where confirmation semantics are needed

**Step 1: Define interactive callback payload handling**

**Step 2: Map interactive responses into channel events**

**Step 3: Add confirmation-card flow for sensitive operations**

### Phase 2.4: Unavailable / Degrade Guard

**Files:**

- Modify: `src/channels/lark_cards.rs`
- Modify: `src/channels/lark.rs`

**Step 1: Detect card unavailability**

**Step 2: Fallback to plain text while preserving task outcome**

### Phase 3.1: Feishu IM Tool Family

**Files:**

- Create: `src/tools/feishu_im_read.rs`
- Create: `src/tools/feishu_im_message.rs`
- Create: `src/tools/feishu_im_resource.rs`
- Modify: `src/tools/mod.rs`
- Modify: `src/tools/schema.rs`

**Step 1: Add read-message capability**

**Step 2: Add send/manage message capability**

**Step 3: Add message resource capability**

### Phase 3.2: Docs / Wiki / Drive Tool Family

**Files:**

- Create: `src/tools/feishu_doc_create.rs`
- Create: `src/tools/feishu_doc_fetch.rs`
- Create: `src/tools/feishu_doc_update.rs`
- Create: `src/tools/feishu_drive_file.rs`
- Create: `src/tools/feishu_wiki_space.rs`
- Modify: `src/tools/mod.rs`

**Step 1: Document CRUD parity**

**Step 2: Drive file access parity**

**Step 3: Wiki navigation parity**

### Phase 3.3: Bitable / Sheets Tool Family

**Files:**

- Create: `src/tools/feishu_bitable_*.rs`
- Create: `src/tools/feishu_sheets_*.rs`
- Modify: `src/tools/mod.rs`

**Step 1: Bitable app/table/field/record/view surfaces**

**Step 2: Sheets read/write surfaces**

### Phase 3.4: Calendar / Task / Search Tool Family

**Files:**

- Create: `src/tools/feishu_calendar_*.rs`
- Create: `src/tools/feishu_task_*.rs`
- Create: `src/tools/feishu_search_*.rs`
- Modify: `src/tools/mod.rs`

**Step 1: Calendar CRUD and attendee management**

**Step 2: Task / tasklist / comment / subtask parity**

**Step 3: Search parity**

### Phase 4.1: OAuth And Device Flow

**Files:**

- Create: `src/auth/feishu_oauth.rs`
- Modify: `src/auth/mod.rs`
- Modify: `src/onboard/*`
- Modify: `src/main.rs`

**Step 1: Implement Feishu-specific OAuth/device flow client**

**Step 2: Add user-facing onboarding flow**

**Step 3: Add auth recovery path**

### Phase 4.2: Scope Management And Permission Checks

**Files:**

- Create: `src/security/feishu_scopes.rs`
- Modify: `src/security/mod.rs`
- Modify: `src/tools/*`

**Step 1: Define Feishu scope registry**

**Step 2: Map tools to scope requirements**

**Step 3: Add preflight permission checks**

### Phase 4.3: Owner Policy And Safe Defaults

**Files:**

- Create: `src/security/feishu_owner_policy.rs`
- Modify: `src/security/mod.rs`
- Modify: `src/channels/lark.rs`

**Step 1: Define owner/user policy rules**

**Step 2: Enforce DM/group safe defaults**

### Phase 4.4: Onboarding Migration Parity

**Files:**

- Modify: `src/onboard/mod.rs`
- Modify: `src/onboard/wizard.rs`
- Modify: `src/config/schema.rs`

**Step 1: Add Feishu-specific onboarding prompts**

**Step 2: Add legacy-to-current migration helpers**

### Phase 5.1: Doctor / Diagnose Commands

**Files:**

- Modify: `src/doctor/mod.rs`
- Modify: `src/main.rs`
- Create: `src/doctor/feishu.rs`

**Step 1: Add Feishu doctor checks**

**Step 2: Add Feishu diagnose output**

### Phase 5.2: Parity Test Matrix

**Files:**

- Create: `tests/feishu_parity/*.rs` or extend channel tests

**Step 1: Build a per-feature parity checklist**

**Step 2: Add regression tests by phase**

**Step 3: Track remaining intentional differences**
