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
  automatic attempt budgeting. Cookie writes must not reset the three-attempt limit.
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
- Confirmed push clears failure cooldown, per-generation attempts and attempted target.
  Failed recovery keeps bounded backoff and at most three attempts per authentication
  generation. A new binding can recover immediately; Cookie writes cannot reset it.
  Preserve a JSONB-defaulted rolling history of automatic starts across success and
  reimport: at most three starts in fifteen minutes, including across new bindings.
- One shared eligibility projection drives the worker and the safe list `recovery`
  field. Expose waiting, cooldown/retry time, pause, missing material, workspace
  ambiguity, retry limits and loop protection without secrets. An uncertain push
  remains blocked; manually obtained valid credentials await explicit push.
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
  Conflicting plans for one ID, unknown plans and equal top-tier workspaces fail closed.
- During OAuth account selection, the authentication transaction `session_id` is not
  a selectable login session. Use `unified_sessions` or an explicit selected-session
  field; never use a transaction ID to resolve missing or ambiguous login choices.
- The runtime venv removes its seeded setuptools after installing worker dependencies.
  Keep `pip check`, isolated imports, a public TOTP test vector and browser-session
  initialization in the image build; none of these checks should make login requests.
  Do not suppress container vulnerability gates to retain unused build dependencies.

## Account Menu and Import Templates

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
tests and existing device-registry regressions. Run `relogin_worker_test.py` offline.
Verify both new push and original in-place rotation paths. Never use real credentials
as fixtures. Deployment and live login acceptance require separate authorization.
On 2026-09-15 one explicitly authorized Free account passed the live Python worker
through token exchange and usage verification. This does not validate live pool
push, automatic recovery, Team/Business workspaces or deployed runtime behavior.
