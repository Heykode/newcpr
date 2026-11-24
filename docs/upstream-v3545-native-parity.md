# CPR v3.54.5 原生更新接入记录

## 固定范围

- 本地起点：`be6129511053c45afb911ded58cc2b1f38ad2f67`（NewCPR v3.54.3）。
- 上次官方基线：`6900da36eedb3099d43c8c9bf5b8b97d82d15688`。
- 固定目标：`21622eeacdab919013408a4b77f5536ed8888f32`，20 个提交、99 个文件。
- 包含官方 v3.54.5 及后三个配额修复；不是官方 v3.54.6。不随远端分支漂移扩张范围。
- 实施验证完成后已获准提交、推送及合并；标签和部署需另行授权。用户确认保留已完成的本机独立 PostgreSQL/Redis 与合成上游结果，后续验证转到既有 OVH 隔离环境，不调用生产账号。
- 排除上游 API Key 导入、互联、插件产品及另案 JSON 转换问题。现有 15 种 Codex 账号套餐筛选不变。

共用政策见 [官方更新合并核对清单](upstream-integration-checklist.md)。

## 行为与调用链

| 编号 | 官方来源 | 本地入口、调用者及错误边界 |
| --- | --- | --- |
| E01 | `1ee67cd6b2`、`7ff77413c8`、`8075487f57` | Responses Provider → transport/client_sse → coordinator/pump/upload；上传同步读帧，首文本前明确 1009 进入大小选择，之后保持原错误且禁重放。exact continuation 经 failure 返回 ClientReplayRequired；裸断管保持 Ambiguous。最终 exclude 头进入 Eq/Hash/stable hash、逻辑连接、forced key 和出口 owner 查找 |
| E02 | `5fced665bf` | Images generation/edit → select_for_provider_endpoint → immediate 或 capacity_wait → image_primary_to_keep → admit_selected_session → 本地 Redis 原子准入。只为仍启用、已授权且缺图片资格的主账号借号；租约实际账号与主绑定分别记录，CAS 冲突释放并重选 |
| E03 | `f46fe43bf3` | Core start/direct plan → Provider::allows_direct_model_scope → OpenAI root affinity/selector scope lookup。严格父账号不在专用模型候选时重建原模型计划；原生续接和显式路由优先；存储不可用 fail closed |
| E04 | `000107597e` | Admin OAuth complete/重授权 → 既有身份事务 → rotate_provider_account_in_transaction；同身份保留快照、访问事实、暂停和预热，换身份清除。初始/周期 quota list include_disabled=true；选号、目录及预热 false。xAI 共享维护修复同样接入 |
| E05 | `a515d54136`、`98724da968`、`21622eeacd` | HTTP/WS/SSE error → UsageLimitWindow → CodexAccountFailure → quota access write → SQL 0100。Store 所有账号投影和 get_quotas 带 hint；调度推导复核边界，reconcile_refresh 处理未匹配 reset、唯一时长提示及漂移下两次观测 |
| E06 | `a515d54136` | Admin ProviderQuota::apply_account_access 保留窗口 measured/null，仅已确认触顶窗口显示 100%；共享前端使用原 DTO |
| E07 | `8075487f57` | 三个结构化 spend-limit code → SpendLimitExceeded → QuotaExhausted + retry prohibited + ProviderDenied；HTTP/WS opening 要 429，流内无状态或429。Retry-After 不变成账号临时冷却；不启用 API Key 产品 |
| E08 | `3039709f36`、`939ae78c8c`、`5f7e50a031`、`baf9992f99`、`037cbcd605`、`89362e54b2` | url/form、cookie、JWT、WS forks、Vue/i18n/lucide 和 install-action 摘要同步；标准库时间接口替代 Host filetime；analytics reset 测试允许空闲轮次 |

## 必要接入与已审差异

