# Excel settings presentation

## Scope

`views/settings/index.vue` keeps one runtime form and save action. Common/Codex and Excel scopes use `v-show` so switching does not reset drafts. Backup routing is unchanged.

- Common retries (`websocketMaxRetries`), account switches, routing attempts and cooldown stay in `RuntimeSettingsCard`, even when Excel also consumes them. Do not duplicate or introduce Excel-only copies.
- `ExcelSettingsCard` owns only Excel models and image controls. Account/template policy stays account-scoped.
- `useExcelImageSettings` handles display units and invalid drafts, not backend behavior. MiB converts to existing integer byte fields without rounding existing values. Empty or invalid drafts block saves; loading saved state clears drafts.
- Keep native-attachment image limits visible; only relay-specific resources are conditional. Inherited transport remains `null`, not an implicit mode change.
- Use existing controls and the same settings API; never alter scheduling, fingerprints, egress, limits or defaults as a UI cleanup.

## Reference Capacity Bounds

- #204 changes same-meaning configurable maxima, not defaults or saved values:
  per-image/total 512 MiB, count 65536, storage 262144 MiB, entries 1048576,
  TTL 10080 minutes. Admin and provider clamps must match the UI exactly.
- Preserve relational checks and byte precision. CPR-specific relay request/download
  bounds remain 512/128; they are not the reference project's shared ingress budget.
- Boundary tests must use each new maximum plus one, not the superseded limits.

## Verification

Run `tests/runtime-settings.test.mjs`, the full Node test suite, type checking and ESLint. `tests/browser/excel-settings.mjs` exercises the real settings page at desktop/mobile sizes in light/dark themes using synthetic API data. All external/API network requests are blocked; screenshots belong outside the repository. Unrelated admin cards are stubbed explicitly, not counted as verified by this runner.
