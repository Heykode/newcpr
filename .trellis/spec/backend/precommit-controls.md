# Precommit Controls And Quota Diagnostics

## Sources And Scope

Selective adaptations of upstream `zyycn/codex-proxy-rs` #301 (`4634d8d`) and
#304 (`3bf7030`). Do not replace New CPR's identity, account egress, quota
authority, finite scheduling or continuation ownership contracts.

## Native Delivery And Timing

Aligned narrowly with official `743a3aba70a4f78f27fb53d80a6e719599d2fe5e`
(`d814582`, `faf50c2`, `7b8339e`), without importing unrelated upstream code.

- Native HTTP SSE and WebSocket deliver each parsed client event immediately,
  independent of `stream_prefetch_bytes`. There is no lower WS pre-read or grace
  clock passed to Provider. This removes local waiting, not upstream generation time.
- Empty created/in_progress, recognized response metadata and comment-only SSE
  frames are replaceable preludes. Their delivery does not close Core's replay
  barrier. Preserve provider replay proof, budgets, cancellation, deadlines and
  continuation ownership; marking a prelude does not itself authorize a retry.
- Semantic/tool/compaction output, unknown events and terminals close replay
  monotonically. A later prelude never reopens it. Preserve raw wire bytes/order
  and atomic failure handling without duplicating already-forwarded preludes.
- Restartable WS checks the first non-prelude frame for a connection-limit error
  while polling the body. The error retains its typed recovery classification.
  Exact continuations retain their existing ownership and recovery restrictions.
- Provider first-token timing follows upstream `91e8105`: first nonempty response
  data seen by the decoder, before JSON/event classification. SSE whitespace and
  `[DONE]` do not count; nonempty WS data does. First-event, first-text and
  first-reasoning remain distinct. This metric never controls delivery/retry.

## 原生 WS 参数错误的 HTTP 交付

官方 v3.53.0 的 `0922f3486c`：Provider 将单条原始 WS `error` 中的 error 对象
附到客户端响应，只适用于禁止重试的 InvalidRequest/400、原生 `/v1/responses`
HTTP SSE 入口。Core 使用同一 ResponseCommit 原子冻结：首字节未提交则返回 JSON/400；
保活或正文已经提交则保留原 SSE 错误事件，不重写 HTTP 状态、不重复发送事件。
AttemptContext 必须从实际 NewModelRequest 接收 endpoint 与 client_transport。
WS 客户端、Chat/Messages 和其他错误不得仅凭状态码套用这条规则。

## Retired Buffer Settings

Excel-only byte/grace buffering is removed. `streamPrefetchBytes` remains readable
for historical configuration compatibility and has no request runtime consumer or UI control.
Native precommit release traces retain `reason`; they no longer contain Excel byte/wait fields.

## Signatures And Error Matrix

`create_response_stream_with_pool_account(request, context, pool_account_id)`
returns an unbuffered native stream; `precommit_started_at` and the transport
prefetch parameter are removed. `PreCommitClientEvents::stage` delivers replaceable
native preludes immediately and commits semantic, unknown and terminal events.
`ProviderEvent::with_replayable_prelude` is evidence consumed by Core's independent
replay barrier, not an instruction to retry.

| Case | Delivery / retry result |
| --- | --- |
| Native empty created then approved overload | Immediate prelude; bounded recovery allowed |
| Native metadata after text/tool/unknown output | Immediate; replay remains closed |
| Restartable WS connection-limit after only preludes | Typed body error; preserve raw error, status and request ID |
| Ambiguous connection loss | Existing no-replay safety remains |
| Native missing output_item.added | First response data starts timing; text/reasoning remain independent |

Good: first prelude arrives while a oneshot gate holds the next upstream event.
Base: Native preludes retain their replay classification. Bad: calling an already
sent prelude committed business output or restarting retries after tool output.

## Required Regression Assertions

- HTTP and WS oneshot fixtures deliver under zero, saved 15 KiB, default and large
  thresholds before upstream continuation; bytes and order remain intact.
- Early failure batches exclude already-forwarded preludes. Retry budgets and
  exact continuation restrictions remain effective; ambiguous/late output is not replayed.
- Core can recover after HTTP commitment of a replaceable prelude and cannot
  reopen replay after an unknown event followed by another prelude.
- WS body-error observations retain current failure status and request ID, while
  opening diagnostics remain separate.
- Metadata and opaque data start response timing without inventing text/reasoning.
- Historical buffer settings remain readable but are absent from the UI; shared
  settings retain their save/reload and responsive-layout contracts.

Wrong: removing only Provider buffering leaves transport waiting before the
Provider sees the event. Correct: remove native waiting at both layers and keep
replay eligibility independent from physical response commitment.

## WS Size-Recovery Exception

Official `12771aca` alignment admits a bounded same-account transport directive
for typed `MessageTooBig` before semantic output even when the send state is
`Ambiguous`. Keep that factual state and `replay_is_safe` unchanged. This exception
does not authorize account rotation, exact-connection replay, late-output replay,
or ordinary ambiguous errors. HTTP fallback follows the transport requirement;
the retired administrator override cannot disable recovery. Accept historical
boolean/null overrides on load, but never serialize or apply them.
See `docs/ws-size-recovery-and-settings-audit.md` for the scoped difference ledger.

## Upstream No-More-Retry Signal (v3.45.2)

`NO_MORE_RETRY` in HTTP error bodies or SSE/WS error response headers is parsed
by the shared OpenAI protocol helper and carried as `same_account_retry_blocked`
through transport diagnostics, Provider classification and Core recovery.
It prevents same-account connection/transport fallback, transient and credential
retries, including a short Retry-After wait. Keep it in both full failure evidence
and compact recovery diagnostics. It is not blanket retry prohibition: another
account still requires the existing safe-send, continuation, output, account-switch
and request-attempt budgets. The signal cannot override a fixed owner or authorize
replaying already delivered output.

## Quota Error Contract

Preserve optional HTTP status and a bounded ASCII error code. Map known states
to static public messages; never echo raw upstream bodies. Quota endpoint auth
rejection remains separate from OAuth terminal credential evidence. Existing
authoritative quota-exhaustion classification and observation clocks remain.

## Verification And Rollback

Cover inherited/default/null, zero, custom values, invalid numbers, API and real
PostgreSQL round trips, immutable snapshots, raw event preservation, HTTP/WS
precommit recovery, semantic/tool/terminal/EOF release, timeout and late errors.
Keep QX/CPR identity, UA changes, account proxies, and continuation tests.

The precommit change itself does not alter the schema. The complete Excel retirement
uses migration 0105 to clean only retired configuration and dedicated task state;
follow the reviewed migration deployment path. Never replace the whole settings
object when reconciling a historical configuration for an older binary.

Connection-default A/B cases live in `tests/transport/http_transport_review.rs`.
After the follow-up approval, standard HTTP follows a5a844a; source-bound IPv6
retains its independent policy. Preserve the pre-alignment builder as an explicit
test-only legacy control, so current-vs-upstream does not compare two identical
builders without a baseline. Local H1/H2 success is not proof of Linux upstream
performance, idle keepalive behavior, proxy/IPv6 acceptance, or risk-control outcomes.
