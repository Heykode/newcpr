# 官方 v3.45.1 原生功能合并记录

## 基线与状态

- 日期：2026-11-11。
- 仓库：`Heykode/newcpr`；本地起点 `51d845bdf8d989b7124cd0abbc4cccbd547dc261`。
- 已合入官方基线：v3.44.0 `cf7a9bd501dcacfcf3b2f722e8f681db5af149d1`。
- 本次固定目标：v3.45.1 `1b24029a9700e4e554c3f4764d1201ff16607f92`。
- 对比范围：25 个提交，`git diff --name-status` 为 346 条（两条重命名各记一条）。末尾逐条登记。
- 本记录形成时仅完成本地接入与隔离验证；后续累计发布纳入 v3.45.2，发布说明与迁移边界见 `release/notes.md`。
- 公共规则继续使用 [合并核对清单](upstream-integration-checklist.md)，本文件只记录固定版本证据。

## 功能、调用链与保留差异

路径以仓库根为起点；“接入”包含官方原函数移植和落到既有模块的必要适配，不表示整文件字节相同。
附录的组名指向下面的实际调用链；混合文件只排除明确的产品分支。

| 组名 | 官方来源 / 新行为 | 本地真实落点与处理 |
| --- | --- | --- |
| 协议 | `dd6566512`：Messages/Responses 双向映射、工具/推理/usage、SSE accumulator、请求事实 | `gateway-protocol/src/anthropic/`、`src/request.rs` 及官方对应测试移植；反向协议工具保留，但未据此开放 Messages API-key 上游产品 |
| Messages | `dd6566512`：Messages 入口、count_tokens、鉴权、模型格式；OAuth 默认 service tier 修复 | `gateway-api/src/openai/{messages,messages_response,models,auth}.rs`、`responses/{http,keepalive,request}.rs` → Core `Operation`/Coordinator → `providers/openai/src/provider/{execution,messages,count_tokens}.rs`；保留本地共享 Responses/Chat 执行器结构，官方 generation 抽取映射到既有模块；Bearer 优先、CORS 新头、模型分页/详情均接入。OAuth 出站省略 auto/default，保留 priority/flex |
| 额度 | `e3befd862`：通用窗口限额及 quota_paused | Core `account/{quota_limits,model,store}` → Store `provider_accounts/{admin_adapter,runtime,core_adapter}` → Admin accounts/Provider pause 准备 → API presenter/wire → 前端 `AccountQuotaPanel/QuotaLimits.vue`。即时、等待重扫、固定目标、提升前及租约后检查；xAI 只补共享字段，保持不支持设置这一能力 |
| 目录 | `758bd24b3`、`d3143406d`：按账号合并、持久化、启动恢复、后台发布 | `providers/openai/src/credential/catalog/{mod,refresh}.rs` → Store `provider_accounts/model_catalogs.rs`；selector/admission 使用有效账号证据。后台并发 32、10ms 启动间隔、128 批量发布；客户端只读本地快照。源码与测试均采用官方 catalog/mod.rs 目录结构 |
| Cron | `4bf120802`、`620db1b87`：明确 idle 检查、五段 Cron | Admin settings → API settings → Store runtime_settings → Provider quota/mod.rs 的现有预热 worker → 前端 AccountWarmupCard/useSettingsForm。真实 0% 与未知用量区分；迁移关闭旧预热，需重新配置 Cron 并启用。官方 warmup.rs 的运行逻辑映射到 quota/mod.rs |
| 周重置 | `e079ba6e4`：周剩余额度不足时用卡，消费后等待恢复 | 复用每账号 `reset_credits/automatic.rs` 的已有 worker、事务 claim 与去重账本，UI 为 `AccountQuotaPanel/AutoReset.vue`。默认关闭；只认有效账号级七天窗口，未填阈值按 0，满额不消费；确认 reset/already_redeemed 后才建立恢复等待。没有复制官方全局设置产品或第二个消费 worker |
| 预算 | `de58b436b` 混合提交中的原生 reset 修复 | `gateway-store/src/postgres/client_budgets.rs`：upsert 记录重置边界，所选周期 start=end=reset_at、used=0；下一次使用按自然周期开启，保留原锁、审计、未选周期和异步结算。官方窗口类型/getter 变化中的原生展示过滤由本地既有 Store 查询完成；插件显式窗口/revision/charge_history_since 排除 |
| 日志 | `c6350fd23`、`114baf669`、`a2972d645`：incomplete 失败、来源、状态及尝试缺口 | Admin `observability/errors.rs` → Store `observability/{queries/usage,mapping,admin_adapter}` → API observability → 前端 usage 的 records/errors、UsageRecordsTable/ErrorDetail/RecordDetail。本地平铺字段承载官方 JSON 事实；`has_upstream_error` 与详情保持一致；downstreamCommitted 可空；不虚构 attemptsComplete |
| UI | `63db306ba`、`628533216` 及额度动作按钮 | 账号并发弹窗按行增长并滚动；`api-keys/utils/codexConfig.ts` 加模型发现。官方 QuotaActionButton 由现有 BaseIconButton 承载，避免多余基础组件；保留原多分组布局和展开交互 |
| 翻译 | accounts/dashboard/keys/settings/usage 三语字段 | 接入已启用功能的对应字段；本地重置卡和预热页面沿用现有组件/中文文案。清理旧预热时间字段引用，不为排除产品开放入口 |
| 公共接口 | 共享 port、worker、CORS、测试注册及结构调整 | 额度/目录/协议新增字段贯穿现有 trait、Admin/Store adapter 和 worker；本地 API 只增原生入口及头。官方 diagnostic/health/support 的 peer 参数或模块重排不构成本地功能，沿用本地同范围测试；测试 mock 普通刷新保留真实 Store 的 quota 行为 |
| 迁移 | `c72136c68` 汇总官方 0040/0041 | 不重写本地历史迁移；新增 `0088_account_scheduling_limits_and_cron.sql`、`0089_account_model_catalogs.sql`、`0090_auto_reset_weekly_recovery.sql`。frozen hash 仅追加。剔除 0040 中插件显式预算和 peer 身份；保留本地周重置恢复标记 |
| 依赖 | `092789f5f`、`bec4c2d2a`、`725de6ead` | SQLx 固定到 `10462fa4414c6c7f1ed1d39bb31678e3b726878c`，SQLite 0.39.1；增加官方 tokenizer/Cron 所需依赖。现有 OpenSSL、crypto-bigint 已高于此次修复版本，不降级；不是安全漏洞全量扫描结论 |
| 传输测试 | `b5365b5c3`、`13edec0b0` | Host mock drop 移至 spawn_blocking；移植两项官方 WS 测试；保留本地已具备的 native TLS 和本地证书负例，不覆盖成较弱测试 |
| 文档 | `a4e60c3d5` 和功能文档 | 更新 README、API、architecture 及受影响的 Trellis specs。旧三账号取样、客户端版本远端缓存、预算保留窗口结束时间等条款被替换；旧 426 文档按实际已有 400 合同纠正 |
| Peer排除 | `2fc347508`：分组共享、X-CPR-Route 环路、连接路径隔离 | 用户已排除 peer_link；无本地节点转发产品，不增加 peer 身份、协议头、共享 scope 或 SDK 转发规则。混合文件的原生逻辑由其他组单独接入 |
| 插件排除 | 插件 runtime/SDK、显式预算 API、扩展中间件声明 | 保持既有排除边界；不引入无消费者的运行时或通用 Provider SDK 路由。原生 Messages/count_tokens 与预算 reset 公共修复已独立接入，不因插件文件而丢弃 |
| API账号排除 | Messages/Responses 上游 API-key 账号配置、transport、定价、API 模型分页 | 本地原生 OAuth 路径不使用这些账号类型，故不引入其 credentials/client_messages/headers/overloaded_error 和配置 UI。OAuth 同文件的 service tier、目录及额度变化分别接入；账号编辑 connection 插槽重排不覆盖本地身份/Excel UI |
| 原状 | `.gitignore`、官方发布/部署/开发流程、版本号 | 保留 NewCPR 现有忽略项、发行身份和制品复用流程；不复制官方版本文件或触发发布。客户端路径说明已落到 README/API，官方部署功能未据此重新开放 |

