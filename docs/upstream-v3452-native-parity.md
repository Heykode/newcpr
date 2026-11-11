# 官方 v3.45.2 原生功能合并记录

## 基线与状态

- 日期：2026-11-11；目标仓库 `Heykode/newcpr`，工作分支 `codex/upstream-v3451`。
- 本地起点：`4b0c50cd214197be74798bebfc7370f033f6ad13`，已本地提交 v3.45.1 接入。
- 官方基线：`1b24029a9700e4e554c3f4764d1201ff16607f92`（v3.45.1）。
- 固定目标：`b22b0cfa4162dd884fb9e2255193af56c4f702cd`（v3.45.2）；12 个提交、112 个变更路径。
- 本地实现与验证完成后，用户已授权累计提交、推送、合并及免备份部署。发布目标为 v3.45.2，实际部署基线为 v3.43.1；版本说明和精确迁移计划随本次变更提交，发布结果以实际执行记录为准。
- 按本轮用户批准范围排除上游 API-key 账号及其专属 CPR 互联转发、插件运行时与已取消功能；公共修复独立接入。
  早期记录中的 blanket peer_link 排除不构成用户永久禁令，本轮边界以上述用途为准。
- 共用规则沿用 [合并核对清单](upstream-integration-checklist.md)，本文是固定版本证据，不另建门禁。

## 来源、作用代码与调用链

下表“接入”包括直接移植官方函数和落到现有模块的必要接口适配，不表示整文件字节相同。
路径以各 crate 为起点；附录逐条关联本次官方路径。

| 分组 | 官方提交与更新 | 本地实际接入 |
| --- | --- | --- |
| 轮询 | `343c57901`：仅成功立即分配推进游标，亲和命中不消耗位置；会话与游标原子准入 | Core `provider_ports/mod.rs` → Store `redis/provider_session_affinity.rs`、`scheduling_cleanup.rs`、`credential_leases.rs` → OpenAI `credential/selector/{mod,admission,capacity_wait}.rs`。使用官方独立 cursor、先比后写 Lua、显式释放与重选分支，保留本地 generation/legacy 兼容 |
| 重试 | `914c00785`：`NO_MORE_RETRY` 跳过同账号恢复 | protocol `openai/events.rs` → OpenAI `transport/diagnostics.rs` 与 `transport/protocol/responses.rs` → `provider/failure.rs` → Core `error/mod.rs`、`engine/coordinator.rs`、`diagnostics/trace.rs`。HTTP 错误体、SSE/WS wire error 与 WS opening failure 均带 flag；完整与裁剪诊断均保留 |
| 档位 | `36ba79ca5`：保留原生 Responses 显式 service tier | 删除 OpenAI `compat/request.rs` 的全局 auto/default stripping；仅在 `provider/mod.rs` 的 Messages→Responses 转换后执行官方删除条件。官方 compat/messages helper 在本地唯一转换调用点内联，不创建无消费者模块；API-key 目标分支排除 |
| 观测 | `c0fdc1bbf`、`e2f194875`：Messages 显式 effort、公共事实、真实会话归属及协议常量 | protocol `request.rs`、`anthropic/thinking.rs`、`openai/codex.rs` → Core `operation/mod.rs`、`engine/{provider,execution}.rs` → OpenAI/xAI Provider、API 和 transport 消费者。纯协议观测归 protocol，Provider 仅补实际 session 的旧 effort、continuation 与端点模型 |
| 配置 | `055da4047`：Codex 配置启用异步压缩 | `frontend/src/views/api-keys/utils/codexConfig.ts` 的 `[features]` 加 `async_compaction=true`。沿实际下载配置入口；不复制官方部署配置或重写本地发行身份 |
| 界面 | `07b61bdc8`：帮助图标对齐、账号排队与 Retry-After 说明 | `HelpPopover.vue` → `RotationStrategyCard.vue`、`RuntimeSettingsCard.vue`、`ApiKeyCreateModal.vue`；同步 en accounts/keys/settings、zh-Hans/zh-Hant settings。适配 BasePopover 的 `trigger` 插槽、BaseIconButton 的 label/size 和现有 label-extra；保留多分组布局 |
| TLS | `1271fa018`：同平台官方 Codex 二进制 ClientHello 对照 | OpenAI `tests/transport/tls.rs` 官方 ignored 测试，复用本地 HTTP builder 与动态证书生成器；既有 tokio process 特性已满足，无依赖修改。操作说明接入现有 `docs/ws-tls-upstream-parity.md` |
| 互联排除 | `b3de0d637`：仅向已发现 CPR 节点发送路由头 | 上游 API-key / peer identity / outbound 路由专属分支不引入。不会影响自有登录账号的请求头构造；混合 Provider failure、selector 和协议文件的公共修复按其他组接入 |
| API账号排除 | `151a3c6bf`：Responses 上游 API-key 的 Codex 模型目录协商 | 不增加 API-key 账号类型、目录协议探测或 peer catalog 回退；OAuth 账号已有目录逻辑保留，公共协议常量与观测改动单独接入 |
| 插件排除 | `e2f194875` 中 plugin SDK、extension owner 与中间件部分 | 本地没有相应运行时，不添加插件专用 Provider state、调度 Pick/Delegate 或 middleware 测试夹具；原生 HTTP/WS、OpenAI/xAI 真实调用者均核对 |
| 文档 | 以上提交附带的当前协议/测试说明 | 更新 API、architecture、TLS 说明及 scheduling-affinity、account-capacity-wait、protocol-compatibility、precommit-controls；替换原生全局省略档位和冻结 RR cursor 的旧条款 |
| 原状 | `bd69329a3`、`b22b0cfa4` 及官方部署特定说明 | 不复制 release 版本/通道或覆盖 NewCPR 部署流程；本轮无 schema 迁移、依赖升级或 CI 门禁变更 |

