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
must not make a cancelled response a valid parent. Full snapshot TTL is one hour,
immutable on repeat writes; 8 MiB per record, 2048 entries and 128 MiB aggregate.
These are optional storage admission limits, never validity checks on complete
incoming history. Unstored snapshots must not publish continuation bindings.

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

Excel image policy (`off` / `auto_compact` / `warn`) is opt-in and must not enter
native Codex preparation or transport. Default `off` returns before reading its
state store. Preserve original image byte limits even after checkpoint reconciliation.
Automatic compaction uses a genuine upstream window and one bounded continuation
on the same pinned client/egress, not proxy retry across phases. Persist a digest-only
checkpoint with CAS before delivering it. Keep new tool batches intact; reject
unverifiable windows and oversized new batches. Account for both phases, including
known usage on failure/cancellation, and preserve upstream failure events. Do not
change ordinary transport compression to simplify tests: decode the actual captured
ZSTD body in native isolation fixtures. See `docs/excel-image-policy.md` for defaults,
the retained CPR image-count definition and the exact reference revision.

| Condition | Result |
| --- | --- |
| Toggle omitted during reimport/relogin | Preserve route |
| API Key or non-OpenAI selects Excel | Reject before mutation |
| Cross-Key/account/workspace/model history | Reject, never borrow another record |
| Unknown/expired parent or switched route | Explicit continuation failure |
| Missing completed event | Failure/incomplete, never synthetic success |
| Undeclared tool | One regeneration only for eligible first unknown tool; otherwise protocol failure, no client tool execution |
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

Sub2API #148 (`207f31e4`) message attribution normalization is Excel-only and
runs after validating content in its original position. Move ordinary top-level
author/recipient into a labelled text part, preserving role/id/phase/status and
the original text/image sequence. Lower agent_message to a user message carrying
all non-content metadata with an explicit collaboration-context warning; never
grant it system/developer authority. Use output_text with empty annotations for
assistant attribution, input_text otherwise. Do not scrub tool arguments/results
recursively or mutate replay/canonical history. Normalization must be idempotent.
Image prevalidation recognizes agent_message, but retains the existing carrier,
size/count, upload and relay rules after normalization.

Sub2API #145 (`8a9c2a4e`) diagnostic alignment adds an optional client-only error
param for unsupported content, encrypted parts and malformed attributed content.
Keep original input/content/output indices despite injected messages/tool calls.
Preserve it in HTTP JSON, SSE response.failed (both error objects, also after
keepalive), and downstream WS errors. No-param responses keep their prior shape.
Do not change rejection codes/status, enable retry or native fallback, decrypt
content, or place opaque error fields in Debug/persistent diagnostics. Regression
must prove the original author-field rejection with a negative wire control,
successful normalized sends, preserved context and safe failure locations.

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
including when the same request uploads user pictures. Tool file_id and HTTPS
references follow Sub2API #139: validate the original carriers before preparation,
then move them into an immediately following user message explicitly labelled as
preceding tool output, not a new user instruction. Keep matching call-ID/image-index
markers in the original result, preserving text order, detail and signed URLs.
Inline screenshots stay in the tool result and never trigger uploads. Do not
silently remove images.
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
scope/content HMAC tokens, bounded image/aggregate capacity, 30-minute default TTL refreshed
on reuse, and request-slot RAII cleanup on cancellation/completion. Cached bytes
outlive the request and expire on stage/read or the minute cleanup loop. Never apply image admission to text or
other providers. Capability routes must not be included in ordinary URI access logs.
Per-instance temporary data requires an instance-directed public origin.

Live tests must respect the configured account request interval, including after
locally rejected requests that acquired a lease. A `RequestInterval` selection
blocker is not a route-switch or credential failure. Usage recording is asynchronous:
match the exact response ID after bounded polling, not the first list row.

Runtime image transport is an optional tagged setting in request_tuning_json:
native, or relay with a validated publicUrl. Omission/null inherits the legacy
startup origin; never reinterpret inheritance as automatic failure fallback.
Snapshot publication carries the mode and origin as one value. Provider freezes
the origin before staging, retains one signing key/store across mode changes,
and initializes temporary storage lazily. Old links remain valid until expiry.
Explicit native overrides even a configured startup relay. This setting does not
participate in account selection, fingerprinting, affinity, or native Codex.

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
Downloads remain 32 by default, independent of request slots. The disk-byte
budget defaults to 1024 MiB and cache entries to 512; these are limits, not eager
allocations. Saved runtime overrides win. The separate transient decode/download
memory budget remains 1024 MiB even when disk capacity increases.

