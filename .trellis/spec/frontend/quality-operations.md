# Quality Operations UI

- Keep a standalone sidebar route, compact overview, rule list, history table and
  editing/detail side panels. Reuse CPR controls and themes.
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
- Browser coverage must include creating through real dropdowns (not just editing
  text), pause, queue deduplication, delete, failed history refresh, both themes,
  and 1440/390/320px bounds. Preview samples are not live model results.
- Render narrow-screen history as individual records with time, verdict and counts;
  do not force sideways scrolling for core results. Paused rule status is neutral,
  regardless of the previous verdict. Set screenshot animations to disabled so
  theme-transition frames are not mistaken for settled colors.
