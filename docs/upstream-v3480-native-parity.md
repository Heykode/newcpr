# 官方 v3.48.0 原生合入记录

本记录遵循 [共用合并规则](upstream-integration-checklist.md)，仅记录本次固定版本的源码、
接入差异和验证边界，不替代当前业务规范。

## 固定基线与范围

- 上次官方：`e1f84c210b5bc7423cec011a65e0fc2310e24aad`。
- 本次官方：`975b73b33b6d247fbe08a9a23e88066467e71d7a`，v3.48.0。
- 本地基础：`9a8f77f1e9141f8ab20b09fa24dc7d7715eed86d`，目标 `Heykode/newcpr`。
- 本轮未读取或修改生产服务，不能以本地基线或版本标签代替已部署源码。
- 不包含此前暂缓的 analytics worker、窄列统计投影及 work_mem 接入，也不恢复插件、
  上游 API Key 账号、专属 CPR 转发或已取消的 PRISM。

| 官方提交 | 更新 | 本地处理 |
| --- | --- | --- |
| `f85d80d` | WS 背压时取消与资源收尾 | 移植 delivery 与终态先归还/销毁的分支，接入本地中断及出口租约 |
| `f61995e` | 请求保留期放开业务上限、防日期溢出 | 接到实际分类清理器、运行设置和直接 SQL 统计，新增迁移 0091 |
| `72215fe` | 管理端 PWA 安装 | 合入 manifest、HTML 和使用说明，名称使用 NewCPR |
| `8f40bfd` | Responses 安装头及关联标识 | 移除安装头；合入 v7/窗口规则；删除强制 cache=thread 的旧适配 |
| `d99ecf9` | 后台模型请求采用全局画像、刷新离线基线 | 接完整数据库读取、目录发现/导出/刷新、预热及诊断快照 |
| `8681d68` | 锁文件依赖更新 | rustix 1.1.6；Quinn 修复在本地基线已存在，无重复降升版本 |
| `d787195`、`975b73b` | 官方版本号、发布说明和通道推进 | 不复制官方发行元数据；本轮没有发布授权，不变更本仓库版本/通道 |

## 作用代码和调用链

表中 OpenAI 路径相对于 `backend/crates/providers/openai/`，Store 路径相对于
`backend/crates/gateway-store/`，Core/Admin/API 分别指其同名 crate。
官方本次 62 个变更文件按以下落点全部核对，路径合并处明确列出。

| 官方落点 | 本地实际落点与接入 |
| --- | --- |
| OpenAI `src/transport/websocket/exchange/stream.rs` | 同路径；普通事件、协议错误和终态均通过 cancellation-aware delivery；终态释放/归池及 continuation 保存先于可能阻塞的发送 |
| OpenAI `src/transport/headers.rs` | 同路径删除共用安装头；受管头过滤仍拦截下游同名头，模型目录同样不发送 |
| OpenAI `src/transport/client_json.rs`、`client_live.rs` | 同路径显式补回独立端点安装头，避免共用头删除误伤 Images/Search/Live |
| OpenAI `src/transport/correlation.rs` | 同路径合入真实 v7 判定、时间保留、普通 v4 及规范窗口识别；本地 `qx_application.rs` 实际投影入口同步接入 |
| Core `src/provider_ports/runtime_policy.rs` | 本地 `src/provider_ports/mod.rs` 增加 `load_request_profile`，由 Store 实现，不增加第二套配置源 |
| Store `src/postgres/runtime_settings.rs` | 同路径从已有 `provider_outbound_user_agents` 读取 `configuration_json`；官方用其 runtime settings 画像字段，本地没有该字段 |
| Core `src/routing/snapshot.rs` | 同路径加入全局画像并传给 `plan_diagnostic`；Store `src/postgres/snapshot.rs` 从同一一致性快照读取配置，真实诊断调用可达 |
| OpenAI `src/transport/profile/identity.rs` | 同路径解析并冻结全局画像；读取、解析失败返回 Profile 错误，不吞错回退 |
| OpenAI `src/credential/catalog/mod.rs`、`refresh.rs`、`src/lib.rs` | 注入 runtime policy，整轮发现、单账号刷新、管理员导出均调用 `discovery_client`，版本参数和请求头共用冻结画像 |
| OpenAI `src/admin/mod.rs` | 本地 `src/admin.rs` 接目录及预热 Profile 错误出口 |
| OpenAI `src/credential/quota/mod.rs`、`warmup.rs` | 本地预热执行在 `quota/mod.rs`；每轮冻结全局画像后使用实际账号出口，额度查询自身画像不变 |
| OpenAI `src/transport/client_catalog.rs` | 本地在 `client_sse.rs::fetch_models_with_context`；显式版本优先，否则使用调用者冻结的画像，官方此文件只修改说明 |
| OpenAI `src/transport/profile/baseline.rs`、`cli_release.rs`、`mod.rs`、`platform_release/mod.rs`、`selection.rs` | 官方基线和消费者全部接入；本地启动默认值在 `src/config.rs`，一起改用 baseline |
| Admin `src/model/retention.rs` | 本地实际校验在 `src/model/log_cleanup.rs` 和 `src/use_case/settings.rs`；请求项支持全部正 u32，显式 0 全清单独保留 |
| Store `src/postgres/retention.rs` | 兼容 repository 同步 checked cutoff 和最早记录保护；实际 owner 是 `src/postgres/log_cleanup/{mod,runner}.rs`，同样接入，不能仅修改兼容路径 |
| Store `src/postgres/observability/analytics/accounts.rs` | 本地无已启用 analytics worker；规则接到 `provider_accounts/admin_queries.rs` 的账号排序及 `account_groups/mod.rs` 的分组金额 SQL，避免 int32 和日期溢出，只统计实际历史 |
| 官方迁移 `0042_usage_retention_days.sql`、`.frozen-sha256` | 新增本地 `0091_usage_retention_days.sql` 及哈希；旧迁移字节不动，保留已有配置和历史数据 |
| 前端 `RequestHistoryCard.vue`、`useSettingsFormRules.ts` | 本地入口在 `components/cleanup/{CleanupRetentionInput,SettingsCleanupSection}.vue` 与 `cleanup.ts`；取消请求项 3650 天上限，日期越界预览不会删记录，仅加宽大数输入框 |
| 前端 `index.html`、`public/manifest.webmanifest`、三语言 `settings.json` | manifest 原生接入，品牌名称 NewCPR；三语言旧 1–31 天文案同步替换 |
| `deploy/README.md`、`docs/api.md`、`docs/architecture.md` | 安装条件、身份规则、后台画像、保留期及架构说明同步；本地 `docs/log-cleanup.md`、`docs/qx-compatible-profile.md` 和四份相关 spec 替换冲突条款 |
| `backend/Cargo.lock` | 仅 rustix 版本和 checksum 更新；不重新生成整个依赖图 |
| `release/notes.md`、`release/version.yaml` | 明确排除官方发行源、版本和通道元数据，留待本仓库获授权发布 |

