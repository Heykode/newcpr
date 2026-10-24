# Quality Operations UI

## Account-List Template Application

- Selecting a monitoring template from the account-list menu immediately freezes the
  selected account IDs, reads current rule revisions and applies the template. Do not
  add a second confirmation drawer. Partial failures use a bounded toast summary and
  never auto-repeat an uncertain mutation.

## Account Menu Switch And Result Badge

### 1. Scope / Trigger
Desktop account rows and mobile cards share quality switching and monitoring results.

### 2. Signatures
- `useAccountQuality` reads `/quality-ops/monitoring` then calls
  `setQualityEnabled({id,revision,enabled})` using the fresh revision.
- `AccountQualitySetup` creates from a selected monitoring template using
  `{id,revision,enable:true,targets:[{accountId,ruleId:null,revision:null}]}`.
- `AccountQualityMonitorBadge` shares verdict/action labels and semantic colors from
  `components/quality-ops/presentation.ts` with the full quality page.

### 3. Contracts
- More menu orders Test Connection first, Quality Switch second, with current on/off.
  Capture requested intent before reading; synchronous per-account guards coalesce clicks.
  Refresh real state after success/failure; do not optimistically claim a successful write.
- An absent rule opens explicit template selection or manual configuration. No automatic
  template choice. Refresh clears prior selection/catalog; read failure cannot leave stale
  choices available. Cancel/close/unmount abort reads. Unknown writes require state review,
  not automatic replay. The drawer blocks close while a mutation is running.
- Monitoring lifecycle color and latest verdict color are independent. State probe correct
  is 智商正常 / text-cp-success, incorrect is 智商异常 / text-cp-error. Reuse existing
  quality-page labels/colors for other states; absent verdict means waiting for first check.
- Account badges show lifecycle and verdict only. Do not add restoration success,
  blocked/released summaries or their timestamps to this compact label/tooltip.
  Restoration details stay in quality history through the shared link. Optional
  detectionMode tolerates an older API. Override inherited whitespace-nowrap and
  ignore swipe-selection gestures.

### 4. Validation & Error Matrix
- Failed monitoring read -> no mutation; CAS conflict -> error plus fresh account state.
- Failed/empty template catalog -> retry/manual setup, no stale submission.
- Unknown application outcome -> disable repeat submit until close and status review.
- Correct result with blocked/released recovery -> show the verdict, never infer restore.

### 5. Good / Base / Bad Cases
- Good: green normal result beside paused monitoring with a link to detailed history.
- Base: no quality rule means no badge; existing account health and Excel labels remain.
- Bad: infer restoration from a correct verdict or overwrite config from an old account row.

### 6. Tests Required
Node tests cover colors, intent/CAS, dedup, disposal and deletion option handling.
Browser account-quality-controls covers template failure after success, cancellation,
first enable, switches/conflicts, single/batch checkbox resets, both themes and 1440/390/320px.
Retain quality-template/group, Excel and mobile-card browser regressions.

### 7. Wrong vs Correct
Wrong: use one color for lifecycle and verdict, or post the entire stale rule to toggle it.
Correct: independently colored verdict text and an enabled-only CAS write after rereading.

## Account Batch Quality Switch

- Selected accounts expose one `质量检测` dropdown with `批量开启` and `批量关闭`
  on desktop and mobile. `AccountQualityBatchMenu` emits the desired boolean;
  `useAccountQuality.setBatchEnabled(accountIds, enabled)` owns the operation.
- Freeze and deduplicate 1–1000 selected IDs before awaiting; include off-page IDs.
  Read monitoring once, then send only `{id,revision,enabled}` for rules whose state
  differs. Keep all detection settings/history and selection; never invert each rule
  or replace configuration through the template API. Missing rules are skipped,
  with a short template-setup hint when enabling, without automatic creation.
- Bound writes to four in flight. Block duplicate batches and single quality switches
  while pending; disable template application and bulk editing/deletion in the page.
  Abort reads on disposal and stop queued writes, without cancelling submitted writes.
- A failed read sends no writes. Conflicts/unknown write results stay unconfirmed and
  are counted separately; other targets continue. Summarize confirmed target states,
  missing rules and failures. Do not automatically replay writes. A manual retry rereads
  versions and skips rules already in the requested state.
