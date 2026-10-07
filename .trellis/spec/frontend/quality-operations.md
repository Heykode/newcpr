# Quality Operations UI

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
- Account More -> quality check links by stable accountId; select an existing rule
  or open a preselected editor. Pagination and missing list items never imply deletion.
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
  templates for the selected action, and disallow automatic reverse restoration.
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
  normal rounds reset the abnormal count, unknown/errors hold both. Changing frequency
  and native recovery are separately opted in. Do not add an account-edit shortcut;
  reuse quality operations and monitoring templates with existing bulk application.
- Browser regressions cover partial success/retry, failed refresh, fresh unrelated
  values, empty field opt-ins on reopen and 1440/390/320px layouts. Interact with the
  visible labels of shared checkbox/switch controls and assert their checked state.
