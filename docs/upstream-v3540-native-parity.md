# 官方 v3.54.0 原生增量接入记录

## 固定基线与范围

- 官方旧基线：`6006cc83944b57c27bf5563dc153d0f0d22f7e66`（v3.53.0）。
- 官方目标：`da866de2c71c34d13a0a370d070eb923f44a2029`（v3.54.0，包含 v3.53.1）。
- NewCPR 本地起点：`11147db71642b5dda4483130bd9e513dbca682b7`，含已提交的 15 种 Codex 套餐筛选。
- 调查时主线：`cd9e756bef3868acc96ab30a432fe1755983a78c`；本轮未查询线上运行提交，不把本地起点作为部署版本。
- 固定两端源码树变化 276 个文件、23 个提交；不是 276 个需要原样覆盖的本地文件。
- 用户批准 E01–E12 的原生更新；排除上游 API Key 账号、CPR 互联、插件专属功能及既有实验分支。下游 Client Key 是访问 NewCPR 的密钥，其批改/统计/预算属于接入范围。
- 合并政策只有 `docs/upstream-integration-checklist.md`；本文记录本次来源、必要适配和证据，不替代现行规范。

## 提交核对

| 官方提交 | 内容 | 处理 |
| --- | --- | --- |
| `d44bc64d1a` | 请求记录布局与错误摘要 | E12，适配本地 Base 组件 |
| `188a6817d1` / `7c2c532b44` | v3.53.1 版本与通道 | 最终版本覆盖中间版本，不导入官方发行源 |
| `f8fa386a7e` | 账号操作具体错误 | E11，共用错误出口 |
| `6420d86aa6` | token_revoked 使 AT 过期 | E02，凭据身份栅栏适配 |
| `9996fae5b6` | 两种预热文案 | E11，不新增后台预热机制 |
| `1427dfc64a` | 国家跟随位置 | E09，采用后续四策略最终实现 |
| `6cfe7ce2fc` | 分组策略与 Key 批改 | E05/E06，互联字段排除 |
| `4b1cae89b2` | 图片模型快捷项 | E11，gpt-image-2 / gpt-image-1.5 |
| `d90b9d129e` | 省略无引用的文本消息 item_ ID | E03，替换冲突的本地 msg_ 改名规则 |
| `1bf948a24f` | Key 统计与预算对齐 | E07，适配本地分析表/永久费用账本 |
| `605b60e58f` | Tailwind 类名整理 | E12，只更新实际本地消费者 |
| `6fc6283f3a` | 成功重放关联 incomplete 请求 | E03，保留本地历史/费用事实 |
| `48a5c9d690` | 目录凭据错误来源 | E02，原生账号链路 |
| `3709ed2484` / `369f17d654` | Guardian、国家与请求头 | E08/E09，共用原生规则；插件和上游 Key 排除 |
| `878d2858e8` | 全量设置新增必填字段 | E10，使用本地独立设置边界 |
| `b02f13c9cc` | 旧管理字段删除、Redis 原子化、凭据解码复用 | E04/E10，同提交公共修复单独接入 |
| `4b5443d2f2` | Guardian 独立设置卡片 | E08，以最终源码为准，不沿用早期发行文案 |
| `0df6c40091` | HTTP/SSE 回退续接要求重放 | E03，HTTP/WS 均在发送前检查 |
| `5b37b574e6` | 合并未发布迁移 | E01，取官方最终 0045 语义，新增本地 0098/0099 |
| `a770419818` / `da866de2c7` | v3.54.0 版本与通道 | 版本 3.54.0，保留 NewCPR 仓库与 published 状态 |

## 实际调用链与错误边界

路径相对仓库根目录。下表中的 OpenAI 路径以 `backend/crates/providers/openai/` 为根；其他后端路径以 `backend/crates/` 为根。

