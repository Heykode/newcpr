# Request Location Contract

- Reuse `openai_location_override_enabled`; default remains false. Disabled means
  no proxy or runtime location projection.
- Persist optional global location in the existing request-tuning JSON. Freeze it
  through SnapshotSettingsFacts, RuntimeSnapshot, RoutingPlan and AttemptContext.
  Do not load mutable global configuration midway through a request.
- Proxy location is metadata on the saved proxy, loaded with account facts.
  Omitted proxy updates preserve it, explicit null clears it. Location-only edits
  must leave account credentials, account revisions, enabled state, scheduling
  and URLs untouched. The proxy/config revision still advances.
- Keep the existing startup-location encoding for identity/affinity compatibility.
  After selecting the lease, rewrite only the outbound body with proxy > runtime
  > startup location. Do not recalculate the identity seed or clear continuation.
- Test actual HTTP and WS payloads, unmarked cache keys, disabled overrides,
  configuration round trips and PostgreSQL account-query projections.
- No capacity freezing or adaptive concurrency belongs to this change.
