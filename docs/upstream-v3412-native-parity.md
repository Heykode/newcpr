# 官方 CPR v3.41.2 增量代码合入差异账

## 固定范围

- NewCPR 起点：`666b46b38e082dcbab075fe9f4141bb254cfb15b`。
- 官方起点：`b9bcb4e0863c619c913b2ebe4cb3230600fd4bbf`；目标：`1240f22580eaa8b4009f6ca3e9b2a15c07916d5f`。
- 共 20 个官方提交、89 个增量路径。使用实际官方补丁/文件与本地 owner 接入；两仓库没有共同 Git ancestor，不合并 unrelated histories。
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
| `4e4cd9647` | 等待跨账号复查 | 可移动等待每 500 ms 重查；固定 owner/已进入严格并发等待的绑定不跨号；目的业务池 FIFO。 |
| `e3f1299a0` | 自定义模型编辑 | 保存前保留取消的自定义候选，可重新选择。 |
| `e39041782` | 无显式会话缓存偏好 | 实际哈希、优先级、Provider 调用链、选号器存储降级与状态隔离合入。 |
| `1240f2258` | 缓存偏好遵循模式 | Strict 等待/迁移、Preferred 分流、Relaxed 跳过、存储失败边界及官方回归合入。 |

## 必要差异与边界

1. **严格模式按官方对齐**：开启等待时，已有绑定 A 并发满只等 A；关闭等待可选择合格 B 并在发送前 CAS 迁移绑定。获准错误重试排除 A 后也可迁移；旧 A 完成不回绑。本地请求间隔配置和原子校验保留，但仅间隔阻塞不额外禁止根请求改选。
2. **严格子线程**：沿官方 follow-only 合同，跟随根绑定；它本身报错不独立迁移根。根迁移后跟随新账号。固定 native owner 冲突要求完整重放，不伪造续接。
3. **Preferred**：暂时满载、间隔、冷却、模型限制及接近额度恢复时保留主账号；永久失格或未知/远期/过期额度 reset 才迁移。使用官方 session hash 稳定备用选择。
4. **Relaxed**：共用根绑定并在准入时迁移；不沿用旧本地独立 child 主绑定。三个模式均不在完成回调回写主账号。turn 别名按官方保存根会话及独立于模式的 follow-only 标记；同根不同 child 可复用 turn，跨根拒绝。旧 child/root 记录按 root 解析并原样续期；Responses 还核对原始 follow-only 身份，根/子角色变化拒绝；图片等独立端点按根键匹配并合并跟随约束。各线程传输身份和真实续接归属单独校验。
5. **本地真实 owner**：继续由 Redis 管理等待/执行 token、deadline、续期和取消；Shared/Reserved 各自 FIFO，Quality 独立。全池重查不能越过目的账号更早的等待者；不增加第二套内存队列。
6. **实时安全校验**：缓存只复用候选投影。保留本地 post-acquire revision/live 容量/凭据终检；route、path、overdraft 等加入缓存 key。额外读取有明确用途，不为了匹配官方测试次数删除。候选授权范围按官方 FrozenAccountScope 重载；修复的是本地旧适配将缓存内容误当完整范围的问题，不是新增官方之外的限制。
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

| 86 | `backend/crates/providers/openai/src/credential/affinity.rs` | 补丁与必要接入；同路径 | 官方缓存偏好哈希和命名空间，真实会话与传输身份分开。 |
| 87 | `backend/crates/providers/openai/src/provider/mod.rs` | 补丁与必要接入；同路径 | 缓存偏好只参与选号，真实会话、续接与 Guardian 优先。 |
| 88 | `backend/crates/providers/openai/src/transport/request.rs` | 接入现有 owner；`backend/crates/providers/openai/src/transport/request/mod.rs` | 官方 message_text_parts 与逻辑账号身份解析，保留官方传输头优先级。 |
| 89 | `backend/crates/providers/openai/tests/provider/contract/cache_preference.rs` | 官方测试接入；同路径 | 键边界、模式、存储超时和排队生命周期回归。 |

## 入口、调用与错误边界