- Refresh real account state once after the operation; a failed refresh must not
  relabel confirmed writes as failures. Do not trigger the run-now endpoint.
- Cover mixed states, selection changes, off-page targets, deduplication, CAS errors,
  missing rules, read/refresh failure, guards, concurrency and disposal in Node tests.
  Extend `browser/account-quality-controls.mjs` with real toolbar interactions,
  partial failures and light/dark 1440/390/320px menu bounds and mobile card updates.

## Editable Model Suggestions

### Scope / Trigger
Test/judge fields in single-account, template, group and bulk editors.

### Signatures
QualityModelPicker accepts `v-model:string`, `scope:QualityModelScope`, emits
`choices:QualityModelChoice[]` and consumes `/quality-ops/models` pages.

### Contracts
Searchable editable combobox. Preserve saved/manual values absent from catalogs.
Scope changes clear suggestions, not input. Label known exact-account efforts while
retaining unknown/custom choices; never inject an effort. Abort and generation-fence reads
on close, scope changes, refresh and unmount. Load additional pages explicitly.
Retry/load-more/refresh return focus to the input before replacing their buttons;
otherwise focusout can close the picker and abort its own request.

### Validation & Error Matrix
Failure -> retry with retained input; empty success -> no known candidates;
stale response -> ignored. One aggregate page is not the complete catalog.

### Good / Base / Bad
Good: manually enter a new model. Base: unchecked bulk fields stay unchanged.
Bad: clear input on empty results or label aggregate capability as account-specific.

### Tests Required
Keyboard selection/manual input, failed read/retry, pagination, stale results, template/
group/bulk integration, and 1440/390/320px synthetic layouts without upstream traffic.

### Wrong vs Correct
Wrong: enforce predefined selection. Correct: suggestions assist the existing string field.

## Group Enrollment And Deletion

- Keep account-monitoring, group-rule and template views distinct. Reuse the existing
  editor for group scopes, with independent tested-group and judge-group pickers.
  UI `all` maps to wire empty group; display it as selected, not an absent selection.
- Group saving returns committed parent state and separate created/updated/failed
  counters. Never claim rollback or replay the POST automatically after uncertainty.
- Batch delete rereads rule versions before confirmation, freezes those targets,
  removes confirmed successes from selection, and preserves failures for fresh review.
- Account More -> quality switch uses the account controls contract above. Its first-use
  manual setup links by stable accountId to a preselected editor. Pagination and missing
  list items never imply deletion.
- Browser fixtures cover group tabs, status filters, pause/resume, retained children,
  failed reads, partial deletion and account links at 1440/390/320px.

## Monitoring Rule Templates

- Keep separate account-monitoring and rule-template tabs. Reuse the rule editor
  with a template name and no account picker; do not duplicate prompts or defaults.
  Existing rules can be saved as templates without changing the rule.
- Account management's monitoring menu captures selected IDs across pages, then
  reads `/quality-ops/monitoring` before explicit confirmation. Show new/replaced
  counts, preserve histories, and submit exact rule/template revisions.
- Partial success keeps completed targets; review/retry only failures with fresh
  versions and another confirmation. An uncertain write response is never replayed
  automatically. Catalog/read errors prevent submission and have retry controls.
- `qualityMonitoring` comes from the existing account list refresh. Status precedence:
  disabled -> monitoring paused, running -> detecting, pending -> queued, otherwise
  monitoring. No rule means no badge. The badge links by accountId/ruleId to history.
  Preserve Excel historical warnings and ordinary account status independently.
- Template editing/deletion is not live propagation. Label source snapshots as
  source templates, distinctly from failure-remediation account templates.
- Cancel/fence reads on close/unmount, keep mutation baselines isolated from polling,
  and never execute probes in the browser/save handler. Applying an enabled rule
  for the first time or enabling a disabled rule makes it immediately due for the
  existing background workers; template catalog edits alone do not run probes.
- Regressions: `quality-monitoring.test.mjs`, `browser/quality-templates.mjs`, and
  existing `browser/quality-ops.mjs`; verify 1440/390/320px, legacy rules, partial
  outcomes, cancelled confirmations, failed reads, uncertain writes and deep links.

