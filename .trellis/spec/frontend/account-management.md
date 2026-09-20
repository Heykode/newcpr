# Account Management Contracts

## Background Imports and Independent Table Preferences

- Browser AT/RT and JSON imports submit server-owned tasks. Preserve JSON document
  boundaries, existing settings/proxy semantics and the synchronous OAuth path.
  Generate submission UUIDs through crypto.getRandomValues on non-secure HTTP too.
  Retry an unchanged uncertain submission with the same ID, never automatically
  retry an unknown credential exchange. Editing the input invalidates the ID.
- Recover task summaries from the server on mount; cancel only progress reads
  when closing/disposal, not accepted work. Stop skips pending items only.
  Refresh accounts when imported-account counts advance; fence stale detail
  responses by selected task and cancellation. Never persist tokens in browser
  task preferences or include them in task results.
- The import settings step exposes the default-off State switch for OpenAI and
  mixed batches. Send `turnStateInjectionEnabled` only for OpenAI documents and
  new OpenAI OAuth; reauthorization must still omit all import settings.
- Task history is process-local, actor-scoped, bounded to 100 retained batches,
  and expires one hour after completion; restart loses it. OAuth authorization
  does not create an import task. Never present this as a durable account-import
  audit. Keep the recent-batch selector visible even with one result and distinguish
  an initial read failure from an authoritative empty list.
- Task detail defaults to all items, not attention-only when any item failed.
  Keep the explicit attention filter, creation/completion times and expandable
  saved account IDs. Returning to the account list is not a batch filter.
- Account-column preferences already exist and remain authoritative. Usage and
  error tables use separate storage keys, keep required identity/actions and error
  columns visible, tolerate invalid stored values and retain original layout.
  New columns follow their defaults, reset does not affect other tables.
- Verify API/composable lifecycle tests plus synthetic desktop/mobile browser
  interactions, including submission response loss, reopen, stop, unknown result,
  persisted columns, reset, keyboard focus and layout.

## 1. Scope / Trigger

Apply when changing account connection tests, account column preferences,
capacity, health colors, drag selection, creation-time sorting, group marks,
batch editing, account list refresh, quota reset countdowns, or IPv6 address
pagination.

## 2. Signatures

- `useAccountBatchEditor` owns five `update*` flags and computed `hasUpdates`.
- `useAccountsQuery` exposes `refreshAccounts(): Promise<boolean>` and a
  `refreshing` ref covering all pending account list requests.
- `AccountQuotaWindowView.reset_at: Option<DateTime<Utc>>` serializes as
  `resetAt: string | null`; TypeScript accepts `resetAt?: string | null` for old
  servers. The existing China-time `resetAtDisplay` string remains unchanged.
- `quotaWindowResetPresentation(window, now)` returns
  `{ display, pending, title } | null`; all quota views share this presenter
  and `useUiClock()` rather than separate per-account timers.
- `POST /api/admin/accounts/batch-update` sends `accountIds` plus only selected
  `enabled`, `concurrencyLimit`, `weight`, `groupIds` and `outboundProxyId`.
- `useAccountConnectionTest({ reload })` owns the modal's asynchronous lifetime.
- `POST /api/admin/accounts/connection-test` accepts a JSON body with `accountId`,
  `modelId`, `interface`, `stream`, and `prompt`; it returns diagnostic SSE events.
  The equivalent GET query remains for legacy clients only.
- Local keys: `cpr.accounts.connection-test-settings` and
  `cpr.accounts.visible-columns`.
- `readAccountColumnKeys(raw)` migrates `addedAtDisplay` to `addedAt`.
- `GET /api/admin/accounts` sorts by `addedAt` / `createdAt` and `asc` / `desc`.
- IPv6 updates submit `revision`, `defaultMode`, and the complete `addresses`.
- Account responses include `effectiveConcurrencyLimit: number`; the existing
  `concurrencyLimit: number | null` remains the editable override.
- Account tables opt into `BaseTable.columnLayout="content"` with `grow` weights
  on identity and usage columns. Other tables default to proportional sizing.
