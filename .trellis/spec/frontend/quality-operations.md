# Quality Operations UI

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
- Scheduling presets serialize to the existing five-field Cron contract. The
  default stays `0 */6 * * *`, evaluated in the chosen timezone; daily time is
  minute-precise. Preserve arbitrary existing Cron expressions in advanced mode
  without rewriting them on open. Repetitions are 1–8 parallel answers per round,
  bounded by normal account concurrency and interval. Disabled rules cannot trigger
  manually and saving a rule does not immediately run it.
- Failed batch creation preserves successes, leaves only failed accounts selected,
  and retries without duplicating completed rules. Failure-action groups are separate
  from the judge group and may include disabled groups. Optional auto_restore is off
  by default. Show persisted action/recovery outcomes, not inferred status changes.
- Detection mode is a dropdown: question or state probe. Probe mode hides judge,
  question, effort and sample-count fields without deleting the question draft.
  Serialize one probe round and no effort; do not require a judge group to save it.
- Render probe normal/suspect/inconclusive labels from the persisted run mode,
  with transport/status/length evidence and a clear heuristic limitation. Never
  render raw State or Cookie. Cancelled or paused states remain neutral.
- `enable_excel` is available in both modes and disables auto-restore. Display the
  persisted pause/skip reason; switching Excel off does not silently resume a rule.
- Browser regressions cover mode switching, preserved question drafts, judge-free
  saves, Excel action, paused results and 1440/390/320px probe panels.
