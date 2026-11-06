# 官方 CPR v3.41.2 增量代码合入差异账

## 固定范围

- NewCPR 起点：`666b46b38e082dcbab075fe9f4141bb254cfb15b`。
- 官方起点：`b9bcb4e0863c619c913b2ebe4cb3230600fd4bbf`；目标：`e3f1299a0cc210b0af55b1a86d67b2e1db85229e`。
- 共 18 个官方提交、85 个增量路径。使用实际官方补丁/文件与本地 owner 接入；两仓库没有共同 Git ancestor，不合并 unrelated histories。
- 本文是工作区代码审阅与验证记录，不代表提交、推送、发布或生产部署。

## 逐提交

| 提交 | 内容 | 处理 |
| --- | --- | --- |
| `272534a3b` | 启动/重启 | startup_arguments、stdin EOF handoff、先释放锁再交接；原生 gateway 入口同步。 |
| `37749001d` | CI action | 更新质量/安全流程 install-action 引用。 |
| `f7f64ead3` | 前端依赖 | 按官方本轮依赖更新，lock 同步。 |
| `2dc6bf015` | 后端依赖 | Cargo.lock 官方补丁。 |
| `26dbc1201` | 模型全名 | 选项截断与完整悬浮提示。 |
| `1d88c9a4a` | WS 生命周期 | 忽略非终结控制帧；空闲上游断开通知；结算与 queued 请求保护。 |
| `c73e7126a` | OAuth | 连接/总超时与 Connection/Ambiguous/Rejected 分类及安全提示。 |
| `3e33e84a4` | 统计统一 | 最终请求 outcome 分母、非成功数、可空健康比率贯穿 Store/Admin/API/UI。 |
| `0c07aac1d` | 官方 v3.41.1 元数据 | 不覆盖 NewCPR 版本与发布说明。 |
| `2b7a64dae` | 开发证据规则 | 由本地现有合并核对清单及本差异账落实，不覆盖项目 AGENTS/skills。 |
| `4527db7b5` | 取消/未完成展示 | 健康卡、诊断卡和多语言分别显示 cancelled/incomplete。 |
| `1655ecc97` | 提示格式 | 成功率 tooltip 格式。 |
| `04a48e848` | mio | 锁定 1.2.4。 |
| `09b22a955` | 目录外模型 | 明确 allowlist 精确命中允许绕过目录缺失，冻结授权和其他资格仍先检查。 |
| `ba3baddbd` | 官方 v3.41.2 元数据 | 不覆盖 NewCPR 版本与发布说明。 |
| `6de1beb1d` | 调度/绑定 | scoped store、批量 Redis、完整投影缓存、三模式、准入 CAS、稳定 spillover、持久失格迁移及回归。 |
| `4e4cd9647` | 等待跨账号复查 | 可移动等待每 500 ms 重查；固定 owner/严格已有绑定不跨号；目的业务池 FIFO。 |
| `e3f1299a0` | 自定义模型编辑 | 保存前保留取消的自定义候选，可重新选择。 |

## 必要差异与边界

