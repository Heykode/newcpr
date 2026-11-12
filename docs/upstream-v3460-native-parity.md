# 官方 v3.46.0 合入差异账

## 固定范围

- 本地目标：`Heykode/newcpr`，分支 `codex/upstream-v3460`。
- 本地起点：`6492dd2309fe47dc07d7e7b646ce0a3122bf8a4e`（origin/main）。
- 上次官方基线：`b22b0cfa4162dd884fb9e2255193af56c4f702cd`（v3.45.2）。
- 本次官方目标：`28ad8fb1c46efbbb2d2678e9bc85205f480e3b2f`（v3.46.0），7 个提交、124 个路径。
- 本次接入未查询、修改或部署生产实例，不据本地基线推断线上版本；发布标签和部署仍按独立授权执行。
- 沿用 `upstream-integration-checklist.md`。本文是本次逐项证据，不取代共享规则。

## 行为与真实入口

| 官方提交 | 合入结果与实际入口 | 必要本地适配 |
| --- | --- | --- |
| `4888681a0` | 5 小时/周/月 resetWindowWeights；Core 评分、OpenAI/xAI 额度投影、设置 API/Store、实际设置页 | 保留 preferHigherWeight；旧配置三项补 0，不恢复旧单 reset_at 评分；不改变严格/优先/宽松准入和切号预算 |
| `9f0d71267` | 物理 WS 握手上限，最终帧发送前检查；socket/租约先释放；允许的请求 HTTP 回退，精确续接要求重放，预热拒绝 | 本地真实入口 client_sse.rs；删除固定阈值、滞后和旧设置，保留历史设置读取丢弃；HTTP store:false 重放与瞬时重连提示回归保留 |
| `9739bb08b` | 官方 gateway-egress 公共模块；默认 HTTP/WS/下载出口、显式优先、NO_PROXY、强制直连与部署出口探测分开 | 保留实际身份/UA/TLS 和显式 IPv6 路由；本地额外 CLI 版本同步也接入；AccountOutboundField 才是实际账号页入口，已接入探测，内部 direct 保留保存合同 |
| `75be1287e` | Codex Responses 最终 HTTP/WS 同时省略三种输出 token 上限 | 真实最终 wire 断言；不改变其他 Provider/Compact/Images，也不删除响应 incomplete reason 解析 |
| `fac37ec56` | PermissionDenied/CrossesDevices 复制回退；复制保留目录权限，清理先仅放开待删目录 | 本地更新器无插件保护步骤；在已有 Web 更新/回滚及回滚后再次更新验证，不改变归档解析策略 |
| `dad91fe9f` | 官方 UI 功能色/中性色独立、切预设保留编辑颜色、默认明暗跟随 | 实际 theme 模块与编辑器，保留本地 --cp-*、类型名和账号多分组布局 |
| `28ad8fb1c` | 官方版本元数据 | 留给用户后续授权的 NewCPR 发布阶段 |

## 兼容与排除

- 新窗口数据复用已有额度观测，设置复用 JSONB；没有新增数据库迁移、后台额度请求或轮询。
- 统一出口的默认环境配置为进程首次读取后缓存，显式代理优先；本轮不改部署变量或 IPv6 禁用策略。
- 本地 XAI endpoint policy 仍返回无显式代理的基础 client，显式账号代理由既有 account_proxy_client 覆盖；二者最终按相同官方优先级运行。
- 请求采集是本地额外消费者：新诊断值 `http2_ws_message_too_large` 加入既有安全枚举，防止日志摘要丢失回退原因；旧值仅兼容历史诊断，不恢复旧路由。
- 本地 `cli_version.rs` 是 UA 自动同步的额外下载消费者，已使用相同默认出口；不改 UA 解析或版本更新策略。
- 不引入上游 API-key 账号、专属 CPR 互联、插件运行时或插件下载；混合提交公共代码分别接入。
- 退休固定阈值测试被握手边界正反例替换；必要的本地 HTTP store:false/身份出口断言迁移保留。
- 账号多平台批量设置仍保留 egressMode:unchanged 的原子清理；仅修改标签不能代替真实出口和保存路径接入。
- 原始 compare、reject、编译日志、隔离数据库配置和截图均放仓库外；不提交临时脚本。

## 逐路径处置

以下路径均相对官方仓库；同路径“移植/接入”指官方补丁作用落入现有实现，不能理解为整份文件字节相同。

