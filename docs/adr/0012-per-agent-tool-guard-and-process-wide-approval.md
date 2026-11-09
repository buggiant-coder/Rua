# ADR 0012: Chat 快照授权、Tool Guard 与进程级 Approval

**Status:** Accepted

## Context

多 Agent 系统需要不同的信任边界。主开发 Agent、自动化 Agent 和外部接入
Agent 不应被迫共享一份最严格或最宽松的 Tool policy。

同时，用户面对的是一个 Rua 进程中的统一审批面。若每个 Agent 各自维护
Approval service，跨 Chat 的 pending request、事件订阅和决定路由会碎片化。

安全判断还包含不同性质的规则：

- 显式 deny/allow permission rules；
- Tool Guard 与 File Guard 的安全 findings；
- Approval Mode 的默认中断策略；
- Chat、Agent 或 Runtime Session 范围的 reusable approval。

这些规则必须有固定优先级。Tool Exposure 只决定模型是否看见 Tool，
不能代替执行时安全检查。ACP runner 的 Permission Relay 又是外部协议状态，
不能被强行映射成 Rua Approval。

## Decision

### 1. Agent Security Profile 是默认模板，Chat 快照是执行依据

`SecurityConfigStore` 按 Agent id 管理默认安全配置。创建 Chat 时将安全策略
与完整 Harness 配置共同物化为 Chat Configuration Snapshot。执行检查使用
claimed Chat/Turn epoch 的授权值，不按 Agent id 重新读取最新默认策略。
缓存必须区分有效快照身份，不能仅按 Agent ID 复用可变 engine。

每个 profile 是独立 snapshot，不使用全局 base 加 overlay。
Builtin rules 是创建或 reset 时的模板，不作为运行时 live merge。

默认 Security Profile 仍由安全配置服务拥有，不塞入 `ResolvedAgent`。
有效安全配置进入 Chat 快照。协作继承来源 Chat 授权而非目标 Agent 的更高
权限；来源 Chat 更新继承项时传播新 epoch，不合并两个 profile。
Agent 默认值修改或删除不自动影响已有 Chat；见 ADR-0028/0053。

### 2. Tool Guard 在真实 Tool Call 执行前运行

所有 built-in、MCP 和 Plugin Tool Calls 在实际 executor 前进入同一
Security Module。Deferred Bridge call 必须先 unwrap 为真实 tool name/args。

评估顺序是：

1. 显式 deny permission rule；
2. enabled safety/File Guard hard deny；
3. matching reusable allow；
4. safety findings 的 audit/approval；
5. Approval Mode fallback 与 tool annotation；
6. allow execution。

hard deny 不得被 allow rule、Approval Mode 或 Hook 覆盖。

### 3. Approval Service 由进程级 `ApprovalAuthority` 实现

一个 kernel process 只有一个 Approval Service 实例；当前 Rust 类型是
`ApprovalAuthority`。它保存 live
Approval Requests、状态、events 和 reuse rules 的 runtime projection。

每个 request 记录：

- calling `agent_id`；
- typed `ApprovalOwner`；
- `turn_id` 与 `tool_call_id`；
- real tool name、args snapshot 和 display；
- trigger source；
- status、decision 和 suggested reuse options。

多 pending request 以 request id 独立寻址，任何决定必须针对明确 id。

### 4. Approval Owner 区分 Chat 与 Runtime Session

`ApprovalOwner` 只有：

- `Chat { address: ChatAddress }`；
- `RuntimeSession { agent_id, session_id }`。

Chat-scoped reuse 以完整 Chat Address 与 matcher 为 key，在 Project Chat
切换 Agent 后仍属于该 Chat。Agent-scoped reuse 以 calling Agent 与 matcher
为 key。Runtime Session reuse 以显式 `{agent_id, session_id}` owner 为 key。

ACP server Session 或其它 runtime session 不得伪造成 Agent-Scoped Chat。

### 5. 当前审批阻塞实际 Tool side effect

当 `SecurityVerdict::RequiresApproval` 且 permission Hook 没有给出决定时，
ReAct loop 先发布 `ToolApprovalPending`，再通过 Agent hook 调用
`ApprovalAuthority::wait_for_decision`。Tool executor 在批准前不得运行。

批准后继续同一 Tool Call；拒绝或失败返回 Tool error。
Approval wait 保持在当前 Turn 内，不创建第二个 continuation Turn，
也不使用跨请求 forced replay。不得把已经被后续实现取代的 parked-call
非阻塞回环描述为当前行为。

### 6. Approval 与 ACP Permission Relay 分离

ACP runner 发出的 permission options 保持 runner-owned identity、顺序和
decision semantics。它们通过 `rua-acp` Permission Relay 往返，
不创建 Rua Approval Reuse Rule。

Rua host sandbox、显式 deny 和 Runtime Authority 仍是 ACP process 的
外层强制边界。

## Hard invariants

- 每个 Agent 使用自己的 Security Profile 和 ToolGuardEngine。
- 一个进程只有一个 Approval Service / `ApprovalAuthority`。
- 真实 Tool name/args 必须在安全评估前确定。
- deny rule 与 hard deny 永远高于 allow/reuse。
- Tool Exposure 不得授予执行权限。
- Approval Request 必须携带 typed owner 和 calling Agent provenance。
- Chat reuse key 必须使用完整 Chat Address。
- Runtime Session 不得伪造成 Chat。
- Tool side effect 不得在批准前发生。
- ACP Permission Relay 不得创建 Rua approval rule。