- `useAccountSwipeSelect` accepts DOM getters, ordered `rowIds`, `selectedIds`,
  `disabled` and `invalidationKey`; it exposes `onMouseDown`, `isDragging` and
  a viewport-clipped `overlayStyle`.
- `AccountStatusBadge.align` defaults to `left`; only the account-list caller
  opts into centered marks alongside the centered enable switch.
- `AccountSchedulingSwitch` distinguishes manual intent from authentication failure.
  An enabled account with backend `status=error` shows an off, read-only switch and
  an automatic-stop mark, retaining the error badge and recovery details. Existing
  account settings remain the explicit manual-pause control. Recovery restores the
  display only if manual enable intent remains true; it never sends a toggle API.
  Quota/rate-limit states do not masquerade as permanent authentication failure.
  Terminal reasons ignore leftover refresh-backoff timestamps.

## 3. Contracts

- Refresh the current account page and summary through `GET /api/admin/accounts`;
  retain filters, sort, page size, page and selected IDs. Neither the top-right
  manual refresh nor the 30-second timer triggers OAuth, models or upstream quota
  refresh. Use the existing query's non-silent error path for manual refresh.
- Track actual pending list requests, including diagnostic silent reloads.
  Disable and animate the manual button while requests are pending; manual
  clicks and timer ticks must not start another list request then. Preserve
  mutation/diagnostic reloads that intentionally supersede older data.
- Automatic refresh stays silent and disposes its one timer with the page.
  A manual failure retains existing data, displays the error and allows retry.
  Keep the refresh icon next to the heading without a fixed header height that
  overlaps wrapped descriptions on narrow screens.
- Batch editing is opt-in for enabled state, concurrency, weight, groups and
  outbound proxy. All five application checkboxes start unchecked on every
  open and reset after close. Place each checkbox at its field header's right
  edge; disable the associated input until selected, including keyboard access.
- Only validate and submit selected fields. Editing then unchecking a field
  must omit it even when its retained value is invalid. Empty selected groups
  send `[]`, selected empty concurrency sends `null`, and selected direct
  connection sends `outboundProxyId: ''`. Omitted fields preserve existing values.
- A selected proxy in `preserve` mode remains a no-op. Disable saving without
  any effective update and guard the submit function too. Single-account
  settings retain their existing controls and behavior without application
  checkboxes.
- Opting out while a proxy menu is open must also close its teleported options.
  Test keyboard focus plus Space, not only pointer clicks, which can hide the
  bug by triggering outside-click dismissal.
- Normalize local storage in the serializer before computed properties read it,
  including cross-tab updates. JSON `null`, scalars, arrays and malformed JSON
  must not crash the connection-test UI.
- Retain string prompts and boolean `false`. The current OpenAI test supports
  Responses only; migrate an old Completions preference to Responses. Do not
  advertise unsupported conversion as a working endpoint.
- Preferences are browser-origin scoped, not shared server-side administrator
  defaults. No database migration is implied by persistence.
- An old model request must not replace a newer modal's model list, error or
  loading state. Invalidate ownership on close, switch and unmount.
- A delayed test completion (including `withMinimumDuration` cleanup) must not
  close, fail or clear the next test. Check the captured generation in success,
  error and finally paths.
- Render validation failures visibly, even before any SSE event exists.
- Send diagnostic prompts in a POST body, never in the browser URL. Consume SSE
  with `eventsource-parser`, incremental UTF-8 decoding and an AbortController.
  Do not automatically reconnect or repeat a real diagnostic request.
- Accept only an event-stream response. HTTP errors, login HTML, invalid event
  JSON and EOF without a terminal event must not appear as successful tests.
- Keep six real five-minute health buckets. Match the reference timeline's
  2px gaps and 1px segment corners, without shadows or rounded capsule ends.
  Use successCount / (successCount + errorCount + nonCompletionCount): at least 95% emerald-500,
  at least 80% amber-400, otherwise red-500. Empty/pending stays slate-400.
  These thresholds supersede the old any-mixed-result-is-yellow rule.
  Exclude pending from the denominator; expose one ended-request count and success rate.
  Do not round a below-threshold rate up across that threshold in the label.
  Do not label outcomes as measured latency, or pending as blue maintenance.
  Keep one active bucket popover at a time and expose the status in its
  accessible name. Verify computed colors and both theme backgrounds, not
  merely the presence of utility classes.