1. **用户明确的严格模式要求**：已绑定 A 仅因并发/请求间隔阻塞不得改用 B，包括关闭普通等待开关时。Core 获准重试并排除 A 后，合法 B 在发送前 CAS 成为主绑定。旧 A 完成不改回主绑定。官方关闭普通等待路径与本需求存在差异，此处以用户明确要求为来源。
2. **严格子线程**：沿官方 follow-only 合同，跟随根绑定；它本身报错不独立迁移根。根迁移后跟随新账号。固定 native owner 冲突要求完整重放，不伪造续接。
3. **Preferred**：暂时满载、间隔、冷却、模型限制及接近额度恢复时保留主账号；永久失格或未知/远期/过期额度 reset 才迁移。使用官方 session hash 稳定备用选择。
4. **Relaxed**：共用根绑定并在准入时迁移；不沿用旧本地独立 child 主绑定。三个模式均不在完成回调回写主账号。turn 别名按官方保存根会话及独立于模式的 follow-only 标记；同根不同 child 可复用 turn，跨根拒绝。旧 child/root 记录按 root 解析并原样续期；各线程传输身份和真实续接归属单独校验。
5. **本地真实 owner**：继续由 Redis 管理等待/执行 token、deadline、续期和取消；Shared/Reserved 各自 FIFO，Quality 独立。全池重查不能越过目的账号更早的等待者；不增加第二套内存队列。
6. **实时安全校验**：缓存只复用候选投影。保留本地 post-acquire revision/live 容量/凭据终检；route、path、overdraft 等加入缓存 key。额外读取有明确用途，不为了匹配官方测试次数删除。
7. **统计和备份**：按本地 raw SQL 与恢复小时基线接入官方最终 outcome 定义；老备份已丢失请求明细时无法反推重分组，不能编造历史取消分类。
8. **不存在的产品代码**：不引入官方插件、peer_link 或 APIKey 产品；同文件中与原生 OAuth/协议相关的改动独立迁入。排除项见逐文件表。
9. **不保留旧错误适配**：去掉 WebSocket 回池时无条件 revoke_control，保留实际下一 owner checkout 与退休时撤销；否则官方终态后控制功能失效。OAuth 读取响应的超时保留 reqwest 来源链，不能经旧 RefreshFailure 字符串丢失。冻结候选以请求 scope 为授权边界，不能因 200 ms 缓存未含恢复账号而永久丢失合法替代者。
10. **界面适配**：官方组件映射现有 Base 组件和分页目录；新增 `BaseCheckbox` 最小宽度约束，真实手机实测修复长模型名撑宽页面。

## 逐文件

路径为仓库相对路径；“补丁合入”不声称整个文件与官方逐字相同。

