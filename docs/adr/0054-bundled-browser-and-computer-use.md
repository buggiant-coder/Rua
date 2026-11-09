# ADR 0054: Bundled Browser Use and Computer Use

**Status:** Accepted

## Context

用户安装一个 Rua Desktop App 后，应当能够使用 Browser Use 和 Computer
Use，无需再安装两个独立工具、配置 MCP 或安装开发语言运行时。独立进程是
内部执行边界，可以与 Rua 一起分发，不要求独立的用户安装流程。

上游桌面输入修复的发布节奏可能快于 Rua 整包发布。开箱即用不应要求所有
兼容驱动修复都等待整包升级，但独立更新不能放弃 Rua 的兼容和安全验收。

选择 `vercel-labs/agent-browser` 提供 Browser Use，选择 `trycua/cua` 中的
Cua Driver 提供 Computer Use。两者已有 MCP 接口，可复用 Rua 的 Agent、
Tool、审批和结果处理链路。浏览器本体复用用户系统安装，首期只支持 Chromium
内核，不实现其他浏览器后端。

ADR-0049 定义桌面外壳与统一 `rua` resident Runtime。这里增加两项明确的
产品内置驱动，并补充其分发、权限和执行边界；普通 Desktop External Toolchain
仍遵守原有的设备本地依赖约定。

## Decision

### D1 - 两个驱动随 Desktop App 分发

macOS 和 Windows 的 Rua 安装包携带固定版本的 `agent-browser`、Cua Driver
及其必要运行文件。用户使用 Browser Use / Computer Use 不需要另装 Node.js、
Python、npm、pip 或独立 Cua App，也不需要经过通用 Plugin 安装器。

驱动作为独立程序运行。构建时可使用可验证的上游发布产物，或从固定源码版本
独立编译；不将上游整个仓库合并进 Rua Cargo workspace。Rua 只维护必要的
接入代码。Cua 的原生宿主 SDK 按 D4 接入 resident Runtime，执行 daemon 仍有
独立进程边界。

### D2 - 复用系统 Chromium 浏览器

安装包不携带浏览器引擎，也不在首次启动时自动下载浏览器。Browser Use 只支持
经过兼容性检测、能够通过 Chrome DevTools Protocol（CDP）控制的 Chromium
内核浏览器。Safari、Firefox 及其他引擎不属于本次范围。

通过 Runtime Authority 集中发现浏览器程序并检查可用性；用户可指定本机路径。
路径选择是设备本地运行偏好，不写入可同步的 Agent/MCP 配置。实际启动使用
已解析的程序路径，不要求该程序位于用户 shell 的 `PATH`。

默认用现有浏览器程序启动 Rua 专用会话，浏览器配置与登录状态保存在该设备
的 Rua 数据目录内。复用程序不自动接管用户正在使用的窗口或个人 profile。
找不到兼容程序或启动检查失败时，Browser Use 显示不可用原因，并提示用户
选择路径或安装兼容浏览器；Rua 其他能力继续可用。

### D3 - 工具通过现有 MCP 与 Agent 链路接入

`agent-browser mcp` 和 `cua-driver mcp` 是标准工具接入入口。产品装配内置
服务定义并解析 Rua 管理的驱动路径，不要求用户手写 MCP JSON；执行仍使用 ADR-0013 的
per-Agent `McpManagerImpl`，不建立第二套 Plugin 生命周期或全局 MCP manager。

内置分发不绕过现有能力与授权规则。工具进入 Effective Capability Snapshot，
真实工具名称和参数经过 Tool Guard；授权依据是 ADR-0012/0028 的有效 Chat /
Turn 快照。驱动宿主、Agent 决策、审批和任务状态均由 resident Runtime 拥有。

桌面浏览器与桌面控制能力统一由 `rua-mcp` 的 bundled Browser Use 与
Computer Use 提供。新建与既有 Chat 均通过当前 MCP 工具接入和有效能力快照
调用这些能力，不保存独立的内置浏览器或截图工具实现。

截图沿 MCP image content 转换为 `ContentPart::Image`，再进入现有模型请求
投影；不能仅把 base64 截图装进 JSON 文本后声称模型已经看到画面。需要视觉
观察的调用必须使用具备图像输入能力的模型配置。

