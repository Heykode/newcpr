# Protocol Compatibility Contracts

## Native Search History

Upstream #478 applies only to OAuth native Responses with complete history.
When input contains `web_search_call` but neither top-level nor historical tools
declare search, add cached-only `web_search` (`external_web_access=false`). Lite
uses a separate developer `additional_tools` item before the final compaction
trigger; ordinary requests append top-level tools. NewCPR has no plugin header
override, so `responses_lite` is the effective flag.

The inserted top-level tool index is private request state: HTTP/WS parameter
cleanup preserves only this synthesized cached-only flag. Client-provided search
options still follow the existing normalization; never accept this marker from JSON.

No existing tools means `tool_choice=none`; existing tools retain their choice,
including required/function choices. Do not modify native previous-response
continuations, Excel, malformed tool structures or already-declared search.
Preserve original history/IDs and idempotence. HTTP/WS capture tests verify the
wire payload; synthetic success is not proof of a particular production 500 cause.

## Ordered Request Field Removal

- Responses ingress consumes `type` and the recognized boolean `use_websocket`
  with `shift_remove`, preserving all remaining JSON object fields in their
  original order. Window rollover removes only `previous_response_id` the same way.
- Do not change transport selection, session ownership or account identity while
  aligning this serialization behavior. Unknown `use_websocket` values retain the
  existing passthrough contract.
- OAuth authorization query order follows the pinned official Desktop contract:
  PKCE fields, then state/scope, then organization/simplified-flow flags. Keep
  NewCPR's existing values, callback and correlation validation.
- Unlike the upstream deletion, NewCPR still consumes passive engine timing in
  diagnostic `providerMetadata.upstreamTimingMetrics`; preserve it. The optional
  timing header is already absent unless supplied, so do not add default injection.
- Test frame type at beginning/middle/end, nested unknown-field order and local
  transport-switch removal, plus the existing ordered OAuth query assertion.

## Native Effort Alias And Invalid Encrypted History

- Native OAuth Generate maps trimmed `minimal` to `none` after selected-account
  scoping, for HTTP and WS alike. This is an explicitly approved alias, not a
  capability promise for all models. Preserve missing/other efforts, summaries,
  immutable ingress, Excel's separate mapping and account selection inputs.
  Persist requested/effective efforts through existing requestSummary fields.
- Adapted from ranxi2001/sub2api production `17c88b0124d6703ef8ff3d229f461c35ff0a4e82`:
  explicit invalid_encrypted_content can trigger one same-lease recovery before
  any client event, under the original deadline/cancellation. Ordinary retry
  settings do not repair a payload and are not modified by this repair.
- Share a request-local transport pin between the initial send and this sequential
  repair. Retain the selected IPv6 route, frozen profile and fresh-mode WS pool
  identity; cloning an account client alone would select another IPv6 address.
  Revalidate account/source availability without advancing the rotation cursor.
  Account rebinding clears the pin; ordinary requests keep their existing policy.
- Remove only rejected reasoning ciphertext, retain available plaintext summary,
  and avoid dangling stateless IDs. Do not remove compaction ciphertext, encrypted
  tool arguments, item references or exact/unknown continuation dependencies.
  Keep persisted previous_response_id: absence of tool output does not prove that
  input contains full history. No account rotation, proxy reselection, UA change,
  TLS change, tool replay after delivery or image-search normalization is added.
- After a completed recovery, retain only SHA-256 digests scoped to selected
  account, downstream Key and local conversation. Bounded in-process retention
  expires after one hour; eviction/restart merely loses the optimization, not
  account eligibility or session ownership. Failed/incomplete/cancelled recovery
  must not populate it. New ciphertext and other scopes remain untouched.
- Test real loopback HTTP 400/SSE and WS, immutable input and wire identity,
  one-shot termination, cancellation/zero buffer/post-output behavior, cache
  isolation/expiry, original/effective effort reporting and Excel regressions.
  Direct test-host capability probes do not prove candidate production deployment.
