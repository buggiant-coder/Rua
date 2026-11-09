# ADR 0013: MCP Per-Agent Manager 隔离

**Status:** Accepted

## Context

MCP server 配置、enablement、credentials、timeout、transport 和可见 Tool
集合可以因 Agent 而异。一个 Agent reload MCP 配置时，不应中断另一个
Agent 的 stdio process、HTTP connection 或 in-flight call。

MCP 又同时包含两种不同范围的数据：

- shared MCP catalog，描述可复用 server definition；
- Agent 的 `agent.json.mcp` section，描述该 Agent 实际启用和覆盖的配置。

如果使用 workspace-wide 单一 `McpManagerImpl`，所有调用都必须携带 Agent
覆盖，连接池和 client state 也会混合。若每次 Tool Call 临时创建 manager，
则失去连接复用、状态诊断和有界 shutdown。

## Decision

### 1. 一个 WorkspaceRuntime 拥有一个 MCP manager

`WorkspaceRuntime::lazy_init_mcp` 在该 Agent 的 runtime 内读取
`agent.json.mcp`，构造一个 `McpManagerImpl`，加载配置后保存为
`Arc<dyn McpManager>`。

manager 的 lifetime 与该 `WorkspaceRuntime` 一致。
不同 Agent 不共享 `servers` map、transport clients、connection status、
tool cache 或 reload 操作。

未配置 MCP 的 Agent 可以保持 manager 为 `None`。
mobile profile 可延迟或禁用自动初始化，但不得改成共享全局 manager。

### 2. Trait 与实现分层

`libs/mcp-manager` 定义 `McpManager`、`McpClient`、状态、配置和结果 contract。
`crates/rua-mcp` 实现 transport、config loading、Tool bridge 和
`McpManagerImpl`。

基础 trait crate 不依赖 `rua-agent`、`rua-workspace` 或 server。
Workspace 只持有 trait object，并通过 lifecycle adapter 管理 start/stop。

### 3. 每个 manager 独立拥有连接与策略

`McpManagerImpl` 保存当前 config、每个 server 的 client/status、
mobile policy、startup/call timeout、retry policy、runtime workspace/cache
目录和 Host Profile。

HTTP/SSE clients 的 headers、OAuth、private-IP policy 和 timeout 属于该
server entry。stdio client 的 executable/env/cwd 通过当前 manager 的
Runtime Authority 解析。

不允许跨 Agent 复用 client pool 后再在每次 request 上补 policy。

### 4. 配置持久化与 runtime reload 分离

`AgentMcpCells` 拥有 Agent MCP section 的 Cell layout；
`SharedMcpCells` 拥有 shared catalog layout。
配置写入成功后，拥有该 Agent 的 runtime 执行 reload 或 invalidation。

MCP manager 不成为 config persistence authority。
它只消费已经解析的 `McpConfig` 并报告 added、removed、updated 与 errors。

manager 的只读 `configuration()` 返回已接受的配置，不连接、不检查可用性，
也不发现工具。Chat 配置读取、初始化和预览只从中复制 server identity 与
工具禁用选择，不复制 transport 或凭据；实时 `list_tools` 仅属于能力执行
解析或显式 MCP 管理操作，不得成为打开对话选项的前提。

### 5. 可用性进入 Agent capability snapshot

manager 的 `list_servers`、`list_tools` 和 connection status 是
Effective Capability Snapshot 的输入。
某 server configured 但 disconnected/unavailable 时，其 Tool 不进入
当前 callable snapshot，并产生 per-Agent diagnostic。

Agent A 的 server failure 不改变 Agent B 的 capability generation。

### 6. 生命周期按 Agent 关闭

`McpServiceAdapter::stop` 调用该 manager 的 `shutdown_all`。
Workspace reload/replace 先 drain 旧 runtime，再关闭其 MCP clients。

初始化 Workspace 时产生的连接任务必须运行在 resident server 的 Tokio
runtime 中。启动辅助线程可使用该 runtime 的 Handle，但不得创建短命
runtime 承载这些任务后立即销毁，否则状态会滞留在 Connecting。

manager shutdown 的局部错误被记录，但 Workspace 关闭继续；
不得因一个 server 关闭失败跳过其它 server。

## Hard invariants