| # | 官方增量路径 | 处理 / 本地 owner | 核对说明 |
| --- | --- | --- | --- |
| 1 | `.agents/skills/cpr-dev-guide/SKILL.md` | 排除工具配置；`本地 Trellis 工作流` | 不覆盖项目工作流；本次保留证据规则由既有 upstream-integration-checklist 执行。 |
| 2 | `.github/workflows/_quality.yml` | 补丁合入；`.github/workflows/_quality.yml` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 3 | `.github/workflows/release.yml` | 接入现有 owner；`.github/workflows/_quality.yml` | 官方仅更新 install-action；本地发布调用共用 quality，不引入第二套发布流程。 |
| 4 | `.github/workflows/security.yml` | 接入现有 owner；`.github/workflows/security-scan.yml` | 同步 action 引用，保留本地安全扫描流程。 |
| 5 | `AGENTS.md` | 排除工具配置；`AGENTS.md` | 不覆盖 NewCPR 目标、隐私和发布授权规则；官方运行时代码另行合入。 |
| 6 | `backend/Cargo.lock` | 补丁合入；`backend/Cargo.lock` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 7 | `backend/apps/gateway/src/bootstrap/startup.rs` | 原生部分接入；`backend/apps/gateway/src/main.rs + gateway-host::system_update` | 启动参数与 handoff 已接入；本地不存在的插件元数据 warmup 不引入插件系统。 |
| 8 | `backend/crates/gateway-admin/src/model/observability.rs` | 必要接入；`backend/crates/gateway-admin/src/model/observability.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 9 | `backend/crates/gateway-admin/src/use_case/observability/mod.rs` | 必要接入；`backend/crates/gateway-admin/src/use_case/observability/mod.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 10 | `backend/crates/gateway-admin/tests/use_case/observability/mod.rs` | 必要接入；`backend/crates/gateway-admin/tests/use_case/observability/mod.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 11 | `backend/crates/gateway-admin/tests/use_case/peer_link/upstream/mod.rs` | 无对应产品 owner；`ProviderAccountStore 的本地测试实现` | 官方此处仅适配 scoped store trait；本地无 peer_link，所有本地 trait 实现随编译检查。 |
| 12 | `backend/crates/gateway-api/src/admin/observability/presenter.rs` | 必要接入；`backend/crates/gateway-api/src/admin/observability/presenter.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 13 | `backend/crates/gateway-api/src/admin/observability/wire.rs` | 补丁合入；`backend/crates/gateway-api/src/admin/observability/wire.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 14 | `backend/crates/gateway-api/src/openai/responses/websocket/forward.rs` | 必要接入；`backend/crates/gateway-api/src/openai/responses/websocket/forward.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 15 | `backend/crates/gateway-api/src/openai/responses/websocket/mod.rs` | 必要接入；`backend/crates/gateway-api/src/openai/responses/websocket/mod.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 16 | `backend/crates/gateway-api/tests/admin/observability/response.rs` | 必要接入；`backend/crates/gateway-api/tests/admin/observability/response.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 17 | `backend/crates/gateway-api/tests/openai/responses/websocket/interrupt.rs` | 必要接入；`backend/crates/gateway-api/tests/openai/responses/websocket/interrupt.rs` | 官方失效、queued 请求保留测试；本地结算先于 completed 交付，gate 释放后读 completed，所有失效与队列断言保留。 |
| 18 | `backend/crates/gateway-core/src/account/store.rs` | 必要接入；`backend/crates/gateway-core/src/account/store.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 19 | `backend/crates/gateway-core/src/concurrency.rs` | 接入现有 owner；`gateway-store/src/redis/capacity_wait.rs + openai/credential/selector/capacity_wait.rs` | 500 ms 跨账号复查与目的队首保护，复用 Redis 等待/执行租约；不引入第二套进程内队列。 |
| 20 | `backend/crates/gateway-core/src/provider_ports/cooldown.rs` | 接入现有 owner；`gateway-core/src/provider_ports/mod.rs` | read_many/read_scoped_many 和本地 protection batch，保留逐项错误。 |
| 21 | `backend/crates/gateway-core/tests/concurrency.rs` | 接入现有 owner；`gateway-store/tests/redis/capacity_wait.rs + openai/tests/credential/contract/scheduling_projection.rs` | 真实 Redis 目的池 FIFO、取消/到期、隔离；Provider 可移动等待重查。 |
| 22 | `backend/crates/gateway-core/tests/engine/account_protection.rs` | 补丁合入；`backend/crates/gateway-core/tests/engine/account_protection.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 23 | `backend/crates/gateway-host/src/serve.rs` | 必要接入；`backend/crates/gateway-host/src/serve.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 24 | `backend/crates/gateway-host/src/system_update/process.rs` | 必要接入；`backend/crates/gateway-host/src/system_update/process.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 25 | `backend/crates/gateway-host/tests/system_update/operations.rs` | 接入现有 owner；`gateway-host/tests/system_update/mod.rs` | 启动参数、handoff 标记及既有更新路径回归。 |
| 26 | `backend/crates/gateway-store/src/postgres/observability/analytics/aggregate.rs` | 接入现有 owner；`gateway-store/src/postgres/observability/queries/{usage,dashboard,restored}.rs` | 本地使用原始请求 SQL 与恢复小时基线；同步 failed/incomplete/cancelled/caller 分类。 |
| 27 | `backend/crates/gateway-store/src/postgres/observability/analytics/metrics.rs` | 接入现有 owner；`gateway-admin/src/model/observability.rs + gateway-store/src/postgres/observability/model.rs` | 统一 RequestHealth 计算和 Option 比率；保留本地成本维度。 |
| 28 | `backend/crates/gateway-store/src/postgres/observability/analytics/query.rs` | 接入现有 owner；`gateway-store/src/postgres/observability/queries/usage.rs` | 诊断排序与 error_count 包括 incomplete；null 比率无样本。 |
| 29 | `backend/crates/gateway-store/src/postgres/observability/mapping.rs` | 补丁合入；`backend/crates/gateway-store/src/postgres/observability/mapping.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 30 | `backend/crates/gateway-store/src/postgres/observability/model.rs` | 补丁合入；`backend/crates/gateway-store/src/postgres/observability/model.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 31 | `backend/crates/gateway-store/src/postgres/provider_accounts/core_adapter.rs` | 补丁合入；`backend/crates/gateway-store/src/postgres/provider_accounts/core_adapter.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 32 | `backend/crates/gateway-store/src/postgres/provider_accounts/repository.rs` | 必要接入；`backend/crates/gateway-store/src/postgres/provider_accounts/repository.rs` | 真实 ANY(text[]) scoped SQL，保留本地字段与账号身份/出口合同。 |
| 33 | `backend/crates/gateway-store/src/redis/account_error_protection.rs` | 必要接入；`backend/crates/gateway-store/src/redis/account_error_protection.rs` | 本地独立保护 owner 增加 128 分批 EVALSHA，保留 revision、probe 和 generation 合同。 |
| 34 | `backend/crates/gateway-store/src/redis/credential_cooldown.rs` | 必要接入；`backend/crates/gateway-store/src/redis/credential_cooldown.rs` | 官方 pipeline/EVALSHA 128 分批、逐项错误、NOSCRIPT 局部重试，接入本地 scoped/protection 端口。 |
| 35 | `backend/crates/gateway-store/tests/postgres/analytics.rs` | 接入现有 owner；`gateway-store/tests/postgres/observability/mod.rs` | 本地 raw SQL 的最终 outcome 统计与 API/Admin 测试，非凭空新增聚合器。 |
| 36 | `backend/crates/gateway-store/tests/postgres/observability/mod.rs` | 必要接入；`backend/crates/gateway-store/tests/postgres/observability/mod.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 37 | `backend/crates/gateway-store/tests/postgres/provider_accounts/core_adapter.rs` | 必要接入；`backend/crates/gateway-store/tests/postgres/provider_accounts/core_adapter.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 38 | `backend/crates/gateway-store/tests/postgres/provider_accounts/mod.rs` | 必要接入；`backend/crates/gateway-store/tests/postgres/provider_accounts/mod.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 39 | `backend/crates/gateway-store/tests/redis/credential_cooldown.rs` | 补丁合入；`backend/crates/gateway-store/tests/redis/credential_cooldown.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 40 | `backend/crates/providers/openai/src/admin/mod.rs` | 接入现有 owner；`backend/crates/providers/openai/src/admin.rs` | OAuth 分类、安全错误文案和 source 链。 |
| 41 | `backend/crates/providers/openai/src/credential/mod.rs` | 必要接入；`backend/crates/providers/openai/src/credential/mod.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 42 | `backend/crates/providers/openai/src/credential/oauth.rs` | 必要接入；`backend/crates/providers/openai/src/credential/oauth.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 43 | `backend/crates/providers/openai/src/credential/quota/service.rs` | 已无旧代码；`本地 quota 实现` | 官方删除单账号 cooldown helper；本地没有该 helper，批量读取在 selector 接入。 |
| 44 | `backend/crates/providers/openai/src/credential/repository.rs` | 补丁合入；`backend/crates/providers/openai/src/credential/repository.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 45 | `backend/crates/providers/openai/src/credential/scheduling_projection.rs` | 必要接入；`backend/crates/providers/openai/src/credential/scheduling_projection.rs` | 官方 200 ms / 32 项 / 8192 候选及 fill 合并；key 额外含本地 route、path、overdraft 等资格字段，防跨合同复用。 |
| 46 | `backend/crates/providers/openai/src/credential/selector.rs` | 拆分接入；`openai/src/credential/selector/{mod,admission,projection,capacity_wait,model_eligibility}.rs` | 实际 scoped reads、缓存、准入 CAS、稳定 spillover、持久失格迁移和排队复查逻辑沿本地执行 owner 接入。 |
| 47 | `backend/crates/providers/openai/src/credential/token_client.rs` | 必要接入；`backend/crates/providers/openai/src/credential/token_client.rs` | 连接 30 s/总 60 s 与官方错误分类；保留已验证的 TokenClientCache、出口和身份分区。 |
| 48 | `backend/crates/providers/openai/src/transport/websocket/control.rs` | 必要接入；`backend/crates/providers/openai/src/transport/websocket/control.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 49 | `backend/crates/providers/openai/tests/admin/mod.rs` | 接入现有 owner；`backend/crates/providers/openai/tests/admin.rs` | OAuth 错误文案、来源及既有本地管理合同。 |
| 50 | `backend/crates/providers/openai/tests/credential/contract/mod.rs` | 接入现有 owner；`backend/crates/providers/openai/tests/credential/contract/mod.rs` | 官方新 projection 模块注册；原 contract.rs 移入目录以遵循本地模块规范；新增本地严格满载/授权重试回归。 |
| 51 | `backend/crates/providers/openai/tests/credential/contract/scheduling_projection.rs` | 必要接入；`backend/crates/providers/openai/tests/credential/contract/scheduling_projection.rs` | 真实官方测试移植；本地执行后 revision fence 额外读取不可删，计数按本地安全合同核对；增加本地 key 隔离。 |
| 52 | `backend/crates/providers/openai/tests/credential/oauth.rs` | 补丁合入；`backend/crates/providers/openai/tests/credential/oauth.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 53 | `backend/crates/providers/openai/tests/credential/token_client.rs` | 补丁合入；`backend/crates/providers/openai/tests/credential/token_client.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 54 | `backend/crates/providers/openai/tests/provider/contract/mod.rs` | 必要接入；`backend/crates/providers/openai/tests/provider/contract/mod.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 55 | `backend/crates/providers/openai/tests/provider/contract/model_account_eligibility.rs` | 必要接入；`backend/crates/providers/openai/tests/provider/contract/model_account_eligibility.rs` | 官方 OAuth allowlist 组合；本地不存在官方 APIKeyCredentialData/ResponsesTransport 产品分支，不凭此引入 APIKey 产品。 |
| 56 | `backend/crates/providers/openai/tests/provider/contract/response_interrupt/mod.rs` | 真实测试移植；`openai/tests/provider/contract/response_control_lifecycle.rs` | 复制官方 lifecycle 测试及 helper，适配本地 &self execute 和合成账号 fixture。 |
| 57 | `backend/crates/providers/openai/tests/provider/contract/session_binding.rs` | 接入现有测试 owner；`openai/tests/provider/contract/{affinity_modes,affinity_recovery,upstream_affinity}.rs` | 三模式、永久/暂时失格、稳定 spillover、并发迁移和严格跟随。 |
| 58 | `backend/crates/providers/openai/tests/support.rs` | 必要接入；`backend/crates/providers/openai/tests/support.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 59 | `backend/crates/providers/xai/tests/admin.rs` | 补丁合入；`backend/crates/providers/xai/tests/admin.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 60 | `backend/crates/providers/xai/tests/support.rs` | 补丁合入；`backend/crates/providers/xai/tests/support.rs` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 61 | `docs/api.md` | 必要接入；`docs/api.md` | 更新现行接口、三模式、统计空值与模型白名单合同，删除已失效 State gate 描述。 |
| 62 | `docs/architecture.md` | 必要接入；`docs/architecture.md` | 更新实际 owner、准入/完成边界、缓存、队列、WS 和 handoff；保留本地独立容量池。 |
| 63 | `frontend/package.json` | 必要接入；`frontend/package.json` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 64 | `frontend/pnpm-lock.yaml` | 必要接入；`frontend/pnpm-lock.yaml` | 锁回本轮声明的 lucide 1.67.0；保留本地其余依赖，新增传递依赖来自本轮升级。 |
| 65 | `frontend/src/api/modules/usage.ts` | 补丁合入；`frontend/src/api/modules/usage.ts` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 66 | `frontend/src/i18n/locales/en/accounts.json` | 必要接入；`frontend/src/i18n/locales/en/accounts.json` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 67 | `frontend/src/i18n/locales/en/dashboard.json` | 补丁合入；`frontend/src/i18n/locales/en/dashboard.json` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 68 | `frontend/src/i18n/locales/en/settings.json` | 补丁合入；`frontend/src/i18n/locales/en/settings.json` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 69 | `frontend/src/i18n/locales/en/usage.json` | 补丁合入；`frontend/src/i18n/locales/en/usage.json` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 70 | `frontend/src/i18n/locales/zh-Hans/accounts.json` | 必要接入；`frontend/src/i18n/locales/zh-Hans/accounts.json` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 71 | `frontend/src/i18n/locales/zh-Hans/settings.json` | 补丁合入；`frontend/src/i18n/locales/zh-Hans/settings.json` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 72 | `frontend/src/i18n/locales/zh-Hans/usage.json` | 补丁合入；`frontend/src/i18n/locales/zh-Hans/usage.json` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 73 | `frontend/src/i18n/locales/zh-Hant/accounts.json` | 必要接入；`frontend/src/i18n/locales/zh-Hant/accounts.json` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 74 | `frontend/src/i18n/locales/zh-Hant/settings.json` | 补丁合入；`frontend/src/i18n/locales/zh-Hant/settings.json` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 75 | `frontend/src/i18n/locales/zh-Hant/usage.json` | 补丁合入；`frontend/src/i18n/locales/zh-Hant/usage.json` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 76 | `frontend/src/views/accounts/components/AccountEditModal.vue` | 必要接入；`frontend/src/views/accounts/components/AccountEditModal.vue` | 模型选项新组件接入本地编辑器，保留现有其他账户设置。 |
| 77 | `frontend/src/views/accounts/components/AccountModelAccessField.vue` | 必要接入；`frontend/src/views/accounts/components/AccountModelAccessField.vue` | 官方分组、筛选、全选、手填与取消保留；适配 Base 组件和既有分页目录。 |
| 78 | `frontend/src/views/accounts/components/AccountModelOption.vue` | 必要接入；`frontend/src/views/accounts/components/AccountModelOption.vue` | 官方全名悬浮与截断组件，Z 组件映射为现有 Base 组件。 |
| 79 | `frontend/src/views/accounts/composables/useAccountConnectionTest.ts` | 必要接入；`frontend/src/views/accounts/composables/useAccountConnectionTest.ts` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 80 | `frontend/src/views/dashboard/composables/useRequestTrendChart.ts` | 必要接入；`frontend/src/views/dashboard/composables/useRequestTrendChart.ts` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 81 | `frontend/src/views/dashboard/presenter.ts` | 接入现有 owner；`frontend/src/views/dashboard/composables/useDashboard.ts` | success_rate 的 null/空值透传，不将无有效样本显示成真实 0%。 |
| 82 | `frontend/src/views/usage/components/UsageDiagnosticCard.vue` | 必要接入；`frontend/src/views/usage/components/UsageDiagnosticCard.vue` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 83 | `frontend/src/views/usage/components/UsageHealthCard.vue` | 必要接入；`frontend/src/views/usage/components/UsageHealthCard.vue` | 官方本轮实际补丁接入同路径；本地其余合同保持，见相关调用链与验证。 |
| 84 | `release/notes.md` | 排除发布元数据；`本合并差异账` | 未授权发布；不把官方发布说明覆盖为 NewCPR 发布说明。 |
| 85 | `release/version.yaml` | 排除发布元数据；`NewCPR 现有版本` | 保留 NewCPR 版本；本次并未发布。 |

