# iroh 升级与设备同步恢复：踩坑记录和回归清单

记录日期：2026-11-14。适用于本次 Mac + 原生 Android APK 同步恢复修复。
这是维护指南和验收记录，不新增连接管理协议；架构边界以
[ADR-0037](adr/0037-device-network-runtime-and-shared-device-sessions.md)、
[ADR-0040](adr/0040-sqlite-inbound-inbox-and-sync-sidecar-store.md)、
[ADR-0043](adr/0043-sync-presence-mobile-power-and-background-scheduling.md) 为准。

## 升级前先读

**不能因为上游版本更新、代码能编译，或补丁发生冲突，就直接删除本地修复。**
先逐项证明新版具有等价行为，再用对应回归和原生 APK 验收确认。
补丁可以被上游实现替代，不要求永久保留同一段代码。

本轮最终依赖组合：iroh `1.0.0-rc.0`、iroh-docs `0.99.0`、iroh-blobs
`0.101.0`、iroh-gossip `0.99.0`、noq/noq-udp `1.0.0-rc.0`、netwatch `0.17.0`。
版本来源是根目录 `Cargo.toml` / `Cargo.lock`；不要把本轮修复描述成已整体升级最新版。
本地差异入口：
[iroh 补丁清单](../vendor/iroh/RUA-PATCHES.md)、
[iroh-docs 补丁清单](../vendor/iroh-docs/RUA-PATCHES.md)。

升级核对要覆盖三层，不能只比较 `vendor/iroh`：

- 传输依赖：iroh、noq、netwatch、relay 的行为及本地补丁。
- Rua 接入：共享 Endpoint、发现服务、Android 回调、Docs/Blobs 参数与 inbound 读取。
- 原生宿主和测试环境：APK 权限、锁屏策略、签名、实际运行二进制、模拟器地址。

## 1. Android 看不到默认路由，不等于没有网络

**症状：** 手机已经获得 IP、网络恢复回调也已到达，QUIC 恢复仍额外等约 5 秒。
`Actor::has_usable_network` 先要求 `default_route_interface.is_some()`，把未知当成了离线。

**证据：** netwatch 0.17 在 Android 使用 `/system/bin/ip route show table 0`；
本轮在模拟器 APK 的普通应用 UID 下得到
`Cannot bind netlink socket: Permission denied`。
ADB shell 的权限与应用 UID 不同，shell 能读路由不能证明 Rua 能读。
真机非 debuggable APK 无法 `run-as`，不能把该失败误当成 Rua 网络故障。

**当前修复：** [socket.rs](../vendor/iroh/src/socket.rs) 的 Android 分支不再以
默认路由可见性作为必要条件，仍要求 `have_v4 || have_v6`；其他平台保留原判断。
否则 `PendingNetworkChangeNotify::MAX_WAIT` 为 5 秒，定时重试可能约 5.5 秒才通知 QUIC。
相关上游线索是 iroh issue #4657（无默认路由的 LAN 场景，链接见补丁清单）；
本轮 Android 分支是本地修复，不称为该 issue 的官方补丁。

**升级删除条件：** 新版在普通 Android 应用权限下能正确识别可用网络，或不再依赖
不可读取的默认路由；必须以原生 APK 断网恢复验证。macOS 上复制一个布尔判断的单测
不能覆盖 Android 权限限制。以后看到 `None`，先区分“不可观测”和“确实不存在”。

## 2. 原有握手加速不能在升级时丢掉

此前 2026-11-06 的冷启动优化已经写入 ADR-0037：共享 Device Session、DNS 与 HTTPS
Pkarr 并行发现、IPv6 候选发布，以及防止旧路径拖住新握手的 Initial 重放。
本轮曾重新遇到同类连接等待，因此升级必须逐项核对这些行为，不能从零再设计一遍。

[remote_state.rs](../vendor/iroh/src/socket/remote_map/remote_state.rs) 只保存连接建立前的
Initial flight：最多 8 个发送项、64 KiB、10 秒有效期；新发现的非本地路径立即尝试，
重复发现不重复发送，连接建立后清空。新握手尝试 selected path 和其他已知路径，
没有正在进行的发现时仍启动发现。**不能改成只试旧 selected path，也不能重放业务数据。**

升级删除条件：新版对“旧连接仍开着，但对端已重启换地址”和“旧 DNS 先到、新 HTTPS 后到”
都能及时握手。对应测试：
[discovery_handoff.rs](../crates/rua-device-network/tests/contract_session/discovery_handoff.rs)。
单测只覆盖当前地址可达不足以替代 stale-first / delayed-discovery 场景。