## Detection Rules

- New remediation uses `apply_account_template` and the shared versioned
  `AccountTemplatePicker`, with a full-settings summary. Show `enable_excel` only
  when editing that legacy action; never silently convert a saved rule.
- Single/bulk edits reuse the existing threshold. `failureTemplate` is opt-in in
  batch patches: preserve per-rule selections when unchecked, reject missing
  templates for the selected action. Full-template autoRestore requires explicit opt-in
  and restores only a still-owned before snapshot; do not infer it from Excel-only recovery.
- Keep historical template names/revisions from `QualityRun.config`, not today's
  template catalog. A changed/deleted selection remains explicit until reselected;
  catalog failure has retry and aborts on unmount.
- Template `excelIgnoreEncryptedContent` is an explicit lossy option, default
  false; serialize false when the template explicitly turns Excel off. Disabling
  inclusion of Excel settings omits the entire optional group. Reuse existing
  IPv6 fields and preserve omission versus null.

- Keep a standalone sidebar route, compact overview, rule list, history table and
  editing/detail side panels. Reuse CPR controls and themes.
- New question rules and synthetic preview share `quality-ops/presets.ts`: the exact
  candy question, reference `21` and Chinese judge prompt from Sub2API `bf782999`,
  with the user's explicit no-search/tool instruction prefixed to the question.
  Preserve saved/custom text when editing; do not migrate or overwrite existing rules.
- Do not place existing `BaseSelect` inside native modal `dialog`: its body-
  teleported listbox is below the browser top layer and cannot be selected.
  Use a conventional overlay with focus handling and shared body scroll locks.
- One `FormItem` provides one control ID. Put search inputs outside the form item
  of a select, otherwise duplicate IDs break accessible names.
- Editing takes a copied config/revision. Polling cannot replace the edit baseline.
  Mutations have a synchronous local guard; stale revisions are rejected server-side.
- Abort and fence list/detail/catalog reads on replacement/unmount. Preserve
  existing history on read failure and mark it failed rather than resetting to zero.
- Render answers and judge explanations as escaped text, never raw HTML/Markdown.
  Keep requested and returned model separate. Capture the round's original prompt
  and reference so later rule edits do not misrepresent past scores.
- Browser coverage must include creating through real selectors (not just editing
  text), pause, queue deduplication, delete, failed history refresh, both themes,
  and 1440/390/320px bounds. Preview samples are not live model results.
- Render narrow-screen history as individual records with time, verdict and counts;
  do not force sideways scrolling for core results. Paused rule status is neutral,
  regardless of the previous verdict. Set screenshot animations to disabled so
  theme-transition frames are not mistaken for settled colors.
- Rule account and judge-group pickers load immediately into bounded, scrollable
  lists. Account creation supports multiple checkboxes (one rule per account), while
  judge group remains single-choice. Search is optional; pagination must allow selecting beyond
  the first 50 records. Fence stale searches and preserve the selected item across
  filters. Load, failure/retry, and a successful empty result are distinct states.
- Account and enabled judge-group catalogs have independent requests/errors;
  searching or failing one must not clear or cancel the other. Editing a rule
  keeps its account immutable. Do not place a whole picker inside one FormItem.
- New rules/templates use integer `intervalSeconds`, default 120, range 5–31536000,
  with a seconds input instead of Cron/timezone controls. Summaries and selective
  batch edits use the same field. First activation is due immediately; ordinary
  edits and subsequent rounds measure the next due time from save or completion.
  Five-second worker scans and capacity may delay actual start.
  Missing/null interval preserves legacy Cron/timezone, including on unrelated edits
  and saving a rule as a template. Only an explicit switch enables seconds scheduling;
  spreading defaults must not silently convert legacy configs. Repetitions are 1–8
  parallel answers per round, exempt from business concurrency but respecting the
  account request interval and safety gates. Paused schedules still allow
  one manual check; disable the button only during mutation, pending or running.
  A manual check must not save or enable the rule. Show queued/running status while
  active and paused again on completion; saving a rule does not immediately run it.
