# Client Continuity Contracts

## Diagnostic Selection and Quota Evidence

- Production provider lists exclude disabled accounts. An explicit administrator
  diagnostic may recover only its pinned account through a direct repository
  read, followed by the existing provider/exclusion/lease checks. Ordinary
  required-account requests must not use that bypass. In-memory test stores
  must reproduce the production enabled-only list behavior.
- Diagnostic selection never enables an account. Preserve existing per-provider
  feedback rules; OpenAI diagnostic health updates are not equivalent to enabling.
- Same-window quota recovery requires two consecutive fresh observations of a
  known account-wide reset with known non-exhausted usage. Persist candidate
  evidence with the exhaustion fact and a microsecond observation watermark.
  Missing windows, unknown usage, renewed limits or mismatched reset interrupt
  evidence; old/duplicate/pre-exhaustion observations cannot advance it.
  New exhaustion invalidates prior evidence, and legacy documents need fresh
  evidence. Keep existing reset-advance recovery and credential/enabled state.

## 1. Scope / Trigger

Apply to OpenAI outbound identity, account import/rotation/deletion, IPv6 policy,
WebSocket pooling, and HTTP fallback. Preserve the unified protocol-specific TLS
policy, account selection, billing and downstream streaming/buffered delivery.

### Unified Profile Supersession (Migration 0018)

The approved unified profile supersedes the independent TLS/session choices and
no-ALPN policy documented historically below. Live settings are Default/Custom
UA only; both Desktop and CLI are parsed without selecting transport. Native
reqwest HTTP advertises h2 then http/1.1; WS retains the original Rustls stack.
Remove the obsolete QX OpenSSL transport, not its applicable regression coverage.
QX account/Key session projection is unconditional at its existing owner.
Scheduling, device persistence, seeds, exact continuations and replay boundaries
remain authoritative. Native reqwest retries are disabled explicitly.

Migration 0018 archives the exact previous row selection before canonicalizing
UA and dropping TLS/session columns. Saves leave `legacy_selection` untouched.
Pinned legacy QX defaults become explicit custom CLI UA; independent null UA
becomes auto-updating Default. Old mode commands are rejected, not silently
ignored. Rollback requires a schema-compatible restore as well as the old binary.
Custom Desktop controls its auxiliary Desktop surface; custom CLI retains the
default auxiliary Desktop snapshot. `verified` describes UA artifact selection,
not full handshake verification. CA remains additive and never selects a backend.

Tests must exercise actual H2/H1 negotiation, streamed delivery and pool reuse,
unchanged Rustls WS ClientHello, both UA grammars, strict certificate/proxy checks,
HTTP and WS identity projection, original-seed continuation, cross-Key/account
isolation and migration/backup persistence. Do not treat skipped DB tests as pass.
Native TLS fixtures need a distinct CA and signed server leaf so both macOS trust
evaluation and Linux OpenSSL exercise a valid chain, not platform-specific trust
of a self-signed leaf. Register each new migration in `.frozen-sha256` without
editing prior entries.

## 2. Signatures

- `ProviderDeviceCodec`: provider-owned credential interpretation. Generic stores
  may replace the installation ID through this codec, not parse OAuth tokens.
- `provider_device_identities`: provider/user/workspace tuple and installation ID.
  Migration 0007 is independent of deletable account rows.
- `ProviderEgressStorePort`, `ProviderEgressConfig`, `CodexEgressRuntime`:
  migration 0008 stores policy, ordered sources, overrides and durable affinity.
- `GET/POST /api/admin/settings/openai-user-agent` and POST `/preview`:
  default/custom commands use migration 0009, not a replacement of all settings.
- IPv6 API signatures are documented in `docs/ipv6-egress-control-plane.md`.
- `RuntimeSnapshotPublisher::{refresh, publish_committed}` and the
  `runtime_snapshot` reconciliation worker publish account capacity alongside
  the immutable request snapshot.

## 3. Contracts

- Default and custom UA are explicit independent modes. A custom string equal to
  the current default is still custom. Verified release updates change only the
  default profile. Parsing custom text is not artifact or TLS verification.
- Freeze one effective profile for a prepared request and its permitted fallback.
  Do not mix a newly published version header with an older opening UA.
- The coordinator captures the optional provider-owned public profile once on
  first provider entry and shares it across all attempts, including account
  switches and HTTP fallback. New executions capture current settings again.
  Snapshot default and custom selections atomically, retaining the independent
  Desktop auxiliary profile for CLI UA. Never include authentication, cookies,
  installation IDs, connection owners or conversation state in this snapshot.
  Provider account rebinding and exact-continuation socket ownership remain
  authoritative; background account operations still use their own profile path.