- E01 本地 socket 保有账号出口、代理容量、压缩阈值和 interrupt 控制通道。业务 Upload 采用官方双向状态机；小消息低于阈值不压缩，其他路径沿用官方分帧。所有最终排除头值也参与本地 exact-owner 查找，不能仅更新默认构造器。旧故障恢复的“收到1009多次重放直到HTTP”断言已退役。
- E02 官方单 selector 映射到本地立即/等待分层。动态借号决策传至 child follow、队列预算、失败重选和错误封装；等待中及准入前复核主账号资格。借号只依据图片模型/套餐/协议资格；本地 Excel auth protection、额度/冷却/容量不是新增借号理由。原授权宇宙、精确 owner、required 和 Redis generation 保留。
- E03 官方身份 helper 映射到本地 gateway_protocol 的 account session/thread 与 subagent 语义，不从任意客户端正文提升身份；未引入 API Key 模拟账号，测试用模型权限模拟父账号不在候选范围。
- E04 本地手工重授权、库重登、普通RT和换身份沿既有入口判断；只有事务确认不换身份时保留配额。credential/turn-state/warmup revision 仍递增，防旧请求写回；本地 token expiry 与设备唯一性不变。
- E05 本地保留观察时间独立防线及 confirms_zero_usage 合同，更新业务恢复规则而不删除防乱序 CAS。旧 candidates 数值格式可解析并保留 pending 基线，但不充当新版首次证据。缺比例不能恢复；不能仅因旧测试允许缺比例而保留旧规则。
- E05 保留上一基线已验证的重置时间兼容：正整数/数字字符串、有效未来绝对时间及相对秒数。绝对值过期或超出日期范围时继续尝试相对值；普通 Retry-After 不充当配额重置。共享解析器迁移时漏掉的日期范围校验已由既有 `quota_relative_reset_uses_valid_fallback_without_reusing_retry_after` 复现并修正。
- E06 只改额度展示，不把账号访问状态当成每个窗口的计量数值。
- E07 客户端原结构化 code 保留；官方持久化诊断 code 白名单未加入三个 spend code，本地也不扩张 Smart 计分白名单。共享分类可能接收这些上游错误，但不代表接入了 API Key 账号。
- 本地历史升级夹具同步新增 schema：旧 v3.54.0 幂等用例固定到迁移 0099；跨版本升级用例逐项断言新窗口字段默认为 null，再完整比较旧字段。原 WS Close 诊断与 request-scoped 用例加入首文本前导，继续验证原错误、凭据、下一请求及禁止重放；精确续接按官方返回 400/`previous_response_not_found`，保留内部 Ambiguous 发送状态。
- E08 本地 SQLx/Excel/Host 依赖继续采用既有图；官方 frontend lock 只含 pnpm 管理器，故本地用项目 pnpm 11.7.0 更新实际应用 lock。`@lucide/vue` 允许范围解析为 1.78.0；Vue 为3.5.46，i18n为11.4.23。没有复制官方发行索引或启用互联。

## 官方文件逐项映射

