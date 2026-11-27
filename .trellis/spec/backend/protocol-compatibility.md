# Protocol Compatibility Contracts

## Native Compatibility Ownership

Pure Codex compatibility now lives under `providers/openai/src/compat/`.
Move the exact private-test owner entries in `apps/gateway/tests/main.rs` with
the source; never leave a deleted owner or widen the inline-test allowlist.
This module cannot acquire account, egress, session or socket ownership.

## Native Error Boundaries

- Follow upstream 958e13c3: administrator-owned errors use actionable Chinese;
  locally generated model API errors use English. Preserve genuine upstream
  status, body, code, type, param and message rather than replacing them with
  a generic local message. Unknown internal failures stay safe.
- Minimum-client-version rejection is 400 with its existing machine codes and
  version fields. Axum missing-upgrade rejection is still 426; these are distinct
  boundaries and tests must not replace all 426 expectations indiscriminately.
- WS close 1009 keeps a nonempty upstream reason, falling back only when absent.
  Retain 413, no replay and no account health penalty. SSE delimiter acceleration
  cannot change LF/CRLF selection, partial-frame retention or terminal semantics.
- Local Live transport errors and system update errors may contain private URLs.
  Public messages must be safe, while internal diagnostics retain controlled
  source chains/redaction facts. Diagnostic storage failure is best effort and
  cannot replace the original result. Do not change deployment algorithms as a
  side effect of aligning diagnostics.
- Port field-specific validation only for fields supported by the local API.
  Unsupported official extensions remain rejected by the existing JSON schema;
  do not invent an accepted field or leave an unreachable error branch for it.

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

The v3.57.0 integration uses the pinned `gateway-protocol/src/chat_completions/`
parsers and converters. API ingress keeps the Chat protocol and raw request; the
selected Provider converts directly to its native target and returns Chat wire.
Do not restore the retired API-owned Responses converter or use its old tests as
the current request contract. Keep native Responses normalization independent.

Chat ingress must retain validated supplemental scheduling hints through
`OpenAiRequestHeaders::session_context`, without forwarding those local headers.
Before target conversion, Codex selection derives explicit account sessions and
supplemental hints in the existing tenant-scoped namespaces and applies the current
strict/preferred/relaxed policy. This is account affinity, not native response or
WebSocket ownership. Request observations and actual admission must use the same
key; checking only the ingress context cannot prove that the selector consumes it.

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

- `gateway-protocol/chat_completions` owns pure Chat request/response conversion.
- API retains Chat wire for routing and delegates target conversion to the selected Provider.
  Codex/xAI target Responses; AGY targets GenAI. No second account selection is allowed.
- `gateway-api/openai/chat/delivery.rs` owns the official Chat accumulator and delivery validator;
  `openai/encoding.rs` invokes it on actual encoded frames. Native Responses retains its separate path.
- Local ExecutionSession owns admission, deadline, cancellation, usage, cost and finalization.
  Raw Provider events carry canonical observations before translation; one-to-many translation keeps
  the terminal envelope on the last wire event only.

## Delivery

- Chat defaults to buffered output; downstream stream and upstream transport are independent.
  Provider receives the original Chat fields, including local transport context. Do not restore the
  retired API-owned Chat-to-Responses conversion or old converter rejection lists.
- Request-specific ChatIdentity stays fixed across the response. Preserve official validation of
  tools, identities, snapshots, incomplete reasons, usage and malformed wire.
- Streaming requires an initial valid chunk, one finish per choice, usage only after finish and one
  DONE. Business output after finish fails. Validate a whole terminal batch before committing it.
- Before HTTP commitment preserve upstream status, body, allowed headers and request identifiers;
  projection/invalid wire failures use the official error boundary. After commitment send Chat error
  SSE plus one DONE, without successful finish/usage. Never substitute Responses error frames.
- The buffered path consumes Chat wire in ChatAccumulator. Provider converters own source-protocol
  collection. Native Responses keeps its existing snapshot and recovery behavior.
- Successful terminal delivery waits for execution finalization. Missing terminal, body drop,
  commit failure, cancellation or malformed trailing SSE cannot become success.
- Wait for the same pending execution; after admission Chat retains the 15-second initial keepalive.
  Heartbeats record HTTP status, not business commitment or successful execution.
- Real source/target wire tests belong to Provider/protocol suites. API tests provide Chat wire and
  verify HTTP status, full body, headers, finalization, cancellation and sibling Responses behavior.
  Follow docs/upstream-integration-checklist.md; do not claim parity from codec equality alone.

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
  Official `75be1287e` supersedes #606: final Codex HTTP and WS requests omit
  max_tokens, max_output_tokens and max_completion_tokens. The shared Codex encoder
  also applies this filter on its copy; preserve the original protocol payload and
  assert absence in both encoded and final-wire requests.
- Compile before timing-sensitive socket suites. Missing database fixtures are
  not successful integration tests. Real upstream capabilities require accounts
  and separate approval; mock completion must never be reported as production
  acceptance.

## Official Chat Parity

The current fixed upstream is `db22550e6172e7a3537eb53441ae9ce73bc50cd5` (v3.57.0 plus
release-channel metadata). `docs/chat-official-parity.md` records the current runtime bindings.
The former v3.35.1 API converter and its rejection list are historical, not current requirements.