- `McpManagerImpl` 的实例范围是一个 Agent WorkspaceRuntime。
- 不同 Agent 不共享 server map、connection state 或 tool cache。
- per-Agent credentials 和 private-IP policy 不得进入共享 client。
- stdio launch 必须经过 Runtime Authority。
- resolved executable path 不得写回 MCP portable config。
- config persistence 与 connection lifecycle 必须分离。
- Agent MCP reload 不得中断其它 Agent。
- configured 但 unavailable 的 MCP Tool 不得进入 callable snapshot。
- MCP Tool Call 必须经过调用 Agent 的 Tool Guard。
- Workspace stop 必须调用该 Agent manager 的 `shutdown_all`。

## Authority, persistence, and failure boundaries

Shared/Agent MCP Cells 是配置持久化边界。
每个 `McpManagerImpl` 是其 Agent 的连接、状态和 Tool discovery authority。
`WorkspaceRuntime` 是实例 lifetime owner。
Effective Capability Resolver 是模型可见性 authority。

server connect、transport 或 tool discovery 失败只影响相应 Agent/server。
失败状态进入 `McpServerStatus` 和 capability diagnostics。
没有主动全局健康 ping 来制造跨 Agent 负载；调用、显式 connect/reload 和
transport failure 驱动状态变化。

## Consequences

- Agent 配置、凭据与连接状态天然隔离。
- 一个 Agent reload 的 blast radius 有界。
- 每 Agent 会支付自己的 client、连接池和首连成本。
- 进程级 MCP 观测需要聚合多个 manager，而不能读取单一全局 map。
- shared catalog 复用定义，但不复用运行连接。

## Rejected alternatives

- process-wide 单例 manager：会把 per-Agent policy 推到每次调用参数。
- 只共享 `reqwest::Client`：auth、timeout 和 private-IP policy 仍会混合。
- 每次 Tool Call 新建 manager：失去连接复用、状态和一致 shutdown。
- 将 shared catalog 直接当 active config：Agent 无法独立 enable/override。
- 周期性为所有 Agent 主动 ping：会造成 N 倍后台流量和无必要恢复逻辑。
- MCP failure 使整个 Workspace 启动失败：MCP 是可降级能力。

## Implementation and verification

当前实现路径：

- manager trait：`libs/mcp-manager/src/manager.rs`
- manager implementation：`crates/rua-mcp/src/manager.rs`
- Agent MCP Cells：`crates/rua-mcp/src/agent_mcp_cells.rs`
- shared MCP Cells：`crates/rua-mcp/src/shared_cells.rs`
- config/client store：`crates/rua-mcp/src/client_store.rs`
- Tool bridge：`crates/rua-mcp/src/bridge.rs`
- Workspace construction：`crates/rua-workspace/src/runtime.rs`
- lifecycle adapter：`crates/rua-workspace/src/services/mcp_adapter.rs`

验证覆盖：

- `crates/rua-mcp/tests/store/agent_mcp_cells_layout.rs`
- `crates/rua-mcp/tests/store/shared_cells_layout.rs`
- `crates/rua-mcp/tests/store/client_store.rs`
- `crates/rua-server/tests/endpoints/agent_mcp_persistence_e2e.rs`
- `crates/rua-agent/tests/runtime/skill_exposure.rs`
- Workspace isolation tests under `crates/rua-workspace/tests/isolation/`

### Current implementation gaps

- `WorkspaceRuntime::lazy_init_mcp` 当前没有 per-runtime single-flight，也不在
  构造前检查 `mcp_manager` 是否已经存在。若同一 runtime 被并发初始化，
  两条路径可能各自启动 MCP clients，最后只有一个 manager 留在字段中；被
  覆盖实例的进程/连接仍需依赖 drop 或 shutdown 清理。实现应让初始化幂等
  且同一 Agent 同时只存在一个 MCP manager generation。
- `load_config` 可以返回包含 per-server errors 的成功 report；当前
  `lazy_init_mcp` 仍安装 manager 并记录 “ready”。这不破坏 Agent 间隔离，
  但 “manager ready” 不能解释为所有 server 可用。Effective Capability
  Snapshot 必须继续按 connection status 和 config errors 将对应 MCP
  capability 标为 unavailable。

## Related decisions

- ADR 0001: Workspace 分层依赖方向与进程组合
- ADR 0003: Runtime Authority 与外部进程执行环境
- ADR 0004: 配置持久化 Cell Store
- ADR 0011: Effective Capability Snapshot 与 Tool Exposure
- ADR 0012: Per-Agent Tool Guard 与进程级 Approval
