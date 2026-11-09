# ADR 0006: Model Knowledge Catalog 与 Reasoning Control

**Status:** Accepted

## Context

不同模型的 context window、输出上限、多模态能力和推理参数并不一致。
Provider wire protocol 也不同：部分模型使用 effort，部分模型使用 token
budget，部分模型不支持请求侧推理控制。

Rua 同时存在三类信息：

- 产品已知的模型能力知识；
- Provider/model 配置中明确保存的能力与默认值；
- Chat model slot 中面向后续 Turn 的推理偏好。

如果运行时每次请求都重新从模型名称猜能力，catalog 会成为隐藏 authority；
如果前端自行生成默认值，多客户端会产生不同结果；如果切换模型时沿用旧
budget/effort，可能向新 Provider 发送无效参数。

Reasoning Control 只描述下一次请求要求模型采用的推理强度。
它不同于模型已输出的 Thinking/Reasoning content，也不同于这些 content
在历史、stream 或 UI 中的保留和显示策略。

## Decision

### 1. Model Knowledge Catalog 是配置补全与 slot fallback 知识输入

`crates/rua-provider/src/model_knowledge.rs` 保存已知 provider/model 组合的
reasoning style、effort options 和 thinking budget range。
通用 model capability baseline 还可提供 context window、最大输出和
多模态能力。

Provider 配置创建、导入和读取时将缺失的已知模型能力与默认值物化到配置；
创建或更新 Chat model slot 时仍可读取 Catalog，作为显式
Provider/model 配置缺失时的 fallback。它不拥有 Provider 凭据，
不保存用户选择，也不持续在每次 Model Call 时覆盖已经物化的 Chat slot。

Catalog miss 不阻止添加模型。未知模型只使用保守基础 fallback，
不得猜测其支持 Reasoning Control。

### 2. ProviderModelEntry 保存模型能力与默认参数

`ProviderModelEntry` 可保存：

- `thinking_enabled`;
- `thinking_budget`;
- `reasoning_effort`;
- `thinking_param_style`;
- `reasoning_effort_options`;
- `thinking_budget_range`;
- 其它 `generate_kwargs`.

显式用户值优先于 catalog。`ProviderConfig::apply_model_defaults` 在配置
ingress 调用 `model_knowledge::apply_missing_defaults`，物化并持久化缺失的
reasoning 字段。`apply_model_catalog_defaults_for_new_model` 仍只补 context
window、输出上限和多模态等通用 capability。未知且没有显式能力配置的
模型不获得 reasoning 默认值；后续 catalog 更新不覆盖已保存的值。

Provider-level capability 可作为 model-level 缺失值的 fallback，
但 model entry 仍是具体模型最精确的配置位置。

### 3. Chat model slot 保存完整推理偏好快照

Chat 的 model slot 与 `{provider_id, model}` 绑定，并保存统一的
`thinking_strength` 以及解析所需的 style/options/range。
兼容输入中的 `thinking_enabled`、`thinking_budget` 和
`reasoning_effort` 会在 authority 侧归一化。

支持的用户强度是 `off`、`auto`、`low`、`medium`、`high`。
authority 根据目标模型的 style 将该强度转换成适用的 provider 参数。

创建或切换 `{provider_id, model}` 时，Conversation Authority 按
model entry、provider-level config、Catalog fallback 和 protocol fallback
的顺序解析缺失 capability，并将归一化结果写入 Chat slot。
切换 `{provider_id, model}` 必须重新依据新模型能力归一化；
不得把旧模型的 budget 或 effort 原样带到新模型。
已存在 Chat slot 是快照，不持续追随后来修改的 model defaults。

### 4. Conversation Authority 验证并持久化 Chat 选择

Chat model 与 Reasoning Control 的读取和修改路由到当前
Conversation Authority Device。该 Device 使用自己的 Provider 配置验证
目标模型存在，并验证 effort option 或 budget range。

控制端 Device 不得用本机 catalog/Provider 状态替远端执行 Device 做决定。
变更只影响后续 claim 的 Turn，不修改已在执行中的 Turn。

### 5. Runtime 从显式配置构造 provider options

Model Call 的有效参数按以下来源合并：

1. Provider `generate_kwargs`；
2. ProviderModelEntry `generate_kwargs` 与明确模型配置；
3. 当前 Chat model slot 的 Reasoning Control。

Runtime 不在这一阶段重新查询 catalog 来改变用户选择。
`auto` 使用模型已保存的默认值或 advanced JSON；显式强度覆盖默认值。
不可关闭推理的已知模型将 `off` 映射到其最低合法 effort/budget。
`llm-multiplex` provider adapter 负责把 canonical 字段转换为具体 wire：
OpenAI-compatible effort、Anthropic thinking、Gemini thinkingConfig 或
兼容 provider 的 extra body。