- Device recovery requires complete provider/upstream user/upstream account
  identity. Never match by email or a newly generated local ID. Restore only
  device facts; preserve freshly supplied authentication material.
- Preserve valid active installation IDs. Ambiguous existing archives fail
  validation instead of silently changing devices. Deployment preflight must
  detect this condition before restarting production.
- Responses HTTP/WS omit the standalone `x-codex-installation-id` header,
  including downstream mixed-case/repeated copies and the final QX projection.
  Retain the selected device in body metadata, existing turn-metadata aliases,
  session derivation and connection ownership. Do not add another generator or
  alter device persistence/locking when aligning the outbound header contract.
- Account/import/delete and affinity mutations share transactions. Acquire the
  egress settings fence before device/account locks; preserve existing managed
  proxy/config revision lock order.
- Credential-only CAS reads the complete account/device archive binding in one
  snapshot, verifies the active installation and projects incoming credentials
  without the global device lock or an archive UPSERT. Lock and revalidate the
  account's revision, principal and credential before projection so deletion and
  recreation with a reused local ID cannot pass an old snapshot. Keep the final
  CAS and account/State retention unchanged. Missing bindings use the serialized binding
  path before acquiring any account row lock. Never escalate from an account row
  lock to the registry lock, silently repair a conflicting archive, or permit a
  binding mutation without advancing the account credential revision.
- Verify lock scope with real PostgreSQL: hold the registry/unrelated-account
  locks while a bound credential CAS completes, verify same-account conflicts,
  missing-archive lock order and rollback, and race CAS with import/delete.
  This removes demonstrated lock contention, not a measured upstream TTFT cost.
- No source pool means normal import still works. A historical fixed binding
  survives deletion, but only live accounts count as occupants when allocating
  vacant addresses. A missing historical address must not be silently replaced.
- `None` account mode inherits; `Some(Unchanged)` explicitly disables IPv6.
  Active local IPv6 and an account proxy cannot coexist.
- Opt-in selection reads immutable runtime state, not PostgreSQL per frame.
  Publish only after commit; failed reload fences stale outbound state.
  Serialize the entire reload read/publication, including failures. Publication
  version checks alone cannot order a delayed success against a failure fence.
- Exact connection-local continuation keeps its original socket, source and
  profile across mode updates. Disabling its source or deleting its account is
  not permission to move that continuation elsewhere.
- Fresh/random policies never authorize post-payload replay. A safe pre-send
  fallback uses the selected account, IPv6 source and frozen profile.
- Explicit IPv6 source binding must never silently become IPv4. Ordinary
  `unchanged` direct HTTP and WS now use IPv4 only; account proxies retain their
  own dual-stack dial and State probes retain their independent egress.
  HTTP and WS use their existing protocol-specific TLS/ALPN. This changes TCP
  address selection, not account scheduling, identity or continuation ownership.
  WS connector errors must retain the underlying I/O kind while redacting
  endpoint details, so pre-send failures keep their existing diagnosis.
- A local egress failure during a shared WebSocket opening remains a typed
  local error for every waiter; it must not become `SharedConnectFailed` or
  count against the origin breaker. After a committed policy reload, retire
  only idle/connecting slots whose live account or source is no longer valid.
  Busy slots finish their current response and are fenced from returning to the
  pool; a mode-only update must not retire an exact continuation owner.
- Do not persist tokens, cookies, session IDs or TLS session keys in device/IPv6
  archives. Do not add per-request body audits for this feature.
- Standalone image/search/compact body identity uses the selected lease,
  not a downstream installation alias. Scope only known account fields while
  preserving opaque business bytes; see `protocol-compatibility.md`.
  This does not add per-request identity storage, network probes or a second
  device-generation mechanism.
- WebSocket pool capacity is per account, across sessions, profiles, upstream
  hosts, proxies and IPv6 routes within the process. Count idle, busy, opening
  and closing ownership together for admission. Do not reintroduce a global
  opening semaphore or a separate static IPv6 connection cap.
- Publish effective account concurrency (override, otherwise the global account
  default) from the consistent runtime snapshot after commit, including disabled
  accounts used by diagnostics. Old request snapshots must not raise live pool
  limits. Missing, suspended or unpublished production capacity fails closed.
