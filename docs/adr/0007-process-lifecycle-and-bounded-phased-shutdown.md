# ADR 0007: 进程生命周期和有界分阶段关闭

**Status:** Proposed

## Context

Rua 常驻进程同时拥有 HTTP listener、Active Streams、Workspace runtimes、
Task Runtime、Plugin subprocess、MCP connections、后台任务、持久化 writer、
observability 和平台 power resources。

简单地收到信号后退出会截断持久化和子进程清理；无限等待所有任务完成又会让
桌面退出、服务重启和 Android lifecycle 无法收敛。第二次退出信号还必须能
越过已经卡住的优雅关闭。

当前代码存在三个互补层次：

- `ShutdownController` 定义通用七阶段顺序和 timeout 配置；
- server `graceful_shutdown` 编排实际进程级资源；
- Workspace `ServiceManager` 和 registry 管理 Agent 级 drain/stop。

这些构件尚未形成统一的有界关闭链路：server 当前没有用
`ShutdownController` 驱动阶段，也没有对每个组件应用阶段 deadline。
因此本 ADR 记录目标契约，并保持 Proposed，直到进程组合层完成统一接线。

## Decision

### 1. 关闭是单向、分阶段状态机

标准阶段顺序是：

1. `stop_accepting`;
2. `notify_streams`;
3. `cancel_non_critical`;
4. `wait_persistence`;
5. `stop_subsystems`;
6. `flush_observability`;
7. `release_power_resources`.

阶段只能前进，不能回退或重复打开已关闭入口。
`ShutdownController` 保存当前 phase，并为 persistence、subsystem 和 flush
阶段提供独立 timeout。

### 2. 第一信号触发优雅关闭，第二信号允许强制退出

server 同时监听 Unix SIGTERM/SIGINT、非 Unix Ctrl+C 和应用内部
shutdown watch channel。首个触发只启动一次关闭流程。

进入关闭后立即安装总时限 watchdog，并在 Unix 上将第二次 SIGINT/SIGTERM
绑定为直接退出。优雅路径不得依赖第二信号才能结束。

### 3. 停止新工作先于清理旧工作

HTTP server 的 graceful-shutdown future 先让 listener 停止接收新连接，
进程级 shutdown signal 同时通知后台观察者。

Workspace runtime 进入 `Draining` 后不再接收新 session/task。
已有工作按 policy 等待 drain；超过边界后取消或强制停止。

关闭过程中不得创建新的 Workspace、Task、approval wait、MCP connection
或 Plugin process 来“完成清理”。

### 4. 持久化与 terminal facts 先于可丢弃资源

Active Turn、Task 和其它 durable lifecycle 的 terminal/interrupted facts
应在对应 runtime 的 shutdown 中先提交，再释放仅运行时 handle。

server 当前先关闭 Task Runtime、清理 Active Streams，再逆序调用
`LifecycleManager::shutdown_all`。usage stats 在进入总体 shutdown 前执行
dirty flush。单个组件失败被记录，但不能阻止后续组件得到关闭机会。

### 5. 子系统按所有权关闭

进程组合层只调用 owner 暴露的 shutdown/stop，不直接操作其内部 task。

- Task Runtime 关闭自己的 attempts 和 durable state；
- ActiveStreamRegistry 关闭/清理 stream；
- Plugin Registry 分发 shutdown hook 并管理 plugin client；
- LifecycleManager 逆注册顺序关闭 ManagedService；
- Workspace ServiceManager 逆 priority 顺序停止 Agent 服务；
- MCP adapter 调用 manager 的 `shutdown_all`；
- pid file 最后移除。

### 6. 所有长期异步任务必须可取消

新增长期 task 必须接收或派生 `CancellationToken`，并由其 owning subsystem
持有 JoinHandle/registration。drop 并不自动等价于正确 shutdown。

可选 cleanup 必须有 deadline。一个失效的外部进程、网络连接或 plugin hook
不得无限占用总关闭预算。

## Hard invariants

- shutdown phase 只能按既定顺序前进。
- 收到关闭后不得再接受新的业务工作。
- 总关闭时间必须有硬上限。
- 第二次终止信号必须能绕过卡住的优雅流程。
- 单个组件 shutdown 失败不得阻止后续组件关闭。
- 持久化 terminal/interrupted facts 必须先于释放运行时资源。
- 子系统只能关闭自己拥有的 task、connection 和 process。
- service stop 顺序必须与 start 顺序相反。
- 所有长期 async task 必须有 cancellation path。
- 强制退出不能被描述为成功完成所有持久化。

## Authority, persistence, and failure boundaries

