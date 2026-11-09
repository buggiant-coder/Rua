# ADR 0010: Tool 输出可见限制与 Raw Capture

**Status:** Accepted

## Context

Tool output 同时面向三个不同目标：

- 防止 subprocess stdout/stderr 在内存中无限增长；
- 为当前 Model Call 提供足够但有界的可见内容；
- 在需要时允许 Agent 从持久化 archive 取回完整输出或切片。

单一 byte limit 无法同时满足这些目标。UTF-8 文本、代码、JSON 和中文的
byte/token 比例不同；把 raw capture cap 当作模型可见 cap 会过早截断；
先完整 materialize 再截断又可能导致内存峰值。

Conversation 的旧 Tool Result 还会在 Context Projection 中再次压缩。
执行时限制、历史投影截断和 History Replacement 是三个不同生命周期，
不能由一个配置项控制。

## Decision

### 1. Tool output 使用三层独立边界

顺序为：

1. Tool-owned semantic pagination 或 continuation；
2. Raw Capture byte hard cap；
3. execution-time model-visible token limit。

默认值由 `rua-core::tool` 定义：

- `DEFAULT_TOOL_RAW_CAPTURE_MAX_BYTES = 1 MiB`；
- `DEFAULT_TOOL_EXECUTION_OUTPUT_MAX_TOKENS = 10_000`；
- `DEFAULT_TOOL_RECALL_SLICE_MAX_TOKENS = 10_000`；
- 历史旧结果的默认 byte cap 为 3,000 bytes。

Agent running config 可调整这些值，但每个值保持独立语义。

### 2. Raw Capture 在读取 stream 时即有界

subprocess stdout/stderr 使用 `RawCaptureBuffer` 边读边统计总 bytes，
内存只保留有界 head 和 tail。超限时在两段之间插入明确 omitted marker。

Shell 与 Python executor 可以同时把完整 stream 写到临时 capture file，
供后续 archive 使用。内存中的 retained bytes 仍受 hard cap 约束。

stdout/stderr 可各分配 raw cap 的一部分，不能先把两个完整 stream
读入内存后再裁剪。

### 3. Model-visible output 按 token policy 压缩

`ToolResultOutputProcessor` 使用 active `ToolContext` 的 token estimate
policy 计算当前结果大小。超过 `execution_output_max_tokens` 时，生成
head/tail 可见内容和 typed `TruncationMeta`。

非文本 ContentPart 保留；文本部分被替换为有界版本。
结果 metadata 记录 original bytes、估算 tokens、visible tokens、
raw cap 是否超限和可用 archive key。

### 3a. 面向模型的语义输出先于通用截断

Tool 的模型可见响应是显式的语义投影，不是内部 runtime、存储或 HTTP DTO
的直接序列化。先按操作生成紧凑响应，再对包含包装字段在内的完整可见内容
应用 token policy；不能以删空值或截断大型内部 JSON 代替输出契约设计。
这不修改 Rust HTTP 边界，也不改变原始输出、审计证据或持久化结构。

响应保留操作结果、真实执行状态、有界内容，以及后续操作所需的准确引用。
省略重复输入、重复状态、无意义空值和仅供诊断的内部元数据；不得删掉
路由/控制凭据、partial 标记、错误、截断说明或可用的续读入口。
语义分页必须给控制引用预留空间，不依赖通用 head/tail 截断保全这些字段。

日志呈现可以确定性地清理 ANSI 控制序列、合并相邻同来源文本块，但须保留
观察到的顺序、换行、警告和错误证据，不改变 raw capture。格式整理不需要
新增 LLM 摘要步骤；Chat 的语义进度摘要仍按 ADR-0053 单独管理。
结果引用必须有授权且有界的读取路径，不能仅返回无法使用的内部 URI。

主动读取、前台转后台回执和自动结果通知都遵循此边界，但不强制共用一个
包含所有字段的响应对象。Task 的 Attempt/续读与交付语义见 ADR-0032，
Chat 的请求/停止引用与交付语义见 ADR-0053。

此节是目标契约，不代表当前工具已全部完成语义投影。验收应检查最终
Model Call 中的内容，而不只检查内部 DTO：覆盖正常、失败、无新内容、
大输出、续读和自动通知，验证精简后仍能定位结果并完成下一步操作。

### 4. 完整文本输出进入 Conversation-owned archive

当 `tool_result_store_dir` 可用且 tool call 有稳定 id 时，processor 将完整
one-shot 文本或 stdout/stderr capture files 保存到 `ToolCallArchive`。
archive key 从 tool-use identity 派生。

Archive 当前是文本 recall seam，不持久化 image、audio、file reference 等
非文本 `ContentPart`。这些 part 在当前 `ToolResult` 的文本压缩过程中必须
保留，但它们的长期所有权由各自媒体/资源存储负责，不能把
`recall_tool_result` 描述为通用二进制归档。

若 archive 成功，截断提示提供 `recall_tool_result`。
若 archive 失败，结果仍保持有界，但必须说明完整 raw output 不可取回。

无 conversation ownership 的执行不能假装有持久化 recall。