- Failed batch creation preserves successes, leaves only failed accounts selected,
  and retries without duplicating completed rules. Failure-action groups are separate
  from the judge group and may include disabled groups. Optional auto_restore is off
  by default. Show persisted action/recovery outcomes, not inferred status changes.
- New rules/templates/groups share `quality-ops/defaults.ts`: state probe,
  `gpt-6-astra`, one round, 120 seconds, two abnormal rounds before remediation
  and two normal rounds before native restoration. Keep failure action `none`
  until the administrator explicitly chooses an action/template. Existing drafts
  use legacy-aware normalization; absent fields never adopt new defaults.
- Detection mode is a dropdown: question or state probe. Probe mode hides judge,
  question, effort and sample-count fields without deleting the question draft.
  Serialize one probe round and no effort; do not require a judge group to save it.
- Render probe normal/suspect/inconclusive labels from the persisted run mode,
  with transport/status/length evidence and a clear heuristic limitation. Never
  render raw State or Cookie. Cancelled or paused states remain neutral.
- Legacy `enable_excel` is available in both modes and disables whole-config restore.
  State-probe template/Excel actions offer `disableExcelOnNativeRecovery`, checked
  in new drafts only; saved configs missing it remain false. Normalize it off for
  answer or unrelated actions. The enabled option shows `excelRecoveryThreshold`.
  Mirror it through templates, groups and selective bulk patches. Explain native
  probing continues after Excel, and recovery closes only this rule's unchanged
  owned route, never the rest of its template or manual/403 suspension.
  Retain historical pause labels; new Excel activation does not pause the rule.
- Browser regressions cover mode switching, preserved question drafts, judge-free
  saves, Excel action, paused results and 1440/390/320px probe panels.

## Selective Batch Editing

- Rule selection survives search changes; select-all affects only current results.
  Opening the bulk editor clears every field opt-in. Never patch account identity.
- Read fresh rules before each batch and save sequentially with each rule's current
  revision. Preserve unselected values, skip unchanged rules, and never blindly
  retry a revision conflict. Refresh failure means no writes.
- Keep successful and failed outcomes distinct. Remove successes from pending
  selection so retries only save unfinished rules. Stopping or unmounting prevents
  subsequent saves but does not claim an in-flight mutation was cancelled.
- Question-only fields do not overwrite probe drafts. Group removal and Excel
  thresholds apply only to the matching final action. Excel never enables automatic
  restore even when that field is selected in a mixed batch.
- New Excel failure/recovery threshold editors default to 2 and accept 1–100 rounds;
  legacy configs missing either field retain 1. Selective patches change only opted-in
  fields. Show reset-on-save semantics: abnormal rounds reset the healthy count,
  normal rounds reset the abnormal count, other unknown/errors hold both. Explicit
  overload resets healthy progress and uses the fixed-two-round rule below. Changing frequency
  and native recovery are separately opted in. Do not add an account-edit shortcut;
  reuse quality operations and monitoring templates with existing bulk application.
- Browser regressions cover partial success/retry, failed refresh, fresh unrelated
  values, empty field opt-ins on reopen and 1440/390/320px layouts. Interact with the
  visible labels of shared checkbox/switch controls and assert their checked state.


## Explicit Overload Presentation

- Shared account/template/group editors and the bulk editor explain the fixed-two-round
  overload exception separately from the configurable pure-incorrect threshold.
  Overload/overload and overload/incorrect (either order) trigger; a healthy round clears
  progress; other unknown/request errors hold it. Quota, generic 503, authentication and
  independent judge errors do not qualify.
- Wire `QualityAnswer.verdict` includes `overloaded`. Aggregate `overloaded` displays
  `上游过载（待确认）` in warning color; promoted aggregate `incorrect` displays the
  mode's abnormal label. Raw sample overload displays `上游服务器过载`, preserving its
  reason even when the aggregate is already abnormal. Do not mislabel it as a wrong answer.
- The server owns counters and actions. No frontend counting, synthetic promotion or
  automatic configuration changes. Read old runs with their original mode snapshots.
- `browser/quality-ops.mjs` covers both modes and both pending/promoted states at
  1440/390/320px. Require the child fixture's ready output so an occupied port cannot
  accidentally test another checkout. Synthetic fixtures never contact upstream models.
