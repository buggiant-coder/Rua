# ADR 0014: Plugin process and protocol isolation

**Status:** Accepted

## Context

Rua 需要扩展 Tool、Skill、CLI command 和若干 lifecycle/operation hook，
但第三方 Plugin 代码不能链接进核心进程并获得任意 Rust 内部引用。
同时，ReAct loop 需要高效、强类型的内部扩展点来接入持久化、安全、
Tool mutation/observation 和 approval。

这两个需求容易被混成一个“通用 Hook API”：

- 若把 `react-agent::AgentHook` 暴露给 Plugin，它会泄漏内部生命周期、
  Rust 类型和 authority；
- 若核心内部逻辑都经 JSON-RPC Plugin protocol 执行，会增加失败面并削弱
  编译期约束；
- 若把 subprocess isolation 称为完整 OS sandbox，又会高估当前安全保证。

因此 Plugin execution boundary 与内部 Agent Hook SPI 必须明确分离。

## Decision

### 1. Plugin 运行在独立 subprocess

`rua-plugin-runtime` 从 `plugin.json` 读取 manifest，并通过
`PluginClient` 启动独立 stdio process。默认 Python entrypoint 或 manifest
指定 command/args 均先经过 Runtime Authority 生成启动计划。

核心进程与 Plugin 只通过结构化 JSON-RPC/stdin-stdout 协议交互。
Plugin 不获得 `AppState`、WorkspaceRuntime、数据库 handle、
ApprovalAuthority 或 `AgentContext` 的内存引用。

当前保证仅是 process 与 protocol isolation，不是 OS security sandbox。
现有 launch 没有施加 seccomp、Landlock、container/chroot、filesystem ACL
或 network isolation；Plugin subprocess 仍以宿主用户权限运行，并接收
working directory、secret directory 参数和配置环境。产品、文档和诊断均
不得把这一边界表述为限制宿主文件系统或网络访问。

### 2. Plugin protocol 是显式 capability surface

协议支持注册/调用 Hook、Skill 和 CLI，以及 MCP-compatible Tool discovery/
call。每个 request 使用固定 method、typed params 和 bounded timeout。

manifest 声明 id、version、entry point、dependencies、minimum Rua version、
CLI commands 和 provider declarations。manifest 元数据不自动授予执行权限。

Plugin Tool 进入 Effective Capability Snapshot 后，仍按真实 Tool name/args
经过 Tool Guard 与 Approval。

### 3. Plugin lifecycle 由 ActivePluginRegistry 管理

registry 保存 running client、status、crash count 和 lifecycle events。
`PluginLifecycleManager` 提供显式 start/stop/restart，并由 crash watcher
记录 crash 与 auto-disable。

`PluginClient` 拥有 stdio transport、call timeout、heartbeat task 和 shutdown。
heartbeat 失败将状态变为 unhealthy/crashed，不把失效 Plugin Tool 继续暴露
给下一次 capability snapshot。

当前 crash watcher 的 backoff/`Starting` 状态变化本身不会重新构造
`PluginClient`；只有持有完整 `PluginClientConfig` 的显式 start/restart
路径才会 spawn subprocess。不得把状态变化描述成已完成的自动重启。

shutdown 必须停止 heartbeat，并请求 transport/subprocess 退出。

### 4. 安装与加载必须防止 archive path escape

远程 Plugin 下载有 timeout 和最大 bytes。
ZIP extraction 必须只接受安全相对路径，并在创建目录或打开文件之前拒绝
symlink parent 与任何 canonical escape。写入后的检查只能作为 defense in
depth，不能作为首次 containment check。

Plugin manifest 只从目标 Plugin 根的 `plugin.json` 加载。
下载或解包成功不等于 Plugin 已被启用或信任。

### 5. 依赖安装是独立的不受信代码执行边界

Python package 安装可能在 wheel/build backend、setup hook 或依赖构建阶段
执行远端代码。安装必须在读取并验证 Plugin manifest 之前就按不受信代码
处理，并具有显式用户信任、Runtime Authority fatal-plan 检查、总 deadline、
资源边界和失败清理。

不得因为 Plugin 尚未 enabled，或因为 `pip install --target` 写入独立目录，
就把安装阶段描述为无代码执行。

