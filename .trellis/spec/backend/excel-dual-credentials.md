# Independent Excel Credentials

## 1. Scope / Trigger

One OpenAI user/workspace has one account row and separate Codex and Excel OAuth
credentials. This replaces model-based Excel routing and reuse of Codex tokens.
The protocol evidence is the locally inspected CPA-BPS 0.5.34 binary, not recovered
original source or proof of upstream model quality. Production/live acceptance is
separate from local synthetic tests.

## 2. Signatures

- `CredentialTarget::{Codex, Excel}` and `CredentialSelection::{Codex, Excel, All}`.
- Migration `0091_openai_excel_credentials.sql` adds `openai_excel_credentials`.
  Keep applied migrations and their frozen hashes unchanged.
- `POST /api/admin/accounts/excel/import`: `data`, optional `accountId`,
  `expectedRevision`, `outboundProxyId`, `settings`; returns `accountIds` and
  `credentialTarget: "excel"`.
- Excel OAuth uses `/accounts/excel/oauth/start` and `/oauth/complete`; refresh,
  actual catalog and export use `/accounts/excel/refresh`, `/models`, `/export`.
- Relogin actions accept `credentialTarget`; omission preserves Codex API behavior.
  Each shared material row keeps Codex fields plus optional `excel` channel state.
- Connection tests accept optional `responsesUpstream`; `test_start` reports the
  frozen OpenAI route. Recovery takes `credentialTarget` and Excel `expectedRevision`.

## 3. Contracts

- Preserve logical account ID, settings, egress and capacity. Independent Excel
  versions, expiry, refresh claims, errors and suspension never enter Codex CAS,
  runtime cooldown, quota, Cookie, State or automatic recovery namespaces.
- Match nonempty upstream user and workspace together. Email alone never merges
  credentials. An Excel-only shell has `authenticationKind: "none"` and no fabricated
  Codex token; it starts with the Excel switch off. Codex import may fill the same
  shell only after exact identity validation.
- The account switch controls all ordinary generation. Share model allow/deny
  rules, but intersect them with the real BPS access catalog. Legacy `excelModels`,
  `excelDefaultModels` and follow-global fields are decode-only routing history.
  Unknown/unsupported models fail explicitly, without model or protocol fallback.
- Freeze route before selection and retain it through lease, capacity waits,
  retries and continuation. A connection test opened before a route change rejects
  the stale route. Native State probes explicitly use Codex without changing the switch.
- Old Excel-enabled rows without independent credentials fail closed until
  reauthorized; never exchange or submit their Codex token as an Excel credential.
- BPS OAuth uses PKCE S256, the BPS client, `/api/accounts/authorize`, the extension
  callback and `/oauth/token`. Verify client ID, user, workspace and expiry. Refresh
  claims are durable before a single RT use, followed by sidecar CAS. No author
  server or third-party credential relay participates in this implementation.
- Real permission comes from `/basispoints/api/responses/access?include_models=true`.
  Preserve the account egress for validation and generation. BPS request metadata
  removes client `bps_tools_version_id`, `task_id`, `turn_id`, `agent_iteration`.
  Native Codex request bodies, headers and transport stay unchanged.
- Relogin login material is shared; cached tokens, attempt IDs, status, selected
  targets and push settlement are per channel. A failed acquisition retains the
  last cached result. Global pause cancels only running channels, preserving an
  inactive sibling's cached result and pending enrollment intent.
- Excel password/TOTP login starts directly at the BPS PKCE authorization entry.
  It must not depend on ChatGPT Web CSRF, session or membership enumeration.
  Codex keeps its existing Web preflight. Use BPS workspace choices and the issued
  token identity; never guess a missing workspace or relax a locked target.
  Browser-only interaction remains an explicit failure, not an automatic bypass.
- Excel worker failures may report only an allowlisted stage and existing error
  code. Public messages are static; never persist URLs, response bodies, exception
  text, passwords, TOTP codes or tokens in diagnostics. Keep Codex error behavior.
- Enrollment batch writes consume canonical library rows, never channel projections.
  Excel replacement updates shared login material and only the Excel channel state;
  preserve the sibling's cache, targets and status. Reject replacement while either
  channel is active or uncertain. Progress tracks the frozen `(id, credentialTarget)`
  pair and reads the corresponding channel, including completion and retry state.
- All operations report per-channel outcomes; success is not rolled back when a
  sibling fails. Codex-first creation may attach Excel only to the same-identity
  account created by that batch. Freeze targets/revisions and never overwrite a
  later credential or infer a workspace from email.
- Validate push identity, settings, egress and BPS access before `Pushing`.
  After that persistence fence, uncertain commit outcomes remain `Uncertain` and
  must not be retried automatically. Refresh/import do not switch the route.
- Normal JSON export excludes password/TOTP. Explicit 2FA export remains separate.
  Preserve successful files when another selected slot is missing or fails.

## 4. Validation & Error Matrix

| Condition | Result |
| --- | --- |
| Missing/expired Excel credential at enable | Reject, preserve Codex and settings |
| API Key or non-OpenAI account | No Excel toggle/import attachment |
| Wrong user/workspace or stale credential version | Conflict before write |
| OAuth target changes before callback | Reject before code exchange |
| Excel refresh claim already consumed | No repeated RT exchange |
| Connection-test switch changes after opening | Conflict; no request on another route |
| Excel 401/403/429 or manual route recovery | Write only Excel state under its revision |
| Quality rule requests a route switch | `route_switch_requires_confirmation` |
| Excel quality action changes shared groups/template | `excel_shared_action_blocked` |
| Ordinary export missing one slot | Return successful files and typed skipped result |

## 5. Good / Base / Bad Cases

Good: Excel refresh rotates only its sidecar while Codex continues using its token.
Base: an old caller without `credentialTarget` retains Codex behavior.
Bad: a static model list, one correct puzzle response or filtered metadata is used
to claim guaranteed model quality or authorization.

## 6. Tests Required

Run the Core dual-slot and provider route/lease/retry/native-isolation contracts;
Admin/API relogin, import/export and expected-route tests; isolated PostgreSQL
sidecar CAS, refresh claim, shell promotion, directory projections, quality and
recovery tests. Run offline Python OAuth tests and the frontend suite, typecheck,
lint/build plus desktop/mobile synthetic browser checks. Do not count skipped
database tests or mocked upstream success as live acceptance.
Cover the complete Python `Login.run()` entry, not only `run_excel_oauth()`, plus
locked and ambiguous workspaces, failure-stage redaction, existing-material Excel
replacement with a preserved Codex channel, and independent progress for both
channels of the same library row.

## 7. Wrong vs Correct

Wrong: clear the whole relogin row on one channel's failure, or fall back to Codex
when Excel access fails. Correct: preserve the sibling, return the selected
channel's error and require an explicit account switch for future requests.
