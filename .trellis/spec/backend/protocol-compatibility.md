# Protocol Compatibility Contracts

## Scope

Apply when modifying Chat conversion, Responses HTTP delivery, explicit compaction
or multipart image editing. These contracts extend, not replace,
`proxy-and-delivery-contracts.md`.

## Ownership

- `gateway-protocol/openai/chat` owns pure JSON conversions, never networking,
  account selection, persistence or billing.
- `gateway-api/openai/chat` decodes and calls the existing model-aware Generate
  service. HTTP delivery uses the same pending execution and cancellation driver.
- `Operation::Compact` is explicit and model-aware. It must not use model-less
  standalone endpoint admission or silently become a Generate operation.
- Images multipart normalization feeds the existing raw JSON image path.

## Delivery

- Chat defaults to buffered delivery. Downstream stream and upstream WebSocket
  selection are independent. Conversion must not add internal HTTP round trips.
- Only wire JSON is converted; canonical metering remains the engine's source
  of truth. Never emit duplicate usage or cost facts from an API encoder.
- Preserve function IDs, argument strings, parallel indices and result order.
  Chat streams use CPA-style deltas and tool done fallback, not terminal/text
  snapshot reconstruction or full-response prefix comparisons.
- Preserve absent usage as absent/null, not invented zero counters.
- Image results use CPA-compatible message.images/delta.images, not Markdown
  in content. Skip empty image payloads; do not decode or validate upstream
  base64 in the Chat output converter. Shared transport/upload limits remain.
- Missing terminal, explicit upstream failure, failed commit and interrupted
  streams must not produce a successful completion. Keep cancellation and
  finalization active. A real empty terminal is not an EOF and is not rejected
  by an extra Chat-only empty-output validator.
- Before commit, Chat errors must be parseable JSON. Preserve upstream error
  status and safe response headers without forwarding stale content-length.
- Native Responses keeps its transparent event and error behavior.

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

## Chat Extension Contracts

### Scope

Apply to explicit Chat built-in/custom tools, image/search output and pre-delivery
control frames. Do not change native Responses, Images endpoints, account
selection, transport retry budgets, identity or canonical billing.

### Signatures

- `decode_chat_request` accepts explicit `image_generation`, `web_search`,
  nested `{"type":"custom","custom":{...}}` and CPA flat custom declarations.
- Custom output uses CPA's type:function/function.arguments envelope. Incoming
  native custom calls and function envelopes matching a validated custom
  declaration map to custom_tool_call; results map by call ID to
  custom_tool_call_output. Never JSON-parse or wrap the free-form string.
- `ExecutionSession::defer_downstream_commit() -> Result<(), EngineError>` is
  opt-in. Implementations not supporting it reject by default.
- `chat_response_from_events` accepts an iterator of borrowed
  `(Option<&str>, &Value)` wire header/body pairs. Buffered Chat scans these
  using the same event/failure classification as streaming; it must not rely
  on native Responses' different header precedence or produce transient chunks.

### Contracts

- A no-output Chat preamble must not commit a fake successful HTTP response.
  Consume the pending nonterminal event and wait within the existing deadline.
  Deferral cannot discard pending atomic failures, canonical Completed, a
  collected complete response, or anything after commit/finalization.
- `response.created` and transport metadata do not emit an empty assistant role.
  Skip unmapped events and annotations. Prefer JSON event type over the SSE
  header, with header fallback; explicit failures in either remain failures.
  Chat completion state must come from its own encoder, not a second Responses
  encoder interpreting the header differently.
- Do not auto-inject image tools. Validate size syntax (`auto` or positive
  `WIDTHxHEIGHT`), leaving model-specific dimension limits to the upstream.
- Accept New API's empty `function:{"name":""}` placeholder on a nonfunction
  tool, never a meaningful conflicting function definition.
- Preserve upstream tool types, names and IDs despite the downstream function
  envelope. Request-local declarations determine restoration, not heuristics
  about argument contents. Reject ambiguous names and namespace/MCP/computer
  approval semantics the converter cannot represent.
- Keep images separate from content. Stream image previews/item-done results,
  with per-frame image index zero as in CPA. Compare only the previous raw
  payload hash for the same nonempty item ID; no ID means no deduplication.
  A -> B -> A sends all three. Do not retain full images or all historical hashes.
  Previews never become completion and do not bypass terminal errors.
- Ignore web_search execution/source metadata and independent annotations.
  Never generate Sources appendices, citation fields, offsets or cached spans.
  Model text stays intact; native Responses annotations remain transparent.
- Chat output has no separate 64 MiB or image-version-count limit. MIME values
  containing `/` pass through; known short formats map to MIME, others default
  to PNG. Do not change independent Images upload validation.
- Streamed text/reasoning/tool argument deltas are sent immediately. Do not
  retain full text or full arguments to repair done/terminal snapshots.
  Tool argument done fills only if no arguments were emitted; output item done
  also closes the tool. Later deltas for a closed item are skipped.
- Resolve tools by item ID, then output index, then call ID. Only genuinely keyless events
  may use the current-tool fallback. An explicitly unknown key must not close
  or append to a different call. This avoids CPA's verified lost done-only-call
  case without bringing back strict field/snapshot rejection.
