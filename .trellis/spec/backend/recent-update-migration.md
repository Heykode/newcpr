# Recent Update Migration Contracts

## Target And Scope

NewCPR is the only implementation and release target. Retired QX is a read-only
behavioral reference, not a merge parent. Preserve NewCPR's account device
identities, account egress, native TLS, Excel transport, managed state, quality
jobs, cost accounting, log cleanup and existing template catalog.

## Affinity And Recovery

- Affinity is integrated into the existing selector, not a parallel scheduler.
  Explicit required accounts and native continuation ownership take precedence.
- `openaiAccountAffinity` defaults to `strict`. Strict children follow the root
  binding; `relaxed` allows an independent child binding without rewriting the
  root. Neither mode permits transferring native encrypted state to another
  account or bypassing model permissions, quota, exclusions or concurrency.
- Preserve official follow-only waiting, including temporarily unavailable,
  cooling, disabled or excluded root owners. Strict children use the existing
  bounded sticky queue even when optional busy waiting is disabled; root
  migration reloads the full frozen pool without restarting the shared clock.
  Same-request exclusions remain effective after re-login. Queue/selection
  failure is terminal for follow-only routing, not an escape to another provider.
  Explicit/native owners retain their separate continuation recovery contract.
- Guardian parent preference supplements the existing top-level
  `openaiGuardianReservedConcurrency`; do not introduce a duplicate tuning field.
  The existing default remains zero.
- Turn aliases point to session keys, not account IDs. Bindings include a
  generation token. Renewal must match both account and generation and must not
  recreate expired state. Late completions cannot overwrite a newer binding.
- Redis accepts old account-only bindings. Settings validate binding TTL from
  1 through 720 hours, default 24. Omitted update fields preserve stored settings.
- Honor numeric and HTTP-date Retry-After, including zero, within the existing
  delivery, account-switch, cancellation and retry budgets. Flex rejection is
  terminal for the request and does not penalize the account.
- Do not force `store=false`, broadly delete history IDs, or replace NewCPR's
  request-normalization and identity-stripping contracts with legacy behavior.

## Refresh And Live

- Migration 0063 changes only the previous 3600-second refresh margin to 300;
  other configured values remain. Historical migration bytes stay frozen.
- Background refresh adds a stable per-account offset within the configured
  margin. Request-time refresh retains the configured margin. Quota observation
  jitter is separate from token refresh and account eligibility.
- Refresh-endpoint 401 and explicit terminal RT errors invalidate the credential;
  429, 5xx and transport failures retain bounded transient recovery. Do not apply
  the refresh classification to ordinary model or quota endpoint responses.
- Live bootstrap uses the existing authorization, account scope and admission
  path. Sideband/hangup retain the call owner and current account transport.
  Unsupported Realtime capabilities return explicit errors; they are not
  silently routed through ordinary Responses or Excel.

## Data And Administration

- Fast group policy is `default`, `enabled` or `disabled`. Disabled wins across
  groups; global disable wins. Keep legacy `disableFast` synchronized and accept
  old requests. Explicit new-mode updates take precedence; omission preserves.
- Import templates reuse the existing catalog and freeze the selected revision.
  Selected templates apply to both inserted and updated accounts. Omission keeps
  the original import semantics; no extra overwrite checkbox is required.
  New first-step prefill clients send `templateSettingsOverride:true` with both
  selection and final settings. Validate the template revision but do not overlay
  its original config onto edited fields. Final group/proxy references are validated
  by the ordinary import path. Derive the internal import proxy mode from the final
  explicit proxy/clear choice; otherwise preserve. Absent/false flag keeps legacy
  template application and the old idempotency fingerprint. The legacy State-only
  template field retains existing handling without reintroducing the retired UI.
- Keep database/wire `raw_upstream_error` / `rawUpstreamError`. Protected error
  details may contain a source chain. Core accepts legacy raw details as fallback;
  bounded queues count both fields and ordinary logs must not emit raw bodies.
- Admin renewal uses an absolute expiry, password-fingerprint revocation and
  atomic Redis comparison. Old sessions without the new expiry are not extended.
  Frontend login/logout/renewal cannot overwrite a newer cookie or request state.
- Duplicate or late budget settlement must not move the active budget window.
  Zero-attempt failures belong in the error view, not successful usage totals.
- Do not add quota-exhaustion prechecks to quality jobs in this migration.

## Verification

### v3.30.2 适配合同

1. 范围：固定官方 `e033e2199`，在原有模块内适配；不引入插件、PRISM、隐私规则或上游 Key。
2. 接口：`passthrough_request_headers(headers, live)` 仅由 Live 入口传 true；
   `ProviderAdmin::read_quotas` 按输入顺序返回逐项结果；备份创建/人工删除传入审计 context。
3. 合同：轮次 ID、父子轮次和历史日期保留原值；会话/thread/window 沿用既有账号派生。
   Live 的 `x-session-id`、`voice-session-id` 是业务引用，Connection 点名的头仍剥离。
   HTTP 保留多值字节；WS 沿用 tungstenite 的文本可表示边界，不能承诺透传非文本头。
   Live opening 必须生成 Host、Upgrade、Connection、Version、Key，使用原拨号与 TLS 路径。
   API 凭据不能覆盖选中账号身份。取消中的失败处理 future 保存在 session，不能重复拉取流。
4. 边界：单账号额度投影失败仅清空该行；revision 不匹配不可展示旧额度；
   已提交分组的展示投影失败返回无 record，但仍发布配置；备份审计失败必须事务回滚。
   重启受理后持有文件锁直到服务退出，重复 restart/update/rollback 返回冲突。
5. 示例：语音请求保留 voice reference，Responses 的同名本地调度提示仍不转发；
   原有 UA/出口不变；把可构建 HeaderMap 当成可实际发送的 WS 证明是错误的。
6. 回归：真实 loopback WS 握手、多值头、HTTP 入站 Connection 剥离、取消期间单次收敛、
   批量额度混合成功/过期版本、备份审计失败回滚、App 私有模块登记及数据库真实连通。
7. 正误：不能为通过测试删除账号隔离或放宽 loopback 数据库限制；应修复具体入口差异，
   并在 OVH 的同一网络空间运行测试服务。浏览器与本地对象断言不证明真实上游接受。
8. 验收夹具：Live 回环服务器需启用与既有 WS 测试一致的 permessage-deflate 配置，
   不能用不支持扩展的默认服务器误判客户端握手失败。备份 Failed 记录必须包含失败事实和完成时间。
   涉及 StoreConfig/真实备份恢复的测试 URL 要提供符合既有长度限制的测试密码，并配套同主版本 pg_dump/psql。

诊断使用 SHA-256 流式 JSON 摘要，原始正文仍受原采集开关约束；header 摘要包含实际
`session-id` / `thread-id`，不是只匹配下划线别名。常规预算 128 条/64KiB，关键证据
另有 32 条/48KiB；淘汰计数和阶段计数可见。ZIP 的 JSON、摘要及说明均从同一份已脱敏
文档构造；未知业务正文不能宣称完全脱敏。`request.usage` 必须先于 `request.finished`。

Run full workspace tests with isolated PostgreSQL and Redis, not silent store
skips. Cover old/new binding formats, stale generations, strict/relaxed roots,
Guardian reservation, Fast legacy updates, refresh classification, Live ownership,
budget settlement, administrator renewal and import-template revision conflicts.
Keep the existing identity, Excel, managed-state and retention regressions.
Frontend validation includes lint, typecheck, build, tests and desktop/mobile UI.
Local mocks do not establish production risk-control or upstream acceptance.
