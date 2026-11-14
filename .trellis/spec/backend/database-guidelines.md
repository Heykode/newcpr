# Database Guidelines

> Database patterns and conventions for this project.

---

## Overview

<!--
Document your project's database conventions here.

Questions to answer:
- What ORM/query library do you use?
- How are migrations managed?
- What are the naming conventions for tables/columns?
- How do you handle transactions?
-->

(To be filled by the team)

## Account Timestamp Invariant

`provider_accounts.updated_at` must not move backwards on an existing row.
Retain the stored value with `greatest(now(), updated_at, ...)` and include
all newly written observation times. The existing database constraint already
ensures the previous value covers retained creation and observation timestamps.
Credential, quota-document and quota-access observations can advance independently;
quota CAS must cover both the document and access observation times.

Do not rewrite observation timestamps, token expiry, reset times or revision
checks merely to satisfy this invariant. Do not increment timestamps artificially
or treat `updated_at` as a unique version. A clock rollback may temporarily keep
the displayed update time unchanged; credential/config revisions remain the
concurrency controls. Verify using isolated PostgreSQL with synthetic future-dated
rows, normal-time writes and stale/concurrent revision attempts, never by changing
the host clock.

---

## Query Patterns

<!-- How should queries be written? Batch operations? -->

(To be filled by the team)

---

## Migrations

<!-- How to create and run migrations -->

(To be filled by the team)

---

## Naming Conventions

<!-- Table names, column names, index names -->

(To be filled by the team)

---

## Common Mistakes

<!-- Database-related mistakes your team has made -->

(To be filled by the team)

## Scenario: Owned Observability Queries And Historical Key Names

### 1. Scope / Trigger

The approved v3.24.1 adaptation changes admin observability query ownership,
usage pagination and historical downstream Key projections. It does not change
model-request timeouts, account selection, cumulative settlement or log cleanup
rate. The database implementation remains SQLx/PostgreSQL.

### 2. Signatures

- `ObservabilityQueryBudget::run(&PgPool, operation, query)` passes one owned
  `ObservabilityConnection` to the query closure.
- `run_stream(&PgPool, operation, query)` holds the same owned connection until
  completion, error or stream drop.
- Helpers accept `&mut PgConnection`; owned billing streams take the connection
  by value. They must not reacquire from `PgPool` inside a budgeted operation.
- Migration `0069_usage_history_and_indexes.sql` adds
  `model_requests.client_api_key_name_snapshot text` and the partial indexes
  `model_requests_delivered_client_idx` / `model_requests_delivered_account_idx`.
  Append its digest to `.frozen-sha256`; never change earlier migrations.

### 3. Contracts

- The existing configured slot count and wait timeout remain authoritative.
  Queueing, connection acquisition and query consumption share the configured
  observability deadline (120 seconds by default). This applies only to admin
  observability, not inference. The owned connection captures and restores its
  original statement timeout after draining SQL and rollback. Failed restoration
  closes the connection instead of returning altered state to the pool.
- On cancellation, error or completion, the connection's SQLx return/rollback
  future keeps the semaphore permit until protocol cleanup finishes. Releasing
  the permit immediately on cancellation would oversubscribe PostgreSQL.
- Stream errors drop the inner database stream before yielding the error, even
  if the caller retains the returned stream without polling again.
- Usage page count and records share a read-only repeatable-read transaction.
  Order is always `(started_at desc, id desc)`. The first short page supplies its
  own count; later pages may scan the nearer end and restore descending order.
  Materialize only candidate IDs/timestamps before joining wide projections.
  Later usage/error pages use exact minute-bucket counts from that same snapshot
  to narrow the detail range. Preserve all NewCPR predicates, substring search,
  historical labels, quality isolation and stable cross-source ordering.
- Snapshot downstream Key names in both request-admission insert paths. Row
  details/list/search use the snapshot before the current Key name. Grouped
  diagnostics prefer a current name, then the latest snapshot, then the Key ID.
