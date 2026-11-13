# Root 与 Agent 配置归属

核查日期：2026-11-13。这里区分配置的持久化位置与真正消费配置的运行入口；根目录不是所有配置的默认值来源。

## 根目录 `config.json`

| 字段 | 实际作用与读取入口 |
| --- | --- |
| `agents.active_agent`、`agent_order`、`profiles` | 全局 Agent 注册、选中项与排序。`profiles` 保存 workspace 引用，不保存 Agent 的工具或运行参数。 |
| `observability` | 进程日志级别、格式、轮转和 metrics；`rua-server/src/app.rs` 启动时读取。 |
| `task_runtime` | 进程内 Task Runtime 的容量、并发、结果限制和取消期限；`rua-server/src/state.rs` 读取。 |
| `user_timezone` | 全局时区，Cron、时间工具和配置接口消费。 |
| `last_api` | 本机服务地址发现；`rua-config/src/utils.rs` 的读写函数消费。 |
| `security.allow_no_auth_hosts` | 本机 HTTP 认证豁免地址；安全配置服务和认证中间件消费，不跨设备复制。 |
| `security.skill_scanner` | 全局扫描模式、超时和白名单。手动扫描从服务自己的 `SecurityConfigStore` 读取当前值，为本批次传入统一配置；下一次扫描重新读取，无需重启。保留扫描器既有的 `RUA_SKILL_SCAN_MODE` 环境变量覆盖优先级。配置损坏时返回错误，不静默退回默认配置。 |
| 未识别扩展字段 | 原样保留。不能仅因当前代码没有识别它，就删除用户或未来版本的扩展数据。 |

根目录的其它文件也有实际职责：`settings.json` 是全局产品设置，`shared_mcp.json` 是共享 MCP 定义；Provider 凭据和全局模型配置由 Secret Root 下的独立文件管理。这些不是根 `config.json` 中的 Agent defaults。

## Agent Workspace

| 文件及字段 | 实际作用 |
| --- | --- |
| `agent.json.tools` | Agent 显式工具清单；由工具配置服务读取。空清单不会在读取时自动授予工具。新建内置 Agent 时由 seed preset 生成工具清单。 |
| `agent.json.mcp` | Agent MCP 客户端配置；Workspace MCP 初始化读取，并按既有规则引用共享定义。 |
| `agent.json.channels` | Agent 渠道配置；每个渠道自己的 `show_tool_details` 控制工具详情展示。不存在有效的根级同名开关。 |
| `agent.json.acp` | Agent ACP runner 配置；ACP Chat registry 读取。 |
| `agent.json.running`、`default_model` | Agent 默认运行参数与模型。创建对话时按既有规则形成对话配置快照。 |
| `agent.json.language`、`system_prompt_files` | Agent 提示文件语言及文件选择。 |
| `agent.json.heartbeat`、`last_dispatch` | Agent 心跳与最近投递目标。兼容调用未指定 Agent 时使用 `default` Agent，不回退到根配置。心跳正文 `HEARTBEAT.md` 仍按现有实现存放在 Rua Root。 |
| `security.json.tool_guard`、`file_guard` | Agent 默认执行安全策略。新 Chat 物化安全快照；既有 Chat 不随 Agent 默认值自动改变。File Guard 配置接口现在和 Tool Guard 一样读写选中 Agent，并发布 Agent 安全配置变更。 |

## 已退役的根字段

以下字段在根配置中直接丢弃，不备份、不套用到任何 Agent：

- 顶层：`tools`、`channels`、`mcp`、`acp`、`claude_code`、`last_dispatch`、`show_tool_details`。
- `agents` 下：`defaults`、`running`、`language`、`system_prompt_files`、`heartbeat`、`active_hours`。
- `security` 下：`tool_guard`、`file_guard`。

`RootConfig` 的序列化和反序列化都过滤这些已知退役字段，未知扩展仍可往返。读文件不会产生隐式写入；下一次规范化保存或 `rua init` 会清除磁盘中的旧字段。`rua config set` 拒绝重新写入这些 Agent 字段。同步的 Agent 注册表和进程安全记录也过滤对应旧字段。

`rua config init` 先创建正常的内置 Agent，再将心跳、语言和各渠道展示选项写入默认 Agent；不会用根级空配置替换 Agent 工具清单。整个 `rua config` 命令组使用主入口解析后的 Rua Root，遵循 `--rua-root` 高于 `RUA_ROOT` 的优先级。诊断命令检查 Agent 的渠道，MCP 与安全基线检查当前活动 Agent 的实际配置。

## 回归验证

- 根配置新建和旧配置往返：移除已知退役字段，保留未知扩展，不修改 Agent 文件。
- CLI 初始化与 `config set`：Agent 有工具、心跳写入 workspace、废弃键写入失败且不改文件。
- File Guard HTTP：路径和兼容请求头选择正确 Agent，A/B 配置隔离，实际 `security.json` 与返回值一致。
- 进程安全配置写入及同步：不会再次写回根级 Tool/File Guard；本机认证地址保留。
- Doctor：根级旧值与 Agent 值冲突时，采用活动 Agent 的 MCP 和执行安全策略。
- 手动 Skill 扫描：通过配置 API 保存后，工作区和 Skill Pool 的扫描立即采用模式及内容白名单；同名 Skill 内容变化后重新扫描。已有扫描缓存不能绕过关闭模式或白名单变更，损坏配置返回明确错误。