Console 的「桌面工具」是 App 和 WebUI 共用的入口，位于「工具」菜单下方。
页面及 HTTP 状态、浏览器路径配置与检测不以查看端是否为 Desktop Shell 作为
访问条件；可用性取决于执行设备上的驱动和系统授权。权限申请、系统设置入口、
驱动重查和重启通过同一 HTTP API 由执行设备的 Runtime 处理，普通浏览器和
App 都可直接操作，不保留对应 Tauri JavaScript 命令或权限白名单。
应用更新、CLI 安装和旧版运行时迁移放在独立「关于」页面，位于菜单末尾。
CLI 安装直接由 Runtime 执行；整包 App 更新和旧 Shell 安装迁移通过私有认证
通道协调安装管理者，不参与自动化驱动或 CLI 的执行。独立 server 没有 App
更新组件时如实报告该组件未连接，不影响两项工具和 CLI 安装。

### D4 - 原生权限宿主与 MCP 连接各有唯一 owner

resident Runtime 使用固定版本的 Cua `EmbeddedCuaDriverHost` 启动和关闭私有
原生 daemon，并执行系统权限探测与申请。驱动使用包内版本，或按 D6 选择本机
已验证且与当前 Runtime 兼容的独立更新版本；路径解析由 Runtime Authority 统一负责。
开发环境可显式提供驱动路径，不依赖 Tauri 注入或用户 shell 的 PATH。
连接描述文件固定在当前 Runtime 的本机 cache 目录，不能从另一个 App 或
Runtime 继承连接描述文件。每个 Agent 仅连接这个宿主，不自行拥有 daemon。

macOS 的 TCC 授权遵循真实的责任进程链。App 直接启动 resident，resident
直接启动嵌入式驱动；独立启动 Rua 时则使用该实际启动链的授权归属。
`--host-bundle-id` 只是标识，不能把权限转移给任意 terminal/gateway/helper。
SDK 的 `--embedded` 与父进程存活管道保留责任链及退出清理；不得替换成
无 owner 的裸 `cua-driver serve`。

首次使用时由 Rua 引导用户完成操作系统授权；授权拒绝、撤销或宿主未就绪
都必须可诊断。需要分别验证签名 App 启动与独立 Runtime 启动；终端测试不
代表签名 App 权限验收完成。Windows 按实际驱动及系统能力报告可用性。

| 范围 | Owner |
|---|---|
| 安装文件、签名、整包 App 更新 | Desktop Shell 与桌面发布流程 |
| 两个驱动的独立更新清单、产物签名与兼容验收 | Rua 组件发布流程 |
| 本机驱动独立更新安装、版本切换与恢复 | resident Runtime |
| Cua 私有 daemon、权限请求、重查与重启 | resident Runtime 的 Cua 宿主适配 |
| 每个 Agent 的 MCP 连接、工具发现与连接关闭 | 该 Agent 的 `McpManagerImpl` |
| Chat 调用、授权、浏览器会话映射与桌面操作协调 | resident Runtime |
| CLI 安装、桌面能力 HTTP API | resident Runtime |

Cua 的每个 Agent MCP 连接可连接到宿主提供的私有执行服务；共享的是物理桌面
执行资源，不是 Agent 的 MCP client、凭据或策略。关闭一个 Agent 的连接不能
关闭其他 Agent 正在使用的原生 daemon。每个物理进程只有一个生命周期 owner。

此边界提供进程与协议隔离，不构成额外的 OS sandbox。

### D5 - 浏览器会话与桌面控制明确归属

Computer Use 中断恢复由现有 resident Runtime 的桌面生命周期 owner 负责，不交给用户手动重启。输入结果未知时不重放已提交操作，不发送后续步骤；保持桌面控制锁，直到 owner 确认旧 daemon 退出、有效新 generation 发布后才允许新输入。恢复失败保留未知状态和实际错误，使用有上限的退避间隔重试；关闭 Runtime 不得重新启动 daemon。工具仍可发现，恢复期间明确拒绝输入；新连接丢弃所有旧观察，必须重新观察目标窗口。换一个 MCP proxy 或收到 session_end 回执不构成旧输入排空证据。