## Authority, persistence, and failure boundaries

SecurityConfigStore 拥有 per-Agent 默认 policy；Chat 配置存储拥有物化的
有效授权快照。ToolGuardEngine 评估已解析的 snapshot policy。
ApprovalAuthority 是 pending request、decision 和 reuse matching 的
process runtime authority。

Agent-scope durable rules由 Agent security config 保存；
Chat-scope rules由 Chat-owned store 保存；
Runtime Session pending/reuse 只属于明确 runtime owner。
Approval transcript rendering 不是 authority。

创建时可按显式 schema 默认物化；执行时缺少所需授权快照必须失败，不能
回退目标 Agent 权限或新的默认值。继承工作目录不代表提升文件权限。
缺少 approval owner、turn 或 session context 时，应拒绝需要审批的调用。
等待期间 cancellation、request rejection 或 authority failure 均不得执行 Tool。

## Consequences

- 不同 Agent 可表达不同 trust delegation。
- 用户仍有统一 pending approval queue 和事件流。
- Project Chat 切换 Agent 不会丢失该 Chat 的 reuse rule。
- 安全规则与默认询问频率可独立配置。
- 当前同步等待模型要求 stream/control path 能在 Turn 仍活跃时提交决定。

## Rejected alternatives

- 单一 process-wide Tool policy：无法表达不同 Agent 的信任差异。
- 全局 base + per-Agent overlay：安全 merge precedence 难以解释和审计。
- 固定 trust-tier enum：不能表达具体 Tool/argument/path carve-out。
- 每 Agent 一个 ApprovalAuthority：会碎片化用户审批面。
- 由 frontend 猜 matcher 或修改 Chat message 表示批准：会产生第二 authority。
- 把 Hidden Tool 当 deny：模型可见性和执行授权是不同概念。
- 将 ACP option 映射成 Rua allow rule：会丢失 runner-owned 语义。

## Implementation and verification

**ADR-0053 修订实现缺口：** 当前按 Agent ID 获取／缓存 engine 的执行路径
需要改为有效 Chat/Turn 快照检查；默认配置管理仍保留。验证权限传播、直接
用户输入、后台 Tool Task 和通用 dispatch 不绕过限制。审批请求／一次批准
不是可复制配置，不能将来源 Chat 的批准变成跨 Chat 授权。

当前实现路径：

- Security composition：`crates/rua-security/src/security_module.rs`
- Tool Guard engine：`crates/rua-security/src/engine.rs`
- Approval Authority：`crates/rua-security/src/approval_authority.rs`
- per-Agent config store：`crates/rua-security/src/config_store.rs`
- File Guard：`crates/rua-security/src/file_guard.rs`
- Agent hook adapter：`crates/rua-agent/src/hooks.rs`
- ReAct enforcement：`crates/react-agent/src/react_loop.rs`
- MCP enforcement：`crates/rua-mcp/src/bridge.rs`
- ACP permission relay：`crates/rua-acp/src/permissions.rs`

验证覆盖：

- `crates/rua-security/tests/`
- `crates/rua-agent/tests/runtime/approval_authority_wiring.rs`
- `crates/rua-server/tests/contract_approval_http/`
- `crates/rua-server/tests/agent_approval/`
- `crates/rua-server/tests/contract_projects_http/`

### Current implementation gaps

- ReAct 在
  `crates/react-agent/src/react_loop.rs::execute_single_tool_canonical`
  通过 Agent hook 完成 Tool Guard、approval pending 和等待决定后，
  才调用已捕获 executor。
- MCP executor `crates/rua-mcp/src/bridge.rs::McpBridgedTool::call`
  当前又独立调用 `SecurityModule::enforce`。这个第二次检查不知道前一次
  approval 的 invocation identity；若再次得到 `RequiresApproval`，它只返回
  security error，不进入同一 ApprovalAuthority wait。
- `ApprovalDecision::RunOnce` 不创建 reusable rule；Chat reuse 也可能因第二次
  检查构造的默认 runtime owner 不同而无法匹配。因此“批准后继续同一 Tool
  Call”对 MCP 路径尚未成立。
- 这是实现缺口，不改变批准前不得发生 side effect、deny/hard deny 优先等
  不变量。修复必须移除 ReAct-owned MCP 路径的重复 enforcement，或向 adapter
  传递不可伪造且绑定 canonical name、args digest、owner 和 tool_call_id 的
  已批准 invocation token。

## Related decisions

- ADR 0005: Agent Profile、Running Config 与 ResolvedAgent
- ADR 0010: Tool 输出可见限制与 Raw Capture
- ADR 0011: Effective Capability Snapshot 与 Tool Exposure
- ADR 0013: MCP Per-Agent Manager 隔离
- ADR 0014: Plugin process and protocol isolation
- ADR 0022: Conversation Authority, placement, and epoch fencing
- ADR 0029: Two-layer Product Hooks
- ADR 0033: ACP Chat Mode, client projection, and permission relay
