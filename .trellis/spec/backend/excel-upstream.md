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
- `ExcelModels` is a validated exact list (initial sol/astra, maximum 64).
  Migration 0040 adds it without rewriting 0039. Omission preserves; empty clears.
  Freeze the model-resolved route, not just the account toggle.
- Migration 0041 adds global `excelDefaultModels` and account `excelModelsFollowGlobal`.
  Existing accounts preserve their stored custom list; new accounts follow global.
  Read stored and effective models in the same account query; only effective models
  enter Core snapshots. Global saves publish the existing configuration revision.
  Omission preserves; list-only legacy writes select custom. Following global never
  clears the stored custom list. Do not change credential or identity revisions.
- Templates carry optional Excel fields. Old templates omit them; import remains
  opt-in. Relogin `newAccountExcel` overrides templates only for new accounts.
  Existing and automatic relogin preserve Excel settings and credential CAS fences.

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
| HTTPS image request authentication failure | Preserve rejection, never drop image |
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
Exclude macOS AppleDouble `._*` archive sidecars: SQLx otherwise attempts to load
them as migration files even when every tracked source checksum matches.
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
Validate image position, carrier, encoded size and detail=auto/low/high before
staging; MIME, Base64 and dimensions before upload/generation. Relay byte admission
must still precede decoding. HTTPS references pass unchanged. User inline pictures
use the existing account-scoped attachment upload, or the optional HTTPS relay
when explicitly configured. User file_id passes with upstream authorization;
syntactic validity is not proof of ownership. Tool inline pictures pass unchanged,
including when the same request uploads user pictures. Tool file_id is rejected
locally with an actionable 400, never moved into a user message or silently removed.
Do not recursively treat tool arguments or arbitrary metadata as image content.
Generic 400/422 never permits generation replay, content removal or history reset.

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

Reference: ranxi2001/sub2api production f671a8d30c34706d8526accadf6a6ad5f40f855e.
Its image schema rejects file_id, accepts only HTTPS image_url, and relays inline input.
Our authorized live matrix found a broader position-dependent upstream contract:
user file_id and tool Base64 succeeded, while user Base64 and tool file_id failed.
Use that evidence for our adapter; do not label the reference policy an upstream limit.
Do not extrapolate success from user to tool images or from permissive mocks to upstream.
Image-bearing requests retain the vision marker. Native routes and pure text are unchanged.
An unconfigured public relay does not prevent user attachment upload or tool Base64.
The optional public HTTPS relay is not live-accepted. Hosted image_generation is unsupported.
Client image tools may send a second request to existing image endpoints, as in
Bridge v0.4.6; do not confuse that with executing OfficeJS or hosted tools.
Image endpoints select the account toggle independently of the text model list,
freeze the route, and preserve the original lease, affinity and cancellation owner.
No Codex Cookie/State or native fallback on Excel image failure. Image transport
remains HTTP JSON; do not mislabel it as SSE. Raw image traces use excel_http_json.

Responses retains user attachment uploads and the scoped file-ID cache, including
bounded cache calls, same-key upload deduplication and conditional invalidation.
Only exact attachment rejection codes invalidate used receipts; generic 422 does not.
No current generation request is replayed. No data migration or new image store is needed.
Replay stores original inputs, never substituted short-lived relay URLs. Each continuation
uploads/reuses user attachments or restages user relay pictures under its own lease,
preserving scoped replay ownership and original tool-image carriers.
When existing request tracing is enabled, emit excel.request.structure from the parsed
wire body with fixed labels and counts only. Bound input/content scanning and mark
truncation; never copy tool names, call IDs, URLs, text or encrypted payloads into facts.
This diagnostic does not validate history or prove the cause of a generic 422.

Server-generated successful compaction may prune the replay window. Retain
system/developer instructions and additional tools; never orphan pending calls
or references across a compaction marker. Clear unused window call mappings,
preserve owner/conversation/TTL and do not prune incoming explicit compact output.
Small-input rollover acceptance is not maximum-context pressure acceptance.

