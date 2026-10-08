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
- Start a durable capture-only job through `/log-cleanup/captures/start`. Reuse the existing
  worker, shared job exclusion and server-selected cutoff. Closing the modal or leaving the
  page only stops status reads, never the job. Remount reloads authoritative progress.
- Prevent duplicate mutations while accepting/cancelling; allow modal closure during a running
  job. Cancellation uses its persisted job ID. Never retry an ambiguous POST automatically or
  claim complete success after a partial error. This command never saves draft cleanup settings.
- Clearing neither saves draft configuration nor toggles capture. Refresh authoritative
  counts/status afterward; retain an explicit read/cleanup error and preserve ordinary logs.
- API has no path/table/instance selector. Browser fixtures must check confirmation cancel,
  rejected writes, no automatic retries, stable cutoff, unchanged switch and 1440/390/320px.

- Capture JSONL adds bounded `diagnostic.evidence` rows and `captureEnd.omissionReasons`.
  Display/export these through the existing lazy text reader; no automatic body fetch on lists.