- On a capacity decrease, reclaim excess idle sockets; busy responses finish
  before retirement. Closing sockets still consume admission capacity but do
  not count as connections that need further retirement. Never erase a busy
  reservation merely because its age has passed the connection lifetime.
- A cancelled opening also retains admission capacity until its lease finishes
  and any raced socket is destroyed, but is no longer a retained connection.
  Repeated retirement must preserve an already-published local egress error.
  New same-key requests wait for that ownership to end instead of subscribing
  to the cancelled opening; existing waiters retain its original failure.
  Do not evict another idle owner when existing retiring slots will provide the
  requested capacity; repeated capacity polls must not progressively erase
  unrelated continuations.
- Resolve the exact continuation owner before selecting idle eviction victims.
  Capacity eviction records a tombstone. Ready reusable sockets do not wait on
  other sockets' closing handshakes. Keep closing bounded and retain ownership
  until the underlying transport has stopped.
- Capacity waits inherit request cancellation and the existing optional-HTTP
  decision deadline. Shared opening and request deadlines are combined without
  resetting or double-charging the wait budget. A disabled HTTP fallback must
  not be bypassed by the fast-path budget branch.
- The foreground decision deadline and origin opening health budget are
  different clocks. Queueing can shorten the remaining foreground wait without
  making a healthy short handshake an origin failure. Genuine slow openings
  and transport failures still count once, including after HTTP fallback.
- Snapshot availability during the 30-second grace period does not imply that
  suspension has cleared. Healthy same-revision reconciliation must recompile
  and publish before clearing suspension. Rejected older or conflicting
  publications follow the same failure protection as failed compilation,
  without lowering the revision high-water mark or extending an active grace
  period. Catalog-only failures may keep the last valid request snapshot.
- `websocketMaxConnecting` and startup `max_connecting` are legacy read-only
  compatibility inputs: accept and discard only those fields, never emit or
  persist them again. Other unknown fields remain invalid.

## 4. Validation & Error Matrix

| State | Required result |
| --- | --- |
| No IPv6 pool, unchanged mode | Original direct/proxy path |
| Proxy plus active local IPv6 | Reject, transaction rollback |
| Selected source not on this machine | Local configuration error before payload |
| IPv4-only upstream and explicit IPv6 | Fail, never silently use IPv4 |
| Policy write with stale revision | Conflict, no partial update |
| Source disabled while old socket exists | No new payload on that socket |
| Different email, same complete principal | Restore original installation ID |
| Same email, different principal | Never share a device based on email |
| Token refresh succeeds, runtime reload fails | Keep successful token; fence routes |
| Custom UA with invalid/control characters | Reject before persistence |
| Same revision becomes readable after a revision-query failure | Recompile and clear suspension before grace expires |
| Older/conflicting publication is rejected | Retain the latest snapshot only within the original grace period |
| A replacement is waiting while an owner is closing | Keep other idle continuations when pending retirement supplies capacity |
| Account eviction races with successful opening | Keep the opening/closing slot until its socket stops |
| Capacity queue leaves less than a normal handshake budget | Foreground fallback may proceed; healthy opening does not trip the breaker |

## 5. Good / Base / Bad Cases

- Good: delete then reimport the same principal, restoring its installation and
  historical address while retaining new tokens.
- Base: all new controls disabled; original transports and account settings work.
- Bad: rotate UA/source mid-attempt or allocate a new fixed address on GET.
- Bad: evict a live continuation solely to implement "fresh connection".
- Good: three 100-ms successful openings following 750-ms capacity waits do not
  trip the origin breaker, while foreground decisions retain their 800-ms limit.
- Bad: treating a successful revision probe as recovery while leaving the
  snapshot's suspension timer armed.

## 6. Tests Required

- Isolated PostgreSQL: startup backfill, deletion/reimport, concurrent imports,
  incomplete identity enrichment, source occupancy, revision conflict, rollback.
- Loopback IPv6: real peer source, account pool isolation, four active modes,
  exact continuation, safe fallback, frozen UA, IPv4-only destination failure.
- Retain the original full `provider-openai` suite, including TLS ClientHello,
  non-streaming delivery, quotas, proxy separation, cancellation and backpressure.
- API authentication, preview/save, defaults/null handling and forbidden fields.
- Account capacity: more than sixteen independent accounts opening together,
  atomic same-account reservation across routing keys, idle/busy/opening/closing
  accounting, live increases/decreases, simultaneous returns, cancellation,
  bounded close/shutdown, exact-owner preservation and eviction tombstones.
