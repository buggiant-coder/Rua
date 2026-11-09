# ADR 0017: Slash Command 多层调度与 Discovery

**Status:** Accepted

## Context

Slash Command 是以 `/` 开头、绕过普通 LLM 规划的用户控制输入。
不同命令需要完全不同的上下文：

- 进程状态与 reload；
- 当前 session、queue、approval 和 Skill；
- Conversation history、compaction、prompt 与 Agent internals。

IM Channel 还需要在入队前识别 `/stop` 等高优先级输入。
Web Console 需要自动补全 metadata，而不能维护一份与 backend registry
无关的硬编码列表。

旧设计在 Channel 入口维护 Claude Code 专用 pass-through 和
`/cc /nocc /cch /ccr`。当前产品已用 ACP Chat Mode 取代：
enter/exit/resume/discovery 走 Chat-scoped HTTP/Conversation Authority，
已进入 ACP mode 后的普通文本由 runtime `intercept_input` 发送给 ACP runner。

## Decision

### 1. 可见 Slash Command 分为三层

执行层按 context dependency 分为：

- Daemon Command：进程级 status、restart、reload-config、version、logs、
  approval 等；
- Control Command：当前运行控制，例如 `/stop`、`/skills`；
- Conversation Command：history、context compaction、prompt 和
  Agent-internal conversation action。

每层有独立 trait、Context 和 Registry。
命令归属由所需 authority 与 interception point 决定，不由名称或 UI 分组决定。

### 2. Runner dispatcher 使用固定优先级

`CommandDispatcher` 的匹配顺序是：

1. Daemon；
2. Control；
3. Conversation。

高 authority 的 process command 不能被未来同名 conversation command
遮蔽。unknown slash command 返回 terminal error outcome，
不进入普通 Agent loop 猜测含义。

`run_command_path` 是 runner fast path。命中非 Agent-required command 时，
直接返回 command event stream，不构造普通 ReAct execution。

### 3. Agent-required Conversation Command 使用同一 Registry

部分 Conversation Command 需要 live Agent reference。
`ConversationCommand::agent_required()` 标记这类命令。

fast path 发现该标记时返回未处理，让请求进入 Agent runtime；
Agent runtime 再使用同一个 ConversationCommandRegistry 和完整 context 执行。

不得为 agent-required commands 建第二份命令定义或 metadata registry。

### 4. ACP Chat Mode 在 slash dispatcher 之前拦截普通输入

当 Chat 已绑定 ACP runner 时，
`CommandRuntimeDispatcherAdapter::intercept_input` 在普通 command fast path
和 ReAct loop 之前检查输入。

非 reserved 输入直接通过 `AcpService::run_turn` 进入 ACP subprocess，
并映射成标准 `RuntimeEvent` stream。当前唯一 Rua-reserved command 是
`/stop`，它退回正常 Control command 路径。

ACP lifecycle 的用户入口是 Agent/Project Chat-scoped HTTP routes，
并通过 Conversation Authority 路由到实际执行 Device。
legacy `/acp`、`/noacp`、`/acph`、`/acpr` handler 类型保留用于兼容，
但不注册进默认 visible Control registry，也不进入 Discovery API。

### 5. Channel Priority Registry 只负责排队

`rua-channel::core::command_registry::CommandRegistry` 保存
`prefix -> priority_level`：

- critical 0：`/stop`；
- high 10：Daemon commands 与短 alias；
- normal 20：默认消息；
- low 30：保留级别。

它使用最长前缀并检查 token boundary，避免 `/statusx` 匹配 `/status`。
它不持有 handler，也不执行 command。

ACP enter/exit 命令不属于可见 slash queue surface。

### 6. Discovery API 从执行 Registry 构造

`GET /api/commands` 返回合并后的 `CommandEntry`：

- `name`;
- `scope`;
- `help`;
- `args_hint`;
- `agent_required`;
- `alias_of`;
- `subsystem`.

metadata 从 Conversation、Daemon 和默认 Control registries 构造并排序。
Web Console 以该 endpoint 为 slash autocomplete 的唯一 source。

Discovery 只列 visible command。ACP Chat Mode 的 HTTP lifecycle controls
和 hidden legacy slash handlers 不得泄漏到列表。

### 7. Command outcome 是短路结果

Command 返回 `CommandOutcome`，包含 events、`last` 与 typed outcome kind。
普通成功、错误、ephemeral response 和 clear-history intent 均通过这一结构
返回。命令错误不应作为 transport error 使 Channel 无响应。

## Hard invariants

- 所有可见 slash command 恰好属于 Daemon、Control、Conversation 一层。
- dispatcher 顺序固定为 Daemon、Control、Conversation。
- agent-required command 必须复用同一 Conversation registry。
- ACP active input interception 发生在普通 slash/ReAct dispatch 之前。
- `/stop` 在 ACP mode 中仍由 Rua 控制。
- ACP lifecycle 不依赖 visible slash commands。
- Priority Registry 不得执行命令或持有业务 context。
- 前缀匹配必须尊重 token boundary 和最长匹配。
- Discovery metadata 必须来自 backend registries。
- legacy `/acp`、`/noacp`、`/acph`、`/acpr` handler 不得进入默认
  visible registry 或 Discovery API。
