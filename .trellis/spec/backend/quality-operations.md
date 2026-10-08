# Scheduled Quality Checks

## Read-Only Model Suggestions

### Scope / Trigger
Quality-test/judge suggestions only; no scheduling, model restrictions or public discovery.

### Signatures
AdminAuth/no-store POST `/api/admin/quality-ops/models` takes
`{accountIds?:string[],group?:string,statuses?:string[],page:number}` and returns
`{models:[{id,name,reasoningEfforts}],nextPage,matchedAccounts,knownAccounts,failedAccounts}`.
`ProviderAdmin::quality_model_choices(account_id, exact)` owns provider-specific discovery.

### Contracts
One explicit account reads its own native catalog/egress. Excel uses configured models,
never Codex fallback. Aggregate reads are cache-only, with 100 candidates per page and
no pool-wide HTTP fanout. Only exact evidence supplies reasoningEfforts; aggregate is null.
An absent in-memory snapshot is not proof of an empty backing cache: fall back to
the existing read_account_catalog cache-only method, never a refresh-on-miss API.
Suggestions never restrict manually entered rule models or mutate account state.

### Validation & Error Matrix
Auth -> 401; schema -> 422; invalid page/scope -> 400. Account IDs cannot combine with
group/status filters. Exact failure propagates; aggregate failures increment failedAccounts.

### Good / Base / Bad
Good: exact-account catalog. Base: rule execution unchanged. Bad: borrow another
account's capability evidence or query all upstream accounts for a dropdown.

### Tests Required
Fake-provider exact/cache-only pagination; native loopback selected credential and zero
aggregate requests; Excel isolation; API auth/no-store/scope; UI pagination/cancellation.
Cover an uninitialized in-memory catalog with both empty and populated backing caches;
neither aggregate read may send an upstream request. Run the full Provider admin suite.

### Wrong vs Correct
Wrong: pool sampler for exact account. Correct: account_catalog_documents for exact
scope, cached_account_models for aggregate scopes.

## Excel Paused Recovery

- `ExcelRecoveryConfig` is default-off, account-scoped, and separate from native quality rules.
  Preserve omission in imports, old templates and selective bulk patches. Validate 1–10080 minutes.
- Use the trusted fixed-account `AccountProbe::excel_recovery`, never a synthetic user key or
  direct HTTP probe. Require published snapshot/credential versions, actual Excel routing, a
  complete successful terminal and exact random nonce text. Never rotate accounts/models or
  fall back to Codex. Ordinary requests cannot set the marker via JSON.
- Recovery alone explicitly requests `reasoning.effort=low`; normal connection tests and
  native quality/Codex defaults must not change. Carry expected nonce as trusted internal
  operation metadata, not protocol JSON or prompt-string inference.
- In the OpenAI provider, validate raw HTTP 200 SSE to EOF before Excel transformation:
  2 MiB total, 1 MiB per line, exactly one completed terminal, final assistant/output_text
  only (ignore reasoning), and exact trimmed nonce. Reject duplicate terminals, `error`,
  `response.failed`, `response.incomplete`, `response.cancelled`, malformed data and
  transport errors before or after completion. Never
  buffer ordinary Excel streams or move wire protocol parsing into Core.
- Durable PostgreSQL slots bound global probes to three. Request deadline is 45 seconds;
  cancellation drains coordinator finalization before releasing the two-minute crash lease.
  Generation, lease, identity, configuration, egress and credential fences are rechecked at commit.
- Real runtime setting changes invalidate claims; config revision bumps from another account's
  recovery must not cancel parallel work. Lock runtime settings before account/recovery mutation.
- Restore only opted-in Excel manual/403 pauses with ready credentials; preserve independent
  quality-owned pauses and quota/auth protections. Keep historic 403 evidence, clear verified
  recovery on later pauses, and never present manual resume as a successful BPS test.
- Tests: Admin worker nonce/failure/cancel cases, Core trusted marker/completion cases, Store
  real migrations/admission/fences/parallel success, and frontend synthetic mobile/desktop flows.
  Provider recovery tests must reject every failure terminal before/after the nonce completion,
  including fragmented streams, without yielding any successful output. The HTTP gate tests
  must cover error/cancelled tails while retaining ordinary Excel streaming behavior.

