# ADR 0004: 配置持久化 Cell Store

**Status:** Accepted

## Context

Rua 的 Provider、Cron、Agent、MCP、Tool、Channel、Skill 和 Plugin 配置
分布在多个 JSON 文件中。部分对象一文件一项，部分对象共享一个文件中的
不同 section，例如一个 Agent 的 `agent.json`。这些领域并未全部完成
Cell Store 迁移；本 ADR 定义统一持久化 seam，以及已迁移配置域必须遵守的
契约。

若每个 manager 自行实现内存缓存、读改写、原子落盘和 reload，容易出现：

- API 修改了内存但没有写盘；
- 写盘失败后内存已经变成新值；
- 两个 section writer 覆盖同一文件；
- 外部编辑文件后进程继续返回陈旧缓存；
- caller 忘记调用额外的 `save()`；
- 每个领域复制一套不一致的临时文件协议。

配置持久化需要一个小而明确的基础层，但业务校验、同步发布和领域身份不能
被塞进通用存储。

## Decision

### 1. Cell 是最小读写单位

Cell 由 `(file_path, json_path)` 定位。`json_path = "$"` 表示整个文件，
其它路径表示共享 JSON 文件中的一个 section。

已迁移的业务模块不直接拼 `(file_path, json_path)`，而是通过 typed
Cells Wrapper 暴露 Cell，例如 `CronCells`、
`AgentCells`、`SharedMcpCells`、`AgentToolsCells` 和
`WorkspaceSkillCells`。

Wrapper 拥有业务 key 到物理 layout 的映射；Cell Store 不知道 Provider、
Agent、MCP 或 Skill 等领域概念。

### 2. Cell Store 只拥有一致性与原子文件操作

`libs/cell-store` 的 `CellStore` trait 提供异步：

- `read`;
- `write`;
- `update`;
- `delete`.

Store 负责 JSON path 取值、同文件串行化、缓存一致性和原子文件替换。
schema 校验、查重、引用检查、连接测试、默认值和业务事件仍由 manager 或
Cells Wrapper 的调用方负责。

### 3. Disk authoritative，内存只作 mtime-tagged cache

读取先比较文件 metadata 的 modified time。mtime 未变时可返回缓存值；
mtime 改变时必须重新读取、反序列化并刷新 cache。

进程启动不要求预加载所有 Cell，也不运行通用 file watcher。
用户或 Agent 对配置文件的外部修改在下一次 Cell read 时被观察到。

cache key 是文件路径。缓存保存完整 JSON document 和 mtime，
section read 从同一文档提取，避免不同 section 对同一文件持有互相矛盾的
缓存副本。

### 4. 写入是 disk-first 且原子

写入序列是：

1. 获取该文件的进程内异步锁；
2. 构造完整目标 JSON document；
3. 在同目录创建唯一临时文件；
4. 写入、flush 并 `sync_all` 临时文件；
5. rename 临时文件覆盖目标；
6. 在 Unix 上同步父目录；当前非 Unix 实现不提供同等的父目录 fsync 保证；
7. 重新读取 metadata，并更新 cache 与 mtime。

任何落盘步骤失败时不得更新 cache。调用方收到错误，后续读取仍以旧磁盘
内容或重新探测到的实际磁盘内容为准。

### 5. `update` 覆盖 read-modify-write 临界区

`update` 在一个文件锁内读取旧 Cell、执行 mutator、修改完整 JSON document
并原子写回。锁按 file path 而不是 json path 分配。

因此同一 `agent.json` 的不同 section 更新也会串行，避免两个独立 section
同时基于旧 document 写回而互相覆盖。不同文件仍可并行。

### 6. 单个进程共享一个 Store 实例

server 组合根创建一个 `Arc<FileSystemCellStore>`，再将其传给各领域 Wrapper。
`FileSystemCellStore::shared()` 可供需要进程共享默认实例的路径使用，但业务
代码仍应依赖 Wrapper，而不是到处调用全局 Store。

## Hard invariants

- 业务 manager 不得直接持有裸 `(file_path, json_path)` 规则。
- Cell Store 不得包含业务 schema、领域 key 或同步逻辑。
- 同一文件的 read-modify-write 必须在同一个 file lock 内完成。
- 写盘成功前不得更新内存 cache。
- 配置写入必须使用 temp、文件 `sync_all` 和 rename；Unix 实现还必须同步
  父目录。未实现父目录 fsync 的平台不得宣称具有同等 crash-durability。