### 6. `AgentHook` 是内部强类型 SPI

`react-agent::AgentHook` 为进程内 runtime adapter 提供：

- before/after model call；
- model event observation/filter；
- before/after tool call；
- finish；
- approval wait 和 permission decision recording。

它用于组合 Rua 自有的持久化、安全和运行时行为。
默认方法保持 no-op/deny-safe，测试可使用 `RecordingHook`。

`AgentHook` 不稳定为第三方 ABI，不通过 Plugin manifest 注册，也不承诺
跨版本 wire compatibility。

### 7. 产品 Operation Hook 通过窄 boundary 接入

公开/可配置 Hook 不直接实现 `AgentHook`。
`ToolBeforeBoundary`、`ToolObserverBoundary` 和
`PermissionRequestBoundary` 使用 event-specific typed input/output，
由 `rua-agent` adapter 接入 ReAct loop。

Plugin-backed product Hook 通过 `ActivePluginRegistry::invoke_hook`
执行 JSON-RPC callback。它只能返回该事件允许的结果，
不能获得通用 mutable Agent context。

### 8. Authority 与安全不可委托给 Plugin

Plugin 或 Hook 不得拥有或替换：

- Conversation Authority、Turn claim 或 terminal persistence；
- cancellation propagation 与 shutdown；
- Tool Guard、File Guard 或 hard deny；
- Approval Authority 与 durable reuse rule；
- Runtime Authority 的 placement；
- canonical Tool Result 或 Conversation State。

Permission Hook 只能对已进入 approval boundary 的具体 request 返回
one-shot allow/deny/no-decision，不能创建永久权限或修改 Tool arguments。

## Hard invariants

- 第三方 Plugin 代码不得 in-process 链接进核心 runtime。
- Plugin 通信只通过明确的 protocol methods 和 typed payload。
- Plugin executable launch 必须经过 Runtime Authority。
- Runtime Authority 的 fatal diagnostic 必须在 spawn/install 前 fail closed。
- Plugin Tool 仍必须经过 Effective Capability、Tool Guard 和 Approval。
- Plugin enablement 无法读取或状态不确定时必须 fail closed，不能把锁竞争、
  加载失败或未知 Plugin 解释成 enabled。
- archive extraction 不得写出目标 Plugin directory。
- Plugin package install 不得在没有明确 trust、deadline 和执行隔离策略时
  运行远端构建代码。
- `AgentHook` 不得作为公共 Plugin ABI。
- product Hook 必须使用 event-specific boundary，不能获得万能 mutable context。
- Hook failure 不能被解释为 allow 或 successful completion。
- Plugin/Hook 不得改变 Conversation Authority 或 hard security decision。
- shutdown 必须有界并终止 heartbeat 与 subprocess ownership。
- Plugin restart 必须经过能够重新生成 launch plan 的显式 lifecycle 路径。

## Authority, persistence, and failure boundaries

Plugin manifest/install store 拥有已安装定义。
`ActivePluginRegistry` 拥有当前进程中的 active instances 和 health。
`PluginClient` 拥有一个 subprocess transport。
内部 `AgentHook` 由 Agent runtime 组合。
Product Hook service 拥有可配置 hook definition/materialization。

Plugin crash、timeout、malformed JSON 或 protocol mismatch 只使该 invocation
或 Plugin capability 失败。核心进程继续运行，并在下一 snapshot 隐藏
不可用能力。Hook failure 不得改写已经提交的 terminal facts。

## Consequences

- Plugin 崩溃与核心进程内存隔离。
- 内部 hook 保持 Rust 类型安全和低开销。
- JSON-RPC protocol 只暴露明确授予的数据与操作；在 OS 层，当前 Plugin
  subprocess 仍保留宿主用户权限，不能据此推导 filesystem/network 隔离。
- subprocess、heartbeat 和 JSON serialization 带来额外成本。
- 当前 process/protocol isolation 不能被夸大为 sandbox；更强 OS policy
  需要独立实现与验证。
- 安装器需要被视为独立 supply-chain execution surface。

## Rejected alternatives