- Legacy backfill uses surviving Keys' names at upgrade time. A Key deleted
  before this migration has no recoverable old name; do not invent one.
- Keep account names/plans, quality-check and prewarm exclusion predicates,
  cumulative account costs and downstream budgets unchanged. New indexes must
  match NewCPR's existing delivered-usage predicate.
- Integration tests require isolated `CPR_TEST_DATABASE_URL`; do not treat a
  missing variable and skipped database tests as successful database validation.
- Enable CI mode and preflight both PostgreSQL and Redis authentication. Backup
  restore and StoreConfig fixtures require nonempty passwords in both test URLs,
  even when an isolated PostgreSQL instance explicitly uses trust authentication.
  Use test-only credentials satisfying StoreConfig limits, with matching-major
  pg_dump/psql; never change production credentials to satisfy a test harness.

### 4. Validation & Error Matrix

| Condition | Result |
| --- | --- |
| Zero slot count or zero wait timeout | `StoreError::InvalidData` |
| Slot wait expires or semaphore closes | `StoreError::Unavailable`, no query |
| Acquire/query/stream exceeds execution deadline | `StoreError::Unavailable`; owned cleanup retains permit |
| Query future or stream is dropped | Roll back unfinished transaction; release slot after protocol cleanup |
| Key renamed/deleted after admission | Original name remains in request rows and search |
| Key missing before legacy upgrade | Historical name remains unknown |
| Page beyond total | Empty page with accurate total and unchanged ordering |

### 5. Good / Base / Bad Cases

- Good: cancel a sleeping transaction; the next observation waits for rollback,
  then reads zero uncommitted rows. A retained failed stream releases its slot.
- Base: ordinary usage pages and grouped names retain existing API shapes,
  filters, cost totals and quality-check exclusion semantics.
- Bad: drop a permit while its SQL is still running, join full rows before
  pagination, or rewrite an applied migration to add the new column.

### 6. Tests Required

- Real isolated PostgreSQL: saturated-slot queue and timeout, cancellation during
  SQL, rollback, stream completion/error/drop, and execution deadline release.
- Both admission paths: snapshot survives rename/delete and remains searchable
  in usage and errors; grouped diagnostics fall back correctly after deletion.
- Pagination: timestamp ties, filters, exclusions, first/last/partial/empty pages
  and counts. Upgrade existing 0068 data through 0069 and verify all freeze hashes.

### 7. Wrong vs Correct

Wrong: `budget.run(..., |_| query.fetch_all(&pool))` or release a permit before
`PoolConnection::return_to_pool()` finishes.

Correct: use the closure's owned connection for all SQL in the operation:

```rust
budget.run(&pool, "report", |mut connection| async move {
    report(&mut connection).await
}).await
```

The connection owner starts return/rollback on drop and releases its permit
only when that cleanup future finishes.

## 场景: 同一账号的 Codex / Excel 凭据隔离

### 1. 范围与触发条件

迁移 `0091_openai_excel_credentials.sql` 为同一 OpenAI 用户和 workspace
增加独立 Excel 凭据。账号代理、模型访问策略、分组、权重和并发仍由
`provider_accounts` 统一管理。不得把 Codex token 复制成 Excel token。

### 2. 签名

- `openai_excel_credentials.account_id` 是关联 `provider_accounts.id` 的主键外键，
  删除账号时级联删除。它独立保存 `credential_revision`、`credential_json`、
  `credential_state`、过期时间、刷新计划、错误、额度、`suspended` 和
  `quality_pause_owner`。
- `ProviderAccountStore::load_current_excel_credential(id)` 返回
  `LoadedExcelCredential { account, metadata, credential }`。
- `compare_and_swap_excel_credential(ExcelCredentialCasUpdate)` 校验 Excel revision
  与用户/workspace，成功只推进 Excel revision。
- `claim_excel_refresh(id, expected_revision) -> bool` 在网络请求前持久化
  `refresh_attempt_revision` 与 `refresh_attempt_started_at`。
