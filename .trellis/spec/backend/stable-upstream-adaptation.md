# Stable Upstream Contracts

## 1. Scope

Approved stable fixes from upstream v3.13.0, adapted to local main without importing
experimental State or alternate release-channel behavior. A separately approved
follow-up adapts only the bounded interval wait and diagnostics from pinned #196.

## 2. Signatures

- `normalize_reasoning_replay(&mut Map<String, Value>)`, selected OAuth path.
- `AccountsService::recover`: disabled account uses enabled-only batch update.
- `QuotaForecastSample::cycle_usage`: current-cycle totals, distinct from `usage`.
- Admin `UsageListRecord::provider_account_custom_name` -> API `accountCustomName`.
- `POST /api/admin/system/update` -> 202 accepted operation.
- Update status includes `needRestart`; Host owns `UpdateOperation`.

## 3. Contracts

Reasoning normalization is after account scoping and before transport selection,
not a change to affinity inputs. Remove top-level status only on reasoning items;
remove nonempty content arrays only when nonblank encrypted content survives.
Stream invalid_prompt is request-scoped. No capacity accumulation is introduced.

Recovery of disabled accounts changes enabled only and publishes the committed
revision. Do not reset credentials, State or scheduling fields. Forecast cycle
usage survives recent-sample changes; reset boundaries restart accumulation. Local
current USD estimates and Plan learning remain separate from token forecasting.

Current account labels use the existing `custom_name` column, not upstream's
absent `notes` column. These are account-ID joins, not rewritten history or email
joins. Deleted accounts return null. Sensitive metadata remains detail-only.

Update tasks own the shared in-process and file locks until terminal persistence.
Persist before emitting terminal events. Disconnects do not cancel the task;
shutdown/drop records failure. Failed POST transport is uncertain, not permission
to replay. Keep local repository, stable/preview policy and verified deployment.

## 4. Validation Matrix

- Reasoning without encrypted history -> keep plaintext.
- Non-reasoning and nested fields -> unchanged.
- Disabled invalid account recovered -> enabled but still error.
- Failed enable write -> no facts notification or snapshot publication.
- Quota reset -> cycle baseline reset, including observations at 100%.
- Same email on distinct internal IDs -> independent notes.
- Accepted update -> running, not successful/restart-ready.
- Live operation lock -> never classify as orphan; completed -> needRestart.

## 5. Cases

Good: HTTP/WS preserve the original identity and normalize only outbound history.
Base: ordinary enable preserves actual account health.
Bad: use recent consumption as all past usage, or claim 202 means installed.

## 6. Tests

Exercise HTTP/WS compatibility, original field ordering, current identity tests,
recover failure/publication, actual PostgreSQL note joins, forecast resets and
unknown coverage, Host disconnect/shutdown/persistence, frontend status recovery.

## 7. Wrong vs Correct

Do not copy upstream whole files to acquire one fix. The local OAuth-only account
model, USD estimates and update channels differ. Preserve those owners and adapt
the smallest behavior. Cache-only clients already receive account/key-scoped stable
session IDs; #196 must not replace them or add Grok-specific headers.

## 8. Billing Snapshots and Key Names

CalculatedCost carries an optional shared breakdown through CostEstimate. Store
owns versioned JSON encoding and validation; migration 0033 is append-only.
Preserve authoritative totals, provider-reported precedence, retry reset, immutable
finalization, cumulative accounting and Key budgets. Never reconstruct a historic
long-context flag from token counts or today's rates. Saved snapshots are preferred
only for calculated costs with matching total/currency; malformed metadata falls
back to the original total. Full #159 price editing, multipliers and sync are excluded.

Key labels use nullable ID-based joins in both usage and ops sources. Rename and
delete must not affect row counts, historical ownership or expose secret Key values.
The yellow detail trigger uses the saved pricing flag and existing theme tokens.

## 9. Bounded Cache Diagnostics

Only enabled trace capture computes request field fingerprints. The budget is
64 KiB across fields, 16 KiB per field, 2048 nodes, depth 64, first eight input items.
Check string and arbitrary-precision number lengths before serialization to bound work as well as allocation;
mark omitted fields explicitly, never present prefix hashes as complete hashes.
Retain no raw prompts. Preserve terminal cached/cache-write counters separately
from truncated metadata, with missing distinct from zero. Existing event size
limits and disabled diagnostic behavior remain unchanged.