Keep the copied shared parser and per-target converters, target-specific parameter errors,
function/custom tools and stream/JSON usage contracts. The local OAuth registry has no native
Messages execution target; retaining a pure protocol converter does not enable an upstream API Key
account. Native Responses, HTTP/WS, Excel/State and scheduling require their own regressions.

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
- Official `75be1287e` removes `max_tokens`, `max_output_tokens`,
  `max_completion_tokens`, `temperature` and `prompt_cache_retention` from final
  Codex HTTP/WS generation requests. Do not alias an output limit back into the
  payload. Preserve `top_p`, `frequency_penalty` and `presence_penalty`.
  Response incomplete-reason interpretation is unchanged. Do not apply this filter to
  Compact, Search, Images or other providers.
- Convert string input during encoding on a copy, before account selection.
  The existing string/list content-anchor equivalence must keep routing,
  session and cache identity unchanged. Preserve the original protocol payload.
- Preserve call_id links, references, encrypted history, compaction and unknown shapes.
  On the selected OAuth send copy, omit an `item_` message ID only when it is
  unreferenced and has complete inline plain-text content. Preserve references,
  encrypted/tool/unknown content and other IDs. Do not rewrite `item_` to `msg_`,
  and do not add store/conversation gates absent from the pinned official helper.
  Account selection and identity derivation still precede the outbound mutation.
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

## Anthropic Messages ingress

- Preserve native Messages validation, authentication precedence, error envelopes and CORS.
  Responses/Messages/GenAI do not convert into one another. Chat alone supports target conversion.
- The included Codex, xAI and AGY OAuth accounts do not execute Messages or Messages count_tokens.
  Reject unsupported operations before upstream I/O; do not restore the retired OAuth local-BPE path.
- Anthropic `/v1/models` and detail call the protocol-scoped catalog with `anthropic`, sort before
  cursor pagination, and return only executable models. An empty OAuth-only menu is valid. Generic
  OpenAI discovery remains independent. Errors use the Messages envelope.
- Native Responses preserves explicit service_tier; translated Codex Chat omits auto/default.
  Observations still report the original protocol rather than relabeling converted Chat as Responses.

### Protocol observations and session ownership (v3.45.2)

- `gateway-protocol::request` owns body/context facts, including image generation,
  compact, request/subagent kind and reasoning effort. Replacing a body refreshes
  its facts; context cannot manufacture image generation. Use wire protocol
  constants independently of Provider identity names.
- Messages logs explicit `output_config.effort` (trimmed, including unknown strings);
  a thinking token budget does not invent an effort. Native Responses retain
  numeric effort observations. `generate=false` determines prewarm; an empty
  authoritative turn-metadata value must not fall back to a lower-priority alias.
- Core chooses the explicitly targeted Provider or the candidate that owns actual
  restored Provider session state, falling back to the first candidate only when
  no owner exists. Candidate order must not erase or fabricate session settings.
- Providers supply requested endpoint model, continuation and an optional previous
  reasoning override from their own real session. They do not reinterpret the
  common wire facts; xAI cannot inherit an OpenAI session override.

## Guardian 身份与传输（v3.54.0）

- 在入口合并身份之前保存结构化 Guardian 角色；HTTP 正文/头各自必须完整且不冲突，
  重复或错误类型标记拒绝推断；WS 仅当前帧正文参与识别，不沿用握手身份。
- reviewer 的专用模型 hook 在 Core 规划中生效，尊重显式映射、账号目录、分组与账号
  模型限制及续接 owner。无合格原生账号则按普通模型规划；不得转入 Excel。
  已确认的 reviewer_model 决定续接，不能因开关改变而把旧 handle 交给另一模型。
- 显式 Guardian 角色头优先；自动 reviewer 标记仅在实际专用模型上生成。
  classifier/housekeeper 保留其 sampler 传输；普通与 reviewer HTTP 正文使用 zstd，
  不再按小正文大小跳过，classifier/housekeeper 不压缩。WS 消息压缩仍按原独立规则。
  HTTP zstd 压缩在 blocking pool 执行，提交前按可用 CPU 数限制并发；取消等待或尚未启动任务可释放资源，已运行任务持有许可直到真实结束，不能声称取消会强杀工作线程。Excel 的独立压缩合同保留。
- `x-codex-turn-id` 作为普通扩展头透传，不再提取为 typed context.turn_id，也不在 HTTP、WS 或 Live 额外屏蔽；正文、client_metadata 和标准 x-codex-turn-metadata 中的轮次身份继续按既有合同解析。
- 所选 OAuth ID token（缺失时 access token）的 FedRAMP claim 决定受管理标记；
  下游不能覆盖该账号的 true。无 claim 的同账号扩展按既有透传合同处理。
  模型目录、预热、Images/Search 和 Live 辅助调用复用账号客户端及池隔离。
- WS opening 不携带 turn-state；在连接就绪后编码本次 payload、填开始时间，
  编码失败丢弃已检出的连接；请求证据来自最终发送正文。
- HTTP/SSE 回退产生 ReplayRequired 的续接在确认归属后以 NotSent 拒绝，
  返回 previous_response_not_found 供完整历史重放；Excel 保留既有合同。
  成功重放只关联真正要求恢复的同 Key/会话 failed 或 incomplete 记录，
  不改写原错误终态与计费事实。

## Antigravity GenAI 与 Chat

- AGY accepts native GenAI and Chat; Responses/Messages compatibility is retired by approved scope.
- Platform system text is prepended to client system parts without losing metadata. Generation and
  countTokens share this rule. Do not replace client global/project/tool instructions wholesale.
- Native GenAI retains JSON bytes and SSE frames; no gateway heartbeat before or between GenAI frames.
  Chat retains its own finalization, keepalive and conversion-error boundary.
- Search follows the selected target's native tools and catalog permissions. Do not document the
  removed Responses web_search adapter as an available endpoint.
