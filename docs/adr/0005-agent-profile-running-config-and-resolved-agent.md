# ADR 0005: Agent Profile、Running Config 与 ResolvedAgent

**Status:** Accepted

## Context

一个 Agent 同时具有身份、加载后的内容和可调运行行为。
早期结构把 `max_iters`、自动继续、Memory/Display policy 等行为字段复制到
`AgentProfile`，同时 `agent.json.running` 又保存完整
`AgentsRunningConfig`。两个对象逐渐成为不同真相，新增字段时容易只接通一条
执行路径。

当前产品还允许默认 Agent 配置作为其它 Agent 文件的 overlay 基础，
并通过 Cell Store 从 `workspaces/{agent_id}/agent.json` 读取。
调用方如果分别查询 profile 与 running，会在 reload 或并发更新时组合出
不同版本的数据。

Security policy 又具有不同所有权：它位于 Agent 的 `security.json`，
由安全配置服务管理默认值，不为了“一次拿全”塞入 ResolvedAgent。创建 Chat
时安全策略与执行配置一起物化；执行读取 Chat/Turn 快照而非 Agent 最新默认值。

## Decision

### 1. AgentProfile 只表示 identity 与 resolved content

`crates/rua-agent/src/profile.rs` 的 `AgentProfile` 包含：

- `id`、`name`、`description`；
- prompt resolution 输入 `workspace_dir` 与 `system_prompt_files`；
- `model_slot`；
- enabled Tool 和 MCP server 名称；
- `language`、`template_id`、`enabled`；
- metadata 与用于兼容 round-trip 的 extra。

结构中仍有 `system_prompt` 兼容字段，但生产
`AgentProfile::from_config` 当前将其置空。有效 system prompt 在执行准备时，
由 `crates/rua-agent/src/prompt.rs::build_system_prompt_from_working_dir`
根据 working directory 和 `system_prompt_files` 加载；不得把
`AgentProfile.system_prompt` 描述为持久化或已解析的 prompt authority。

`AgentProfile` 可以包含从 on-disk DTO 解析出的工具/MCP enablement 和 prompt
resolution 输入，但不得重新加入 runtime behavior 字段。

`AgentProfileConfig` 是磁盘 DTO；`AgentProfile` 是运行时解析结果。
二者相关但不是同一个类型，也不能互换 authority。

### 2. AgentsRunningConfig 是运行行为的唯一 Agent 默认

`crates/rua-config/src/types/agents.rs` 的 `AgentsRunningConfig` 保存
用户可调的 Agent runtime behavior，包括 ReAct 迭代与自动继续、
LLM retry/rate limit、Context Compact、Tool Result Compact、
Memory Summary、Embedding、Heartbeat 和 auto-search 等配置。

新增行为字段应进入其自然 sub-config，不在 `AgentProfile` 建镜像字段。
未知字段通过 serde flatten 保留，以维持兼容 round-trip；
保留未知字段不等于运行时已支持其语义。

Agent Running Config 是 Agent 的 device-local 默认模板。
Chat 若拥有已物化的完整运行配置，则后续 Agent 默认变化不应静默改写
已有 Chat；Turn 使用其 claim 时冻结的有效配置。

### 3. AgentRegistry 返回完整 ResolvedAgent

`ResolvedAgent` 的稳定形态是：

```rust
pub struct ResolvedAgent {
    pub profile: AgentProfile,
    pub running: Arc<AgentsRunningConfig>,
}
```

`AgentRegistry::get_agent`、`active_agent` 和 `list_agents` 均返回
`ResolvedAgent`。执行路径传递这个完整值，只有在消费点才可显式 destructure。

禁止沿调用链把 `AgentProfile` 与 `Arc<AgentsRunningConfig>` 分成两个参数，
也禁止先取 profile、稍后再次从磁盘查询 running。

### 4. Profile 与 Running 来自同一次配置解析

`ResolvedAgent::from_config` 从同一个 `AgentProfileConfig` 同时构造
profile 和 running。持久化 registry 通过 `AgentProfileStore` 与
`AgentCells` 加载 default/target overlay，完成 backfill 后再创建
`ResolvedAgent`。

registry cache 以 Agent id 保存完整 `ResolvedAgent`。reload 必须整体替换
缓存，不分别刷新两个 half。

### 5. Security Profile 保持正交

Agent Security Profile 是 `workspaces/{id}/security.json` 中的
独立 per-Agent artifact。它由 `rua-security` 的 Tool Guard/Approval
配置物化路径读取，不属于 `ResolvedAgent`。实际执行使用 Chat/Turn 中已物化的
授权策略；Agent 文件是默认来源而非已有 Chat 的动态策略依赖。

身份内容、运行行为和安全策略是三个独立 ownership boundary。
一次 Agent 解析不授予工具权限，也不把安全策略降级为普通行为开关。

## Hard invariants

- ResolvedAgent 是创建／显式更新配置的完整输入，不是每 Turn 动态补全来源。
- 协作目标贡献执行配置，来源 Chat 贡献授权和默认环境；按 ADR-0028/0053
  物化并传播继承项，不合并两个 Agent 安全策略。