## Account HTTP 403 Policy

`excel403Action` is the canonical enum: `none`, `pause_account`, `disable_excel`.
Legacy true means pause, false means none; reject conflicting dual-field updates.
Omitted values preserve settings. Migration 0049 backfills legacy policy and stores
mode-disable diagnostics separately from pause diagnostics. Never modify frozen migrations.
Disable-mode changes only the subsequent route to Codex, preserving enabled,
quality pause ownership, credential/identity and billing preferences. It never replays
the failed request. Pause-mode retains the existing behavior described below.
New database rows and new template/import forms default cache-write billing on;
existing values and historical usage are untouched. Routing off must not clear that
preference, and only actual Excel execution reads it. Ordinary edits must not reset it.

The default action is none and is OpenAI OAuth/account scoped.
Only the real HTTP handshake rejection can trigger it, including compact's
HTTP stage; a 403 encoded in an HTTP200 SSE payload cannot. Exclude
`basispoints_model_access_changed`. Clear retry/account-failure intents only
on this opted-in path, preserve the current response evidence, and never
replay the current request through another protocol.
Store locks config then account and conditionally changes `enabled` to false
for the current credential revision/type/flags and an enabled or quality-owned paused
row. In the latter case the independent HTTP 403 pause takes ownership, preventing
quality auto-recovery from undoing it. Config revision increments in that transaction
exactly once. Preserve the Excel route, credentials,
quotas, model list, other options and opt-in flag. No Codex fallback or in-flight
cancellation. Resume uses the existing manual scheduling control, not a timer.
Keep legacy `excelAutoDisableOn403` and `excelAutoDisabledAt` wire/storage names
for compatibility; do not rewrite historical Codex auto-disable records as pauses.
Mainline adds the diagnostic timestamp in migration 0047; retain 0046 quality
operations unchanged. Unpublished Excel branch migration numbers are not reusable.
Clear the timestamp on explicit scheduling resume/admin recovery or an actual route
change, not on re-saving the same Excel route. Manual Excel-off clears the subordinate
flag, while automatic pause preserves it. The two-second bounded write outlives
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
Structured output and undeclared tools never enter this envelope-only correction.
Separately, align unknown-first-tool regeneration with Sub2API production
`dafc174f1cfa7d600f0385f1829f04be4daaad4f`: require a nonempty catalog, expanded
history without calls/results, a typed unknown-tool error, completed output, and
exactly one run_officejs call in the last slot. Regenerate once from the prepared
body with a developer reminder before any final compaction trigger. Never append
a fake call/result, guess a tool name, or fall into envelope correction again.
Preserve the original response identity and emitted prefix; replay new message
events from the replaced slot once. Validate all corrected tools, schema, call IDs,
choice/parallel limits and any structured output before exposing executable events.
Use the current request's frozen catalog throughout either correction path.

| Condition | Outcome |
| --- | --- |
| Corrected envelope passes and preserves operations | Deliver once; cache actual delivered source |
| Added/reordered/changed operation or second invalid correction | Fail without executing tools or publishing a parent |
| HTTP rejection | Preserve HTTP provenance and account error policy, never replay |
| SSE failure | Preserve SSE error/code, not an HTTP auto-disable signal |
| Incomplete terminal, truncation, timeout or cancellation | Stop; retain observed usage, no success cache |

Normalize usage aliases before summing measured counters. Each correction
checkpoint yields an internal empty chunk which the provider consumes into
cumulative `ProviderMeteringCheckpoint` before another network await. Core consumes
it through the existing observation/metering owner, not canonical sequence checks,
commit barriers, timing or client adapters. Never relax the global validator or
fabricate Started/Completed. Core merges snapshots, not deltas; provider-reported
cost keeps its precedence. Progressive reports within one correction use per-field
maxima as the fallback when terminal usage is absent, not a sum of snapshots.
Only separate upstream calls are added together.
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
two-hour idle read TTL, 1 MiB per item, 512 items and 16 MiB aggregate.
Timeouts are bounded; an explicit request remains usable if persistence fails.

