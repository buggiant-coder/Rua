# ADR 0016: Channel Trait、实现隔离与 Feature Gates

**Status:** Accepted

## Context

Channel 将 DingTalk、Feishu、Telegram、Discord、OneBot、QQ、Mattermost、
WeChat、WeCom、Slack、SIP、Xiaoyi、Yuanbao、Matrix、MQTT 和 macOS 上的
iMessage 等当前已组合平台接入同一个 Agent/Conversation runtime。
iMessage 通过显式配置、feature 透传和 service registration 接入
workspace/server，仍受 macOS target 限制。

这些实现共享消息模型、排队、rendering、approval presentation、
slash priority 和 Process callback，但各自拥有不同 SDK、认证、连接生命周期
和 target semantics。

当前 Channel 子系统采用“多 Channel、单 implementation crate”结构：
`rua-channel` 同时提供共享 Channel domain 和各平台实现；
`rua-command` 提供 slash execution，`rua-workspace` 组合实例，
`rua-server` 暴露管理与 Console routes。各外部 Channel implementation
位于 `crates/rua-channel/src/<channel>/` 的隔离模块；旧
`rua-channel-core` 与 `rua-channel-*` sibling crates 已经合并并删除。

因此当前物理边界也是本决策的一部分：新增 Channel 默认进入
`rua-channel` 的独立 module 和 feature，不重新创建 per-platform crate。

## Decision

### 1. Channel domain 与平台实现由单一 `rua-channel` crate 拥有

`crates/rua-channel/src/core/` 保存共享 Channel domain：

- inbound/outbound content types；
- `ChannelId`、target 与 request；
- Channel/Process traits；
- ChannelManager；
- queue、streaming、renderer；
- approval card 与 command priority support；
- Console service 与终端输出适配。

外部业务 crate 依赖该公开 domain，不直接依赖 DingTalk/Telegram 等模块的
私有 transport/client details。

平台实现位于同一 crate 不代表共享内部状态。Rust module privacy、feature
gates 和 Channel/Process boundary 仍是实现隔离手段。

### 2. Channel 与 Process 分离 transport 和业务处理

唯一的 `core::channel::Channel`（同时从 `core::Channel` 导出）负责：

- identity；
- 使用注入的 `Arc<dyn Process>` 启动；
- stop；
- send；
- native payload 到 `AgentChannelRequest` 的转换；
- 可选 token/webhook refresh。

`Process` 只负责把标准化 `AgentChannelRequest` 转成
`BoxStream<RuntimeEvent>`。Channel 不拥有 Agent loop，Process 不依赖
平台 SDK。

这允许同一业务 Process 被多个 Channel 使用，也允许 Channel contract tests
使用 fake Process。

### 3. 单一 Channel 契约，不保留历史双轨

`ChannelManager` 通过 `register` 注册唯一 `Arc<dyn Channel>` 实现及其生命周期状态。
旧 `core::traits::Channel`、`AnyChannel::{Legacy, Modern}` 和旧注册入口已删除；
平台与 Voice 共用同一契约，调用方不得重新增加历史接口适配层。

未接入生产的旧 BrowserChannel 和 ConsoleChannel 对象已删除。
生产 Console Chat 继续使用 `ConsoleChannelService`，不是外部平台 Channel；
不能将两者混同或删除当前 Console 的请求、媒体及事件处理能力。

生命周期和刷新结果由 Manager 统一维护，不恢复旧 health/doctor trait。
注册不等于启动：存在 Channel 时，启动必须提供 Process。启动、停止对所有
实例执行后汇总错误，单个平台失败不会跳过其它平台。失败实例先停止再重启；
运行实例不能直接替换。重复启停不会重复启动实例或刷新任务。

刷新任务只处理运行实例，停止时取消并等待任务结束；重启创建新的取消令牌。
HTTP 查询直接使用 Manager 的 registered/starting/running/stopping/stopped/failed
状态；刷新错误表示运行中的 degraded，不等于停止。start 成功只证明启动完成，
不证明第三方网络已连通，缺少独立观测的 transport 状态为 unknown。
Cron 的 `stream` 模式逐事件交付可见内容，由共享 MessageRenderer 转成
OutgoingMessage 后调用 Channel.send，不保证平台端原位编辑消息。`final` 模式
只在成功结束后发送一次收集到的正文；审批提示和错误不等待正文完成。运行错误、
取消和发送失败不得计作成功，不把部分正文作为 final 交付。

Console Cron 通知直接写入现有推送收件箱，不依赖旧 ConsoleChannel；
`save_result_to_inbox` 只为外部 Channel 额外保存结果，避免 Console 重复入箱。
已支持的 Tool Guard pending 提示保留 approval metadata，发送失败向上传播。
没有对应控制通道的交互请求明确报错，不把普通通知伪装成可执行的审批入口。
无可见内容的生命周期/usage 事件不生成消息；无人调用的 broadcast_event 已移除。