- Private recovery-clock and request-normalization unit modules must be registered
  by exact owner/name in the App architecture audit. Preserve test-only/private
  visibility and rejection of path/cfg_attr hooks. Run the App architecture suite
  as well as provider tests before publication; provider tests alone cannot catch
  a missing registration.

## Official Chat Request Contract

The user authorized replacing CPA Chat conversion with official CPR on 2026-10-29.
Use the pinned upstream request converter in gateway-api/openai/chat/request.rs.
Flat reasoning_effort maps to Responses reasoning.effort. Non-null nested reasoning,
reasoning_content and Responses-only continuation fields are rejected; optional null
values follow official defaults. Do not restore old aliases or custom tools.
Keep native Responses request normalization independent.

## Downstream Environment and WS Error Projection

- Strip downstream `x-stainless-*`, `sec-ch-ua*`, `sec-fetch-*`, Origin and
  Referer at the existing ingress/provider passthrough boundaries. Do not
  broaden this to unknown business headers, traceparent/tracestate or bodies.
- Extract `session_id` alias semantics before dropping its raw passthrough.
  The selected account/Key projection remains the sole owner of generated
  session/thread/installation headers, including the regenerated QX aliases.
- Native WS bare errors may become `response.failed` for delivery, using the
  observed response snapshot. Preserve consumable status errors and special
  continuation/connection-limit control codes. Do not change canonical
  failure, billing, finish-gate or SSE semantics.
- Cover real HTTP/WS outbound headers, repeated downstream WS frames, opaque
  business bytes, unchanged identity and single terminal failure delivery.

### Organization Extension Headers

#### Scope And Signatures

API `passthrough_header_name` and Provider `provider_managed_header` own the
existing request filters. No public API, database or runtime configuration changes.

#### Contracts

- Match merged upstream PR 117 (`1e6d69a3499ea67762289042aa84e22745f2cb27`)
  for `chatgpt-organization-id`, `chatgpt-org-id`, `x-openai-organization` and
  `x-openai-project`: preserve them as opaque request extensions at both API
  ingress and Provider decoding. Keep duplicate values and existing transport
  representability rules; Connection-nominated fields remain hop-by-hop.
- Continue filtering `chatgpt-project-id`, `openai-organization`,
  `openai-project`, authentication, cookies and account IDs. Installation,
  session and fingerprint ownership stay unchanged.
- This request-side alignment does not relax response metadata redaction and
  does not grant per-frame mutation of WebSocket opening headers.

#### Validation Matrix

| Input | Result |
| --- | --- |
| Four extension names, mixed case and repeated values | Preserve normalized names and values |
| Extension nominated by Connection | Drop at ingress |
| Existing managed organization/project/credential header | Keep existing filter |
| Non-ASCII header bytes | Preserve over HTTP; retain existing WS representability rule |

#### Good, Base And Bad Cases

Normal extensions survive both filters; ordinary requests stay unchanged.
Relaxing all organization headers or allowing downstream account credentials is wrong.

#### Tests Required

- Cover API decoding and real loopback HTTP/WS output, duplicates and identity.
- ID-preservation tests must include messages, tool calls, reasoning, encrypted
  history, unknown/shorthand items, phase, references and explicit continuation.
  A mock success proves outgoing
  encoding, not upstream acceptance or production error-rate improvement.

#### Wrong Versus Correct

Wrong: delete the response-side redaction list or all project-related filters.
Correct: remove only the four request-side rules verified against the pinned upstream.

## Scope

Apply when modifying Chat conversion, Responses HTTP delivery, explicit compaction
or multipart image editing. These contracts extend, not replace,
`proxy-and-delivery-contracts.md`.

## Ownership

- `gateway-api/openai/chat/{request,response}.rs` owns the copied official JSON
  conversions, never networking, account selection, persistence or billing.
- `gateway-api/openai/chat` decodes and calls the existing model-aware Generate
  service. HTTP delivery uses the same pending execution and cancellation driver.
- `Operation::Compact` is explicit and model-aware. It must not use model-less
  standalone endpoint admission or silently become a Generate operation.