- Frontend typecheck/lint/build; never treat a missing database environment as
  evidence of successful persistence integration tests.
- Recovery tests must execute the reconciliation worker, including unchanged
  revisions, rejected publications and repeated failures. Assert that recovery
  clears suspension and that rejection starts, but never extends, the grace.
- Tests combining virtual deadlines with real TCP must resume the real clock
  before waiting for network close acknowledgement. `advance` and a single
  `yield_now` do not establish an I/O completion barrier; paused-clock automatic
  advancement can otherwise expire the server fixture before a real Close arrives.
- Hold a close flush pending across multiple capacity polls and assert only the
  necessary idle owner is retired. After release, assert other exact
  continuations remain reusable.
- Race account retirement against an already-created opening socket, assert
  no replacement is admitted before destruction, and preserve typed local
  failure through repeated retirement and maintenance.
- Exercise queued capacity followed by both healthy and genuinely slow/failing
  openings. Assert breaker state as well as foreground HTTP fallback timing.
- Run both library and integration tests (`cargo test --workspace --lib --test
  main --locked`). Capacity/close private-state regressions are library tests,
  so `--test main` alone is insufficient. The exact test-only exceptions live
  in the gateway architecture gate and `docs/architecture.md`; do not delete
  regressions or grant a workspace-wide `cfg(test)`/path-hook exemption.
- Import upserts must keep `updated_at` at least as new as the stored
  `updated_at` and incoming credential observation. PostgreSQL `now()` remains
  fixed at transaction start; an older transaction can acquire mutation locks
  after a newer transaction has committed the same principal. Using only
  `greatest(now(), incoming_observation)` can violate the account time
  constraint. Preserve the existing update timestamp, without weakening the
  constraint, changing lock order or rejecting explicitly imported tokens.
  Keep both the real concurrent-import test and a deterministic legal row
  where stored `updated_at > created_at > importing transaction time`.

## 7. Wrong vs Correct

Wrong: "fresh" always means a different physical socket, including a continuation
whose previous response exists only on the original connection.

Correct: resolve that continuation's physical owner first. Fresh/random policy
applies only when starting an independent connection is semantically permitted.

Wrong: the foreground deadline expired, so the origin handshake was slow.

Correct: capacity queueing spends foreground time but not handshake health
time. Observe actual opening duration and its outcome independently.

## 8. 原地凭据替换与刷新边界

本节根据任务 `09-14-qx-profile-capacity-wait` 的当前实现与主体研究合同同步；
记录非明显行为，不代表真实上游、跨层事务或全量回归已验收。

- 手工替换指定账号的凭据、已有账号 OAuth 重新授权，与正常 RT 刷新是不同语义。
  前两者必须经 OpenAI Provider 的 `prepare_candidate_oauth_rotation` 检查本次新材料；
  不能借刷新专用 `prepare_refreshed_oauth_rotation` 保留旧身份来绕过候选检查。
- `candidate_oauth_metadata` 先分别读取本次 ID token 与 AT 的身份投影：
  两份材料的已知 user/workspace 冲突，或候选与旧账号已知字段冲突，返回
  `principal_conflict`。不得先补旧 ID token 或旧 RT，再把补出的身份当作新材料证明。
  email/plan 逐字段投影，类型损坏或缺失不能遮蔽可观察的主体冲突。
- 没发现冲突不等于已确认连续。原地替换需要旧账号有非空 user 锚，且新材料给出
  相同 user 和完整 workspace；旧 workspace 已知时必须一致。旧 user 已知而 workspace
  缺失可按该条件补齐；仅同 workspace、同 email、双方都缺字段均不足以认领旧设备。
- 不满足连续性条件返回 `principal_unconfirmed`，写入前拒绝，旧账号保持不变；
  提示使用新建授权，不将其称为 Token 无效。该决策不禁止全新 opaque OAuth 材料
  按原有 unresolved 导入语义保存，也不自动冻结、删除或批量重置存量账号。
- 完整一致的投影在既有受信管理员模型下允许。opaque AT 加本次显式提交的完整一致
  ID token 不因 AT 无法解析自动拒绝；仅旧 ID 能补齐则不能通过。
  这是误换主体的一致性检查，不是 JWT 验签、issuer/audience 校验，
  也不证明 opaque AT 与 ID token 的密码学关联。不据此宣称能防恶意管理员伪造 JWT。