- `AgentProfile` 不得包含 runtime behavior 的镜像字段。
- `AgentsRunningConfig` 是 Agent 默认行为的唯一 typed source。
- registry lookup 必须返回 `ResolvedAgent`，不能只返回 profile。
- profile 与 running 必须来自同一次配置解析或同一个显式测试 fixture。
- 执行路径不得把 profile/running 拆成独立查询。
- hot reload 必须替换完整 resolved value。
- unknown serde fields 可 round-trip，但不得被假定为已执行。
- Agent Security Profile 不得并入 `ResolvedAgent`。
- Chat-owned snapshot 不得持续继承可变 Agent Running Config。
- `MemoryPolicy`、`DisplayPolicy` 等已删除的 profile 行为层不得复活。

## Authority, persistence, and failure boundaries

磁盘 `AgentProfileConfig` 是 Agent 定义和默认运行配置的持久化输入。
`AgentProfileStore` 拥有 default overlay 解析，`AgentCells` 拥有文件 layout，
`AgentRegistry` 拥有进程内 resolved cache。

`AgentProfile` 拥有身份和内容解析输入语义；prompt builder 拥有 working
directory 上的有效 system prompt resolution；`AgentsRunningConfig` 拥有
行为语义；Security Module 拥有安全策略语义。任何一层的读取失败不得用
另一层的默认值悄悄掩盖，除非 schema 明确定义了默认或兼容 backfill。

非持久化测试 registry 可用 `ResolvedAgent::from_profile` 配置默认 running，
但生产持久化路径必须从完整 config 构造。

## Consequences

- 调用方一次 lookup 即获得 coherent Agent 运行输入。
- 增加 runtime knob 不需要同步修改 `AgentProfile`。
- profile identity/model/tool/prompt-resolution 输入与行为配置可分别测试。
- Security reload 不要求重建 Agent identity，反之亦然。
- `ResolvedAgent` 会成为 Agent runtime、Task runner 和 Workspace wiring 的
  高频传递类型，这是显式一致性换来的成本。

## Rejected alternatives

- 在 `AgentProfile` 复制常用 behavior：会再次产生双真相。
- 让调用方分别读取 profile 与 running：reload 时无法保证版本一致。
- 删除 `AgentProfile`，直接到处传磁盘 DTO：会把持久化格式泄漏给运行时。
- 建立 `AgentIdentity`/`AgentAssets`/`AgentBehavior` 更多单用途 wrapper：
  当前没有足够独立不变量支撑额外层次。
- 把 Security Profile 放入 `ResolvedAgent`：会混淆行为配置与执行授权。
- 修改 Agent 默认后自动更新所有 Chat：会破坏 Chat/Turn 的配置稳定性。

## Implementation and verification

当前实现路径：

- runtime profile：`crates/rua-agent/src/profile.rs`
- effective prompt resolution：`crates/rua-agent/src/prompt.rs`
- running config schema：`crates/rua-config/src/types/agents.rs`
- `ResolvedAgent` 与 registry：`crates/rua-agent/src/registry.rs`
- default overlay 与 config loading：
  `crates/rua-agent/src/profile_store.rs`
- Cell layout：`crates/rua-agent/src/cells.rs`
- Workspace 消费：`crates/rua-workspace/src/runtime.rs`
- Agent Task 消费：`crates/rua-workspace/src/task_agent_execution.rs`

验证覆盖：

- `crates/rua-agent/tests/registry_via_cells.rs`
- `crates/rua-agent/tests/registry_via_cells/profile_store.rs`
- `crates/rua-agent/tests/cells_layout.rs`
- `crates/rua-config/tests/unit_agents_config/running.rs`
- `crates/rua-agent/tests/runtime/react_config.rs`

检查应搜索并拒绝运行时代码重新读取已删除的
`AgentProfile.max_iters`、`MemoryPolicy` 或 `DisplayPolicy`。

### Configuration application boundary

Agent 默认配置提交不自动替换已加载的 `WorkspaceRuntime`，也不使既有 Chat
重新继承 Agent 默认值。Chat 的物化、完整配置 epoch 和 Turn-stable capture
遵守 ADR-0028。属于长期运行资源的配置由对应 owner 局部应用，属于执行策略
的配置由既有捕获边界提供，不通过保留陈旧快照并重建整个 Runtime 刷新。

配置保存和资源应用分别报告；应用失败不自动回滚已提交配置，也不隐式使用
旧资源配置接收新工作。具体生命周期所有权与失败边界见 ADR-0001 §2.1–2.2。

## Related decisions

- ADR 0001: Workspace 分层依赖方向与进程组合
- ADR 0004: 配置持久化 Cell Store
- ADR 0008: Agent Work Contract 与 Collaboration Mode
- ADR 0012: Per-Agent Tool Guard 与进程级 Approval
- ADR 0013: MCP Per-Agent Manager 隔离
- ADR 0028: Chat Running Config and Turn-stable snapshot
