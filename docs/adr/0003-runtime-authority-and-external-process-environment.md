# ADR 0003: Runtime Authority 与外部进程执行环境

**Status:** Accepted

## Context

Rua 的外部进程表面包括 Shell、Python、MCP stdio、ACP runner、
Skill Runtime Probe、Plugin subprocess 和安装命令。调用方知道自己要执行
哪类工作，但不应知道本机 Node/Python 路径、Windows shell、Android proot
位置、cache 目录或环境包装细节。

仅有 Runtime Layout 还不够。Layout 描述路径和 adapter 能力，
但外部进程还需要统一回答：

- 当前 Device 上哪些工具链可用；
- 一个进程形态应进入 native 还是 Linux Compatibility Runtime；
- 应使用哪个 executable；
- cwd、env、cache 和资源限制如何进入启动参数；
- 缺失能力如何诊断且不产生隐式 fallback。

如果 ACP、MCP、Tool、Plugin 和 Skill 各自做 discovery，它们会选择不同的
Node、Python 或 shell，并把解析出的设备路径错误写入 portable config。

## Decision

### 1. Runtime Authority 是 Agent-visible managed subprocess 的设备本地 launch-planning authority

`libs/rua-core/src/runtime_authority.rs` 定义 `RuntimeAuthority`。
它组合 `RuntimeLayout`、`RuntimeInventory`、workspace directory 和
runtime cache policy，并返回 `RuntimeLaunchPlan`。

调用方提交：

- `ProcessShape`：`Shell`、`Interactive`、`Stdio`、`Direct`、
  `Install` 或 `SkillProbe`；
- `ToolchainIntent`：Shell、Python、uv、Node、npm、npx、
  Node runner 或 generic command；
- `RuntimeLaunchRequest`：args、env、env_remove、cwd 和可选资源限制。

调用方不得提交 Android host/guest placement 作为产品级选择。

### 2. Runtime Inventory 由当前 Device 探测

`RuntimeInventory` 保存 resident runtime、默认 external runtime、
Host Profile、OS family、shell、Python/uv/Node/npm/npx/git/rg/ffmpeg
能力、Linux Compatibility 可用性、cache policy 和 diagnostics。

inventory 是启动或刷新时的设备本地快照。它用于 planning 和诊断，
不是业务配置，不同步，也不由远端 Device 覆盖。

desktop/headless 在当前 PATH 与平台规则中解析工具链。
Windows 统一处理 `.cmd`/`.bat` shim 和 shell preference。
Android 的外部工具链从 Linux Compatibility Runtime 的约定路径解析。

### 3. Android Agent-visible managed subprocess 统一进入 Linux Compatibility Runtime

Android resident server 由 APK 直接启动，运行在 Native Android Runtime。
所有 Agent-visible managed external subprocess 统一进入 Linux Compatibility Runtime：

- Shell 与 Python；
- MCP stdio；
- ACP install/check/run；
- Skill Probe 与 Skill script；
- Plugin subprocess。

这不是可由 Agent、API、Skill metadata 或单个调用方覆盖的选项。
用户终端也通过 Android PTY 启动 Linux Compatibility Runtime 中的 shell，
使用相同的 Runtime Layout 与工具链事实；其 Session、Viewer 和 input lease
仍由 ADR 0034 定义，与 Agent 执行生命周期独立。

若 Linux Compatibility Runtime 不可用，launch plan 带诊断并最终失败。
安装器报告环境就绪后必须刷新 inventory，再允许相应执行。缺失环境或
工具链时不得直接执行原始 command，也不得跨 host/guest 重放。

### 4. Launch plan 是 caller 与执行器之间的完整边界

`RuntimeLaunchPlan` 固化：

- process shape 与 toolchain intent；
- selected `RuntimeKind`；
- executable、argv、env 和 cwd；
- host resource limits；
- runtime cache policy；
- Agent Path Contract；
- planning diagnostics。

可执行材料保持私有；执行器只能通过 `process()` / `into_process()` 取得，
fatal diagnostics 统一返回错误，不能取得原始 command 继续 spawn。
执行器消费该 plan，不再重新做 PATH 搜索或 runtime 选择。
同一调用的诊断工具和真正执行路径必须来自同一 authority。