| 官方文件 | 处理 | 本地落点与理由 |
| --- | --- | --- |
| `.github/workflows/_quality.yml` | 原样规则/必要接入 | E08：适用依赖/安装器更新，保留现有应用依赖图和平台配置 |
| `.github/workflows/release.yml` | 必要接入 | E08：工具安装器摘要接入本地 build-cache；保留自定义正式发行权限、通道和资产合同 |
| `.github/workflows/security.yml` | 必要接入 | E08：接入本地 security-scan.yml 的安装器摘要 |
| `backend/Cargo.lock` | 原样规则/必要接入 | E08：适用依赖/安装器更新，保留现有应用依赖图和平台配置 |
| `backend/crates/gateway-admin/src/model/provider_credentials.rs` | 原样规则/必要接入 | E04/E05/E06：观测/恢复/展示；保留凭据和观察时钟防线 |
| `backend/crates/gateway-admin/src/use_case/credential_mutation.rs` | 既有实现 | E04：本地 use_case/openai.rs 的提交后额度观测原本不排除停用账号，保留该路径 |
| `backend/crates/gateway-admin/src/use_case/peer_link/upstream/service.rs` | 用户排除 | 互联专属实现/测试，不增加 peer 账户产品能力 |
| `backend/crates/gateway-admin/tests/main.rs` | 原样规则 | E06：真实百分比/未知值展示回归 |
| `backend/crates/gateway-admin/tests/use_case/credentials/peer.rs` | 用户排除 | 互联专属实现/测试，不增加 peer 账户产品能力 |
| `backend/crates/gateway-admin/tests/use_case/peer_link/upstream/mod.rs` | 用户排除 | 互联专属实现/测试，不增加 peer 账户产品能力 |
| `backend/crates/gateway-core/src/account/model.rs` | 必要接入 | E04/E05：现有身份、额度、SQL/CAS 合同接入新字段或 include_disabled 参数 |
| `backend/crates/gateway-core/src/account/store.rs` | 必要接入 | E04/E05：现有身份、额度、SQL/CAS 合同接入新字段或 include_disabled 参数 |
| `backend/crates/gateway-core/src/engine/execution/service.rs` | 必要接入 | E03：本地 engine/execution.rs，使用 RuntimeSnapshot 与现有 execution ports |
| `backend/crates/gateway-core/src/engine/provider.rs` | 必要接入 | E03：本地 root identity / Provider hook 接入 Core 候选范围检查 |
| `backend/crates/gateway-core/tests/engine/execution/mod.rs` | 必要接入 | E03：本地 tests/engine/execution.rs，真实 start 入口测试原模型回退与专用模型 |
| `backend/crates/gateway-host/Cargo.toml` | 原样规则/必要接入 | E08：适用依赖/安装器更新，保留现有应用依赖图和平台配置 |
| `backend/crates/gateway-host/tests/system_update/mod.rs` | 原样规则/必要接入 | E08：依赖接口/标准库适配，保留原断言 |
| `backend/crates/gateway-host/tests/system_update/operations.rs` | 必要接入 | E08：本地 system_update/mod.rs 和 logging/maintenance.rs 使用 std::fs 时间接口 |
| `backend/crates/gateway-store/src/postgres/account_groups.rs` | 必要接入 | E05：account_groups/mod.rs 查询带窗口提示 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/admin_queries.rs` | 必要接入 | E04/E05：现有身份、额度、SQL/CAS 合同接入新字段或 include_disabled 参数 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/core_adapter.rs` | 必要接入 | E04/E05：现有身份、额度、SQL/CAS 合同接入新字段或 include_disabled 参数 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/repository.rs` | 必要接入 | E04/E05：现有身份、额度、SQL/CAS 合同接入新字段或 include_disabled 参数 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/rows.rs` | 必要接入 | E04/E05：现有身份、额度、SQL/CAS 合同接入新字段或 include_disabled 参数 |
| `backend/crates/gateway-store/src/postgres/provider_accounts/runtime.rs` | 必要接入 | E04/E05：现有身份、额度、SQL/CAS 合同接入新字段或 include_disabled 参数 |
| `backend/crates/gateway-store/tests/postgres/analytics_reset.rs` | 原样规则 | E08：允许无进展轮次，完成仍核对实际删除进度 |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/authorization.rs` | 必要接入 | E04/E05：quota_reauthorization.rs 走本地身份事务 API；不引入官方 authorization receipt 产品 |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/core_adapter.rs` | 必要接入 | E04/E05：现有身份、额度、SQL/CAS 合同接入新字段或 include_disabled 参数 |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/mod.rs` | 必要接入 | E04/E05：现有身份、额度、SQL/CAS 合同接入新字段或 include_disabled 参数 |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/quota.rs` | 必要接入 | E05：quota_reauthorization.rs / core_adapter.rs 验证访问时钟与窗口提示；保留本地配额暂停合同 |
| `backend/crates/gateway-store/tests/postgres/proxies.rs` | 原样规则/必要接入 | E08：依赖接口/标准库适配，保留原断言 |
| `backend/crates/providers/openai/Cargo.toml` | 原样规则/必要接入 | E08：适用依赖/安装器更新，保留现有应用依赖图和平台配置 |
| `backend/crates/providers/openai/src/credential/admin.rs` | 用户排除 | 上游 API Key 默认名称、导入表单及其测试；不影响现有 OAuth 套餐筛选 |
| `backend/crates/providers/openai/src/credential/affinity.rs` | 必要接入 | E03：本地 root identity / Provider hook 接入 Core 候选范围检查 |
| `backend/crates/providers/openai/src/credential/catalog/refresh.rs` | 必要接入 | E04：include_disabled 参数贯穿 store、provider worker、选择及测试桩；不同调用者显式选择 |
| `backend/crates/providers/openai/src/credential/mod.rs` | 必要接入 | E05：导出及 fixture 补 limit_window_minutes，不放宽旧凭据合同 |
| `backend/crates/providers/openai/src/credential/quota/bucket_warmup.rs` | 原样规则/必要接入 | E04/E05/E06：观测/恢复/展示；保留凭据和观察时钟防线 |
| `backend/crates/providers/openai/src/credential/quota/recovery.rs` | 原样规则/必要接入 | E04/E05/E06：观测/恢复/展示；保留凭据和观察时钟防线 |
| `backend/crates/providers/openai/src/credential/quota/scheduling.rs` | 必要接入 | E05：本地 quota/mod.rs 的 quota_refresh_candidate 按窗口提示推导复核 reset |
| `backend/crates/providers/openai/src/credential/quota/service.rs` | 必要接入 | E04/E05：本地 quota/mod.rs 的 list、确认耗尽和主动同步 |
| `backend/crates/providers/openai/src/credential/quota/snapshot.rs` | 原样规则/必要接入 | E04/E05/E06：观测/恢复/展示；保留凭据和观察时钟防线 |
| `backend/crates/providers/openai/src/credential/quota/warmup.rs` | 必要接入 | E04：本地 quota/mod.rs 的 enabled-only 预热 |
| `backend/crates/providers/openai/src/credential/repository.rs` | 必要接入 | E04：include_disabled 参数贯穿 store、provider worker、选择及测试桩；不同调用者显式选择 |
| `backend/crates/providers/openai/src/credential/selector.rs` | 必要接入 | E02/E03/E07：本地 selector/{mod,admission,capacity_wait}.rs；动态借号决策贯穿 immediate/wait/CAS/reselect |
| `backend/crates/providers/openai/src/provider/failure.rs` | 必要接入 | E01/E05/E07：从原始错误到 Provider 分类、配额写回、Core 禁重试和客户端错误 |
| `backend/crates/providers/openai/src/provider/mod.rs` | 必要接入 | E03：本地 root identity / Provider hook 接入 Core 候选范围检查 |
| `backend/crates/providers/openai/src/transport/client.rs` | 必要接入 | E01：读取最终 opening headers 形成池身份，覆盖本地 egress 路径 |
| `backend/crates/providers/openai/src/transport/client_stream.rs` | 必要接入 | E01：本地 client_sse.rs 的 early-size 传输选择与连接预算 |
| `backend/crates/providers/openai/src/transport/diagnostics/mod.rs` | 必要接入 | E05/E07：本地 diagnostics.rs；不扩张既有持久化错误码白名单 |
| `backend/crates/providers/openai/src/transport/protocol/responses.rs` | 必要接入 | E01/E05/E07：从原始错误到 Provider 分类、配额写回、Core 禁重试和客户端错误 |
| `backend/crates/providers/openai/src/transport/websocket/control.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/src/transport/websocket/coordinator.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/src/transport/websocket/error.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/src/transport/websocket/exchange/io.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/src/transport/websocket/exchange/stream.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/src/transport/websocket/handshake.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/src/transport/websocket/mod.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/src/transport/websocket/pool/state.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/src/transport/websocket/pump.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/src/transport/websocket/upload.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/tests/credential/admin.rs` | 用户排除 | 上游 API Key 默认名称、导入表单及其测试；不影响现有 OAuth 套餐筛选 |
| `backend/crates/providers/openai/tests/credential/contract/mod.rs` | 必要接入 | E05：导出及 fixture 补 limit_window_minutes，不放宽旧凭据合同 |
| `backend/crates/providers/openai/tests/credential/quota/initial_sync.rs` | 必要接入 | E04：本地 refresh_timing.rs 的禁用账号首次观测与分散期限 |
| `backend/crates/providers/openai/tests/credential/quota/refresh_timing.rs` | 原样规则/必要接入 | E04/E05/E06：观测/恢复/展示；保留凭据和观察时钟防线 |
| `backend/crates/providers/openai/tests/credential/quota/reset_recovery.rs` | 原样规则/必要接入 | E04/E05/E06：观测/恢复/展示；保留凭据和观察时钟防线 |
| `backend/crates/providers/openai/tests/credential/quota/slots.rs` | 原样规则/必要接入 | E04/E05/E06：观测/恢复/展示；保留凭据和观察时钟防线 |
| `backend/crates/providers/openai/tests/provider/contract/guardian_model.rs` | 必要接入 | E03：guardian_routing.rs 和 Core execution 回归，不添加上游 API Key 账号 |
| `backend/crates/providers/openai/tests/provider/contract/image_model_access.rs` | 必要接入 | E02：image_account_eligibility.rs 和 affinity_recovery.rs，分组权限、直接/排队/CAS/负例 |
| `backend/crates/providers/openai/tests/provider/contract/mod.rs` | 必要接入 | E01：OAuth advertised/early 1009、连接预算、后续 WS 与精确续接回归 |
| `backend/crates/providers/openai/tests/provider/contract/quota_usage.rs` | 原样规则/必要接入 | E04/E05/E06：观测/恢复/展示；保留凭据和观察时钟防线 |
| `backend/crates/providers/openai/tests/support.rs` | 必要接入 | E04：include_disabled 参数贯穿 store、provider worker、选择及测试桩；不同调用者显式选择 |
| `backend/crates/providers/openai/tests/transport/diagnostics/mod.rs` | 必要接入 | E05/E07：本地 diagnostics.rs 及 quota_usage.rs 中跨 HTTP/SSE/WS 回归 |
| `backend/crates/providers/openai/tests/transport/latency.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/tests/transport/websocket/diagnostics.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/tests/transport/websocket/fingerprint.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/openai/tests/transport/websocket_pool.rs` | 原样规则/必要接入 | E01：官方读写与错误边界；保留本地压缩、出口、控制帧生命周期；对应回环测试 |
| `backend/crates/providers/xai/Cargo.toml` | 原样规则/必要接入 | E08：适用依赖/安装器更新，保留现有应用依赖图和平台配置 |
| `backend/crates/providers/xai/src/credential/quota.rs` | 必要接入 | E04：本地 credential/catalog.rs 的 refresh_quota 不再拒绝停用账号；目录资格不放宽 |
| `backend/crates/providers/xai/src/credential/repository.rs` | 必要接入 | E04：include_disabled 参数贯穿 store、provider worker、选择及测试桩；不同调用者显式选择 |
| `backend/crates/providers/xai/src/provider/workers.rs` | 必要接入 | E04：include_disabled 参数贯穿 store、provider worker、选择及测试桩；不同调用者显式选择 |
| `backend/crates/providers/xai/tests/admin.rs` | 必要接入 | E04：保留本地 workers/目录拆分，禁用查询用本地 catalog 回归覆盖 |
| `backend/crates/providers/xai/tests/credential/oidc_verifier.rs` | 原样规则/必要接入 | E08：依赖接口/标准库适配，保留原断言 |
| `backend/crates/providers/xai/tests/credential/quota.rs` | 必要接入 | E04：本地 credential/catalog.rs 验证停用账号查询成功仍停用 |
| `backend/crates/providers/xai/tests/support.rs` | 必要接入 | E04：include_disabled 参数贯穿 store、provider worker、选择及测试桩；不同调用者显式选择 |
| `backend/migrations/.frozen-sha256` | 必要接入 | E04/E05：现有身份、额度、SQL/CAS 合同接入新字段或 include_disabled 参数 |
| `backend/migrations/0046_quota_limit_window.sql` | 必要接入 | E05：改用本地下一编号 0100_quota_limit_window.sql；不修改已发布迁移 |
| `docs/api.md` | 必要接入 | E01–E07：更新本地合同，排除 peer/plugin/API Key 产品说明 |
| `docs/architecture.md` | 必要接入 | E01–E05：更新本地调用路径、身份、时钟与主绑定语义 |
| `frontend/package.json` | 原样规则/必要接入 | E08：适用依赖/安装器更新，保留现有应用依赖图和平台配置 |
| `frontend/pnpm-lock.yaml` | 必要接入 | E08：官方该文件只锁包管理器，不能覆盖本地应用 lock；用项目 pnpm 11.7.0 重算完整依赖 |
| `frontend/src/i18n/locales/en/accounts.json` | 用户排除 | 上游 API Key 默认名称、导入表单及其测试；不影响现有 OAuth 套餐筛选 |
| `frontend/src/i18n/locales/zh-Hans/accounts.json` | 用户排除 | 上游 API Key 默认名称、导入表单及其测试；不影响现有 OAuth 套餐筛选 |
| `frontend/src/i18n/locales/zh-Hant/accounts.json` | 用户排除 | 上游 API Key 默认名称、导入表单及其测试；不影响现有 OAuth 套餐筛选 |
| `frontend/src/views/accounts/components/AccountNameField.vue` | 用户排除 | 上游 API Key 默认名称、导入表单及其测试；不影响现有 OAuth 套餐筛选 |
| `frontend/src/views/accounts/components/AccountSettingsFields.vue` | 用户排除 | 上游 API Key 默认名称、导入表单及其测试；不影响现有 OAuth 套餐筛选 |
| `frontend/src/views/accounts/components/account-create-modal/index.vue` | 用户排除 | 上游 API Key 默认名称、导入表单及其测试；不影响现有 OAuth 套餐筛选 |
| `frontend/src/views/accounts/composables/useAccountApiKeyFormRules.ts` | 用户排除 | 上游 API Key 默认名称、导入表单及其测试；不影响现有 OAuth 套餐筛选 |
| `frontend/src/views/accounts/composables/useAccountOnboarding.ts` | 用户排除 | 上游 API Key 默认名称、导入表单及其测试；不影响现有 OAuth 套餐筛选 |
| `release/notes.md` | 必要接入 | 本地 v3.54.5 源码说明，明确包含 tag 后三项修复、迁移和未发布状态 |
| `release/version.yaml` | 必要接入 | 本地版本 3.54.5，published 留空，不复制官方发行通道 |