## Dynamic Group Rules

### 1. Scope / Trigger
- Group enrollment is administrative background work, never part of user scheduling.
  It reuses the existing fixed-account executor and database-wide admission.

### 2. Signatures
- Migration `0055_quality_group_rules.sql`: `quality_group_rules`, `quality_group_members`.
- AdminAuth/no-store routes: `GET groups`, `POST groups/save`, `POST groups/delete`
  under `/api/admin/quality-ops`.
- Save accepts `{id?,revision?,name,filter:{group,statuses},config}` with no accountId.
  Response is `{group,sync:{created,updated,failed}}`; the parent has already committed
  even when child synchronization fails. Delete accepts `{id,revision,deleteRules}`.

### 3. Contracts
- Empty group means all, `ungrouped` means no group, otherwise use a real group ID.
  Reuse account-list status semantics and active runtime cooldowns. OAuth/OpenAI only.
- One-minute reconciliation pages account candidates and owned updates by 100, without
  model requests. New enabled rules become due immediately in the shared save
  transaction and run through the existing leased worker queue.
- Shared configuration lock + parent config/filter/revision check + child CAS prevent
  stale propagation. Existing independent/other-group rules are not adopted.
- Deleting a child leaves a NULL-rule membership tombstone; deleting the actual account
  cascades the membership. Leaving a group retains the established child rule.
- Pause cancels owned leases/pending runs atomically. An unchanged pause must still work
  after its remediation template or selected account group disappears. Resumption needs
  valid references. Disabled parents synchronize edits but never enroll new accounts.
- Parent deletion either detaches children or deletes them, per explicit administrator
  choice. It never reverses account remediation already performed.

### 4. Validation & Error Matrix
- Invalid filter/status, bound accountId, duplicate statuses, bad config -> reject save.
- Stale parent/rule -> no overwrite. Expected ownership/tombstone races are no-ops;
  a still-pending target with invalid references is a failure, not successful sync.
- Account-list errors are not authoritative emptiness and never cause deletions.

### 5. Good / Base / Bad Cases
- Good: new matching account gets its own rule within the next reconciliation cycle.
- Base: manual rules, templates and global ten-job / per-round 1-8 bounds are unchanged.
- Bad: group pause unpauses an account, or a deleted account is recreated by late finish.

### 6. Tests Required
- Store `groups.rs` and `group_safety.rs`: ownership races, tombstones, CAS, stale
  templates, pause/resume, detach/delete, account cascade, 100+ pagination, no account edits.
- Keep frozen migration checksum and exact-schema/reopen integration test in sync.
- Browser `quality-groups.mjs`: filters, pause/resume, detach, partial batch deletion,
  fresh versions, account menu, 1440/390/320px; no external requests.

### 7. Wrong vs Correct
- Wrong: overwrite every matching account or report all conflicts as success.
- Correct: only own explicitly linked rules, and retain failed pending propagation.

## Monitoring Rule Templates

### 1. Scope / Trigger
- Monitoring templates reuse detection configuration, not the separate account
  templates used by failure remediation. They never create a new request chain.

### 2. Signatures
- Migration `0053_quality_rule_templates.sql` adds `quality_rule_templates` and
  nullable `quality_rules.source_template`; keep frozen migrations unchanged.
- Admin routes under `/api/admin/quality-ops`: `GET templates`,
  `POST templates/save`, `POST templates/delete`, `POST templates/apply`,
  `POST monitoring`. All require AdminAuth and no-store responses.

### 3. Contracts
- Template `{id,revision,name,config}` uses QualityRuleConfig without accountId.
  Real rule saves still require accountId. Applications contain a template id/revision
  and targets `{accountId,ruleId,revision}`; new rules use null ruleId/revision.
- Each account application is an independent transaction returning a success/error
  result. Lock configuration first, recheck template revision and rule CAS, and use
  the database's config/name instead of caller-supplied snapshots.
- Keep one rule per account. Replacement preserves rule id/history and cancels old
  leases through the existing save transaction. Template edits/deletion do not alter
  applied rules; sourceTemplate is provenance, not a live link.
- Account list responses include nullable qualityMonitoring with ruleId/revision,
  enabled/running/pending, nextRunAt, lastStatus/lastRunAt/lastAction and sourceTemplate.
  Read once for the current page; no per-account browser polling or model-path query.
