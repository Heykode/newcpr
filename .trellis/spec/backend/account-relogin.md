# Account Relogin Contracts

## Optional Material Deletion With Account Removal

### 1. Scope / Trigger
Single and batch account removal may explicitly delete shared login-library material.

### 2. Signatures
- `POST /api/admin/accounts/delete` accepts optional `deleteRelogin:boolean`, default
  false. Carry it as `delete_relogin` through CredentialDeletion, DeleteAccounts and
  DeleteProviderAccounts; omit secrets and caller-provided emails from the payload.
- OpenAI opted-in deletion enters `ReloginService::delete_accounts(CredentialDeletion)`.

### 3. Contracts
- Resolve authorized OpenAI OAuth accounts' emails server-side. Delete email-matching
  account_relogin_entries case-insensitively in the account deletion transaction.
  Missing material is harmless. Preserve the entry whenever an unselected OpenAI
  OAuth account still uses that email. Delete shared material only when every remaining
  same-email workspace is selected. Unrelated accounts/emails remain untouched.
- Hold the existing relogin gate across deletion, matching worker settlement/manual
  push ordering. Afterward cancel affected active jobs only when their material is
  absent. Recheck after publication errors because the transaction may have committed.
  Failed transactions retain jobs/material. Late settlement cannot recreate deleted rows.
- Audit the opted-in material change without email, password or TOTP contents.
  Deleting local material does not disable upstream 2FA. No extra upstream calls.
- This gate inherits the existing single-instance worker constraint; it is not a
  cross-replica distributed lock.

### 4. Validation & Error Matrix
- Absent option -> retain material; nonboolean/null -> reject request (422).
- Missing/wrong-provider account -> complete transaction rollback, no partial cleanup.
- Post-commit publication error -> propagate uncertainty; reconcile material before
  cancelling tasks and never blindly replay the account deletion request.

### 5. Good / Base / Bad Cases
- Good: remove a selected account's unique material; keep shared material for an
  unselected workspace, or delete it when all same-email workspaces are selected.
- Base: old clients and unchecked confirmations remove only accounts.
- Bad: two browser-side delete calls, caller-selected arbitrary emails, or job cancellation
  before a transaction that later rolls back.

### 6. Tests Required
- API DTO default/true/invalid-type cases.
- Isolated Store deletion checks for keep/delete/missing material, shared mixed-case
  emails with selected/unselected workspaces, unrelated rows and full-batch rollback.
- Admin active-login deletion rejects late resurrection; failed deletion leaves login live.
- Frontend default/reset/cancel/dedup, frozen batch intent and partial-provider failure.

### 7. Wrong vs Correct
Wrong: delete library data first in a separate operation and hope account deletion succeeds.
Correct: one Store transaction under the relogin service gate, followed by absence-based
task cancellation and ordinary missing-entry/revision fences.

- Bulk 2FA import and its preview recognize LF, CRLF (one source line), bare CR,
  NEL, Unicode line/paragraph separators, blank lines and per-row BOM padding.
  Email padding and whitespace in Base32 TOTP material may be normalized; password
  bytes, including spaces and interior `----`, must remain unchanged. Never join
  fragments across physical lines or silently drop invalid records. Both import
  textareas disable visual wrapping without changing the submitted text.

- Account-management 2FA enrollment is a separate explicit intent, not a change to
  ordinary library import/manual queue semantics. Persist frozen import settings
  and administrator context; reuse the existing worker, identity/CAS and create-only
  import. Existing-account enrollment only rotates credentials, preserving settings.
  New enrollment login uses the selected managed proxy; validate again before import.
  Paused queues reject new enrollment. Active/uncertain rows cannot be replaced by it.
  Workspace selection preserves intent; Ready enrollment resumes push after pause
  without logging in again. Cancel, manual queue/push and successful settlement clear it.
- New-account push must retain the enrollment's custom name when no replacement name
  is supplied. Inheriting egress or applying Excel options never implies clearing a
  name. Test the complete queued-login-to-import path with a name and absent,
  inherited, direct and managed-proxy egress, comparing all committed settings with
  the original submission and checking the proxy used during login as well as import.
- Failed new-account enrollment reuses `maxRetries` and `retryIntervalSeconds`.
  Count the initial attempt (including proxy-resolution failure) in persisted
  `automatic_attempts`, without marking the job as existing-account automatic
  recovery. Retain enrollment settings, proxy and selected workspace across retries.
  Only failed rows without cached credentials/targets may enter this path; pool
  identity appearing meanwhile, terminal errors, workspace choice, cancellation and
  uncertain pushes remain fenced. Old failed enrollment rows with zero attempts
  count as one prior attempt. Explicit workspace selection starts a fresh budget.
  This finite import budget does not use the existing post-success expiry loop guard.
  It does not make ordinary library imports automatically enroll.
- Enrollment owns optional `purchase_cost` separately from reusable template config.
  Carry the existing `AccountPurchaseUpdate` through construction, persisted JSONB
  and `settings()` replay without changing decimal strings or the optional cycle date.
  Reuse `AccountPurchaseUpdate::validate` before queuing and on replay; the enrollment
  API does not run the ordinary credential-import request's `validate()` method.
  Missing fields in old tasks deserialize as no cost update; explicit zero is retained.
  No schema migration or normal account-template cost setting is introduced. Existing
  account enrollment stays credential-only. Costs already omitted by an older writer
  cannot be recovered or guessed. Verify against the original submitted cost, not
  another call to the same potentially lossy settings converter.
