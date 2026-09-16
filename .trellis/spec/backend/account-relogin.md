# Account Relogin Contracts

- `gateway-admin/model/relogin` owns login-library facts; provider-owned JSON is opaque.
  Do not add passwords or TOTP material to managed credential documents.
- `ReloginStore::save_batch` must be atomic. Per-row writes and deletes use revisions.
  Never derive `Debug` for secret-bearing records or return them as API DTOs.
- Manual login only caches verified credentials. Explicit push confirms the library
  revision. Automatic recovery requires enabled OAuth, expired credential state, a
  matching terminal expiry reason, and an enabled email-matched library entry.
- OpenAI `expired` plus `access_token_expired` with a refresh token is reserved for
  bounded token-refresh recovery, not password relogin. It remains blocked from
  inference even when the stored access-token expiry is in the future or missing.
  The refresh worker respects `next_refresh_at`; explicit revocation, invalid grant
  or exhausted recovery uses terminal `credential_expired`. Do not rewrite tokens
  from an inference request's stale snapshot.
- Existing account recovery captures account ID, principal, workspace and credential
  revision. Recheck these at push; automatic push also rechecks recovery eligibility.
  Use the existing prepared rotation and CAS transaction, never delete/recreate.
- Successful prepared credential replacement updates authentication facts even for
  manually paused accounts, but preserves `enabled`. A concurrent pause must not
  be undone by a relogin push. Ordinary file import is different: its explicit
  scheduling settings remain authoritative.
- New push uses the existing import preparation and initialization with create-only
  semantics. The PostgreSQL transaction checks email/identity under the configuration
  lock, and the final INSERT conflict clause forbids updating an existing identity.
  Normal account imports retain their original upsert behavior.
- Persist `Pushing` before pool mutation. A crash or ambiguous failure becomes
  `Uncertain`, not an automatic retry. Library and pool commits have distinct owners;
  no distributed transaction or exactly-once claim is made.
- Manual and automatic login share bounded concurrency. Publish in-memory occupancy
  only after all fallible claim writes complete; otherwise a partial claim failure
  can permanently fill the queue with jobs that were never started.
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
  a locked workspace. Conflicting plans for one ID fail closed; team/business are aliases.
- During OAuth account selection, the authentication transaction `session_id` is not
  a selectable login session. Use `unified_sessions` or an explicit selected-session
  field; never use a transaction ID to resolve missing or ambiguous login choices.
- The runtime venv removes its seeded setuptools after installing worker dependencies.
  Keep `pip check`, isolated imports, a public TOTP test vector and browser-session
  initialization in the image build; none of these checks should make login requests.
  Do not suppress container vulnerability gates to retain unused build dependencies.

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

## Verification

Run the relogin model/service tests, API auth/wire tests, isolated PostgreSQL relogin
tests and existing device-registry regressions. Run `relogin_worker_test.py` offline.
Verify both new push and original in-place rotation paths. Never use real credentials
as fixtures. Deployment and live login acceptance require separate authorization.
On 2026-09-15 one explicitly authorized Free account passed the live Python worker
through token exchange and usage verification. This does not validate live pool
push, automatic recovery, Team/Business workspaces or deployed runtime behavior.