- Creating an enabled rule or changing disabled to enabled makes it due immediately.
  Ordinary enabled-rule edits use the next configured occurrence; disabled rules stay
  unscheduled. This is owned by `PgQualityOpsStore::save_checked`, covering direct,
  template and group paths. Do not issue upstream requests from the save handler.
  Provider compatibility, account settings, quality ownership and probes are unchanged.

### 4. Validation & Error Matrix
- Empty/bound/invalid template -> reject. Name: 1–128 non-control characters.
- 0, >1000, duplicate or invalid target IDs -> reject the entire application request.
- Stale template or rule revision -> no overwrite; account-level conflicts return
  a failed result while independent successful targets remain applied.
- Excel state probes use the trusted native override without changing the account route.
- Read failure is not an empty authoritative catalog or an absent rule.

### 5. Good/Base/Bad Cases
- Good: apply a six-hour template to several accounts and preserve their exits.
- Base: legacy rules with SQL NULL source_template remain readable and editable.
- Bad: deleting a catalog entry cascades through active rules, or editing a template
  silently changes active rules and historical question snapshots.

### 6. Tests Required
- `tests/postgres/quality_ops/rule_templates.rs`: CRUD/CAS, forged snapshots,
  replacement/history, cancellation, Excel guards, account list/detail projection,
  source deletion and unchanged account settings against isolated PostgreSQL.
- Include the table in the exact schema snapshot; run migration/reopen validation.
- Admin validation and API auth/no-store tests cover the new contracts.

### 7. Wrong vs Correct
- Wrong: use browser account-page data as the overwrite version, or trust a posted
  template config. Correct: reread monitoring before confirmation and recheck all
  versions inside each application's transaction.

## Detection Execution

- Optional `intervalSeconds` in persisted rule/template JSON selects fixed delays of
  5–31536000 integer seconds. New UI defaults to 120; absent/null retains the exact
  legacy five-field Cron and timezone contract, never silently migrate old rules.
  Seconds take precedence over retained Cron fields. `next_run` is shared by save,
  template application and completion; checked timestamp addition rejects overflow.
  Initial enabled rules and disabled-to-enabled transitions are due immediately;
  ordinary edits and subsequent rounds retain the delay from save/completion, not
  from the previous start. Keep five-second worker
  scans, global admission, no-overlap and revision fences unchanged; actual starts
  may be later. Old strict-deserialization binaries cannot read seconds configs;
  reconcile configs before downgrade. No schema migration is required.

- `AccountProbe::quality_retest` (target) and `quality_check` (judge) are separate
  from diagnostic `probe`. Both MUST use
  the persistent ordinary coordinator with its fixed-account quality selection. Never
  reuse `start_diagnostic`, synthesize a user API key, or call upstream HTTP directly.
- Only Core's target entry point may set the request-local `quality_retest` marker.
  The OpenAI fixed-target selector loads disabled targets and bypasses cached enabled,
  credential health, quota, cooldown and Excel auth-block eligibility. It still requires
  an existing in-scope account, live publication, matching credential revision and
  decodable credential. Judges enforce ordinary availability, including quality-owned
  pauses. Neither client JSON nor ordinary requests may request the target override.
- Fixed-account quality requests ignore only the target account's local model
  allowlist/denylist through a Core-only, request-local FrozenAccountScope override.
  Apply it before routing and pass the same scope to normal/queued selection. The
  original directory, other accounts, ordinary requests and client catalogs retain
  their policies; missing/out-of-scope accounts cannot gain access. This does not
  grant upstream model entitlement. Native state probes alone bypass the Excel route;
  answer checks and judges keep model-specific Excel routing.