- Relogin export requires explicit selected IDs (at most 200), administrator auth,
  confirmation and no-store. Export one CPR JSON bundle or one three-field text file.
  Resolve actual pooled identity/workspace before reading credentials; reject deleted,
  mismatched or ambiguous targets. Never log in implicitly for export. Audit ID/format
  only; no sensitive Debug derivation, normal-list fields or credential JSON additions.

- `gateway-admin/model/relogin` owns login-library facts; provider-owned JSON is opaque.
  Do not add passwords or TOTP material to managed credential documents.
- `ReloginStore::save_batch` must be atomic. Per-row writes and deletes use revisions.
  Never derive `Debug` for secret-bearing records or return them as API DTOs.
- Manual login only caches verified credentials. Explicit push confirms the library
  revision. Automatic recovery requires enabled OAuth, expired credential state, a
  matching terminal expiry reason, and an enabled email-matched library entry.
- OpenAI `expired` plus `access_token_expired` can trigger password/TOTP recovery
  even when a refresh token exists. The refresh worker remains independent; recheck
  account health and authentication generation before pushing so its successful
  recovery is not overwritten. Expired accounts remain blocked from inference.
  Do not rewrite tokens from an inference request's stale snapshot.
- Existing account recovery captures account ID, principal, workspace and credential
  revision. Recheck these at push; automatic push also rechecks recovery eligibility.
  Use the existing prepared rotation and CAS transaction, never delete/recreate.
- A queued or cached target survives Cookie-only credential writes. Project the existing
  `turn_state_binding_revision` into the internal admin account record and require
  `current binding <= captured credential revision <= current credential revision`,
  along with the original account, user, workspace and OAuth-kind checks. Binding
  changes are stamped with the resulting credential revision, so old JSONB target
  snapshots need no guessed generation or migration. Never compare only the emails
  or allow a target revision from the future.
- Use the same target check for account-menu confirmation, queued execution and
  automatic attempt budgeting. Cookie writes must not reset the configured attempt limit.
  After Provider preparation, reread the target and match the exact prepared revision
  before the Store CAS; a Provider may have reloaded newer material during preparation.
  Existing-account pushes may prepare/commit at most three times when proven-safe
  revision movement or a definitively rolled-back conflict races the write. Keep one
  operation ID based on the original target. Never retry unavailable/ambiguous commits,
  publication or settlement failures. Exhausted Cookie races retain the cached result.
- Successful prepared credential replacement updates authentication facts even for
  manually paused accounts, but preserves `enabled`. A concurrent pause must not
  be undone by a relogin push. Ordinary file import is different: its explicit
  scheduling settings remain authoritative.
- The provider rotation parser accepts the worker's complete token document, including
  optional `email`, `account_id` and `type: codex`. Email/workspace metadata must agree
  with the new token claims; it cannot supply missing identity or override claims.
  Keep unknown-field rejection and same-principal/device continuity checks. Test this
  contract against the real provider, not only an admin fake that ignores the JSON.
- New push uses the existing import preparation and initialization with create-only
  semantics. The PostgreSQL transaction checks email/identity under the configuration
  lock, and the final INSERT conflict clause forbids updating an existing identity.
  Normal account imports retain their original upsert behavior.
- Persist `Pushing` before pool mutation. A crash or ambiguous failure becomes
  `Uncertain`, not an automatic retry. Library and pool commits have distinct owners;
  no distributed transaction or exactly-once claim is made.
- Successful inference or diagnostic recovery uses the start time of that transport
  attempt for the credential-state observation, carried through both streaming
  completion paths. The store must atomically fence that time and credential revision:
  a response started before a later authentication failure cannot clear it at completion
  and suppress automatic relogin push. A fresh successful diagnostic after the failure
  may still restore Ready. Retain disabled/replaced-account and relogin identity guards;
  do not remove `needs_relogin` checks or add a verification request to every push.
  This is a local correction to upstream's completion-time recovery, verified against
  upstream `4bf1208`; it is not an upstream-provided relogin fix. Quota observations,
  cooldown recovery and session-affinity ownership keep their separate existing rules.
- A Store conflict is returned only after transaction rollback succeeds. It can restore
  `Ready` for bounded target revalidation/retry; failed rollback or unknown commit
  remains `Pushing`/`Uncertain`. Do not generalize this exception to all failures.
- For existing-account push, prepare and validate rotation before persisting `Pushing`.
  A preparation error has not attempted the pool commit: retain `Ready`, the cached
  document and its revision so an explicit push can retry. New-account import still
  fences before calling the combined prepare/commit import use case. Do not move the
  fence past the commit or downgrade post-commit publication/settlement failures.
- Manual and automatic login share bounded concurrency. Publish in-memory occupancy
  only after all fallible claim writes complete; otherwise a partial claim failure
  can permanently fill the queue with jobs that were never started.
