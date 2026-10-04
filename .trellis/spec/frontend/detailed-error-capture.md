# Detailed Error Capture Controls

- Reuse the settings modal next to usage/error details. Keep request lists lightweight;
  only opened error details fetch capture bodies. No separate management menu is needed.
- `quotaPolicy` is optional in old API configurations; render missing as `stop`. Present
  stop-at-capacity versus overwrite-oldest without changing enabled/globalErrors/media.
  Display indexed logical byte/count snapshots, not guessed disk allocation or zeros on failure.
- `retentionDays` accepts `0`, which means clear all finished capture material at or before the
  selected cutoff; running capture tasks and their active material remain protected.
- A capture-only clear command requires explicit second confirmation. Explain permanent
  material deletion, preserved usage/errors/accounts, and a fixed cutoff excluding new records.
- Reuse one server-selected cutoff through bounded clear batches. Prevent duplicate mutation
  while saving/clearing, allow stopping after the current batch, and stop on unmount. Never
  replay ambiguous failed deletes automatically or claim complete success after a partial error.
- Clearing neither saves draft configuration nor toggles capture. Refresh authoritative
  counts/status afterward; retain an explicit read/cleanup error and preserve ordinary logs.
- API has no path/table/instance selector. Browser fixtures must check confirmation cancel,
  rejected writes, no automatic retries, stable cutoff, unchanged switch and 1440/390/320px.