## 必要适配的实际影响

1. **额度观察完整性**：保留 PR #260 的请求开始时刻、捕获观察时刻、credential revision、Store 原子条件及两处成功回写防护；官方暂停判断与这些条件组合。缺失窗口不能擦除已有触顶证据，迟到旧请求不能恢复新状态。普通 RT 刷新保留快照与身份；人工重登按现有规则处理。
2. **严格模式**：严格子线程仍跟随根账号，额度暂停不授权它自行改绑；固定账号/真实续接所有者优先。优先/宽松按官方各自规则重选；普通业务不能借本地透支开关绕过 quota_paused。诊断质量重测保留官方豁免范围。
3. **模型目录和 Excel**：原生 scope 按账号/revision/套餐；Excel 额外按其模型配置构成 adapter scope。混合目录以原文 native IDs 校验原生权限，不因含 Excel 条目跳过筛选。混合 hash 含 adapter，因此不用原生 SQL hash 遗漏下推，改由已加载的本地证据检查；纯原生仍使用官方批量下推。启动适配 Excel 多一次本地账号读取，不同步请求上游。
4. **源协议计量与目标交付**：Coordinator 先保存 Responses 源事实，再转换为 Messages；一源多事件在最后一项完成。保留既有 best-effort 观测写入合同，不能将日志写失败混同为结算失败。count_tokens 的 Started/Completed 与结果 wire 同步交付，避免本地流校验器误判 EOF。
5. **周重置卡**：复用已有每账号 worker 和数据库去重。成功或已消费确认后建立等待，未知结果保留原去重保护；同身份参数编辑不能再次消费，身份变化后重新保存清旧等待。与官方全局设置的差异是配置位置和现有事务载体，周额度触发/恢复边界按官方接入。
6. **数据库与 UI**：使用本地平铺观测列，不增加第二套观测 JSON；官方测试改用真实本地 Admin adapter 和合法 SQL fixture。多分组继续保留自适应换行及全部分组弹层，不套用官方单图标尺寸。

## 验证证据

所有结果来自本机隔离 PostgreSQL/Redis、合成协议和浏览器；没有访问生产或执行真实账号推理、用卡、重登。
已有合格证据按源码未变范围复用，不重复全量矩阵；旧失败记录不会当作通过。