Raw `exec_command.cmd` uses FUNCTION_CMD while raw `code` support takes precedence.
Do not transform command bytes, relax schema validation, or execute them in CPR.
Corrections preserve both raw bytes and the other parameter fields.

Image limits extend existing RequestTuning and override storage: 20 MiB per image,
32 MiB deduplicated inline bytes, 20 references by default. Saved overrides win;
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

## 13. Cache Admission And Completion

Complete request input does not need to fit or be admitted to the full snapshot
cache. Missing ID-only history is different: it must still fail explicitly, not
guess content or cross owner scope. Snapshots above 8 MiB use a level-1 zstd
envelope with encoding `cpr-excel-replay-zstd-v1`, `decoded_bytes` and Base64 `data`.
Decode/encode runs on blocking workers; decoded data is limited to 128 MiB and the
stored envelope to 8 MiB. Verify the actual length, owner and record version.
Old plain JSON records stay readable. Oversized/incompressible records are not
stored and cannot publish a continuation binding; full-history requests remain
valid independently. This is compression, not deduplication or unbounded storage.

Tool receipts use independent 16 MiB / 1024-entry storage with a 1 MiB item limit,
two-hour idle expiry and access-refreshed eviction scores. Read old shared-space
receipts for compatibility without writing new receipts there. Complete client
calls can reconstruct native envelopes after receipt-cache failure; outputs
without a complete call cannot. Deduplicate reads, use bounded concurrency and
one two-second budget per read/write phase, not a separate timeout per call.

Optional snapshot-write failure cannot replace a validated upstream completion
with a protocol error. Preserve genuine usage and downstream event ordering;
Provider publishes session updates only when the snapshot persisted. This does
not promise recovery after cache eviction, timeout, cancellation or expiry.
Delta-only catalog CAS exhaustion returns a conflict after eight attempts;
explicit declarations remain frozen for that request.

Mixed tool corrections retain valid original parameters when the decoded target
type/name/namespace is unchanged, adopting only corrected call identities. A
changed target or extra call remains rejected. Assert both native cached data
and decoded downstream payload, not just the existence of completion.
The correction reader's 32 MiB response budget must not reject the input request.
Keep ordinary ingress/resource limits and the maximum correction count unchanged.

Prompt-cache measurements use actual upstream cached/input tokens, distinct from
local Redis hits. Separate cold and warm turns, HTTP byte size and token counts.
Whitespace padding tests byte admission, not high-entropy context limits.
An upstream token_revoked rejection followed by local no_available_provider is
an authentication/scheduling observation, not evidence of a context-size limit.

Current local implementation is not a full reference-equivalence claim. Never
mark live image/concurrency acceptance as passed based only on provider mocks
or a previously valid test credential.

## 14. Disk Images, Diagnostics And Recovery

Sub2API #139 duplicate comparison ignores only top-level description/defer_loading,
normalizes non-null function schema aliases in parameters/inputSchema/input_schema
order, and compares all remaining fields plus namespace/name identity. Persist the
original declaration separately from the model-facing catalog so cache round trips
retain strict and unknown execution constraints. Current/inherited annotations win
over history additions; rejected conflicts never publish a new catalog. Old lossy
records cannot reconstruct omitted constraints; explicit full catalogs remain
authoritative. Hosted-tool omission is the existing fixed-Excel policy, not an
authorization to add native fallback or change sticky account selection.

The 256-tool admission limit counts unique catalog identities, not declarations.
Compare an existing key before applying that limit to a new key; preserve all
execution-conflict checks, explicit/inherited precedence and stable catalog order.
The boundary must survive historical additional_tools and cache round trips.
Native Codex HTTP/WS requests do not use this Excel catalog admission policy.

Before releasing a completed tool batch, check both call_id and converted item id
for uniqueness, including structured-output requests without a repair sender.
Use converted identities: custom calls intentionally receive their scoped derived
item ids. Reject the whole malformed batch before any executable event, preserving
the existing bounded repair and terminal rules. Native stream processing is unchanged.

Scope: Excel image storage/settings, the existing opt-in 403 diagnostic, and
fresh quota reset recovery. Do not change selection scores, route fences,
fingerprints, credentials or configured egress.

