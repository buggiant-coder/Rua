# ADR 0002: Runtime Layout 与 Agent Path Contract

**Status:** Accepted

## Context

Rua 在 desktop、headless 和 Android mobile-apk Host Profile 上运行。
不同宿主具有不同的 shell、HOME、临时目录、共享存储和路径表示。
Android 由 APK 直接托管 Native Android Runtime，并通过 PRoot 提供
Linux Compatibility Runtime。原生驻留进程与 Linux 工具环境有独立的
启动、分发和数据边界。

Shell、Python、MCP stdio、ACP runner、Skill Probe 和 Plugin subprocess
都需要启动外部进程。若每个调用方自行拼接 APK 原生程序路径、rootfs、bind、
PATH、cwd 和 Windows shell，路径语义会漂移，Agent 也会得到互相矛盾的
执行环境说明。

因此需要区分两个相关但不同的契约：

- Runtime Layout 描述宿主和运行时适配器的路径事实；
- Agent Path Contract 将这些事实投影成 Agent 可依赖的稳定路径规则。

Runtime Layout 不选择具体工具链，也不判断某个进程应进入哪个 runtime；
这些职责属于 ADR 0003 的 Runtime Authority。

## Decision

### 1. APK 直接托管 Native Android Runtime

Android APK 负责界面、前台服务、进程启动与停止、恢复和原生权限能力。
统一的 Android ELF `rua` 链接 resident server 实现，以独立子进程执行
`rua server start --profile mobile-apk`；Agent 编排、HTTP 服务和终端
Session Authority 均由该进程拥有。原生手机能力通过 ADR 0057 的私有认证
通道提供给 resident，凭据不得传入工具环境。

```text
Rua Android APK
├─ Native Android Runtime：rua server start --profile mobile-apk
│  ├─ Agent、HTTP、Runtime Authority
│  └─ Terminal Session Authority / Android PTY
└─ PRoot + loader → Linux Compatibility Runtime
   └─ bash、Python、Node、Skills、MCP、ACP、Plugin subprocess
```

APK 携带 Rua ELF、WebUI、终端 bridge、PRoot、loader、安装进程组启动器和必要原生依赖。
Rua、PRoot 与 loader 从 Android 安装器管理的 `nativeLibraryDir` 启动，
每次 APK 更新或进程启动重新解析实际位置。即使为了 APK 打包使用 `.so`
文件名，Rua 仍以 ELF 子进程执行。可写的数据目录用于 Linux rootfs、
工具链和用户数据，不作为下载后直接执行 Android 原生程序的分发入口。

APK 的前台服务统一管理驻留进程与 ADR 0043 定义的通知、电源和网络策略。
Linux 工具环境未就绪时，resident 可独立启动；首次启动界面保持全屏准备页，
全部必需工具验证就绪后才创建并显示正式 WebUI。

### 2. Linux 环境独立获取与安装

Linux Compatibility Runtime 使用固定版本的 Debian rootfs，通过 PRoot
提供 guest 文件系统和工具执行环境。PRoot 提供用户态 Linux 兼容能力，
不提供虚拟机内核或独立的安全隔离边界；进程仍受 APK UID 和 Android 权限约束。

APK 不内嵌完整 Linux rootfs 和 Python/Node 工具链。Mobile Runtime Manifest
固定发行版、版本、架构及所需工具链的兼容组合，并给出可信来源与校验信息。
发行版与工具链可使用官方 rootfs、发行版包仓库和官方 Linux 工具发行物，
无需为 APK 构建整套定制 Linux 用户空间。具体版本由 manifest 管理，
不由每个调用方选择。

首次打开 APK 时，宿主在启动 resident 的同时自动准备尚未安装的 Debian、
Python 和 Node 环境，不要求选择发行版、镜像或点击安装。界面呈现阶段、
进度和下载/空间需求，并提供取消、重试与离线导入。准备页占满应用内容区，
安装完成前不显示聊天、导航或其它正式功能。准备期间可通过权限设置入口跳转
系统设置，安装任务由 service 持续执行；正式界面通过更多菜单进入环境管理，
不保留常驻状态栏。应用离线时显示等待网络，
网络恢复后有界重试；用户明确取消后持久化暂停意图，直到用户主动继续。
已有可用环境只做检查，不因再次打开或普通 APK 更新自动重装、换发行版。
产品尚未发布，安装器仅管理本架构创建的 Debian 环境；不提供历史 APK、
Termux prefix 或 Ubuntu 数据迁移、探测与采用分支。

安装器对维护的可信镜像列表并发探测实际 HTTPS 元数据地址，以单调时钟测量
请求耗时，并验证所需版本、架构和内容是否存在。在有界探测窗口内选取合格
来源中耗时最低者，失败后依次尝试其余候选。Debian 和 Node 分别选源；
不以 ICMP ping 或仅能连接到主机作为下载可用性依据。支持缓存和离线导入。
镜像切换只改变
传输来源，不能改变预期内容或绕过校验。校验依据来自可信 manifest 或发行方
签名链，不能仅信任下载镜像同时提供的哈希。