- Health `nonCompletionCount` counts `cancelled` plus `incomplete`, matching
  account hotspot diagnostics within the same time/account scope and excluding
  recovered requests. It is disjoint from `errorCount` (failed only), and is
  not an exact count of empty text responses. Missing
  buckets expose zero; backend aggregation, recovery exclusion and final
  request account ownership are unchanged by frontend color thresholds.
- Quota summary rows fill their original available column width. Give each
  cycle a row with four aligned columns: period label, flexible progress bar,
  that cycle's used percentage and reset countdown. Bars consume the remaining
  track space; do not copy QX's fixed 32px bar or halve the containing column.
- Do not show an unlabeled highest-use percentage above the cycle rows.
  Render each known percentage even without local requests, retaining `0%`
  versus unknown `—`. Preserve the existing window costs and recent quota-group
  selection, but do not retain the old highest-use `cost / percent` estimate.
  The list shows `预计周额度：$50.00` using the backend current weekly estimate
  (`usage.quotaWindow.estimatedUsd`), or `预计周额度：—` when unavailable.
  `usage.quotaWindow` identifies the backend-selected account-wide source; never
  choose an unrelated percentage from the visible quota group. Both operands must
  be finite and positive, with no 5% threshold. Recompute on every row update.
  Keep the entry clickable for the weekly/monthly modal, whose USD values now
  use the same current-window calculation. Detail reads use the page cache
  (two in flight, 200 entries, ten-second TTL); the open modal rereads every
  thirty seconds. No automatic list prefetch.
  Hide expired source-window values; retain partial-cost warnings.
  A manual quota refresh invalidates its account and synchronizes the reset/plan
  signature before publishing updated row facts. Queue cancellation remains
  per consumer.
- Directory status is disabled-first: `enabled=false` projects `disabled`,
  displayed and filtered as "暂停". Reuse Core's existing `resolve_account_status`
  through the Store directory adapter; do not change scheduling or credential facts.
  SQL filtering, pagination, status sorting and summary counts use the same priority.
  The five status counts partition total; normal excludes paused accounts, but does
  not promise free concurrency slots or request-specific routing eligibility.
  Enabling reveals retained error/quota/cooldown facts instead of clearing them.
  Switch mutations immediately reread the current query and summary. Remove the
  toggled ID from selection only after an accepted reread excludes it; preserve
  other cross-page selections and keep selection on failed/superseded reads.
  The 30-second timer, Dashboard availability and group lifespan logic stay unchanged.
- Pair each quota reset countdown with its own window key and duration label,
  never the group's highest-use or representative window. Preserve per-window
  progress percentages and the existing recorded window costs. Forecasts are
  separate, read-only paired-sample estimates, never balances or billing facts;
  follow `../backend/quota-forecast.md` for missing and partial data.
- Compute countdowns only from timezone-qualified `resetAt`, not from
  `resetAtDisplay`. Retain the absolute China-time value in details/tooltips
  and identify its UTC+8 timezone. Missing/invalid raw timestamps produce no
  countdown, even when a legacy display string exists.
- A real quota window with a valid reset time still gets a summary countdown
  when `usedPercent` is null. Keep its percentage unknown, not zero; distinguish
  these quota windows from the presenter's local-only rolling statistics mode.
- Retain every reported quota cycle when all percentages and reset times are
  unknown, including after a refresh loses observations. Show `—` for missing
  values; never collapse the group to its first window. Only local-only
  rolling request statistics use the compact fallback.
- At or after reset time display `待确认`, including when used percentage is
  zero. Do not reset percentages, clear limits, enable scheduling or send
  upstream requests based on the browser clock. Local request statistics stay
  rolling windows without quota reset labels.
- Countdown text uses QX-style units with a space between components:
  `6d 7h`, `2h 10m`, `25m`, `<1m`. Summary, popover and details use the same
  formatting; precision and expiry semantics remain unchanged.