Signatures: `excelImageRelayTtlMinutes` defaults to 30 (1..1440); migration 0046
adds nullable `excel_auto_disabled_at`, projected as `excelAutoDisabledAt`.
`ProviderCooldownPort::clear_if_observed` compares revision, deadline AND a
store-owned random observation token. Every valid incoming account cooldown
rotates the token even when its deadline is equal or shorter; retain the original
deadline, TTL and write-result semantics. Stale-revision/expired writes do not
rotate it. Missing legacy tokens cannot authorize recovery. Random generations
also fence delete/recreate ABA; model scopes and normal successful-clear behavior
remain unchanged.
Only Excel-enabled accounts participate in this new quota-reset recovery;
ordinary Codex accounts neither read nor clear cooldowns via this path.

Contracts: per-image/total bytes each allow up to 128 MiB, disk storage up to
16 GiB and 65536 entries. Validate effective defaults plus overrides using
single <= total <= storage and count <= entries. Retain the legacy one-byte
minimum for saved image budgets. Pixel limit is 64 * 1024 * 1024.
Private files retain disk and entry permits through active downloads. Decode
and download memory admission is separate. Disk staging and reads use blocking
workers, pure text does not. Publish only fully staged batches. Same scoped
content reuses its HMAC URL and refreshes configured TTL.
One weak-owned minute task prunes expired entries and abandoned directories.
Session markers hold OS file locks; only unlocked directories older than the
fixed 31-minute grace can be removed. Keep live and unmanaged directories.
Restart changes the random signing key: links are not persistent hosting.

| Condition | Expected |
| --- | --- |
| Opt-in HTTP403 disables Excel | Store timestamp in the same fenced transaction |
| Excel explicitly reopened | Clear timestamp, preserve credential revision |
| Unrelated patch | Preserve diagnostic time |
| Fresh explicit zero usage in both 5h/7d windows, allowed and not reached | Clear only the pre-fetch observed cooldown |
| New cooldown/revision or stale/partial/nonzero quota | Do not clear |
| Model-scoped cooldown | Do not clear via account reset recovery |
| Expired image with an active download | Hide new reads, retain permits until body drops |
| Disk corruption/oversize/unavailable | Bounded failure, no path/credential disclosure |

Good: complete text success survives optional snapshot failure. Base: native
requests keep their existing transport, scheduling and identity. Bad: treating
missing relay configuration as permission to delete client images.

Required tests: runtime default/persistence/relationship checks; PostgreSQL
403 atomicity, unrelated patch and reopen; Redis newer-cooldown fencing; actual
quota-fetch old/new failure race; disk TTL, corruption, download lifetime,
orphan/live lock isolation; account badge and narrow viewport rendering.

Wrong: introduce a proxy pool or fallback to native merely to match reference
Mihomo/hosted-tool policies. Correct: preserve the single configured CPR proxy,
send-state/error evidence and existing capability warnings; incompatible
optional policies need separate explicit design, not hidden defaults.

## 15. Optional Encrypted Message Omission

Sub2API #154 (`96cb37623963d4759f3b5b09b58a3a8ee49df55b`) is adapted as
account-level `excelIgnoreEncryptedContent`, default false. Only after an
actual Excel route is resolved may its outgoing request copy replace
`encrypted_content` parts in message/agent_message `content` or function/custom
tool result `output` arrays. Replace each part in place with a fixed omission
notice (`output_text` for assistant messages, otherwise `input_text`). Never
decrypt, fabricate plaintext, drop neighboring parts or change call IDs.

Run after scoped replay restore and before content/image validation. Do not
mutate the incoming payload or replay capture. Reasoning/compaction encrypted
items, tool definitions/arguments and strings are outside this option's scope.
The default still rejects unsupported encrypted message parts before sending;
other unsupported content must continue to fail validation. Native HTTP/WS,
including native models on Excel-enabled accounts, must bypass this transform.

Migration 0050 adds a false-default column without changing existing settings.
Omitted API fields preserve the saved value; explicit false disables it. An
explicit route change to Codex clears it. Credential refresh/relogin must not
overwrite it. UI single edits send only changes; batch edits require an
independent opt-in. Show a clear lossy warning: omitted content is unavailable
to the model, not recovered plaintext. Never enable accounts automatically.

