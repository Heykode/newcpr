# 官方 v3.53.0 原生增量接入记录

## 固定基线

- 官方旧基线：`38446e8a0c1745b59b4c7cc2407f3ff5ff3cd77a`（v3.52.1）。
- 官方目标：`6006cc83944b57c27bf5563dc153d0f0d22f7e66`（v3.53.0，含 v3.52.2）。
- NewCPR 本地起点：`9f85f4d79f50272cca6f22420ded73dc69b24883`。
- 部署源码基线：`d48941a22a241c24a20dcb9e4a65870acb579504`，与本地起点文件树相同。
- 对比固定两端文件树和 17 个提交；本次无 SQL 迁移。NewCPR 发行版本为 v3.53.0，发布通道不变。
- 合并政策仍由 `docs/upstream-integration-checklist.md` 统一维护；本文只记录此次接入证据。

## 提交核对

| 官方提交 | 内容 | 接入结论 |
| --- | --- | --- |
| `8d282dcdf9` | Codex CLI 0.170.0 传输、WS 分帧及依赖 | 接入原生调用链、锁文件及测试 |
| `3d834c9e93` | 官方版本升至 3.52.2 | 不覆盖 NewCPR 发行文件 |
| `d917f948cf` | 官方 3.52.2 发布通道 | 不导入官方发行源 |
| `050214860e` | README、项目支持与宣传 | 不导入宣传素材及官方部署入口 |
| `15ed54d452` | 套餐颜色、换行和表格间距 | 接入现有 BaseTable 与主题变量 |
| `dca9806961` | 按错误来源筛选、错误明细列布局 | 接入前端、API、领域分类、SQL 过滤和计数 |
| `ab1bf4372c` | Desktop 承载的 TUI 按 CLI 版本检查 | 接入公共鉴权识别及原生入口测试 |
| `843e1367bd` | 精简文档、澄清当前行为 | 核对行为说明；维护本地现行规范，不覆盖自有部署说明 |
| `437322ee4b` | 汇总执行准备阶段失败处理 | 接入 coordinator 实际失败出口 |
| `a9e8fa8a44` | 精简选号状态、候选加载及会话准入 | 接入状态与冷却优化，复用已拆出的本地准入模块 |
| `193e83f9f3` | 旧连接测试不能关闭新测试 | 接入当前 POST/SSE 测试生命周期 |
| `8253fbc497` | 管理反馈和请求记录展示具体原因 | 接入分组校验、存储冲突、OpenAI/xAI 诊断 |
| `f4058c1d2e` | 清理过时测试与说明 | 删除已撤销的 TLS 二进制对比辅助；更新适用用例 |
| `0922f3486c` | 早期 WS 参数错误返回 HTTP 400 | 接入 Provider → Core → 原生 Responses 交付 |
| `d62868b692` | WS 快速尝试等待预算可配置 | 接入解析、校验、连接池、实际 SSE 调用者及示例 |
| `e7f8b5a354` | 官方版本升至 3.53.0 | NewCPR 使用相同版本号，保留独立发行通道 |
| `6006cc8394` | 官方 3.53.0 发布通道 | 不导入官方发行源 |

## 实际作用代码与调用边界

以下路径均相对仓库根目录，`openai` Provider 位于 `backend/crates/providers/openai/`。