| 项目及官方落点 | 本地入口、helper、调用者与错误出口 | 验证落点 |
| --- | --- | --- |
| E01：官方 migrations/0045、Store settings/group/key | `backend/migrations/0098` 增加分组及请求策略；`0099` 增加独立预算 revision/last_operation_request。`gateway-store/postgres/runtime_settings.rs`、`snapshot.rs`、`account_groups/mod.rs` 将字段送入真实 RuntimeSnapshot | 新建隔离 PG；旧 0097 升级/二次执行；group 默认/省略保留；备份恢复预算新列 |
| E02：credential/quota、catalog | `src/credential/quota/evidence.rs` 只把 401 + token_revoked 归为 AccessTokenRevoked；前台/后台 quota → Store `expire_access_token`。同 token 的 CAS/延期保留提前过期；真正换 token 可更新。`catalog/mod.rs` 保留 CredentialRepositoryError → admin 错误及诊断来源 | quota 矩阵；刷新与撤销并发；目录 conflict/outage 与 cause；PG credential revision 测试 |
| E03：provider continuation、compat、Store recovery | `src/provider/mod.rs` 先做真实 owner 校验，再对 ReplayRequired 返回 NotSent/ClientReplayRequired；Core continuation error → API HTTP 400 或 WS 协议错误。`compat/request.rs` 的原生出站 copy 调官方文本 ID 省略；`postgres/execution.rs` 关联真实 replay_required failed/incomplete | HTTP/SSE 回退后 HTTP/WS 都不发送 delta；真实 HTTP/WS 捕获引用、加密/工具历史和原正文不变；PG recovery 同 Key/会话边界 |
| E04：Redis cooldown/protection、credential decode | `gateway-store/redis/credential_cooldown.rs` WRITE/READ/INVALIDATE/RECOVER 与 `account_error_protection.rs` BEGIN/READ/CLEAR/ADVANCE 复用 MIGRATE Lua；策略发布遍历遗留索引。选择后的安全凭据解码传到账号客户端 | 隔离 Redis 的读取、写入、手动清除竞争；旧 READ Lua 失败/新迁移+READ 通过 |
| E05：group policies、account scope、fast mode | API group create/update/models → Admin → PG → SnapshotAccountGroupFacts → FrozenAccountScope。`allows_model` 要求同一有效绑定组同时允许账号/目标模型，并与账号规则相交；models 目录和 engine 执行均使用。HTTP/WS 子代理门禁消费冻结 scope。`src/provider/mod.rs` 选号后查真实账号 catalog 的档位能力 | Core 共同分组和权限合并；Fast 四档矩阵；Provider 真目录 GET 后捕获 POST；PG 省略字段保留；浏览器 group 创建 |
| E06：client key batch | `gateway-api/admin/client_keys.rs` batch-update → `gateway-admin/use_case/client_keys.rs` → `gateway-store/postgres/client_keys.rs`：1–1000、去重/空补丁拒绝、Key/组校验、画像合并、审计和 revision 事务；发布快照沿用原机制 | PG 失效项整批回滚，限额/画像不清预算，未选 Provider 和种子保留；wire 零值/省略；浏览器只发选中字段 |
| E07：client key statistics/windows | `gateway-api/admin/client_key_usage.rs` 管理鉴权 → Admin observability → `postgres/observability/analytics/key_usage.rs`。facts/15m/1h 按 Key 和部署日期归并，趋势/表共享 asOf。账号搜索走 key 范围 SQL；preview/align 经 AccountsService 缓存 quota、严格日周长度、权限与窗口重读；`client_budget_windows.rs` 锁 Key、比较独立版本、按 charge_events.completed_at 重算 | PG facts/rollup 时区及缺费用；参考账号大池分页；重复提交、过期版本、并发/延迟结算、到期恢复周期；API 日期/鉴权与浏览器 asOf/对齐 |
| E08：protocol Guardian、provider hook、headers、WS | Responses HTTP/WS decode 在合并头之前 capture identity → `gateway-protocol/openai/guardian.rs` → Provider `direct_account_model` → Core `plan_with_direct_account_model` → 真实 native candidate。`reviewer_model` 续接确认；父响应 Key+账号归属和有限缓存。`transport/headers.rs`、OAuth claim、selected-account client、live 辅助调用；WS opening/payload 分离 | 角色冲突/重复/不完整/帧身份；Core native catalog/Excel/映射/owner 回退；真实 HTTP/WS 专用 model、zstd、FedRAMP、role header；原父 Cookie 与并发回归 |
| E09：request_country helper/settings | settings → PG → RuntimeSnapshot/RoutingPlan → RequestAttemptContext → 选号后的原生请求 copy。proxy > enabled global > null；default=null、passthrough 保留、custom=A–Z 两字母 | 设置必填/非法值/数据库往返；真实 HTTP/WS 四策略与缺位置；浏览器四选项 |
| E10：管理 wire 和消费者删除旧字段 | accounts 更新/批改/OAuth start 删除 outboundProxyUrl；Key 删除旧 Provider profile 单字段；dashboard 删除旧 metric_label/value/last_used。前端 types、表单、三语文案与文档同步。全量 settings 新增必填 Guardian/country | API wire unknown field、settings round trip、旧夹具替换；前端类型检查/全量测试 |
| E11：账号模型/预热/错误 | AccountModelAccessField → commonImageModels；QuotaSettings 区分定时预热和额度重置后预热；Provider admin 安全具体错误；admin/plans 模块保留现有 15 项映射 | 浏览器快捷项去重及 15 项菜单滚动；原账号与错误回归 |
| E12：Usage columns/error detail | useUsageRecordColumns → UsageErrorCell → 本地 BasePopover；error 第一左固定、time 第二、actions 末列右固定。摘要先保留本地 WS close/续接原因，再采用官方 reason/message | 1440/390/320 宽度真实浏览器：列序、sticky、长详情界限/滚动、关闭、close code、页面宽度 |