- 外部文件修改必须能通过 mtime 变化在后续 read 中生效。
- `delete` 整文件与删除 section 必须保持幂等的 not-found 语义。
- Store failure 必须返回给 caller，不得记录日志后假装成功。
- Sync publication 只能在本地写成功后由领域 owner 发出。
- portable config 与 device-local runtime state 不得因共用 Store 而混域。

## Authority, persistence, and failure boundaries

磁盘文件是配置 authority；cache 是可丢弃的性能层。
Cells Wrapper 是 layout authority；业务 manager 是校验和业务语义 authority。
Cell Store 是原子持久化 authority，但不决定某个值是否合法。

Store 只保证单进程内、同一文件上的串行更新。跨进程同时写同一文件不在
该锁的保护范围内，因此 resident server 应保持唯一持久化 writer。

序列化、IO、rename 或已实现平台上的目录同步失败时，操作失败且 cache 不前移。
Sync、HTTP response 和 runtime reload 只能发生在成功写入之后。

## Consequences

- manager 不再维护各自的 write-through/cache 协议。
- 一文件多 section 的用户可读布局得以保留。
- 外部编辑无需常驻 watcher，也能在下一次读取时生效。
- 同文件操作串行会限制极端并发，但配置写入不是高吞吐数据路径。
- Store 保持泛型和 layout-agnostic，新增领域只需新增 Wrapper。

## Rejected alternatives

- 内存 authoritative、磁盘只作备份：外部编辑和写盘失败会造成双重真相。
- 通用 file watcher：回声、事件合并和跨平台行为增加不必要复杂度。
- caller 直接传 layout：会让路径知识散落并削弱类型约束。
- 全局字符串 key registry：会形成新的配置 god object。
- 拆分所有 `agent.json` section 为独立文件：破坏用户可管理的单文件形态。
- 让 Store 执行业务校验或发布 Sync：会反向耦合所有领域。
- 先更新 cache 再异步写盘：会重现成功响应后重启丢失的问题。

## Implementation and verification

当前实现路径：

- Store trait：`libs/cell-store/src/store.rs`
- filesystem 实现：`libs/cell-store/src/fs_store.rs`
- JSON path 操作：`libs/cell-store/src/json_path.rs`
- Provider wrapper：`crates/rua-provider/src/cells.rs`
- Cron wrapper：`crates/rua-cron/src/cells.rs`
- Agent wrapper：`crates/rua-agent/src/cells.rs`
- shared/Agent MCP wrapper：`crates/rua-mcp/src/shared_cells.rs`、
  `crates/rua-mcp/src/agent_mcp_cells.rs`
- Tool wrapper：`crates/rua-tool/src/agent_tools_cells.rs`
- Skill wrapper：`crates/rua-skills/src/cells.rs`
- server 共享实例：`crates/rua-server/src/state.rs`

### Current implementation gaps

- `ProviderCells` 当前虽然接收共享 `FileSystemCellStore`，但其读写路径通过
  `secret_field_io` 和独立的 per-path lock 直接访问 Provider 文件；共享
  store 字段未被使用。因此 Provider 配置不共享 Cell Store 的 mtime cache、
  file-lock registry 和 JSON-path update 协议。它是待迁移的旁路，不是本
  ADR 已完成覆盖的示例。
- Cell Store 的 path lock 只在一个 `FileSystemCellStore` 实例内部协调。
  若同一进程意外创建多个 store，或多个进程同时写同一文件，仍可能出现
  last-writer-wins。生产 composition 必须复用 `AppState` 的共享实例；跨进程
  文件锁不在当前实现中。

验证覆盖：

- `libs/cell-store/tests/basics.rs`
- `libs/cell-store/tests/atomic_update.rs`
- `libs/cell-store/tests/mtime_cache.rs`
- `libs/cell-store/tests/disk_first_writes.rs`
- 各领域的 `cells_layout`、persistence 和 endpoint contract tests

验证必须包含写失败不污染 cache、同文件并发 update 不丢 section、
外部 mtime 变化触发重读、删除幂等，以及 Unix 上的父目录持久化。

当前 `libs/cell-store/src/fs_store.rs::sync_parent_dir` 仅在 Unix 打开并
`sync_all` 父目录；非 Unix 分支是 no-op。因此 portable contract 是
temp-file + file sync + rename，父目录 crash-durability 当前只在 Unix
实现和验证。

## Related decisions

- ADR 0001: Workspace 分层依赖方向与进程组合
- ADR 0005: Agent Profile、Running Config 与 ResolvedAgent
- ADR 0013: MCP Per-Agent Manager 隔离
- ADR 0015: Skill Bundle 安装来源与扫描信任
