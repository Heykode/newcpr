# Recent Update Migration Contracts

## Target And Scope

NewCPR is the only implementation and release target. Retired QX is a read-only
behavioral reference, not a merge parent. Preserve NewCPR's account device
identities, account egress, native TLS, Excel transport, managed state, quality
jobs, cost accounting, log cleanup and existing template catalog.

## Affinity And Recovery

- Affinity is integrated into the existing selector, not a parallel scheduler.
  Explicit required accounts and native continuation ownership take precedence.
- `openaiAccountAffinity` defaults to `strict`. Strict children follow the root
  binding; `relaxed` allows an independent child binding without rewriting the
  root. Neither mode permits transferring native encrypted state to another
  account or bypassing model permissions, quota, exclusions or concurrency.
- Guardian parent preference supplements the existing top-level
  `openaiGuardianReservedConcurrency`; do not introduce a duplicate tuning field.
  The existing default remains zero.
- Turn aliases point to session keys, not account IDs. Bindings include a
  generation token. Renewal must match both account and generation and must not
  recreate expired state. Late completions cannot overwrite a newer binding.
- Redis accepts old account-only bindings. Settings validate binding TTL from
  1 through 720 hours, default 24. Omitted update fields preserve stored settings.
- Honor numeric and HTTP-date Retry-After, including zero, within the existing
  delivery, account-switch, cancellation and retry budgets. Flex rejection is
  terminal for the request and does not penalize the account.
- Do not force `store=false`, broadly delete history IDs, or replace NewCPR's
  request-normalization and identity-stripping contracts with legacy behavior.

## Refresh And Live

- Migration 0063 changes only the previous 3600-second refresh margin to 300;
  other configured values remain. Historical migration bytes stay frozen.
- Background refresh adds a stable per-account offset within the configured
  margin. Request-time refresh retains the configured margin. Quota observation
  jitter is separate from token refresh and account eligibility.
- Refresh-endpoint 401 and explicit terminal RT errors invalidate the credential;
  429, 5xx and transport failures retain bounded transient recovery. Do not apply
  the refresh classification to ordinary model or quota endpoint responses.
- Live bootstrap uses the existing authorization, account scope and admission
  path. Sideband/hangup retain the call owner and current account transport.
  Unsupported Realtime capabilities return explicit errors; they are not
  silently routed through ordinary Responses or Excel.

## Data And Administration

- Fast group policy is `default`, `enabled` or `disabled`. Disabled wins across
  groups; global disable wins. Keep legacy `disableFast` synchronized and accept
  old requests. Explicit new-mode updates take precedence; omission preserves.
- Import templates reuse the existing catalog and freeze the selected revision.
  Selected templates apply to both inserted and updated accounts. Omission keeps
  the original import semantics; no extra overwrite checkbox is required.
  New first-step prefill clients send `templateSettingsOverride:true` with both
  selection and final settings. Validate the template revision but do not overlay
  its original config onto edited fields. Final group/proxy references are validated
  by the ordinary import path. Derive the internal import proxy mode from the final
  explicit proxy/clear choice; otherwise preserve. Absent/false flag keeps legacy
  template application and the old idempotency fingerprint. The legacy State-only
  template field retains existing handling without reintroducing the retired UI.
- Keep database/wire `raw_upstream_error` / `rawUpstreamError`. Protected error
  details may contain a source chain. Core accepts legacy raw details as fallback;
  bounded queues count both fields and ordinary logs must not emit raw bodies.
- Admin renewal uses an absolute expiry, password-fingerprint revocation and
  atomic Redis comparison. Old sessions without the new expiry are not extended.
  Frontend login/logout/renewal cannot overwrite a newer cookie or request state.
- Duplicate or late budget settlement must not move the active budget window.
  Zero-attempt failures belong in the error view, not successful usage totals.
- Do not add quota-exhaustion prechecks to quality jobs in this migration.

## Verification

Run full workspace tests with isolated PostgreSQL and Redis, not silent store
skips. Cover old/new binding formats, stale generations, strict/relaxed roots,
Guardian reservation, Fast legacy updates, refresh classification, Live ownership,
budget settlement, administrator renewal and import-template revision conflicts.
Keep the existing identity, Excel, managed-state and retention regressions.
Frontend validation includes lint, typecheck, build, tests and desktop/mobile UI.
Local mocks do not establish production risk-control or upstream acceptance.