- Images multipart normalization feeds the existing raw JSON image path.

## Delivery

- Chat defaults to buffered delivery. Downstream stream and upstream transport are
  independent. Conversion must not add internal HTTP round trips.
- The official converter ignores the old Chat-only use_websocket extension.
  Native Responses transport controls remain unchanged.
- Use official ChatIdentity based on the gateway request ID, creation time and
  requested model; do not switch identity when an upstream response changes.
- Preserve official text/refusal/function validation, snapshot prefix checks,
  duplicate identities, argument strings and incomplete reason handling.
  Do not restore CPA image/custom/reasoning output or digest-based extensions.
- response.created can emit the initial assistant role chunk. Unknown auxiliary
  events can be skipped, including after terminal; business events after terminal fail.
- Before commit, output conversion failures return JSON 502. After commit, return
  Chat error SSE and DONE. Never emit success finish/usage before the original
  execution finalizes; verify trailing raw SSE, cancellation and commit failure.
- Preserve absent usage without invented counters. Canonical accounting stays in
  the engine. Native Responses keeps its transparent event/error behavior.
- Chat uses the copied official collect_response helper before projection. Empty
  terminal output can use only complete, contiguous, nonconflicting done items
  from the same response. Do not apply legacy delta recovery to Chat.
  Native Responses keeps its separate existing recovery path.
- Source equality of the request/response codec is not endpoint parity. Audit
  admission, first-event waits, upstream HTTP failures and finalization separately.
  After successful admission, Chat uses the official 15-second initial keepalive
  and optional Chat error encoder while native Responses keeps the default encoder.
  Poll the same pending future without restarting it; a heartbeat records HTTP
  status but must not commit business delivery or synthesize a successful result.
- Before HTTP commitment, preserve client-visible upstream bytes, content type,
  status and forwardable headers through engine_error_response. Do not rebuild a
  Chat-only generic error and discard recovery fields. Keep existing capacity
  projection and failure-request-ID precedence over opening IDs.
- After initial keepalive, use the copied official chat_error_frame for Chat
  errors, preserve recovery fields and Retry-After, then emit exactly one DONE.
  Do not send response.failed to Chat. Admission rejection still returns its
  original HTTP error, and native Responses retains its existing SSE contract.
- Regress a four-minute first-event wait, both early and late errors, JSON and
  non-JSON bodies, capacity projection, cancellation and admission rejection at
  the endpoint. Exercise native Responses alongside Chat; codec-only tests do not
  cover these integration boundaries.

## Compaction

- Require model and complete input, preserve opaque encrypted output and future
  fields. Reject non-null previous_response_id until owner-safe routing exists.
- Use the same account/model constraints, affinity, identity, proxy and capacity
  rules. No automatic compression or input-token pre-counting.
- A successful HTTP envelope alone is insufficient: require valid nonempty
  compaction output. Missing billing facts cannot be synthesized.

## Uploads

- Authenticate before consuming multipart. Bound the form to 64 MiB, each part
  to 50 MiB and input images to 16.
- Parse with multer, validate file signatures and MIME, reject duplicate scalar
  fields/masks/indexes and mixed indexed/unindexed inputs.
- Normalize to images[].image_url and mask.image_url data URLs without fetching
  remote content, writing files or adding a second model orchestration path.
- Local JSON equivalence is not proof of private Codex image-edit support.

## Standalone Account Identity

### Scope / Trigger

- Image generation/edit, standalone search and explicit compact share
  `execute_raw_json_endpoint`. After selecting the lease, scope the outgoing
  body from the original immutable payload to that lease's installation ID.
  Re-execution on another account must not reuse an already-scoped body.

### Signatures

- `scope_raw_json_to_account(body: &Bytes, installation_id: &str) -> Bytes`
- `scope_raw_turn_metadata(raw: &str, installation_id: &str) -> Option<String>`
  is the header projection; body metadata retains its own JSON whitespace.

### Contracts