## 验证记录

已完成的本机验证（用户确认保留结果，后续不再在本机运行）：

- 全量首次运行：API 670、Core 565、Host 145、Egress 133、xAI 396 项集成测试通过；相关库测试通过，Egress 保留 1 项既有环境相关忽略项。App、Admin、Store 和 OpenAI 的失败逐项归因，没有以部分通过作为验收。
- OpenAI 修正旧断言和夹具后，324 项库测试通过；集成 1321 项通过，1 项 IPv6 回环超时。相同二进制的出口模块 7 项串行复验全部通过，未放宽超时或修改实现。新增精确续接支出上限分支另行通过。
- Store 首轮 708 项通过，4 项旧升级夹具未计新增 null 列；修正后相关升级模块 6 项和 v3.54.0 迁移用例 1 项通过。新同身份重授权/窗口时钟 PostgreSQL 用例 2 项在首轮已通过。Redis 独立 ACL 用户模块 121 项通过。
- Admin 旧显示断言已替换，耗尽显示相关 2 项复验通过。App 架构检查因新增私有测试模块失败，该模块已移回既有外部测试；最终复验转 OVH。
- 前端 1186 项测试、ESLint、Vue 类型检查和生产构建通过；最终 workspace 全目标/全特性 Clippy 通过。
- 旧实现失败证据：WS 部分上传超时、Core Guardian 父账号范围断言失败、未匹配配额窗口错误恢复；新实现对应测试通过。相对 reset 日期范围兼容由既有回归先失败、修复后通过。
- 发布 Ruby 六套、Python CI 18 项、部署 49 项通过（2 项既有忽略），隐私工具自身 52 项通过。发布 Python 全套 77 项中 7 项受 macOS 缺少 GNU `realpath -m` 限制，转 OVH Linux 复验；未为此改生产脚本。
- 迁移冻结摘要与升级计划树已核对；候选临时 index 隐私检查通过，最终快照的隐私复验转 OVH。实施阶段的候选检查不替代提交时准确暂存内容及完整发布历史的隐私检查。