- Provider transport, credentials, fingerprint, UA,
  egress and request interval remain authoritative. Quality checks bypass business
  concurrency and local user-facing model policy; only target retests additionally
  bypass cached availability. This never enables ordinary scheduling of an unavailable
  account or removes upstream entitlement, authentication or quota enforcement.
  A fixed quality account blocked only by its request interval waits within the original
  request deadline and cancellation scope, then reloads live safety facts. It does not
  consume a business wait-queue slot, switch accounts or inherit the ordinary hard-pin
  interval rejection. Redis lease races recheck the same account and interval; they
  must not turn a just-started parallel sample into `NoEligibleCredential`.
  Quality lease requests use a separate Redis active key in the same account hash
  slot, sharing interval/fence keys but never business active/waiting counts. Keep
  cancellation-safe ownership, deadlines and cleanup; never merely subtract in UI.
  Only explicitly configured
  quality failure actions may pause scheduling or remove selected group memberships.
  Legacy configs default to `none`; auto_restore defaults false. Explicit wrong answers
  trigger actions even when another sample failed; errors alone are inconclusive
  except for the explicit overload sequence defined below.
- Use isolated requests without user continuation or cache keys. The request kind
  is `account_quality_check`. Retain execution/cost audit but exclude it from
  ordinary client usage projections; account cumulative cost remains real.
- Business request/error lists, counts, dashboard totals/trends, diagnostic dimensions
  and account request-health timelines exclude quality successes and failures before
  pagination/aggregation, including existing history. Share the NULL-safe predicate
  in `usage_facts`; identify the trusted combination of request kind,
  `protocol=admin_quality_check` and `client_transport=internal`. Client turn metadata
  can forge request kind alone and must never hide a business request. Preserve NULL
  legacy kinds, unknown kinds and independent ops events with no request row.
- Quality results remain in quality operations. Direct administrator request-ID audit
  lookup, finalization, real account cost and provider feedback remain intact. Do not
  delete raw audit as a substitute for fixing projection scope.
- Core's trusted quality selection skips `RequestCaptureFactory::start` entirely,
  for both targets and judges; no new UI switch. Other requests cannot opt out via
  client metadata. Existing capture files remain subject to normal retention.
  Its audit-only `TraceContext` retains bounded sanitized finalization facts but
  does not emit `request_trace` or `request_dump`, including attempt/exchange clones
  and drop. Probe infrastructure, transport, protocol, timeout and availability
  failures still emit a bounded `quality_probe` warning without raw error text.
  Independent infrastructure warnings remain unchanged.
- Quiet only explicit quality upstream quota/rate-limit payload warnings using the
  existing classifier and structured code/type evidence. Bare/unknown 429, connection
  capacity, authentication, transport, storage and internal failures remain logged;
  ordinary traffic retains its warnings. Logging must not alter error classification,
  cooldown/quota updates, retry decisions, credentials, identity or scheduling.
- Test mixed historical facts against unchanged business projections (including all
  diagnostic dimensions, pagination and independent system errors), forged metadata,
  trusted target/judge capture bypass and HTTP/SSE/WS warning behavior.
- Bound output to 64 KB and request duration to 120 seconds. Inspect completion
  reason; truncated, filtered, tool-only and missing terminal output are not a
  successful answer. Commit the streaming delivery boundary only once.
- Cancellation must be awaited through coordinator finalization, including judge
  timeout. Dropping a live probe future is not cleanup.
- Judge only reference/actual answer data, not the original question; exclude the
  tested account and use an enabled configured group. Parse strict JSON with
  duplicate/unknown/trailing fields rejected. Judge errors never count against the
  target; only its explicit overload sequence can promote request failures.
- Reference judge presets name `candidate_answer`; send that field when the saved
  instructions name it, otherwise preserve the legacy `actual_answer` field. Send
  exactly two escaped values with `reference_answer`, never duplicate the answer
  or include the original question. Do not rewrite saved custom instructions.
- PostgreSQL owns global ten-job admission, five-minute renewable leases and
  rule revision fences. Five-second worker checks cancel changed/deleted rules;
  updates atomically invalidate old results. Do not schedule from browser polling.
- A round has 1–8 concurrent answer/judge samples. Cancellation waits for all active
  requests to finalize and never publishes a partial cancelled round as a verdict.
- One rule per account; enabled is required only for scheduled runs. Manual runs
  use a one-shot pending flag and never enable the schedule. Saving never queues
  an immediate run. Reference: ranxi2001/sub2api f80611d6; unlike its
  per-scan worker cap, CPR keeps a database-wide admission bound.
- Quality-owned scheduling pauses have a separate owner marker for remediation and
  opt-in recovery, not target retest eligibility. Retesting an unavailable target
  does not clear its manual pause; successful checks may undo only the rule's owned
  mutations through the existing policy transaction. Ordinary traffic and judges
  cannot request target retest eligibility or select paused accounts.