- A cleared table sort remains cleared in UI state. Apply the default newest
  first order in request construction, not by forcing UI state back to `desc`.
- Retain intentional hidden-column choices. Preference watchers must not assign
  a fresh equivalent array to the same watched ref on every invocation.
- Composite group colors against the current theme surface before choosing
  contrasting text. Truncate names in the summary only. Every nonempty group
  summary, including a single long name, has a focusable hover/click popover
  with a 100ms hover delay. Show all names and disabled markers without
  truncation inside the bounded, scrollable popover.
- Only account-list callers request `AccountGroupMarks layout="stacked"`:
  13px labels, centered wrapping within two rows, separate overflow count.
  Measure actual label and container widths to show fitting short names beyond
  the former two-group limit. Reserve room for the count when truncating long
  labels, and recalculate on resize or group changes without network requests.
  Keep the measurement strip hidden, noninteractive and clipped.
  Inline summaries elsewhere
  retain their default size. Group and last-used columns stay 184px and 112px;
  preserve local padding, narrow-screen scrolling and hidden-column choices.
- Account status and plan columns center the header label, mark and switch on
  the same column axis. The two sortable headers place their indicators beside
  the label without shifting the label itself. Keep this CSS scoped to the
  account table; shared sort buttons and status badges retain their defaults.
  Verify actual DOM centers in all three sort states and both themes.
- Dragging starts only on an account identity icon or blank cell space after
  5px vertical mouse movement. Text, buttons, labels, inputs, native touch,
  modifiers and scrollbars do not arm selection. Click alone does not toggle.
  Select/deselect mode comes from the starting row's initial state. Shrinking
  restores exited rows; Escape restores the initial selection; other-page
  selected IDs persist. There is no automatic page change.
- Use current DOM geometry and an ordered ID snapshot, never cached row nodes.
  Same-ID background refresh can continue; order/filter/page/column/expansion
  changes, disabled state or external selection edits end the gesture.
  Flush pending movement before mouseup; blur, resize and disposal clear all
  listeners and animation frames. Clip below sticky header cells and within
  the actual scroll viewport.
- Capacity text and colors use the same effective upper limit. Do not fill
  editing forms with the inherited value or send `effectiveConcurrencyLimit`
  back in mutation bodies. Unknown in-flight values remain visibly unknown.
  Only the left in-flight number uses load colors: zero/unknown is neutral,
  active is success-text, at least 75% is warning-text, and full/over capacity
  is error-text. The separator and effective limit remain secondary text.
  Zero stays neutral even with a zero limit; an unknown limit does not imply
  saturation. Verify actual theme colors, contrast and maximum-u32 geometry.
- Pagination slices IPv6 display rows only. Saving a later page must not remove
  addresses on other pages. Clamp the page after deleting its final row.
- A fixed-height `BaseTable` needs an explicit height override (`h-96!`), since
  its root also has `h-full`. Verify the rendered scroll viewport, not just the
  presence of the height class. On narrow account tables, disable horizontal
  column pinning before the pinned columns consume the entire content viewport.

## 4. Validation & Error Matrix