## 3. “中继队列已满”不是网络状态快照排队

本次报错来自 iroh 内部 **256 项 relay 发送队列**，不是 Rua 新建的网络探测消息队列，
也不是持久业务消息 Inbox。对端没有运行时，仍可能有连接尝试和 QUIC 重传；
接收方离线不代表发送方没有活动。但具体一次满队列仍须结合发送、relay 连接和退避日志，
不能仅凭“手机没运行”断定唯一原因。

真正的阻塞点是 peer actor 等 relay 队列容量，连带无法处理新直连地址、查询和关闭。
当前修复是在该 Initial 发送路径队列满时丢弃该数据报，让 QUIC 按原规则重传；
不在 actor 内等待容量，也不另起无限排队任务。**不要推广成随意丢弃业务同步操作。**
回归用真实 256 项队列验证满载时查询、直连发现、关闭仍可工作，容量恢复后无过期握手补发。

另一个独立等待点是 relay 重连退避：最长 16 秒的等待不能挡住优先 route 查询和取消。
当前保留正常退避，但断开的 relay 立即回答无路由。上游线索 #4512 / #4444 见补丁清单。

不采用“业务只走直连”的撤回方案：连接路径和打洞仍交给 iroh，允许 relay 承载业务。
网络观察只需当前状态的地方不要另加 FIFO；但不能据此删除 QUIC 必要的传输缓冲，
或 ADR-0040 中负责持久业务操作的 SQLite Inbound Inbox。

## 4. 共享对端地址，不代表各条 QUIC 连接共享路径健康

- 上游 #4284 对应修复：旧连接 abandon 一个地址，不能关闭同地址上的健康兄弟连接。
- 本地路径选择修复：已建立连接使用自己的 RTT 和切换阈值；peer-wide selected address
  是拨号提示，不能把新连接唯一可用路径降为 Backup。
- 保留 #4296 的冗余 direct path 清理：即使选择未变化也要清理，避免耗尽 path ID。
  本轮“只有选择变化才清理”的尝试已撤回。
- #4551 对应修复：pending path-open retry 按地址去重。一次 tick 会遍历多条连接，
  不能每条失败连接再追加一份，导致下一轮工作量成倍增长。保留原 333 ms 调度节奏。

升级时不仅检查能连接，还要跑
[真实多连接路径隔离测试](../vendor/iroh/tests/unit_path_isolation.rs) 和
[重试有界测试](../vendor/iroh/tests/unit_path_retry.rs)，观察健康连接仍能发送数据。

## 5. 协议完成不能被旧连接关闭拖住

iroh-docs 已完成 reconciliation，但旧对端没有发 FIN / 已被杀掉，若 live actor 等待
transport close 才释放交换状态，新一轮仍会碰到 `AlreadySyncing`，直至 QUIC 超时。

当前 live actor 在协议完成时报告 `SyncAcceptFinished`，同一个受 JoinSet 管理的任务
继续排空并关闭流；后到的关闭错误不能再次完成交换、清掉新一轮状态。
公开 `net::handle_connection` 仍保留等待关闭的语义。回归必须真实扣住客户端 FIN，
验证第一次协议完成事件和同 peer 的第二次同步，而不是只 mock “close 慢”。
实现及回归入口见 iroh-docs 补丁清单。

## 6. Blob 下载和应用层等待也会伪装成连接慢

| 坑 | 当前处理 | 升级必须验证 |
|---|---|---|
| 内容已在本地，却先等交互 Device Session 建立 | inbound 先读本地 Blob；缺失内容使用 native Blobs 下载，不额外经过 Device Session | 离线 session 不拖住本地读取；被阻塞的 session 不拖住缺失 Blob 下载 |
| Downloader 默认 1 秒 connect timeout 杀掉尚可完成的握手 | production 和 test bind 都设为 10 秒 | 1.2 秒握手延迟仍成功；这不是固定睡 10 秒 |
| Downloader 默认保留空闲连接 5 秒，重启后复用死亡连接 | `idle_timeout = Duration::ZERO`，保留并发 in-flight 下载共享 | 对端突然退出、换地址后下载新 hash，无旧空闲连接等待 |
| Docs 重启未重新带入持久化 paired peers | `start_sync` 恢复保存的 peers | 不调用手动 sync cycle 也能恢复双向同步 |

