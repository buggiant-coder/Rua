# ADR 0015: Skill Bundle 安装来源与扫描信任

**Status:** Accepted

## Context

Skill 不只是 `SKILL.md` 文本，还可以包含 `references/`、`scripts/` 和其它
资源文件。它可由用户本地创建、从 Skill Pool 复制、由 Plugin 提供，或从
多个 Hub/GitHub/URL 来源下载。

来源信息、内容安全和运行权限是三个不同问题：

- provenance 回答“从哪里、何时、以什么版本安装”；
- scanner 回答“当前 bundle 是否发现阻断级风险”；
- Tool Guard/sandbox 回答“运行时脚本实际允许做什么”。

若把来源平台当作信任，恶意或被接管的远端内容会绕过扫描。
若只扫描 `SKILL.md`，bundle 脚本和引用文件仍可隐藏危险行为。
若安装直接写最终目录，扫描失败会留下部分可见内容。

Project Skills 又是从 Project Resource Root 原位发现的源码，不经过 Hub
安装流程，不能错误地复制到 Agent workspace 或自动赋予信任。

## Decision

### 1. Skill Bundle 是完整目录单元

标准 bundle 至少包含 `SKILL.md`，并可包含 `references`、`scripts`
和其它安全相对路径文件。Hub payload 先由
`rua-skills-hub::normalize_bundle` 归一化。

所有 bundle path 必须经过 safe path parsing；绝对路径、`..` escape
和保留目标外写入被拒绝。目标目录名经过规范化。

### 2. 支持的安装来源只表示 provenance

`BundleSource` 当前支持：

- Skills.sh；
- GitHub；
- LobeHub；
- ModelScope；
- SkillsMP；
- SkillHub；
- Aliyun；
- ClawHub；
- direct URL。

`provider_key()` 产生开放字符串，如 `github`、`modelscope`、`url`。
增加来源不要求修改持久化 enum。

本地创建和 Plugin 来源使用 `SkillSource` 表达内容归属，
与 Hub install channel 分离。

### 3. Hub 层不直接写 workspace

`rua-skills-hub` 负责解析来源、下载、重试、取消、大小限制和 bundle
归一化。它通过 `BundleInstaller` 或 `BundlePoolInstaller` 把完整 bundle
交给拥有 filesystem layout 的 `rua-skills`/server adapter。

Hub crate 不知道 Agent workspace 路径、manifest Cell 或 enablement layout。
安装器必须先创建完整内容，再决定是否 enable。

### 4. `_meta.json` 保存不可从内容重建的 provenance

Hub-installed Skill 在目录内保存 `_meta.json`，字段包括：

- `installed_from`;
- `source_url`;
- `installed_at` 与可选 `updated_at`;
- `installed_version`;
- `content_hash`;
- 可选 `market_meta`.

`content_hash` 当前按规范化 `SKILL.md` 内容计算，用于识别本地修改。
workspace/pool manifest 可由磁盘 reconcile 重建，不能作为唯一 provenance
存储。手工创建 Skill 可以没有 `_meta.json`。

### 5. 安装、修改、启用和导入在 staging 上扫描

`SkillService` 将目标内容写入临时 staging directory，写完
`SKILL.md`、references、scripts、extra files 和 install metadata 后，
调用可注入的 `ScannerHandle`。

scanner 允许后才复制到最终目录并更新 manifest。
scanner block 时返回错误，最终 workspace/pool 不得出现部分安装。

已有 Skill 内容修改、rename、ZIP import、pool upload/download 和 enable
同样必须经过适用的 scanner path。

### 6. Scanner 是内容风险判定，不是执行授权

`rua-skills-scanner` 有有界文件发现、文件大小/数量限制、symlink skip、
canonical containment、analyzer panic isolation、finding 去重和 policy。
高于 block threshold 的 findings 阻止操作，并可记录 blocked history。

scan allow 不授予 shell、network、filesystem 或 secret 权限。
Skill script 执行仍受 Runtime Authority、Tool Guard、Approval 和 sandbox。
whitelist 只豁免明确内容 hash/policy，不是来源域名通配信任。

### 7. Project Skills 原位、有界发现

Project Skills 只从 `<project-root>/.agents/skills/**/SKILL.md` 发现。
默认最大深度 6、最多访问 2,000 个目录；隐藏目录被忽略；
canonical visited set 防止 symlink alias/cycle 重复。

单个 malformed/unreadable Skill 产生 diagnostic 并被跳过，不阻止其它 Skill。
Project file watcher 只使下一次 capability snapshot 失效。

Project Skills 不复制到 Agent workspace、不进入 Hub provenance，也不单独获得
“Project trusted”权限；其脚本继续走普通 Agent security。

## Hard invariants