| Input / event | Required result |
| --- | --- |
| Manual refresh / 30-second tick while a list request is pending | No duplicate request; button remains busy |
| Manual refresh fails | Keep data and selection, show error, re-enable retry |
| Automatic refresh fails | Keep data without a toast; next refresh can proceed |
| Same reset instant expressed as UTC / UTC+8 / UTC-4 | Same countdown in every browser timezone |
| Reset timestamp missing, invalid or without timezone | No countdown; retain legacy absolute time in details |
| Positive remaining time under one minute | `<1m`, not pending |
| Reset time reached or passed, including zero usage | `待确认`; preserve all quota and scheduling values |
| Multiple windows with different reset times | Each label/bar keeps its own countdown |
| 5H exceeds 7D, or 7D exceeds 5H | Each row retains its own percentage; no mixed top-level percentage |
| No local requests but known quota percentages | Still show all cycle percentages |
| Open/reopen batch editing | Five unchecked application boxes; associated inputs disabled |
| Select only weight or proxy | Send that field and selected IDs only |
| Edit then uncheck any field | Omit the retained value and skip its validation |
| No application selected, or only proxy `preserve` | No update request |
| Keyboard opt-out with proxy menu open | Close the menu and disable the trigger; retain the unsubmitted value |
| Explicit empty groups / concurrency / direct proxy | Send `[]` / `null` / `''` respectively |
| Stored `null`, array, malformed JSON | Usable defaults before first render |
| Old Completions preference | Responses UI and Responses request |
| Empty / whitespace prompt | Visible error, no test request |
| Prompt over 4096 UTF-8 bytes | Visible error, no test request |
| Control character except CR/LF/TAB | Visible error, no test request |
| Unicode / `+ & ? #` / newline | Exact prompt after JSON body decoding |
| 4096-byte Chinese prompt behind an 8KB HTTP header limit | POST succeeds; no prompt in URL |
| Fragmented UTF-8 / multiline SSE / burst events | Preserve all content in order |
| Terminal event followed by stale events | Cancel stream and ignore trailing data |
| HTTP error / HTML / premature EOF | Visible failure; no automatic replay |
| Account A response after opening B | B's state remains unchanged |
| Old cancelled test finishes after B starts | B's connection remains active |
| Stored `addedAtDisplay` | Creation column remains visible as `addedAt` |
| Delete every row on last IPv6 page | Return to previous valid page |
| Cancelled / incomplete / failed health requests | First two count as non-completion; only failed counts as errors |
| One, two, or many long group names | Bounded summary and complete names on hover/focus/click |
| Global default changes, override absent | New query shows new effective limit; override stays null |
| Success rates 79.99%, 80%, 94.99%, 95% | Red, yellow, yellow, green |
| Drag shrinks or reverses | Exited rows recover their initial selection |
| Row IDs reorder while dragging | End immediately; never select through stale indices |
| Same IDs receive background data | Keep valid gesture, use refreshed DOM geometry |

## 5. Good / Base / Bad Cases

- Good: test a disabled account without toggling its production enable switch;
  successful and failed diagnostics still update that account's health.
- Base: omitted options retain `Responses`, `stream=true`, and the original
  default prompt.
- Bad: interpret a mock browser success as proof of real New API conversion or
  real upstream availability.
- Good: a used-up window whose reset time has passed still shows `100%` and
  `待确认` until the backend reports recovery. Bad: infer recovery from time
  passage or replace a missing timestamp with `now + windowSeconds`.
- Bad: write an inherited capacity back into the nullable account override.
- Good: drag a current-page range while retaining accounts selected on page 2.

## 6. Tests Required

- `account-batch-editor.test.mjs` executes the real composable and scheduling
  parser, covering each opt-in field, invalid unchecked values, reopen/reset,
  no-op rejection, explicit clearing and selections spanning multiple pages.
- Browser: verify each right-aligned application checkbox, disabled controls,
  request bodies and single-account compatibility on desktop and narrow screens.
- `node --test tests/*.test.mjs`: storage, byte limits, exact POST payload,
  async ownership, column migration, sorting, and refresh regressions.
- Browser: real Vue components, intercepted local APIs, both themes, desktop
  and 390/320px screens, health hover/click, column toggles, prompt persistence,
  pagination and complete IPv6 save payload.
- Run frontend typecheck, ESLint and build after the final source edits.
- Backend: POST authentication and JSON extraction, legacy GET compatibility,
  plus an isolated PostgreSQL run for disabled
  diagnostic health writes, revision/time fences and sorting before pagination.
  Tests returning early without a database are not persistence verification.
- Local HTTP transport tests are not production verification. Maximum-size
  prompts must remain in the POST body; do not raise production proxy limits
  or shorten user input to accommodate legacy URL encoding.
- `account-health-timeline.test.mjs` compiles and renders the real Vue SFC to
  check empty, pending, successful, mixed and failed buckets, without inventing
  history, plus the non-completion value in the popover. Browser checks cover
  active bucket ownership and full-width quota.