## 必要适配、排除与额外修正

1. **绑定代次与旧数据**：官方 claim 的 account 落为本地完整 binding（account + generation）；Lua 仍先比较全部条件再同时写入。兼容 raw account ID 与 JSON legacy，保持迟到旧请求不能恢复新绑定。
2. **分拆的等待实现**：官方单文件 selector 落到本地即时选择/等待两条路径。外层立即取得槽以及入队前重查已 Ready 均提交成功分配 cursor；已入队晋升和排队中改选不提交。绑定 CAS 冲突等待真实 lease release 后重选，不消耗凭据 snapshot 的三次冲突预算，仍受原取消/截止控制。
3. **错误分类与缓存偏好**：带 session claim 的准入存储失败沿现有 SessionBound 处理，无 session 的 cursor 错误沿普通 Store 错误。Preferred 的可选缓存写失败可丢弃绑定偏好，但仍须独立提交实际 RR 分配；没有扩张换号授权。
4. **协议与会话**：保留本地 Codex helper 的两参兼容入口，公共观测调用三参 continuation 入口。Provider 身份与 wire 协议常量分开使用；没有用同名字符串批量改写账号身份。Core 从实际 Provider session 选择观测归属，插件 owner 检查因没有插件运行时排除。
5. **错误恢复**：使用本地 `with_transient_retry_policy` 承载官方 policy，不改动既有账户切换预算、精确续接归属、发送安全和已交付输出边界。`NO_MORE_RETRY` 不是全局禁止换号，也不是强制换号。
6. **前端组件接口**：浏览器发现官方 `reference` 插槽在本地无效，已改接现有 `trigger` 并保留命名 dialog。没有替换基础组件、收紧分组尺寸或改动账号列表排版。
7. **旧验证清理**：移除 Provider 中已由公共协议/Core 承接的重复观测断言和专属 helper；删除本轮误带的两个旧身份重复用例。API frozen test-tree 移除基线已不存在的 `messages/session.rs` 路径，不恢复失效文件。保留必要官方正反例和本地代次/租约回归。
8. **验证环境问题**：Store worker lifecycle 首次失败是本轮隔离服务连接 URL 未含其既有夹具要求的密码；补齐合成测试密码后通过。浏览器首次有错误文案 locator，按真实文案更正。均未修改生产逻辑迎合夹具。

这些是本轮合入时的接口适配和校验修正，不应计作新发现的官方线上缺陷。
所有临时脚本、源码对照快照、截图与原始日志留在仓库外。

## 验证证据

均为本机隔离环境、合成请求或源码检查，没有真实上游推理、重登或生产流量。
先前已通过且未改变的有效范围复用，只补新增与失败项。