Default/true parallel_tool_calls permits independent calls, false must validate
at most one call before any executable event is emitted. Never silently discard
extra calls. Count contiguous client tool-result rounds before wire reconstruction.
Inert name(JSON) wrappers resolve an exact catalog callee (then optional functions.
prefix fallback). The JSON literal is the arguments object or custom string;
name/arguments inside it are payload, not a second tool envelope. Ambiguous
trailing objects or executable statements fail closed.

Image relay admission precedes base64 decoding. Bytes owners hold memory permits
through outstanding downloads, including after lease/entry deletion. Separate
download permits last through the HTTP body lifecycle; text never takes permits.
Keep acceptance limitations in feature documentation and do not claim native parity.

`excelImageRelayRequests` is a separate per-instance admission counter, default
128 (1–512). Only image-bearing relay leases consume it. Lease clones retain a
single permit until the last clone drops; decode errors release admission.
Downloads remain 32 by default, independent of request slots. The decoded-byte
budget defaults to 1024 MiB and cache entries to 512; these are limits, not eager
allocations. Saved runtime overrides win. Do not silently extend single-image,
per-request, pixel or TTL limits to match an upstream disk-backed architecture.

## Optional HTTP 403 Auto-disable

`excelAutoDisableOn403` defaults off and is OpenAI OAuth/account scoped.
Only the real HTTP handshake rejection can trigger it, including compact's
HTTP stage; a 403 encoded in an HTTP200 SSE payload cannot. Exclude
`basispoints_model_access_changed`. Clear retry/account-failure intents only
on this opted-in path, preserve the current response evidence, and never
replay the current request through another protocol.
Store locks config then account and conditionally updates only the Excel route
for the current credential revision/type/flags; config revision increments in
that transaction exactly once. Preserve credentials, quotas, scheduling,
other options and the opt-in flag. Manual Excel-off clears the subordinate
flag, while automatic-off preserves it. The two-second bounded write outlives
client cancellation; failures must not replace the original upstream error.
No per-account polling or credential guardian is introduced.

## 10. Excel Compatibility Diagnostics

Scope: only Excel request/stream conversion. `reasoning_effort` returns the
validated requested label and actual wire label. max/ultra map to xhigh,
none/minimal to low; unknown values and nonstandard reasoning modes fail.
Response reasoning.effort reports the actual value. `excel.compatibility`
records only validated labels and known unavailable hosted-tool types.

Converted plaintext function calls require `encrypted_function_args: []`,
including item-added, item-done and final response. Preserve explicit encryption
only on direct same-client-tool calls; never copy outer transport encryption
to the inner client arguments. Existing contaminated sessions are not repaired.

Auto-mode known hosted declarations are omitted with a developer capability
warning. Forced hosted choices, undeclared tools and unknown declarations still
fail. Client `required` choices need at least one call; named function/custom
choices need exactly one matching catalog call. Explicit refusals remain valid.
Enforce choices at completion before releasing tool events, project the effective
choice, and never silently substitute tools or retry through a different route.
These are prompt constraints with local validation, not upstream constrained
decoding. This does not implement hosted search/image/connector tools.

Compile function schemas once per catalog using the existing JSON Schema 2020-12
validator and network/file-denying retriever (1 MiB schema limit). Validate the
complete argument object before client delivery; do not replace full validation
with a subset of keywords. Custom grammar is still opaque. A transport wrapper's
namespace must not qualify the decoded catalog target; direct calls retain their
own namespace checks. Test internal refs, composition, bounds, boolean schemas,
extra properties, external-ref denial, forced-call identity/count and refusals.

Content errors contain only input index, fixed content/output field, part index
and whitelisted type labels. Unknown strings become unknown; missing/non-string/
non-object types get fixed labels. Never log arbitrary type text or private input.
Valid images and encrypted reasoning remain intact, and the compaction trigger
is last. Generic upstream 422 is not permission to retry or drop content.

Required tests: explicit empty versus preserved encryption; exact and aliased
callees with colliding payload fields; integers above 2^53 and u64; custom
strings; executable/multi-argument rejection; safe error paths for messages and
tool outputs; auto declarations versus forced choices; projected effective effort.
Good: a max request sends xhigh and reports xhigh. Base: native Codex unchanged.
Bad: deleting genuine encrypted history or pretending filtered hosted tools ran.