- Match health and hotspot queries in an isolated PostgreSQL test, including
  recovered requests, time boundaries, account scope and empty buckets.
  API tests must exercise list/detail presenters, not only hand-built DTOs.
- Group regressions cover one/two long names, overflow count, complete popover
  names and keyboard focus; verify narrow screens and both themes in-browser.
- `account-quota-countdown.test.mjs` covers time boundaries, offsets, legacy and
  invalid timestamps, immutable quota state, local statistics and real SFC
  summary/detail rendering, per-cycle percentages, unchanged recorded costs,
  and the clickable current-window weekly estimate. The backend calculates raw
  USD window costs * 100 / usedPercent; the frontend only formats
  `usage.quotaWindow.estimatedUsd` for a weekly period and live reset boundary.
  There is no 5% threshold or historical/Plan fallback. List data updates
  immediately; the modal uses the same backend calculation.
  Browser checks all four row columns for fit/alignment, advance the shared
  clock, preserve one list request per 30 seconds, and prohibit upstream/mutation calls.
- Backend account-route regressions exercise list/detail/quota/refresh
  presenters and serialization, preserving each nullable UTC timestamp,
  subsecond precision, legacy display text and expired limit state.
- `account-table-layout.test.mjs`: opt-in widths, hidden flexible columns,
  proportional fallback, centered status/plan columns and unchanged defaults.
- `account-status-alignment.test.mjs`: real SFC rendering for default/explicit
  left and centered marks, every status, pill behavior and unchanged popovers.
- `account-capacity-cell.test.mjs`: real SFC rendering at zero/unknown,
  75%/100% boundaries, effective-limit precedence, nullable override fallback,
  zero limit, maximum u32 and an always-neutral right-hand limit.
- `account-swipe-select.test.mjs`: range restoration, 5px threshold, controls,
  DOM replacement, scroll/autoscroll, clipping, ownership and full cleanup.
- Browser: 2560/1920/1440/390/320px, both themes, live selection and Escape,
  native text selection, pagination, same-ID refresh vs reordered IDs,
  hidden columns and group/health popovers. Local mock APIs are not production.

## 7. Wrong vs Correct

Wrong: default batch application flags to true or send a proxy change merely
because the proxy mode was edited, even after its application checkbox is cleared.

Correct: default all application flags to false and gate both validation and
payload fields on their flags. A weight-only update sends
`{ accountIds: ['acct-a', 'acct-b'], weight: 17 }` without scheduling/group/proxy defaults.

Wrong: unconditionally clear the shared active test in an old async `finally`.

Correct: capture the run generation and clean up only while it still owns the
current test.

Wrong: parse `2026-09-13 12:10:00` in the browser's local timezone or declare a
quota usable when its countdown reaches zero.

Correct: calculate from `2026-09-13T04:10:00Z`, display `待确认` after expiry and
wait for authoritative backend quota data.

Wrong: read a stale row index after sorting and toggle whichever account now
occupies it. Correct: capture ordered IDs, end on an order change, and use the
existing account selection Set as the only selection owner.

## 8. Relogin Table Selection

The relogin page reuses `useAccountSwipeSelect`. Keep its batch toolbar mounted,
with disabled controls when nothing is selected: inserting a toolbar during a drag
moves the row geometry and changes which rows the pointer reaches.
Polling must retain the same selection Set when no selected IDs disappeared.
Keep processing, credential verification and pool membership states separate.
The relogin table labels its cached JSON separately from current pool health.
Render `ready + synced` in the processing column only; never infer normal health from
membership or sync. Read safe `poolAccounts` from the same polled list response and
keep disabled and error filters orthogonal. Old servers without pool facts display
unknown, not normal. Uncertain pushes stay fenced even when the pool later recovers.
Keep newest material imports first using server `importedAt` order. Background changes
do not reorder entries. Display unknown import times honestly.
Bound long plan names and workspace IDs within their column; retain full values in
titles and the workspace picker. Known workspace choices are deduplicated; no free-text
ID editor. Text actions `重登` / `推送` have stable, non-wrapping widths. On viewports
below 1600px, the action column scrolls normally instead of covering other facts.
Push confirmation captures row revisions and must not silently switch to a newer
credential returned by background polling. Keep action failures independent of
successful list refreshes, so polling cannot erase a failed push message.