- 通过后才保留允许缺失的旧材料，并采用新 email/plan；缺失的非身份资料保留旧值。
  原 installation ID 不变，不由 email、local account ID 或不完整主体恢复历史设备。
- Provider 解释 token 并生成 `replacement_identity=Some(confirmed_identity)`；
  Provider Admin 将其传入现有待提交事实，Store 的
  `prepare_rotated_device` 在既有 revision/设备事务防线中再次检查已知身份不可移动。
  通用 Store 不反解 OAuth token；CAS 只证明 revision 一致，不能替代主体判断。
- 正常手工“刷新”和自动 RT exchange 保留现有刷新合同，成功准备路径仍使用
  `replacement_identity=None`，不借刷新改主体或设备，不新增无条件身份网络查询，
  不要求 opaque 新 AT 能本地解析。缺失字段保留沿原刷新链路处理，不与人工替换合并。

必须保留的反例与断言：

| 场景 | 必要结果 |
| --- | --- |
| 新 AT=B、无新 ID，但旧 ID=A | 冲突拒绝；不得靠补旧 ID 遮蔽 |
| 本次 ID=A/AT=B，或同 user 异 workspace、同 workspace 异 user | 冲突拒绝；不得生成成功待提交变更 |
| 新完整 A 或新完整 ID=A 配合 opaque AT，旧主体完整 A | 按受信 token-set 模型允许；保留设备，消费新 metadata |
| 新材料 unknown，只有旧材料能补齐 | `principal_unconfirmed`，不是 InvalidToken，零凭据提交 |
| 旧有 user 锚但缺 workspace / 旧仅有 workspace | 前者仅允许同 user 的完整候选补齐；后者不能认领旧设备 |
| 正常 RT 刷新得到 opaque AT / 全新 unresolved 导入 | 保留既有成功语义，不引入无条件身份查询 |
| 通过 Provider 准备后遇到 Store stale revision 或设备归属冲突 | 事务失败；无部分凭据、设备、身份或成功 audit 更新 |

负例须覆盖真实 `ProviderAdmin::prepare_rotation`、OAuth start/complete 和 Admin
提交边界，并断言失败时 commit/success audit/snapshot 次数为零及 guard 正确释放。
仅向 Store 直接传 `Some(B)` 的测试不能证明 Provider 没丢掉新候选身份；
合成未验签 JWT 测试也只能证明投影与决策，不能命名为验签通过。

## 9. Optional QX-Compatible Transport

- The global `qx-compatible` mode is explicit, not inferred from a UA string.
  Preserve `default` and `custom` UA semantics. CPR HTTP uses explicit native-tls;
  CPR WS retains Rustls. Do not introduce a third selectable TLS profile.
  The optional OpenSSL backend uses public APIs and locked vendored dependencies.
  Functional compatibility is not exact Go ClientHello or risk-control equivalence.
- Freeze both effective selection and the independent Desktop surface in one read.
  `CodexWireProfileState::frozen()` preserves both; recreating a state from only a
  QX wire profile is insufficient for later Desktop-only auxiliary requests.
- Include backend identity in WS pool keys, even if two UA strings are identical.
  Exact continuations restore the original owner profile before opening or sending.
  Account capacity remains shared across all profiles.
- Native HTTP preserves streaming through the existing response interface, never
  buffers a complete SSE response to adapt libraries, and disables transparent
  pooled-request replay. Configuration/connect errors are distinct from uncertain
  post-send errors. Token retry decisions must retain that distinction.
- Request deadlines include response-body polling. Cancelling or dropping a body
  must release its resources. Use cached async construction so cold native trust
  loading does not block a runtime worker; hot requests should hit bounded caches.
- QX application policy uses plain JSON; Native retains zstd, independently of
  TLS and UA. Managed identity headers apply after passthrough. The approved QX
  policy derives account/key-scoped IDs and sets cache=thread on the outbound
  copy only; Native preserves explicit caches. Never create another device or
  feed the projected cache back into account scheduling.
- Account scoping must also normalize an opaque
  `x-codex-turn-metadata` passthrough mirror before header projection, including
  when the account is otherwise the same. A downstream installation alias in
  that header must not override the selected account's installation identity.
- Any rewritten turn metadata sent through QX body or HTTP/WS header projection
  must use the existing ASCII JSON representation. Escaped Unicode is wire
  representation only; parsing it upstream must retain the original Unicode
  values.