| 范围 | 结果 |
| --- | --- |
| Rust | workspace/all-targets check 与严格 Clippy `-D warnings` 通过；fmt/diff 检查通过 |
| Core / protocol / Providers | 首轮 2665 项通过、1 项 TLS reference ignored。后补 credential 324 项、Core operation 14 项、Messages 档位/真实 session owner/可选 cursor 各 1 项通过；删除重复用例后 protocol 141 项复验通过；轮询 5 项、扩展原生档位矩阵复验通过；本地等待分支 1 项覆盖立即准入/入队前 Ready/已入队晋升并通过 |
| 实际 Redis | 120 项中首次 119 通过，worker fixture 补合成密码后该项通过；含原子 cursor/session CAS、legacy/generation、实际释放及停机/双池 |
| API / 应用架构 | API 首轮 2 单元+651 集成通过，唯一旧路径清单失败；修正后 API architecture 5 项通过。应用 composition architecture 14 项通过 |
| 前端 | 1158 项测试通过；帮助插槽修正后 lint、含 typecheck 的 build 再次通过 |
| 真实浏览器 | 1440/390px：权重/换号/Retry-After/安装种子帮助可打开；弹层不越界，多分组无重叠、页面无横向溢出与脚本错误；实际生成配置含 async_compaction |
| 旧代码对照 | 固定旧 HEAD 两项实际断言失败：新 session 应轮到下一账号却重复旧账号；原生 auto 应保留却被删为 None。当前实现两项通过，档位补齐 HTTP/WS、缺字段和 null。独立源码快照曾误用 Cargo 缓存；清理受影响 crate 缓存后重新编译取得证据，编译错误不计作回归复现 |
| 隐私 | 最终完整工作树快照检查通过。仅对既有 TLS 文件的两处函数名误报和一处 loopback 合成代理夹具逐条复审并补记新哈希，保留历史记录；无规则放宽。未触碰用户 index，提交/发布时仍须检查实际 staged 和历史 |
| 未执行 | 官方 Codex 二进制 TLS 对照默认 ignored；未做 Linux 制品构建、真实上游调用、生产回放或部署，不据此承诺零缺陷 |

关键回归：

- `strict_round_robin_rotates_each_new_session_across_eligible_accounts`、`round_robin_counts_concurrent_first_requests_as_one_session`、`busy_leases_do_not_consume_the_next_round_robin_assignment`。
- `round_robin_wait_path_commits_only_before_queue_registration`：本地分拆等待入口仅对入队前立即准入推进游标。
- `round_robin_retains_the_assignment_when_a_cache_preference_cannot_be_saved`、`round_robin_commits_only_with_the_current_cursor_and_session_version`。
- `no_more_retry_skips_same_account_wait_even_when_retry_after_is_short`、`upstream_no_more_retry_blocks_same_account_recovery_for_a_short_retry_after`（HTTP/SSE/WS opening 的正负例）。
- `native_responses_preserve_explicit_service_tiers`（HTTP/WS，缺字段/null/auto/default/priority/flex）、`translated_messages_service_tiers_follow_the_selected_responses_upstream`。
- `request_observation_is_stable_across_candidate_order_and_respects_session_ownership`、`request_observation_only_inherits_native_openai_session_configuration`、`image_generation_fact_tracks_body_replacement_and_ignores_context`。

## 完整官方路径账

每条来自固定两提交的实际 name-status；组名关联上表实际本地落点。混合文件按组分别处理，
不以没有同名文件为排除公共功能的理由。