Regression gate: default rejection vs opt-in for HTTP/downstream WS/compact;
native wire and identity isolation; plaintext/order/tool pairing; no-op and
idempotence; persistence/default/omission/close semantics; editor save/readback,
batch opt-in reset and narrow viewport rendering. Use synthetic local mocks,
not real BPS requests, for these tests.

Keep pure-transform regressions in the already audited `history_tests.rs` and
provider contracts under `tests/provider/contract/`. New production helper
files must not add inline test modules. Run the gateway application's
architecture suite as well as provider tests; do not broaden the architecture
allowlist to accommodate a new helper's test placement.

## 16. Narrow Tool And OAuth Wire Compatibility

Scope: a wrong CUSTOM transport marker around a valid FUNCTION envelope can
make a completed correction fail operation preservation despite unchanged
arguments. This is independent of optional encrypted message omission.

`repair::mislabeled_function` is guard-only: custom marker and JSON envelope
must explicitly name the same declared FUNCTION, with object arguments that
pass its existing schema. Only name/tool and arguments/args fields qualify;
ambiguous aliases, unknown targets, raw FUNCTION_CODE/CMD and true CUSTOM input
do not gain this exception. Decode through the existing converter, compare
canonical operations, and retain count/order/parameter/source equality. Do not
increase correction retries, execute tools, or relax batch/terminal validation.

The selected OAuth send copy uses
`request::compatibility::default_missing_format_name` and
`normalize_custom_history_ids` after account/identity selection, before transport
selection. These are common OAuth request-format fixes, not Excel-only policy.
API-key routes, canonical input, affinity, fingerprints and cache keys are unchanged.
No new setting, migration or production log is needed.

| Input | Behavior |
| --- | --- |
| Object JSON schema, name absent, existing local validation passes | Add stable `text.format.name = response` |
| Existing name, including empty/null/invalid | Preserve for normal validation; do not guess a replacement |
| Invalid/oversized/external-reference schema or invalid format fields | Do not normalize into a valid request |
| Complete plaintext paired custom history with a wrong `fc_` item id | Remove only that optional item id; keep call_id/input/output |
| Correct custom id, references, opaque/encrypted or incomplete history | No id repair |
| previous_response_id or conversation continuation | No id repair |

Good: identical long source code survives a wrapper correction, and a complete
legacy custom-call replay continues. Base: valid requests remain unchanged.
Bad: replacing argument contents, deleting all history IDs, silently omitting
ciphertext, or changing request scope to manufacture a successful response.

Required regressions assert unchanged operations and changed-operation rejection,
cumulative correction usage and one delivered call, format/id invalid boundaries,
HTTP/WS wire identity and original-payload isolation, and #154 on/off behavior.
Wrong: count a synthetic fixture or build as real upstream acceptance. Correct:
report mock regressions separately from bounded isolated live requests, including
genuine upstream rejections and any untested scenarios.

## 17. Shared Retry Budgets

- Excel initial HTTP failures reuse `websocketMaxRetries` as the same-account
  retry count, plus the existing `maxAccountSwitches` and `maxRequestAttempts`
  limits in Core. No new retry settings or provider-owned retry loop are added.
- Apply this policy only after the actual Excel route is resolved, including
  initial compact/image HTTP failures. Native Codex HTTP/WS policy is unchanged.
- An explicit complete HTTP 429 rejection may supply replay proof. Keep account
  health/cooldown isolation; never reinterpret quota, authentication, permission,
  malformed requests or SSE error status as this endpoint-level rejection.
- Other transient failures retain existing proof requirements. A proven NotSent
  connection failure may retry; ambiguous sends, delivered output and tool-repair
  failures must not gain new replay permission.
- Use existing Core transient retry ownership and backoff. Preserve Retry-After,
  respect cancellation/deadline and count every attempt against the total budget.
  Zero same-account retries replaces old transient intents with account rotation;
  hard continuation ownership and route isolation can still prohibit rotation.
- Tests must cover zero/configured budgets, HTTP provenance, no synthetic SSE
  proof, shared-quota isolation and the unchanged native regressions. This task's
  verification runs on OVH with isolated resources, never production credentials.