- Scan every two seconds while login exchanges are active and refill available slots
  after individual completions. Poll exchanges while awaiting claim: settlement may
  hold the gate across an asynchronous save. Awaiting claim alone can deadlock it.
  Keep execution owned by the scheduled cycle, not detached tasks; dropped cycle
  owners release stale occupancy and orphan persisted rows retain failure fences.
- Confirmed push immediately clears failure cooldown, per-generation attempts,
  attempted target and stop reason. A subsequent expiry gets a new retry budget;
  never require a successful inference request or a health observation period.
  Failed recovery uses a fixed configured interval and at most `maxRetries + 1`
  automatic attempts per authentication generation. The first attempt is not a retry;
  `maxRetries=0` permits it but stops after failure. A new binding can recover
  immediately unless an explicit terminal reason requires manual intervention;
  Cookie writes cannot reset the budget.
  Preserve a JSONB-defaulted rolling history of automatic starts across success and
  reimport: at most three starts in fifteen minutes, including across new bindings.
- `account_relogin_settings` owns `maxRetries` (default 2, range 0..10) and
  `retryIntervalSeconds` (default 30, range 1..86400). Migration 0070 renames
  the column from migration 0034, changes the former five-minute default to thirty
  seconds and converts other saved minute values without changing their duration.
  Do not reinterpret the old minute API field as seconds; stale writes are rejected.
  Old settings requests that only send concurrency/paused preserve these values.
  Apply optional updates while holding the service gate. Retry-setting changes do
  not cancel active jobs, clear counts, or rewrite existing cooldown deadlines.
  Failures and orphan-running recovery use the interval current at settlement.
- Persist optional JSONB-defaulted `stop_reason`: `account_banned` or
  `workspace_unavailable`. The worker consumes typed Provider errors; never classify
  generic HTTP 401/403, password rejection or arbitrary error text as an account ban.
  Python recognizes explicit structured denial codes on 400/401/403 and its own
  missing-locked-workspace result. HTTP 429 remains transient. The one exact legacy
  message `指定工作区不可访问，未回退到个人空间` is compatible with workspace unavailability.
  Terminal failed rows remain stopped until manual login, material reimport or a
  validated workspace change; settings edits and pool credential writes cannot
  silently resume them. A banned pool account is also ineligible. Manual login
  remains available without bypassing any identity/workspace/push checks.
- One shared eligibility projection drives the worker and the safe list `recovery`
  field. Expose waiting, cooldown/retry time, pause, missing material, workspace
  ambiguity, retry limits, `manual_required` with its short cause, loop protection
  and retry progress (`retriesUsed` excludes the initial attempt) without secrets.
  `Pushing`/`Uncertain` take precedence and never automatically replay; neither
  settings edits nor terminal metadata changes settle an uncertain push.
  Manually obtained valid credentials await explicit push.
- Pausing, deleting, editing material or changing per-entry automatic/workspace state
  cancels in-flight results using both cancellation and revision fencing.
- The current worker assumes a single CPR instance. Multi-replica relogin requires a
  separate durable claim/lease design, not merely another copy of the in-memory gate.
- Python login is an isolated, bounded subprocess with secrets on stdin, verified TLS,
  fixed redirect allowlists and OAuth state checks. Unknown/interactive pages fail
  closed. Protocol fixtures are not proof of current live upstream login compatibility.
- Relogin supports password plus TOTP only. Reject legacy mailbox four-part and pipe
  formats, including UUID-plus-token suffixes that might resemble TOTP material.
- Integration tests mirror the production module layout: a directory module such
  as `use_case/relogin` uses `tests/use_case/relogin/mod.rs`. Run the App workspace
  architecture tests as well as the API architecture checks before merging.
  Email verification pages require manual interaction; never send or read email codes.
- Old JSONB rows with an empty TOTP secret remain readable/deletable but cannot queue,
  auto-login, enable automatic recovery or push cached credentials. The list hides their
  cached credentials and reports invalid material without mutating storage. Preserve
  uncertain push states. Reimporting valid TOTP material resets the row through normal
  revision fencing; unknown legacy fields are no longer serialized on subsequent writes.
- Relogin/maintenance resolves pooled accounts' current effective route from account management.
  Unpooled material retains its imported proxy independently of enrollment/auto-push intent;
  clearing legacy enrollment first copies that route. Explicit template route overrides apply
  only to new pushes; preserve inherits import, clear uses deployment default. Validate the
  effective reference before the push fence. Never fall back to direct after proxy failure.
  Known fixed IPv6 bindings pass through to the worker; rotating sessions use the existing
  provider selector once and retain the source while allowed. Listing must not advance the
  selector. Unpooled IPv6 fixed affinity is still allocated by the account-creation transaction,
  not by reading material. Preserve explicit deployment IPv6-disabled policy.
  A whole operation uses one route snapshot. Explicit proxies clear libcurl NOPROXY; explicit
  source addresses disable environment proxies with CurlOpt.PROXY. trust_env alone is insufficient
  in pinned curl_cffi 0.15.0. Unbound requests retain normal deployment environment behavior.
  Ordinary lists return sanitized endpoints and timestamped test IPs, never proxy credentials.
  Multiple unresolved workspace routes must not be presented as one default route.