| 功能 | 入口 → helper → caller/出口 | 关键错误/回归 |
| --- | --- | --- |
| 绑定与缓存 | Provider execution → credential selector/projection/admission → scoped store / live lease / affinity CAS → transport；observation 仅记录健康与 Guardian parent | Strict 开排队满载等待、关排队可迁移、预算允许重试可迁移、准入前绑定、新代次赢、旧完成不回绑、严格 child 跟随共享截止 |
| 等待 | select_with_capacity_wait → wait_for_account / reload_wait_pool → Redis try_acquire/try_promote | 500 ms 重查、固定 owner 不跨号、FIFO、取消与到期清理、终检释放、原 deadline 不重置 |
| 批量读取 | FrozenAccountScope → scoped Store/PG ANY → cooldown/scoped/protection 128 分批 EVALSHA | 顺序/重复 ID、逐项错误、NOSCRIPT、revision/probe 保留；缓存争用与失效重载 |
| OAuth | admin/import/refresh → token client → OAuth error mapping | connect30/total60、超时/连接/模糊结果/拒绝区别；不暴露响应凭据 |
| WS | PumpControl.receive → provider lifecycle → API forward/idle loop | 非文本控制帧不中断、terminal 后控制保留、复用撤销旧control、结算前不能丢 queued 请求 |
| 健康统计 | raw PG/backup hour → RequestHealth → Admin → API wire → Usage/Dashboard | cancelled/caller 排除 eligible、incomplete 计非成功、0 样本 null，成本与历史备份边界 |
| 模型 | FrozenAccountScope/explicit allowlist → selector eligibility → editor/catalog | 目录外精确 allowlist、权限先行、自定义取消保留/全选/新 ID、窄屏及完整悬浮 |
| 重启 | gateway main → startup_arguments/serve → system_update process | --restart-handoff、stdin EOF、先释放锁再交接；保留本地受控发布入口 |

## 验证状态

### 修复前后对照

在隔离源码副本中分别恢复表中基线的真实实现，保留对应新增测试。
回退版成功编译后行为断言失败；还原合入代码后重新运行相同测试。
没有把编译失败算作回归证据，验证后源码均还原为合入版本。

| 真实回退范围 | 不变的测试 | 修复前 → 修复后 |
| --- | --- | --- |
| `PumpControl::receive` 与 `put_reserved` 的基线实现 | `response_controls_use_the_original_socket_through_terminal_and_revoke_on_reuse` | 失败 → 通过；终态后的控制及复用撤销可实际观察 |
| `retain_model_eligible` 的基线函数 | `account_allowlists_override_catalog_omissions_but_weight_affinity_and_pinning_do_not` | 失败 → 通过；精确白名单、权重、亲和与指定账号边界保持完整 |
| `e5a76755` 的 Strict 关闭排队忙拒绝分支 | `strict_queue_disabled_migrates_busy_owner_before_send` | 失败 → 通过；恢复官方迁移。 |
| 仅间隔 fallback 沿用 Strict 固定等待 | `strict_interval_alone_can_migrate_to_a_ready_account` | 失败 → 通过；B 在等待中空闲后被选中。 |
| 将传输 turn 作为逻辑 alias | `responses_turn_alias_uses_logical_metadata_independently_of_transport_turn` | 失败 → 通过；同传输头不同逻辑 turn 允许，同逻辑 turn 跨根拒绝。 |

### 最新官方合入及旧断言纠正

2026-11-06 固定官方 `1240f225`，包括 `e39041782` 和其模式修正。此前 `ff168afdc`
保留的 Strict 关闭排队禁令已撤销。此前 `e5a76755` 修复同根 sibling turn 兼容，本轮
补齐 Responses 的 raw follow-only 身份冲突检查，以及独立端点按根匹配的区别。

| 新增官方路径 | 实际本地接入 |
| --- | --- |
| `credential/affinity.rs`、`credential/mod.rs` | 完整 cache preference 字段哈希、namespace、导出；真实会话与 transport key 分离 |
| `credential/selector.rs` | 本地 selector/{mod,projection,admission,capacity_wait}.rs；读写失败边界、CAS、telemetry 和等待重查 |
| `provider/mod.rs` | 只向选号器传缓存偏好；真实会话、restored/native、Guardian 优先；Excel 不误认 owner |
| `transport/request.rs` | 本地 transport/request/mod.rs 的 message_text_parts；新增逻辑账号身份字段及 Protocol 官方解析 helper |
| `tests/provider/contract/cache_preference.rs` | 官方键边界、存储超时、Strict 等待/队满/取消/迁移矩阵，适配本地真实容量端口 |
| `tests/provider/contract/session_binding.rs` | 本地 cache_preference.rs 实际 HTTP A→B→A、turn-state 保留、无 alias |
| `tests/provider/contract/mod.rs` | Relaxed 不读写缓存偏好；日志短哈希；旧 content-as-session 断言更新 |
| `docs/api.md`、`docs/architecture.md` | 模式、缓存偏好、真实 session/turn/owner 边界同步 |

