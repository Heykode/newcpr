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
- Keep location validation tests in `gateway-core/tests/account/location.rs`;
  production source files must not contain inline test modules.
- No capacity freezing or adaptive concurrency belongs to this change.

## Opt-in Automatic Proxy Location

- Migration 0038 adds default-off `auto_location`, separately stored
  `detected_location_json`, and the latest location outcome. Manual
  `request_location_json` remains independently editable/preserved.
- Detect only on explicit enable, actual connection change while enabled, or
  an administrator-triggered test. Reuse the four probe slots, proxy and CA
  builder; no inference-path lookup, background warmup or direct fallback.
- Query the measured address, validate the returned address and full location,
  reject mismatching IPv4/IPv6 timezones, and bound lookup time/body size.
- Connectivity and geography have separate outcomes. Retain detection after
  connectivity failure or same-exit lookup failure; clear on a known changed
  exit plus lookup failure, dual-stack conflict, or connection/mode change.
- A missing address family is not evidence that its exit changed. A failed
  lookup retains the original detection and timestamp when every observed
  address still matches; any new/changed observed address invalidates it.
- Auto test writes are proxy-revision fenced and advance that revision;
  publish the global revision only when effective location changes.
  Lock settings before proxy rows and explicitly finish stale-test rollback.
- All provider-account projections use the same effective proxy location.
  Preserve the existing total switch, global fallback and identity seed.
  A test response refreshes the open editor's revision before the next save.
  With a blank connection draft, changed automatic-mode settings must be saved
  before a saved-record test; never silently test the old mode as the new draft.