## 适配、修复归因与明确排除

| 差异 | 依据及实际影响 |
| --- | --- |
| 本地 `item_ → msg_` 旧修复删除 | 属于本地遗留冲突，先改名会让官方新 ID 省略规则失效。保留官方正反例并更新旧断言/规范；plaintext reasoning 与工具兼容仍在各自边界生效 |
| quota 撤销与在途 refresh | 新提前过期不推进凭据 revision。本地旧完整账号比较会误判同身份的 refresh 终态为 stale；改用已存在的 revision-fenced 完整凭据加载，保留真正身份变更栅栏。不是宣称官方已经解决所有本地 CAS 差异 |
| settings JSON 投影 | 新列实际为 openai_request_country_json，快照 SELECT 必须显式别名；隔离 PG 回归发现并修正初次接入时的列名错误 |
| 本地身份、出口、Excel/State、Guardian 并发 | 保留既有身份种子、设备和凭据修订、账号维护出口、绑定与租约合同。direct model 仅允许目标模型走原生的账号；Excel 不参与新版原生重放/国家映射，继续自身协议 |
| 分组监控、质量运维和无模型请求 | 既有功能保留。管理员诊断和质量运维固定账号检测继续其明确模型规则豁免；不扩大普通 Key。无模型 Images/Search 按既有入口授权 |
| 预算与分析存储 | 官方表结构无法直接覆盖本地 model_requests、analytics fact/rollup 与永久 charge_events。复用原预算行锁和完成时间语义；不复制另一份账本，备份包含新增版本列 |
| 预算配置并发边界 | align 确认重读参考账号和边界，写事务只对预算 revision 做 CAS；二者之间不是全局配置 CAS。此限制与官方预算版本模型一致，不承诺消除所有并发管理员配置变化 |
| 内部预算兼容类型 | 保留窗口 command 的可选形状与 resume_calendar 字段，公开 API 只提交当前有效的一次性窗口。内部 None 分支只推进版本/审计，未暴露长期外部预算控制功能 |
| 本地设置拆分 | resetCreditOnLowWeeklyQuota 对应既有 AutoResetConfig.onLowWeeklyQuota；UA 仍走独立 openai-user-agent 接口。只扩充实际全量 settings 所需的两个必填字段 |
| UI 组件与翻译 | 使用本地 Base 组件和主题，未整站导入官方 UI v0.13.0。保留仪表盘、表格偏好和统计视图。部分既有风格的新表单提示为中文固定文案，不声称每段提示已完整国际化 |
| 上游 Key、互联、插件 | 不新增上游 API Key 身份、CPR forwarding、插件运行时或相关徽标/管理入口。同提交的原生 Guardian、Redis 和协议公共变化仍接入 |
| 发行和迁移 | 自有仓库/通道；published 为空。只追加 0098/0099，不导入官方未发布迁移中间稿，不改历史冻结字节。`deploy/upgrades/v3.54.0.json` 绑定 v3.53.0 至 v3.54.0 的准确迁移目录，使用验证镜像、按授权选择备份，失败采用人工恢复 |

