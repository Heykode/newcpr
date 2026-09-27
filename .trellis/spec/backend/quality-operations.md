# Scheduled Quality Checks

- `AccountProbe::quality_check` is separate from diagnostic `probe`. It MUST use
  the persistent ordinary coordinator with `Scheduled(Some(account))`. Never
  reuse `start_diagnostic`, synthesize a user API key, or call upstream HTTP directly.
- Provider transport, model-specific Excel policy, credentials, fingerprint,
  egress, concurrency and availability remain authoritative. Never change an
  account's scheduling state based on a quality score.
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
- PostgreSQL owns global two-job admission, five-minute renewable leases and
  rule revision fences. Five-second worker checks cancel changed/deleted rules;
  updates atomically invalidate old results. Do not schedule from browser polling.
- Runs preserve their configuration snapshot. List queries omit answers; details
  are authenticated and no-store. Retain seven days / 200 runs per rule.
- Regression coverage lives in Core execution, Admin parsing/Cron, API auth,
  Store lease/fence/retention tests and frontend `tests/browser/quality-ops.mjs`.