Regression runners: `frontend/tests/relogin.test.mjs` and
`frontend/tests/browser/relogin.mjs` (isolated fake API, desktop/mobile screenshots).

Both lists share `ReloginCountCell`, reading backend `reloginCount` and
`lastReloginAt`. Place the sortable account column after last use, and the relogin
column after pool membership. Zero is neutral; unresolved workspace counts are
`null` and render as unknown. Do not derive counts from button clicks, statuses,
attempts or cached credentials. Last-success tooltips use explicit UTC+8 time.
Account column storage uses `{ version: 2, keys }`: migrate legacy arrays once by
adding the new count column while preserving hidden existing columns. Respect
versioned choices, including an empty list, so hiding the new column persists.
Use `relogin-count.test.mjs` and `browser/relogin-count.mjs` for regression; the
opt-in `relogin-count-preview.mjs` disables the backend proxy and serves only fake,
read-only data.

The account directory may mark an avatar with a compact top-left key badge only when
the safe relogin list reports `hasTotp=true` for the same normalized email. The badge
must not resize the identity cell, expose secret material or imply that cached credentials
or the pool account are currently healthy. Relogin-list read failures retain the last safe
projection and do not fail the account directory.

The identity avatar uses an inset ring and bottom-right shield only when
`provider === 'openai' && turnStateInjectionEnabled === true`. Gold means opted in
without a ready model. Green requires an unexpired model in the authenticated
account list's `turnState.readyModels`; its tooltip names both ready and pending
models. Partial readiness must not claim that every model can be scheduled.
Disabled, credential-error and quota-exhausted account status overrides historical
ready or refreshing State metadata. The avatar uses the error tone and the panel
shows the shared blocked reason while retaining valid cached slots. Distinguish refreshable access
token expiry from revoked credentials requiring relogin. Recovery uses the next
ordinary account-list snapshot; no optimistic readiness or extra polling.
The server readiness projection checks global/account policy, enabled status, model list,
binding revision, plan shape, credentials/quota/cooldown and the one-minute cutoff.
Cached metadata remains visible when switches are off; `enabled:false` restores the ordinary avatar.
Use the shared UI clock to age the projection, not a per-row timer or probe.
Missing projection or global-off never renders green. Missing opt-in fields and
non-OpenAI providers retain the original identity tone.
Keep the provider icon, fixed 36/40px avatar, separate top-left 2FA badge and
`data-swipe-select-handle` behavior. The existing mutation/list refresh supplies
updates; failed mutations must not invent an enabled mark.
Use `account-state-indicator.test.mjs` for rendered on/off/legacy/provider/2FA
regressions and `browser/account-state-indicator.mjs` with the read-only preview
plus intercepted synthetic mutations for menu toggles and both themes at narrow
widths. These UI checks do not validate upstream State acceptance.

The expanded account row places a persistent `State 注入` summary below the
left quota area; do not rely on avatar hover for operational detail. For every
configured model, show the safe current/next slots, character counts, capture time,
attempt count, safe failure reason, refresh/readiness label and local expiry countdown. The header
summarizes ready models and total valid slots. Missing slots say `未获取`, while
disabled, globally unavailable and empty-model states remain distinct.
Show the successful attempt ordinal separately from total attempts. Prioritize both
counts before the capture time in narrow rows, with full detail in the title.
The existing account-list timer uses three seconds while visible enabled healthy
accounts have queued/refreshing State models, otherwise thirty seconds. Keep one
timer, prevent overlapping reads, and dispose it with the page; never probe upstream.
Use `turnState.models` when available and preserve `requiredModels` /
`readyModels` as a compatibility fallback for older servers. Countdown aging
uses the shared UI clock and never creates per-account timers. Desktop uses two
slot columns; narrow screens stack slots and constrain the panel to the viewport
even though the surrounding account table remains horizontally scrollable.
Keep the summary outside the scroll region. Model rows use a stable 80px desktop
height or 96px narrow-screen height; cap the list at three rows and scroll further
models internally. One/two-model lists retain their natural height. Overflowing
lists are named keyboard-focusable regions; long model names retain full titles.
Neither the panel nor its title attributes may contain the opaque State value.
There is no permanent spare placeholder. Show `待切换` only when a next slot exists;
off retains original countdowns, while cached slots do not imply scheduling readiness.
Cover the real SFC, legacy fallback, totals and countdowns in
`account-state-detail-panel.test.mjs`; the account browser regression expands a
real row and checks both themes at 1440/390/320px for clipping and overflow.

