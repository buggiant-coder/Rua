<p align="center">
  <img src="console/src-tauri/icons/128x128.png" width="88" alt="Rua 标志" />
</p>

# Rua

**All your devices. One mind.**

**让手机和电脑上的 AI 一起为你工作。**

每台设备都有完整的 Agent。把自己的设备配对，就能同步聊天和状态，
并选择让哪台设备执行任务。

用手机让工作站处理代码，在笔记本上接着查看同一段对话，
也可以让手机直接使用自己的文件和工具。

[English](README.md) · [使用入门](docs/getting-started.zh-CN.md) · [架构设计](docs/adr/README.md)

> **预发布阶段：** Rua 正在积极开发，各平台的打包和验收仍在推进。
> 当前进度见下方的平台状态。

## 为什么用 Rua

### 每台设备都有完整的 Agent

手机、笔记本和工作站各自拥有运行中的 Agent、本机文件和工具。
Android 原生运行 Rua，并自动准备 Debian Linux 工具环境，
用于执行脚本、技能和外部程序。

### 配对设备，连接交给 Rua

跨网络连接，无需配置公网 IP、路由器端口映射或 VPN。
Rua 自动发现连接路径并尝试内网穿透（NAT 打洞），能直连就直连；
直连不可用时可通过中继传输，连接保持端到端加密。
实际连通仍受网络限制和中继服务可达性影响。
详见[设备网络设计](docs/adr/0037-device-network-runtime-and-shared-device-sessions.md)。

### 换了设备，对话和上下文也能跟上

设备配对后，通过加密的点对点同步共享聊天、Agent 配置、技能和结构化记忆。
在另一台已配对设备上打开对话，就能继续查看上下文。
本地数据是各设备的依据，另一台设备离线不会阻止本地数据写入。

### 需要哪台设备的能力，就让哪台执行

创建聊天时选择执行设备。你可以在手机上保持对话，让已配对的电脑
使用它自己的代码仓库、文件和工具完成任务。设备之间地位平等，
各自提供本机拥有的能力。

### 从手机访问电脑上的编码助手

在执行设备上配置 Codex、Claude Code 等支持 ACP 的编码助手，
就能从另一台已配对设备查看并使用这段编码对话。
ACP 是 Rua 与这些编码助手连接的通信方式；Rua 保存对话，
并转达编码助手提出的权限请求。编码助手需要在执行设备上单独安装和配置。

## 可以怎么用

| 场景 | Rua 的使用方式 |
| --- | --- |
| 代码在工作站，你手里只有手机 | 创建一段由工作站执行的聊天，让它处理那里的代码仓库。 |
| 从手机换到笔记本 | 在已配对笔记本上打开同步过来的同一段对话。 |
| 想直接在 Android 上工作 | 使用手机本机 Agent、文件、Linux 工具和 Android 系统能力。 |
| 已有常用的外部编码助手 | 在执行设备上配置助手，通过 Rua 使用它的 ACP 聊天。 |

跨设备执行需要目标设备在线、相关工具可用。聊天的执行设备在创建时确定，
后续保持不变。

## 按自己的需要扩展

- **模型：** 配置模型服务商或兼容的模型服务地址。
- **工具：** 连接 MCP 工具服务、启用技能、安装插件；依赖和可用范围因设备而异。
- **自动化：** 使用定时任务和已支持的外部消息渠道。
- **设备操作：** 使用桌面浏览器和电脑自动化、Android 原生集成；具体能力受平台支持和系统权限约束。
- **权限控制：** 按 Agent 配置安全策略，查看操作确认和审计记录。
- **记忆：** 使用结构化记忆、本地向量检索及可同步状态。

Rua 的运行时使用 Rust 构建，桌面和 Android 共享 Web 界面。
模型调用使用你配置的服务；数据优先存本地，不代表所有模型和工具都能离线运行。
Android 的 PRoot 环境提供 Linux 兼容能力，不是安全沙箱。
详见[安全模型](docs/security/threat-model.md)。

## 平台状态

目标是提供 Windows、macOS、Linux 独立桌面应用，以及 Android APK。
当前源码包含以下构建路径：