- Deduplicate membership aliases by the real workspace ID before ranking or matching
  a locked workspace. Normalize the official plan wire aliases consistently across
  memberships, access/id token claims and usage verification: self-serve Business,
  enterprise CBP/ent26/hc, education and prolite retain their paid policy tier.
  Conflicting plans for one ID and unknown plans fail closed. Equal top-tier workspaces
  produce bounded choices for explicit manual selection; never guess a workspace.
- During OAuth account selection, the authentication transaction `session_id` is not
  a selectable login session. Use `unified_sessions` or an explicit selected-session
  field; never use a transaction ID to resolve missing or ambiguous login choices.
- The runtime venv removes its seeded setuptools after installing worker dependencies.
  Keep `pip check`, isolated imports, a public TOTP test vector and browser-session
  initialization in the image build; none of these checks should make login requests.
  Do not suppress container vulnerability gates to retain unused build dependencies.

## Manual Workspace Selection

### 1. Scope / Trigger

Only an unlocked manual acquisition with tied highest-plan memberships can enter
`AwaitingWorkspace`. Original/automatic/account-menu recovery stays workspace-locked.

### 2. Signatures

- Python `WorkspaceSelectionRequired` emits
  `{ok:false, code:"workspace_ambiguous", workspaces:[{id,name,planType}]}`.
- `validate_workspace_choices(&[ReloginWorkspaceChoice])` owns typed validation.
- `ReloginService::resume_workspace(id, revision, workspace_id)` is exposed by
  authenticated `POST /api/admin/relogin/workspace/resume`.

### 3. Contracts

Choices contain 2..64 distinct nonempty IDs (<=128 bytes, no whitespace/controls),
names <=128 characters without controls and the same recognized normalized plan.
Provider rejects malformed choices and ambiguity for an already locked request.
No raw membership, login session or credential enters the public choice DTO or public error message. A provider error may privately carry a verified WebSession to the relogin worker; Debug, API errors and logs must never serialize it.
The JSONB entry defaults `workspace_choices` to empty and `selected_workspace_id`
to None; waiting survives restart without holding an exchange or retrying.
Resume queues a fresh OAuth exchange (reusing only a verified account Web session) under the gate using the observed revision and
one captured candidate. Keep mode and frozen replacement targets; revalidate their
identity/revision/email, then recheck membership and acquired identity upstream.
Never set automatic_job or manual_push_context. Successful acquisition stays Ready
for separate explicit push. New acquisitions/material edits clear old choices.
Old binaries cannot deserialize the new status: drain/clear waiting rows before
downgrade. No database schema change or automatic deployment is implied.

### 4. Validation & Error Matrix

- Missing/unknown DTO fields: HTTP extraction rejects before service access.
- Unknown candidate: invalid request; stale revision/status or paused/active queue:
  conflict, no mutation.
- Changed/deleted frozen target or original recovery intent: conflict, no requeue.
- Cancelled or revised in-flight task: discard candidates, no late waiting state.
- Selected workspace missing or returned credential mismatch: fail, no Free fallback.
- Pushing/Uncertain: cannot resume or infer successful settlement.

### 5. Good/Base/Bad Cases

- Base: one Business plus Free still acquires Business with no prompt.
- Good: tied Business choices wait; confirmed choice acquires, manual push follows.
- Bad: choose the first candidate, allow typed arbitrary IDs or retry while waiting.

### 6. Tests Required

Python ranking/dedup/sanitization and safe subprocess output; Admin no-retry,
slot release, original lock, cancellation, stale/unknown selection, target retention,
no automatic push and wrong-workspace rejection; API auth/required typed fields;
isolated PostgreSQL waiting/selected JSONB roundtrip and CAS. UI regression:
`frontend/tests/browser/relogin-workspace-selection.mjs` with mock APIs only.

### 7. Wrong vs Correct

Wrong: save a preferred workspace then issue a normal highest-mode queue request;
this ignores the choice and may replace the frozen targets with a later snapshot.
Correct: one revision-fenced resume atomically selects the captured candidate and
preserves acquisition mode and targets; push is a separate confirmation.

## Account Menu and Import Templates

- `ReloginEnrollment` may persist a revision-fenced monitoring-template selection.
  Apply it only after the account credential commit is confirmed; failure leaves the
  account synced and records a warning rather than replaying login or pool writes.
- Explicit selected-account JSON export includes valid matching password/TOTP material
  in a top-level `relogin` array in the same no-store response. Match only selected
  OpenAI OAuth account emails, audit the relogin entry export and keep every Debug view
  redacted. Accounts without valid material produce no synthetic secret fields.

- Library manual acquisition explicitly selects `original` or `highest`; omitted API
  mode and old JSONB rows remain `original`. Highest ignores the stored preferred
  workspace only for this acquisition, freezes same-email targets before login and
  verifies the returned principal. Never fall back to Direct if a captured target
  disappears. Multiple candidates with different proxies must not be guessed.
- Highest acquisition never automatically pushes. An explicit push includes the entry
  revision and selected account ID plus `switchWorkspace`. Select only a captured
  target. If the destination workspace is already pooled, update that row only; later
  arrivals, deleted/recreated accounts and changed bindings require reacquisition.