## 入口、调用与错误边界

| 功能 | 入口 → helper → caller/出口 | 关键错误/回归 |
| --- | --- | --- |
| 绑定与缓存 | Provider execution → credential selector/projection/admission → scoped store / live lease / affinity CAS → transport；observation 仅记录健康与 Guardian parent | Strict 满载不换、预算允许重试才换、准入前绑定、新代次赢、旧完成不回绑、严格 child 跟随共享截止 |
| 等待 | select_with_capacity_wait → wait_for_account / reload_wait_pool → Redis try_acquire/try_promote | 500 ms 重查、固定 owner 不跨号、FIFO、取消与到期清理、终检释放、原 deadline 不重置 |
| 批量读取 | FrozenAccountScope → scoped Store/PG ANY → cooldown/scoped/protection 128 分批 EVALSHA | 顺序/重复 ID、逐项错误、NOSCRIPT、revision/probe 保留；缓存争用与失效重载 |
| OAuth | admin/import/refresh → token client → OAuth error mapping | connect30/total60、超时/连接/模糊结果/拒绝区别；不暴露响应凭据 |
| WS | PumpControl.receive → provider lifecycle → API forward/idle loop | 非文本控制帧不中断、terminal 后控制保留、复用撤销旧control、结算前不能丢 queued 请求 |
| 健康统计 | raw PG/backup hour → RequestHealth → Admin → API wire → Usage/Dashboard | cancelled/caller 排除 eligible、incomplete 计非成功、0 样本 null，成本与历史备份边界 |
| 模型 | FrozenAccountScope/explicit allowlist → selector eligibility → editor/catalog | 目录外精确 allowlist、权限先行、自定义取消保留/全选/新 ID、窄屏及完整悬浮 |
| 重启 | gateway main → startup_arguments/serve → system_update process | --restart-handoff、stdin EOF、先释放锁再交接；保留本地受控发布入口 |

