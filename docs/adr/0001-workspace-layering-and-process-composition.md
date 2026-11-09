# ADR 0001: Workspace 分层依赖方向与进程组合

**Status:** Accepted

## Context

Rua 是一个多 crate Rust workspace。它同时包含基础库、业务子系统、
运行时组合层、HTTP 服务、统一 CLI、桌面壳和移动端宿主。
如果这些层之间没有稳定的依赖方向，业务 crate 会反向依赖进程入口，
基础 trait 会被具体实现污染，测试也只能通过启动完整服务验证。

当前 workspace 的根 `Cargo.toml` 将代码分为两类：

- `libs/` 保存可复用的基础类型、trait 和无产品入口的基础设施；
- `crates/` 保存业务实现、运行时适配、服务组合和二进制入口。

`AGENTS.md` 明确规定依赖方向为 `crates/ -> libs/`，禁止基础库反向依赖
应用 crate。`rua-core`、`rua-contract` 和 `agent-types` 位于依赖图上层，
分别承载通用领域类型与运行时基础、冻结的跨 crate trait，以及 LLM 消息类型。

产品同时需要两种组合尺度：

- 进程级对象，例如配置 Cell Store、Approval Authority、Task Runtime、
  Plugin Registry、Project Catalog 和 HTTP AppState；
- Agent 级对象，例如 Workspace Runtime、Agent Runtime、MCP Manager、
  Memory/Context Manager、Agent ToolKit 和 Channel Manager。

二者不能混为一个全局 service locator。进程级状态必须只创建一次，
而 Agent 隔离状态必须按 Agent 创建、缓存、重载和关闭。

## Decision

### 1. 保持单向 workspace 分层

`libs/` 不依赖 `crates/`。跨业务边界需要共享的类型和 trait 放入最窄的
基础 crate，而具体实现留在拥有该行为的应用 crate。

`libs/rua-contract` 是跨 crate 的稳定 trait 边界。修改其公共 surface
会扩大整个 workspace 的编译与行为影响，因此不得为了局部便利把业务实现、
HTTP DTO 或进程状态下沉进去。

`libs/rua-core` 保存跨子系统的领域值与运行时原语，但不负责组装具体
Provider、Agent、MCP、Channel、Plugin 或 Server。

### 2. `rua-workspace` 是 Agent 运行时组合层

`crates/rua-workspace` 依赖所需的业务 crate，并将它们组装为
`WorkspaceRuntime`。一个 `WorkspaceRuntime` 对应一个 Agent 工作空间，
持有该 Agent 的真实运行资源引用及生命周期状态。工作执行和终止事实仍由
Conversation Authority、Task Runtime 等所属领域拥有，不另建 Workspace 队列
或以未接入生产执行的镜像计数决定排空完成。

`WorkspaceRuntime` 不是进程入口，也不是全局配置容器。它通过
`WorkspaceContext` 接收进程级共享依赖，通过 `ServiceManager` 管理
Agent 级服务，并通过 `WorkspaceRegistry` 按 `agent_id` 获取或创建实例。

`DefaultWorkspaceRegistry` 是进程内 Agent runtime 的索引和生命周期入口。
生产组合调用方依赖 `WorkspaceRegistry` trait，而不是自行构造并缓存
`WorkspaceRuntime`。`WorkspaceRuntime::new` 仍是公开构造器，供 registry、
测试和受控 adapter 使用；公开可构造不代表生产调用方可以建立第二套
runtime authority。

### 2.1 生命周期只管理真实资源

Agent 级生命周期只管理具有真实初始化、停止或清理行为的资源，不预注册
`NoOpService`，也不以只读对象长度或修改状态的 Adapter 冒充资源启动。
Provider、Toolkit 等对象的存在与能力展示，不自动构成 per-Agent 启停职责。
进程共享 Provider、Cron scheduler 等资源仍由进程组合根管理。

真实资源装配与生命周期记账使用同一流程；尚未装配、明确禁用、启动成功和
启动失败分别表达，不以占位成功报告 Ready。通用生命周期状态只有一个
记账所有者，真实运行健康探测与启动结果不得混为一谈。

同 Agent 的初始化、资源重配置与关闭由 owner 协调，防止交错或重复启动；
状态查询不持锁等待外部启动或停止。Registry 在创建前保证 per-Agent
single-flight，不同 Agent 的创建不因全局索引锁串行化。

移除实例和关闭必须执行真实清理，不能通过 `mark_stopped` 或删除索引代替。
停止接收工作、请求取消、工作终止和资源释放是不同事实；报告只包含实际完成
的动作，超时或清理失败须如实报告，并继续清理其他独立资源。

### 2.2 配置更新不默认重建 Runtime