- Workspace replacement uses dedicated Provider preparation and Store commit ports.
  Both independently require the same nonempty upstream user and matching email;
  complete newly acquired tokens are mandatory. Ordinary rotation, account-menu
  recovery, refresh and automatic recovery remain workspace-locked.
- Confirmed switching preserves the local account ID, names, settings and history.
  Rebind its durable installation identity under the existing transaction locks;
  conflicting active or archived destination devices fail closed, never steal a binding.
  Credential CAS, identity uniqueness, audit, success count, device update and old quota
  invalidation are atomic. The new binding generation fences old State slots.
  On success prefer the selected destination for subsequent automatic recovery.
  Preserve uncertainty fencing and Cookie-only bounded retries.

- Account-menu actions query only requested pool IDs and expose matching valid TOTP
  availability, task status and a target snapshot, never login secrets. Queueing
  rechecks material revision and the selected account/principal/workspace/credential
  revision. Manual confirmation is independent of the entry's automatic switch.
- Persist `manual_push_context` with a default of `None` for old JSONB rows. This
  authorizes worker settlement to the selected existing account only; it cannot
  create a replacement if that account disappears. Cancel/edit/library queue paths
  clear the intent. Preserve the administrator's audit context and pool settings.
- `AccountTemplatesService` owns the one shared template catalog; account management
  and relogin use the existing `/api/admin/relogin/templates*` endpoints and storage.
  Templates contain only named account settings and an optional saved proxy ID.
  `account_relogin_templates` stores independent revisions and unique normalized names.
  Updates/deletes require their confirmed revision. Changing a template never changes
  accounts that previously used it.
- Optional `turnStateInjectionEnabled` is a boolean switch only, not State parameters.
  Legacy missing/null preserves existing account State when applied; new imports retain
  their default-off behavior. Newly saved UI templates use explicit true/false.
- `POST /api/admin/accounts/apply-template` accepts frozen `accountIds` (1-1000 unique)
  and `template: { id, revision }` only. Resolve and validate one server-side snapshot,
  then use `AccountsService.batch_update` for atomic settings, auditing, provider
  notification and configuration publication. Groups replace assignments, empty groups
  clear them, null concurrency restores defaults, and no proxy explicitly selects Direct.
  Reject State=true on any non-OpenAI target before mutation. No credential/device
  replacement, template binding, automatic retry or schema migration is implied.
- Manual push optionally captures template ID/revision; resolve one immutable config
  snapshot per batch. Stale/deleted templates fail before any push. `newAccount` carries
  the shared import form's final settings, proxy and optional monitoring template.
  Reuse `apply_prefilled_import` with that snapshot: edits own the settings; the template
  only supplies legacy State defaults. Keep legacy template-only clients compatible.
  Only create-only imports receive settings/proxy/monitoring; existing rotation remains
  credential-only. Clearing enrollment intent must preserve its imported route. An
  omitted route retains that exit; explicit direct/proxy/IPv6 settings replace it.
  Monitoring failure after import is a warning, never grounds to replay credentials.
- Validate group existence and tested proxy availability before entering `Pushing`.
  A preflight rejection preserves `Ready` and cached credentials. Import transactions
  still own final reference checks, create-only identity fencing and device creation.
  Races or failures after the import fence retain the existing uncertain semantics.
- Verify template CRUD/CAS, missing references, mixed batches, default imports and
  original credential/device preservation against an isolated PostgreSQL instance.

## Custom Account Names

- `provider_accounts.custom_name` is nullable local display metadata, introduced by
  additive migration `0032_account_custom_names.sql`. Never substitute it for
  provider-owned name/email, login matching, principal, workspace, credentials,
  installation identity or Core scheduling facts. Every SQL projection feeding
  `account_summary_from_row` must select it, including refresh and group members.
- `customName` is optional on imports and manual push. Normalize through the shared
  Admin helper: reject control characters and raw inputs over 512 UTF-8 bytes before
  trimming; blank becomes None. HTTP validation and Store mutations use the same
  rule. Additive migration `0080_account_name_bytes.sql` aligns the database bound.
- Single/batch updates preserve omitted names and clear explicit null/blank names.
  Import omission/blank preserves an existing name and leaves a new row unnamed.
  Name changes belong to the existing settings/audit transaction; audit field names
  only. Failed group/proxy/audit validation rolls back the name too.
- Templates never contain names. Template application explicitly omits custom_name.
  Manual push freezes the independent batch name once and applies it only to the
  create-only import branch. Existing/manual/automatic rotation and token refresh
  never update the column. No-template named pushes retain ordinary new defaults.
- Search includes custom names; real email and upstream profile fields stay intact.
  Existing sorting and historical request snapshots are not redefined by a rename.
- The migration is not eligible for the normal deployment fast path. Deployment is
  separate and must follow the reviewed migration procedure; do not rewrite frozen
  migrations or promise an automatic rollback across the schema change.

## Success Counts

- `provider_accounts.relogin_count` / `last_relogin_at` are pool-owned facts, not
  library fields or `automatic_attempts`. Both account and relogin views read them.
- Only relogin replacement commits carry a stable operation ID derived from the
  locked account ID and original credential revision. In the existing Store CAS
  transaction, insert the unique `account_relogin_successes` event and increment
  the counter. CAS, event, or audit failure rolls back all changes.