- `AccountStore::commit_excel_credential_import(prepared, target, settings, context)`
  接受 `ExcelCredentialImportTarget { account_id, expected_revision }`；缺省 revision
  仅允许新增该账号尚不存在的 Excel 凭据。

### 3. 数据与并发合同

- 一个用户/workspace 对应一个账号行。匹配已有账号时只写 Excel 侧表，不覆盖
  Codex revision、token、设备、State、配额或共享设置。
- 仅有 Excel 凭据时创建 `authentication_kind='none'`、Codex material `{}` 的账号，
  默认 `responses_upstream='codex'`。它在显式开启 Excel 前不可用于生成请求。
  后续同身份 Codex 导入可将 `none` 提升为 `oauth`，保留原账号 ID 和共享设置。
- 所有显式开启 Excel 的账号编辑、批量编辑和模板写入都要求存在同身份、ready、
  未过期的独立 Excel 凭据；失败时事务整体回滚。
- 读取完整凭据先锁父账号，再锁侧表，和导入保持相同顺序；metadata 与 material
  来自同一事务，管理 API 不返回 `credential_json`。
- 一份持久化 Excel revision 最多发起一次刷新。抢占失败不发送 HTTP；请求结果
  不明时保留抢占标记；成功 CAS 后新 revision 才清除标记。重启不能重试旧 token。
- Excel 403 的旧 `pause_account` / `disable_excel` 值都只暂停 Excel，不改父账号
  `enabled` 或入口。自动恢复只解除 Excel 暂停，且不得接管 Excel 质量规则的 owner。
- 活跃入口的状态、筛选、计数、过期时间及额度来自相应凭据；Excel 模式不使用
  Codex cooldown、额度窗口或错误保护。旧账号开启 Excel 但无侧表时显示待授权并拒绝调度。

### 4. 校验与错误矩阵

| 条件 | 结果 |
| --- | --- |
| Excel 用户或 workspace 与目标账号不符 | Invalid，双方凭据不变 |
| 导入预期 revision 过时，或无预期 revision 覆盖已有凭据 | Conflict |
| Excel CAS revision、身份或观测时间不匹配 | Conflict，不能覆盖新凭据 |
| 当前 revision 已抢占刷新 | `false`，不再使用该 refresh token |
| 缺少或过期 Excel 凭据时开启入口 | Invalid，整个配置事务回滚 |
| Excel 质量规则拥有暂停状态 | 恢复任务不抢占，也不解除暂停 |

### 5. 正常、基础与错误案例

- 正常: Codex 账号附加 Excel 后关闭入口，继续使用原 Codex 凭据；开启后使用独立 Excel 凭据。
- 基础: Excel-only 账号初始关闭入口，添加同身份 Codex 凭据后仍保持同一账号行。
- 错误: 刷新 Excel 失败后写父账号 invalid，或 403 自动切回 Codex，都会破坏凭据隔离。

### 6. 必需测试

- `tests/postgres/provider_accounts/excel_credentials.rs`: 父账号完整 JSON 不变、
  CAS/导入过期 revision、每 revision 一次刷新、删除级联、账号身份隔离、shell
  提升、开启条件、活跃入口的列表筛选与状态汇总。
- `excel_recovery.rs`: Excel 独立暂停、恢复条件失效、质量 owner 互斥、旧结果不可解除新暂停。
- 对现有 `postgres::provider_accounts` 运行真实隔离 PostgreSQL 回归，验证 Codex
  设备归属、State 保留、令牌刷新和普通账号导入。仅使用合成凭据和自建临时 schema。

### 7. 错误与正确实现

错误: `apply_state_change(excel_failure)` 或直接重用父账号 `credential_revision`。

正确: `apply_excel_state_change` / `compare_and_swap_excel_credential` 仅更新
`openai_excel_credentials`，并使用调用开始时捕获的 Excel revision 和同一身份。
