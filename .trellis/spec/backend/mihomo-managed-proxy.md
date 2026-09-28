# Managed Account Session Proxies

## 1. Scope / Trigger

Account-opted-in Codex/Excel session exits, Mihomo management, and a separate ordinary
proxy pool. Reference Sub2API commit `030c1fd776f1e4523728665c0a6127a0f4dbac33`.
This does not restore State collection or change native Codex transport decisions.

## 2. Signatures

- `account::RequestProxySource::{Account, Mihomo, ProxyPool}` owns the stored value.
  Provider ports may re-export it; account values must not depend on those ports.
- Migration `0051_request_proxy_source.sql`: `provider_accounts.request_proxy_source`
  defaults to `account`, constrained to `account | mihomo | proxy_pool`.
- Admin `requestProxySource` is optional for patches, templates and new imports.
- `GET/POST /api/admin/proxies/mihomo`; `POST .../mihomo/check` with
  `{node, quality}`. All require `AdminAuth`; mutations retain audit context.
- `SessionProxyPool::acquire(source, route, scope, transient)` returns a shared
  `SessionProxyLease`. Its last owner releases the logical reference.

## 3. Contracts

An already-resolved Codex or Excel route can apply a managed source, independently
of the Excel switch. The default `account` source preserves the account proxy/IPv6 path. Omitted
patches preserve stored values; changing source never mutates identity,
credentials, groups, weights, billing, affinity or cache keys.

Acquire uses only in-memory qualified snapshots, not network probes or database
queries. Background capacity reads occur every five seconds. Keep source and protocol pools
independent and scope bindings by account, client key and trusted session.
Anonymous requests use transient bindings. Never rebind an in-flight session.

The same attempt lease covers uploads, main generation, repair and compaction.
Native WS openings and active background forwards retain the lease through cancellation
cleanup; idle pooled sockets must not retain an active reference. Preserve exact
continuation ownership and actual-egress pool keys. No new native replay is added.
Report completion before yielding the final SSE event; consumers may never poll
again. Business 400/401/403/429 responses do not mark a node unhealthy. At most
one Excel switch to an already-qualified exit is allowed after definite connect failure,
not a timeout, not after uploads, and not with concurrent session references.

Keep the Host implementation private; export management/pool traits via
`HostBundle`. Validate candidates before activation, persist privately and restore
the previous configuration on failure. Retired listener ports reject traffic;
they cannot silently point to another node. Parent stdin-pipe EOF terminates the
owned kernel without unsafe pre-exec hooks. During validation retain the taken
stdin handle because Tokio `Child::wait` closes stdin still owned by the child.

## 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| No qualified exit / manager unavailable | Explicit `account_proxy_*`, never direct fallback |
| Cold node | Background qualification, not request-path probing |
| Unauthenticated management | Reject before reads or mutations |
| Invalid source enum | Reject before persisting |
| Invalid candidate configuration | Preserve previous active configuration |
| Stale probe or retired generation feedback | Cannot revive a current failed node |
| Cancelled response | Release lease, do not fabricate network failure |
| Source is `account` | Existing transport and scheduling unchanged |
| Excel is off and source is managed | Native HTTP/SSE/WS still uses the selected pool |
| Excel node fails | Codex qualification, cooling and sessions remain independent |

## 5. Good / Base / Bad Cases

Good: concurrent requests in one session share its qualified exit and finish before
rebind. Base: existing accounts migrate to `account` and keep their original
settings. Bad: a failed managed exit falls back to default IPv4 or replays an
upload on a second node.

## 6. Tests Required

Host tests cover node identity, redaction, two pools, stale generations, cooling,
references and retry fences. The isolated official-kernel test must prove relay,
candidate rejection, disabled-node rejection, restore and parent-drop cleanup.
Store tests must execute migration and source roundtrip in temporary PostgreSQL.
App integration also requires temporary loopback Redis, not just PostgreSQL.
Provider tests cover terminal/drop lifetime and no business-error penalization,
alongside full Excel and existing IPv6 regressions. Run architecture checks without
loosening their allowlists. Frontend checks cover edit/batch/template roundtrips,
management writes, visible labels and desktop/mobile layout.

## 7. Wrong vs Correct

Management display audit reference: Sub2API production `ca5dd3cf5299df8778cdfa752430253ad4e071e8`
(v2.9.1). OpenAI/Grok quality probes intentionally omit credentials and accept HTTP
401; this is reachability, not account/model authorization. Keep scores compatible,
but surface failed/challenge/warn checks before the letter grade in the UI.

Country admission continues to use the independent country.is observation. Node
names and IP-API diagnostic geography must not overwrite it. An unobserved or failed
lookup follows `allowUnknown`; a dynamic exit follows supplier-management/unknown
policy. UI explanations must distinguish these from explicitly excluded countries,
and default `ready` is enabled status, not proof of successful qualification.
Keep unsaved country drafts across asynchronous mutation acknowledgements and stale
polls; clear dirty state only when the persisted policy matches, including all flags.

Node management uses 50-row pages and exact subscription-ID filtering (including
multi-source nodes). Batch connection diagnostics use 3 workers; quality diagnostics
use 2, sharing the existing read-only check endpoint rather than the state-changing
`probe` command. Freeze batch targets on start: selected nodes intersect current
filters, otherwise all filtered nodes, not just the current page. Filter changes
clear selection and reset the page; refreshes clamp it. A failed node must not stop
the batch. Summarize failures without per-node toasts, and stop dispatching queued
work when the panel is disposed. Do not mutate routing or region admission here.

Wrong: put `RequestProxySource` in execution ports and import it into account values.
Correct: store the value in the account layer and re-export from the port if needed.

Wrong: yield completion, then record success when the consumer resumes polling.
Correct: record success first and release the lease on response completion/drop.