- Reuse the existing Responses account-identity and account-bound-state key
  lists. Remove those keys at the root and in object-valued `client_metadata`;
  replace existing installation aliases, including duplicate/escaped keys.
  Do not invent missing installation fields, client metadata or request headers.
- Scope known identity fields in JSON-object turn-metadata strings. Header
  projection remains ASCII, but unknown duplicate members and number lexemes
  survive. Header projection removes actual CR/LF outside strings after JSON
  validation; escaped newlines inside strings survive. Opaque/nonstring body
  turn metadata remains opaque; undecodable header metadata is omitted as before.
- Do not descend into input/history, images, commands, tools, ordinary
  `metadata`, or future business extensions. Session correlation fields such
  as search `id`, image `session_id` and `conversation_id` are not account IDs.
- Use borrowed raw JSON member values rather than a whole-object Value/Map
  round trip. A clean body remains byte-identical. For changed bodies, retain
  untouched member bytes, duplicate unknown keys, precise numbers, escaping
  and order. Apply the change to every duplicate known identity member.
### Validation & Error Matrix

| Input | Outcome |
|---|---|
| Malformed/nonobject raw body | Existing passthrough, no new validation/size gate |
| Opaque/nonstring body turn metadata | Preserve existing value |
| Invalid/nonobject header metadata | Omit header as before |
| Pretty JSON header metadata | Scope, ASCII-escape, remove actual CR/LF, send |
| Compact model denial/nonempty previous response | Existing rejection before projection |

### Good / Base / Bad Cases

- Good: clean body returns the original bytes; changed bodies retain unknown
  duplicate members and precise number lexemes.
- Base: an existing installation alias becomes the selected account's value.
- Bad: an old account ID or turn state survives at an owned scope. A similarly
  named field inside business `input` is not this failure and must survive.
- Compact's existing model mapping can normalize its payload independently;
  the byte-preservation contract here belongs to identity projection.

### Tests Required

- Verify actual loopback outgoing bytes for all four endpoints, clean/dirty
  payloads, two-account switching and immutable original input. Keep the
  existing raw upstream error, proxy/IPv6, single-completion, Chat/Responses
  and WS account-isolation regressions.

### Wrong vs Correct

- Wrong: deserialize the entire payload to a map and serialize it again, or
  reuse the prior attempt's scoped body after switching accounts.
- Correct: `scope_raw_json_to_account(&request.body, lease.installation_id())`
  after lease selection, borrowing untouched member bytes from the original.

## Evidence

- Exercise real local New API converter fixtures, retaining their provenance.
  Converter output is not a captured production channel request.
- Test API delivery and actual provider encoding, not only intermediate JSON.
  Existing Codex normalization removes max_output_tokens and temperature; do not
  assert these controls are enforced because the Chat converter maps them.
- Compile before timing-sensitive socket suites. Missing database fixtures are
  not successful integration tests. Real upstream capabilities require accounts
  and separate approval; mock completion must never be reported as production
  acceptance.

## Official Chat Parity

### Source and Allowed Adaptation

Pinned upstream: zyycn/codex-proxy-rs
d89db84a77d0143a91770b66bec47ab8f2e57a35 (v3.35.1).
Copy request.rs and response.rs; only map TokenUsage cached_input_tokens to
cached_tokens and reasoning_output_tokens to reasoning_tokens. Formatting is
allowed. NewCPR uses native ExecutionSession instead of upstream plugin middleware;
this is an executor binding, not permission to change the official codec.

### Validation Matrix

| Input/state | Required result |
| --- | --- |
| Standard text/image input/function call history | Official Responses conversion |
| Optional null fields | Official absent-value defaults |
| Chat custom/search/image-generation tool declarations | Request error before account execution |
| Chat file or public reasoning-history extensions | Official unsupported request error |
| Invalid tool ID/name/argument types or conflicting identity | Conversion error, never success |
| Unknown/missing incomplete reason | Conversion error, never stop |
| Unknown auxiliary events before/after terminal | Ignore, retain one finish/usage |
| Business output after terminal or conflicting snapshot | Conversion error |
| Terminal-first reply followed by finalization failure | No successful finish/usage |
| Raw SSE split across UTF-8 boundaries | Decode incrementally without corruption |
| Malformed trailing SSE | Failure, not a successful terminal |
| Wire failure followed by engine failure | One error and one DONE |
| Body dropped | Cancel and finalize the original execution |
| Native Responses custom/search/image tools | Original transparent behavior |

