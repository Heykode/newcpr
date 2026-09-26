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
- `ExcelModels` is a validated exact list (fallback astra/sol/terra, maximum 64).
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
- Share the three-model frontend fallback across settings/edit/batch/template/import/
  relogin. Stored values, including empty and migration-seeded lists, always win.
  Never rewrite applied migrations merely to replace a presentation fallback.

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

Confirmed Excel HTTP 401 enters the existing expiry/revocation policy before any
failure event is yielded, including attachment staging and either tool-repair path.
Persist only a fixed authentication reason in a detached two-second-bounded task;
preserve downstream evidence without enabling replay. Install an account/revision
runtime block synchronously before awaiting persistence. Check both selectors,
including after affinity awaits; a late old-revision failure cannot block new tokens.
Refreshable ordinary-401 fallback blocks last at most ten minutes; revoked/no-refresh
blocks require new credentials or successful diagnostics. Expiry of a runtime block
does not clear persisted credential state. Normal late successes cannot clear an
active block; diagnostics compare the observed block before clearing. This local
fallback is not a distributed replacement for the existing state store.
Keep 403 auto-disable unchanged; do not add #105 group migration.

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
Validate image position, carrier, encoded size and detail=auto/low/high/original before
staging; MIME, Base64 and dimensions before upload/generation. Relay byte admission
must still precede decoding. HTTPS references pass unchanged. User inline pictures
use the existing account-scoped attachment upload, or the optional HTTPS relay
when explicitly configured. User file_id passes with upstream authorization;
syntactic validity is not proof of ownership. Tool inline pictures pass unchanged,
including when the same request uploads user pictures. Tool file_id is rejected
locally with an actionable 400, never moved into a user message or silently removed.
Do not recursively treat tool arguments or arbitrary metadata as image content.
Generic 400/422 never permits generation replay, content removal or history reset.

The only encrypted-history exception follows Sub2API #118 at 3bfce058:
an initial Excel HTTP 400 with explicit invalid_encrypted_content (or its strict
uncoded verification/decryption diagnostic) may retry once before streaming.
First sends preserve ciphertext. Only remove nonempty encrypted reasoning items
from the prepared request copy, never canonical/replay history. Reject recovery
if other encrypted carriers remain or no user/assistant/tool history survives.
Keep compaction, messages, images, tools, metadata, cache key, model and effort.
Reuse the pinned account/egress and uploaded attachments, honoring cancellation;
never reselect accounts, retry native routes or handle a 200 SSE failure this way.
Do not nest this recovery into tool-format correction sends. Second failures retain
normal error/401/403 handling. Diagnostics contain no ciphertext. Excel-only model
descriptors explicitly null both multi-agent encrypted capability fields; native
catalog capabilities remain unchanged.
For mixed pools apply the restriction after raw catalog caching using the frozen
client scope plus enabled OAuth Excel model policy, not the sampled metadata
account alone. Temporary credential/quota failures must not re-enable v2. Include
Excel mode/model selection in catalog-only cache keys so switching off recovers
the native document; do not change inference cache keys or account selection.

Sub2API #117 normalization belongs in the native final HTTP/WS body preparation:
remove only top-level external_web_access from web_search* tool entries. Never
recurse into function schemas, change identity/fingerprints or apply it to Excel.

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

## 11. History And Bounded Tool Correction

Scope: Excel protocol only. `replay::restore` accepts the complete source map
and returns the effective `ClientTools` with restored input. Rebuild uncached
CUSTOM/FUNCTION_CODE history from that catalog; preserve matching native cache
records and raw source bytes. A missing call reports only its input position.

`transform_stream_with_repair` accepts an optional account-bound HTTP sender.
Only a completed, wholly unexecuted native tool batch can request correction:
two attempts maximum, original model/lease/egress, no extra scheduler selection.
Keep valid operations and raw code plus metadata, original output slots and
response identity. Validate the whole batch before exposing executable events.
Structured output never enters correction. One unknown native call may enter a
separate single correction only before any tool interaction and with a nonempty
frozen catalog. It must be the last output item; never fabricate a tool result or
chain this correction into the two-attempt format repair loop.

| Condition | Outcome |
| --- | --- |
| Corrected envelope passes and preserves operations | Deliver once; cache actual delivered source |
| Added/reordered/changed operation or second invalid correction | Fail without executing tools or publishing a parent |
| HTTP rejection | Preserve HTTP provenance and account error policy, never replay |
| SSE failure | Preserve SSE error/code, not an HTTP auto-disable signal |
| Incomplete terminal, truncation, timeout or cancellation | Stop; retain observed usage, no success cache |

Normalize usage aliases before summing measured counters. Each correction
checkpoint yields an internal empty chunk which the provider consumes into
cumulative Usage/CalculatedCost facts before another network await. Core merges
these snapshots; they are not deltas, client wire events or first-token signals.
This preserves known usage when Core's cancellation boundary wins before the
provider can report its own cancellation. Clear the recorder on aggregated
completion; error/deadline paths take any unreported snapshot once. Cancellation
drops the sender future rather than spawning background generation. Ordinary
text stays incremental.

Good: repair a missing CUSTOM marker without changing raw source. Base: valid
tools, image position rules and the native route retain existing behavior.
Bad: repair parameter values, retry a 422 without images, or erase encrypted
history. Required regressions cover metadata preservation, duplicate identity,
batch atomicity, aliases, cancellation, incomplete/SSE/HTTP errors, replay
success/failure, structured bypass and original image detail.

## 12. Catalog And Resource Hardening

`restore_scoped` freezes the effective catalog per request. Explicit tools replace,
including an empty array; omission inherits an immutable parent snapshot first,
otherwise a trusted session hint. Additional declarations use only the current
delta, so old history cannot resurrect an explicitly cleared catalog. `none`
suppresses the current turn without erasing reusable declarations.
Hints share the response owner isolation and add the trusted session identifier.
Do not key them by anonymous fallback text. Redis CAS uses a random version nonce,
one-hour non-sliding read TTL, 1 MiB per item, 512 items and 16 MiB aggregate.
Timeouts are bounded; an explicit request remains usable if persistence fails.

Raw `exec_command.cmd` uses FUNCTION_CMD while raw `code` support takes precedence.
Do not transform command bytes, relax schema validation, or execute them in CPR.
Corrections preserve both raw bytes and the other parameter fields.

Image limits extend existing RequestTuning and override storage: 4 MiB per image,
6 MiB deduplicated inline bytes, 16 references by default. Saved overrides win;
validate both admin writes and the provider snapshot. Reference count includes
HTTPS/file IDs; byte checks do not fetch external resources. Replay/request bounds
remain independent. Upload admission is 32 requests and 60 seconds including wait;
text takes no permit. This is not a total process decoding-memory guarantee.

Excel fallback context_management uses 920000 only when omitted. Preserve every
explicit client value, including an empty array. This is not a proven model limit.
Ordinary Excel endpoint 429 preserves wire evidence and Retry-After but cannot
cool down or rotate native Codex accounts. Genuine quota/auth categories retain
their recovery policy. Off/native routing is unchanged.

Capture transport facts as fixed labels only. Terminal business outcomes are
independent of HTTP status; an unverified completed event is not proof of success.
Do not copy error strings or URLs into the fixed-label capture facts.
