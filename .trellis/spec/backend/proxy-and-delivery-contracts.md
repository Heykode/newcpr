# Proxy and Responses Delivery Contracts

## 1. Scope / Trigger

Apply when changing proxy import, account mutation, transport selection or
buffered Responses delivery. These contracts describe the CPR implementation,
not Sub2apiQX. The opt-in IPv6 and device-continuity extension approved on
2026-09-11 is specified in `client-continuity.md`; it does not import QX TLS
profiles or random fingerprint generation.

## 2. Signatures

- `POST /v1/responses`: only boolean `stream=true` selects downstream SSE;
  omitted, false or non-boolean values use buffered delivery. Preserve the
  original field for the existing Provider normalization and validation path.
- `decode_request_object`: consumes boolean `use_websocket` into protocol context.
- `websocket_upstream_request(&CodexResponsesRequest)`: normalizes a clone.
- `ExecutionSession::fail_delivery(GatewayError)`: finalizes a local delivery
  failure without reporting it as a client cancellation.
- Account mutations select `AccountProxySelection::{Direct, Url, Saved}`.
- `outbound_proxies` owns proxy credentials; `provider_accounts` retains both
  `outbound_proxy_id` and the resolved `outbound_proxy_url`.
- Migration 0005 adds managed proxies; 0001-0004 remain byte-identical.
- `AccountDirectoryItem.effective_concurrency_limit: NonZeroU32` projects to
  read-only `AccountView.effectiveConcurrencyLimit: u32`.

## 3. Contracts

- A stale proxy-test result must finish transaction rollback before returning
  its conflict, releasing the import-exclusion advisory lock. Transaction drop
  alone queues rollback and can spuriously reject the next valid operation on
  another pooled connection. Cover stale-then-current writes without retry sleeps.
- `stream=false` must not force HTTP. Preserve transport override, fallback and
  continuation ownership. Both HTTP and WebSocket send streaming upstream copies;
  only the downstream HTTP response is buffered into complete JSON.
- The HTTP omitted-stream default is buffered and must not insert `stream=false`
  into the opaque request body. A downstream WebSocket `response.create` frame
  keeps its existing omitted-stream default of streaming and rejects explicit
  false or non-boolean values.
- Never mutate the original request's `stream` or copy downstream credentials
  into the selected account's identity. Reuse existing transport projection.
- Generate normalizes string `input` and the exact `fast` tier alias only in
  outbound copies. HTTP adds `store=false` only when absent. Preserve explicit
  values, `context_management`, local continuation intent and WS store policy;
  do not broaden the existing parameter removals. Body and routing tier hint
  must agree without changing the original request's affinity inputs.
- Account `outboundProxyId`: omitted preserves, empty string selects direct,
  nonempty selects a saved and successfully tested proxy. Do not send a redacted
  display endpoint back as a connection URL when toggling account status.
- Batch `concurrencyLimit`: absent preserves, `null` restores default, number
  sets the override. Use `deserialize_optional_nullable`; plain nested `Option`
  does not distinguish JSON null from an omitted field.
- Account directory/list/detail/refresh views expose the configured effective
  capacity separately from the nullable override. Resolve account override
  first, else the `SettingsStore.load_runtime_settings()` default; read the
  default once per page. Never write this projection into account settings.
  Valid limits are 1..u32::MAX; disabled/cooling accounts retain configured
  capacity rather than being presented as zero. Global changes appear on the
  next directory query; this is not a new scheduler or capacity refresh loop.
- Proxy connection changes update bound URLs and runtime config revision, but
  never rewrite provider credentials or increment `credential_revision`.
- File/token imports reserve a selected saved proxy before upstream preparation
  and revalidate before persistence. Cancellation must release the reservation.
- Proxy probing receives the same custom-CA builder as OpenAI transport.
  A proxy failure is not permission to connect directly.
- Unsaved proxy probes use the same four test slots and transport as saved
  proxy tests. They require admin authentication but never call the store,
  publish a revision, or update account bindings. Cancellation releases a slot.
- Editing a proxy with an empty URL keeps its saved credentials. The form can
  test that saved proxy by ID; a new URL is probed before an explicit save.
- When local account scheduling ends with no eligible account or no capacity,
  that terminal scheduling error takes precedence over a stale retryable
  account-authentication failure for client delivery. The earlier failure
  remains in intermediate request diagnostics; genuine final upstream 429/502
  responses keep their existing delivery contract.
- A terminal send-state update and final request write belong to the same
  session-owned future. Cancelling an event wait must not drop or restart the
  send-state update, replace the terminal reason, or lose pending cleanup.
- Local delivery failure stops remaining work and reuses the existing
  finalization/settlement/release path. Before commit it is Failed; after commit
  it is Incomplete. Preserve a pending provider error or already-started terminal
  write, including true cancellation. Do not create provider health penalties.
- A remote WS close 1009 is `InvalidRequest` with status 413 and code
  `message_too_big` before HTTP delivery. After streaming has committed, retain
  the already-sent HTTP status and deliver the error event instead. Preserve
  close details and send state, but do not spend
  recovery budget, replay the request, or classify it as an invalid account.
  The close handler adds no replay or post-send HTTP fallback policy.