- 动态链接第三方 Rust library：ABI、崩溃和内存权限边界不可接受。
- 将 `AgentHook` 直接公开为 Plugin API：泄漏内部生命周期与 authority。
- 所有内部 hook 都走 JSON-RPC：会让 correctness-critical path 依赖外部进程。
- 一个通用 Hook context/action：会让每个事件拥有过宽权限。
- 把 Plugin hook 结果当安全 authority：Plugin failure 可能放宽 hard policy。
- 仅按文件扩展名检查 archive：不能阻止 zip-slip 和 canonical escape。
- 把 `pip install` 当作纯文件下载：Python build/install hooks 可以执行代码。

## Implementation and verification

当前实现路径：

- Plugin manifest/load：`crates/rua-plugin-runtime/src/manifest.rs`、
  `crates/rua-plugin-runtime/src/loader.rs`
- Plugin client 与 Runtime Authority：
  `crates/rua-plugin-runtime/src/client.rs`
- active lifecycle：`crates/rua-plugin-runtime/src/registry.rs`、
  `crates/rua-plugin-runtime/src/manager.rs`
- JSON-RPC protocol：`crates/rua-plugin-runtime/src/protocol.rs`
- Plugin hook registry：`crates/rua-plugin-runtime/src/hook_registry.rs`
- internal Agent Hook：`crates/react-agent/src/hooks.rs`
- narrow boundaries：`crates/react-agent/src/tool_before.rs`、
  `crates/react-agent/src/tool_observer.rs`、
  `crates/react-agent/src/permission_request.rs`
- product adapters：`crates/rua-server/src/services/product_hook_execution.rs`
- ZIP extraction：`crates/rua-plugin-runtime/src/loader.rs`
- Python package installer：`crates/rua-plugin/src/installer.rs`

验证覆盖：

- `crates/rua-plugin-runtime/tests/all/lifecycle.rs`
- `crates/rua-plugin-runtime/tests/all/plugin_lifecycle_manager.rs`
- `crates/rua-plugin-runtime/tests/all/protocol_round_trip.rs`
- `crates/rua-plugin-runtime/tests/all/hook_registry.rs`
- `crates/rua-plugin-runtime/tests/all/runtime_authority.rs`
- `crates/rua-server/tests/contract_product_hooks_http/`

### Current implementation gaps

- 当前没有 OS-level sandbox。`PluginClientConfig` 向普通 subprocess 传递
  `working_dir`、`secret_dir` 和 env，Cargo/runtime 中没有 seccomp、
  Landlock、container、chroot 或 network confinement。现有安全保证只能称为
  process crash isolation 与 typed protocol boundary。
- `crates/rua-plugin-runtime/src/loader.rs::safe_extract_zip` 先
  `create_dir_all`/`File::create` 并写入，再 canonicalize target 检查是否逃逸。
  若目标树中预置 symlink parent，越界写入可在检查前发生。修复必须在 open
  之前逐级拒绝 symlink 并验证 parent containment，或使用安全目录句柄 API。
- `crates/rua-plugin/src/installer.rs::PluginInstaller::install` 在读取
  `plugin.json` 前执行 `pip install`；该调用没有总 timeout、没有独立资源限制
  或 OS isolation，并且没有拒绝 Runtime Authority fatal diagnostics。
  Python 安装构建阶段可以执行远端代码。修复必须在此执行点建立明确的信任与
  隔离策略，而不是把风险推迟到 Plugin enable/start。
- `crates/rua-plugin/src/lib.rs::PluginManager::is_enabled_sync` 在
  `try_read` 因锁竞争失败时返回 `true`。`AppState` 将该闭包安装为
  `PluginRegistry` 的 enabled checker，因此“当前无法确认状态”会被解释成
  “Plugin 已启用”，并可能让 Tool/Hook 进入 active surface。该路径必须改为
  fail closed，或改成可等待的一致性读取；锁竞争不能授予能力。

## Related decisions

- ADR 0003: Runtime Authority 与外部进程执行环境
- ADR 0007: 进程生命周期和有界分阶段关闭
- ADR 0009: Bounded execution guardrails without a shared budget entity
- ADR 0011: Effective Capability Snapshot 与 Tool Exposure
- ADR 0012: Per-Agent Tool Guard 与进程级 Approval
- ADR 0029: Two-layer Product Hooks