- First acquisition/create-only import, cached JSON, failed/cancelled jobs,
  ordinary refresh, manual JSON rotation and OAuth authorization do not count.
  Reimporting or deleting library material does not reset pool-owned history.
  Account deletion ends its statistics lifecycle; do not infer old history.
- A library settlement failure after a committed pool rotation does not lose the
  count. Preserve the existing `Pushing` / `Uncertain` fence; the count is not proof
  that library settlement or snapshot publication succeeded.
- Select statistics by locked account/principal/workspace, explicit workspace or a
  unique email candidate. Return `null` for unresolved targets, not combined or
  guessed counts. An entry with no pool matches returns zero.
- Sort by the persisted counter before pagination. Keep counts out of provider
  JSON, core scheduling policy, device identity and transport configuration.
- `account_summary_from_row` is also used by account-group member/monitor queries.
  Keep every SQL projection feeding it in sync, not just account-directory reads.

## List Projections

- `imported_at` records the latest explicit material import, not login/automatic-toggle
  time. Preserve it through background updates. Old JSON can omit it: derive legacy
  creation time from a valid UUIDv7 entry ID, otherwise expose unknown. Never backfill
  using `updated_at`. Sort the list view by import time and ID descending without
  changing worker queue ordering or database migrations.
- `poolAccounts` contains safe same-email pool facts and the shared Core operational
  status projection, using the existing runtime rate-limit snapshot. `enabled` remains
  independent from runtime errors. Do not infer HTTP 401 from a generic expired state.
- Cached credential verification, previous sync and current pool health are independent.
  Neither a normal pool account nor a success counter settles an uncertain library push.
- Safe list projections may expose `hasTotp` only as the result of full local material
  validation. Never expose the password, TOTP secret, derived code or secret metadata.
- Workspace selection accepts known same-email pool workspaces or the current verified
  document workspace. Unknown choices fail without clearing cached credentials. Selection
  changes still cancel/fence old work, clear cached credentials and require a fresh login;
  upstream ownership and locked-target checks remain authoritative.

## Account Name and Credential Preview

### 1. Scope / Trigger
The relogin presentation follows current pool names and offers explicit 2FA/JSON viewing.
This changes display metadata and UI entry points, not matching or worker eligibility.

### 2. Signatures
- `GET /api/admin/relogin` adds `items[].displayName: string | null`.
- Viewing uses the existing `POST /api/admin/relogin/export` with one selected ID,
  `format: two_fa | json` and `confirm: export_sensitive_relogin`.

### 3. Contracts
Resolve `displayName` from the existing `statistics_account` result, preferring
`custom_name` to provider `name`. Do not persist a second name or pick the first
same-email workspace. The frontend falls back to email; normal list polling reflects renames.
The dialog displays the export content unchanged, including the full three-field
2FA line. JSON retains existing pool credential selection and audit/no-store behavior.
Close, account change, tab change and unmount abort old reads and discard content.
Mobile cards emit the same queue/push/workspace/delete and selection actions as the table.

### 4. Validation & Error Matrix
Ambiguous or changed pool identity => null name, email fallback. Invalid 2FA or
unavailable JSON => existing export error; never trigger a login to fill the preview.
No sensitive fields enter list metadata, browser persistence or ordinary logs.

### 5. Good / Base / Bad Cases
Good: a pool rename appears on refresh and the JSON tab displays current export data.
Base: old servers without displayName use email. Bad: selecting an arbitrary same-email
account, synthesizing credentials, or letting a late response replace another tab.

### 6. Tests Required
Extend the existing identity/workspace statistics test for ambiguous, renamed,
provider-name fallback and changed-identity results. Browser coverage exercises full
copy text, tab/account/close races, name search, mobile actions and existing confirmations.

### 7. Wrong vs Correct
Wrong: adapt worker status or queue/push rules to simplify a card. Correct: reuse the
existing presentation helpers and handlers; template management remains in accounts,
while create-only push retains its template picker and existing-account settings.

## Verification

### Interactive OAuth Reauthorization

- Require matching, nonempty old and new upstream user IDs in addition to the
  existing email/workspace checks. Preserve existing error precedence and reject
  before persisting any account or credential mutation.
- Scope this check to `complete_claimed_authorization`, not the shared
  `prepare_candidate_oauth_rotation`: trusted administrator file imports
  intentionally retain their existing metadata-update contract.
- Keep automatic relogin identity checks, token refresh, installation identity
  and State binding rules unchanged. Test rejection releases the pending claim,
  leaves storage unchanged, and successful same-user reauthorization preserves
  the device and fallback refresh token.

Run the relogin model/service tests, API auth/wire tests, isolated PostgreSQL relogin
tests and existing device-registry regressions. Run Python offline tests with
`python3 -m unittest discover -s backend/crates/providers/openai/tests -p 'relogin*_test.py'`.
Verify both new push and original in-place rotation paths. Never use real credentials
as fixtures. Deployment and live login acceptance require separate authorization.
On 2026-09-15 one explicitly authorized Free account passed the live Python worker
through token exchange and usage verification. This does not validate live pool
push, automatic recovery, Team/Business workspaces or deployed runtime behavior.

