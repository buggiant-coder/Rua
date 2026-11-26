# Android 构建与运行环境

Rua APK 直接启动 Android/Bionic `rua` 可执行程序。PRoot 只承载 Debian、Python、Node、技能与外部进程，交互终端也进入同一 Debian。

## 构建

需要 JDK 17 或更高、Android SDK（platform 36、build-tools 35）、NDK 29.0.14206865、Rust Android targets、cargo-ndk、Node/npm、Python 3、patchelf 和 protoc。SDK 路径通过 `ANDROID_HOME` 指定。仓库 Cargo 配置使用 sccache，需安装并可执行。

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

### arm64 主机（Linux / Android PRoot Debian）

上述流程默认 x86-64 主机。arm64 主机（含在 Android 上用 PRoot 跑 Debian 的场景）
只需替换少数原生工具，**不需要重写打包流程，也不需要 x86-64 机器或 CI**。

关键前提：**Gradle 与 AGP 是纯 Java 程序，与宿主架构无关**。真正只有 x86-64
发布物的，仅是 AGP 会 fork 出去调用的个别命令行工具。因此正确做法是保留真实
AGP，只替换那几个工具——绕开 Gradle 自行拼装 APK 需要重新实现 AAR 资源合并、
Manifest 合并与 R.id 分配，代价远高于此。

**1）`aapt2`（唯一的架构缺口）**

`aapt2` 各版本均只提供 x86-64 ELF，不存在 `-linux-aarch64` 产物。若内核无
`binfmt_misc`（PRoot 内不可用），x86-64 ELF 直接执行会 SIGILL，必须显式包装：

```bash
# 取 AGP 自身依赖的那一份，保证 proto 版本与 AGP 匹配
AAPT2_SRC=$(find ~/.gradle/caches -path '*aapt2-*-linux/aapt2' -type f | head -1)
mkdir -p "$HOME/android-host-tools"
cp "$AAPT2_SRC" "$HOME/android-host-tools/aapt2-x86_64"
printf '#!/bin/sh\nexec qemu-x86_64-static %s "$@"\n' \
    "$HOME/android-host-tools/aapt2-x86_64" > "$HOME/android-host-tools/aapt2"
chmod +x "$HOME/android-host-tools/aapt2"
```

再让 AGP 使用它（写入 `~/.gradle/gradle.properties`）：

```properties
android.aapt2FromMavenOverride=/absolute/path/to/android-host-tools/aapt2
```

AGP 以 daemon 模式通过 stdin/stdout 与 aapt2 通信，qemu 转发透明，开销可忽略。

**2）Rust 侧一次性准备：`scripts/android/prepare-aarch64-host.sh`**

NDK 官方仅发布 `linux-x86_64` 宿主包，但**宿主二进制是 x86-64，`sysroot` 内的库
是目标架构 aarch64**。因此用发行版原生 clang 接 NDK sysroot 即可交叉编译，无需
模拟执行。这一步的准备工作由脚本完成：

```bash
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/29.0.14206865"
scripts/android/prepare-aarch64-host.sh --check   # 只报告缺失项
scripts/android/prepare-aarch64-host.sh           # 应用
```

脚本是幂等的，且在非 aarch64 主机上直接退出（x86-64 上 NDK 自带驱动可用，
`cargo-ndk` 也会生成同名 wrapper）。幂等指**按内容校正**，不只是"文件已存在
就跳过"：每个 wrapper 都内嵌绝对路径，SDK 换了位置（例如把工具链在应用数据与
guest rootfs 之间搬迁，或换了 `ANDROID_HOME`）后，旧 wrapper 仍能执行却指向已
不存在的目录，症状是头文件或运行库「找不到」而错误信息里没有任何路径线索。
因此 `--check` 会对内容不符的项报 `needs update`，重跑即就地改写。它做四件事：

- 为 `sysroot` 内**每个** API 目录补 `libunwind.a`（`rustc` 的 android target
  链接时带 `-lunwind`，而 NDK 把它放在 clang 运行时旁边而非 sysroot 里）；
- 为每个 API 补 `libclang_rt.builtins.a` 到 clang 资源目录的
  `aarch64-unknown-linux-android<API>/`（`rustc` 默认要 `-lgcc`，改用
  `-rtlib=compiler-rt` 后找这个文件）；
- 生成 `aarch64-linux-android<API>-clang` 与**无 API 后缀**的
  `aarch64-linux-android-clang` wrapper。无后缀那个不是冗余：`aws-lc-sys` 经
  `cc` crate 按三元组查找编译器，缺它会报 `failed to execute compile`；
- 把 NDK 内 x86-64 的 `clang`、`llvm-strip`、`llvm-ar`、`ld.lld` 等驱动替换为
  发行版原生版本（原件备份为 `*.orig-x86`），使 `stage-native-payload.py` 这类
  从 NDK 目录调用工具的路径可直接工作。NDK 把这些工具中的若干组织为指向同目录
  兄弟文件的符号链接（`llvm-strip -> llvm-objcopy`、`llvm-ranlib -> llvm-ar`、
  `llvm-readelf -> llvm-readobj`），替换时必须先 `unlink` 再写，否则会透过链接
  覆盖掉被指向的那个工具，表现为 `ar` 报 `unable to load 'cq'`、`strip` 变成
  `objcopy` 之类难以归因的失败。

