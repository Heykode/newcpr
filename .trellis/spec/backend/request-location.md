# Request Location Contract

- Align official `fdd124e7`: `openai_location_override_enabled` controls only the
  global location and defaults false. Proxy locations remain independent.
- Persist optional global location in the existing request-tuning JSON. Freeze it
  through SnapshotSettingsFacts, RuntimeSnapshot, RoutingPlan and AttemptContext.
  Do not load mutable global configuration midway through a request.
- Proxy location is metadata on the saved proxy, loaded with account facts.
  Match the official account builder: changing or removing the proxy clears its
  previous location; an account accepts a location only after a proxy is bound.
  Omitted proxy updates preserve it, explicit null clears it. Location-only edits
  must leave account credentials, account revisions, enabled state, scheduling
  and URLs untouched. The proxy/config revision still advances.
- Both observation and pre-selection encoding pass no location. After selecting
  the lease, use proxy > enabled global location; absent both, preserve the client
  payload. Never fall back to the wire-profile startup location. Keep the original
  request for affinity/cache identity and apply the official helper only to the
  outbound copy. Do not clear continuation or change account/device identity.
- Retain existing JSON field names for stored-settings compatibility. Compile
  the enabled switch into an optional frozen global location. Legacy null global
  values use the official RequestLocation default, not startup configuration.
- The admin editor uses one global switch. Disabled saves keep the last saved
  location, ignoring incomplete unsaved drafts. Successful explicit detection
  fills the draft and enables global overlay; saving remains required. Normalize
  enabled form values; validate raw backend input before trimming city/region.
- Test actual HTTP and WS payloads, unmarked cache keys, disabled overrides,
  configuration round trips and PostgreSQL account-query projections.
- Keep location validation tests in `gateway-core/tests/account/location.rs`;
  production source files must not contain inline test modules.
- No capacity freezing or adaptive concurrency belongs to this change.
- A complete valid `environment_context` XML user input can be aligned when
  classification is absent. If `content_item_kinds` is an array, each exact
  content index is authoritative; null/unknown/missing slots stay unchanged.
  Do not filter entries before indexing or rewrite all parts because one matches.
  Keep quoted XML, malformed XML and unrelated messages unchanged. Handle user
  environment_context and developer codex_apps_client_time_context; replace only
  direct timezone fields. Preserve historical dates, nested fields and metadata.

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
  Proxy locations are independent of the global switch; preserve the identity seed.
  A test response refreshes the open editor's revision before the next save.
  Connection tests use the saved automatic-mode setting without saving the draft.
  Explicit `detectLocation:true` forces one-shot lookup even when auto mode is off.
  The form's location action fills only the manual draft; saving remains explicit.
  New URL connectivity tests do not look up location; the separate location action
  probes the new URL without changing a saved proxy or account binding.