## 验证记录

2026-11-21 本地验证结果如下。原始失败和成功日志、浏览器截图留在仓库外；不记录真实账号、服务地址或凭据。

| 检查 | 已执行结果与边界 |
| --- | --- |
| 后端编译与静态检查 | Rust 1.99.1；`cargo test --workspace --no-run`、`cargo fmt --all -- --check` 和 `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` 通过。macOS 链接器有大型 unwind section 警告，不是 Linux 发行构建结果 |
| 后端全工作区回归 | 已执行 `cargo test --workspace --no-fail-fast`：App 架构 62、Admin 467、API 668、Core 564、Protocol 145、OpenAI 库 324、xAI 396 等原有回归通过；Store 708 通过/2 失败、OpenAI 集成 1299 通过/2 失败。四个失败分别为新增 PG 夹具缺字段、事件夹具不满足既有约束，以及两个 quota 矩阵复用已经过期的账号 |
| 最终定向复测 | 修正上述夹具后，用最终编译的测试二进制执行 18 项：两个 quota 矩阵、10 项 Client Key API、日/周与 5 小时窗口边界、三个预算窗口事务/分页测试、旧库升级、模型统计聚合，全部通过。没有将初次全量回归记成一次全绿，也没有删除失败断言 |
| PostgreSQL 与 Redis | 使用独立回环实例；新库、0097 升级、预算重试/并发/结算、分析 rollup 与账号身份回归均实际访问数据库。Redis 默认认证包含在全量回归；另关闭 default 用户、使用具名 ACL 用户执行全部 121 项 Redis 测试，121/121 通过，测试后恢复隔离实例认证配置 |
| 前端检查 | `typecheck`、`lint`、生产 `build` 通过；全量测试 1180/1180 通过 |
| 浏览器交互 | `upstream-v3540.mjs` 在 1440/390 宽度通过批改、统计截止时间、参考账号/对齐、分组与国家策略；`usage-v3540.mjs` 在 1440/390/320 通过列固定、详情弹层、长内容与关闭；`account-plan-filters.mjs` 在同三种宽度通过全部 15 项选择、清除与滚动。使用合成接口，检查页面错误与横向溢出 |
| 迁移与隐私 | 冻结清单的 97 个摘要全部匹配，清单相对本地起点只追加 0098/0099。`git diff --check` 通过。使用临时索引形成工作树快照并执行隐私 tree 扫描通过，真实索引未暂存；没有执行尚未产生的提交/发布历史检查 |

全量回归保留一个既有 ignored：`mihomo::tests::official_kernel_relays_reloads_rejects_and_stops`，需要官方内核和隔离 Linux namespace，未计作通过。

修复前后的有效对照包括：同一合成过期 probe 在旧 READ Lua 下丢失，在新原子迁移+READ 下保留；`quota_revocation_during_oauth_refresh_preserves_terminal_and_retry_results` 在旧完整账号比较下失败，改用凭据 revision 栅栏后通过；快照新列投影错误在隔离 PostgreSQL 首次回归中失败，修正别名后通过。文本 ID 的遗留冲突通过固定源码比较定位，再运行官方正反例与真实 HTTP/WS 出站捕获。以上分别证明各自的本地路径，不代表已在真实上游或生产环境复现。

## 未执行范围与状态

以上为实现阶段的本机源码、回环模拟上游、隔离 PostgreSQL/Redis 和合成浏览器验证。没有真实上游账号验证，没有生产 Linux/多副本混跑验证；旧实例继续写 legacy Redis probe 的混跑边界需另行验收。
后续提交、主线合并、版本标签、CI 与服务器部署分别核验实际结果。不能称为全服务字节一致或零缺陷；实际发布状态以对应授权与后续记录为准。
