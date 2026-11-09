# ADR 0009: Bounded execution guardrails without a shared budget entity

**Status:** Accepted

## Context

一个用户工作请求可能包含多次 Model Call、并行 Tool Calls、SubAgent、
Task Attempt 和同步 Hook。若每个子路径拥有无限且独立的执行额度，
父 Agent 可以通过拆分工作绕过限制，取消也可能只停止一层。

历史设计曾提出持久化 `budget_id`、Provider token ledger、Task lineage
继承和进程级 Execution Budget Registry。当前代码已经移除该实现：
workspace 中不存在 execution-budget store、budget identity 或 Provider usage
扣账 registry。

当前运行时实际使用多种有界机制：

- ReAct `max_iterations`；
- 可选 `total_timeout_secs`；
- model inactivity timeout；
- 可选 orchestrator tool timeout；
- tool 自身 timeout 和 raw/output limits；
- parallel tool count；
- `CancellationToken` propagation；
- Task Runtime concurrency、Attempt lifecycle 和 cancellation；
- Provider context/token limits；
- product Hook 的 caller-provided remaining Turn duration。

本 ADR 接受当前架构选择：不建立持久化 Shared Agent Execution Budget
业务实体。迭代、超时、并行、Task、Hook 和 cancellation 限制分别由各自
owner 管理；局部调用链可以传递剩余时长，但不能据此宣称存在全局共享
ledger。

## Decision

### 1. 当前不存在 Shared Agent Execution Budget 业务实体

已删除的设计曾让一个顶层工作及其子工作共享 `budget_id`、Provider usage
ledger 和跨重启余额。当前实现没有这些对象，也没有可权威查询的 shared
remainder。

当前执行边界由 `ReactConfig`、Tool/Hook 配置、Task concurrency 与
cancellation、Provider 限制共同构成。它们是并列 guardrails；任何单一字段
或它们的集合都不构成 Shared Agent Execution Budget。

### 2. ReAct loop 使用迭代、超时和并行上限

`ReactConfig` 提供：

- `max_iterations`：Tool-use rounds 的硬上限；
- `total_timeout_secs`：显式有界 run 的总时限，0 表示 normal Chat 不启用；
- `model_timeout_secs`：模型 stream event 之间的 inactivity timeout；
- `tool_timeout_secs`：可选 orchestrator tool deadline；
- `max_parallel_tools`：单轮并行 Tool 上限；
- `auto_continue_max_retries` 和 compaction retry 上限。

Normal Conversation Turn 默认不设置统一 wall-clock deadline。
模型持续产出时可以超过单次 inactivity interval。

`model_timeout_secs` 通过 `ChatModel::chat_stream_with_idle_timeout` 传递到
每次 Provider request。每次重试从完整 inactivity interval 开始；retry backoff
和 rate-limit queueing 不消耗此 interval，后者由自己的 acquire timeout 约束。
Retry、fallback、usage recording 和 rate-limit wrappers 必须转发该时限，
不得在整组尝试外再套相同的 inactivity timer。

尚未输出内容的超时由 Provider retry policy 处理，遵守配置的 retry count；
耗尽后保留最后的 Provider error，ReAct 不再重新启动整组 retries。
已经输出部分内容时，保留 ReAct 的一次恢复机会，先发出 `ModelCallReset`
再重试；最终失败仍保留 partial response。

显式 `RetryPolicy.max_total_time` 对 streaming 的整组请求和 backoff 生效，
不在新尝试开始时重置。若在 backoff 中耗尽，返回之前的实际错误；若仍在请求，
返回不可自动重试的 `RetryDeadlineExceeded`。显式 `total_timeout_secs` 和
cancellation 始终能终止请求或 backoff；model timeout、retry deadline 与整轮
执行超时必须在错误中区分。

### 3. 协作以一层委派和在途请求准入保持有界

ADR-0053 移除独立 Subagent 执行器，用预配置只读 Agent 替代。来源 Chat
有在途协作请求上限：排队、执行、等待审批均计数，满额在接受前拒绝，不建
溢出队列。历史／空闲 Chat 不占额度。Task 依赖图与执行并发限制独立保留。
主 Chat Stop 不递归停止独立协作 Chat；进程关闭仍按各实际执行 owner 清理。

以下仅记录待移除的实现路径，不是目标执行约束：