| 官方路径 | 处置 | 本地落点 / 原因 |
| --- | --- | --- |
| `backend/Cargo.lock` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/Cargo.toml` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/apps/gateway/tests/architecture.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/apps/gateway/tests/bootstrap/server.rs` | 必要接入 | `backend/apps/gateway/tests/bootstrap.rs`：ProbeRoute 签名，保留本地初始化合同 |
| `backend/crates/gateway-admin/src/model/accounts.rs` | 已核对 | 官方仅修改禁用代理注释；本地无该旧注释，实际出口由 Provider 公共出口处理 |
| `backend/crates/gateway-admin/src/ports/proxy.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-admin/src/use_case/plugins/distribution.rs` | 用户排除 | 插件专属路径；不新增插件消费者 |
| `backend/crates/gateway-admin/src/use_case/proxies.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-admin/tests/use_case/proxies.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-admin/tests/use_case/service_contract.rs` | 用户排除 | 仅插件 SDK settings 声明对比；本地无插件 SDK，原生 settings API/数据库单独验证 |
| `backend/crates/gateway-api/src/admin/proxies.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-api/src/admin/settings.rs` | 必要接入 | 本地设置探测已有 `test_direct` 专门入口；Host 明确使用 ProbeRoute::Direct |
| `backend/crates/gateway-api/tests/admin/proxies.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-api/tests/admin/settings.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-core/src/account/mod.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-core/src/account/selection.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-core/src/account/smart_scheduling.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-core/tests/account/mod.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-core/tests/account/smart_scheduling.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-core/tests/engine/execution/mod.rs` | 已核对 | 官方仅补新字段为空；本地对应 fixture 使用默认构造，保持空窗口 |
| `backend/crates/gateway-core/tests/engine/policy.rs` | 已核对 | 同上；工作区 all-targets 编译覆盖构造点 |
| `backend/crates/gateway-core/tests/provider_ports/mod.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-egress/Cargo.toml` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-egress/src/lib.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-host/Cargo.toml` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-host/src/client_distribution.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-host/src/outbound/http.rs` | 无本地消费者 | 官方通用受管 HTTP 服务在本地无运行入口；公共 helper 已供实际 Host/Provider 消费 |
| `backend/crates/gateway-host/src/peer.rs` | 用户排除 | 专属 CPR 互联入口 |
| `backend/crates/gateway-host/src/pricing.rs` | 无本地消费者 | 本地无 ModelsDevPricing 外部价格同步服务，不新增该产品入口 |
| `backend/crates/gateway-host/src/proxy_probe.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-host/src/system_update/metadata.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-host/src/system_update/release.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-host/src/system_update/swap.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-host/tests/outbound/http.rs` | 必要接入 | 默认出口真实 HTTP/ProbeRoute 测试迁入 host/tests/proxy_probe.rs；插件受管 HTTP 部分排除 |
| `backend/crates/gateway-host/tests/proxy_probe.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/gateway-host/tests/system_update/operations.rs` | 必要接入 | host/tests/system_update/mod.rs 覆盖只读 Web 更新/回滚；回滚后再次更新覆盖只读递归清理；插件特有 rollback 排除 |
| `backend/crates/gateway-plugin/runtime/tests/generation/policy/mod.rs` | 用户排除 | 插件专属路径；不新增插件消费者 |
| `backend/crates/gateway-plugin/runtime/tests/support/environment.rs` | 用户排除 | 插件专属路径；不新增插件消费者 |
| `backend/crates/gateway-plugin/sdk/docs/capabilities.md` | 用户排除 | 插件专属路径；不新增插件消费者 |
| `backend/crates/gateway-plugin/sdk/docs/upstream-adapters.md` | 用户排除 | 插件专属路径；不新增插件消费者 |
| `backend/crates/gateway-plugin/sdk/src/call/services/settings.rs` | 用户排除 | 插件专属路径；不新增插件消费者 |
| `backend/crates/gateway-store/src/redis/credential_leases/mod.rs` | 必要接入 | 本地 redis/credential_leases.rs 新 quota_reset_windows 字段；Redis 不捏造 Provider 额度窗口 |
| `backend/crates/gateway-store/tests/postgres/runtime_settings.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/Cargo.toml` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/compat/request.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/credential/quota/scheduling.rs` | 必要接入 | 本地 quota/mod.rs + snapshot.rs 投影真实周期并 clone 新非 Copy signals |
| `backend/crates/providers/openai/src/credential/quota/snapshot.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/credential/selector.rs` | 必要接入 | 本地 credential/selector/mod.rs 的实际候选信号构造 |
| `backend/crates/providers/openai/src/credential/token_client.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/provider/execution.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/provider/failure.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/provider/mod.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/session_transport.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/transport/client.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/transport/client_stream.rs` | 必要接入 | 本地 transport/client_sse.rs 在 prepare 后、业务发送前执行官方大小判断 |
| `backend/crates/providers/openai/src/transport/profile/cli_release.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/transport/profile/mod.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/transport/profile/platform_release/mod.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/transport/protocol/websocket.rs` | 已核对 | 官方删除旧固定阈值计数 helper；本地没有该 helper，实际发送读取最终编码文本长度 |
| `backend/crates/providers/openai/src/transport/websocket/coordinator.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/transport/websocket/error.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/transport/websocket/handshake.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/transport/websocket/model.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/src/transport/websocket/pool/state.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/tests/compat/request.rs` | 必要接入 | 本地 transport/request/mod.rs 验证编码副本省略上限；generate_compat/identity_isolation/messages 验证最终 wire；原始协议 payload 保留 |
| `backend/crates/providers/openai/tests/credential/contract/capacity.rs` | 已核对 | 仅补空窗口的 fixture；本地默认构造已覆盖 |
| `backend/crates/providers/openai/tests/credential/contract/scheduling_projection.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/tests/credential/quota/scheduling.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/tests/provider/contract/capacity.rs` | 已核对 | 仅补空窗口的 fixture；本地默认构造已覆盖 |
| `backend/crates/providers/openai/tests/provider/contract/messages.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/tests/provider/contract/mod.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/tests/provider/contract/passthrough.rs` | 必要接入 | 本地 provider/contract/generate_compat.rs：HTTP/WS 三种上限均省略，保留上下文选项 |
| `backend/crates/providers/openai/tests/provider/contract/session_binding.rs` | 已核对 | 仅补空窗口的 fixture；本地默认构造已覆盖，亲和模式不因此更改 |
| `backend/crates/providers/openai/tests/provider/contract/upstream_adapter.rs` | 用户排除 | 上游 API-key 适配器；原生 OAuth 的公共省略逻辑已单独接入并验证 |
| `backend/crates/providers/openai/tests/support.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/tests/transport/account_proxy.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/tests/transport/latency.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/openai/tests/transport/profile/mod.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/xai/Cargo.toml` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/xai/src/credential/quota.rs` | 必要接入 | 本地 credential/catalog.rs：周/月且有权威额度证据才投影重置窗口 |
| `backend/crates/providers/xai/src/credential/refresh.rs` | 已核对 | 官方仅修改默认出口注释；本地真实刷新复用 network.rs 默认 builder 和显式账号代理 |
| `backend/crates/providers/xai/src/credential/verification.rs` | 已核对 | 官方仅修改默认出口注释；本地验证同样通过 network.rs 出口 |
| `backend/crates/providers/xai/src/transport/network.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/xai/src/transport/profile.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/xai/tests/admin.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/xai/tests/credential/quota.rs` | 必要接入 | 本地 tests/credential/catalog.rs 移植官方周期/额度证据正反例 |
| `backend/crates/providers/xai/tests/credential/selector.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `backend/crates/providers/xai/tests/provider/contract/mod.rs` | 已核对 | 仅补空窗口的 fixture；本地默认构造已覆盖 |
| `backend/crates/providers/xai/tests/transport/network.rs` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `deploy/README.md` | 必要接入 | 只同步公共部署代理规则，插件章节不引入 |
| `docs/api.md` | 规范同步 | 更新对应当前文档；旧历史差异账不作为新行为约束 |
| `docs/architecture.md` | 规范同步 | 更新对应当前文档；旧历史差异账不作为新行为约束 |
| `docs/plugins.md` | 用户排除 | 插件专属说明 |
| `docs/theme.md` | 规范同步 | 更新对应当前文档；旧历史差异账不作为新行为约束 |
| `frontend/package.json` | 必要接入 | 本地无外部 UI 包依赖；官方 UI 更新落到本仓库 src/theme 真实源码 |
| `frontend/pnpm-lock.yaml` | 必要接入 | 不引入未使用的 UI 包；本地依赖无新增，主题实际源码已移植 |
| `frontend/src/api/modules/proxies.ts` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/api/modules/settings/index.ts` | 必要接入 | 本地 api/modules/settings.ts 默认值/类型，useSettingsForm 读取保存及校验 |
| `frontend/src/i18n/locales/en/accounts.json` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/i18n/locales/en/plugins.json` | 用户排除 | 插件专属路径；不新增插件消费者 |
| `frontend/src/i18n/locales/en/proxies.json` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/i18n/locales/en/settings.json` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/i18n/locales/zh-Hans/accounts.json` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/i18n/locales/zh-Hans/plugins.json` | 用户排除 | 插件专属路径；不新增插件消费者 |
| `frontend/src/i18n/locales/zh-Hans/proxies.json` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/i18n/locales/zh-Hans/settings.json` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/i18n/locales/zh-Hant/accounts.json` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/i18n/locales/zh-Hant/plugins.json` | 用户排除 | 插件专属路径；不新增插件消费者 |
| `frontend/src/i18n/locales/zh-Hant/proxies.json` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/i18n/locales/zh-Hant/settings.json` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/views/accounts/components/AccountProxyField.vue` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/views/accounts/composables/useAccountBatchEditor.ts` | 保留接口并核对 | 内部 direct 继续驱动 accountEgressPatch；显示/运行含义为部署默认出口，避免混合平台批量遗漏 egressMode |
| `frontend/src/views/accounts/composables/useAccountEditor.ts` | 保留接口并核对 | 通过本地 account-egress 工具与 AccountOutboundField 接入；保存合同沿用 direct 内部值 |
| `frontend/src/views/accounts/composables/useAccountProxyTest.ts` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/views/accounts/utils/accountCreate.ts` | 必要接入 | 本地 onboarding/create-model 与 account-egress 共同保存显式路由；内部 direct 不重命名 |
| `frontend/src/views/plugins/components/PluginSourceProxyField.vue` | 用户排除 | 插件专属路径；不新增插件消费者 |
| `frontend/src/views/proxies/components/ProxyAssignAccountsModal.vue` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/views/settings/components/SmartSchedulingModal.vue` | 必要接入 | 本地 RotationStrategyCard.vue 已有实际设置入口，追加三个窗口输入及官方帮助文案 |
| `frontend/src/views/theme/components/ThemeGlobalTokenPanel.vue` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/views/theme/components/theme-scenes.svg` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `frontend/src/views/theme/composables/useThemeEditor.ts` | 移植/接入 | 同路径实际实现或现有 fixture；包含本地接口/默认构造适配 |
| `modules/ui` | 必要接入 | 官方 UI 46ade6447 → 45b076735（功能 cd20839a4）；8 个 theme 源文件移植、appearance 删除；保留 --cp-* 与类型名 |
| `release/notes.md` | 发布阶段 | 本轮不修改 NewCPR 发布记录；官方版本说明以本账记录 |
| `release/version.yaml` | 发布阶段 | 不以官方版本覆盖 NewCPR 发布身份，本轮未授权发布 |

## 验证

验证使用 Rust 1.99.1 和本机隔离 PostgreSQL/Redis，测试端点仅 loopback；普通 socket 测试清除宿主代理变量，代理矩阵由隔离子进程分别设置。未使用生产账户或生产服务。

| 范围 | 结果与证据 |
| --- | --- |
| 官方路径审阅 | 124 个路径均有处置记录；新增 gateway-egress 的 Cargo.toml 与 lib.rs 与固定官方目标字节一致 |
| Rust 工作区 | check --workspace --all-targets、fmt --all --check、clippy --workspace --all-targets -- -D warnings 通过 |
| Core | 550 项通过，包含智能评分、周期、近似候选、既有亲和与切号预算 |
| Admin / API | 受影响 settings/proxies 共 23 + 49 项通过；非零窗口保存重读、非法小数拒绝、旧阈值丢弃、直连/部署出口/显式代理及冲突请求 |
| Store | 隔离真实 PostgreSQL/Redis 107 项通过；runtime_settings、snapshot、credential_leases、capacity_wait、request_capture |
| OpenAI | 单测 327 项通过；集成首轮 1246 项通过，5 项旧预期/夹具/环境失败修正后定向 6 项全部通过，无遗留失败；1 项官方 Codex 可执行文件 TLS 指纹比较因缺少参考文件而跳过 |
| XAI | 396 项通过，包含周/月窗口额度证据、显式与环境代理 |
| Host / App | 133 + 62 项通过；只读目录更新/回滚及回滚后再次更新、默认出口环境矩阵；App 复测已配置隔离数据库与 Redis |
| 前端 | 类型检查及构建、全量 ESLint 通过；受影响 643 项测试通过，覆盖主题、运行设置、非零窗口保存与嵌套隔离、账号出口合同 |
| 浏览器 | 实际账号出口组件/批量/模板/导入在 1440/390/320 宽度通过；智能调度三种宽度通过；主题 1440/390 通过，均屏蔽外部与真实 API 流量，未发现页面错误 |
| 隐私 | 仅对 26 条已存在的合成夹具逐条复审并更新精确文件/行哈希，保留历史记录；完整工作树临时 index 的 staged 检查与完整 tree 快照扫描均通过 |

本轮额外接入集中于本地真实消费者：AccountOutboundField、CLI 版本同步、请求采集诊断、安全的历史设置读取，以及保留本地账号出口/主动回切接口。没有新增运行限额、重试限制、后台查询或数据库迁移。旧测试与规范随官方合同更新；临时环境、脚本、原始日志和截图均留在仓库外。

验证边界：没有对真实上游进行请求、没有部署 Linux 生产实例；Windows 只读文件清理和实际跨文件系统复制回退尚未实机验证。未执行的外部参考 TLS 比对不算通过。后续提交、发布时仍需按既有规则检查确切 staged 内容、发布历史与目标平台 CI；本轮不新增重复全量门禁。