- Responses HTTP and WS opening send the account-owned installation header in
  both profiles. Use only `context.installation_id`; omit missing/blank values,
  reject invalid nonblank header values before connecting, and never recover this
  authority from downstream headers or body. Keep the rule in the Responses-only
  builder, not shared model/account/OAuth helpers. Existing WS openings do not
  change per frame. Test real Provider header/body agreement after account scoping.
- Auxiliary service factories must use `profile.frozen()`, not
  `CodexWireProfileState::new(profile.snapshot())`: the latter turns the selected
  CLI profile into the default and loses the independent Desktop fallback.
  Test quota/reset/profile-statistics service entry points, not just transport
  helpers, while the live global selection is QX.
- QX quota's reference client is a separate Chrome impersonation surface. This
  change does not implement that third backend. Quota/reset, profile statistics,
  avatars and Desktop artifacts retain CPR's Desktop surface and native HTTP.
  QX authorization operations use their own UA/originator policy without chat IDs.
- Add migrations; never rewrite 0009 or other frozen SQL. Global QX selection
  does not imply per-account overrides or import/export support for such overrides.
- Treat durable UA selection as authoritative even when the save receipt is lost
  or its future is cancelled after commit. The Admin-owned reconciliation task is
  registered with Host, runs on every process without a leader lease, and shares
  the save mutex across the entire database read and provider publication.
  Successful cycles run every five seconds; failed cycles use bounded backoff.
  A five-second cycle deadline includes waiting for that mutex. Read, validation,
  timeout or cancellation failures preserve the last valid selection; preview is
  read-only. Do not create a detached publisher or replay the mutation to recover.
- Construct the old rustls WS custom-CA connector only on the CPR path.
  QX trust loading belongs to the native async opening and its deadline.
  A plain `ws://` regression may prove unused CA work is skipped, but does not
  replace real `wss://` certificate and hostname rejection tests.
- Preserve loopback TLS/CA rejection, proxy/IPv6, streamed deadlines, cancellation,
  same-UA backend isolation, old-owner continuation and global save/reload tests.
  Report native platform and exact linked OpenSSL version; do not label another
  library build's vector as verification of the deployed binary.

## 10. Independent Selection and Session Isolation

- `Independent { user_agent, tls_profile, session_policy }` is one atomic global
  selection. UA null follows verified CPR releases; custom values remain fixed.
  Both other choices are mandatory. Parsing Desktop/CLI does not select TLS.
  Keep legacy mode semantics, migration 0012, and frozen auxiliary Desktop state.
- Derive QX session/thread from length-prefixed, domain-separated SHA-256 over
  trusted Key ID, durable installation, workspace, and original anchors. Preserve
  raw seeds across projection and owned session restoration. UUIDv7-shaped output
  is deterministic, not a timestamp or an upstream credential.
- Pool key equality/hash, routing owners, logical matches and tombstones include
  a non-reversible caller-scope digest. QX opening profiles also include projected
  session/thread; new threads must not inherit another opening's immutable headers.
  Exact continuation resolves the original owner before applying current settings.
- Cleared turn state must stay cleared through passthrough and known body mirrors,
  including flat JSON turn-metadata. Unknown opaque business content is not a
  license for recursive deletion. Retain complete input and valid current state.
- Content fallback normalizes supported string/message/text-block forms. String
  first-user text, tools and nontext now participate: this intentionally migrates
  weak content-only affinity, not explicit or restored local conversation IDs.
  Same content never proves permission to use a prior response or shared socket.
- Tests must cover actual Provider HTTP/WS projection, trusted Key overriding
  untrusted adapter aliases, saved seeds, cross-account/cross-Key changes, new
  thread openings, same-Key reuse, tombstones, old owners, and full selected TLS
  ClientHello matrix. Profile snapshots alone do not prove transport selection.

## 11. Native HTTP Dependency Boundary

- CPR account HTTP, IPv6 HTTP and token clients explicitly use
  `build_reqwest_native_client_with_custom_ca`. Custom CA bundles are additive;
  `CODEX_CA_CERTIFICATE` takes precedence over `SSL_CERT_FILE`. Invalid/missing
  bundles fail closed, without disabling hostname checks or reverting to Rustls.
- The legacy `build_reqwest_client_with_custom_ca` explicitly retains Rustls
  with or without a CA bundle. XAI clients and Host update/download clients also
  select Rustls explicitly. Cargo feature unification must not silently switch
  those clients when OpenAI enables reqwest native-tls.
