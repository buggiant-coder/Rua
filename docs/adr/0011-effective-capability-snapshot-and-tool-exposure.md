# ADR 0011: Effective Capability Snapshot 与 Tool Exposure

**Status:** Accepted

## Context

Agent 配置可以启用 built-in Tools、MCP servers、Plugins 和 Skills，
但“已配置”不等于“当前执行 Device 可用”。远端同步来的 MCP command
可能不存在，本地 Plugin process 可能未激活，Skill 可能解析失败，
MCP connection 也可能在两个 Model Call 之间失效。

模型看到的 schema 与实际 dispatch registry 如果来自不同 generation，
模型会调用一个刚刚不存在或从未可用的 Tool。仅把所有非核心 Tool 放进
Disclosure Bridge 也不能解决问题：Deferred 仍然表示当前可调用。

因此每个 Model Call Boundary 需要一个不可变的能力解析结果，
并从该结果同时构造模型可见 surface 与执行 surface。

## Decision

### 1. 每个 Model Call 捕获一个 Effective Capability Snapshot

`rua-agent` 在准备 Model Call 前解析：

- base/built-in ToolKit；
-当前 Memory tools；
- MCP live discovery 与 connection status；
- Plugin registry 和 active plugin process；
- Agent Skills；
- 当前 Project root 下发现的 Project Skills；
- Agent enablement、channel 和 workspace/project scope。

结果包含 generation、有效 `AgentToolkit`、Skill prompt block、
Skill names、不可用 diagnostics 和 capability state。

snapshot 在该 Model Call 的 response Tool Calls 完成前保持不变。
下一个 Model Call 可以因配置或运行时可用性变化获得新 generation。

### 2. 配置身份与本机可执行性分离

配置仍可显示一个 MCP server、Plugin、Tool 或 Skill，即使当前 Device
不能执行它。不可用原因进入 diagnostics/configuration UI，
但能力不得进入当前 callable toolkit。

MCP fingerprint 至少包含 enabled、connection state、tool count、
last error、mobile unavailable reason 和 config errors。
fingerprint 变化会使 prepared cache 失效，即使磁盘配置未改变。

一个损坏的 Skill 被跳过并记录 warning，不阻止其它 Skills。
一个不可用 MCP server 不应让整个 Agent runtime 启动失败。

### 3. 同一 Snapshot 决定 Tool Exposure 与 dispatch

每个有效 Tool 被分类为：

- `Direct`：完整 schema 进入当前 request；
- `DeferredCandidate`：在需要时通过 Disclosure Bridge 暴露；
- `Hidden`：不进入直接 schema 或 deferred index。

`AgentToolkit` 同时保存 definition、executor 和 disclosure metadata。
模型 schema、Deferred catalog 和执行 lookup 必须来自同一个 toolkit。

Hidden 只表示模型不可见，不等价于 security deny。
Unavailable 不是 Deferred；它根本不在 callable snapshot 中。

### 4. Disclosure Bridge 按上下文成本启用

`ToolDisclosureConfig` 支持 `auto`、`on`、`off`。
默认 auto threshold 为 model context window 的 10%，search 默认返回 5 项，
最大 20 项。

启用后，模型直接看到 Direct Tools 和：

- `tool_search`;
- `tool_describe`;
- `tool_call`.

search 只返回短候选，describe 返回一个完整 schema。
`tool_call` 只能调用当前 snapshot 中的 DeferredCandidate。

### 5. Bridge call 在安全检查前还原真实 Tool Call

`tool_call` 的 wrapper name 不是安全和审计对象。
runtime 在 Tool Guard、Approval、Hook、logging 和 execution 前把它 unwrap
成真实 canonical Tool name 与 arguments。

bridge 工具不能递归调用自身，也不能调用 Direct、Hidden、Unavailable
或不属于当前 snapshot 的 Tool。

### 6. Skills 使用独立的按需加载模型

Skills 不为每个 `SKILL.md` 注册一套 Disclosure Bridge schema。
当前 snapshot 生成一个完整 Skill Catalog，并注册单一 `skill` pseudo-tool。
模型先从 catalog 选择，再按需加载对应 bundle 内容。

Agent Skills 与 Project Skills 可同时存在。同名但来源不同的 Skill 不得
静默互相覆盖；歧义必须显式失败或要求限定来源。

### 7. 能力变化以 current snapshot 为准

generation 变化可产生 Capability State Runtime Note，用于解释某能力为何
在后续 Turn 变为 direct、deferred、hidden、unavailable 或 removed。
历史 note 不是当前 catalog 的 authority。

正在执行的 Tool 保留其捕获 executor；变化只影响后续 Model Call。

## Hard invariants

- 配置侧能力来自 claimed Chat Configuration Snapshot，设备可用性仍在 Model
  Call Boundary 解析；不能因读取 Agent 最新默认值改变已有 Chat 的工具配置。
- 只读研究 Agent 是普通工具配置，只开放 Read/Grep/Glob 和检索工具，不开放
  Shell、写入及通用执行入口；不新增特殊 Subagent/只读 runtime。