| 行为 | 本地实际入口、helper 与调用者 | 验证与边界 |
| --- | --- | --- |
| 传输基线 | `src/transport/profile/baseline.rs`、`protocol/websocket.rs`、`websocket/handshake.rs`；Cargo.toml/Cargo.lock 固定官方 fork revision | CLI 0.170.0；保留本地 vendored TLS；握手采样限本机平台 |
| 大消息分帧 | `websocket/pump.rs` 的发送路径：按 1 MiB 拆 text/continuation，保留单条消息的压缩上下文与发送回执；model/coordinator 随消息类型同步 | `tests/transport/websocket/fingerprint.rs` 覆盖大小边界、压缩/未压缩及重组；不增加输入大小限制 |
| WS 等待预算 | `src/config.rs` → `websocket/pool/mod.rs` → `transport/client_sse.rs` | `fast_path_budget_ms` 默认 800，允许 1–15000；只影响可回退 HTTP 的前台等待，后台建连生命周期不变 |
| 客户端识别 | `gateway-api/src/openai/auth.rs::identify_codex_client` → 公共鉴权 → models/Responses HTTP/WS | 使用 UA 中真实客户端标记和 Core 版本；原 Desktop 应用/手机远程识别边界保留 |
| 选号状态 | `credential/selector/mod.rs::AffinitySelectionState`、候选直接引用、`model_eligibility.rs::scheduling_cooldowns` | 互斥枚举代替多 Option；只保存活动冷却，账号与模型冷却取较晚期限 |
| 候选与准入 | `selector/projection.rs` → `select_snapshot` → `admission.rs::scoped_accounts/admit_selected_session` | 本地原先已拆 helper；复核候选加载、CAS、可选缓存偏好失败后轮询仍前进、排队迁移证据；不引入插件账号 |
| 准备失败 | `gateway-core/src/engine/coordinator.rs::handle_prepare_failure` | 官方 account/infrastructure/selection/not_sent 分类进入实际分支；跨 Provider 的旧账号恢复状态由本地 advance_provider_candidate 清除；保留本地换号预算和连接预算类型 |
| HTTP 400 | NewModelRequest endpoint/client_transport → RequestAttemptContext → `provider/execution.rs` 两处调用 `with_atomic_failure_events` → coordinator pending terminal → ResponseCommit → API HTTP 响应 | 仅原生 `/v1/responses` HttpSse、InvalidRequest/400、禁止重试、单个原始 WS error 对象；首字节前 JSON/400，提交后保留流内事件 |
| 来源分类 | `gateway-admin/src/use_case/observability/errors.rs` 的 UsageErrorEvidence/RULES；列表与详情使用同一优先级 | 上游事实优先；不根据客户端状态或文案推测；旧非 JSON 错误不被误当上游 JSON |
| 来源查询 | `gateway-api/src/admin/observability/query.rs` → Admin UsageQuery → Store admin_adapter → `postgres/observability/queries/usage.rs` | 本地保留 `recordStatus`，新增 `errorOrigin`；仅 error 模式允许来源；先过滤后计数分页；汇总/概览/健康/性能/费用入口拒绝列表专属状态与来源；共享 SQL 保存事实解码 |
| 分组反馈 | `gateway-admin/src/model/account_groups.rs::validate_group_fields` → API/use_case/Store；PostgreSQL 唯一约束 → DuplicateName → map_store_error | 空名、长度、控制字符分别提示；重名与其他冲突区别；不更改账号分组监控逻辑 |
| 上游诊断 | OpenAI `provider/failure.rs`；xAI `provider/failure.rs`、`transport/canonical.rs/inference.rs/network.rs` | 凭据、额度、限流、历史、参数、网络等具体原因；保持错误分类、重试边界及原始错误保护 |
| 前端错误明细 | `frontend/src/views/usage/index.vue`、useUsageRecordsTable、useUsageRecordColumns → UsageRecordsTable → UsageErrorCell/UsageTimeCell | 错误来源下拉、固定首列、紧凑时间、独立显示/顺序偏好；保留本地全部视图及详情入口 |
| 基础列设置 | BaseTable/columns.ts、useTableColumns.ts、BaseTableColumnSettings.vue | 可移动列不跨固定分区；只有显式左固定且处于分区末尾的 custom 列可 grow；其他表格默认交互不变 |
| 套餐和连续测试 | AccountPlanBadge、accounts/constants/index、useAccountConnectionTest | 保留当前风格、计数、POST 参数与代次隔离；完成即解除忙态，旧流结束不清掉新流 |

## 必要差异与明确排除

