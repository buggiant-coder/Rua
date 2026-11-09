# ADR 0008: Agent Work Contract 与 Collaboration Mode

**Status:** Accepted

## Context

Agent 需要一组稳定的工作约束：控制探索范围、显式处理假设、在复杂任务中维护
计划、完成实现与验证，并在无法继续时给出具体 blocker。
这些约束不能只存在于用户可编辑 prompt 文件，也不能依赖模型输出某个
`[MODE:...]` 标记后自行改变运行时控制状态。

Rua 当前还暴露 `Default`、`Plan`、`Execute` 三个
`AgentCollaborationMode` 值。它们用于选择完整的 Developer instructions，
并通过 Chat 默认值或 Turn override 进入 Model Call Context。

该 enum 是当前已实现的运行时控制状态。Chat metadata 也为更一般的
`harness_configuration` 保留扩展空间，但它尚未取代 Collaboration Mode 的
读写、Turn override 和 Developer section 注入。当前决策只接受现有三个
mode 及其工作契约；未来若由 Effective Harness 接管，必须通过新的 ADR
完整替换这些权威路径，不能在两套机制间隐式分流。

Collaboration Mode 不是安全权限、Provider reasoning strength、ACP Chat Mode、
Task Runner kind 或 tool mode。它只约束 Agent 如何推进当前工作。

长任务还需要可见的结构化进度，但如果为 checklist 创建独立 Plan entity、
独立生命周期和独立持久化 authority，会与 Turn 的 Tool history 产生双重真相。

## Decision

### 1. Agent Work Contract 由 runtime-owned guidance 强制注入

每次 Model Call 都接收由 runtime 选择的完整 Collaboration Mode Developer
section。该 section 承载当前实现中的 Agent Work Contract：

- 只调查阻塞当前步骤的具体未知；
- 允许有证据目标的深入调查；
- 没有新问题时不重复等价检索；
- 多步骤工作使用 `update_plan`；
- 实现任务完成 focused verification；
- 结束前做 requirement-by-requirement audit；
- 报告交付结果、验证证据和真实 blocker。

这些规则由 Rust runtime 注入，Agent prompt 文件只能补充领域内容，
不能移除 mandatory work discipline。

### 2. Collaboration Mode 是 typed runtime control state

`AgentCollaborationMode` 只有：

- `Default`：按任务比例探索或澄清，然后端到端完成普通工作；
- `Plan`：调查、讨论并形成 decision-complete plan/ADR，未经明确授权不改
  tracked product files；
- `Execute`：对充分定义的任务作合理假设，独立实现、验证和交付。

Chat 保存默认 mode，单个 Turn 可提供 override。
effective mode 在 Turn 执行前解析，并作为
`ModelCallSectionKind::CollaborationMode` 加入每次模型调用。

普通 assistant text、tool output 或历史中的 mode 文本都不能修改该状态。
Runtime composition 不得依赖 Mode 增加新能力。若未来 Harness
Configuration/Turn Harness Override 产生等价 Developer guidance，切换必须
一次性更新持久化、Turn snapshot、Context Projection 和 API 权威边界。

### 3. Mode 不授予能力或权限

Plan、Default、Execute 使用同一 Capability Snapshot 和 Tool Guard pipeline。
Mode guidance 可以要求 Agent 不执行某类动作，但不通过隐藏 Tool deny list
实现安全隔离。

Execute 不绕过 approval；Plan 也不保证从技术上移除写工具。
真正的执行权限来自 Tool Exposure、Tool Guard、File Guard、Approval Authority
和进程 sandbox。

### 4. `update_plan` 是普通 Tool 与工作清单

`update_plan` 接收完整有序 checklist snapshot，每个 step 为
`pending`、`in_progress` 或 `completed`，且最多一个 step 为
`in_progress`。空 step 被拒绝。

调用结果作为普通 Tool Use/Tool Result 进入当前 Turn。
当 `conversation_store_dir` 可用时，最新 checklist 还保存为 Chat 的
Active Work Plan snapshot，用于 Context Projection 的有界重注入。

它不是 Collaboration Mode 切换命令，不创建新的业务 Plan entity，
也不拥有独立执行生命周期。

### 5. Work Plan 重注入是 context policy

Active Work Plan 只在 running config 启用
`active_work_plan_injection_enabled` 时参与 Context Projection。
投影根据 message sequence、checkpoint 和 token interval 决定是否重注入，
避免每次 Model Call 无条件重复。

重注入内容是 runtime 生成的工作上下文，不伪装为新的用户消息，
也不改变原 Tool history。

### 6. 协作派工纪律属于 runtime-owned Work Contract

协作 Chat 的选择与派工规则进入 runtime-owned guidance，不只放在用户可编辑
Agent prompt 或单个 Tool description 中。它不增加 Collaboration Mode，
也不充当权限控制。执行者、单层委派和预置 Agent 边界遵循 ADR-0053。

- 简单、可控的工作默认自行完成；只有明确的独立调查、上下文隔离或独立交付
  收益时才委派，不为使用多 Agent 而拆任务。
- 先理解需求、已有契约和关键依赖，再划分工作；边界不清时先做只读调查，
  不把整个未知子系统直接派给可写执行者。
- 讨论、分析和审查不授权写入；可写派工必须处于用户已授权的实施范围内。
- 根据实际可用执行者的标识、用途和有效能力选择，不猜 Agent ID，不在失败后
  擅自替换为权限更大的执行者。协作者发现信息来自当前配置与可用能力。