必要的本地接入：Guardian 父线程成功记录独立于真实会话；hint 历史迁移仍读取实际旧键，
测试不再调用新版内容偏好来伪造旧存储。持久 `lc_`、账号设备/出口、Excel、State、
Shared/Reserved/Quality 容量与 Redis 原子准入保持其各自合同。

官方缓存键边界和 Preferred 存储超时测试已在 `e5a76755` 的真实旧源码成功编译后失败，
合入后同用例通过。旧测试将内容哈希作为 continuation identity 的断言已更新；
不能恢复该旧实现来获得测试通过。正式回归验证明确身份、真实续接和发送状态，临时执行脚本不入库。

进一步核对官方实际调用链后，Responses turn alias 从原始 payload 的逻辑 metadata 解析，
对应官方 `remember_generate_turn → turn_id_with_headers → codex_turn_id`；传输字段
`client_turn_id` 仍按官方 header-authoritative 顺序。不能把传输头优先级改掉来代替逻辑 turn 解析。
仅间隔受限的 Strict fallback 保持可移动，全池重查能够发现后来空闲的账号；旧 fixed_wait
把所有 Strict 绑定当不可移动，已通过编译成功后的失败回归确认不符合官方并发等待边界。
测试夹具同时统一直接/等待路径的并发上限，并执行真实等待人数上限；Guardian 回归验证
仅读取自身绑定而不固定内部读取次数，避免旧计数断言撤销身份复核。


### 本轮检查结果

- `provider-openai --test main`：全包 1,225 项已逐项通过。最终整包运行 1,224 通过、1 项本地 HTTP 模拟传输失败；随后 `request_alignment` 整组 7 项单线程通过。没有修改该失败用例的业务断言，不能将此记录称为单次全包全绿。
- 最后一次测试代码整理后，显式 hint 优先级定向回归再次通过；Provider `--lib` 329 项、Protocol 91 项通过。
- 启动与架构 62 项通过。普通 Cargo 链接因隔离机磁盘不足中止；使用其同一 rustc 测试编译参数，将输出放到已有临时内存目录后执行通过，未改源码、依赖或断言。
- 官方缓存偏好 6 项、完整候选缓存恢复、Strict 关排队迁移/仅间隔等待分流、turn 逻辑身份与真实续接 owner 边界均通过。
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` 通过；本地 `cargo +stable fmt --all --manifest-path backend/Cargo.toml -- --check` 与 `git diff --check` 通过。
- 后端 1,137 个跟踪文件与隔离机校验一致。精确暂存内容隐私扫描通过；无新增隐私豁免、无绕过 hooks、无临时执行脚本入库。
- 官方路径差异账：89 个路径、89 条记录，无缺项或重复。PR 的新提交 CI 在推送后独立验证，旧 HEAD 的通过结果不代替它。

以下为此前 `e5a76755` 及首轮合入的历史验证，不能替代本轮测试。

### 检查结果

- 前端：1,155 项 Node 测试通过；eslint、vue-tsc 和 Vite build 通过。隔离浏览器实测桌面/手机模型编辑、长名悬浮与健康空值通过。
- 后端：workspace/all-targets/all-features cargo check 已通过；全 workspace/all-targets/all-features Clippy（`-D warnings`）与格式检查通过。全工作区一次性链接曾因隔离机磁盘不足中止，最终采用下表逐包 all-features 验证，不能视作单次 workspace test 成功。
- 9 个包共 4,468 项测试通过；1 项原有 ignored 未执行，见下表。Provider 包当时包括严格绑定忙拒绝（本轮已按官方纠正）、合法重试迁移、可移动等待发现空闲账号以及取消清理。
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
