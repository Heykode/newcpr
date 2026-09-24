# Account Relogin Contracts

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
  `retryIntervalMinutes` (default 5, range 1..1440), added by migration 0034.
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
- Login proxy propagation is distinct from managed account device/IPv6/UA state.
  The Python runner does not implement the pool's IPv6 source-binding policy.
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
No raw membership, login session or credential enters this DTO or error metadata.
The JSONB entry defaults `workspace_choices` to empty and `selected_workspace_id`
to None; waiting survives restart without holding an exchange or retrying.
Resume queues a fresh manual login under the gate using the observed revision and
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
  snapshot per batch. Stale/deleted templates fail before any push. Only create-only
  imports receive settings/proxy; existing account rotation never applies them.
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
  Admin helper: trim, blank to None, max 128 Unicode scalar values, reject control
  characters. HTTP validation and Store mutations use the same rule.
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