- Store action, recovery ownership, config revision and audit commit together. Lock
  runtime configuration before rule/account mutations. Rule/lease/identity fences
  reject stale actions. Same-identity credential rotation does not reset ownership.
- Full-round pass plus opt-in auto_restore may undo only owned mutations. Explicit
  scheduling commands relinquish pause ownership, including repeated false writes;
  full account forms echoing the unchanged flag do not. Renaming must not prevent recovery.
  Independent Excel HTTP 403 pauses supersede quality ownership even when the account
  is already quality-paused. Do not clear auth/quota state. Group restoration requires
  the recorded remaining group IDs to be unchanged and target groups to exist; ignore
  rewritten timestamps of unchanged memberships and restore removed timestamps.
  A pause/delete of the rule never automatically re-enables or regroups its account.
- Runs preserve their configuration snapshot. List queries omit answers; details
  are authenticated and no-store. Retain seven days / 200 runs per rule.
- Regression coverage lives in Core execution, Admin parsing/Cron, API auth,
  Store lease/fence/retention tests and frontend `tests/browser/quality-ops.mjs`.
- Private Admin parsing/Cron and Store usage-predicate tests have exact owner/file
  entries in `apps/gateway/tests/main.rs`. Keep them private, inline, gated only
  by `cfg(test)`, and after implementation items. Do not expose test APIs or exempt
  entire directories. Validate the gateway architecture suite and Clippy with
  `--all-targets`, not only the modified libraries.
- Core quality trace tests must use the public quality executor under
  `tests/engine/execution.rs`, without exposing its private audit constructor.
  OpenAI's trusted quality logging marker remains private: only the exact inline
  `quality_logging_tests` module in `provider/failure.rs` has a test-only exception.
  Reject other files, names, visibility, external paths or non-test gates.
- New persisted tables must also be registered in the exact schema snapshot in
  `crates/gateway-store/tests/postgres/mod.rs`; a frozen migration checksum and
  feature-specific Store tests alone do not cover that integration contract.
- New failure-policy config fields are backward-compatible on upgrade, but older
  binaries using strict config deserialization cannot read them. Do not promise a
  binary-only downgrade; reconcile stored configs and owned account mutations first.

## Manual Checks While Paused

### 1. Scope / Trigger
- Pausing a schedule must not prevent an explicit one-shot check.

### 2. Signatures
- `POST /api/admin/quality-ops/run` retains `{id, revision}` and AdminAuth.
- `QualityOpsStore::enqueue` sets pending; `claim` consumes it under the existing lease.

### 3. Contracts
- Claim eligibility is `pending or (enabled and next_run_at<=now())`, plus the
  existing global admission and lease guards. Completion does not enable a rule.
- Paused manual runs retain configured verdict actions, current configuration
  fences and account checks. No new request route or schema is introduced.

### 4. Validation & Error Matrix
- Pending, live lease, stale revision or missing rule -> existing conflict.
- Excel account state probes remain eligible through the trusted native route.
  A route change fences account mutations, not continued detection or manual runs.
  Historical paused rules remain paused until an explicit administrator action.

### 5. Good / Base / Bad Cases
- Good: pause, request one manual run, store its result and remain paused.
- Base: enabled rules still run on Cron and support a deduplicated manual run.
- Bad: a due Cron time causes a paused rule to run again after manual completion.

### 6. Tests Required
- Isolated PostgreSQL tests cover due-but-paused rules, single-run completion,
  duplicate enqueue, old completion fencing, deletion and Excel route changes
  before enqueue, while queued and while leased. Browser tests cover the same
  paused/queued/running button states without changing the enabled flag.

### 7. Wrong vs Correct
- Wrong: remove only the disabled button condition, or enable the rule temporarily.
- Correct: separate scheduled eligibility from explicit pending work at the store.

## State Probe Mode

- `detectionMode` defaults to `answer` for legacy rules. `state_probe` is one round
  of at most two serial HTTP SSE quality requests, 45 seconds per request, with no judge.
  It uses the ordinary coordinator, fixed account, credentials, request identity, quota,
  request interval and safety gates while bypassing the business concurrency cap.
  No direct upstream client or
  diagnostic availability bypass is allowed.