### 4. 每个平台实现保持模块隔离

持久化配置使用 schema 的字段名：Matrix `homeserver`/`encryption`、MQTT `host`、
Xiaoyi `access_key`/`secret_key`。HTTP 写入和运行时解析只校验当前契约，不维护
历史字段识别或迁移规则；未知扩展字段仍保留。SIP 配置了
`bind_addr` 时必须显式选择 `sip_mode`，不能由地址字段推断 native 模式。
内部 Rust 字段名和平台 SDK 字段不因此改名。

MQTT `port` 使用 JSON 整数，按 `u16` 范围校验；仅省略字段时采用 1883。
显式 null、字符串、越界或其它类型不转换成默认端口，HTTP 写入和运行时解析
都报告当前字段错误。此规则不改变其它可选字段的缺省语义。

Channel 的非可空布尔字段只接受 true/false；省略字段采用各自声明的默认值，
显式 null 不转换为 false。HTTP 和运行时入口按当前 catalog 中的布尔默认值
校验这些字段。默认值为 null 的可选开关和未知扩展字段不被此规则改写。

展示配置统一为 `show_thinking`、`show_tool_calls`、`show_tool_results` 三个独立
正向字段；`show_tool_details` 只控制详情，不决定整条工具消息是否可见。
Workspace 将完整配置映射为 MessageRenderOptions 后传给各平台；Renderer 和 SSE
使用同一组开关。请求级 metadata 仅覆盖显式给出的项，不再推导其它开关。
普通展示开关不隐藏错误、审批或生命周期控制事件。产品配置默认值仍由 channel
catalog 解析；未装配配置的 IM renderer 保留其隐藏默认值。

每个 channel 模块拥有自己的：

- config mapping；
- API/WebSocket/client；
- inbound parsing；
- target identity；
- outbound rendering/upload；
- auth/token refresh；
- platform-specific tests。

共享逻辑只有在至少两个实现需要且语义相同时才进入 `core/`。
不得让一个 channel 直接读取另一个 channel 的私有状态。

### 5. Cargo feature 决定编译可用性

`crates/rua-channel/Cargo.toml` 为每个平台定义 feature，
`src/lib.rs` 用对应 `#[cfg(feature = "...")]` 导出模块。
optional dependencies 必须挂在实际需要它们的 feature 下。

`rua-channel` 的 default feature 集包含主要 IM integrations；
`matrix`、`mqtt`、`imessage` 和 `sip-livekit` 为独立 feature。
`rua-workspace` 与 `rua-server` 显式透传这些 feature；
`rua-cli` 同时透传 `sip-livekit`，用于统一 server executable。
workspace 的默认 feature 集启用 `imessage`，其 implementation 和
service registration 同时受 macOS target gate 约束。Matrix/MQTT
仍需显式启用。

SIP dev/native transport 由 `sip` feature 提供；`sip-livekit` 额外引入
官方 LiveKit Rust RTC SDK 和 WebRTC native library。它不进入
`mobile-default`。Apple executable 的 build script 仅在该 feature 启用时
添加 `-ObjC`，保留 WebRTC archive 中的 Objective-C categories。
RTC room、audio resampling、ASR/TTS 和取消生命周期属于 SIP Channel；
Process 仍只接收标准 Agent request。

server/mobile profile 可以通过自己的 feature 组合裁剪 Channel
capability，不应直接用 runtime bool 模拟未编译代码。

### 6. 编译可用、配置启用和运行健康是三层状态

- Cargo feature：代码和依赖是否存在；
- Channel config：该实例是否 enabled；
- health/runtime status：当前连接是否可用。

缺少 feature 的 channel 不得出现在构造分支中。
已编译但 disabled 的 channel 不启动。
已启动但 degraded/unhealthy 的 channel 通过 health/doctor 报告，
不改变其它 channel。

### 7. Channel 只适配交互，不拥有核心 authority

Channel 可以管理本地 transport、排队、typing/stream rendering 和
approval presentation，但不拥有 Conversation Authority、Task Runtime、
Approval Authority 或 Agent config。

slash priority registry 只决定入队优先级；真正 command execution 属于
`rua-command` 与 Agent runtime。

## Hard invariants

- 外部 platform payload 必须转换成标准 `AgentChannelRequest`。
- Agent runtime 不得依赖平台 SDK 类型。
- Process 不得持有平台 transport 私有状态。
- Channel implementation 之间不得共享私有 mutable state。
- feature-disabled 模块不得被引用或拉入 optional dependency。
- 新增 Channel 默认必须作为 `rua-channel` module + Cargo feature 落地；
  未经新 ADR 不得恢复 per-platform sibling crate。
- iMessage implementation 与 workspace registration 必须同时满足
  `imessage` feature 和 macOS target；其它 target 不构造该 Channel。