## Attempt History and Explicit TOTP Reads

- `ReloginEntry.last_attempt` defaults to absent for old JSONB. The worker starts
  a distinct UUIDv7 attempt per actual claim/retry. The safe attempt contains source,
  start/end time, login result/message, account/workspace and separate push outcome.
  It never contains passwords, TOTP, Token or credential documents.
- Migration 0087 adds `account_relogin_history(entry_id,id,detail)` with a cascading
  material foreign key. Save the latest snapshot and historical row in the same
  existing revision-fenced transaction; batch rollback leaves no phantom history.
  Reimport preserves history, deletion of material removes it. Business pg_dump
  includes the new table. Preserve all old migrations and success statistics.
- `POST /api/admin/relogin/history` accepts `{id,before?}`; return up to 50 descending
  UUIDv7 records and `nextCursor`. Invalid cursor => invalid request, absent entry =>
  not found. List `lastAttemptAt` is independent from pool-owned `lastReloginAt`.
- Queuing/cancelling another login must preserve the previous attempt and its push outcome.
  A generic Failed entry with cached credentials is not evidence of a failed push;
  only the push operation records push failures. Keep attempt creation at worker claim.
- Preserve preflight push failure semantics: `save_attempt(id,revision,attempt)`
  changes only the same attempt snapshot/history under CAS without incrementing the
  library revision or consuming the existing valid push confirmation. Post-commit
  ambiguity remains uncertain; a history outcome is not proof of pool commitment.
- `POST /api/admin/relogin/totp` accepts only `{id}`, requires admin auth, audits
  `view_relogin_2fa` with the entry ID and returns only `{secret}` with `no-store`.
  Missing/invalid material fails without a secret; audit failure prevents disclosure.
  This is a deliberate sensitive endpoint, not a change to safe list/export schemas.
- Cover acquisition failures followed by retries, successful login/failed push and
  later push success, cancellations, stale revisions, atomic rollback, pagination,
  material deletion/reimport, explicit secret access and list/history non-disclosure.


## Web Session and Account Maintenance

### 1. Scope / Trigger

Relogin maintenance reuses the existing account gate, global concurrency, revision CAS,
private JSONB material and history. It does not modify request scheduling or State ownership.

### 2. Signatures

- `POST /api/admin/relogin/maintain`: `{id,revision,action,target?,deviceIdHash?,confirm?}`.
- `POST /api/admin/relogin/cancel`: `{id,revision}`.
- `POST /api/admin/relogin/material`: `{id}` -> one private snapshot containing its
  `revision`, current material, pending secret, `needsResolution`, and history.
- `POST /api/admin/relogin/material/update`: `{id,revision,password,mfaSecret,
  confirmMaintenanceResolution?:boolean}`. Explicit manual reconciliation only.
- `ReloginEntry.maintenance` defaults for old JSONB. No schema migration.

### 3. Contracts

`MaintenanceView` is the only ordinary-list projection: no cookies, password, TOTP key,
enrollment session or credential document. Sensitive reads require admin auth, successful
access audit and `Cache-Control: no-store`. Secret-bearing records never derive Debug.

`WebSession` binds normalized email, device/session IDs, verified time and cookie domain,
path, expiry, secure and host-only semantics. Capture cookies at the successful identity
check, not from a later unverified jar. Failed candidates never overwrite a saved session.
A completed Web stage can survive later OAuth failure or workspace ambiguity.

Session checking is distinct from OAuth authorization. Authorization always makes fresh
state/PKCE and exchanges an authorization code; it never labels a cached export as relogin.
Token string inequality and revocation of all older grants are not guaranteed contracts.
Fresh login is a separate explicit action. HTML, malformed session JSON, transport errors
and identity mismatch must not silently trigger a password login; only explicit expiration
may fall back. Web navigation can skip a denied membership; final OAuth stays target-locked.
An accounts-backend `401 token_revoked/token_expired` invalidates the earlier Web snapshot
even if `/api/auth/session` returned a token. A rejected mutation has no unknown effect;
a read failure after an accepted mutation still does. Explicit authorization may rebuild
an expired saved Web session once before rereading memberships, preserving its target.
Do not apply this Web-session classification to the separately verified OAuth usage token.
The MFA enroll/disable endpoints can explicitly reject a POST with HTTP 401 and
`detail: User must re-authenticate to enroll/disable a factor`. Classify that
as `reauthentication_required`, with no uncertain mutation effect. Preserve the
ordinary Web session and allow explicit force-login followed by a fresh attempt.
Do not replay the mutation automatically or generalize this to unknown 401/403 replies,
transport failures, or a failed read after an accepted mutation.

Remote mutations are never automatically retried. Persist running intent before submission;
prepare TOTP and activation are separate calls with the candidate persisted in between.
A cancelled, lost or restarted mutation becomes uncertain. The independent `uncertain`
flag survives unrelated successful reads; `login_paused` survives logout and recovery scans.
`MaintenanceState::blocks_login` is shared by explicit queues, enrollment and automatic recovery.
One process owns a job until settlement; stale revisions cannot publish late results.