- `GenerateRequest::quality_probe` is trusted in-process context, never populated
  from client JSON. OpenAI also requires `AttemptContext::is_quality_check`.
  Each step sends at most once; ordinary retries invalidate the comparison instead
  of turning it into a State collector.
- Compare complete successful HTTP responses only: first State required; a nonempty
  different second State is suspect, equal/absent second State means no observed
  replacement. This follows reference PR 132, not a proven model-capability test.
  Do not add assumptions about State length or mandatory route cookies.
- First request has no State or account cookies; second carries only the transient
  State and optional `__cflb` / `__oailb` from the first response. Account Cookie
  persistence and ordinary session capture are disabled only for these requests.
  Stored results contain status, transport, length and comparison, never raw values.
- Each probe shot has a distinct session anchor before normal account identity and
  affinity derivation. Keep QX account-scoped projection; do not let identical probe
  prompts derive the same session or change ordinary client sessions. Probe payloads
  include the reference instructions, parallel-tool setting and encrypted-reasoning
  include field without modifying normal question or user requests.
- Preserve configured proxies, UA, fingerprint, identity and IPv6 selection policy.
  Each shot independently follows random/round-robin egress; the probe never pins an IP.
  State probes force HTTP because WS opening metadata is not response-local evidence.
  A proxy URL cannot prove that its public IP remained stable.
- Definite incorrect/degraded results and a completed explicit overload sequence
  invoke the configured policy. Other inconclusive, request errors and cancelled
  rounds do not. Existing auth and quota feedback remains unchanged.
- Enabling Excel validates current identity, OAuth type, model allowlist, credentials,
  quota and scheduling, and fences relevant account/proxy/UA/egress policy changes.
  Do not use the global config revision as a fence: independent concurrent account
  actions must not invalidate each other. A 403-disabled Excel marker vetoes auto-enable.
- Account route mutation, audit/config publication, recovery ownership and result
  commit are transactional. Enabling Excel no longer pauses native state probes.
  Only Core's trusted probe operation can set `native_quality_probe`; selector
  route freeze and actual execution both use Codex. Client JSON, judges and answer
  checks cannot acquire this override.
- `disableExcelOnNativeRecovery` defaults false when absent from persisted config;
  new applicable UI drafts explicitly enable it. It is valid only for state probes
  using enable_excel or apply_account_template. Persist `recovery.excel_owner`
  only when this rule switches Codex to Excel, after its audit/config publication.
  Record account identity, model and relevant policy scope, including account audit
  revision. Correct native results may close that route only when all still match.
  Manual/preexisting Excel, policy/identity edits, 403 protection and late results
  must not be overwritten. Never restore the whole template or clear 403 markers.
  While ownership matches, degraded rounds do not repeatedly reapply the template.
  Rule model/mode/action/template edits release ownership; frequency or recovery
  checkbox edits preserve it. Closing Excel leaves the rule active for future cycles.
- Changing detection mode clears the latest rule verdict but retains mode snapshots
  in historical runs. Keep existing owned scheduling/group recovery behavior intact.

### Account changes during State comparison

`quality_probe::rejected(AccountChanged)` keeps the existing Unsupported/NotSent
control flow and inconclusive report, but attaches an existing ProviderDiagnostic
with stage `quality_probe` and code `account_changed`, plus the concrete client
message that account/egress state changed during the check. The provider does not
retry or bypass the identity fence. Ordinary scheduled checks and quota recovery
continue unchanged; no quota-exhaustion parking or additional scheduler is added.
The regression asserts the reason, verdict, diagnostic and absence of retry intent.

## Excel Failure Threshold

- `excelFailureThreshold` defaults to 1 for legacy configs and accepts 1–100.
  New UI drafts explicitly set it to 2 without migrating saved rules/templates.
  Count definite incorrect rounds, never individual parallel samples. A correct
  round resets progress; unknown/request errors neither increment nor reset it.
  Explicit overload uses the separate fixed-two-round rule below.
- Persist progress in `quality_rules.recovery.excel_streak`, retaining unrelated
  recovery ownership. Count only after existing lease/revision fences within the
  finish transaction. Duplicate or stale completion cannot add evidence.