- 派工交代目标、必要上下文、工作目录、允许修改范围、禁止事项、交付物、
  验证要求及需要暂停询问的条件；独立 Chat 不隐式继承来源对话历史。
- 共享目录不是隔离环境；并行写入需有不重叠的所有权边界，否则串行执行或
  明确使用工作区隔离。相同协作目标的继续工作复用已有 Chat。
- 主 Agent 负责检查交付与验证证据，不把执行者的完成声明直接当验收通过；
  使用有界读取和结果通知，避免无新信息的高频轮询。
- 控制操作仅使用真实返回的目标/版本引用。普通“停止工作”消息不等于 Stop，
  停止失败不得宣称已停止，也不能以重新提交消息替代控制语义。

只读执行者的角色指令强调证据、结论和不确定项；可写执行者强调范围内实施、
改动说明、验证结果及剩余风险。实际权限仍由 Tool Exposure、Tool Guard 和
Approval 等既有机制实施，prompt 不能保证只读或阻止嵌套委派。

此节为待落实的目标契约。验收同时覆盖 runtime 注入和行为场景：简单工作不
无故委派、讨论不派写入、模糊范围先调查、并行写入不冲突、复用协作目标、
控制失败如实报告，以及主 Agent 在交付前检查证据；不能只断言模板包含关键词。

## Hard invariants

- Collaboration Mode 必须是 typed enum，不从模型文本推断。
- Collaboration Mode 只包含当前三个工作语义；新增 mode 需要新的架构决策。
- 一个 Turn 的 effective mode 在执行期间保持稳定。
- Mode 不得改变 Tool Guard、Approval 或 sandbox 权限。
- Mode 与 Harness Configuration 不得成为两套并行 authority。
- `update_plan` 不得切换 mode。
- checklist 每次调用必须提交完整 snapshot。
- checklist 最多一个 `in_progress` step，step 文本不得为空。
- Tool Use/Result 是 checklist 的历史证据，不建立第二份 transcript。
- Active Work Plan 重注入不得覆盖 canonical conversation messages。
- Plan mode 未经明确授权不得修改 tracked product files。
- Execute mode 仍必须验证，不等于跳过调查或安全边界。

## Authority, persistence, and failure boundaries

Chat/Turn metadata 是 Collaboration Mode 的控制输入，
`rua-agent` runtime 是 effective mode 与 Developer section 的组装 authority。
模型输出不是控制 authority。

`WorkspaceSessionStore` 保存 Active Work Plan snapshot；
原始 `update_plan` Tool Use/Result 仍属于 Message Log。
snapshot 可重建或替换，不是独立业务对象。

mode 解析失败应在进入 Agent loop 前返回 validation error。
`update_plan` 参数失败只使该 Tool Call 失败，不改变原 checklist。
Active Work Plan 读取或重注入失败可记录诊断并继续使用 canonical history。

## Consequences

- Agent 的工作纪律不依赖可编辑 prompt 文件。
- Plan 与 Execute 的行为差异可在 Model Call Context 中稳定测试。
- checklist 有用户可见进度，但不引入第二套任务状态机。
- Mode 与安全、推理和 ACP 概念保持正交。
- Base Agent Contract、typed Mode 和 checklist 是当前契约；未来 Harness
  迁移若发生，必须整体替换而不是长期叠加。

## Rejected alternatives

- 让模型通过文本标记切换 mode：输出内容不能成为控制状态。
- 为 Plan mode 建 Tool 名称 deny list：行为指导不应冒充安全策略。
- 为 checklist 建独立业务 Plan 服务和 authority：会与 Tool history 双写。
- 按固定时间或 Read 次数判定“研究过度”：无法区分必要调查与重复检索。
- 把所有产品协议塞入一个静态 System Prompt：难以按 Turn 组合和测试。
- 把 Mission Mode 作为长期工作模型：独立 Mission lifecycle 已不再是当前架构。

## Implementation and verification

当前实现路径：

- mode enum 与 metadata key：`libs/rua-core/src/chat.rs`
- mode Developer templates：`crates/rua-agent/src/collaboration_mode.rs`
- Chat 中的 mode 与 opaque Harness Configuration：
  `crates/rua-chat/src/lib.rs`
- runtime metadata 解析与 section 注入：
  `crates/rua-agent/src/runtime/mod.rs`
- Model Call section ordering：`crates/react-agent/src/model_call_context.rs`
- `update_plan` Tool：`crates/rua-tool/src/extended/update_plan.rs`
- Active Work Plan persistence：`crates/rua-session/`
- projection reinjection：`crates/rua-agent/src/projection_preparation.rs`

验证覆盖：

- `crates/rua-agent/tests/runtime/react_config.rs`
- `crates/rua-agent/tests/unit_hooks/work_checklist.rs`
- `crates/rua-tool/tests/all/update_plan.rs`
- `crates/rua-agent/tests/runtime/incremental_turn_persistence.rs`
- Chat/Project HTTP contract tests中的 `collaboration_mode` cases

## Related decisions

- ADR 0005: Agent Profile、Running Config 与 ResolvedAgent
- ADR 0006: Model Knowledge Catalog 与 Reasoning Control
- ADR 0009: Bounded execution guardrails without a shared budget entity
- ADR 0011: Effective Capability Snapshot 与 Tool Exposure
- ADR 0017: Slash Command 多层调度与 Discovery
- ADR 0026: Context Projection owns model-call assembly
- ADR 0028: Chat Running Config and Turn-stable snapshot