Only successful acknowledged activation installs its candidate. Unknown security data must
replace stale flags with unknown. TOTP disabled requires consistent false flags and no factor,
including factors lacking a usable ID. Ordinary edits preserve pending/uncertain state.
The disable endpoint may return message-only JSON without `success`. After a valid
acknowledgement, always reread security state before confirming disablement; never resend
the mutation to compensate for an unfamiliar acknowledgement. Negative/HTML responses or
an unconfirmed state retain uncertainty. Activation still requires its explicit success.
Explicit reconciliation archives pending enrollment material before clearing the restriction.
Keep at most ten private material versions. Logout-all pauses automatic reauthentication;
logout-and-relogin persists phase one then queues the existing worker as phase two.
All manual authorization actions clear prior enrollment and account-menu automatic-push
intent before execution. Preserve the imported egress through `clear_enrollment` and
use the captured route for the current operation; successful acquisition waits for
explicit push. Ordinary account-management enrollment keeps its automatic import path.
Desktop and mobile list login buttons use the existing queue. Maintenance authorize,
force-login and switch-workspace persist their action in the existing JSONB entry and
wait for the same worker and concurrency gate. Preserve force-login and explicit workspace
intent through persistence; call the corresponding maintenance provider action on execution.
Capture the original route owner before changing workspace selection, recheck its identity
on claim, and resolve its current configured egress. A known route owner permits choosing
another workspace without adopting that workspace's proxy. Unknown routes keep existing
ambiguity checks. Queries and security operations retain immediate shared-gate execution.
Queued authorization success requires a credential; failure must finish its attempt and
release capacity. Cancellation clears queued intent and fences late results, but does not
disable automatic recovery. A later eligible automatic recovery is a separate task.
Confirmation freezes the selected workspace and revision; successful acquisition still
waits for explicit push.
The logout-all endpoint's successful JSON `null` acknowledges acceptance; allow it only
for that endpoint. It does not prove immediate global revocation. Keep the delay notice,
clear the locally retained session, and preserve rejection of HTML/redirect/negative replies.

Frontend confirmations freeze the displayed revision and target; do not reread a new revision
and silently apply old intent to it. Material and revision must come from the same response.
Watch modal identity as separate Vue watch sources; polling a new object for the same ID must
not reset the selected tab. Include the requested initial tab as a watch source: Vue may
update visibility before that prop in the same parent render. Maintenance history uses
operation-result labels, not login-success labels for device or security reads. Keep non-login
activity in last_activity and the credential-producing attempt in last_attempt; persist both in
existing history in the same transaction. Cancellation/restart finishes the correct activity,
and subsequent push updates the original authorization attempt. Material edits follow the same
rule. Successful authorization records the verified credential's workspace in that attempt,
including when no workspace was selected before login. Table and card layouts retain current
pool-health, retry and relogin-count projections.
Relogin account, pool and push-target views expose `planTypeDisplay` from the same provider
mapping as account management, while retaining raw `planType` for identity and authorization.
Desktop/mobile badges and plan filters use those labels; aliases share one filter option.
Workspace-directory choices already carry normalized plan categories; do not reinterpret
those categories as raw provider plan wire values.
Header filters use options inside their own popover, avoiding nested teleported select dismissal.
Close/account change invalidates pending reads and TOTP timers;
remote writes may complete and notify the list but never repopulate a closed dialog.

### 4. Validation & Error Matrix

- Unknown fields/action, missing revision -> reject before provider call.
- Changed revision, same-account activity or pause -> conflict, no provider call.
- Global concurrency full -> authorization queues; immediate maintenance returns conflict.
- Uncertain security effect / pending enrollment -> reads allowed, reauthorization blocked.
- 302/HTML/negative logout acknowledgement -> uncertain, no success claim.
- Missing directory target -> distinct diagnostic; explicit 402 disabled workspace -> terminal
  workspace-unavailable. Never classify the former solely as confirmed deactivation.
- Remote password change / extra email or browser challenge -> manual official verification;
  no placeholder action that claims to implement automatic password changes.

### 5. Good / Base / Bad Cases

- Good: valid Web session -> new OAuth code exchange -> verified candidate -> explicit push.
- Base: legacy row -> no session -> password/TOTP login -> verified Web snapshot and OAuth.
- Bad: activation response lost -> inspect security -> ordinary edit -> automatic retry.
  The uncertain flag and candidate must still block that retry.

### 6. Required Tests

Python offline fixtures cover fresh authorization with reuse, strict session parsing,
verified cookie snapshots, membership denial and alias deduplication, MFA factors, 402,
TOTP three-state flags, and negative logout acknowledgements.
Rust tests cover shared gate, cancellation, stale revision, identity mismatch, durable
uncertainty, private material resolution/history and logout pause. API tests cover auth
and strict DTOs. Frontend tests cover RFC TOTP vectors, frozen confirmations, delayed
responses and same-account polling. Browser previews must use synthetic data and isolated
mock APIs; real maintenance and production acceptance require separate authorization.

### 7. Wrong vs Correct

Wrong: refresh the revision after the user confirms, then submit; use HTTP 200 as proof
that the new password/TOTP works; treat a Web session check as new OAuth credentials.
Correct: submit the confirmed revision; persist and evaluate action-specific outcomes;
make the OAuth code exchange explicit and preserve unknown states until reconciled.
