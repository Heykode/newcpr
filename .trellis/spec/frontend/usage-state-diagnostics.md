# Usage State Diagnostics

## 1. Scope / Trigger

Apply when changing the usage State column, its sensitive-detail lifetime or the
provider/store/API projection it consumes.

## 2. Signatures

- `UsageListRecord.turnState?: UsageTurnStateSummary | null`.
- `useUsageTurnState(requestId: () => string, visible: () => boolean)` returns
  reactive `value`, `loading`, `failed` and a `retry` command.
- `GET /api/admin/usage/records` returns the bounded `turnState` summary.
- `GET /api/admin/usage/records/detail?id=<request-id>` returns
  `metadata.turnState.injectedState` only under existing admin authentication.
- Storage is `model_requests.provider_observation_json`; list SQL projects
  `{turnState,summary}` rather than fetching the whole metadata object.

## 3. Contracts

- `UsageListRecord.turnState` is optional for compatibility with older servers
  and records. Render missing evidence as `未记录` and explicit non-injection as
  `—`; never infer the historic request's state from current account settings.
- The configurable `State 注入` column shows a bounded preview and character
  counts, using the existing `lg` (144px basis) column sizing and preference
  system. Do not size the table column for the full-value popover or change
  adjacent numeric columns to accommodate it.
- Different returned length uses a warning tone only. Same length must not be
  labelled valid, accepted or normal. Missing returned State displays `无回显`.
- Detail identifies the final attempt and HTTP header versus WS request frame.
  Exact-value equality is separate from length equality.

### Sensitive Detail Lifetime

- Fetch full State only when an injected row's hover/click/focus popover opens,
  using the existing authenticated usage-detail API.
- `useUsageTurnState` checks the returned request ID and owns an AbortController
  and generation guard. Close, row change or disposal aborts the old read and
  clears the full value. Late responses/errors cannot repopulate it.
- No raw-value browser storage, list prefetch, persistent cache or safe diagnostic
  export. Keep loading, absent-value, read-error and explicit retry states.
- Full values wrap inside a viewport-bounded popover and remain selectable.
  Reserve the same content height during loading/error/success so the panel
  cannot expand over its own trigger after fetching the raw value.
  Keyboard Escape must close the popover and cancel pending hover-open timers,
  so sensitive content cannot reopen after dismissal.

## 4. Validation & Error Matrix

| Condition | Result |
| --- | --- |
| Summary absent | `未记录`; no detail fetch |
| Explicit `injected: false` | `—`; no detail fetch |
| Injected with no returned State | Keep injected count; `无回显` |
| Returned count differs | Warning color, not a validity verdict |
| Detail request fails | Read error and explicit retry |
| Wrong request ID or oversized/missing State | No full value published |
| Close, row change or disposal | Abort old read and clear raw value |
| Delayed response or hover timer after close | No restored sensitive detail |

## 5. Good / Base / Bad Cases

Good: a 332-character injection and 356-character echo show both counts and a
warning. Base: successful requests without an echo retain their injected count.
Bad: showing the account's newest active State as an older request's evidence.

## 6. Tests Required

- Unit tests cover lazy reads, cancellation/disposal, stale response ownership,
  missing/mismatched/oversized detail, retry and row display distinctions.
- Browser checks use synthetic data and block non-fixture API and socket
  traffic. Check full-value loading, no storage, dismissal, and bounded text at
  1440, 390 and 320 pixels in both themes.
- Preserve the existing independent usage/error column preferences, required
  columns, reset, keyboard navigation and table scrolling.

## 7. Wrong vs Correct

Wrong: prefetch all raw values, cache them in preferences, or label equal lengths
as accepted. Correct: keep request-local summaries, fetch only visible detail,
clear it on dismissal and report observed facts without acceptance claims.