进程信号与总体顺序由 `rua-server` 拥有。
通用 phase vocabulary 与 timeout contract 由 `rua-core` 拥有。
Agent workspace drain/stop 由 `rua-workspace` 拥有。
每个业务子系统负责自己的 durable terminal facts 和资源释放。

优雅关闭中的错误进入日志与 shutdown report，流程继续。
总 watchdog 到期或第二信号触发时可直接退出，此时只保证已经完成的阶段，
不保证尚未执行的 flush。调用方不得把 watchdog 退出解释为 clean shutdown。

## Consequences

- 正常退出有机会保存状态、终止子进程并移除 pid file。
- 卡住的外部资源不能无限阻塞桌面或服务进程退出。
- shutdown 实现必须保持幂等或容忍部分阶段已执行。
- 组件增加时需要同时定义 startup ownership、shutdown hook 和 timeout。
- 通用 phase controller 与 server 具体编排之间需要测试保持语义对齐。

## Rejected alternatives

- 收到信号立即 `process::exit`：会截断持久化与子进程清理。
- 无限等待所有工作自然完成：无法保证操作系统和桌面退出。
- 并行关闭所有服务：会破坏依赖逆序与持久化先后关系。
- 由每个 endpoint 自行清理：无法覆盖后台任务和非 HTTP 入口。
- 仅依赖 Drop：async flush、kill-tree 和 durable commit 无法保证完成。
- 某组件失败即停止整个 shutdown：会泄漏其后的独立资源。

## Implementation and verification

当前实现路径：

- phase 与 timeout contract：`libs/rua-core/src/runtime/shutdown.rs`
- phase unit tests：
  `libs/rua-core/tests/unit_runtime/shutdown_controller.rs`
- server signal 与 graceful orchestration：
  `crates/rua-server/src/shutdown.rs`
- Axum graceful shutdown/watcher wiring：`crates/rua-server/src/app.rs`
- process ManagedService 生命周期：`crates/rua-server/src/lifecycle.rs`
- Workspace lifecycle types：`crates/rua-workspace/src/lifecycle.rs`
- Workspace service stop：`crates/rua-workspace/src/service_manager.rs`
- Workspace registry drain：`crates/rua-workspace/src/registry.rs`

验证应覆盖 phase 顺序和 wire name、配置 timeout、逆序 stop、
无活跃工作时立即 drain、超时后取消、组件失败继续、第二信号和 watchdog。

### Current implementation gaps

- `crates/rua-server/src/shutdown.rs::graceful_shutdown` 当前按 Plugin shutdown
  hook、Task Runtime、Active Streams、`LifecycleManager::shutdown_all`、
  pid file 的顺序直接调用，没有创建或推进 `ShutdownController`，也没有实现
  七阶段 report。
- `ShutdownConfig` 的 persistence/subsystem/flush timeout 目前只存在于
  `libs/rua-core/src/runtime/shutdown.rs` 的独立状态机和单元测试中，
  server orchestration 没有消费这些阶段 deadline。
- `crates/rua-server/src/lifecycle.rs::start_background_tasks` 使用
  `tokio::spawn` 创建永久循环，但不保留 JoinHandle，也不接收
  `CancellationToken`，因此无法由 owner 在 shutdown 中显式取消和等待。
- 总 watchdog 与第二信号强制退出已经存在，但它们只提供最终上限，不能替代
  persistence、subsystem、observability 和 power-resource 阶段的实际接线。

### Agent lifecycle clarification

ADR-0001 §2.1–2.2 已确定 Agent 真实资源生命周期与配置局部应用规则。
这些规则不等于本 ADR 的完整七阶段进程关闭方案已被接受或接线；本 ADR
仍为 Proposed。进程级资源不由 per-Agent Adapter 假装启停，Workspace
镜像计数不能替代 Conversation Authority/Task Runtime 的终止事实。

本轮审查还确认 Registry `remove`/`shutdown_all` 未执行真实资源关闭，后者
忽略 ShutdownPolicy 并无条件报告完成；必须在实际清理、策略执行及失败/超时
报告验证后才能声称符合关闭契约。

## Related decisions

- ADR 0001: Workspace 分层依赖方向与进程组合
- ADR 0003: Runtime Authority 与外部进程执行环境
- ADR 0013: MCP Per-Agent Manager 隔离
- ADR 0014: Plugin process and protocol isolation
- ADR 0031: Durable Task Runtime and Attempt state machine
- ADR 0034: Interactive Terminal Session and Viewer Runtime
- ADR 0049: Tauri Desktop Shell and local resident server