### Required Tests and Limits

Port upstream request/response tests to a native executor fixture. Cover atomic
delivery batches, eligible pending-commit deferral, auth before decompression,
headers/context, commit failure, cancellation, keepalive, EOF and single finalization.
Run native Responses and actual provider HTTP/WS regressions separately.
Do not preserve obsolete CPA-only acceptance tests as a product contract.

A mocked completion does not prove real account permission, cache-hit improvement
or reduced account risk. Detailed provenance and binding differences are recorded
in docs/chat-official-parity.md.

## Buffered Output Recovery

### Scope and Signatures

`openai::output_recovery::recover_response_output(&mut Value, borrowed_events)`
owns pure JSON recovery for native Responses complete-JSON delivery.
The caller still decides terminal/failure status. Native SSE/WS and official Chat
are not recovery consumers; Chat uses its copied upstream collection contract.

### Contracts

- A nonempty terminal output remains authoritative. Only empty tool input may
  be supplemented from the same call; never append unrelated historical items.
- With empty/missing output, prefer each raw done item, retaining opaque search,
  image and future fields. Recover other identifiable items from received text,
  reasoning or tool deltas, not invented success or image previews.
- Sort indexed items, retain unindexed arrival order and deduplicate only with
  identity evidence. A missing index is not zero or proof of another item's
  identity. Distinct call IDs must never share argument recovery.
- Use borrowed event references; do not create a second full event/image cache.
  Missing status on a native terminal does not itself prohibit recovery.

### Validation and Error Matrix

| State | Result |
| --- | --- |
| Correct nonempty terminal | Preserve its output and envelope |
| Empty terminal with done and independent delta items | Keep both, preferring each item's done |
| Empty tool input with matching identity | Restore actual received arguments/input |
| Conflicting call identities at the same position | Do not copy arguments between calls |
| Unindexed done beside a distinct indexed delta | Retain both; do not infer shared identity |
| Failure or missing terminal | Existing failure contract; never synthesize completion |
| Real empty terminal without recoverable content | Preserve existing empty-result behavior |

### Examples and Required Tests

Good: recover a complete image item without decoding its payload or losing
unknown fields. Base: a complete correct terminal stays unchanged.
Bad: replay a different tool's arguments merely because its index is reused.

Required tests cover mixed done/delta, missing IDs/indices, ordering, duplicate
events, custom/function identity conflicts, absent status, unknown fields,
native Responses JSON delivery, unchanged native/Chat streaming, and proof that
official Chat buffering uses its own copied upstream collector instead.

Wrong: enable recovery in a stream encoder or use canonical metering to invent
client output. Correct: recover only the complete response from borrowed wire
events, then use the existing encoder and single execution finalization.

## Codex Generate Compatibility

- Normalize explicit `input[]` items with `type=message` and `role=system`
  to `role=developer` in `adapt_codex_responses_body`, on the encoded copy.
  Preserve the original protocol payload, item order/content/extensions,
  nested roles, shorthand messages and top-level instructions. HTTP and WS
  share this encoding owner; do not duplicate role conversion in send paths.
- Align with upstream `f3236d041098bfa49698a95d3dc4b63174563181`:
  default absent `store` to false, preserving explicit true/false/null and
  unknown values. Internal store interpretation, transport and session capture
  must read this same encoded value. Explicit true follows existing persisted
  continuation semantics; missing/false retains replay/connection-local guards.
- Remove only `max_output_tokens`, `temperature` and `prompt_cache_retention`.
  Preserve `max_completion_tokens`, `top_p`, `frequency_penalty` and
  `presence_penalty`, leaving validation to the upstream. Do not apply this filter to
  Compact, Search, Images or other providers.