下载内容先校验，再安全解包至 staging，拒绝路径穿越及通过归档链接越界
写入；完成依赖探测后原子提交安装状态。安装中断或失败不激活半成品，已有
可用环境继续保留。升级/替换环境需协调使用它的进程，并保留用户安装的依赖；
不得覆盖或清空独立存储的 Rua workspace、配置和用户文件。

APK Native Payload 随 APK 更新；Linux rootfs 和工具链有独立的设备本地
安装状态与版本。发布流水线只需构建 Android 原生载荷与应用资源，不依赖
Docker 重建整套移动用户空间。镜像安装或更新后刷新 Runtime Inventory，
仅在对应能力探测通过后开放相关工具和终端。

### 3. Runtime Layout 统一拥有路径与适配器事实

`libs/rua-core/src/runtime_layout.rs` 定义 `HostProfile`、
`RuntimeKind`、`NativeShellSpec`、`LinuxCompatLayout`、
`ProcessLaunchPlan` 和 `AgentPathContract`。

Host Profile 的标准值是 `mobile-apk`、`desktop` 和 `headless`。
它表达产品宿主形态，不等于操作系统名称，也不等于外部进程 runtime。

Runtime Layout 负责：

- 当前 Host Profile 与 OS family；
- Android `nativeLibraryDir`、应用数据目录和 Linux Compatibility rootfs；
- proot executable、guest HOME、bind roots 与基础环境；
- host cwd 到 guest cwd 的映射；
- Unix、Git Bash、PowerShell、cmd 和 WSL shell 选择；
- mobile process temp directory；
- clean-env 与 preserve-process-env 两种环境组装方式。

### 4. Android canonical workspace 保持 host 路径

Android 上 Rua workspace 的 canonical path 是 APK 私有数据目录下
独立于 rootfs 的 host 绝对路径。Linux Compatibility Runtime 通过 bind 在 guest 内看到同一
路径，不把 workspace 搬到 `/root`。

用户可见手机文件使用 `/sdcard/...`。`/storage/emulated/0/...` 在进入
Linux Compatibility Runtime 时映射为 `/sdcard/...`。`/storage`、
应用 workspace/home、`/proc`、`/sys`、`/dev` 和必要设备文件由受控 bind 暴露。
私有认证凭据不属于 guest bind 的数据范围。

显式 cwd 不可表示时不得悄悄回退到 `/root`。启动计划应保留原 cwd，
让启动失败产生可诊断错误。

### 5. 环境合并规则必须显式

`LinuxCompatEnvStrategy::Clean` 从固定 Linux 基础环境开始，将调用方 env
和 PATH additions 合并后通过 `/usr/bin/env -i` 传入。

`LinuxCompatEnvStrategy::PreserveProcessEnv` 用于必须通过进程环境传递
secret 的 runner。基础非敏感环境可出现在 argv，调用方 secret 保留在
`ProcessLaunchPlan.env`，不得序列化进命令参数或诊断文本。

`env_remove` 在合并后生效。调用方不能通过自己的合并顺序绕过删除规则。

### 6. Agent Path Contract 是模型看到的路径真相

`RuntimeLayout::agent_path_contract(workspace_dir)` 生成可序列化的路径契约，
包含 workspace、home、temp、用户文件根、别名、规则和 warning。

Android 契约必须明确：

- Agent workspace 使用 `workspace_dir`；
- 用户可见手机文件使用 `/sdcard/...`；
- 工具参数避免使用 `~`；
- `/root` 只表示 Linux Compatibility Runtime 的 guest HOME；
- `/sdcard` 是否可访问仍受 Android storage permission 控制。

desktop/headless 契约要求跨工具传递文件时使用绝对路径，并在 Windows
提示 shell adapter 可能具有不同路径规则。

### 7. 执行设备决定有效契约

执行一个 Turn 的 Authority Device 使用本机 Runtime Layout 渲染
Execution Environment prompt。控制端设备的路径、HOME 或工具安装状态
不得进入远端执行 Turn 的契约。

`crates/rua-agent/src/prompt.rs` 负责将契约和 Device/Host Profile 信息
加入有效 prompt。`runtime_info` 使用同一事实生成诊断读模型。

## Hard invariants

- APK 原生载荷、Linux 工具环境和用户数据具有独立的分发与存储边界。
- Rua 驻留进程由 APK 直接启动，Linux 工具环境就绪不是其启动前置条件。
- Runtime Layout 是路径事实的唯一通用来源。
- Agent 不得从 `~`、进程名或 shell 类型推断稳定路径。
- Android workspace 不得迁移或别名化为 `/root`。
- `/root` 不得描述为手机文件或 Rua workspace 的 canonical root。
- `/sdcard` 访问不得被描述为绕过 Android permission。
- caller 不得自行复制 proot bind、guest HOME 或 Windows shell 选择逻辑。
- secret 不得因 clean-env 包装而进入 argv。
- cwd 映射失败不得静默改为另一个工作目录。
- 从另一 Device 获得的绝对路径不得宣称在当前 Device 可直接访问。
- Runtime Layout 不持久化或同步设备解析出的 executable path。