普通配置提交不是全量重建 `WorkspaceRuntime` 的理由。配置应用层根据有效变化
更新所属资源或缓存，本地写入与同步落地遵循相同规则；无变化不刷新，未加载
的 Agent 不为应用配置创建 Runtime，已加载实例不因普通配置更新被替换。

配置捕获严格遵守 ADR-0005/0028：Agent 默认值是未来 Chat 的模板，现有 Chat
不实时继承它；显式 Chat 配置更新发布新 epoch，已 claim 的 Turn 保持原快照。
长期资源不得持有陈旧的执行策略快照，并依靠全量重建使其生效。

MCP、Channel、Memory/Embedding 等有状态资源由各自 owner 局部重配置；
Skills 与 prepared 缓存按实际依赖刷新，维护周期只更新所属任务。
Channel 更新不得连带重建 Memory、MCP 或进程 Cron scheduler。
显式全量重建若保留，必须有独立的管理用途和真实结果报告，不能作为配置
应用或缓存失效的通用后处理。

配置持久化成功与运行资源应用成功分别报告。资源应用失败不自动回滚已提交
配置，须明确报告已保存但未生效，并允许重新应用当前配置。失败能力不得
自动使用旧地址、旧凭据或已禁用资源接收新工作；无关能力继续运行。
配置提交不统一取消已有工作，底层资源失效时由所属执行领域如实终止和报告。
这些资源可用性规则不修改已经捕获的 Turn 配置或安全授权快照。

Embedding 模型或维度变化不得混用新查询向量与不匹配的旧索引；保留原始
记忆内容，在匹配前限制向量检索，不隐式替换明确要求的检索方式。
仅地址或凭据变化不默认重建索引，具体模型身份与索引更新由 Memory 领域负责。

### 3. `rua-server` 是常驻进程组合根

`crates/rua-server` 负责创建 `AppState`、进程级单例、HTTP 路由、
后台服务和 `WorkspaceRegistry`。HTTP endpoint 只通过注入的状态和领域服务
访问运行时，不自行建立第二套 Agent、MCP、Approval 或持久化 authority。

独立 `rua-server` binary 调用 `rua_server::serve`。
统一 `rua` CLI 的 `server` 子命令也调用同一 server library；
Android 打包路径只携带统一 `rua` executable，并通过
`rua server start --profile mobile-apk` 启动常驻服务。

CLI 的普通管理命令优先作为 resident server 的客户端。只有明确支持的
standalone 路径可以在本进程构造运行时，而且不得与已运行的 server 同时
打开同一批 SQLite、Cell Store 或同步存储。

### 4. 入口壳不拥有业务行为

Console、Tauri Desktop Shell、移动 APK shell 和 CLI 参数解析是入口或
控制面。Agent、Channel、Cron、Sync、Security、Task 和 Workspace 行为
仍由 Rust 业务 crate 与 server 组合层拥有。

前端或平台壳可以发起命令、展示状态和管理生命周期，但不得成为后端领域
状态的第二 authority。

## Hard invariants

- `libs/` 不得依赖 `crates/`。
- 具体业务实现不得放入 `rua-contract` 的冻结 trait registry。
- 一个进程中的生产 Agent runtime 必须由 `WorkspaceRegistry` 索引和交付；
  测试与受控 adapter 可以直接构造隔离实例。
- Agent 级可变状态不得提升为无 Agent 维度的进程全局单例。
- 进程级 authority 不得在每个 `WorkspaceRuntime` 中重复创建。
- HTTP endpoint、CLI adapter 和 UI 不得直接重建领域 manager。
- resident server 存在时，CLI 不得并发打开同一持久化根。
- Android resident runtime 是统一 `rua` binary 内的 server 路径，
  不是 APK 内另一个独立 `rua-server` payload。
- crate 边界表达所有权和依赖方向，不以减少文件数量为理由破坏它。

## Authority, persistence, and failure boundaries

进程组合 authority 位于 `rua-server::AppState` 的构造路径。
Agent runtime authority 位于 `WorkspaceRegistry` 和对应
`WorkspaceRuntime`。领域持久化仍由各自 store/repository 拥有，
`rua-workspace` 只组合，不接管其数据格式。

Agent runtime 初始化失败只使该 workspace 进入 `Failed` 或降级状态；
不应通过临时创建第二个全局 manager 掩盖失败。进程级关键依赖创建失败可阻止
server 启动。可选服务失败由生命周期状态和诊断表达，不改变依赖方向。

关闭时先停止接收新工作，再由进程组合层协调 Task、stream、Workspace 和
Plugin 等子系统；单个业务 crate 只关闭自己拥有的资源。

## Consequences

- 新业务能力通常落在独立 `crates/rua-*` 中，并通过 trait 或窄类型接入
  `rua-workspace`/`rua-server`。