### 5. Cache 与资源限制属于设备运行策略

默认 runtime cache 根由进程组合层提供，npm 与临时目录从
`RuntimeCachePolicy` 派生。cache path 不写回 Agent、MCP、ACP、
Plugin 或 Skill portable config。

Linux Compatibility Runtime 的 uv 使用复制模式（`UV_LINK_MODE=copy`），避免
PRoot 硬链接转换生成 guest 不可访问的原生缓存路径。npm 全局 prefix 为持久化
`/root/.local`，其 `bin` 在 guest PATH 中；工具链版本目录不承载用户全局包。
工具链版本由 guest 实际执行探测后记录，Inventory 仅通过命令当前指向的安装
目录读取该记录，不将已被用户替换的命令标为原发行物版本。

native 进程可接收 host resource limits。Linux Compatibility Runtime
通过 guest 内的 `prlimit` 包装；缺失 `prlimit` 必须进入 diagnostics，
不得宣称限制已生效。

Android 的 Python 默认进程额度为 512。`RLIMIT_NPROC` 按实际 UID 的所有
线程计数，APK、WebView、resident 和 guest 共享此额度；它不是每个工具独立
的进程配额，也不是 PRoot 沙箱隔离。不能把桌面默认 64 直接用于已运行这些
组件的 APK，否则合法的嵌套 Python 也可能无法 fork。内存地址空间、文件大小
限制仍施加到 guest payload；Stop/超时清理其进程组。
语义依据：[Linux getrlimit(2)](https://man7.org/linux/man-pages/man2/getrlimit.2.html)。

### 6. 配置只保存意图

ACP/MCP/Plugin/Skill 配置可以保存 command、args、声明的 env、
enabled 状态和业务选项，但不得保存 Runtime Authority 已解析出的
Python、Node、npm、npx、shell、proot 或 cache 绝对路径。

允许持久化的例外只能是显式 Device-local preference，例如 Windows shell
preference；它仍不是可同步的 Agent/Workspace 配置。

## Hard invariants

- 本 ADR 列出的 Agent-visible managed subprocess surface 必须由
  Runtime Authority 或其薄 adapter 生成 plan。
- Android Agent subprocess 的默认 runtime 永远是 Linux Compatibility。
- resident runtime 与 external runtime 是两个独立事实。
- caller 只能表达 Process Shape 和 Toolchain Intent，不能选择 Android runtime。
- 解析后的 executable path 不得进入 portable config 或 Sync。
- diagnostics 是 authority 的读模型，不得自行重新探测并覆盖结果。
- 同一启动不得在失败后跨 runtime 自动重放。
- secret env 不得出现在 argv、日志或可同步配置中。
- cwd、cache 和资源限制必须包含在同一个 launch plan 中。
- 缺失工具链应降低对应能力，不应阻止无关的核心进程启动。

## Authority, persistence, and failure boundaries

Runtime Authority 属于当前进程和 Device。其 inventory 与 launch plan
是运行时状态，可重建，不是持久化 authority。业务配置拥有“要运行什么”，
Runtime Authority 拥有“在本机如何运行”。

工具链缺失、Linux Compatibility 缺失、cwd 不可映射或资源限制不可实施时，
planning diagnostics 必须可见，执行路径返回失败。失败只影响依赖该能力的
Skill、MCP、ACP、Plugin 或 Tool，不应让不相关的 Chat 浏览、配置读取或
server health 失效。

Linux stdio/安装入口未指定 cwd 时使用当前 Agent 工作区；相对 cwd 也由同一
工作区解析。不存在的 guest 工作目录在产生可执行材料前拒绝。MCP 配置无法
应用时，已有状态接口保留具体诊断；环境补齐后显式应用配置重新读取 Inventory。

ACP 的 install/check/run 读取同一 Runtime Authority。APK 的准备流程拥有
Debian 与 Node 安装；ACP 安装器只安装 runner npm 包，不自行下载、替换 Node，
不依赖 Termux prefix。runner 使用持久化 guest home 的 npm prefix 与公共 bin
入口，配置仍只保存 command/args/env。check 和会话启动重新读取 Inventory；
Node 或工作目录缺失直接返回可恢复诊断。协议进程应用 plan 的 cwd 与隔离环境，
安装超时或取消释放整个进程组，权限 relay、session/cancel 和会话退出沿用 ACP 契约。

Plugin subprocess 的 guest adapter 清空继承环境，再应用 launch plan；显式
`env_remove` 同样作用于 native 子进程的继承变量。进程注册表关闭先取消正在
执行的协议调用，释放读锁后逐个 shutdown；关闭开始后拒绝新注册。resident
退出必须关闭 active plugin 注册表，不能以分发 shutdown hook 代替进程清理。

MCP stdio 调用的超时或连接错误不自动重放，避免重复执行结果未知的副作用。
已完整发送的工具请求在取消、超时或等待 future 被丢弃时发送有界的
`notifications/cancelled`；不取消 initialize，不重启共享服务。取消请求内的工作
依赖服务遵守 [MCP 取消协议](https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/cancellation)；
停用服务或关闭 resident 则由本地 owner 终止进程子树。

进程执行后的 stdin/stdout 协议、超时、取消和 kill-tree 由具体调用方拥有；
Runtime Authority 只拥有计划，不成为子进程生命周期 manager。

## Consequences

- 外部进程在所有 surface 上使用一致的工具链和环境。
- Android 的 Shell、Python、MCP、ACP 和终端共享 Linux 工具链事实。
- Desktop 安装缺失工具链时可通过统一诊断和 Toolchain Setup 修复。
- 配置可在 Device 间移动，而不会携带无效的本机 executable path。
- 每个新 subprocess surface 必须先定义 Process Shape 与 Toolchain Intent。

## Rejected alternatives

- caller 直接执行配置中的 command：会绕过平台、cache、cwd 和资源策略。
- 为每个 surface 增加 runtime toggle：会形成无法验证的组合矩阵。
- 根据命令文本选择 host/guest：嵌套脚本与 shebang 使判断不可靠。
- 将 inventory 当成同步配置：会把一台机器的路径错误传播到另一台机器。
- 缺失能力时静默使用 PATH fallback：会使诊断与真正执行不一致。
- 由 doctor 独立探测：会形成第二份 runtime truth。

## Implementation and verification

当前实现路径：

- authority、inventory 与 launch plan：
  `libs/rua-core/src/runtime_authority.rs`
- path/layout adapter：`libs/rua-core/src/runtime_layout.rs`
- Shell executor：`crates/rua-tool/src/executor/runtime.rs`
- Python executor：`crates/rua-tool/src/builtin/execute_python_code.rs`
- runtime diagnostics：`crates/rua-tool/src/builtin/runtime_environment.rs`
- ACP runner adapter：`crates/rua-acp/src/client.rs`
- Plugin adapter：`crates/rua-plugin-runtime/src/client.rs`
- Skill Probe：`crates/rua-skills/src/android_runtime.rs`

验证覆盖：

- `libs/rua-core/tests/unit_runtime/runtime_authority.rs`
- `crates/rua-tool/tests/all/runtime_executor.rs`
- `crates/rua-tool/tests/all/execute_python_code.rs`
- `crates/rua-acp/tests/client/command_resolution.rs`
- `crates/rua-plugin-runtime/tests/all/runtime_authority.rs`

测试应证明 desktop native 解析、Windows shim 包装、Android
Linux Compatibility 选择、cache env、secret 传递、缺失 runtime 诊断和
资源限制包装来自同一 plan。

## Related decisions

- ADR 0001: Workspace 分层依赖方向与进程组合
- ADR 0002: Runtime Layout 与 Agent Path Contract
- ADR 0011: Effective Capability Snapshot 与 Tool Exposure
- ADR 0014: Plugin process and protocol isolation
- ADR 0015: Skill Bundle 安装来源与扫描信任
- ADR 0033: ACP Chat Mode, client projection, and permission relay
- ADR 0034: Interactive Terminal Session and Viewer Runtime