| 官方路径 | 处理组 |
| --- | --- |
| `backend/crates/gateway-admin/src/use_case/peer_link/upstream/projection.rs` | 互联排除 / API账号排除 |
| `backend/crates/gateway-admin/src/use_case/peer_link/upstream/service.rs` | 互联排除 / API账号排除 |
| `backend/crates/gateway-admin/src/use_case/peer_link/upstream/snapshot.rs` | 互联排除 / API账号排除 |
| `backend/crates/gateway-admin/tests/use_case/credentials/peer.rs` | 互联排除 / API账号排除 |
| `backend/crates/gateway-admin/tests/use_case/peer_link/upstream/mod.rs` | 互联排除 / API账号排除 |
| `backend/crates/gateway-admin/tests/use_case/peer_link/upstream/refresh.rs` | 互联排除 / API账号排除 |
| `backend/crates/gateway-api/src/openai/endpoint.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/generation/http.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/generation/mod.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/generation/response.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/generation/validation.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/images/http.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/live/http.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/live/mod.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/middleware/request.rs` | 观测（原生映射）/ 插件排除 |
| `backend/crates/gateway-api/src/openai/models.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/responses/request.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/responses/response.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/responses/validation.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/responses/websocket/forward.rs` | 观测（原生映射）/ 插件排除 |
| `backend/crates/gateway-api/src/openai/responses/websocket/mod.rs` | 观测（原生映射）/ 插件排除 |
| `backend/crates/gateway-api/src/openai/search/http.rs` | 观测 |
| `backend/crates/gateway-api/src/openai/service.rs` | 观测 |
| `backend/crates/gateway-api/tests/admin/upstream_info.rs` | 互联排除 / API账号排除 |
| `backend/crates/gateway-core/src/account/upstream.rs` | 互联排除 / API账号排除 |
| `backend/crates/gateway-core/src/diagnostics/trace.rs` | 重试（原生调用链） |
| `backend/crates/gateway-core/src/engine/coordinator.rs` | 重试（原生调用链） |
| `backend/crates/gateway-core/src/engine/execution/service.rs` | 观测 |
| `backend/crates/gateway-core/src/engine/provider.rs` | 观测 |
| `backend/crates/gateway-core/src/error/mod.rs` | 重试（原生调用链） |
| `backend/crates/gateway-core/src/operation.rs` | 观测 |
| `backend/crates/gateway-core/src/provider_ports/affinity.rs` | 轮询 |
| `backend/crates/gateway-core/src/provider_ports/lease.rs` | 轮询 |
| `backend/crates/gateway-core/src/provider_ports/mod.rs` | 轮询 |
| `backend/crates/gateway-core/tests/engine/coordinator.rs` | 重试（原生调用链） |
| `backend/crates/gateway-core/tests/engine/execution/mod.rs` | 观测 |
| `backend/crates/gateway-core/tests/operation.rs` | 观测 |
| `backend/crates/gateway-plugin/sdk/docs/capabilities.md` | 插件排除 |
| `backend/crates/gateway-protocol/src/anthropic/mod.rs` | 观测 |
| `backend/crates/gateway-protocol/src/anthropic/thinking.rs` | 观测 |
| `backend/crates/gateway-protocol/src/openai/codex.rs` | 观测 |
| `backend/crates/gateway-protocol/src/openai/events.rs` | 重试 |
| `backend/crates/gateway-protocol/src/openai/mod.rs` | 观测 |
| `backend/crates/gateway-protocol/src/request.rs` | 观测 |
| `backend/crates/gateway-protocol/tests/openai/codex.rs` | 观测 |
| `backend/crates/gateway-protocol/tests/openai/events.rs` | 重试 |
| `backend/crates/gateway-protocol/tests/request.rs` | 观测 |
| `backend/crates/gateway-store/src/redis/credential_leases/cleanup.rs` | 轮询 |
| `backend/crates/gateway-store/src/redis/credential_leases/mod.rs` | 轮询 |
| `backend/crates/gateway-store/src/redis/mod.rs` | 轮询 |
| `backend/crates/gateway-store/src/redis/provider_session_affinity.rs` | 轮询 |
| `backend/crates/gateway-store/tests/bundle.rs` | 轮询 |
| `backend/crates/gateway-store/tests/redis/provider_session_affinity.rs` | 轮询 |
| `backend/crates/providers/openai/Cargo.toml` | TLS（既有 process 特性） |
| `backend/crates/providers/openai/src/admin/mod.rs` | 观测 |
| `backend/crates/providers/openai/src/compat/messages.rs` | 档位 / 观测；排除 API账号目标 |
| `backend/crates/providers/openai/src/compat/mod.rs` | 档位 / 观测；排除 API账号目标 |
| `backend/crates/providers/openai/src/compat/request.rs` | 档位 / 观测；排除 API账号目标 |
| `backend/crates/providers/openai/src/credential/selector.rs` | 轮询；排除 API账号/插件分支 |
| `backend/crates/providers/openai/src/lib.rs` | API账号排除 / 互联排除 |
| `backend/crates/providers/openai/src/provider/execution.rs` | 观测 |
| `backend/crates/providers/openai/src/provider/failure.rs` | 重试（原生调用链） |
| `backend/crates/providers/openai/src/provider/live.rs` | 观测 |
| `backend/crates/providers/openai/src/provider/messages/mod.rs` | 档位 / 观测；排除 API账号目标 |
| `backend/crates/providers/openai/src/provider/messages/translator.rs` | 观测 |
| `backend/crates/providers/openai/src/provider/mod.rs` | 观测 / 档位（实际本地消费者） |
| `backend/crates/providers/openai/src/provider/observation.rs` | 观测 |
| `backend/crates/providers/openai/src/provider/upstream_adapter.rs` | 观测（原生映射）/ 插件排除 |
| `backend/crates/providers/openai/src/transport/canonical.rs` | 观测 |
| `backend/crates/providers/openai/src/transport/catalog.rs` | API账号排除 / 互联排除 |
| `backend/crates/providers/openai/src/transport/client_catalog.rs` | API账号排除 / 互联排除 |
| `backend/crates/providers/openai/src/transport/diagnostics/mod.rs` | 重试（原生调用链） |
| `backend/crates/providers/openai/src/transport/outbound.rs` | 互联排除 / 插件排除 |
| `backend/crates/providers/openai/src/transport/protocol/responses.rs` | 重试（原生调用链） |
| `backend/crates/providers/openai/src/transport/request.rs` | 观测 |
| `backend/crates/providers/openai/tests/admin/mod.rs` | 观测 |
| `backend/crates/providers/openai/tests/credential/catalog/mod.rs` | API账号排除 / 互联排除 |
| `backend/crates/providers/openai/tests/credential/contract/capacity.rs` | 轮询；排除 API账号/插件分支 |
| `backend/crates/providers/openai/tests/credential/contract/mod.rs` | 轮询；排除 API账号/插件分支 |
| `backend/crates/providers/openai/tests/provider/contract/cache_preference.rs` | 轮询；排除 API账号/插件分支 |
| `backend/crates/providers/openai/tests/provider/contract/cookies.rs` | API账号排除 / 互联排除 |
| `backend/crates/providers/openai/tests/provider/contract/messages.rs` | 档位 / 观测；排除 API账号目标 |
| `backend/crates/providers/openai/tests/provider/contract/mod.rs` | 观测 / 档位（实际本地消费者） |
| `backend/crates/providers/openai/tests/provider/contract/outbound.rs` | 互联排除 / 插件排除 |
| `backend/crates/providers/openai/tests/provider/contract/passthrough.rs` | 档位 / 观测；排除 API账号目标 |
| `backend/crates/providers/openai/tests/provider/contract/recovery.rs` | 重试（原生调用链） |
| `backend/crates/providers/openai/tests/support.rs` | 轮询 / 观测测试适配；排除互联夹具 |
| `backend/crates/providers/openai/tests/transport/tls.rs` | TLS（既有 process 特性） |
| `backend/crates/providers/xai/src/admin.rs` | 观测 |
| `backend/crates/providers/xai/src/provider/mod.rs` | 观测 / 档位（实际本地消费者） |
| `backend/crates/providers/xai/src/transport/canonical.rs` | 观测 |
| `backend/crates/providers/xai/src/transport/compaction.rs` | 观测 |
| `backend/crates/providers/xai/src/transport/request/mod.rs` | 观测 |
| `backend/crates/providers/xai/tests/provider/contract/mod.rs` | 观测 / 档位（实际本地消费者） |
| `backend/crates/providers/xai/tests/support.rs` | 轮询 / 观测测试适配；排除互联夹具 |
| `deploy/README.md` | 配置 / 原状（本地部署流程） |
| `docs/api.md` | 文档 |
| `docs/architecture.md` | 文档 |
| `docs/development.md` | TLS（接入现有 TLS 文档） |
| `frontend/src/components/HelpPopover.vue` | 界面 |
| `frontend/src/i18n/locales/en/accounts.json` | 界面 |
| `frontend/src/i18n/locales/en/keys.json` | 界面 |
| `frontend/src/i18n/locales/en/settings.json` | 界面 |
| `frontend/src/i18n/locales/zh-Hans/settings.json` | 界面 |
| `frontend/src/i18n/locales/zh-Hant/settings.json` | 界面 |
| `frontend/src/utils/client.ts` | 配置 |
| `frontend/src/views/keys/components/ApiKeyFormModal.vue` | 界面 |
| `frontend/src/views/plugins/components/PluginHelpPopover.vue` | 插件排除 |
| `frontend/src/views/settings/components/ConcurrencyPolicyCard.vue` | 界面 |
| `frontend/src/views/settings/components/SmartSchedulingModal.vue` | 界面 |
| `release/notes.md` | 原状 |
| `release/version.yaml` | 原状 |