| 范围 | 证据与结果 |
| --- | --- |
| Rust 编译规范 | 全 workspace、all-targets Clippy `-D warnings` 通过；Rust fmt 通过 |
| 公共协议 | gateway-protocol 135 项通过，含官方 Anthropic 请求、工具、SSE、usage 及 request facts |
| Core/Admin | Core 原 543 项通过，后补真实 Coordinator 转换/观测失败用例通过；Admin 最终 415 项通过，含来源判断和周额度边界 |
| Store | 既有全量 655 集成 +12 单元通过；新增 quota merge CAS、周重置身份/消费 fence、Cron 升级分别通过；补验错误来源列表/详情 2 项、timestamps 8 项通过 |
| Provider/API | 原全量 Provider 1243 项与 API 649 项通过；当轮 Messages 失败已修并以 Provider 4 项 +相关单元 7 项、API 13 项复验通过。最终模型列表 19 项、默认 service tier 1 项、credential 321 项通过 |
| 目录/调度 | 已覆盖官方持久恢复/万账号、晚到 fence、候选替补、未知套餐自己的目录、混合 Excel、等待提升前重查，以及 Strict/Preferred/Relaxed 的暂停边界；依据对应 Provider/Store 实际测试，不声称穷尽真实流量组合 |
| 共享消费者 | xAI 396 项、Host update discovery 12 项通过；组合根架构 14 项和其余 48 项通过（修正官方依赖边及新模块布局后定向复验） |
| 前端 | 类型检查、完整 ESLint、Vite build 通过。既有 Node 1158 项中 1157 首轮通过，唯一 i18n 失败修复后该文件 2 项通过 |
| 浏览器 | 1440/390px 合成 API：额度上限保存重读、Cron 输入、周额度设置保存、30 条并发记录滚动、错误来源与不完整提示通过；真实账号页面 3 账号/5 分组无重叠、可打开全部分组、无页面错误/横向溢出 |
| 验证局限 | 未做真实上游多模态/工具端到端、生产流量回放、Linux 制品构建、生产迁移/部署或本轮 cargo audit；这些不能由本地通过结果代替 |

临时夹具、浏览器脚本、截图和原始日志留在仓库外；不新增永久验证框架。隐私检查针对最终工作树快照登记，提交时仍须扫描实际 index 和发布历史。

关键回归定位（保留官方正反例和实际适配边界）：

- `messages_use_selected_oauth_account_and_translate_tools_and_usage`、`messages_count_tokens_is_local_and_cancellation_releases_selection`：真实 Provider 选号与转换/本地计数。
- `messages_commit_and_settlement_failures_never_emit_success_terminal`、`translated_messages_preserve_source_usage_when_observation_writes_fail`：交付失败、源计量与日志写入边界。
- `quota_pause_preserves_strict_child_owner_and_other_modes_can_use_backup`、`quota_merge_can_only_replace_the_snapshot_it_read`：亲和模式与 Store 快照竞争。
- `ten_thousand_restored_accounts_serve_concurrent_native_reads_while_upstream_stalls`、`unknown_plan_uses_its_own_catalog_entitlements`、`failed_native_catalog_cannot_turn_excel_synthetic_models_into_refusal_evidence`：目录恢复、账号归属和 Excel 边界。
- `weekly_reset_fence_requires_confirmed_consumption_and_fresh_recovery`、`weekly_recovery_fence_survives_policy_edits_but_not_identity_changes`：用卡确认与身份切换。
- `cron_upgrade_disables_old_warmup_without_changing_other_settings`：升级默认值和重新启用要求。
- `usage_error_origins_agree_for_saved_errors_without_failure_status`、`usage_error_origins_preserve_saved_failure_and_connection_boundaries`：真实 Store 列表和详情来源一致。
- `manual_reset_restarts_selected_windows_only_after_new_usage`、`manual_reset_excludes_old_completions_but_counts_inflight_requests_after_reset`、`manual_reset_leaves_unused_and_expired_windows_inactive_and_reports_missing_keys`：原生预算窗口与迟到结算。

隐私元数据只补记三个已审阅夹具文件的新哈希，共七条逐行记录：原有文档域代理、临时
loopback 代理，以及 IPv6 地址类别解析。匹配行与先前获准版本逐字相同，重新检查调用上下文
后保留历史记录并追加新快照；没有扩大规则、屏蔽目录或加入新凭据豁免。

## 完整官方路径账

以下表由固定两提交的真实 name-status 路径建立；重命名同时列出前后名。组内运行路径与排除理由见上文，
“接入”不代表文件机械覆盖，也不把已存在的行为重复算作新增功能。

