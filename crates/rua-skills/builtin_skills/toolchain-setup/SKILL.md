---
name: toolchain-setup
description: 配置或修复 Rua 所需的桌面外部工具链。Use when the user asks to install, configure, repair, or inspect Python, uv, Node.js LTS, npm/npx, Git, ripgrep, ffmpeg, PATH, or China-friendly package mirrors on macOS, Windows, or a command-line Linux host.
metadata:
  builtin_skill_version: "1.0"
  rua:
    emoji: "🧰"
---

# Toolchain Setup

目标：让当前机器具备 Rua Skills、MCP、ACP 和媒体工具常用的外部命令。

推荐能力：

- Python 3
- uv
- Node.js LTS
- npm / npx
- Git
- ripgrep (`rg`)
- ffmpeg

这些工具安装到操作系统和用户平常使用的默认位置。不要创建 Rua 私有 Python、Node 或工具链目录。

## 工作流

1. 调用 `runtime_info`，读取 Runtime Authority 当前看到的能力、路径和版本。
2. 只处理缺失、损坏或用户明确要求重装的能力。不要替换已经工作的安装。
3. 根据操作系统、CPU 架构、现有包管理器和网络环境生成计划。
4. 在执行前向用户展示：
   - 要安装或修改什么
   - 将运行的命令
   - 将修改的配置文件或环境变量
   - 是否可能出现 sudo、UAC 或系统确认
   - 是否需要重启终端或 Rua Runtime
5. 获得明确确认后，使用 `bash` 分步执行。不要把整套安装塞进一个不可检查的大脚本。
6. 每项完成后立即验证；失败时保留已经成功的项目，并报告具体失败项。
7. 再次调用 `runtime_info`。如果新终端能看到命令而当前 Runtime 看不到，提示用户退出并重新打开 Rua Desktop。

## macOS

优先复用 Homebrew。Homebrew 不存在时，先说明将使用官方安装方式并再次确认，不要静默安装包管理器。

常规安装：

```bash
brew install python uv git ripgrep ffmpeg
```

Node.js 必须使用当前 LTS。先用 Homebrew 查询可用版本，再选择 LTS formula；本 Skill 发布时对应 `node@24`：

```bash
brew install node@24
brew link --force --overwrite node@24
```

仅当 Node、npm、npx 都不可用时才安装整套 Node.js。若只缺其中一项，先诊断现有 Node 安装，避免覆盖用户的 nvm、fnm、Volta 或其他工作环境。

## Windows

优先使用 Windows Package Manager (`winget`)。逐项安装，保留正常 UAC 提示：

```powershell
winget install --exact --id Python.Python.3.14
winget install --exact --id astral-sh.uv
winget install --exact --id OpenJS.NodeJS.LTS
winget install --exact --id Git.Git
winget install --exact --id BurntSushi.ripgrep.MSVC
winget install --exact --id Gyan.FFmpeg
```

安装 Git 后重新检查 Git Bash。不要为了 Rua 创建私有 PATH；使用安装器写入的用户或系统 PATH。安装完成后用新终端验证。

## Linux 命令行主机

Linux 没有 Rua Desktop Shell，但可以在 CLI/headless 场景使用本流程。优先使用发行版标准包管理器（apt、dnf、pacman 等），先展示计划再执行。不要假设所有发行版使用同一包名。

## 中国网络镜像

如果用户位于中国大陆、下载明显受阻，或用户明确要求国内镜像，先检查已有配置，不要覆盖用户已经选择的镜像。

推荐先让用户确认镜像提供方。可使用以下常见配置：

```bash
python -m pip config set global.index-url https://pypi.tuna.tsinghua.edu.cn/simple
npm config set registry https://registry.npmmirror.com
```

uv 使用同一 Python 包索引：

```text
UV_DEFAULT_INDEX=https://pypi.tuna.tsinghua.edu.cn/simple
```

将 `UV_DEFAULT_INDEX` 写入用户当前平台的正常持久化环境配置前，必须展示将修改的位置并获得确认；避免重复追加 shell profile。

Homebrew 镜像地址和变量可能变化。需要配置时，先读取清华 TUNA 或中科大 USTC 当前 Homebrew 镜像说明，再设置其文档列出的 API、Bottle、brew Git 和 core Git 地址。不要凭记忆写入过期地址，也不要全局改写所有 GitHub URL。

## 验证

至少验证：

```bash
python3 --version
uv --version
node --version
npm --version
npx --version
git --version
rg --version
ffmpeg -version
```

Windows 可用 `where.exe`，Unix 可用 `command -v` 检查实际路径。

最终报告必须列出：

- 成功且可用的能力
- 仍缺失或失败的能力
- 实际命令路径
- 已配置的镜像
- 是否需要重启终端或 Rua Runtime

## 护栏

- 未获得明确确认，不安装软件、不修改配置。
- 不卸载或覆盖用户已有的可用工具链。
- 不使用 Rua 私有缓存目录伪装系统安装。
- 不关闭 sudo、UAC、签名校验或包管理器安全检查。
- 不把平台安装逻辑实现成 Skill 自带脚本；由 Agent 使用现有工具按计划执行。