- generic dispatch 和后台 Tool Task 不能调用该快照禁用的工具；可见性隐藏与
  disabled 不同，真实执行仍受 ADR-0012 的来源 Chat 授权快照约束。

- Model-visible schemas 与 dispatch executors 必须来自同一 snapshot。
- 配置存在不得被当成本机可执行。
- Unavailable capability 不得标记为 Deferred。
- Hidden 不得被当作 security deny 或 permission allow。
- bridge invocation 必须在 Tool Guard 前还原真实调用。
- bridge 不能跨 snapshot 调用旧 Tool。
- MCP availability 变化必须使下一 snapshot 失效重建。
- Skill parse failure 不得阻断其它能力。
- Active Tool execution 不得因下一 generation 改变 executor。
- current complete snapshot 优先于历史 capability notes。

## Authority, persistence, and failure boundaries

配置 manager 拥有 configured identity。
Runtime Authority 和各 runtime manager 提供 Device-local availability。
`EffectiveCapabilityResolver` 拥有 per-Model-Call snapshot。
`AgentToolkit` 拥有该 snapshot 的 definition/executor 对应关系。
Tool Disclosure 只拥有 visibility planning。

prepared cache 是可丢弃优化，以 generation、channel、workspace/project root
和 MCP fingerprint 为 key。它不是持久化 authority。
Capability State Runtime Note 可持久化变化事实，但不能重建执行 registry。

单个 MCP/Plugin/Skill 失败会降低 capability 并产生诊断；
snapshot 构造只有在核心准备无法完成时才使 Model Call 失败。

## Consequences

- 模型不会收到当前 Device 无法执行的 Tool schema。
- Direct/Deferred/Hidden 可独立于 permission 演进。
- MCP live state 能在不改配置的情况下更新下一次 Model Call。
- schema 和 executor 的 generation 一致性可通过测试直接验证。
- 每次 snapshot 重建有成本，因此允许严格 keyed cache。

## Rejected alternatives

- 以磁盘配置直接生成 Tool schemas：忽略本机可用性。
- schema 与 executor 分别构建：会产生可见但不可 dispatch 的 Tool。
- 所有配置 Tool 都 Deferred：Deferred 仍承诺可调用。
- 调用时才发现缺失 executable：会浪费 Model Call 并诱发重复失败。
- 用历史 Runtime Notes 重放当前 catalog：notes 只是变化记录。
- 把 Skill bundle 的每个能力都注册为普通 Tool：schema 数量和上下文成本过高。

## Implementation and verification

当前实现路径：

- snapshot resolver：`crates/rua-agent/src/runtime/mod.rs`
- Rua Tool 分类：`crates/rua-agent/src/tool_disclosure.rs`
- Toolkit 注册：`crates/rua-agent/src/toolkit.rs`
- generic toolkit：`crates/react-agent/src/toolkit.rs`
- Disclosure Bridge：`crates/react-agent/src/tool_disclosure.rs`
- Skill pseudo-tool：`crates/react-agent/src/skill_tool.rs`
- MCP bridge：`crates/rua-mcp/src/bridge.rs`
- Plugin Tool 注入：`crates/rua-agent/src/plugin_tool_handler.rs`

验证覆盖：

- `crates/rua-agent/tests/runtime/skill_exposure.rs`
- `crates/rua-agent/tests/runtime/mcp_tool_output.rs`
- `crates/rua-server/tests/console/tool_disclosure_e2e.rs`
- `crates/rua-server/tests/console/mcp_tool_exposure.rs`
- `crates/react-agent/tests/`

### Current implementation gaps

- `crates/rua-agent/src/runtime/mod.rs` 当前仍会同时注入 legacy
  `PluginRegistry` 和 `ActivePluginRegistry` 的 tools。
- legacy 注入路径
  `crates/rua-agent/src/plugin_tool_handler.rs::populate_plugin_tools`
  只检查 manifest registry 中的 enabled 状态；其 executor
  `PluginToolHandler::execute` 对所有调用固定返回
  `Legacy WASM tool invocation removed`。
- 因此已知不可执行的 legacy Plugin tool 仍可能进入 model-visible toolkit，
  违反 Unavailable capability 不得 Direct/Deferred 暴露的不变量。修复必须
  删除该注入路径，或在 snapshot resolution 时将其明确解析为 Unavailable
  并仅保留 diagnostic；只有 active subprocess registry 的 live tools 才能
  进入 callable snapshot。

## Related decisions

- ADR 0002: Runtime Layout 与 Agent Path Contract
- ADR 0003: Runtime Authority 与外部进程执行环境
- ADR 0010: Tool 输出可见限制与 Raw Capture
- ADR 0012: Per-Agent Tool Guard 与进程级 Approval
- ADR 0013: MCP Per-Agent Manager 隔离
- ADR 0015: Skill Bundle 安装来源与扫描信任
- ADR 0026: Context Projection owns model-call assembly
- ADR 0029: Two-layer Product Hooks
