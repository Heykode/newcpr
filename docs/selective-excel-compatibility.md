# Selective Excel compatibility and account refresh

This adaptation covers Sub2API PRs #229, #231/#232 and #195. It does not
introduce #234 IP-401 tolerance, #250 feature search, or new settings.

## Message IDs

Only Excel's outgoing ordinary-message copy omits an optional ID matching
`item_` followed by exactly 24 ASCII hexadecimal characters. Native/unknown
IDs, tool call identities, message content and canonical/replay history stay
intact. Agent message IDs remain part of their existing attribution context.
The transform runs before the attribution normalizer's no-metadata fast path.
Native Codex HTTP/WS preparation does not call this Excel transform.

## Attachment diagnostics

Upload failures retain their original error variant, HTTP status, response
evidence and retry ownership. The existing `excel.transport.failed` trace adds
fixed attachment preparation, transport, HTTP, response or capacity labels,
and `generation_started=false`. Invalid JSON, invalid file IDs and oversized
responses have separate causes. No error payload, URL, credential, file ID or
image contents are copied into these diagnostic facts. Generation failures
continue using the existing general transport diagnostics.

Image-limit errors now explicitly identify the local gateway configuration.
This changes the explanation, not image admission limits or failure handling.

## Account row refresh

Quota refresh and account recovery pass the complete server-returned account
and the displayed row captured before the operation. The page wrapper must
forward both to the query composable while preserving forecast invalidation.

A row can be replaced without a list request only when its original snapshot,
accepted query parameters, list membership, summary status and supported sort
keys remain unchanged. Replacement invalidates older reads, leaves other rows
intact and preserves the current page/summary. Changed filters, identity,
groups, status, dynamic sorting or a concurrent row refresh require a silent
authoritative reread. Existing last-page fallback still applies. Partial switch
patches retain their original reread behavior. Failed/superseded reads must not
infer selection removal, and disposed pages ignore late mutations.

## Regression coverage

- Existing Excel history/image test modules cover exact IDs, unchanged content,
  original history, native wire isolation, upload HTTP errors, malformed upload
  responses, sanitized traces and no generation/fallback after upload failure.
- `frontend/tests/accounts-refresh.test.mjs` covers row replacement, caller
  snapshots, cancellation, stale results, filtering, ordering and pagination.
- `frontend/tests/browser/account-row-refresh.mjs` drives the actual account
  page at desktop and narrow widths, checks request counts and retained row DOM,
  and verifies the expanded quota panel remains open.
- Existing provider contracts, architecture checks and account Excel browser
  regressions remain part of verification. Synthetic regressions do not prove
  real-upstream latency or cache-hit improvements.