- Keep the official default HTTP no-ALPN policy; compiling reqwest's `http2`
  feature alone does not enable `native-tls-alpn`. QX keeps its own HTTP/2
  negotiation and OpenSSL configuration. Do not conflate these implementations.
- Native ClientHello tests compare the linked native backend on the same OS;
  exact vectors from another OpenSSL version are not Linux release verification.
  Preserve native extension order; normalize only Rustls's randomized order.
- Use isolated child processes for CA environment tests and proper CA-signed
  server certificates. Cover both CA variables, precedence, missing/invalid
  bundles, untrusted issuers and wrong hosts through real loopback HTTPS.
  Give the synthetic CA a distinct subject from the leaf and verify its chain;
  same-subject fixtures can fail OpenSSL trust while passing the macOS backend.
- The HTTP/WS selection matrix must include a real WS pool and account owner.
  Stop capture at ClientHello; a TLS-rejecting fixture cannot serve later
  connection restart attempts. Never disable WS coverage to avoid a timeout.

## 12. Location and Weak Identity Characterization

- Current Generate encoding replaces search `user_location`, including explicit
  downstream locations, and rewrites marked user environment XML date/timezone.
  Missing configuration uses the default location; it does not disable rewriting.
  There is no admin toggle. Tests documenting this behavior are not approval to
  change that policy.
- Weak content identity is derived after location encoding. Search location changes
  can change the fallback seed; a marked first-user environment date can change it
  across days. Fixed encoded-date fixtures exercise the real hash, not the wall
  clock. Do not claim an end-to-end midnight simulation from such fixtures.
- Explicit identity and restored owned seeds remain stable across location changes.
  Verify real Provider calls and reject foreign Key state before upstream send.
- The pre-existing first-user-only fallback can group different questions when the
  first user message is an identical environment block. This is a same-owner weak
  grouping limitation, not evidence of cross-account transcript leakage.
- macOS-only loopback tests do not verify Linux native TLS. Isolated Linux
  loopback tests verify that platform's protocol and reuse behavior, not upstream
  generation, risk acceptance or production throughput. Do not change the native
  no-ALPN policy as part of verification.
- Linux integration verification must preserve database endpoint guards. Use
  container-local loopback PostgreSQL/Redis with the authenticated password shape
  from CI, not host databases or relaxed assertions. Enable CI's fail-on-missing
  infrastructure checks and record the exact builder and runtime image.

## Client Pool Test Boundary

- A pool's maximum idle count is not a hard bound on total socket creation.
  The pinned Hyper client can finish speculative dials after an idle checkout wins.
- Do not cap accepted sockets to the number of concurrent requests: an unfinished
  or unused speculative dial can occupy the cap and deadlock a streaming barrier.
  Give each accepted connection a response-visible synthetic ID, and verify reuse
  from connections that actually carry requests. Keep same-connection sequential
  reuse, HTTP/2 multiplexing, concurrent HTTP/1 streaming, and reuse between waves.
- Include an established TCP dial that deliberately sends no TLS or HTTP bytes.
  It must not block the normal streams or count as an application connection.
  Bound request counts, deadlines and fixture shutdown, not socket admission.
  Separate accepted TCP counts from used connections in diagnostic output.
  This is functional loopback verification, not upstream throughput evidence.

## Responses Compression Boundary

- Complete account/session projection and serialization before deciding HTTP
  compression. Both session policies use ZSTD level 3 at 1024 UTF-8 bytes or
  above, independent of selected TLS/UA. Set Content-Encoding only for encoded
  bodies and keep downstream transport headers filtered. Other endpoints retain
  their existing behavior. Do not introduce post-send retries for encoding errors.
- WS thresholds are negotiated connection state: 128 bytes with client context
  takeover, 512 with client_no_context_takeover, none without permessage-deflate.
  Server-only no_context_takeover does not change the outbound threshold.
  Preserve the existing handshake offer, level 6 and library-owned frame masking.
- Bypass compression before touching the dictionary for a small data message.
  Compressing then discarding the result breaks context takeover synchronization.
  Control frames and inbound decompression must keep their existing paths.
- Test real HTTP bytes and raw WS frames, threshold boundaries, multibyte UTF-8,
  compressed/plain alternation, negotiated dictionary resets and pool reuse.
  Existing TLS, identity and pre-send fallback regressions remain required.
  Synthetic compression timing is not end-to-end latency or upstream acceptance.

## 13. 固定 Rustls 画像与安全依赖