| 官方路径 | 差异处理组 |
| --- | --- |
| `.gitignore` | 原状 |
| `README.md` | 文档 |
| `backend/Cargo.lock` | 依赖 |
| `backend/Cargo.toml` | 依赖 |
| `backend/apps/gateway/src/bootstrap/server.rs` | Peer排除 |
| `backend/apps/gateway/tests/architecture.rs` | Messages |
| `backend/crates/gateway-admin/src/model/accounts.rs` | 额度 |
| `backend/crates/gateway-admin/src/model/audit.rs` | 插件排除 |
| `backend/crates/gateway-admin/src/model/client_keys.rs` | 预算 |
| `backend/crates/gateway-admin/src/model/observability.rs` | 日志 |
| `backend/crates/gateway-admin/src/model/provider_credentials.rs` | 额度 |
| `backend/crates/gateway-admin/src/model/reset_credit_automation.rs` | 周重置 |
| `backend/crates/gateway-admin/src/model/settings.rs` | Cron |
| `backend/crates/gateway-admin/src/ports/plugin_client_keys.rs` | 插件排除 |
| `backend/crates/gateway-admin/src/ports/store.rs` | 公共接口 |
| `backend/crates/gateway-admin/src/use_case/accounts/mod.rs` | 额度 |
| `backend/crates/gateway-admin/src/use_case/accounts/reset_credit_automation.rs` | 周重置 |
| `backend/crates/gateway-admin/src/use_case/client_keys.rs` | 插件排除 |
| `backend/crates/gateway-admin/src/use_case/observability/errors.rs` | 日志 |
| `backend/crates/gateway-admin/src/use_case/observability/mod.rs` | 日志 |
| `backend/crates/gateway-admin/src/use_case/peer_link/mod.rs` | Peer排除 |
| `backend/crates/gateway-admin/src/use_case/peer_link/projection.rs` | Peer排除 |
| `backend/crates/gateway-admin/src/use_case/peer_link/upstream/projection.rs` | Peer排除 |
| `backend/crates/gateway-admin/src/use_case/peer_link/upstream/refresh.rs` | Peer排除 |
| `backend/crates/gateway-admin/src/use_case/plugin_client_keys.rs` | 插件排除 |
| `backend/crates/gateway-admin/tests/main.rs` | 公共接口 |
| `backend/crates/gateway-admin/tests/model/provider_credentials.rs` | 额度 |
| `backend/crates/gateway-admin/tests/model/quota_forecast.rs` | 额度 |
| `backend/crates/gateway-admin/tests/use_case/account_groups.rs` | 额度 |
| `backend/crates/gateway-admin/tests/use_case/accounts/mod.rs` | 额度 |
| `backend/crates/gateway-admin/tests/use_case/accounts/reset_credit_automation.rs` | 周重置 |
| `backend/crates/gateway-admin/tests/use_case/client_keys.rs` | 插件排除 |
| `backend/crates/gateway-admin/tests/use_case/freeze_recovery.rs` | 额度 |
| `backend/crates/gateway-admin/tests/use_case/mod.rs` | 公共接口 |
| `backend/crates/gateway-admin/tests/use_case/observability/errors.rs` | 日志 |
| `backend/crates/gateway-admin/tests/use_case/observability/mod.rs` | 日志 |
| `backend/crates/gateway-admin/tests/use_case/peer_link/upstream/mod.rs` | Peer排除 |
| `backend/crates/gateway-admin/tests/use_case/proxies.rs` | 额度 |
| `backend/crates/gateway-admin/tests/use_case/settings.rs` | Cron |
| `backend/crates/gateway-api/src/admin/accounts/credentials.rs` | API账号排除 |
| `backend/crates/gateway-api/src/admin/accounts/presenter.rs` | 额度 |
| `backend/crates/gateway-api/src/admin/accounts/wire.rs` | 额度 |
| `backend/crates/gateway-api/src/admin/client_keys.rs` | 预算 |
| `backend/crates/gateway-api/src/admin/observability/presenter.rs` | 日志 |
| `backend/crates/gateway-api/src/admin/observability/wire.rs` | 日志 |
| `backend/crates/gateway-api/src/admin/plugins/management.rs` | 插件排除 |
| `backend/crates/gateway-api/src/admin/settings.rs` | Cron |
| `backend/crates/gateway-api/src/client_error.rs` | Peer排除 |
| `backend/crates/gateway-api/src/key_usage/presenter.rs` | 预算 |
| `backend/crates/gateway-api/src/lib.rs` | 公共接口 |
| `backend/crates/gateway-api/src/middleware/mod.rs` | Peer排除 |
| `backend/crates/gateway-api/src/middleware/peer_route.rs` | Peer排除 |
| `backend/crates/gateway-api/src/openai/auth.rs` | Messages |
| `backend/crates/gateway-api/src/openai/chat_completions/http.rs` | Messages |
| `backend/crates/gateway-api/src/openai/endpoint.rs` | Messages |
| `backend/crates/gateway-api/src/openai/generation/http.rs` | Messages |
| `backend/crates/gateway-api/src/openai/generation/mod.rs` | Messages |
| `backend/crates/gateway-api/src/openai/generation/request.rs` | Messages |
| `backend/crates/gateway-api/src/openai/generation/response.rs` | Messages |
| `backend/crates/gateway-api/src/openai/generation/validation.rs` | Messages |
| `backend/crates/gateway-api/src/openai/images/http.rs` | Messages |
| `backend/crates/gateway-api/src/openai/live/http.rs` | Messages |
| `backend/crates/gateway-api/src/openai/live/websocket.rs` | Messages |
| `backend/crates/gateway-api/src/openai/messages.rs` | Messages |
| `backend/crates/gateway-api/src/openai/middleware/http.rs` | Messages |
| `backend/crates/gateway-api/src/openai/middleware/request.rs` | Messages |
| `backend/crates/gateway-api/src/openai/mod.rs` | Messages |
| `backend/crates/gateway-api/src/openai/models.rs` | Messages |
| `backend/crates/gateway-api/src/openai/peer_link/wire.rs` | Peer排除 |
| `backend/crates/gateway-api/src/openai/responses/http.rs` | Messages |
| `backend/crates/gateway-api/src/openai/responses/keepalive.rs` | Messages |
| `backend/crates/gateway-api/src/openai/responses/mod.rs` | Messages |
| `backend/crates/gateway-api/src/openai/responses/request.rs` | Messages |
| `backend/crates/gateway-api/src/openai/responses/response.rs` | Messages |
| `backend/crates/gateway-api/src/openai/responses/validation.rs` | Messages |
| `backend/crates/gateway-api/src/openai/responses/websocket/forward.rs` | Messages |
| `backend/crates/gateway-api/src/openai/responses/websocket/mod.rs` | Messages |
| `backend/crates/gateway-api/src/openai/responses/websocket/protocol.rs` | Messages |
| `backend/crates/gateway-api/src/openai/router.rs` | Messages |
| `backend/crates/gateway-api/src/openai/search/http.rs` | Messages |
| `backend/crates/gateway-api/src/openai/service.rs` | Messages |
| `backend/crates/gateway-api/src/openai/usage.rs` | 预算 |
| `backend/crates/gateway-api/src/provider.rs` | 插件排除 |
| `backend/crates/gateway-api/tests/admin/accounts/handlers.rs` | 额度 |
| `backend/crates/gateway-api/tests/admin/accounts/mod.rs` | 额度 |
| `backend/crates/gateway-api/tests/admin/errors.rs` | 额度 |
| `backend/crates/gateway-api/tests/admin/mod.rs` | 额度 |
| `backend/crates/gateway-api/tests/admin/observability/response.rs` | 日志 |
| `backend/crates/gateway-api/tests/admin/settings.rs` | Cron |
| `backend/crates/gateway-api/tests/diagnostics.rs` | 公共接口 |
| `backend/crates/gateway-api/tests/health.rs` | 公共接口 |
| `backend/crates/gateway-api/tests/key_usage/fixtures.rs` | 预算 |
| `backend/crates/gateway-api/tests/middleware/mod.rs` | Peer排除 |
| `backend/crates/gateway-api/tests/middleware/peer_route.rs` | Peer排除 |
| `backend/crates/gateway-api/tests/openai/auth.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/chat_completions/http.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/chat_completions/request.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/chat_completions/response.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/live/websocket.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/messages.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/mod.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/models.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/peer_link/catalog.rs` | Peer排除 |
| `backend/crates/gateway-api/tests/openai/peer_link/mod.rs` | Peer排除 |
| `backend/crates/gateway-api/tests/openai/responses/mod.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/responses/websocket/connection/middleware.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/responses/websocket/connection/mod.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/responses/websocket/mod.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/router.rs` | Messages |
| `backend/crates/gateway-api/tests/openai/usage.rs` | 预算 |
| `backend/crates/gateway-api/tests/support/generation.rs` | Messages |
| `backend/crates/gateway-api/tests/support.rs` → `backend/crates/gateway-api/tests/support/mod.rs` | Messages |
| `backend/crates/gateway-core/Cargo.toml` | Messages |
| `backend/crates/gateway-core/src/account/mod.rs` | 额度 |
| `backend/crates/gateway-core/src/account/model.rs` | 额度 |
| `backend/crates/gateway-core/src/account/quota_limits.rs` | 额度 |
| `backend/crates/gateway-core/src/account/scope.rs` | Peer排除 |
| `backend/crates/gateway-core/src/account/selection.rs` | Peer排除 |
| `backend/crates/gateway-core/src/account/store.rs` | 额度 |
| `backend/crates/gateway-core/src/engine/budget.rs` | 预算 |
| `backend/crates/gateway-core/src/engine/execution/contract.rs` | Peer排除 |
| `backend/crates/gateway-core/src/engine/execution/service.rs` | Peer排除 |
| `backend/crates/gateway-core/src/engine/extensions.rs` | Peer排除 |
| `backend/crates/gateway-core/src/engine/middleware/contract.rs` | 插件排除 |
| `backend/crates/gateway-core/src/engine/provider.rs` | Messages |
| `backend/crates/gateway-core/src/error/mod.rs` | 公共接口 |
| `backend/crates/gateway-core/src/operation.rs` | Messages |
| `backend/crates/gateway-core/src/policy/mod.rs` | Peer排除 |
| `backend/crates/gateway-core/src/provider_ports/mod.rs` | 公共接口 |
| `backend/crates/gateway-core/src/provider_ports/runtime_policy.rs` | 公共接口 |
| `backend/crates/gateway-core/tests/account/scope.rs` | Peer排除 |
| `backend/crates/gateway-core/tests/engine/execution/mod.rs` | Peer排除 |
| `backend/crates/gateway-core/tests/engine/middleware/mod.rs` | 插件排除 |
| `backend/crates/gateway-core/tests/provider_ports/mod.rs` | 公共接口 |
| `backend/crates/gateway-core/tests/routing/snapshot.rs` | Peer排除 |
| `backend/crates/gateway-host/tests/system_update/discovery.rs` | 传输测试 |
| `backend/crates/gateway-plugin/runtime/src/callback/dispatch.rs` | 插件排除 |
| `backend/crates/gateway-plugin/runtime/src/callback/keys.rs` | 插件排除 |
| `backend/crates/gateway-plugin/runtime/tests/callback/keys.rs` | 插件排除 |
| `backend/crates/gateway-plugin/runtime/tests/generation/data.rs` | 插件排除 |
| `backend/crates/gateway-plugin/runtime/tests/support/environment.rs` | 插件排除 |
| `backend/crates/gateway-plugin/runtime/tests/support/worker/scheduled.rs` | 插件排除 |
| `backend/crates/gateway-plugin/sdk/docs/capabilities.md` | 插件排除 |
| `backend/crates/gateway-plugin/sdk/src/call/key_budgets.rs` | 插件排除 |
| `backend/crates/gateway-plugin/sdk/src/call/services/accounts.rs` | 插件排除 |
| `backend/crates/gateway-plugin/sdk/src/call/services/settings.rs` | 插件排除 |
| `backend/crates/gateway-plugin/sdk/src/client/keys.rs` | 插件排除 |
| `backend/crates/gateway-protocol/src/anthropic/capabilities.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/content.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/error.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/mod.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/request.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/response.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/stream/accumulator.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/stream/mod.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/stream/to_messages.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/stream/to_responses.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/thinking.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/tools.rs` | 协议 |
| `backend/crates/gateway-protocol/src/anthropic/usage.rs` | 协议 |
| `backend/crates/gateway-protocol/src/lib.rs` | 协议 |
| `backend/crates/gateway-protocol/src/peer.rs` | Peer排除 |
| `backend/crates/gateway-protocol/src/request.rs` | 协议 |
| `backend/crates/gateway-protocol/tests/anthropic/capabilities.rs` | 协议 |
| `backend/crates/gateway-protocol/tests/anthropic/mod.rs` | 协议 |
| `backend/crates/gateway-protocol/tests/anthropic/request.rs` | 协议 |
| `backend/crates/gateway-protocol/tests/anthropic/response.rs` | 协议 |
| `backend/crates/gateway-protocol/tests/anthropic/stream/accumulator.rs` | 协议 |
| `backend/crates/gateway-protocol/tests/anthropic/stream/mod.rs` | 协议 |
| `backend/crates/gateway-protocol/tests/anthropic/stream/translation.rs` | 协议 |
| `backend/crates/gateway-protocol/tests/anthropic/usage.rs` | 协议 |
| `backend/crates/gateway-protocol/tests/main.rs` | 协议 |
| `backend/crates/gateway-protocol/tests/request.rs` | 协议 |
| `backend/crates/gateway-store/src/backup/snapshot.rs` | 插件排除 |
| `backend/crates/gateway-store/src/bundle.rs` | Peer排除 |
| `backend/crates/gateway-store/src/postgres/account_groups.rs` | 额度 |
| `backend/crates/gateway-store/src/postgres/client_budget_windows.rs` | 插件排除 |
| `backend/crates/gateway-store/src/postgres/client_budgets.rs` | 预算 |
| `backend/crates/gateway-store/src/postgres/client_keys.rs` | 预算 |
| `backend/crates/gateway-store/src/postgres/mod.rs` | 插件排除 |
| `backend/crates/gateway-store/src/postgres/observability/admin_adapter.rs` | 日志 |
| `backend/crates/gateway-store/src/postgres/observability/mapping.rs` | 日志 |
| `backend/crates/gateway-store/src/postgres/observability/queries/usage.rs` | 日志 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/admin_adapter.rs` | 额度 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/admin_queries.rs` | 额度 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/core_adapter.rs` | 额度 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/mapping.rs` | 额度 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/mod.rs` | 额度 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/model_catalogs.rs` | 目录 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/quota.rs` | 额度 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/repository.rs` | 额度 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/rows.rs` | 额度 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/runtime.rs` | 额度 |
| `backend/crates/gateway-store/src/postgres/proxies.rs` | 额度 |
| `backend/crates/gateway-store/src/postgres/runtime_settings.rs` | Cron |
| `backend/crates/gateway-store/tests/postgres/backup_export.rs` | 插件排除 |
| `backend/crates/gateway-store/tests/postgres/client_budget_windows.rs` | 插件排除 |
| `backend/crates/gateway-store/tests/postgres/client_budgets.rs` | 预算 |
| `backend/crates/gateway-store/tests/postgres/mod.rs` | 公共接口 |
| `backend/crates/gateway-store/tests/postgres/observability/mod.rs` | 日志 |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/admin_adapter.rs` | 额度 |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/core_adapter.rs` | 额度 |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/mod.rs` | 额度 |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/quota.rs` | 额度 |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/timestamps.rs` | 额度 |
| `backend/crates/gateway-store/tests/postgres/proxies.rs` | 额度 |
| `backend/crates/gateway-store/tests/postgres/runtime_settings.rs` | Cron |
| `backend/crates/gateway-store/tests/redis/worker_lease.rs` | Peer排除 |
| `backend/crates/providers/openai/Cargo.toml` | Messages |
| `backend/crates/providers/openai/src/admin/mod.rs` | 额度 |
| `backend/crates/providers/openai/src/admin/quota.rs` | 额度 |
| `backend/crates/providers/openai/src/compat/messages.rs` | Messages |
| `backend/crates/providers/openai/src/compat/mod.rs` | Messages |
| `backend/crates/providers/openai/src/compat/request.rs` | Messages |
| `backend/crates/providers/openai/src/credential/admin.rs` | API账号排除 |
| `backend/crates/providers/openai/src/credential/api_key.rs` | API账号排除 |
| `backend/crates/providers/openai/src/credential/catalog.rs` | 目录 |
| `backend/crates/providers/openai/src/credential/catalog/mod.rs` | 目录 |
| `backend/crates/providers/openai/src/credential/catalog/refresh.rs` | 目录 |
| `backend/crates/providers/openai/src/credential/codec.rs` | API账号排除 |
| `backend/crates/providers/openai/src/credential/mod.rs` | 公共接口 |
| `backend/crates/providers/openai/src/credential/quota/mod.rs` | 额度 |
| `backend/crates/providers/openai/src/credential/quota/passive.rs` | 额度 |
| `backend/crates/providers/openai/src/credential/quota/scheduling.rs` | 额度 |
| `backend/crates/providers/openai/src/credential/quota/service.rs` | 额度 |
| `backend/crates/providers/openai/src/credential/quota/snapshot.rs` | 额度 |
| `backend/crates/providers/openai/src/credential/quota/warmup.rs` | Cron |
| `backend/crates/providers/openai/src/credential/selector.rs` | 额度 |
| `backend/crates/providers/openai/src/lib.rs` | 公共接口 |
| `backend/crates/providers/openai/src/provider/execution.rs` | Messages |
| `backend/crates/providers/openai/src/provider/failure.rs` | API账号排除 |
| `backend/crates/providers/openai/src/provider/messages/count.rs` | Messages |
| `backend/crates/providers/openai/src/provider/messages/mod.rs` | Messages |
| `backend/crates/providers/openai/src/provider/messages/stream.rs` | Messages |
| `backend/crates/providers/openai/src/provider/messages/translator.rs` | Messages |
| `backend/crates/providers/openai/src/provider/mod.rs` | Messages |
| `backend/crates/providers/openai/src/provider/observation.rs` | Messages |
| `backend/crates/providers/openai/src/provider/upstream_adapter.rs` | 插件排除 |
| `backend/crates/providers/openai/src/provider/workers.rs` | 公共接口 |
| `backend/crates/providers/openai/src/transport/catalog.rs` | 目录 |
| `backend/crates/providers/openai/src/transport/client.rs` | API账号排除 |
| `backend/crates/providers/openai/src/transport/client_catalog.rs` | API账号排除 |
| `backend/crates/providers/openai/src/transport/client_messages.rs` | API账号排除 |
| `backend/crates/providers/openai/src/transport/client_stream.rs` | Peer排除 |
| `backend/crates/providers/openai/src/transport/diagnostics/mod.rs` | Peer排除 |
| `backend/crates/providers/openai/src/transport/headers.rs` | API账号排除 |
| `backend/crates/providers/openai/src/transport/mod.rs` | API账号排除 |
| `backend/crates/providers/openai/src/transport/outbound.rs` | Peer排除 |
| `backend/crates/providers/openai/src/transport/request.rs` | Messages |
| `backend/crates/providers/openai/src/transport/usage.rs` | API账号排除 |
| `backend/crates/providers/openai/src/transport/websocket/model.rs` | Peer排除 |
| `backend/crates/providers/openai/tests/admin/mod.rs` | 额度 |
| `backend/crates/providers/openai/tests/admin/peer_info.rs` | Peer排除 |
| `backend/crates/providers/openai/tests/credential/admin.rs` | API账号排除 |
| `backend/crates/providers/openai/tests/credential/catalog.rs` → `backend/crates/providers/openai/tests/credential/catalog/mod.rs` | 目录 |
| `backend/crates/providers/openai/tests/credential/contract/mod.rs` | 公共接口 |
| `backend/crates/providers/openai/tests/credential/quota/mod.rs` | 额度 |
| `backend/crates/providers/openai/tests/credential/quota/reset_recovery.rs` | 额度 |
| `backend/crates/providers/openai/tests/credential/quota/scheduling.rs` | 额度 |
| `backend/crates/providers/openai/tests/credential/quota/slots.rs` | 额度 |
| `backend/crates/providers/openai/tests/credential/quota/snapshot.rs` | 额度 |
| `backend/crates/providers/openai/tests/credential/quota/warmup.rs` | Cron |
| `backend/crates/providers/openai/tests/provider/contract/catalog_startup.rs` | 目录 |
| `backend/crates/providers/openai/tests/provider/contract/messages.rs` | Messages |
| `backend/crates/providers/openai/tests/provider/contract/mod.rs` | 公共接口 |
| `backend/crates/providers/openai/tests/provider/contract/model_account_eligibility.rs` | 目录 |
| `backend/crates/providers/openai/tests/provider/contract/passthrough.rs` | Messages |
| `backend/crates/providers/openai/tests/provider/contract/search_model_access.rs` | API账号排除 |
| `backend/crates/providers/openai/tests/provider/contract/upstream_adapter.rs` | 插件排除 |
| `backend/crates/providers/openai/tests/provider/live.rs` | API账号排除 |
| `backend/crates/providers/openai/tests/support.rs` | 公共接口 |
| `backend/crates/providers/openai/tests/transport/tls.rs` | 传输测试 |
| `backend/crates/providers/openai/tests/transport/websocket/mod.rs` | 传输测试 |
| `backend/crates/providers/xai/src/admin.rs` | 额度 |
| `backend/crates/providers/xai/src/credential/repository.rs` | 额度 |
| `backend/crates/providers/xai/tests/admin.rs` | 额度 |
| `backend/crates/providers/xai/tests/credential/quota.rs` | 额度 |
| `backend/crates/providers/xai/tests/credential/refresh.rs` | 额度 |
| `backend/crates/providers/xai/tests/credential/selector.rs` | 额度 |
| `backend/crates/providers/xai/tests/support.rs` | 额度 |
| `backend/migrations/.frozen-sha256` | 迁移 |
| `backend/migrations/0040_account_scheduling_key_budgets_and_peer_routing.sql` | 迁移 |
| `backend/migrations/0041_account_model_catalogs.sql` | 迁移 |
| `deploy/README.md` | 原状 |
| `docs/api.md` | 文档 |
| `docs/architecture.md` | 文档 |
| `docs/development.md` | 原状 |
| `frontend/src/api/modules/accounts/index.ts` | 额度 |
| `frontend/src/api/modules/accounts/management.ts` | 额度 |
| `frontend/src/api/modules/accounts/quota.ts` | 额度 |
| `frontend/src/api/modules/accounts/reset-credits.ts` | 周重置 |
| `frontend/src/api/modules/accounts/types.ts` | 额度 |
| `frontend/src/api/modules/dashboard.ts` | 额度 |
| `frontend/src/api/modules/settings/index.ts` | Cron |
| `frontend/src/api/modules/usage.ts` | 日志 |
| `frontend/src/components/usage/UsageErrorCell.vue` | 日志 |
| `frontend/src/components/usage/shared/errors.ts` | 日志 |
| `frontend/src/i18n/locales/en/accounts.json` | 翻译 |
| `frontend/src/i18n/locales/en/dashboard.json` | 翻译 |
| `frontend/src/i18n/locales/en/keys.json` | 翻译 |
| `frontend/src/i18n/locales/en/settings.json` | 翻译 |
| `frontend/src/i18n/locales/en/usage.json` | 翻译 |
| `frontend/src/i18n/locales/zh-Hans/accounts.json` | 翻译 |
| `frontend/src/i18n/locales/zh-Hans/dashboard.json` | 翻译 |
| `frontend/src/i18n/locales/zh-Hans/keys.json` | 翻译 |
| `frontend/src/i18n/locales/zh-Hans/settings.json` | 翻译 |
| `frontend/src/i18n/locales/zh-Hans/usage.json` | 翻译 |
| `frontend/src/i18n/locales/zh-Hant/accounts.json` | 翻译 |
| `frontend/src/i18n/locales/zh-Hant/dashboard.json` | 翻译 |
| `frontend/src/i18n/locales/zh-Hant/keys.json` | 翻译 |
| `frontend/src/i18n/locales/zh-Hant/settings.json` | 翻译 |
| `frontend/src/i18n/locales/zh-Hant/usage.json` | 翻译 |
| `frontend/src/utils/client.ts` | UI |
| `frontend/src/views/accounts/components/AccountApiKeyFields.vue` | API账号排除 |
| `frontend/src/views/accounts/components/AccountConcurrencyTable.vue` | UI |
| `frontend/src/views/accounts/components/AccountConnectionTestModal.vue` | UI |
| `frontend/src/views/accounts/components/AccountEditModal.vue` | API账号排除 |
| `frontend/src/views/accounts/components/AccountOverviewCards.vue` | 额度 |
| `frontend/src/views/accounts/components/AccountSettingsFields.vue` | API账号排除 |
| `frontend/src/views/accounts/components/account-quota-panel/Entry.vue` | UI |
| `frontend/src/views/accounts/components/account-quota-panel/QuotaActionButton.vue` | UI |
| `frontend/src/views/accounts/components/account-quota-panel/QuotaLimits.vue` | 额度 |
| `frontend/src/views/accounts/components/account-quota-panel/ResetCreditAutomation.vue` | 周重置 |
| `frontend/src/views/accounts/components/account-quota-panel/ResetCredits.vue` | UI |
| `frontend/src/views/accounts/components/account-quota-panel/index.vue` | 额度 |
| `frontend/src/views/accounts/components/account-status-badge/index.vue` | 额度 |
| `frontend/src/views/accounts/components/account-status-badge/presenter.ts` | 额度 |
| `frontend/src/views/accounts/composables/useAccountApiKeyFormRules.ts` | API账号排除 |
| `frontend/src/views/accounts/composables/useAccountEditor.ts` | API账号排除 |
| `frontend/src/views/accounts/composables/useAccountOnboarding.ts` | API账号排除 |
| `frontend/src/views/accounts/composables/useAccountsQuery.ts` | 额度 |
| `frontend/src/views/accounts/constants.ts` | 额度 |
| `frontend/src/views/accounts/index.vue` | 额度 |
| `frontend/src/views/accounts/utils/upstreamApiKey.ts` | API账号排除 |
| `frontend/src/views/dashboard/components/AccountOverviewCard.vue` | 额度 |
| `frontend/src/views/settings/components/AccountWarmupCard.vue` | Cron |
| `frontend/src/views/settings/components/ResetCreditCard.vue` | 周重置 |
| `frontend/src/views/settings/composables/useSettingsForm.ts` | Cron |
| `frontend/src/views/settings/composables/useSettingsFormRules.ts` | Cron |
| `frontend/src/views/settings/index.vue` | Cron |
| `frontend/src/views/theme/fixtures/dashboard.ts` | 额度 |
| `frontend/src/views/usage/components/UsageErrorDetailModal.vue` | 日志 |
| `frontend/src/views/usage/components/UsageErrorDiagnosis.vue` | 日志 |
| `frontend/src/views/usage/components/UsageRecordDetailModal.vue` | 日志 |
| `release/notes.md` | 原状 |
| `release/version.yaml` | 原状 |