- Evidence is scoped to account identity and the existing outbound/policy action
  scope. A changed identity, UA, proxy or egress policy cannot inherit old progress.
  Saving any rule clears progress without queueing an immediate detection.
- Reaching the threshold calls the existing Excel enable policy, including its
  403 veto, model/account guards and audit. Native probes continue; only the separate
  explicit native-recovery option may reverse their owned Excel route.
- No migration is required for the JSON addition, but old strict-config binaries
  still require stored-config reconciliation before a downgrade.

## Native Recovery Threshold

- Optional `excelRecoveryThreshold` accepts 1–100; missing/null keeps the legacy
  single-success behavior and is omitted on serialization. New UI drafts set 2.
- Count correct native rounds in `quality_rules.recovery.excel_pass_streak` only
  after existing lease/revision/identity/model/Excel-owner guards. An incorrect
  or explicit overloaded round resets the count; other unknown/request errors
  neither increment nor reset it. Preserve an existing reset count as JSON zero.
- Before the threshold, persist `excel_recovery_counted`; at the threshold, reuse
  existing fenced Excel close/audit logic. Never close a manually enabled route,
  clear an independent 403 pause or restore the rest of a remediation template.
- Rule saves and ownership record/release clear the healthy count. Threshold-only
  edits preserve valid route ownership while invalidating previous-round evidence.
  Count survives store restarts; no schema migration or probe-wire change is needed.
- Regressions cover old-rule single-success behavior, two normal rounds, unknown/error
  holds, incorrect resets, persistence, threshold edits and stale-result fencing.

## Account Template Remediation

### 1. Scope / Trigger
- `apply_account_template` is a quality failure action, not a new scheduler or
  request path. It applies the existing full template only after the confirmed
  incorrect-round threshold or the fixed-two-round explicit overload sequence.
  Excel on/off is determined by that template.

### 2. Signatures
- `QualityRuleConfig.failure_template: Option<ReloginTemplate>` serializes as
  optional `failureTemplate: { id, revision, config }`. Old rules omit it.
- `apply_account_template_in_transaction(tx, account_id, config)` reuses the
  ordinary account scheduling/egress and group-assignment helpers.

### 3. Contracts
- Save resolves `id + revision` from `account_relogin_templates` under the
  configuration transaction and replaces the submitted config with the catalog's
  authoritative snapshot. Execution rechecks the version and snapshot. A catalog
  edit must not silently change an existing scheduled action.
- Preserve omitted / null / explicit `egressMode` semantics. Full templates also
  control enabled, concurrency, weight, groups and proxy; omitted optional Excel
  fields preserve current values. No credential, fingerprint or transport edits.
- Retain rule/lease/action-scope/identity fences, unavailable-account guards and
  independent 403 protection. Do not restore an independently paused account.
- Lock configuration before egress and account mutation. Group/proxy references
  are shared-locked; their normal mutation paths also take the configuration fence.
  Template edits do not acquire that fence after locking the template catalog.
- Apply account settings inside a savepoint. Validation/conflict failure rolls
  back every account/egress/group change while preserving the quality result.
  Storage-unavailable errors roll back the entire finish transaction. Successful
  mutation, config publication, audit and result share one transaction.
- Reuse `excel_streak` and `excelFailureThreshold` for pure incorrect rounds;
  the shared overload sequence can satisfy the trigger independently. Successful
  template application clears old recovery and records route ownership if it newly
  enables Excel for a native probe. No whole-template restoration or auto-pause.
- Historical `QualityRun.config` retains template name, version and settings.
  A binary-only downgrade cannot read the new action; reconcile configs first.

### 4. Validation / Error Matrix
- Missing/stale/deleted template at save: reject, no rule write.
- Missing/stale template at finish: `template_unavailable`, no account mutation.
- Missing group or untested proxy: `template_blocked_references`.
- Credential/expiry/quota/independent pause: `template_blocked_account`.
- 403-disabled Excel or disallowed model: existing `excel_blocked_*` guards.
- Proxy/IPv6 conflict: `template_blocked_settings`, complete savepoint rollback.
- Success: `template_applied`; retain historical paused-action labels for old runs.

### 5. Good / Base / Bad Cases
- Good: a confirmed degraded OAuth account receives the selected Excel/IPv6
  template after three incorrect rounds, with all settings published together.
