# Scheduled Quality Checks

- `AccountProbe::quality_check` is separate from diagnostic `probe`. It MUST use
  the persistent ordinary coordinator with its fixed-account quality selection. Never
  reuse `start_diagnostic`, synthesize a user API key, or call upstream HTTP directly.
- Provider transport, model-specific Excel policy, credentials, fingerprint,
  egress, concurrency and availability remain authoritative. Only explicitly configured
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
- PostgreSQL owns global ten-job admission, five-minute renewable leases and
  rule revision fences. Five-second worker checks cancel changed/deleted rules;
  updates atomically invalidate old results. Do not schedule from browser polling.
- A round has 1–8 concurrent answer/judge samples. Cancellation waits for all active
  requests to finalize and never publishes a partial cancelled round as a verdict.
- One rule per account; enabled is required for scheduled and manual runs. Saving
  never queues an immediate run. Reference: ranxi2001/sub2api f80611d6; unlike its
  per-scan worker cap, CPR keeps a database-wide admission bound.
- Quality-owned scheduling pauses have a separate owner marker. Fixed-account quality
  tests may ignore only that verified pause, never credentials, quotas, cooldown,
  model access, concurrency or request interval. Revalidate ownership after lease
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