## 验证状态

### 修复前后对照

在隔离源码副本中恢复基线 `666b46b38` 的下列真实实现，保留同一批新增测试。
回退版成功编译后，3 项行为断言全部失败；还原合入代码后，同 3 项全部通过。
没有把编译失败算作回归证据，验证后源码已还原为合入版本。

| 真实回退范围 | 不变的测试 | 修复前 → 修复后 |
| --- | --- | --- |
| `PumpControl::receive` 与 `put_reserved` 的基线实现 | `response_controls_use_the_original_socket_through_terminal_and_revoke_on_reuse` | 失败 → 通过；终态后的控制及复用撤销可实际观察 |
| `retain_model_eligible` 的基线函数 | `account_allowlists_override_catalog_omissions_but_weight_affinity_and_pinning_do_not` | 失败 → 通过；精确白名单、权重、亲和与指定账号边界保持完整 |
| direct selector 原有的满载重选分支，移除本次新增的两处 Strict 满载保护 | `strict_busy_owner_stays_bound_and_authorized_retry_migrates_before_send` | 失败 → 通过；满载不换、合法重试准入迁移、迟到完成不回绑 |

Strict 并发与间隔同时阻塞由
`strict_interval_wait_keeps_the_owner_and_shared_deadline` 验证，属于本地明确保留的
请求间隔合同。原先把同根不同 child 复用 turn 判为身份冲突，实际是旧测试将旧的
独立 child 绑定规则带入新实现，不能归类为官方缺陷；已按下述复核修正。