## 必要适配与行为变化

1. **WS 本地收尾接入。** 官方 delivery 用于三个发送出口；保留本地 ResponseControl 主动
   中断、多 JSON 文档解析、终态后的尾帧丢弃及 opening metadata 恢复。账号 session-proxy
   租约在 socket 收尾之后、等待下游消费之前释放，避免背压继续占住出口。
2. **旧缓存适配已删除。** 显式 `prompt_cache_key` 独立映射，同原值在同账号/Key 范围内
   保持相等；未提供就不生成。不再把所有缓存键改成 thread，不再把正文 session/thread
   别名强制覆盖成 header seed。用户数据、轮次 ID、响应 ID、工具 ID 不受此映射改写。
3. **保留本地下游 Key 隔离和恢复状态。** 哈希仍使用既有带长度前缀的 Key、安装身份、账号
   组合，非官方仅按安装身份的字节算法。保留已冻结 header seed、缺省 `thread:0` 窗口和
   `correlation_originals` 防止恢复请求重复映射；普通底层调用缺少 Provider 身份授权时不
   补造账号映射。这些用于现有 Excel/State/精确续接的归属连续性，不应宣称输出字节相同。
   统一映射域和修正 v7 形状可能使升级后关联 ID、缓存键变化，首次请求可能缓存冷启动。
4. **画像沿本地持久配置接入。** 完整预设从 `mode=configuration` 读取；旧 default/custom/
   custom-auto-version 沿已发布画像快照。后台任务不引用任意 Client Key 的配置，保留
   账号凭据和出口。诊断取同次运行快照的全局配置；客户端 Key 请求仍按其既有覆盖规则。
5. **清理与统计合同衔接。** 官方仅删除保留期上限；本地此前将短期限存于分类清理配置。
   0091 仅把已保存 1–30 天请求期限同步到 usage/ops 并推进配置 revision，0 和其他期限
   不改写。迁移不执行删除。请求清理 0 全清、运行中请求保护和独立累计金额继续保留。
   请求项范围 `0..=u32::MAX`，运行设置请求期限范围 `1..=u32::MAX`；分类文件/审计仍
   使用已有 3650 天上限，详细采集仍是 30 天。这些不是请求历史上限。
6. **PWA 只接安装能力。** 沿用登录和联网请求，不添加 Service Worker、业务缓存或离线
   功能。超长保留期输入框加宽是实际浏览器发现的显示兼容问题，不调整其他账号列表布局。