关键代码：[SyncNode](../crates/rua-sync/src/endpoint.rs)、
[inbound](../crates/rua-server/src/sync_inbound_apply.rs)、
[peer directory](../crates/rua-device-network/src/peer_directory.rs)。
不要为了“统一连接”把 Docs/Blobs 塞回交互 Device Session；这违背 ADR-0037 的协议边界。

## 7. Android 回调与 UDP 分段不能只在桌面验收

APK `ConnectivityManager` 回调经 200 ms 合并后，通过已认证的
`/api/platform/network-change` 通知原生 Runtime，再调用官方 `Endpoint::network_change()`
和同步恢复入口。Linux Compatibility Runtime 未准备好不能阻止原生网络通知；
网络变化不能擅自唤醒已按宿主策略暂停的 Runtime。

本轮曾猜测 capabilities 回调反复重绑连接，检查 netwatch 的状态去重后撤回了额外过滤。
不要把已撤回的猜测写成根因，也不要因重复回调日志就新增一层连接管理器或网络版本机制。

Android UDP 批量发送还观察到 `sendmsg: I/O error (os error 5)` 并降级 segmentation
offload，而当前 iroh 仍缓存原发送分段上限。当前
[n0_endpoint_builder](../crates/rua-device-network/src/runtime.rs) 在 Mac/Android 均关闭 GSO。
这是当前版本的兼容处理，**不是所有长延迟的唯一解释**。若新版修正发送能力降级传播，
可以重新评估开启，但需要两端真实 UDP 握手后传输、断网和重启验收；编译通过不够。

## 8. 测试环境自身踩过的坑

- 模拟器临时直连提示曾用 `10.0.2.2`；混合 LAN/IPv6 路径时，抓包看到失败握手回复
  从 `127.0.0.1` 发往模拟器，持续重传。改用实际 Mac LAN 地址后继续测试。
  这是特定样本的抓包证据，不能解释所有长尾；结束后恢复原临时提示。
  不把测试地址写入产品配置要求。
- USB 连接不保证系统报告 USB 供电：本轮真机报告交流供电，常亮值 `2` 不生效。
  验收期间 AC+USB 位掩码 `3` 才生效；先记录原值，整个连续验收结束后统一恢复。
  分段脚本提前恢复常亮导致手机再次锁屏，不能反复把这个问题留给用户。
- 锁屏触发 `warm_suspended` 时不能当作前台恢复失败。先检查 `runtime_state` 和屏幕状态；
  需要用户正常解锁，不绕过锁屏。恢复后的真实后台策略另按 ADR-0043 验收。
- 本轮仅退出 Mac app 未杀掉子 Runtime，曾有旧进程残留。部署后核对 endpoint PID、
  实际执行路径、产物 SHA256；不要只看 app 窗口或构建成功。
- 更新实机 APK 要匹配既有签名，用 `install -r` 保留身份和数据。模拟器可用 debug APK；
  实机可把 debug native payload 打包并用原签名更新，不必为普通 E2E 构建 thin-LTO。
- 空白草稿未必物化（ADR-0018）。创建带一次 mock 回复的完整对话，检查列表和内容标记。
  使用临时 mock provider，避免模型费用和外部生成时间污染同步测量。
- 源端生成完成前的时间不是传输耗时。本轮有提交到接收约 7 秒，但源端生成约 6.8 秒，
  内容就绪后的同步只有约 0.37 秒。分别记录源端就绪、接收端完整内容、API ready。
- `up_to_date` 与“列表出现”都不能单独证明内容补齐：检查完整内容、删除传播、双端身份，
  并检查 pending/retryable/dormant/failed/conflicts 全部为零。
- 接受测试期间不能调用 Repair Scan / 手动 cycle 强行补齐。本轮早先孤立测试环境遗留
  dormant 项曾在正式验收前通过 Repair Scan 清理；不能把那次手工恢复算作自动同步通过。
- 临时日志/ADB forward/reverse/mock provider/测试对话/亮屏设置都要清理；勿输出认证 token。

## 9. 升级操作和回归命令

1. 记录当前版本、补丁清单和 APK/Mac 实测基线。查看新版本对应源码与变更，不凭版本号
   假设覆盖修复；每项登记“保留 / 等价上游替代 / 不再适用”以及源码、测试证据。
2. 更新依赖时同时检查 discovery 配置、IPv6 发布、GSO、Downloader 参数和 Android 回调。
   不把整份新 vendor 导入与业务修复混在一起，使新增数万行掩盖本地补丁丢失。
3. 运行下列 focused tests。测试文件位于 `tests/`，部分通过外置 `#[path]` 挂入 `--lib`，
   不能误用同名 `--test`；不能因为过滤器命中了 0 个测试就记为通过。