- 单元与 contract 测试可以在 crate 边界验证，不必总是启动完整服务。
- 进程级和 Agent 级对象的构造位置可直接反映隔离语义。
- `rua-workspace` 与 `rua-server` 的依赖较多，这是组合根的预期成本。
- 修改基础 trait 的影响范围较大，必须先验证所有实现和调用方。

## Rejected alternatives

- 把所有实现放入单一 crate：会失去依赖方向、隔离测试和 feature 裁剪能力。
- 让 `rua-server` endpoint 直接构造各 manager：会产生重复 authority，
  并使 CLI、Task 和 Channel 路径行为漂移。
- 把所有状态放入一个全局 service locator：会抹去 per-Agent 隔离边界。
- 让每个 binary 各自实现业务组合：会使 `rua-server` 与统一 `rua`
  的 server 行为分叉。
- 为减少依赖而把业务类型复制进 `libs/`：会形成多份协议和转换层。

## Implementation and verification

当前实现路径：

- workspace 成员与共享依赖：`Cargo.toml`
- 基础层约束：`libs/rua-core/`、`libs/rua-contract/`、
  `libs/agent-types/`
- Agent runtime 组合：`crates/rua-workspace/src/runtime.rs`
- runtime registry：`crates/rua-workspace/src/registry.rs`
- 生命周期服务组合：`crates/rua-workspace/src/service_manager.rs`
- 进程状态组合：`crates/rua-server/src/state.rs`
- server library 入口：`crates/rua-server/src/lib.rs`
- 独立 server binary：`crates/rua-server/src/main.rs`
- 统一 CLI 入口：`crates/rua-cli/src/main.rs`
- CLI server adapter：`crates/rua-cli/src/server.rs`

验证应检查：

- `cargo metadata --no-deps` 中不存在 `libs/* -> crates/*` 路径依赖；
- `WorkspaceRegistry` 的测试覆盖按 Agent 创建、获取、重载和关闭；
- server 与统一 CLI 使用同一 `rua_server::serve` 组合路径；
- touched-crate 检查使用 `scripts/tests/clippy-touched.sh`，不运行全 workspace
  的本地 clippy/test。

### Current implementation gaps

以下核心缺口已修复并有聚焦行为验证，但完整 PRD 尚未通过最终验收：

- Registry 使用 per-Agent 操作闸门和创建结果代次，成功并发获取共享实例，
  同次失败共享结果且后续允许重试；不同 Agent 并行初始化。
- 已删除生产 `NoOpService`、Provider/Tools/Cron 空生命周期包装、未消费
  `init_phase`、Adapter 重复状态及无生产用途通用 reload/reusability。
- Registry 移除和关闭执行真实清理，失败保留 owner 供重试；关闭报告区分
  终态确认、未解决工作、资源失败和超时，不宣称未执行的 flush。批量关闭
  使用同一期限。Task admission 和 Conversation 接纳闸门接入所属 authority。
- Running、Channel 与同步普通配置应用不再默认重建 Runtime。MCP 局部
  首次装配、失败重试、未应用资源新调用拒绝及 Console 重试已有行为验证。
- Runtime/ServiceManager 操作协调与短状态锁分离，慢启动时可观察状态；
  显式重建先清理旧实例，成功初始化后由 owner 恢复接纳。
- Memory 搜索策略来自调用快照；Embedder 局部更新并检查索引模型/维度身份。
  Skills 和维护周期改为所属能力刷新，同步复用资源应用逻辑并记录失败诊断。

剩余缺口包括初始化/重建被调用方取消后的未发布资源最终清理、所有工作
接纳与关闭竞争的完整生产覆盖、自动连接状态与配置注册结果的准确展示、
资源应用失败的完整端到端重试及实际平台 E2E。完整 touched tests 和 release
gate 当前仍有失败，不得据聚焦测试通过声称实现全部符合本决策。

验收应补充真实资源只启动一次、移除执行清理、失败继续清理、无占位 Ready、
配置更新保持无关实例身份、本地与同步应用一致、无变化不刷新、资源失败不
假报生效，以及 Chat/Turn 快照边界测试。源码字符串测试不能代替行为验证。

## Related decisions

- ADR 0002: Runtime Layout 与 Agent Path Contract
- ADR 0004: 配置持久化 Cell Store
- ADR 0005: Agent Profile、Running Config 与 ResolvedAgent
- ADR 0007: 进程生命周期和有界分阶段关闭
- ADR 0013: MCP Per-Agent Manager 隔离
- ADR 0016: Channel Trait、实现隔离与 Feature Gates
- ADR 0028: Chat Running Config and Turn-stable snapshot
- ADR 0049: Tauri Desktop Shell and local resident server