离线基线采用本次官方固定源码：CLI `0.169.0`；macOS/Linux Core
`0.169.0-alpha.11.1`、Desktop `26.1111.21308` build `27194`；Windows Core
`0.167.0-alpha.4.1`、Desktop `26.1106.51428` build `26605`。这些是离线回退值，
不是本轮独立核验的实时最新版本；固定版本、自定义 UA 和持久账号设备不被覆盖。

## 测试接入和验证证据

| 官方测试文件 | 本地落点 |
| --- | --- |
| OpenAI `tests/transport/websocket/cleanup.rs` | 移植两项背压、终态资源回归并接入 `websocket/mod.rs` |
| OpenAI `tests/provider/contract/correlation.rs` | 移植四项 Provider 合同回归，覆盖 HTTP/WS、换号、v7、窗口、缓存及独立端点 |
| OpenAI `tests/compat/headers.rs`、`tests/transport/headers.rs` | 本地统一在 `tests/transport/headers.rs` 核对 HTTP/WS 最终头；移除旧安装头预期 |
| OpenAI `tests/transport/websocket/fingerprint.rs` | 保留本地实际底层投影路径，握手顺序、压缩及映射规则在真实 loopback socket 验证 |
| OpenAI `tests/credential/catalog/mod.rs`、`contract/mod.rs`、`oauth.rs`、`quota/warmup.rs` | 接新构造参数、当前全局画像和端点安装身份规则；本地 `native_catalog_local.rs` 核对实际出口 |
| OpenAI `tests/provider/contract/mod.rs`、`cookies.rs`、`tests/support.rs` | 本地共享 Provider 构造器及 support 接 runtime policy；cookie 测试沿共享构造器，不复制官方拆分目录 |
| 本地 Linux `tests/provider/live.rs` | 将同原值 session/thread 必须不同的旧断言改为相等；继续验证 bootstrap、sideband 和 hangup 的画像冻结、关联连续性、凭据及代理刷新 |
| OpenAI `tests/transport/profile/{cli_release,selection}.rs`、`platform_release/{mod,artifact}.rs` | 更新基线和官方平台正反例；CLI 后继版本测试从当前基线构造，避免过时固定版本倒逼生产回退 |
| Admin `tests/model/retention.rs`、API `tests/admin/settings.rs` | 本地清理模型测试和 API 正 u32 边界；wire u64 解析与业务 u32 验证分别断言 |
| Core `tests/engine/execution/mod.rs` | 本地 `tests/engine/execution.rs` 验证真实 diagnostic plan 携带全局画像 |
| Store `tests/postgres/{retention,runtime_settings,account_groups}.rs`、`provider_accounts/usage_projection.rs` | 对应本地 retention/runtime_settings、account_groups/mod 和 provider_accounts/mod；额外覆盖实际分类清理、全局画像 DB 回读及快照 |

已执行的针对性证据：

- Provider 范围回归与修正旧断言后的定点复验合计覆盖 1192 项非忽略用例；一个 socket
  keepalive 用例曾出现 BrokenPipe，独立复验通过，未修改生产代码或弱化断言掩盖它。
- Store 相关 187 项通过，使用真实隔离 PostgreSQL/Redis 和 `CI=true`，未静默跳过。
  覆盖短期/长保留期/u32 最大值、迁移整行保留、删除保护、金额、账号排序、分组和画像回读。
- App 62 项含架构检查通过；初次 3 项因本机隔离数据库未启动失败，启动后复验通过。
  Core 诊断 1 项、Admin 3 项通过。API 相关 41 项包含修正 wire u64 测试预期后的复验。
- 对同一官方回归临时替换成旧生产实现，5 项失败：账号 correlation、显式缓存共享/缺失、
  窗口别名、背压取消/停机、终态交付前释放/复用；恢复新实现后这 5 项全部通过。
- Workspace 测试编译、Clippy all-targets、前端 ESLint、vue-tsc、Vite build 和清理单元测试
  已执行。浏览器验证保存 1/3651/u32 最大值、全清 0、取消全清恢复、大数预览及手机显示；
  Chrome manifest 无解析错误，installability errors 为空。

最终工作树通过完整候选树隐私检查、迁移哈希及历史 SQL 字节检查；API 接口往返复验和
Clippy 通过。本记录不把旧失败日志当成当前通过证据。原始测试日志、
浏览器截图和一次性对照脚本均保存在仓库外，未添加新的 CI、永久临时脚本或通用门禁。

未执行真实上游请求、生产吞吐压测、真实账号风控验证或跨平台发布。PWA 验证使用本机构建
页面和合成管理 API，不代表已修改线上服务。实现验收与提交、PR 合并、版本发布和部署
分别记录，提交及合并状态以对应 PR 为准；后续发布需按实际授权和新增迁移准备部署计划。