- Convert string input during encoding on a copy, before account selection.
  The existing string/list content-anchor equivalence must keep routing,
  session and cache identity unchanged. Preserve the original protocol payload.
- Preserve call_id links, references, encrypted history, compaction and unknown shapes.
  On the selected OAuth send copy only, `normalize_message_history_ids` repairs the
  rejected `item_` prefix to `msg_` for complete stateless `type=message` items.
  Require a recognized role and content, a unique source ID, no target collision,
  an ASCII alphanumeric/underscore/hyphen suffix and a resulting length at most 64.
  Stored/conversation/previous-response histories and unknown input item types are
  excluded. Duplicate/reference IDs, reasoning IDs and tool IDs remain untouched.
  Account selection, identity derivation and the client's original body precede this repair.
- `normalize_plaintext_reasoning` may move nonempty content consisting solely of
  `{type: reasoning_text, text: string}` parts into an absent/null/empty summary.
  Preserve order/text; do not overwrite existing summaries or drop unknown fields.
  Only missing/null/empty ciphertext permits this conversion. An unreferenced `rs_`
  ID is removed in stateless mode to avoid an impossible store lookup; explicit
  store=true retains it. Never alter the encrypted reasoning ID or ciphertext.
- Create only client_metadata["x-codex-installation-id"]. Replace existing
  installation_id/installationId aliases but never create missing aliases.
  Keep the selected durable installation, credential version and all session,
  cache and pool algorithms unchanged.
- Retain same-account opaque metadata mirror scoping and ASCII serialization
  after local identity projection. These prevent stale device aliases and lost
  Unicode-bearing WS headers; they do not prove upstream error-rate improvements.

### Validation

- Assert role normalization immediately after encoding, original payload
  immutability, and matching HTTP/WS loopback wire bodies.
- Test absent/false/true/null store through HTTP SSE and WebSocket, including
  captured continuation scope and downstream WebSocket transport requirements.
- Test unsupported-field removal, string/list input equivalence, tool-call/result
  linkage and encrypted/compaction history preservation.
- Test account switching after request normalization; no normalized body may
  be reused as the source for a later account.
- Compare missing/legacy/malformed installation aliases through real HTTP/WS
  requests: account, device, fingerprint headers, affinity, seed and cache key.
- Initialize the Provider with an isolated persisted identity secret when
  verifying generated lc_ conversation IDs; comparing two null values is not
  evidence of stable identity. Require canonical response completion.
- Removing a legacy alias on a subsequent request must reuse the same eligible
  socket; a different client key or account must not reuse it.

### Request error classification and retry exhaustion

- `CodexUpstreamFailure` parses `error.param` for HTTP and streamed failures.
  Structured parameter rejection codes, or `invalid_request_error` with a non-model
  parameter, take precedence over broad model wording for 400/422 or statusless
  stream errors. Explicit authentication/429 handling remains intact.
- Apart from the explicitly approved native `minimal` alias above, no model-effort
  mapping or silent downgrade is introduced. The actual upstream remains
  authoritative; other efforts pass through unchanged. Invalid parameters
  must not update account/model health or supply account-rotation replay proof.
- Statusless `invalid_request_error` retains request scope after specific auth,
  model, quota and capacity classification. Unknown tool/parameter capabilities are
  not manufactured by deleting the user's options.
- Core routing/account-switch exhaustion consumes `last_retryable_failure` via
  the same `finish_retained_failure` path as empty account selection. Preserve raw
  response, status and atomic terminal events exactly once, without another attempt.
  Cancellation/deadline and unrelated provider infrastructure errors keep their own
  semantics; existing replay-proof gates and retry budgets are unchanged.
- Tests cover typed HTTP classification, HTTP-SSE/WS parameter failures and unchanged
  account health, original response bodies, atomic terminal delivery, complete message
  IDs versus references/opaque history, and lossless versus unrecognized reasoning parts.