### 5. Recall 自身保持有界

`recall_tool_result` 可以返回完整结果的 token-bounded slice，并提供下一段
continuation hint。已经是 Recall 或 semantic continuation 的结果不得被
output processor 再包一层 recall marker。

Recall tool 本身不重新 archive 自己的输出，避免 archive 递归和无界复制。

### 6. 历史投影截断是后续独立策略

execution-time result 首先写入 canonical conversation/tool result storage。
在后续 Model Call 的 Context Projection 中，旧 Tool Results 可根据
`projection_truncation_enabled`、recent window 和 `old_max_bytes` 再缩减。

Projection Truncation 不改变原始 archive，也不把执行时 10,000 token
上限重新解释为历史 byte policy。

## Hard invariants

- raw byte cap 与 model-visible token cap 必须是独立配置。
- stream capture 必须边读边限制内存，不能先无界收集。
- 超限必须保留 head/tail 与 omitted 数量。
- 可见截断必须产生 typed metadata 或明确 continuation。
- archive 不可用时不得生成虚假的 recall key。
- `recall_tool_result` 不得 archive 或递归包装自身。
- 非文本 ToolResult content 不得因文本压缩而丢失。
- execution-time 限制不得覆盖历史 Projection Truncation 配置。
- canonical archive 与可见 projection 必须具有同一 tool-use identity。
- cleanup 只能删除过期/无 owner capture，不得影响仍可 recall 的结果。

## Authority, persistence, and failure boundaries

Tool implementation 拥有 semantic pagination。
executor/raw-capture layer 拥有流式内存安全。
`ToolResultOutputProcessor` 拥有当前 Model Call 的可见结果。
`ToolCallArchive` 拥有完整结果持久化与 recall identity。
Context Projection 拥有后续历史缩减。

archive 写入失败不会把 Tool execution 改成失败，但必须清理不再拥有的
临时 capture file，并让结果显示不可 recall。
capture file 写入失败时仍需继续有界内存 capture；不得因此无限缓存 stream。

### Current implementation gap

`capture_stream_with_observer` 当前忽略 capture sidecar 的 `write_all`、
`flush` 和 `sync_all` 错误。只要文件创建成功，返回值仍可能携带该 path，
随后 `ToolCallArchive` 可能保存一个不完整 sidecar 并生成 recall key。该行为
违反“只有完整文本/raw stream 成功归档后才声明可 recall”的不变量。实现应
跟踪 sidecar IO 失败并撤销 path，或让 archive 显式标记 partial capture；
不能把不完整文件称为完整 raw output。

## Consequences

- 大输出不会造成与完整 stdout 等量的内存增长。
- 模型获得比旧 10,000-byte 策略更合理的 token-bounded上下文。
- Agent 可通过 Recall 按需读取完整结果，而不是重复执行有副作用的 Tool。
- archive 与 cleanup 增加磁盘管理成本。
- token estimate 不是 Provider 精确计费，但足以作为可见输出边界。

## Rejected alternatives

- raw 与 visible 共用一个 byte limit：混淆内存安全和模型上下文质量。
- 先完整读取再 truncate：存在无界内存峰值。
- 所有输出直接塞入 Message Log：会使长期 Chat 无法投影。
- 截断后让 Agent 重新执行 Tool：有副作用且浪费执行预算。
- Recall 返回无限完整文本：只是把同一问题移动到下一次 Tool Call。
- 用 Projection Truncation 取代执行时限制：首个 Model Call 仍可能过大。

## Implementation and verification

当前实现路径：

- 默认限制与 ToolContext：`libs/rua-core/src/tool.rs`
- running config：`crates/rua-config/src/types/agents.rs`
- stream raw capture：`crates/rua-tool/src/raw_capture.rs`
- process executor：`crates/rua-tool/src/executor/runtime.rs`
- Python executor：`crates/rua-tool/src/builtin/execute_python_code.rs`
- output processor：
  `crates/rua-tool/src/result_store/output_processor.rs`
- archive：`crates/rua-tool/src/result_store/tool_call_logger.rs`
- truncation metadata：`crates/rua-tool/src/result_store/truncation.rs`
- Recall Tool：`crates/rua-tool/src/extended/recall.rs`
- history projection：`crates/rua-agent/src/projection_tool_results.rs`

验证覆盖：

- `crates/rua-tool/tests/all/command_executor.rs`
- `crates/rua-tool/tests/all/runtime_executor.rs`
- `crates/rua-tool/tests/all/tool_result_output_processor.rs`
- `crates/rua-tool/tests/all/recall_round_trip.rs`
- `crates/rua-tool/tests/all/result_store_compressor.rs`
- `crates/rua-agent/tests/runtime/mcp_tool_output.rs`

## Related decisions

- ADR 0005: Agent Profile、Running Config 与 ResolvedAgent
- ADR 0009: Bounded execution guardrails without a shared budget entity
- ADR 0011: Effective Capability Snapshot 与 Tool Exposure
- ADR 0012: Per-Agent Tool Guard 与进程级 Approval
- ADR 0026: Context Projection owns model-call assembly
- ADR 0027: Durable History Replacement
