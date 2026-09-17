# Upstream Integration and Release Guide

## Scope and Authority

Use this guide when comparing CPR with the official upstream tree, integrating
an upstream fix, reviewing dependency drift, preparing a PR, or preparing a
release.

- Each task must pin its own official reference in its PRD before editing.
  The v3.4.1 integration used `5205c07d`; the v3.5 integration uses
  `b98c2f01a572da42531b34b4bc57299caabf702e`. Do not use a moving branch,
  an unmerged feature branch, or a Dependabot branch as a compatibility baseline.
- Preserve local QX contracts: smart sticky scheduling, account continuity,
  outbound proxy and IPv6 control, device identity, outbound user agent,
  cumulative account cost, non-streaming routing, and runtime settings.
- Integrate behavior by narrow, reviewable changes. Do not copy the upstream
  `AGENTS.md`, `.agents/`, agent definitions, or GitHub workflows into this
  repository.
- Do not import XAI/Grok-specific behavior, unrelated PR work, or a branch that
  has not entered the fixed official baseline.

## Evidence Before Conclusions

Every review or integration report must identify the exact refs it examined:

1. Record the target branch, the fixed upstream SHA, the current `HEAD`, and
   the merge base with `main`.
2. Review the complete PR range, not only the latest commit:
   `git diff "$(git merge-base main HEAD)" HEAD`.
3. Re-check `HEAD` immediately before reporting a review result. If it changed,
   refresh the diff and state that the conclusion applies to the new SHA.
4. Inspect the worktree and index separately. Uncommitted or parallel-session
   changes are not evidence of the committed PR range and must not be silently
   included or reverted.
5. For dependency comparisons, compare both manifests and lockfiles. A
   matching lockfile does not prove that a new direct dependency or feature is
   present, and a lockfile-only change does not prove a behavior change.

Record validation as `passed`, `failed`, `skipped`, or `not run`, together with
the exact command, commit SHA, relevant environment, and a short result.
Skipping because a service or environment variable is unavailable is not a
passing integration test. A build or health check alone is not proof of the
end-to-end business behavior.

Combined sources need a fresh workspace-wide compile and App architecture check:
a clean three-way patch does not verify renamed session fields, port signatures
or test module layout. Test fixtures must also match the real initialized
provider: terminal-only SSE is not evidence of a canonical completed response.
Preserve matching response-created/terminal events and actual completion
assertions rather than weakening them to make a transplanted fixture pass.

## Security and Data Handling

- Never place passwords, API keys, OAuth tokens, cookies, proxy credentials,
  signing material, real account identifiers, or complete request/response
  dumps in commits, PR text, screenshots, logs, or task research.
- Use synthetic fixtures or redacted excerpts. Mask hostnames, user IDs,
  authorization values, request IDs, and proxy endpoints unless the value is
  explicitly public and needed to reproduce the issue.
- Capture only the log and configuration fragment needed to locate a problem.
  Do not collect real upstream traffic merely to improve a report.
- Integration tests that exercise PostgreSQL, Redis, proxying, or provider
  transport must use isolated test resources and sanitized configuration.
  Never use production databases or real provider credentials.

## PR Review and Authorization Gates

The PR target is `main`. Keep one PR focused on one coherent goal and include
the final behavior, design trade-offs, risks, and actual validation evidence.
For UI changes, include an actual preview or browser evidence for the covered
states; a successful build is not visual acceptance.

The following actions are separate authorizations:

- Local editing and local verification are limited to the approved task scope.
- Creating a commit requires the task's explicit commit authorization.
- Pushing a branch requires explicit push authorization.
- Merging into `main` requires explicit merge authorization after maintainer
  approval and resolved review comments.
- Creating a version commit, tag, GitHub Release, or dispatching release
  automation requires explicit release authorization.
- Deploying or restarting a running instance requires explicit deployment
  authorization and is not implied by a successful release.

Do not describe a local check as approval, a pushed branch as merged, or a
generated artifact as a deployed instance. Do not add CI jobs, service
matrices, or new automation merely to increase confidence; reuse the existing
checks because this project does not authorize extra CI quota.

## Upstream Change Compatibility

Before porting an upstream change:

- Locate the owning module, its callers, the nearest existing implementation,
  and the relevant protocol or database contract.
- Preserve request transport selection, account-bound proxy credentials,
  device and UA projections, and downstream versus upstream streaming
  semantics.
- For request decompression, test supported encodings, malformed input,
  stacked encodings, multi-member or multi-frame input, and the post-decode
  size bound. Reject unsafe combinations before JSON parsing.
- For header isolation, test both HTTP and WebSocket requests. Strip
  downstream proxy, Cloudflare, compression-negotiation, and tracing headers
  while retaining protocol-required and intentional business headers.