| 平台 | 架构 | 安装包 | 发布准备状态 |
| --- | --- | --- | --- |
| macOS | Apple Silicon、Intel | `.dmg` | 已有构建路径；正式签名、苹果公证和安装验收仍需完成。 |
| Windows | x64 | `.exe` 安装程序 | 已有构建路径；需完成原生构建、签名和安装验收。 |
| Linux | x64 | `.deb` 桌面应用 | 已加入打包入口；原生构建和安装验收待完成。 |
| Android | ARM64 | `.apk` | 已有构建路径；当前使用公开测试密钥，正式签名与设备验收仍需完成。 |

以上是构建目标，不表示各平台已经有公开下载的安装包。
桌面自动化另有系统要求，见[桌面自动化说明](docs/desktop-automation.md)。

## 开始使用

1. 在设备上运行 Rua，配置模型服务。
2. 创建聊天，先用一个小的测试目录验证任务。
3. 配对另一台设备，再创建一段由那台设备执行的聊天。

[使用入门](docs/getting-started.zh-CN.md)包含模型配置、设备配对、权限、
更新和数据位置。模型服务凭据由你提供，Rua 不附带模型调用额度。

### 从源码运行

安装 Rust 1.91+、Node.js/npm、Python 3、protoc 和平台所需的编译工具。
仓库默认使用 sccache；可安装它，或为构建设置 `RUSTC_WRAPPER=`。
桌面壳的系统依赖见 [Tauri 前置要求](https://v2.tauri.app/start/prerequisites/)。

```bash
cd console
npm ci
cd ..
RUA_ROOT="$HOME/.rua-dev" cargo run -p rua-cli -- init
RUA_ROOT="$HOME/.rua-dev" cargo run -p rua-cli -- server start --host 127.0.0.1 --port 8765 --no-auth
```

另开一个终端启动前端：

```bash
cd console
npm run dev
```

打开 `http://127.0.0.1:5173`。开发 API 监听 `http://127.0.0.1:8765`。
这个开发入口关闭认证，仅供本机使用。模型可在界面配置，也可执行
`cargo run -p rua-cli -- setup-env`。

桌面打包使用对应原生平台上的 [build-desktop.sh](scripts/ci/build-desktop.sh)，
签名要求见[发布签名说明](docs/release-signing.md)。
Android 打包见 [Android 构建说明](docs/android-packaging.md)。

## 参与贡献

[领域词汇](CONTEXT.md)和[架构决策](docs/adr/README.md)说明项目如何组织。
测试规范和贡献约束见 [AGENTS.md](AGENTS.md)。

| 目录 | 内容 |
| --- | --- |
| `libs/` | 基础类型、接口约定、存储和模型服务抽象 |
| `crates/` | Agent 运行时、服务端、同步、工具、渠道、安全、记忆和命令行 |
| `console/` | 共享 Web 界面与桌面壳 |
| `mobile/mobile-shell/` | Android 原生应用宿主 |
| [python_sdk/](python_sdk/README.md) | Python 插件开发工具 |

只检查本次修改涉及的 crate：

```bash
scripts/tests/clippy-touched.sh --test
```

常规本地检查根据 Git 差异选择范围。没有 Git 信息或无法取得比较基线时，
脚本自动检查全部 workspace 成员和独立的 Desktop crate，耗时会更长。

## 许可证

Rua 自有代码和文档默认使用 [Apache License 2.0](LICENSE)，
文件或子目录另有声明的除外。早期 Web 界面及部分兼容模块包含来自
QwenPaw 的材料，相关归属保留在 [NOTICE](NOTICE) 中。

Android 宿主源码使用 Apache-2.0；随 APK 提供的 PRoot、原生依赖以及
其他第三方和 vendored 材料分别保留上游许可证。四项内置文档技能使用
固定版本的公开 Apache-2.0 上游来源。

完整边界见 [LICENSES.md](LICENSES.md) 和
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
各发行产物尚需完成的许可与源码交付要求见
[发行许可要求](docs/open-source-licensing.md)。