### 最新官方复核与旧断言纠正

2026-11-06 重新从官方 main 获取 `e390417821f676e02a705aec3754b8d458c0da4b`
（提交时间 12:01:01 UTC）。它比本次固定合入范围多一条无显式会话请求的缓存偏好
提交。本节以它核对严格模式及 turn 合同，不表示该新增功能已包含在前述 18 条合入中。

| 场景 | 最新官方 | 本地已提交版本 `ff168afdc` |
| --- | --- | --- |
| 普通根会话 A 并发满、开启排队 | 等 A；队满或超时失败 | 相同；空闲 B 不改变正在等待的严格绑定 |
| 普通根会话 A 并发满、关闭排队 | 可选择 B；准入时迁移绑定 | 额外拒绝换号，可能有空闲 B 仍返回容量不足 |
| 获准错误重试且 A 已被排除 | 可准入 B 并 CAS 绑定，旧 A 完成不回绑 | 相同；重试种类和次数仍由 Core 控制 |
| 严格子线程、已有根绑定 | 只跟随根；关闭普通排队也有界等待 | 相同 |
| required/native owner | 固定指定或续接 owner，不借忙跨号 | 相同 |
| 账号请求间隔 | 当前官方选择器没有本地 RequestInterval 合同 | 本地扩展，严格绑定仍等原账号；不能称为官方要求 |
| 同根不同子线程复用 turn | 按根会话兼容，传输身份单独隔离 | 原先额外拒绝；本轮撤销该限制并同步旧测试 |