| 项目 | 依据及实际影响 |
| --- | --- |
| 账号身份、绑定代次、Redis 根/子线程归属 | NewCPR 现有身份和会话合同。官方选号重构落到现有 BindingToken/CAS，不用官方较薄的账号 ID 绑定覆盖历史归属 |
| Excel 原生路由、诊断/质量恢复、账号保护 | 既有已批准功能；仅原生 Codex 账号参与模型冷却。诊断与已确认的质量恢复可读取对应停用账号，不放宽普通调度 |
| 凭据维护出口、定时和批量安排入池 | 既有已批准功能。本轮没有更改维护、代理和入池链路，不新增直连兜底 |
| 选号耗尽后保留真实选号错误 | 已记录于 `docs/upstream-v3500-native-parity.md`：先前账号的 401 不冒充号池错误；本轮提取函数时保留，未新增限制 |
| xAI 既有输入兼容 | 本地起点已经将 legacy local_shell 历史转换为 shell 格式、response_format 转为 text.format、扁平 reasoning effort 归一化。固定官方两端的相应生产转换代码未改变；本次新增的拒绝这些格式的官方断言不适用于这些既有消费者，因此没有据此删除已支持的转换。保留现有正例及歧义输入拒绝测试，本轮不宣称这部分与官方完全一致。 |
| 本地 SQL/API 合同 | 使用 `model_requests.raw_upstream_error`、`recordStatus` 与当前 Admin adapter；不导入官方不同表名/字段名或新增不必要迁移 |
| 本地 UI 与连接测试 | 使用现有 Base 组件、POST/SSE 和 generation fence；没有官方的 GET EventSource/插件请求架构。Axios 错误归一化已提供 HTTP 分类回退；账号编辑从已加载行读取，不存在官方 configurationRequest 加载器 |
| 旧 max_connecting 配置 | 仅忽略解析旧字段以兼容现有配置，不恢复已撤销的全局建连人数限制。ws_pool 可单独覆盖任意合法字段；本地 wire_profile 的必需合同不变 |
| 错误表与汇总 | 来源/记录状态是列表专属，图表仅沿用公共筛选。保留本地使用统计、请求健康、热点诊断、独立卡片和手机结构 |
| 上游 API Key、CPR 互联、插件 | 按用户既有边界不引入相关账号、徽标、插件运行时、插件测试和文档；同文件原生公共改动单独接入 |
| BPS/Excel 双凭据实验 | 继续排除；不合入无关旧 PR 或实验分支 |
| 官方发布与文档改写 | 版本、发布通道、捐赠资源、安装入口和插件说明不覆盖 NewCPR。纯注释、测试改名及不存在的官方模块不为凑文件一致新增空壳 |
| 测试更新 | 保留官方正反例和本地有实际用途的回归。清理旧 connection-test run/minimum-duration 专属测试模拟及官方已删除的 ignored TLS 二进制对比 helper；不添加一次性脚本到仓库 |

## 验证记录

- 前端全量既有测试首轮 1177 项，1172 通过；5 项涉及旧测试辅助/存储命名空间、缺少官方翻译和固定列伸展边界。处理后定向重跑 33 项全部通过，包含新增连续测试与顺序隔离用例。
- 前端 Vue 类型检查、修改文件 ESLint 和 Vite 生产构建通过。
- 真实浏览器以合成记录验证套餐换行、错误文本、列固定、横向滚动、排序、隐藏、恢复默认和成功/错误视图隔离；实际窄屏宽度 480 CSS px。预览脚本只在仓库外。
- `cargo check --workspace --all-targets --locked` 通过；其后新增测试由后续编译和 Clippy 检查。
- 后端主套件：App 架构 62、Admin 9 个库测试及 465 个集成测试、API 2 个库测试及 665 个集成测试、Core 7 个库测试及 561 个集成测试通过。
- OpenAI 324 个库测试通过；集成首轮 1291 通过、1 个配置夹具失败。按本地必需 wire_profile 合同构造完整配置后，配置模块 13 项重测通过；未修改业务规则来满足夹具。
- xAI 首轮 395 通过、1 项新移入的拒绝断言不适用于上述既有兼容转换；移除不适用的子例后，歧义输入测试定向通过。沿用已经通过的正向转换用例，没有重跑无关全套。
- 新增 Desktop 承载 TUI 的原生 models/Responses HTTP/真实回环 WS 门禁测试通过；汇总参数边界用例已验证修复前失败、修复后通过，API 观测模块 45 项通过。
- 隔离 PostgreSQL 观测 52 项、分组 14 项通过；隔离 Redis 会话归属/绑定 16 项通过。
- Workspace 全目标/全特性 Clippy 通过；最后新增测试后的 API/Store/xAI 全目标、全特性定向 Clippy 也通过；cargo fmt --check 通过。
- 13 条既有合成 URL/DNS 测试夹具经人工重新检查，保留历史审核记录并补充当前文件精确哈希；未修改隐私扫描器或放宽规则。工作树临时快照的隐私检查通过；本轮没有暂存/发布，因此不将其记作 staged 或 publication history 检查。
- 数据库首次运行使用了 SQL_ASCII 测试库、无密码 URL，导致中文名称夹具与备份用例失败；改用独立 UTF8 测试库及完整测试 URL 后，观测模块 52 项全部通过。没有为测试修改业务约束。

## 验证范围与发布状态

本轮仅本地源码、模拟上游、回环网络与隔离 PostgreSQL/Redis 验证。未访问真实上游账号，
未在 OVH 或生产 Linux 执行新版本，不声称真实上游已验证、全服务字节一致或零缺陷。
以上为合入阶段的验证记录。发布说明见 `release/notes.md`；镜像来源、实际部署版本及健康检查以发布时的记录为准。