- Non-streaming conversion keeps recognized text/refusal parts, raw tool input
  and image entries while ignoring unknown output/parts. Optional malformed
  wire usage fields are omitted, not grounds to reject an otherwise valid
  reply. Canonical metering, costs, budget settlement and DB deduplication
  remain unchanged.
- Chat streaming must not clone the final Responses object just to observe
  completion. Before enqueueing successful finish/usage, require the existing
  execution finalization to succeed, including terminal-first replies.
- Do not reconstruct output images into subsequent model input automatically.
  Assistant images is an output-only extension; explicit user/tool image_url
  input keeps its existing semantics. Never feed generated base64 into text.

### Validation Matrix

| Input/state | Required result |
| --- | --- |
| Heartbeat/known metadata before text | No commit until a deliverable Chat chunk |
| Only controls/unknown events then EOF | Error, not empty success |
| Unknown event then valid text and terminal | Skip unknown, preserve text and completion |
| Deferral after terminal or commit | InvalidDeliveryState |
| Explicit valid image result | Independent images entry, content and usage unchanged |
| Image preview then interruption | Optional preview, then error; no successful finish |
| Mixed custom/function deltas | Stable function-envelope indices, raw strings and restored upstream types |
| Argument done after deltas | Skip full snapshot, never replay arguments |
| Delta after output-item done | Skip without failing the request |
| Distinct explicit key with done but no added | Announce a new tool, do not close another call |
| Search source metadata and annotations | Ignored without editing model text |
| Same item image A -> B -> A -> A | Send A, B, A; skip final duplicate |
| Nonempty unconventional image payload/format | CPA-style image URL, no Chat decoder/size gate |
| Completed first, execution finalization fails | Error only; no successful finish or usage |
| JSON format plus search/images | Original JSON content remains valid JSON |
| No image/search tool requested | No automatic tool declaration |

### Good, Base, Bad

- Good: custom input becomes function.arguments and restores using its declaration,
  preserving `print(1)` exactly and retaining the result ID.
- Base: ordinary text/function Chat keeps its existing delivery and scheduling.
- Bad: treat a heartbeat as success, or claim all Responses tools fit Chat.

### Required Tests

Exercise Core deferral guards, cancellation, pre-commit rotation and single
usage/finalization; Chat JSON/SSE image/custom/search cases; actual provider
HTTP/WS encoders and canonical decoders; native Responses/Images regressions.
Use real local New API DTO fixture generation and preserve source hashes.

At the verified local New API revision, typed request declarations drop
additional built-in image options and flat custom declaration fields. Nested
custom declarations survive. Typed streaming response formatting retains the
function envelope but drops images. Transparent image relay requires both
force-format and thinking-to-content paths to remain disabled.
This is a compatibility condition, not a production setting changed by CPR.
No fixture assertion proves real account permission or end-to-end live tooling.

The output convergence reference is CPA
`ac02da6c05e18f465aa7e3ed5b0a65a2f060917d`,
`internal/translator/codex/openai/chat-completions/codex_openai_response.go`.
Keep CPR's existing refusal support, complete non-streaming text concatenation,
Chat ID/usage conventions and public failure lifecycle; convergence does not
authorize replacing networking, scheduling or accounting with CPA's executors.

### Wrong vs Correct

Wrong: discard an empty encoded first frame then immediately call `next_event`,
or commit an empty 200 to unlock the session.

Correct: explicitly defer an eligible pending event without resetting attempts,
metering or deadlines; commit exactly once when actual Chat output is ready.

Wrong: queue a terminal-first success and usage before checking the execution's
pending cleanup result, or call the same event terminal using two type policies.

Correct: use Chat's terminal classification and hold its finish/usage frames
until the existing finalization check succeeds.

## Buffered Output Recovery

### Scope and Signatures

`openai::output_recovery::recover_response_output(&mut Value, borrowed_events)`
owns pure JSON recovery for native Responses and Chat complete-JSON delivery.
The caller still decides terminal/failure status. Native SSE/WS and Chat stream
encoders are not recovery consumers.

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
Responses/Chat JSON delivery, and unchanged native/Chat streaming.

Wrong: enable recovery in a stream encoder or use canonical metering to invent
client output. Correct: recover only the complete response from borrowed wire
events, then use the existing encoder and single execution finalization.

## Codex Generate Compatibility

- Ordinary OAuth Generate is sent with `store=false`, regardless of a
  downstream `store` value. The encoded request, transport requirement and
  captured session state must observe the same effective value; do not only
  rewrite final serialized bytes.
- Remove only fields known to be unsupported by the Codex Responses endpoint:
  `max_output_tokens`, `max_completion_tokens`, `temperature`, `top_p`,
  `frequency_penalty` and `presence_penalty`. Do not apply this filter to
  Compact, Search, Images or other providers.
- Convert string input to the existing Responses message shape on the
  outbound copy. Preserve the original request used for affinity/content
  fallback.
- Standalone complete message and tool-call items may drop an invalid optional
  item `id` when no continuation or unknown reference semantics are present.
  Never rewrite or delete `call_id` links, item references, encrypted history
  or unknown item shapes. Native continuation keeps complete input unchanged.

### Validation

- Test effective `store=false` through HTTP SSE and WebSocket, including
  session capture and downstream WebSocket transport requirements.
- Test unsupported-field removal, string/list input equivalence, tool-call/result
  linkage and encrypted/compaction history preservation.
- Test account switching after request normalization; no normalized body may
  be reused as the source for a later account.