turn 修正直接依据官方 `credential/affinity.rs::{from_turn_alias,alias_record}`、
`credential/selector.rs::remember_turn` 和 `provider/execution.rs` 的显式/推断根键比较。
本地删除 child 身份拒绝分支，新别名登记根键，历史别名保留原记录续期。
回归 `child_turn_alias_accepts_siblings_across_modes_but_rejects_another_root`
覆盖三个模式的 Responses 与图片入口同根复用，并保留跨根负例。相同新版测试在
`ff168afdc` 实现下成功编译后，以 `InvalidRequest` 失败，证明旧限制实际阻止请求。
修正后同一用例通过。OpenAI 全部 1,215 项集成测试已覆盖：整包先通过 1,214 项，
另 1 项 IPv6 出口测试在补齐隔离容器的 `::1 localhost` 解析后定向通过；没有修改
出口测试或生产配置来取得通过结果。线程隔离、严格子线程跟随和真实续接归属回归保留。
本轮 Provider 的 all-targets/all-features Clippy（`-D warnings`）、格式及 diff 检查通过。

测试夹具修正均有独立原因：冻结 scope 不预先授权后续才导入的账号；旧账号
turn-state 携带真实 `account_state_owner`；共享根绑定计数与 Guardian 成功记录分别断言；
队列压力模拟维护实际存活等待者；纯资格/队列测试排除请求间隔对准备步骤的干扰。
隔离验证容器补齐本地 IPv6 回环解析及匹配 PostgreSQL 16 的备份客户端。
原 fast-acquire 测试要求业务请求越过已有等待者，与本次 FIFO 合同冲突；现验证
队列未清空时业务准入为 Busy、Quality 保持独立容量、释放队列后准入成功，最后
释放执行槽。清理线程只在全部准入断言完成后关闭。