- unknown slash command 不得落入 LLM 当普通消息处理。

## Authority, persistence, and failure boundaries

Daemon Registry 拥有进程 command dispatch。
Control Registry 拥有 runtime/session command dispatch。
Conversation Registry 拥有 conversation command dispatch。
Channel Priority Registry 只拥有 enqueue priority。
ACP Chat Session Manager/Conversation Authority 拥有 ACP binding 与 placement。

Discovery API 是 frontend command metadata authority，不是执行 authority。
Chat 中持久化的 ACP binding 可以在执行 Device 恢复；内存 session 丢失时，
runtime 尝试按 persisted runner session resume，失败则返回明确错误。

command handler failure转换为 terminal `CommandOutcome`。
ACP runner failure产生标准 Error/End events，不得回退到 Rua Agent loop
重新执行同一用户输入。

## Consequences

- 进程控制、运行控制和 Conversation 操作拥有最小依赖 Context。
- 常用 command 不消耗 Model Call。
- `/stop` 可通过 Channel queue priority 更快到达控制路径。
- Web Console autocomplete 与 backend visible registry 一致。
- ACP Chat Mode 复用统一 RuntimeEvent stream，同时保持独立 runner lifecycle。

## Rejected alternatives

- 把所有 command 放入一个拥有全部依赖的 trait：会形成巨大 optional context。
- 在前端硬编码 command list：必然与 backend 漂移。
- 让 priority registry 同时执行命令：会让 Channel 依赖 process/Agent internals。
- 将 agent-required commands 复制成第二套 registry：metadata 与实现会分叉。
- 恢复 Claude Code 专用 pass-through：ACP Chat Mode 已提供
  runner-neutral protocol。
- 把 ACP enter/exit 继续作为默认 slash command：无法正确路由跨 Device authority。
- ACP 失败后自动改走 Rua Agent：可能重复有副作用的用户请求。

## Implementation and verification

当前实现路径：

- command traits/registries：`crates/rua-command/src/`
- dispatcher：`crates/rua-command/src/dispatch.rs`
- Daemon registry：`crates/rua-command/src/daemon/mod.rs`
- Control registry：`crates/rua-command/src/control/mod.rs`
- Conversation registry：`crates/rua-command/src/conversation/mod.rs`
- Agent registry adapters：`crates/rua-agent/src/command_adapters.rs`
- ACP intercept：`crates/rua-agent/src/runtime/command_dispatch.rs`
- ACP session manager：`crates/rua-command/src/acp_chat.rs`
- Channel priority：`crates/rua-channel/src/core/command_registry.rs`
- Discovery endpoint：
  `crates/rua-server/src/endpoints/commands_discovery.rs`
- route registration：`crates/rua-server/src/router.rs`
- Chat-scoped ACP HTTP：`crates/rua-server/src/endpoints/acp_chat.rs`

验证覆盖：

- `crates/rua-command/tests/dispatcher/dispatch_tests.rs`
- `crates/rua-agent/tests/conversation_commands/registry_e2e.rs`
- `crates/rua-agent/tests/runtime_full_dispatch/dispatch_tests.rs`
- `crates/rua-server/tests/control_path/stop_command.rs`
- `crates/rua-server/tests/commands_discovery/discovery_api.rs`
- `crates/rua-server/tests/contract_acp_http/acp_routes.rs`

### Current implementation gaps

- `crates/rua-command/src/dispatch.rs::CommandDispatcher::dispatch` 当前在查询
  Daemon/Control registry 之前先调用
  `conversation.is_agent_required(name)`；`run_command_path` 也采用相同预检查。
- 如果未来出现一个与 Daemon 或 Control command 同名、且
  `agent_required=true` 的 Conversation command，当前实现会提前返回
  `AgentRequired`/绕过 fast path，使低层命令遮蔽高层命令，违反固定
  Daemon → Control → Conversation 优先级。
- 现有同名优先级测试只覆盖普通 Conversation command，没有覆盖
  agent-required 冲突。修复必须先按层选择最终命中项，只在最终选择
  Conversation command 后检查 `agent_required`，并为两种同名冲突补回归测试。

## Related decisions

- ADR 0001: Workspace 分层依赖方向与进程组合
- ADR 0008: Agent Work Contract 与 Collaboration Mode
- ADR 0012: Per-Agent Tool Guard 与进程级 Approval
- ADR 0016: Channel Trait、实现隔离与 Feature Gates
- ADR 0022: Conversation Authority, placement, and epoch fencing
- ADR 0033: ACP Chat Mode, client projection, and permission relay
- ADR 0035: Chat CLI single-request adapter and resident server boundary
