# Excel Upstream Contracts

## 1. Scope

Account-scoped Excel generation and retirement of managed State, including
HTTP/SSE, downstream WS, tools, images, route fences and replay persistence.

## 2. Signatures

- `ResponsesUpstream::{Codex, Excel}`; optional admin `responsesUpstream`.
- Migration 0039 defaults every existing account to `codex`.
- `AttemptContext::{provider_route, freeze_provider_route}` is execution-local.
- `ProviderReplayPort` stores opaque bounded records; Redis namespace is separate.
- `transport/excel` owns protocol translation, not account selection.
- `ExcelModels` is a validated account-scoped exact list (default sol, maximum 64).
  Migration 0040 adds it without rewriting 0039. Omission preserves; empty clears.
  Freeze the model-resolved route, not just the account toggle.

## 3. Contracts

Only OpenAI OAuth may select Excel. Omission preserves an existing selection.
Do not reset credentials, scheduling, group membership or proxies on toggle.
Preserve Core scoring and capacity; both selectors must filter by frozen route.
No retry can cross Codex/Excel. No Excel upstream WS or Codex Cookie/State.

Replay binds Key/account/user/workspace/model; explicit parent means incremental.
Core publication and Provider storage are separate guards. A provisional record
must not make a cancelled response a valid parent. TTL is one hour, immutable on
repeat writes; 8 MiB per record, 2048 entries and 128 MiB aggregate.

Transport observations must retain `excel_http_sse` after generic SSE facts are
merged, including pre-stream rejection. Record client transport separately.

Managed State workers, eligibility gates and WS version/expiry constraints are
retired. Historical storage remains compatible; native protocol state and shared
Cookie/relogin identity protections are not the retired collector.

## 4. Validation Matrix

| Condition | Result |
| --- | --- |
| Toggle omitted during reimport/relogin | Preserve route |
| API Key or non-OpenAI selects Excel | Reject before mutation |
| Cross-Key/account/workspace/model history | Reject, never borrow another record |
| Unknown/expired parent or switched route | Explicit continuation failure |
| Missing completed event | Failure/incomplete, never synthetic success |
| Undeclared tool | Protocol failure, no client tool execution |
| Unsupported effort/tool choice/warmup | Explicit request error |
| Image upload authentication failure | Preserve rejection, never drop image |
| Old State enabled but empty | No State scheduling gate |
| Existing account upgrades with legacy State enabled | New Excel route remains `codex`; identity unchanged |

## 5. Cases

Good: downstream WS continues through HTTP/SSE using a scoped, published parent.
Base: switch off uses the original Codex transport and scheduling contracts.
Bad: globally changing base URL, dropping unknown parents or using global call IDs.

## 6. Required Tests

Run workspace/store/provider regressions with real isolated PostgreSQL and Redis.
Check route omission, unchanged credentials, both selectors, client header filtering,
real completion, sequence monotonicity, scoped replay, TTL and capacity eviction.
Browser checks cover menu/edit/batch, legacy State absence and narrow layouts.
Live acceptance covers text, tools, images, WS second turn/reconnect, cross-Key,
switching and cancellation using only the authorized test environment.
Keep raw credentials and traffic outside the checkout.
Isolated test runners must pass only `CPR_TEST_DATABASE_URL` and
`CPR_TEST_REDIS_URL`, not application configuration overrides. The bootstrap
integration suite requires loopback service endpoints. After source transfer,
verify content equality and invalidate stale timestamp-based build artifacts.
Private Excel protocol tests have exact owner/file/module allowlist entries;
do not add public test hooks or exempt the whole provider directory.

## 7. Wrong vs Correct

Wrong: a static Excel model descriptor proves the account has model permission.
Correct: it declares adapter capability only; actual upstream authorization wins.
Wrong: label Excel in initial metadata and let subsequent SSE observation replace it.
Correct: apply the route label to response observations while retaining response facts.

## 8. Compatibility Completion

Tool envelopes may be normalized only without guessing tool names or missing values.
Full history can rebuild a native wrapper; output-only history still requires scoped
cache evidence. Cached call IDs must match the full canonical client call contents.
Validate all tools and terminal completeness before releasing executable events.
JSON Schema validation is local, with no HTTP/file retrieval. Only structured text is
held until terminal validation, not ordinary text. Never claim constrained decoding.

Excel permission/model failures must not inherit status-only 429 cooldown or replay.
Preserve true status, upstream evidence and final client response. Other routes stay unchanged.
Compact retains the same selection/lease/cancellation/metering boundaries and requires
a genuine encrypted compaction result.
`tool_choice: none` is consumed by the local tool adapter. Do not add it back to
the prepared Excel wire body, including explicit compact; the upstream rejects
that extra field. Keep `compaction_trigger` as the final input item.
Generic 400/422 validation failures are not evidence of an image-URL rejection;
only explicit image error codes permit the bounded attachment fallback.

The optional image relay belongs to the provider; Core exposes only a neutral download
capability and API serves bytes. Default off, HTTPS origin only, no filesystem paths,
random per-request tokens, bounded image/aggregate capacity, five-minute maximum TTL,
and RAII cleanup on cancellation/completion. Never apply image admission to text or
other providers. Capability routes must not be included in ordinary URI access logs.
Per-instance temporary data requires an instance-directed public origin.

Live tests must respect the configured account request interval, including after
locally rejected requests that acquired a lease. A `RequestInterval` selection
blocker is not a route-switch or credential failure. Usage recording is asynchronous:
match the exact response ID after bounded polling, not the first list row.

## 9. Removal and Acceptance Boundaries

Feature identity is `excel-upstream`; follow `docs/excel-removal.md` for removal.
Do not revert the whole introducing change: managed State retirement must remain.
Preserve applied migrations, shared scheduling/identity/Cookie contracts and
historical `ExcelHttpSse` usage decoding. Delete neutral replay/image/route ports
only after checking for additional consumers.

Inline image acceptance currently fails with generic upstream 422; the public
HTTPS relay is not live-accepted. Native image generation is unsupported.
Explicit compact success is not proof of automatic history rollover: replay still
appends inputs/outputs without replacing old history after compaction. Long-context
limits and pressure behavior are not live-accepted. Keep these limitations in PR
and feature documentation; do not claim full parity with native Codex.