- 来源标识不得被当作安全信任。
- bundle 内所有写入路径必须保持在 staging/目标目录内。
- 扫描必须发生在最终目录可见或 enable 之前。
- scan block 不得留下部分安装或 manifest entry。
- `_meta.json` 不得被 reconcile 丢弃。
- 手工 Skill 缺少 `_meta.json` 是合法状态。
- scanner allow 不得绕过 Tool Guard、Approval 或 Runtime Authority。
- whitelist 必须绑定内容/明确规则，不能只按 source host。
- Project Skills 不得复制或同步为 Agent-owned bundle。
- malformed Project Skill 不得阻断完整 catalog。

## Authority, persistence, and failure boundaries

Hub adapter 拥有网络来源解析与下载。
Bundle normalizer 拥有输入 shape 和相对路径合法性。
SkillService/PoolService 拥有最终目录和 manifest。
`_meta.json` 拥有安装 provenance。
Skill Scanner 拥有静态风险 finding。
Runtime security 拥有真正执行授权。

下载、取消、normalize、写 staging 或 scan 任一步失败，都不提交最终安装。
Project discovery 失败按单目录/单 Skill 降级并产生 diagnostics。
Android runtime compatibility scan 只产生 probe/requirement 信息，
不改变 Skill 在 Linux Compatibility Runtime 中执行的策略。

## Consequences

- 用户可追踪 Skill 来源和本地修改。
- 多 Hub 接入不污染 workspace storage boundary。
- 危险 bundle 在提交前被阻止。
- 安全扫描增加安装和 enable 延迟，但不影响已加载其它 Skill。
- Project Skills 保持与仓库版本控制一起演进。

## Rejected alternatives

- 只在 manifest 保存 provenance：reconcile 无法从内容重建并会丢失。
- 根据来源平台自动信任：供应链来源不是执行授权。
- 只扫描 `SKILL.md`：scripts/references 可包含实际风险。
- 先写最终目录再扫描：失败会留下半安装状态。
- 后台自动轮询所有 Hub 更新：来源语义和版本规则不一致且产生额外流量。
- 对 Project Skills 再建独立 trust state：Project 已显式绑定，运行权限仍由
  现有 security boundary 控制。

## Implementation and verification

当前实现路径：

- source routing：`crates/rua-skills-hub/src/types.rs`、
  `crates/rua-skills-hub/src/sources/`
- bundle normalization/installer trait：
  `crates/rua-skills-hub/src/bundle.rs`
- install orchestration：`crates/rua-skills-hub/src/lib.rs`
- provenance：`crates/rua-skills/src/install_meta.rs`
- staged Skill operations：`crates/rua-skills/src/service.rs`
- scanner adapter：`crates/rua-skills/src/scanner_integration.rs`
- scanner engine：`crates/rua-skills-scanner/src/scanner.rs`
- policy/whitelist/history：`crates/rua-skills-scanner/src/`
- Project discovery/watch：`crates/rua-skills/src/project.rs`
- Android runtime probe：`crates/rua-skills/src/android_runtime.rs`

验证覆盖：

- `crates/rua-skills-hub/tests/all/`
- `crates/rua-skills/tests/core/scanner_integration.rs`
- `crates/rua-skills/tests/runtime/pool_service.rs`
- `crates/rua-skills/tests/runtime/project.rs`
- `crates/rua-skills-scanner/tests/all/`
- `crates/rua-skills/tests/core/android_runtime.rs`

### Current implementation gaps

- `crates/rua-skills-scanner/src/whitelist.rs::is_skill_whitelisted`
  当前把空 `content_hash` 解释为按 `skill_name` 永久匹配；没有目录参数时，
  非空 hash entry 也直接返回 true。
  `crates/rua-server/src/endpoints/security.rs::add_whitelist` 只校验
  `skill_name`，允许提交空 hash。更新同名 Skill 因而可绕过内容扫描。
- 这是对“whitelist 必须绑定内容或明确规则”不变量的违反。修复必须拒绝空
  content hash，并且在无法计算/提供当前 bundle hash 时 fail closed；若产品
  确实需要 name-wide exemption，应另建明确命名、可审计的 policy 类型，不能
  复用空字符串。
- `crates/rua-skills/src/pool_service.rs::download_to_workspace` 在
  `overwrite=true` 时先删除现有 workspace Skill，再扫描 pool source；
  扫描失败会丢失旧版本。扫描后又通过 create + recursive copy 提交，
  copy/meta/manifest 任一步失败都可能留下部分目录。
- `PoolService::upload_from_workspace` 也直接复制到最终 pool 目录并随后更新
  manifest，没有 rename/swap transaction。修复必须在目标 filesystem 上
  构造完整 staging 副本，通过扫描后原子 rename/swap，并在 manifest 更新
  失败时保持可恢复的一致状态。

## Related decisions

- ADR 0002: Runtime Layout 与 Agent Path Contract
- ADR 0003: Runtime Authority 与外部进程执行环境
- ADR 0004: 配置持久化 Cell Store
- ADR 0011: Effective Capability Snapshot 与 Tool Exposure
- ADR 0012: Per-Agent Tool Guard 与进程级 Approval
- ADR 0019: Project, Project-Scoped Chat, and fixed resource binding