不适用于目标 adapter 的字段必须被消费或移除，不能盲目透传。

## Hard invariants

- Catalog 是缺失 capability 的补全来源；物化后的 Provider config 和 Chat
  slot 承担各自的持久化 authority，每次 Model Call 不重新查询 catalog。
- Catalog 只填缺失值，不覆盖显式用户配置。
- 未知模型不得被猜测为支持 Reasoning Control。
- Chat reasoning preference 必须绑定 provider/model identity。
- 模型切换不得继承旧模型的 effort 或 budget。
- authority Device 必须验证模型存在和强度合法性。
- Active Turn 的模型与 reasoning snapshot 不得被中途修改。
- 请求侧 Reasoning Control 不得与输出 Thinking block 设置混用。
- provider adapter 只能收到其支持的 wire shape。
- frontend 不得成为默认值或合法性判断的唯一 authority。

## Authority, persistence, and failure boundaries

Model Knowledge Catalog 是代码随版本发布的 fallback 知识。
Provider/model 配置对其中已保存的模型能力和默认参数具有持久化
authority，包括配置 ingress 时物化的 Catalog 默认值。
Chat model slot 是该 Chat 后续 Turn 选择的持久化 snapshot。
Conversation Authority 是 mutation 与执行前验证 authority。

catalog miss 只减少自动补全，不使模型配置失败。
目标 Provider/model 不存在、strength 不支持、budget range 无效时，
Chat mutation 必须失败且保持旧 slot。
provider adapter 的 wire 构造失败属于 Model Call failure，不回写 catalog。

## Consequences

- 模型发现和手工添加得到一致的能力补全。
- Chat 可以独立选择推理强度，而不修改全局 Provider model config。
- 跨设备控制以实际执行 Device 的模型可用性为准。
- Provider 差异被限制在 adapter，而 Chat/API 使用统一概念。
- catalog 需要随主流模型演进，但过期条目不会覆盖已有用户配置。

## Rejected alternatives

- 每次 Model Call 按 model id 猜能力：会让 catalog 成为隐式动态 authority。
- 前端硬编码模型列表和默认值：多客户端会漂移且无法校验远端 Device。
- 单独建立 Chat reasoning endpoint/object：会把同一 model slot 拆成双状态。
- 切换模型时保留旧 strength 的 provider 原始字段：容易产生无效请求。
- 将 output thinking visibility 当作 reasoning strength：混淆请求与结果。
- 对未知模型默认开启推理：没有能力证据，可能被 Provider 拒绝。

## Implementation and verification

当前实现路径：

- 模型知识：`crates/rua-provider/src/model_knowledge.rs`
- model 配置 DTO：`crates/rua-provider/src/config_loader.rs`
- 通用 capability default 与有效 kwargs：
  `crates/rua-provider/src/manager.rs`
- Chat model DTO：`crates/rua-server/src/endpoints/dto/mod.rs`
- Chat model API 归一化：`crates/rua-server/src/endpoints/chats.rs`
- authority-side model mutation：
  `crates/rua-server/src/services/conversation_input_intake.rs`
- provider wire adapters：`libs/llm-multiplex/src/providers/`

验证覆盖：

- `crates/rua-provider/tests/contract_providers/`
- `crates/rua-server/tests/contract_chats_http/chats_lifecycle.rs`
- `crates/rua-server/tests/contract_projects_http/projects_routes.rs`
- `crates/rua-agent/tests/runtime/skill_exposure.rs`
- `libs/llm-multiplex/tests/`

测试必须覆盖 catalog miss、显式值不被覆盖、effort/budget validation、
model switch reset、authority Device 校验和各 provider wire shape。

`crates/rua-server/src/endpoints/chats.rs` 和
`crates/rua-server/src/services/conversation_input_intake.rs` 在 Chat slot
归一化时保留 `model_knowledge::lookup` fallback；Provider 配置 ingress 已
补齐已知模型默认值。目录来源与更新流程见
[catalog update workflow](../../scripts/INDEX.md#upstream-catalogs)；协议映射由
上述 provider wire adapters 与契约测试维护。

## Related decisions

- ADR 0003: Runtime Authority 与外部进程执行环境
- ADR 0005: Agent Profile、Running Config 与 ResolvedAgent
- ADR 0008: Agent Work Contract 与 Collaboration Mode
- ADR 0011: Effective Capability Snapshot 与 Tool Exposure
- ADR 0022: Conversation Authority, placement, and epoch fencing
- ADR 0028: Chat Running Config and Turn-stable snapshot