- Base: old `enable_excel`, `none`, pause and remove-groups rules stay compatible.
- Bad: a template with enabled=true revives a 403-paused or invalid account.

### 6. Tests Required
- `tests/postgres/quality_ops/templates.rs` runs against an isolated PostgreSQL:
  forged snapshot replacement, thresholds, full settings, off template, probe
  pause, references/versions, current availability, conflicts, egress values,
  duplicate/stale completion and independent concurrent account actions.
- Preserve ordinary template service, relogin/import, egress and legacy quality
  regressions; missing database env is not evidence of a passing transaction test.

### 7. Wrong vs Correct
- Wrong: finish the quality result, then call the public account-template service
  in a second transaction, or trust the browser's template config.
- Correct: resolve the versioned catalog template and reuse account mutation
  helpers in the caller's existing finish transaction.


## Explicit Upstream Overload Sequence

### 1. Scope / Trigger
Both answer and state-probe modes treat explicit target upstream overload as abnormal
evidence. This does not change background quota checks, retries or ordinary traffic.

### 2. Signatures
- `AccountProbeError::is_upstream_overloaded() -> bool` checks
  `GatewayErrorKind::UpstreamUnavailable` and exact structured client code
  `server_is_overloaded`, without matching messages or HTTP status alone.
- `StateProbeReason::UpstreamOverloaded` serializes as `upstream_overloaded`;
  Core keeps the raw probe verdict `inconclusive`.
- `QualityVerdict::Overloaded` serializes as `overloaded`. Admin preserves this only
  for tested-account failures; independent judge failures remain `unknown`.
- `overload_streak::advance(tx, claim, status, overloaded) -> AdminStoreResult<bool>`
  runs after the valid lease/revision update inside `finish`.

### 3. Contracts
Persist `recovery.overload_streak` with bounded `count` (1–2), `overloaded`, account
identity, existing action scope, model and detection mode. Each completed round adds
at most one; at least one of the two abnormal rounds must contain explicit overload.
Correct rounds and rule saves clear it; other unknown/request errors hold it.
Scope/identity changes start fresh. Duplicate, stale or cancelled completion cannot
contribute. Existing healthy native-recovery progress resets on overload.

Once reached, persist aggregate `status=incorrect` and call the original policy with
`overload_trigger=true`, satisfying the template/Excel trigger without adding another
`excelFailureThreshold` wait. Pure incorrect rounds retain the configured threshold.
Keep raw answers `overloaded` and count them in `requestErrors`, not incorrect answers.
Reuse all account/template identity, quota, authentication, reference, ownership and
auto-restore protections. `none` records the result without mutating the account.

No SQL migration or new setting. Old strict binaries cannot decode the new answer or
probe-reason enum: binary-only rollback requires compatibility handling of newly stored
results first. Do not infer overload or backfill counters from old generic error text.

### 4. Validation & Error Matrix
| Sequence / condition | Result |
| --- | --- |
| Overload then overload | Second run is incorrect; apply configured action |
| Overload then incorrect, or incorrect then overload | Second run satisfies overload trigger |
| Overload then fully correct | Clear progress; next overload is first again |
| Overload then unknown/request error | Hold progress, no action |
| Multiple overloaded samples in one run | One abnormal round only |
| Quota, authentication, generic 503, timeout or judge overload | No overload evidence |
| Account changed during run | No overload promotion or stale action |

### 5. Good / Base / Bad Cases
Good: a threshold-five template still applies on the second overload round.
Base: pure wrong answers keep their threshold; healthy results use existing recovery.
Bad: two parallel samples or two completions of one lease trigger an action.

### 6. Tests Required
Core HTTP/stream errors through both modes; strict Admin classification and judge JSON
rejection; real PostgreSQL two-round/mixed/normal/unknown sequences, store restart,
lease/edit/identity/policy fencing, one-round parallel samples, account protection,
record-only/pause/template actions and existing recovery. Test UI pending versus final
aggregate states while preserving raw error reasons at desktop/mobile widths.

### 7. Wrong vs Correct
Wrong: classify all 503s or upstream messages as incorrect; reuse one sample as two rounds.
Correct: retain exact structured evidence and promote only in the fenced finish transaction.
