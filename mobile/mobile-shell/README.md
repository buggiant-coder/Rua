# Rua Android

APK 直接启动 Android/Bionic Rua 主程序，WebView 提供界面。PRoot 下的 Debian 为 Agent、技能、MCP、ACP、Plugin 和交互终端提供 Python/Node 环境。

首次启动自动检测镜像并安装环境；APK 只分发原生程序、WebUI、PRoot 及必要依赖。

- [构建与安装](../../docs/android-packaging.md)
- [环境材料与更新](../runtime/README.md)

本项目尚未发布，运行和验收均以此架构为准，不提供历史 APK 或其他发行版数据转换。

Android 宿主采用 Apache-2.0，见 [许可证](LICENSE.md)。不包含 Termux App 宿主、终端模块或 DocumentsProvider。PRoot 及原生第三方依赖保留各自许可证，见 [runtime/licenses](../runtime/licenses/)。

## 代码入口

- `MainActivity.kt`：WebView、原生启动与环境进度。
- `NativeRuntimeService.kt` / `NativePayload.kt`：驻留进程与 APK 资源。
- `RuntimePreparation.kt` / `RuntimeMirrorDownloader.kt`：自动准备与镜像测速下载。
- `LinuxRuntimeInstaller.kt` / `RuntimeToolchains.kt`：校验、Debian 与工具链激活。
- `RuntimeUpdate.kt`：Debian 环境独立更新、依赖保留与事务恢复。
- `PhoneUseBridge.kt` / `MobileAccessibilityService.kt`：Android 手机能力。
- `MobileFileBridge.kt`：系统文件选择器保存与 Downloads 导出；附件由 WebView 调用系统文件选择器。

宿主不向系统文件选择器提供私有 HOME 根目录，不包含 Termux App 的 DocumentsProvider。