## Authority, persistence, and failure boundaries

Runtime Layout 是纯设备本地事实和值对象。它从当前进程环境、Host Profile
和约定路径构造，不是 Agent config、Workspace config 或同步数据。

Agent Path Contract 是 Runtime Layout 的只读投影。它可进入 prompt 和诊断，
但不能反向修改 Runtime Layout，也不能成为 runtime 选择开关。

Linux Compatibility Runtime 缺失、proot 不可执行、bind/cwd 不可用时，
调用方应返回明确失败。Runtime Layout 不执行“先在一个 runtime 失败，再到
另一个 runtime 重放”的策略，因为原调用可能已经产生副作用。

## Consequences

- 所有 subprocess surface 有一个可复用的 Android 与 Windows 路径语义；
  调用方仍必须显式接入该 seam。
- Agent prompt、运行诊断和 Skill Probe 可共享同一组路径事实。
- Android workspace 可同时被 resident runtime 与 proot 子进程访问。
- Windows shell adapter 可以演进，而不改变 Agent workspace identity。
- Runtime Layout 需要保持轻量和无业务依赖，以便被多 crate 复用。

## Rejected alternatives

- 每个 caller 各自拼接平台路径：已导致 MCP、ACP、Shell 和 Skill 行为漂移。
- 把 `/root` 作为 Android workspace：会把产品状态放入兼容 runtime。
- 只向 Agent 暴露 `~`：其含义随 adapter 改变，无法跨工具稳定使用。
- 用命令文本猜测路径或 runtime：无法识别嵌套脚本和 shebang。
- 将 Runtime Layout 存入可同步配置：会把一台 Device 的本地路径传播到
  其它 Device。
- 对不可映射 cwd 自动回退：会在错误目录执行并隐藏真实配置问题。

## Implementation and verification

实现归属：

- 核心类型与路径规则：`libs/rua-core/src/runtime_layout.rs`
- Runtime Authority 消费端：`libs/rua-core/src/runtime_authority.rs`
- Execution Environment prompt：`crates/rua-agent/src/prompt.rs`
- 运行环境诊断工具：`crates/rua-tool/src/builtin/runtime_environment.rs`
- Shell 进程 adapter：`crates/rua-tool/src/executor/runtime.rs`
- Python 进程 adapter：`crates/rua-tool/src/builtin/execute_python_code.rs`
- Android Skill Probe：`crates/rua-skills/src/android_runtime.rs`

相关测试入口：

- `libs/rua-core/tests/unit_runtime/runtime_layout.rs`
- `libs/rua-core/tests/unit_runtime/runtime_authority.rs`
- `crates/rua-agent/tests/unit_prompt/prompt_builder.rs`
- `crates/rua-tool/tests/all/runtime_executor.rs`
- `crates/rua-tool/tests/all/execute_python_code.rs`
- `crates/rua-plugin-runtime/tests/all/runtime_authority.rs`

测试必须覆盖 `/storage/emulated/0 -> /sdcard`、workspace bind、不可映射 cwd、
secret 不进入 argv、Host Profile prompt 和 Windows shell 选择。

Android 生产 APK 的设备测试位于 `mobile/mobile-shell/app/src/androidTest/`。验收覆盖原生认证启动、
自动镜像安装、真实 Agent/Python/技能、协议子进程、终端、APK 复用与环境恢复。
覆盖升级还须验证身份、历史及用户依赖保留。多 Android 版本和真机结果须分别
记录；架构 Accepted 状态不能替代实际设备验收。

### Current implementation gaps

- `AgentPathContract` 目前主要通过 `RuntimeLayout` 与 prompt builder 的测试被
  间接覆盖，没有独立的序列化/字段语义契约测试。新增字段或改变 alias 时
  可能出现 runtime 事实正确、Agent projection 漂移而测试未捕获的情况。
- 部分低层构造器仍可在没有上层注入时自行调用
  `rua_config::dirs::Paths::new(None)` 或 `std::env::temp_dir()` 选择 cache
  路径，例如 ACP client/service 与 Plugin installer 的默认构造路径。生产
  composition 会注入 runtime dirs，但低层默认值说明“所有 subprocess
  surface 已统一”尚不能作为实现事实；这些默认路径必须继续收敛到显式
  `RuntimeLayout`/`RuntimeAuthority` 输入。

## Related decisions

- ADR 0001: Workspace 分层依赖方向与进程组合
- ADR 0003: Runtime Authority 与外部进程执行环境
- ADR 0011: Effective Capability Snapshot 与 Tool Exposure
- ADR 0015: Skill Bundle 安装来源与扫描信任
- ADR 0033: ACP Chat Mode, client projection, and permission relay
- ADR 0034: Interactive Terminal Session and Viewer Runtime