- For quota and budget display changes, retain cumulative usage, budget
  enforcement, local plan-name mappings, and reset-time semantics. Do not
  claim a performance improvement without measurements.
- For a migration, append a new migration and preserve all applied migration
  bytes. Verify old data semantics, especially cumulative cost and budget
  records, before accepting the change.

### Stability, Diagnostics, and Forecast Integration

- Serialize the full configuration refresh from reading through publication,
  while keeping request-path snapshot reads independent of that lock.
- A cancelled terminal write must be resumable by the existing session cleanup.
  Verify cancellation during finalization, not only before a request begins;
  budget settlement and admission release must each happen exactly once.
- Zero-attempt failures still need a request record and safe diagnostics.
  Observation queue overflow or database failure must not block delivery or
  invent an upstream attempt.
- Preserve each failed request's own upstream ID. A WebSocket connection's
  opening ID must not be substituted for a later failed event's ID. Chat,
  Responses, Compact, images and search retain the same public correlation
  contract without changing their transport or account selection.
- Forecasts are read-only, on-demand reports derived from paired quota
  observations and local usage in the same window. Do not add model calls,
  per-account polling, or a second `cost / percent` estimate. Keep unknown,
  discontinuous, partial and extrapolated samples explicit.
- Confirm prewarm semantics from the provider request, not a client-supplied
  metadata label or zero output tokens. The inference view, health counts,
  immutable cumulative ledger and API-key budgets have different purposes;
  test their interactions before changing a shared usage predicate.
- Central request notifications must support explicitly silent background
  refreshes. Preserve pending-operation ownership across page refreshes and
  account switches, and redact exported diagnostics independently of display.

## Migration, Backup, and Version Notes

Before applying a database migration:

1. Identify the current schema and migration freeze digest.
2. Create a verified logical backup with the repository's PostgreSQL backup
   path, and record where the sanitized backup verification occurred.
3. Test restore or at least checksum and artifact readability before changing
   the live schema. Do not delete, rewrite, or renumber an applied migration.
4. Update the migration freeze list and compatibility documentation in the
   same change when the schema changes.
5. Confirm that upgrade and rollback preserve cumulative account costs,
   budgets, provider credentials, proxy bindings, and scheduling state.

Release notes are part of the release input, not an after-the-fact summary.
Following the release process introduced by upstream commit `9b19cf08`:

- `release/notes.md` must be tracked, non-empty, and start with the exact target
  heading `# vX.Y.Z`.
- Notes describe only behavior actually present in the tagged source, in
  Chinese, with upgrade and migration impact stated when applicable. Do not
  list unmerged branches, unverified fixes, or unsupported performance claims.
- Before formal publishing, verify the version file, existing tags, complete
  commit range, clean tracked worktree and index, branch/upstream relationship,
  and remote tag availability.
- `release/publish` creates the version commit and annotated tag, atomically
  pushes them, and dispatches the existing release workflow. It is not a
  dry-run command. Run it only after explicit release authorization.
- Follow the release workflow for quality checks and artifact verification.
  A successful workflow means the release pipeline completed; it does not
  mean a runtime instance was upgraded.

## Release Platform Policy

- `release/platforms.yaml` is the only active platform list. The approved default
  is Linux AMD64 (`x86_64-unknown-linux-gnu`, `linux/amd64`).
- Keep cross-platform source/build support; add other release targets only after
  explicit confirmation. Restoration examples live in `deploy/README.md`.
- The optional non-Docker job checks `has_non_docker` before matrix expansion.
  Packaging permits `skipped` only when no such target was requested; required
  jobs must succeed. Publication explicitly checks all direct dependencies.
- Preserve frontend, backend, security, checksum, image verification and signing
  gates. Reuse the existing workflow-lint job for platform regressions.
- Run `ruby release/tests/platforms_test.rb` and actionlint. Tests cover the actual
  metadata/packaging scripts and conditional gates, but do not replace a future
  authorized GitHub release run or native execution tests for restored targets.
- Do not move historical tags or replace historical assets to change platforms.
- The pinned artifact downloader flattens a single matched artifact, including
  pattern downloads. Packaging must accept this root layout only when exactly
  one platform is configured, while retaining named directories for multiple
  platforms. Test both download layouts, not only pre-created named fixtures.

## Minimum Verification Checklist

- [ ] Exact base, upstream, head, merge base, and complete diff recorded.
- [ ] Worktree and index checked; parallel changes identified separately.
- [ ] Relevant Rust, frontend, migration, container, and security checks
      recorded with command, SHA, environment, and result.
- [ ] Integration tests use isolated services and contain no sensitive data.
- [ ] Migration backup, restore/readability evidence, and freeze digest are
      recorded when schema changes are involved.
- [ ] PR target is `main`; review evidence matches the current head.
- [ ] Push, merge, release, tag, and deployment actions each have their own
      authorization.