`crates/rua-agent/src/subagent.rs::execute_subagent` 创建独立 ReAct
runtime，并使用 `SubAgentConfig` 的 `max_iterations`、`timeout_secs` 和
Tool whitelist。其总 timeout 同时进入 `ReactConfig`，并包裹整个 stream。

durable Task 的 `TaskSpec` 与 `TaskRunnerSpec::SubAgent` 当前不携带
`max_iterations` 或 total timeout；`SubAgentTaskRunner` 也只转发
instruction、context、dependency、session 和 cancellation。因此不能把
legacy direct SubAgent 的边界描述为 durable Task/SubAgent 的统一保证。

Task Runtime 当前拥有 process-level concurrency、Attempt lifecycle 和
runner cancellation，但没有通用 per-Task total timeout。Retry 创建新 Attempt，
但不把已经发生的副作用当作未发生。

父工作 cancellation 必须传播到仍由其拥有的子执行。
detach 或 durable background Task 只有在 Task contract 明确接管 ownership
后，才可脱离前台 Turn 的直接等待。

### 4. Provider usage 只作观测，不作当前共享扣账

Provider 返回的 token usage 可进入 usage reporting、诊断和 UI，
但当前不用于阻止下一次 Model Call，也不形成跨 foreground/Task 的共享余额。

不得使用 Context Projection token estimate、输出长度或金额估算伪造
Provider usage。未来若重新引入 token ledger，需要新的 ADR、持久化 schema、
idempotency 和并发 overshoot 规则。

### 5. Hook 只能消费调用方给出的剩余 Turn 时间

同步 pre-terminal Hook 和 Candidate Completion Hook 接收
`remaining_budget: Duration`。每个 handler 的 deadline 是其自身 timeout
与 remaining budget 的较小值。

Hook continuation 不重置剩余时间。budget 为零时 Hook execution 失败，
不能通过启动另一个 handler、另一个 Turn 或另一个 runtime 获得新额度。

当前 30 秒 caller 值是具体 Hook 调用边界，不构成持久化 Agent token budget。

### 6. `max_iters` 是最后防线，不是成本模型

`max_iters` 防止零 usage、Provider 缺失 usage、重复 Tool round 或异常 loop
无限运行。它不衡量模型 token、金额或 wall-clock，也不能替代 Tool timeout、
Task concurrency 或 cancellation。

`max_iters = 0` 显式关闭工具轮次上限；正整数限定一次执行的工具轮次。
Agent 默认配置、Chat 配置、HTTP 与 Console 均接受并保留 0。
0 不强迫模型持续调用：模型正常结束、用户 Stop 和执行错误仍可结束执行。
达到正数上限时持久化 `max_iterations` Conversation Runtime Terminal Note，
说明停止原因并保留后续继续所需的事实。

## Hard invariants

- 当前系统不得宣称存在持久化 `budget_id`、shared remainder 或
  execution-budget ledger。
- 不得把互相独立的 runtime guardrails 重新包装成一个已实现的
  Shared Agent Execution Budget。
- normal Chat 不得因默认值被意外套上总 wall-clock timeout。
- 显式 bounded run 的 total timeout 不得在每个内部阶段重置。
- 协作只有一层且受在途请求上限限制；不新增独立 Subagent 或共享预算实体。
- cancellation 沿实际执行 owner tree 传播，单 Chat Stop 不级联独立协作 Chat。
- retry 不得假设先前 Attempt 没有副作用。
- Hook timeout 必须受 remaining Turn budget 上限约束。
- Provider usage 与 Context Projection estimate 不得混算。
- `max_iters` 不得被描述为 token 或 monetary budget。
- 并行调用已经开始后的有限 overshoot 必须被视为显式 tradeoff。

## Authority, persistence, and failure boundaries

`ReactConfig` 拥有单个 Agent run 的 loop limits。
Tool implementation 拥有 operation-specific timeout 与 cleanup。
Task Runtime 拥有 Task/Attempt concurrency、lifecycle 和 cancellation；
当前不拥有通用 per-Task total timeout。
Provider 拥有 context/output token hard limits。
Hook caller 拥有传入的 remaining duration。

这些限制各自是运行时 authority，不持久化为共享余额。
Task durable records 保存 lifecycle 和结果，不保存已删除的 budget ledger。

