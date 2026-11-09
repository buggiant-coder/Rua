# Android 构建与运行环境

Rua APK 直接启动 Android/Bionic `rua` 可执行程序。PRoot 只承载 Debian、Python、Node、技能与外部进程，交互终端也进入同一 Debian。

## 构建

需要 JDK 17、Android SDK（platform 36、build-tools 36）、NDK 29.0.14206865、Rust Android targets、cargo-ndk、Node/npm、Python 3、patchelf 和 protoc。SDK 路径通过 `ANDROID_HOME` 指定。仓库 Cargo 配置使用 sccache，需安装并可执行。

```bash
export ANDROID_HOME="$HOME/Library/Android/sdk"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/29.0.14206865"
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --locked
scripts/package-mobile-apk.sh --debug
```

唯一打包入口为 `scripts/package-mobile-apk.sh`。默认 debug，用于日常开发和模拟器。`--release` 使用 `release-android`，用于最终载荷和体积验收。只有明确已有匹配产物时才传 `--skip-runtime-build` / `--skip-console-build`。Gradle 直接调用需要先准备对应 Rust 二进制和 console/dist。

产物为 `target/mobile-apk/rua-{debug,release}-arm64-v8a.apk` 和对应 `rua-*-payload.json`。报告记录 APK/ELF/资源/manifest SHA-256、版本、SDK、ABI、签名证书、许可证和首装下载量。当前签名是仓库开发验收密钥，不等于正式分发签名。

当前 APK 仅 ARM64；min SDK 26、target SDK 28、compile SDK 36。x86_64 Rust target 检查不代表提供 x86_64 APK。支持范围以本次设备矩阵实际结果为准；旧 Android 镜像不可用或未测时不得记为通过。

构建并安装到指定设备时，使用 `scripts/package-mobile-apk.sh --debug --install -s SERIAL`；正式构建改用 `--release`。打包过程会先校验 APK 载荷再安装。安装已有 APK 时，先运行 `python3 scripts/android/report-native-apk.py --apk PATH --out target/apk-payload.json` 校验，再用 `adb -s SERIAL install -r PATH` 覆盖安装。

## APK 与独立环境

APK 携带 Rua ELF、WebUI、终端 bridge 和小型 PRoot/loader/talloc/shmem/进程管理组件。原生依赖的固定版本、来源和 SHA-256 在 `mobile/runtime/native-dependencies.json`，许可文本随 APK 发布。

Debian 13、Python 3.13 及其固定 Debian 包、Node 24 和 uv 由 `mobile/runtime/runtime-manifest.json` 定义，约 165.5 MB 下载材料，均须大小和 SHA-256 校验。APK 不携带这些用户空间归档。

首次打开显示全屏准备页并自动准备环境；原生服务独立启动，Debian、Python、Node、uv 全部验证就绪后才创建正式 WebUI。下载器并发检测候选 HTTPS 源，按实际 HTTP 响应延迟排序，仅使用能提供目标材料的源；不使用 ICMP ping 代替下载可用性。失败自动切换，已验证材料复用。用户不需要选择源或点击安装，可查看进度、取消、重试和离线导入。首装页提醒准备需要时间，并提供“设置所需权限”；用户跳转系统设置时安装继续。准备页沿用 Console 的灰色明暗配色。就绪后的正式界面不保留环境状态栏，“工具环境管理”和“权限设置”收在聊天页更多菜单中。Debian 包候选包含官方、清华、中科大、华为云和固定快照；Node 包含官方、npmmirror 和华为云；uv 包含中科大与 Astral 官方。各文件独立选择响应最快的合格来源，不保证固定走某个站点；所有来源必须通过同一 SHA-256 校验。

离线导入材料可由以下命令生成；生产首次启动不依赖此 ZIP：

```bash
python3 scripts/android/prepare-runtime-offline.py --out target/debian-runtime-offline.zip
```

环境与 APK 独立更新。覆盖 APK 保留兼容环境，不自动重装；显式环境更新先在私有 staging 中验证，保留 guest HOME、workspace 和已装依赖，通过运行租约阻止更新与活跃子进程同时修改环境。失败及中断按日志恢复。

目录边界：`files/home` 为规范用户数据，`files/runtime/debian` 为 Debian，`files/guest-home` 为 Linux HOME，`files/app-resources/<sha>` 为 APK 配套资源。首次安装按 manifest 创建 Debian；后续更新仅管理本架构创建的环境。

## 构建验证

打包入口会校验 APK 的原生载荷、资源哈希和签名。无需 SDK 或设备的打包脚本检查可运行：

```bash
python3 scripts/tests/check-android-release-contract.py
```

该检查不代表 APK 已构建或真实设备验收通过。公开发布的设备支持范围、安装占用、首启耗时和 APK 体积应以对应版本的发布说明为准。