4. 独立并行构建 `cargo build -p rua-cli` 和 `scripts/package-mobile-apk.sh --debug`，
   先模拟器，再原生真机；保留身份，核对实际进程与安装产物。
5. 重复在线双向写入、双方断网各自写入后恢复、手机 force-stop 后 Mac 写入再打开手机、
   重开后手机反向写入、删除传播。不得依靠手动 cycle/repair。至少对照下面的轮数和时延，
   记录每轮而不只记平均值；5 秒是本轮告警门槛，不是性能目标或允许无理由退化的预算。
6. 记录最终双端状态、身份、清理结果；按证据移除已被上游替代的补丁，并更新本指南和清单。

```bash
cargo test -p rua-device-network --test contract_session discovery_handoff
cargo test -p iroh --lib backpressure_tests
cargo test -p iroh --lib relay_backoff_
cargo test -p iroh --lib path_retry_work_stays_bounded
cargo test -p iroh --lib abandoned_path_does_not_close
cargo test -p iroh-docs --lib completed_sync_does_not_wait_for_the_previous_peers_fin
cargo test -p rua-sync --features test-util --test contract_device_network runtime_sync
cargo test -p rua-server --lib local_content_tests
cargo test -p rua-server --test contract_platform_lifecycle_http network_change_keeps_the_runtime
scripts/tests/clippy-touched.sh
```

Android 回调回归，在 `mobile/mobile-shell` 目录运行：

```bash
./gradlew :app:testDebugUnitTest --tests com.rua.mobile.DnsSyncManagerTest
```

`contract_device_network` 需要 `test-util`；默认 CI 不足以证明已经运行这一组。
上述主机测试不能替代普通应用 UID 的 Android 路由权限和实机网络恢复验收。
维护者本地 `docs/sync-v4-e2e-smoke.md`（被 Git 忽略）覆盖更多数据域，但含 Repair Scan 且超时较宽，
不能拿它代替这里的自动恢复性能验收。

## 10. 本轮验收基线（2026-11-14，Asia/Shanghai）

下表是实际采样结果，不是跨网络、跨机型的时延保证。恢复从发出恢复网络命令计时，
包含重新联网；重开从启动 Activity 计时，包含原生服务启动；在线同步从源端完整内容可见计时。
源端和接收端并发轮询，计时精度约 0.1 秒，API ready 是首次探测成功的时间。

| 场景 | 模拟器 | Mac + USB 真机 |
|---|---|---|
| 6 轮断网恢复，双向完整内容 | 0.92–1.47 秒 | 2.69–3.79 秒 |
| 退出重开后完整补齐 | 6 轮，2.09–2.83 秒 | 4 轮，3.23–3.57 秒 |
| 在线双向同步，源端就绪后 | 0.34 / 0.33 秒 | 0.37 / 0.26 秒 |
| 重开后手机新增，源端就绪后 | 0.21–0.33 秒 | 0.27 / 0.25 / 1.63 / 0.36 秒 |
| 删除传播 | 2.59 秒 | 2.42 秒 |
| 最终状态 | 双端 up_to_date，全部积压为零 | 双端 up_to_date，全部积压为零 |

真机断网恢复逐轮（Mac→手机 / 手机→Mac，秒）：
`2.691/2.916, 3.291/3.082, 3.180/2.961, 3.691/3.644, 3.645/3.661, 3.792/3.572`。
真机重开：`3.565, 3.229, 3.397, 3.357`；首次 API ready 后再用
`0.152, 0.123, 0.181, 0.162` 秒确认内容已补齐，不能把全部约 3 秒称为网络握手耗时。
本轮未再出现之前 7–25 秒的长等待；不能用有限轮数宣称所有网络条件下均无长尾。

验收产物 SHA256：

- 实机 APK：`4b53d8fec797dba7a6ee2334b8e1989242c40d1d062d58343049fcf5733b148a`
- Mac Runtime：`a2a26b8bb4776475cfc37bbbde88ea7259bc1014b292c8f728cd49c61dc7ce5d`

本地原始记录在 `target/iroh-relay-backpressure-e2e/`：
`emulator-android-route.json`、`physical-route-acceptance.json`、
`deployment-verification.json`，以及对应阶段日志和测试日志。
**该目录不入版本控制、可能被清理；长期可用的结论、方法和数值已固化在本文。**
以后升级应保存新的验收记录，不能复用本轮 PASS 或旧产物哈希。