OVH Linux 收尾验证：

- 校验上传源码 Git tree 与候选快照一致，使用项目 Rust 1.99.1，验证容器限制为 3 核/7 GiB。
- `cargo fmt --all --check`、App/OpenAI 测试目标编译通过。
- App 59 项架构和启动检查通过。剩余 3 项首次缺少测试连接配置，随后暴露隔离 Redis 的挂载权限问题；补齐独立 PostgreSQL/Redis 并验证就绪后，3 项真实重授权、轮换和同身份重导入测试全部通过。没有修改业务实现或断言。
- OpenAI 29 项关键回归通过，覆盖立即/排队生图借号、CAS 复核、忙碌/停用负例、Guardian 范围、未匹配窗口、旧候选格式、相对 reset、HTTP/SSE/WS 支出上限及精确续接、早期/首文本后 1009、排除头归属与部分上传。
- 发布 Python 全套 77 项通过，确认前述 7 项为本机工具差异。候选快照隐私检查（含原有私有规则）和本次补丁空白检查通过。
- 本轮隔离存储容器与临时凭据已清理；既有服务的容器身份和启动时间前后相同。

原始完成日志留在仓库外。本机测试服务、编译目录、前端依赖/构建输出和本轮临时文件已按用户要求清理；本轮后续手工验证继续使用 OVH，不回到用户本机。

## 升级与未验证边界

新增 `0100_quota_limit_window.sql` 和冻结摘要，`deploy/upgrades/v3.54.5.json` 固定 3.54.3→3.54.5 迁移树。只新增可空列，不改旧迁移；应用失败须人工向前恢复，不跨 schema 自动回滚。生产部署需后续授权及精确 main 提交的 CI/Security/验证镜像。

未执行真实 OpenAI 账号、生产数据回放、多实例新旧二进制混跑或正式发行镜像验收。隔离合成测试不等于真实上游验收；本次适用更新不意味着整个 NewCPR 服务逐字节等同官方或零缺陷。