参考：Codex [`rmcp-client/request_cancellation.rs`](https://github.com/openai/codex/blob/main/codex-rs/rmcp-client/src/request_cancellation.rs) 将本地 future 取消与远端请求生命周期关联；Cua [`MCP protocol`](https://github.com/trycua/cua/blob/main/libs/cua-driver/docs/mcp-protocol-and-skills.md) 明确响应丢失后先验证状态、不能直接重试动作。Rua 固定版 Cua 的已提交 OS 操作不可撤回，因此普通 Stop 只等待当前一步收尾；异常 EOF/future 丢弃通过 owner 的停止、reap 和新 generation 完成安全恢复，不假设 MCP 取消通知等于 OS 操作撤销。

浏览器会话按执行 Device 和完整 Chat Address 隔离，可在同一 Chat 的后续
Turn 中复用。per-Agent MCP manager 的隔离不能代替同一 Agent 下不同 Chat
的浏览器会话隔离。接入层必须传递或绑定正确的会话标识，不能依赖驱动的全局
默认会话。会话目录、登录状态和本机标签页句柄不作为 Chat 同步数据传播。

同一物理桌面的输入控制需要在 Device 范围协调，不得把不同 Agent 的 MCP
连接当成不同桌面。会改变焦点、鼠标或键盘状态的交互不能由多个执行 owner
相互穿插；具体协调接口在实现时确定，不新增另一套 Agent 调度或任务模型。

前台工具的取消遵守现有 Stop 语义。停止调用后不得继续发送该调用的后续输入；
只清理该 owner 的工作和资源，不关闭用户浏览器、其他 Chat 的会话或共享
原生宿主。App 退出和更新则按所有权有界地排空调用、停止自有驱动及浏览器
进程。恢复连接不自动重放可能已经生效的点击或输入。

### D6 - 版本与交付由 Rua 统一管理

每次桌面发布固定并记录两个驱动的上游版本或 commit、对应平台产物和完整性
信息，保留其许可证及必要 notices。`agent-browser` 使用 Apache-2.0，Cua
Driver 使用 MIT；实际打包的附属文件也需要保留各自的许可证。

安装包继续携带已验证的驱动版本，保证无需另行安装即可使用。Cua Driver 与
`agent-browser` 按以下约束分别独立更新，复用发布信任、安装和恢复机制，
各自验证兼容性与生命周期。两者都不得自行
升级到上游 `latest`。MCP 注册只负责工具连接，不拥有驱动安装；删除 MCP 配置
不改变系统浏览器的软件包所有权。系统浏览器由用户更新，因此兼容性探测和
失败提示是运行条件。

#### D6.1 - Rua 验证并发布兼容更新

独立更新的来源是 Rua 发布的签名组件清单，不是运行时直接信任上游最新版。
清单绑定驱动版本或 commit、平台与架构、系统要求、产物摘要、许可证、适用
Rua 版本以及宿主、MCP、工具 schema 和能力协议要求。只有通过 Rua 集成
验收的产物才可供对应 Runtime 安装；摘要不能替代清单真实性和原生产物签名校验。

驱动版本检查应遵循已验证的兼容清单，不得简单删除现有版本或协议校验。
若新版本需要修改编译在 Runtime 中的宿主适配、授权或工具接入，提示先升级
Rua；不为独立更新引入动态宿主加载或放宽 Tool Guard、能力与 Chat/Turn 契约。

首期支持两个组件的手动检查、用户确认安装和恢复上一已验证兼容版本。桌面工具
页显示当前版本及来源、可安装的兼容更新、更新状态和失败原因；区分“上游有
新版”与“当前 Rua 可安装”。App 与 WebUI 通过同一 Runtime HTTP API 操作，
沿用设备访问和授权边界。首期不支持静默自动安装、任意下载 URL 或 nightly，
不先建设通用插件更新平台。

#### D6.2 - 安装与恢复不修改签名 App

保留包内驱动，独立更新安装在执行设备的 Rua 本机应用数据目录，使用版本化
位置；安装记录和当前版本选择不随 Agent/Chat 同步。下载与校验完成前不改变
当前可用版本。安装元数据与版本切换必须采用原子写入并保证崩溃一致性。

包内产物仍按整包 release-set receipt 校验，独立产物按签名组件清单及安装
记录校验；不能原地覆盖签名 App 的文件或复用包内 receipt 为外部产物背书。
整包升级、回退与组件切换必须协调，并重新判断已安装独立版本是否兼容；
不兼容时明确报告并选择已验证的兼容版本，没有可用版本则禁用对应组件。

Runtime 继续通过 D4 的嵌入式宿主启动 Cua，Browser Use 保持 D5 的 Chat 会话
所有权；两者均保持唯一 owner、私有连接及自有进程清理。macOS App 外产物的
签名、公证、隔离属性与 TCC 责任链必须专项验收，
不能假设路径变化不会影响授权；Windows 同样验证产物信任与安装恢复。

#### D6.3 - 在安全空闲点切换，不重放桌面操作

下载可以与使用并行，切换必须与退出、重启、整包更新串行协调。Runtime 在
切换前阻止对应组件的新调用进入并有界排空在途调用；无法安全排空时保留当前版本、
推迟切换，不为了更新强行打断用户桌面输入。停止仅限 Rua 自有驱动。

Browser Use 的空闲 MCP 连接不代表 Chat 浏览器会话已结束。不得为更新静默
关闭活跃会话或迁移其登录状态；仍有会话时推迟激活并提示用户先结束会话。
Computer Use 的宿主重启则不得关闭用户日常使用的应用。

候选版本完成启动、协议及权限状态检查后才能恢复服务。重建 per-Agent MCP
连接，作废旧 generation 的观察和元素句柄；新操作必须重新观察，并继续遵守
现有能力快照和审批规则，不在活动 Turn 中静默替换工具契约。

启动或兼容检查失败时恢复上一已验证兼容版本，并明确报告失败；权限缺失或
撤销应如实报告，不能宣称回退能恢复系统授权。安装中断、进程崩溃和机器断电后
应能恢复确定的已验证版本。任何恢复都不得自动重放点击、输入或提交，也不能
撤销已经产生的外部效果。

#### D6.4 - 上游优先，补丁不是永久产品契约

优先使用未经修改的稳定上游产物及公开接口。生命周期能力若可由 Rua 现有
进程所有权机制可靠提供，则不修改驱动；不能为了去补丁另建更复杂的浏览器
启动、会话、取消协议，或放宽 Stop、防重放和隔离要求。

每项仍需保留的上游补丁必须记录具体缺口、版本证据、对应上游 issue/PR、
回归验证和退出条件。上游具备等价能力后删除补丁并更新调用方，不保留平行
兼容路径。不得将当前 `rua-agent-browser/1` 当作所有未来版本的必需标志。
源码差异和 issue 状态只是决策证据；删除安全补丁必须经过真实取消和进程清理
验收，而不能以“上游版本更新了”代替验证。

首期交付平台为 macOS / Windows，可由 App 或独立 Rua Runtime 提供，
App 与普通 WebUI 使用相同 API。Linux CLI/server
和 Android 不因本记录获得桌面能力承诺；尤其不能把桌面驱动打进
`mobile-default`，或用 proot 运行成功推断可以控制 Android 应用。

## Consequences

- 一个 Rua 安装包交付两个驱动，用户只需已有兼容浏览器及必要系统授权。
- 不内置浏览器减少分发体积，但不承诺任意系统浏览器和版本都能工作。
- 独立进程保留上游升级边界，同时增加包内产物、权限宿主和退出更新的验证。
- 两个组件的兼容修复可独立交付，但 Rua 仍承担签名组件发布、兼容矩阵和恢复验收成本；
  宿主或工具契约不兼容的更新仍需升级 Rua。
- 接通 MCP 不是完成集成；会话归属、图像投影和真实桌面并发仍由 Rua 验证。

## Rejected alternatives

- 要求用户先安装两个外部工具或手动配置 MCP：不符合一个 App 的交付目标。
- 为这两项能力先建设通用插件安装器：已有桌面打包与 MCP 边界可以承载。
- 将两个上游仓库整体合并、链接进 resident Runtime：增加维护和运行耦合；
  仅复用固定版本的轻量 Cua lifecycle/权限宿主代码。
- 默认携带或静默下载 Chromium：用户已选择复用系统浏览器。
- 同时支持 Safari、Firefox 或其他引擎：超出首期 Chromium 范围。
- 让 Plugin lifecycle 和 MCP manager 同时拥有一个驱动进程：产生重复生命周期。
- 直接下载上游最新版并替换签名 App 内驱动：绕过兼容验收，破坏产物信任和恢复边界。

## Implementation and verification

### Current implementation anchors and gaps

- D6 的双组件独立更新尚未实现：当前仍绑定包内驱动版本和整包 receipt，没有
  签名组件更新清单、版本化安装、独立切换/恢复或对应 UI。下列已有整包更新
  和驱动重启实现不构成独立更新的验收证据。
- `console/src-tauri/automation/manifest.json` 固定 agent-browser 0.39.4
  (`04ba5264…`) 的源码、Cargo.lock 与剩余的 Rua 生命周期补丁；Cua Driver 0.38.0
  (`85e2b131…`) 固定 macOS universal / Windows x86_64 归档、程序和许可证。
  `scripts/ci/stage-desktop-automation.py` 独立构建并整体 staging 三个程序，
  写入 release-set receipt；Shell 编译时绑定 receipt，校验资源/原生签名或 PE
  内容摘要与实际版本。上游原生禁止重放不确定的可变操作，Unix watchdog 清理自己的
  Chromium 进程组，Windows 保留上游 Job Object。
- Browser Use 的每个 Chat 使用独立 MCP 传输进程，通过上游
  `AGENT_BROWSER_PROFILE` 环境变量固定自己的 profile；能力发现和探测使用
  单独传输及临时 profile。不通过工具 `extraArgs` 注入上游已禁止的 `--profile`，
  不增加放宽上游参数校验的补丁。
- `runtime_authority/browser.rs` 负责发现/选择 Chromium；
  `rua-config/desktop_automation.rs` 保存本机偏好；Console Desktop 设置显示
  探测与恢复入口。macOS 11/12 保留 Runtime/Browser，固定 Cua 程序要求 13+。
- `rua-mcp/bundled_browser.rs` 与 `bundled_computer.rs` 装配 per-Agent
  连接，限定工具/参数并绑定完整 Chat Address 与执行 Device。截图以
  `ContentPart::Image` 进入 Chat/模型请求；实际模型能力覆盖不可信 metadata。
- `crates/rua-server/src/desktop_automation.rs` 使用固定 SDK 的
  `EmbeddedCuaDriverHost` 并调用原生权限 API；SDK 源码与摘要校验由
  resident crate 拥有。只保留宿主 lifecycle 和权限探测，不链接其进程内 capture
  runtime，避免 resident 在 macOS 11/12 因
  缺 ScreenCaptureKit / Swift Concurrency 而无法启动。原生 capture 仍在独立
  daemon 中，11/12 实机降级尚待验证。`computer_control.rs` 对完整 `interact` 持有设备控制，输入
  按 Unicode scalar 提交并在每步间检查取消与目标。已提交的一步不能撤销；
  断连结果未知会锁住该 generation 的后续输入，显式重启后须重新观察。
- 复合操作的 leaf deny/approval 通过现有 Tool Guard，parent 的可复用
  allow 不能吞掉子动作审批。
- Runtime 串行化驱动重启和权限重查，退出取消启动并清理自有 daemon；
  resident 意外退出通过父进程存活管道终止 daemon。Shell 串行化整包更新。macOS 用原子 App 目录交换并在校验失败时回退；
  Windows updater supervisor 等旧 Shell 退出，校验/备份安装目录，执行 NSIS，
  校验失败时恢复文件及安装注册信息。浏览器 profile 与设置始终在安装包外。
- 本机 native Chromium、双代理/宿主生命周期、协议取消/故障和组合正常 Chat
  已有测试。正式签名权限矩阵、Windows 编译/安装/GUI、真实视觉模型、干净
  安装、真实更新/回退仍是发布阻塞项。进程/机器在更新中断电后的自动恢复和
  启动健康回退没有验收证据，不能从目录事务测试推断完成。

### Acceptance evidence required for implementation

1. 在没有 Node.js/Python/独立驱动安装的受支持桌面上，只安装 Rua 并使用系统
   Chromium 浏览器，即可完成两项能力的工具发现和调用；不触发浏览器下载。
   普通浏览器通过 HTTP 完成状态查询、权限申请/重查、驱动重启与 CLI 安装；
   独立启动 Runtime 的测试不得连接 Tauri 或复用运行中 App 的宿主描述文件。
2. 浏览器完成导航、观察、点击/填写及结果验证；截图能进入视觉模型。自动发现、
   含空格的自定义路径、浏览器缺失及不兼容版本都有可核对的结果。
3. 在正式签名的 macOS App 中完成权限申请、拒绝、重新授权和撤销场景；使用
   计算器或文本编辑器验证观察、输入及结果。Windows 执行对应的桌面验证。
4. 同一 Agent 的两个 Chat 不串用浏览器会话；多个 Agent 的桌面输入得到协调，
   关闭一个 MCP 连接不破坏其他 Agent 的连接或原生宿主。
5. Stop、超时、驱动崩溃和断连不引发后续输入或自动重复操作；退出和更新只停止
   Rua 拥有的进程。取消检查覆盖等待桌面控制和正在执行的工具。
6. 打包版本、签名及更新/回退恢复经过验证；`mobile-default` 不引入桌面重依赖。
   普通工具调用仍使用现有能力、Chat 授权快照和审批链路。
7. 两个组件分别验证独立更新的清单签名、产物摘要/签名、平台及协议兼容；损坏、未授权或
   不兼容的更新不能激活。离线和下载失败保留当前可用版本；整包升级/回退后
   不激活不兼容的独立版本。App 与 WebUI 显示相同的执行设备版本和更新状态。
8. 在正式签名安装环境中验证独立更新后的权限、Chrome 真实键盘输入、回车到
   搜索结果页、组合键和多窗口目标选择；文字出现在地址栏或 MCP 握手成功不
   代表输入成功。更新前后均验证 Stop、调用排空、连接重建及旧句柄失效。
9. 独立更新在下载、安装、切换、启动检查阶段的取消、崩溃、断电和恢复都有
   证据；候选启动失败能恢复已验证兼容版本，恢复不重放桌面输入，不修改用户
   浏览器数据，不以版本回退掩盖系统权限撤销。
10. Browser Use 独立更新及补丁删除验证阻塞命令取消、响应丢失不重放、Runtime/
    daemon 异常退出后自有浏览器清理和不同 Chat 隔离。活跃会话阻止版本切换；
    恢复后的操作必须重新观察，不复用旧 refs，不影响用户日常浏览器。

## Related decisions and upstream references

- [ADR-0003](0003-runtime-authority-and-external-process-environment.md): Runtime Authority。
- [ADR-0011](0011-effective-capability-snapshot-and-tool-exposure.md): Effective Capability Snapshot。
- [ADR-0012](0012-per-agent-tool-guard-and-process-wide-approval.md): Chat 授权与 Tool Guard。
- [ADR-0013](0013-mcp-per-agent-manager-isolation.md): per-Agent MCP manager。
- [ADR-0014](0014-plugin-process-and-protocol-isolation.md): 进程与协议隔离。
- [ADR-0024](0024-stop-interjection-and-viewer-disconnect.md): Stop 语义。
- [ADR-0028](0028-chat-running-config-and-turn-stable-snapshot.md): Chat 配置快照。
- [ADR-0049](0049-tauri-desktop-shell-and-local-resident-server.md): Desktop Shell 与发布边界。
- [agent-browser](https://github.com/vercel-labs/agent-browser): 原生程序、系统浏览器与 MCP 接口。
- [Cua Driver](https://github.com/trycua/cua/tree/main/libs/cua-driver): MCP 接口与原生权限宿主。
- [Cua embedding contract](https://github.com/trycua/cua/blob/main/libs/cua-driver/rust/Skills/cua-driver/EMBEDDING.md)。

发布依赖必须固定具体版本，并验证所使用的接口和平台支持；不能以浮动
`main` 作为发布依赖。