- Before any WS opening, optional `NewChain` requests at or above
  `websocket_large_request_threshold_bytes` use the existing HTTP transport.
  Default is 15 MiB; zero or disabled `websocket_http_fallback_enabled` disables
  size selection. Measure the final projected WS JSON UTF-8 bytes before
  compression. Native connection-local first turns, warmup and every previous
  response scope retain their existing transport contract. Keep the frozen
  attempt client, selected account, identity, proxy and original input.
  Do not persist size selection as a session failure or permanent HTTP mode.

## 4. Validation & Error Matrix

| Input / state | Result |
| --- | --- |
| HTTP non-streaming, transport override absent | Existing transport policy, complete JSON |
| HTTP non-streaming, `use_websocket=false` | HTTP/SSE upstream, complete JSON |
| HTTP non-streaming, `use_websocket=true` | Existing WebSocket policy, complete JSON |
| Downstream WS frame with `stream=false` | Existing protocol rejection |
| Both proxy ID and URL supplied | Validation error before mutation |
| Batch with only a proxy ID | Valid update, other fields preserved |
| Proxy changed / in use by import | Conflict, no partial credential mutation |
| Local encoding fails before / after commit | Failed / Incomplete; cleanup and settlement once |
| Commit fails or finalization was already started | Preserve the existing terminal reason; no false success |
| Actual client disconnect/cancellation | Existing cancellation and detached cleanup contract |
| WS closes with 1009 before / after HTTP commit | HTTP 413 / streamed error; no replay or account-health penalty |
| Independent new chain at the configured size threshold | HTTP before WS opening; `http_large_request` decision |
| Zero threshold, fallback disabled, warmup or continuation | Existing transport policy; no size-driven escape |
| Retryable upstream 401 then local no-account/capacity failure | Local 503 contract; no stale 401 body or events |
| Retryable upstream 502 then local capacity failure | Preserve original upstream detail and raw response |
| Inherited account limit + changed global default | Query returns new effective value; override remains null |
| Explicit account limit + changed global default | Explicit override still takes precedence |
| `effectiveConcurrencyLimit` in an account mutation | Reject unknown read-only field; no account update |

## 5. Good / Base / Bad Cases

- Good: reauthorize through a selected proxy while preserving the new token after
  an unrelated proxy rename.
- Base: import a direct account without a proxy selection; keep prior behavior.
- Bad: status-toggle writes the masked endpoint and loses proxy authentication.
- Bad: buffered output forces HTTP for a connection-bound continuation.

## 6. Tests Required

- Decode matrix: stream true/false x transport absent/true/false; local override
  stays out of wire body, previous-response ID survives.
- Actual loopback HTTP and WS: upstream stream true, original false, account,
  authorization, UA and session projection unchanged.
- Buffered JSON retains full text/tool output and token usage; cancellation and
  commit-failure regressions remain green.
- Cover local delivery failure during finalization, settlement and admission
  release; dropping the await must resume the original writes exactly once.
  Retain upstream status/request IDs, usage, cost and pending provider failures.
- Exercise 1009 before and after delivery on new/reused sockets, another close
  code as control, and Responses/Chat error status/body projection.
- Account-exhaustion regressions must cover retained atomic error events as
  well as raw bodies. Keep intermediate authentication diagnostics while
  finalizing once with the local error, without committing a stale response.
- PostgreSQL: migrate old URLs without dropping passwords; bound proxy edits
  preserve credentials/groups/scheduling, import locks release on cancellation,
  proxy-only batch preserves unselected fields.
- Set isolated `CPR_TEST_DATABASE_URL` and `CPR_TEST_REDIS_URL` for integration
  tests. A local environment-variable skip is not a successful integration test.
- Include passwords in both test URLs. Store bootstrap requires each password
  to be exactly 48 hexadecimal characters; trust-only or arbitrary-password URLs
  can pass low-level DB tests but fail the worker-lease StoreBundle test. Generate
  24 random bytes as hex, configure the isolated services consistently, and run
  that bootstrap test as an environment preflight before the full suite.
- Match transport fixture capabilities to request context. WS-only event/pool
  fixtures must use an existing required-WS context, preserve the wire body,
  and prove a slow opening cannot silently escape to unsupported mock HTTP.
  Keep fallback-policy tests on the real optional-WS path.
- Single-response HTTP fixtures must advertise `Connection: close` or actually
  read subsequent requests on the same connection. Holding the old socket while
  waiting for a new one can deadlock against the client's keep-alive pool.
- Test deadline/expiry semantics with controlled time after real socket setup;
  do not use total loopback wall time as a proxy for a transport budget.
  Preserve bounded waits and complete logs for serial and concurrent reruns.
- Finish compilation/static checks before a high-concurrency real-I/O timing
  run. Report contention failures separately rather than silently retrying them
  into an all-green claim; real socket tests are not virtual-time-only tests.
- Admin/API capacity regressions cover mixed inherited and explicit accounts,
  list/detail/quota-refresh projection, live global changes, disabled accounts,
  zero rejection, u32::MAX and refusal to write read-only projection fields.

## 7. Wrong vs Correct

Wrong: fix upstream's streaming requirement by forcing every buffered request
onto HTTP. This conflates downstream delivery with upstream connection policy.

Correct: normalize the provider's upstream request copy and keep the API's
existing complete-JSON collector. Protect this distinction with both transport
tests and downstream collector tests, not one decoder assertion alone.