## Relogin Templates and Account Actions

- Keep the key badge independent from the account action's busy/blocked state.
  The menu uses a bounded current-page action query, immutable confirmation target
  and version, independent error state, and cancellation for reads only.
- A confirmed account-menu action queues login plus automatic settlement to the
  selected account. Show progress, block duplicate submissions, refresh account
  facts/counts on completion, and retain actionable failures. Do not implicitly
  enable scheduling or require the per-entry automatic-relogin switch.
- Template management reuses import scheduling/group/proxy controls. Persist on
  the server; copy editable group arrays and retain template revision for save/delete.
  Missing group references must remain visible and explicitly removable.
- New-account push confirms an optional template ID/revision plus unchanged row
  revisions. Default to no template on each open, show a readable config summary,
  and do not send a template for an existing-only batch. Mixed batches apply it
  only to new imports. Show request failures inside an open confirmation as well
  as after closure; polling must not erase them or adopt new versions.
- Verify CRUD, reload, cancellation, stale responses, no-template/mixed pushes and
  desktop/mobile layouts with synthetic endpoints only. Change themes through
  the actual theme control so generated CSS variables match the selected theme.

## 9. Group Monitor Overview

- Keep provisional-account counts off the cards. Existing help text explains
  peer fallback, shared dynamic capacity and mixed-lifespan expiry only.
  When occupancy is unknown, display neither a numeric numerator nor denominator.
  Concurrency progress uses the server's dynamic capacity, not configured limits.
- Keep total/normal in one compact left summary, with up to three monitor groups
  per page (two/one at narrower widths). Divide remaining width by the actual
  visible card count, including the last page. Default card height is about 128px.
  Do not restore the removed error/quota cards.
- Display a group when pinned OR `memberCount > 0`, regardless of enabled state,
  healthy-account count or traffic. A compact pin chooser includes empty groups.
  An all-empty catalog may read one existing group to initialize the viewer scope
  and restore pins, but must not keep displaying/polling an unpinned empty group.
- Pins are origin- and authenticated-viewer-scoped display preferences only.
  Latest pin goes first; toggling a pin returns to page one. Do not mutate group
  priority, membership, account selection or the account table's pagination.
- Query only visible IDs. Cancel older requests on page/visibility changes and
  scope disposal; guard publication by generation. Keep per-group sample times:
  a recent page-two reply must not make old page-one records look fresh.
- Monitor polling is independent at 10 seconds; account-list polling stays at
  30 seconds. One refresh icon in the last visible card's header controls the
  entire visible monitor page without increasing card height. Disable it while
  loading and guard repeated calls. Initial load, automatic polling and
  page/visibility changes read shared server snapshots without a refresh flag.
  Only manual refresh/retry sends `refreshForecasts=true` to trigger a global
  sample. Background sampling continues without a visible page; never request
  upstream refresh. A successful GET must retain the server sample timestamp,
  so repeated reads of an old snapshot still become stale after 45 seconds.
  Preserve page, pins and existing values on failure, without refreshing the
  account list or its separate quota cache.
- Unknown data and failed reads are not zero. Retain stale values with a warning;
  hide balances when their source reset expires. Idle rate never implies infinite ETA.
- `tests/group-monitor-preview.mjs` is an opt-in local-only fixture server using
  the actual overview component and theme. It disables the regular backend proxy
  and rejects unsupported API operations; never present its samples as live data.