达到 iteration、timeout、cancellation 或 Provider limit 时，应产生 typed
partial/failed/interrupted outcome，并停止启动新的同属工作。
不得为了生成“最终总结”额外越过已经耗尽的硬限制。

## Consequences

- 当前实现没有 Shared Agent Execution Budget 的数据库和 token accounting
  一致性负担。
- legacy direct SubAgent 有显式限制；durable Task 仍需补齐统一的
  iteration/total-timeout contract。
- Provider usage 可独立演进为观测能力。
- 不同限制的语义必须在 API 与测试中保持清楚，不能都叫 timeout。
- 若产品需要真正跨 Task 的 token quota，必须重新设计而不是恢复旧字段。

## Rejected alternatives

- 恢复旧持久化 token ledger 而不重做实现：当前没有对应代码或 schema。
- 用 Context Projection estimate 扣账：estimate 不是 Provider 实际 usage。
- 只依赖 `max_iters`：一次 iteration 的成本可以相差数个数量级。
- 每个子 Agent 获得全新无限额度：会允许父工作通过 fan-out 绕过限制。
- normal Chat 默认固定 wall-clock timeout：长时间但持续产出的工作会被误杀。
- Hook 获得独立于 Turn 的完整 timeout：会让 extension 延长终止路径。

## Implementation and verification

当前实现路径：

- ReAct limits：`crates/react-agent/src/config.rs`
- loop enforcement：`crates/react-agent/src/react_loop.rs`
- per-attempt model inactivity：`libs/llm-multiplex/src/traits.rs`
- Provider retry/backoff deadline：`crates/rua-provider/src/retry.rs`
- cancellation ownership：`crates/react-agent/src/context.rs`
- bounded SubAgent：`crates/rua-agent/src/subagent.rs`
- Agent Task adapter：`crates/rua-workspace/src/task_agent_execution.rs`
- Task lifecycle/cancellation：`crates/rua-task/src/runtime.rs`
- Hook remaining duration：
  `crates/rua-server/src/services/product_hook_execution.rs`
- Candidate Completion budget：
  `crates/rua-server/src/services/candidate_completion.rs`

验证覆盖：

- `crates/react-agent/tests/all/model_call_protocol.rs`
- `crates/rua-provider/tests/unit_retry/stream_retry.rs`
- `crates/rua-provider/tests/usage/usage_recording_stream.rs`
- `crates/rua-provider/tests/fallback/fallback_chat_model.rs`
- `libs/llm-multiplex/tests/all/rate_limited_stream.rs`
- `crates/react-agent/tests/all/tool_parallelism.rs`
- `crates/rua-agent/tests/runtime/task_runner.rs`
- `crates/rua-workspace/tests/unit_workspace/task_agent_execution.rs`
- `crates/rua-task/tests/unit_runtime/runtime.rs`
- `crates/rua-server/tests/contract_product_hooks_http/`

仓库级检查还应确认不存在 `execution-budgets/`、`budget_id` 或旧 registry
的生产引用。

### Current implementation gaps

- legacy direct SubAgent 路径有 `SubAgentConfig` 边界，但 durable
  `TaskRunnerSpec::SubAgent` 和 `TaskSpec` 没有对应字段，不能由 Task 创建者
  指定或审计 iteration/total timeout。
- `crates/rua-agent/src/task_runner.rs::SubAgentTaskRunner` 与
  `crates/rua-workspace/src/task_agent_execution.rs` 没有把统一边界注入
  durable Task execution；`crates/rua-task/src/runtime.rs` 可以持续等待
  runner，直到其自行结束或 cancellation 到达。
- 这是对本 ADR“子执行必须显式有界”不变量的实现缺口，不意味着 durable Task
  可以合法无限运行。修复需要在 durable Task contract 中定义并传播 typed
  limits，且总时限不得在 Attempt 内的 model/tool 阶段重置。

## Related decisions

- ADR 0007: 进程生命周期和有界分阶段关闭
- ADR 0008: Agent Work Contract 与 Collaboration Mode
- ADR 0010: Tool 输出可见限制与 Raw Capture
- ADR 0014: Plugin process and protocol isolation
- ADR 0029: Two-layer Product Hooks
- ADR 0031: Durable Task Runtime and Attempt state machine