### 检查结果

- 前端：1,155 项 Node 测试通过；eslint、vue-tsc 和 Vite build 通过。隔离浏览器实测桌面/手机模型编辑、长名悬浮与健康空值通过。
- 后端：workspace/all-targets/all-features cargo check 已通过；全 workspace/all-targets/all-features Clippy（`-D warnings`）与格式检查通过。全工作区一次性链接曾因隔离机磁盘不足中止，最终采用下表逐包 all-features 验证，不能视作单次 workspace test 成功。
- 9 个包共 4,468 项测试通过；1 项原有 ignored 未执行，见下表。Provider 包包括严格绑定不跨号、合法重试迁移、可移动等待发现空闲账号以及取消清理。
- Store 的 640 项集成测试在全包运行中通过，另 3 项单独复测通过。两项备份测试使用独立临时空间，未降低 1 GiB 剩余空间保护；真实导出/恢复断言均执行。模块目录修正后，主程序全部 62 项集成测试重新通过。
- 上述 3 项真实代码回退的 failing-before / passing-after 对照已通过。
- CI/toolchain/规划测试 26 项、隐私检查自测 48 项通过。
- 最终模块移动后 Provider contract 98 项补测通过；全工作区 Clippy 再次通过。后端本地与隔离验证源码 checksum 一致；临时 index 的精确待提交内容隐私扫描通过，真实 index 未改动。
- 无真实上游账号业务请求，无生产部署；测试用合成账号与独立数据服务。

| 包 | 单元测试通过 | 集成测试通过 | 备注 |
| --- | ---: | ---: | --- |
| `provider-openai` | 329 | 1,215 | 三种模式、排队、缓存、OAuth、WS 和模型资格 |
| `gateway-store` | 12 | 643 | 640 + 3 分批通过；真实 PostgreSQL/Redis 与备份客户端 |
| `gateway-api` | 2 | 639 | HTTP/WS、管理接口与统计 |
| `gateway-admin` | 9 | 405 | 管理用例、统计与配置 |
| `gateway-core` | 8 | 527 | 重试次数、禁止重试、预算与调度 |
| `gateway-host` | 23 | 113 | 另有 1 项原有 ignored：实际 Mihomo 内核与 Linux namespace 测试，未执行 |
| `gateway-protocol` | 0 | 90 | 协议合同 |
| `provider-xai` | 0 | 391 | 共用接口和既有行为 |
| `codex-proxy-rs` | 0 | 62 | 启动、架构及模块布局 |