- 平台和 Voice 只实现唯一 Channel trait，不增加历史接口分流。
- 单个 Channel failure 不得停止其它 Channel。
- Channel 不得成为 Conversation、Approval 或 Task authority。
- priority registry 不得执行 slash command。

## Authority, persistence, and failure boundaries

具体 Channel 拥有自己的 connection、token 和 platform callback。
ChannelManager 拥有进程内实例注册、启动、停止与 broadcast。
`rua-workspace` 拥有 Agent 级 Channel composition。
`rua-command` 拥有 slash execution。

凭据和 enablement 由配置 persistence owner 保存；Channel runtime 只消费
resolved config。transport failure 更新 health 并返回错误，不写成
Conversation terminal authority。

启动和停止必须有界。一个 Channel 的 start/stop/send 失败由 manager 聚合
或报告，但不得跳过其它独立 Channel 的生命周期。

## Consequences

- 平台代码与 Agent/Server 业务逻辑保持隔离。
- feature gates 可裁剪大型 SDK 和不适用平台实现。
- 单一契约避免重复注册、生命周期和发送适配逻辑。
- 所有 Channel implementation 集中在一个 crate，跨 crate consumer 只依赖
  该 crate 的公开 domain surface。
- 新增或横切修改只维护一个 Channel manifest，同时保留 per-platform feature。
- 新增 Channel 需要同时定义 module、feature、optional deps 和 contract tests。

## Rejected alternatives

- 让 server endpoint 直接调用各平台 SDK：会把 transport 泄漏到组合层。
- 一个包含所有平台细节的巨大 Channel trait：多数实现只能返回不支持。
- 用 runtime config 代替 Cargo feature：无法移除编译依赖和平台限定代码。
- 保留 legacy/modern 双接口：增加分支和测试负担，没有当前生产调用需求。
- 恢复 `rua-channel-core` 加 per-platform sibling crates：会重新引入多个
  manifest、linking jobs 和跨 crate public helper 成本，而当前实现规模不需要。

## Implementation and verification

当前实现路径：

- crate features：`crates/rua-channel/Cargo.toml`
- module gates：`crates/rua-channel/src/lib.rs`
- Channel/Process traits：`crates/rua-channel/src/core/channel.rs`
- health data：`crates/rua-channel/src/core/health.rs`
- manager/adapter：`crates/rua-channel/src/core/manager.rs`
- content model：`crates/rua-channel/src/core/content.rs`
- Workspace composition：
  `crates/rua-workspace/src/services/channels_adapter.rs`
- command priority：`crates/rua-channel/src/core/command_registry.rs`

验证覆盖：

- `crates/rua-channel/tests/contract_channels/feature_gated.rs`
- `crates/rua-channel/tests/contract_channels/`
- `crates/rua-channel/tests/core_pipeline/`
- 各 `unit_<channel>` 或 `<channel>_*` test target
- `scripts/tests/check-feature-flags.sh`

iMessage 的 feature/target 边界由 `crates/rua-channel/src/lib.rs`、
`crates/rua-channel/Cargo.toml` 与 workspace registration 共同约束。
`crates/rua-workspace/tests/adapter/adapter_platform.rs` 验证配置到
ChannelManager 的组合，`integration_channel_lifecycle/imessage.rs`
使用临时数据库和 fake imsg 验证跳过旧消息、收发及重启。

SIP 的 UDP/RTP/ASR/TTS、注册认证、打断、挂断和重启由
`integration_channel_lifecycle/sip.rs` 的本地协议夹具验证；
`livekit.rs` 的显式 RTC 测试连接本地 LiveKit server，覆盖真实音频收发及
房间重连。Matrix 的 live sync、加密附件、线程、会话恢复和群策略由
`tests/matrix_matrix_channel/` 验证。

Twilio Voice 的公共 URL、callback provisioning 和可选 Cloudflare
tunnel 生命周期由 server 的 `VoiceRuntime` 拥有，HTTP listener 提供实际
绑定端口；它使用现有 Voice manager，不将 tunnel/HTTP authority 放入
Process。停用只关闭它拥有的 tunnel，启动失败通过 channel health 报告。

QwenPaw channel 声明快照、Rua 显式差异与更新流程见
[catalog update workflow](../../scripts/INDEX.md#upstream-catalogs)。配置声明的同步
不取代各平台 Rust transport 的实现与验证。面向使用者的配置见
[Channel configuration](../channels.md)。

## Related decisions

- ADR 0001: Workspace 分层依赖方向与进程组合
- ADR 0007: 进程生命周期和有界分阶段关闭
- ADR 0012: Per-Agent Tool Guard 与进程级 Approval
- ADR 0017: Slash Command 多层调度与 Discovery
- ADR 0033: ACP Chat Mode, client projection, and permission relay
