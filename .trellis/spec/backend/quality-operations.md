# Scheduled Quality Checks

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
- Saving/applying schedules the next configured occurrence without immediate execution.
  Provider compatibility, account settings, quality ownership and probes are unchanged.

### 4. Validation & Error Matrix
- Empty/bound/invalid template -> reject. Name: 1–128 non-control characters.
- 0, >1000, duplicate or invalid target IDs -> reject the entire application request.
- Stale template or rule revision -> no overwrite; account-level conflicts return
  a failed result while independent successful targets remain applied.
- Incompatible Excel state probe -> reject via existing checks, never change route.
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
  5–31536000 integer seconds. New UI defaults to 60; absent/null retains the exact
  legacy five-field Cron and timezone contract, never silently migrate old rules.
  Seconds take precedence over retained Cron fields. `next_run` is shared by save,
  template application and completion; checked timestamp addition rejects overflow.
  Delay begins at save/completion, not at the previous start. Keep five-second worker
  scans, global admission, no-overlap and revision fences unchanged; actual starts
  may be later. Old strict-deserialization binaries cannot read seconds configs;
  reconcile configs before downgrade. No schema migration is required.

- `AccountProbe::quality_check` is separate from diagnostic `probe`. It MUST use
  the persistent ordinary coordinator with its fixed-account quality selection. Never
  reuse `start_diagnostic`, synthesize a user API key, or call upstream HTTP directly.
- Fixed-account quality requests ignore only the target account's local model
  allowlist/denylist through a Core-only, request-local FrozenAccountScope override.
  Apply it before routing and pass the same scope to normal/queued selection. The
  original directory, other accounts, ordinary requests and client catalogs retain
  their policies; missing/out-of-scope accounts cannot gain access. This does not
  grant upstream model entitlement or bypass model-specific Excel routing policy.
- Provider transport, model-specific Excel policy, credentials, fingerprint,
  egress, request interval and availability remain authoritative. Quality checks bypass
  only the target account's business concurrency cap and local user-facing model policy.
  A fixed quality account blocked only by its request interval waits within the original
  request deadline and cancellation scope, then reloads live safety facts. It does not
  consume a business wait-queue slot, switch accounts or inherit the ordinary hard-pin
  interval rejection. Redis lease races recheck the same account and interval; they
  must not turn a just-started parallel sample into `NoEligibleCredential`.
  Only explicitly configured
  quality failure actions may pause scheduling or remove selected group memberships.
  Legacy configs default to `none`; auto_restore defaults false. Explicit wrong answers
  trigger actions even when another sample failed; errors alone are inconclusive.
- Use isolated requests without user continuation or cache keys. The request kind
  is `account_quality_check`. Retain execution/cost audit but exclude it from
  ordinary client usage projections; account cumulative cost remains real.
- Bound output to 64 KB and request duration to 120 seconds. Inspect completion
  reason; truncated, filtered, tool-only and missing terminal output are not a
  successful answer. Commit the streaming delivery boundary only once.
- Cancellation must be awaited through coordinator finalization, including judge
  timeout. Dropping a live probe future is not cleanup.
- Judge only reference/actual answer data, not the original question; exclude the
  tested account and use an enabled configured group. Parse strict JSON with
  duplicate/unknown/trailing fields rejected. Network errors are never incorrect.
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
- Quality-owned scheduling pauses have a separate owner marker. Fixed-account quality
  tests may ignore only that verified pause, the business concurrency cap and their
  explicit local model-policy exception, never credentials, quotas, cooldown or
  request interval.
  Revalidate ownership after lease
  acquisition. Ordinary traffic cannot request this override or select paused accounts.
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
- An Excel account's state probe -> reject, even when its rule is already paused.
- Switching to Excel invalidates enabled, queued or leased state probes; an idle
  paused probe is not repeatedly revised. Commit automatic pause before rejection.

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
- Only definite incorrect/degraded results invoke `enable_excel`; inconclusive,
  request errors and cancelled rounds never invoke quality policy. Existing auth
  and quota feedback still applies through the normal request chain.
- Enabling Excel validates current identity, OAuth type, model allowlist, credentials,
  quota and scheduling, and fences relevant account/proxy/UA/egress policy changes.
  Do not use the global config revision as a fence: independent concurrent account
  actions must not invalidate each other. A 403-disabled Excel marker vetoes auto-enable.
- Account route mutation, audit/config publication, rule pause, queue/lease cleanup
  and result commit are transactional. Answer rules remain enabled; probe rules
  pause. Already-Excel probes pause on claim/enqueue/heartbeat/finalization.
  Later Excel switch-off never resumes a rule automatically. EnableExcel has no
  auto-restore-to-Codex policy.
- Changing detection mode clears the latest rule verdict but retains mode snapshots
  in historical runs. Keep existing owned scheduling/group recovery behavior intact.

## Excel Failure Threshold

- `excelFailureThreshold` defaults to 1 for legacy configs and accepts 1–100.
  Count definite incorrect rounds, never individual parallel samples. A correct
  round resets progress; unknown/request errors neither increment nor reset it.
- Persist progress in `quality_rules.recovery.excel_streak`, retaining unrelated
  recovery ownership. Count only after existing lease/revision fences within the
  finish transaction. Duplicate or stale completion cannot add evidence.
- Evidence is scoped to account identity and the existing outbound/policy action
  scope. A changed identity, UA, proxy or egress policy cannot inherit old progress.
  Saving any rule clears progress without queueing an immediate detection.
- Reaching the threshold calls the existing Excel enable policy, including its
  403 veto, model/account guards, audit and probe pause. This feature never turns
  Excel off automatically and does not alter normal routing or retry decisions.
- No migration is required for the JSON addition, but old strict-config binaries
  still require stored-config reconciliation before a downgrade.

## Account Template Remediation

### 1. Scope / Trigger
- `apply_account_template` is a quality failure action, not a new scheduler or
  request path. It applies the existing full template only after the confirmed
  incorrect-round threshold. Excel on/off is determined by that template.

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
- Reuse `excel_streak` and `excelFailureThreshold`; no second counter. Successful
  template application clears this rule's recovery, has no auto-restore, and
  pauses state-probe rules only if the resulting account route is Excel.
- Historical `QualityRun.config` retains template name, version and settings.
  A binary-only downgrade cannot read the new action; reconcile configs first.

### 4. Validation / Error Matrix
- Missing/stale/deleted template at save: reject, no rule write.
- Missing/stale template at finish: `template_unavailable`, no account mutation.
- Missing group or untested proxy: `template_blocked_references`.
- Credential/expiry/quota/independent pause: `template_blocked_account`.
- 403-disabled Excel or disallowed model: existing `excel_blocked_*` guards.
- Proxy/IPv6 conflict: `template_blocked_settings`, complete savepoint rollback.
- Success: `template_applied` or `template_applied_probe_paused`.

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