- `ensure_rustls_provider` 是现有进程默认 Rustls provider 的初始化 owner。
  CPR WS、显式 Rustls HTTP helper、共享该 provider 的代理、XAI/Host 等消费者
  均受影响，不能将它描述为 OpenAI 私有配置。宿主预装 provider 必须保留。
  CPR 默认 native HTTP 和 QX OpenSSL 主链路不由这份签名列表控制。
- 安全库版本和固定画像是两个合同。Rustls 签名列表固定为
  Desktop 26.901.51231 / Core 0.153.4 的旧十项，通过新版 aws-lc provider 的
  `signature_verification_algorithms.mapping` 配置，不通过改写报文实现。
  `all` 证书链算法、cipher suites、KX、随机源及密钥 provider 保留新版实现。
- 固定列表不含 ML-DSA 握手签名，不支持仅使用这些签名的对端。这不等于删除
  混合后量子 KX，也不能据此关闭证书或主机名校验。
- 依赖升级不得仅为通过测试而修改参考值。要么保留已批准画像，要么在重新采样、
  审阅并批准目标后版本化改变；不得退回漏洞版本或屏蔽安全告警。
- 合并其他传输改动后，保留主分支 native HTTP、Rustls helper/WS 和 QX 的
  各自断言与扩展顺序语义。旧架构采样不能冒充合并后架构的对照；
  使用当前主分支和同一采样器重新比较安全依赖前后差异。
- 兼容验证覆盖宿主预装与并发初始化、正常 HTTPS/WSS、错误证书/主机名、
  代理内外层校验及重复采样。随机扩展顺序、临时公钥和 session ID 不纳入
  逐字节相等承诺。
- TLS 密钥切换边界回归将完整明文
  EncryptedExtensions 追加到 ServerHello 同一记录，断言
  `KeyEpochWithPendingFragment` 和 `unexpected_message`。合法服务端 flight
  必须成功；普通证书负例不能冒充这个边界回归。
- 本地 macOS 结果不代表 Linux 发布二进制、数据库 TLS 或真实上游验收。未执行的
  持久化/网络测试必须单独记录，不把提前返回写成通过。

## 14. Account Client Cache Restoration

- Ordinary direct account HTTP requests reuse the base client's direct pool.
  Retain that original client across proxy rebinding so removing a proxy cannot
  reuse the proxy pool. Authentication, Cookie, State and the frozen UA remain
  request-scoped; account or UA cache invalidation must not tear down the shared
  base pool. Preserve ordinary direct IPv4 binding and independent IPv6/probe clients.
- Account HTTP cache clients (including proxied requests) retain the process-wide
  256-entry LRU keyed by account/proxy, additive CA and frozen UA. Do not replace
  LRU with clear-all eviction or remove account isolation from proxy clients.
- Token refresh has its own 128-entry LRU keyed by account/proxy, CA, frozen
  UA and token endpoint. Both automatic and manual refresh pass the account ID.
  Pre-account authorization/import and credential lease/CAS semantics stay intact.
- LRU capacity counts cached clients, not sockets, account admission or requests.
  Eviction drops cached references; live requests retain their clients and finish.
  Configuration/account callbacks invalidate affected caches. Reapplying an
  unchanged effective UA must not evict warm pools during periodic reconciliation.
  Routine Cookie persistence must not acquire a new cache-invalidation hook.
- WS direct, proxy and source-bound dialing use the official default TCP
  NODELAY policy; do not force NODELAY to implement response delivery or retries.
  SOCKS buffering remains handshake-only. Preserve compression, flush
  acknowledgement, 1200-ms/64-KiB precommit buffering, replay safety and ownership.
- Regression coverage must use real loopback sockets for direct sharing with
  isolated request headers, proxy account/profile separation, runtime UA changes
  on the same base client, LRU pressure and eviction during a partial response.
  Isolate global-cache pressure tests in a child process so unrelated tests
  cannot invalidate their expected hot entries.
- Unrelated tests using separate Tokio runtimes must not share synthetic account
  IDs with a process-cached client. A runtime teardown can drop that client's
  connection tasks while another test is using its pool. Give independent
  fixtures unique account IDs and include them in the frozen request scope;
  keep reuse assertions within one runtime rather than weakening retry policy.
- Quota seed helpers need the same isolation as conversation fixtures. Test two
  helper instances for distinct account IDs, and characterize runtime teardown
  with a held real response: a deliberately shared ID loses its old-runtime
  connection, while a distinct ID's response finishes. Do not change production
  account identity or pool keys to compensate for independent test runtimes.