wrapper 默认写入 `$HOME/.local/bin`，可用 `ANDROID_HOST_TOOLS` 改到别处，需确保
该目录在 `PATH` 上。

注意 `cargo ndk` 的 `-P/--platform` **默认为 21**，且以
`CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` 之类的环境变量注入，优先级高于
`.cargo/config.toml` 里按三元组配置的 `linker`。因此不显式传 `-P` 时，
cargo-ndk 会选 `aarch64-linux-android21-clang`，产物 `.note.android.ident`
记录 API 21——即使 `minSdkVersion=26`、config.toml 写的是 26。这种降级**不会
让构建失败**，只会让 ELF 注记与 APK manifest 声明不一致。

为避免这个问题，`build-android-native-runtime.sh` 总是显式传 `-P`，取值顺序为：

1. `-P/--platform` 命令行参数；
2. `CARGO_NDK_PLATFORM` 环境变量；
3. `mobile/mobile-shell/gradle.properties` 的 `minSdkVersion`；
4. 兜底常量 26。

`package-mobile-apk.sh` 用同名参数透传，CI 从 `gradle.properties` 读出后传入，
因此 `minSdkVersion` 变更时链路自动跟随，无需改 workflow。构建结束会回读
`.note.android.ident` 打印实际 API，不一致时告警。手动核对：

```bash
llvm-readelf -n <binary> | sed -n '/note.android.ident/,/description data/p'
# description data 的前 4 字节是小端 uint32，例如 "1a 00 00 00" = 26
```

**3）验证交叉编译**

```bash
cargo build -p rua-cli --target aarch64-linux-android --features mobile-default
```

产物应只 NEEDED `libdl`/`libm`/`libc`，且 LOAD 段按 16 KB 对齐，满足
Android 15+ 设备要求。

**4）build 目录不能放在 FUSE 文件系统**

Gradle/AGP 依赖可靠的 POSIX 文件锁。若仓库位于 FUSE 挂载（如 Android
`/storage/emulated`），构建会长时间停在
`Waiting to acquire exclusive lock`，或不再产出 `intermediates`。用 init 脚本把
build 目录重定向到本地文件系统：

```groovy
// ~/.gradle/init.d/redirect-builddir.gradle
def outRoot = new File(System.getenv('HOME'), 'android-build')
gradle.allprojects { p ->
    p.layout.buildDirectory.set(p == p.rootProject
        ? new File(outRoot, 'root')
        : new File(outRoot, p.path.replace(':', '/')))
}
```

**副作用**：`scripts/package-mobile-apk.sh` 的产物路径按仓库内
`mobile/mobile-shell/app/build/...` 计算。脚本已改为向 Gradle 查询 `buildDir`，
因此在重定向后仍能取到 APK。Rust 产物若不在仓库 `target/` 下，用
`RUA_ANDROID_BINARY` 指定（`app/build.gradle` 支持的覆盖点）：

```bash
export ANDROID_HOME=/path/to/android-sdk GRADLE_USER_HOME="$HOME/.gradle"
export RUA_ANDROID_BINARY=/path/to/target/aarch64-linux-android/debug/rua
export RUA_GRADLE=/path/to/gradle        # 优先使用已安装的 Gradle，跳过 wrapper 下载
scripts/package-mobile-apk.sh --debug
```

`RUA_GRADLE` 未设置时，脚本优先使用 `PATH` 上的 `gradle`，否则回退到仓库自带的
`gradlew`。

**5）其它注意点**

- AGP 8.13.2 请求 **build-tools 35**，不是 36；首次构建会自动下载，需先接受
  SDK 许可证（`licenses/` 下写入 `android-sdk-license`）。
- 包名 `lib*.so` 之外的条目须 16 KB 页对齐。发行版打包的 `zipalign` 版本较旧、
  不支持 `-P`，应使用 SDK `build-tools` 内的 `zipalign -c -P 16` 校验。
- `stage-native-payload.py` 需要 `patchelf`，打包前置检查会显式验证。
- 若仓库位于 noexec 挂载，脚本没有执行位，必须用 `bash scripts/...` 调用。
- FUSE 类文件系统不支持 `fcntl.flock`（返回 `ENOSYS`）。Console 构建锁在这种
  情况下会降级为不加锁执行并打印一次提示，而不是中断打包。
- 需要 `build-tools` 的脚本应按"已安装的最高版本"探测，不要硬编码版本号：
  AGP 请求的版本与手工安装的未必一致。
- 全程仅 `aapt2` 经 qemu 执行，C 编译与 Rust 链接均为原生，构建时间与 x86-64
  主机同数量级。

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
