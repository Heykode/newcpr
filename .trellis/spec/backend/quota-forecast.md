# Quota Forecast Contracts

## 当前合同：列表动态额度、监控持久学习与可恢复寿命

列表与分组不再共用动态比值。第 9 节仅供分组监控，不能改动列表或详情公式。

- Admin `current_window_estimate` 是列表/详情动态金额估值 owner：当前窗口已知 USD 消费
  `* 100 / used_percent`，剩余 `max(total-cost, 0)`。有限正数即可，无 $5/3pp/5pp 门槛。
  `window.local_usage` 与原 `usage_window()` 共用归属，不能混合模型桶或历史累计。
- API `usage.quotaWindow` 投影源窗口及 `estimatedUsd/remainingUsd`，前端不重复计算。
  列表每 30 秒请求时现算。分组每 10 秒通过 `AccountsService::monitor_quota`
  读取缓存百分比，并用 `AccountStore::load_quota_learning_usage` 对齐观察时刻的费用；
  不加载健康/累计/Token 预测历史，也不增加上游流量。
- 详情美元值同样动态计算，Token 配对算法保留。详情缓存 10 秒，打开时 30 秒重读；
  列表不预取详情。所有读取均不主动请求上游额度/认证。
- 分组汇总可用账号的真实账号级窗口，多窗口取最小剩余，不累加或外推。
  复用 `0014` 两张表，用 `monitor-v2:` 加结构化窗口维度键隔离旧样本；
  旧键不借用、不覆盖。`0015` 只保存最新分组结果。
- `0016` 保存监控生命周期，首次时间只复用既有 `provider_accounts.created_at`。
  用现有 Provider/上游用户/空间自然标识作结构化 key；无用户标识时以本地账号 ID 隔离。
  不依赖设备记录是否已生成，不用邮箱，不改任何身份/指纹字段。
- 仅使用 CPR 规范化终态：banned/account_banned 立即计入，
  invalid/credential_invalid 与 expired/credential_expired 持续两分钟后计入。
  纯 access token 到期、临时 401、网络故障、限流、额度耗尽、正常删除不计失效。
- 恢复必须 ready、token 未过期，且成功推理开始时间晚于失效与当前 credential_observed_at；
  旧请求、重导入或错误文本清空不能伪造恢复。恢复撤销样本但保留原创建时间。
  同 Provider/Plan 按最近最多五个仍失效账号求均值，恢复后允许较早样本替补。
- 同身份跨删除/重导入保留创建时间与已确认失效历史；不同空间不合并。
  Store 串行同步只写监控表，后续业务统计仍只读；过期采样不得覆盖新生命周期。
  生命周期与额度学习的样本数、失效规则和用途不同，不能互相替代。

### Group Monitor Dynamic Capacity

- Group occupancy reads existing Redis client-admission leases using server time
  and exclusive expiry bounds. This observer never prunes, renews, admits or
  releases leases. Batch reads are bounded to 128 keys; no keyspace scan.
- Reuse current group/key bindings, including disabled keys with draining
  requests. Multi-group keys can contribute to multiple groups. Unscoped keys
  do not acquire a fictitious group; their account occupancy still consumes
  shared free slots.
- `usedSlots` is group-key occupancy; `totalSlots` is that occupancy plus the sum
  of `max(account_limit - account_global_in_flight, 0)` for unique eligible
  members. Failed reads yield unknown concurrency, not zero; the UI hides both
  numbers when `usedSlots` is null. Configured account limits never change.
- Group-only references use the arithmetic mean of the latest three completed
  learned bindings, ordered by binding time, not account creation time. They
  survive donor deletion, an empty pool and restart. Match provider, normalized
  explicit Plan, structured window key/group/role/limit ID and exact duration.
  New Plan names work without registration. A completely new dimension needs
  natural learning; never bootstrap from an unstable list estimate or old keys.
- Recipients require a valid current percentage, not zero local cost. Before
  completing their own learning, zero-cost, paid-0%, and low-percent accounts all
  keep using the matching persisted reference. Remaining is the learned limit
  times unused fraction. A positive known fee subtotal remains usable despite
  missing fees; all-missing zero cannot create a baseline.
  Upstream reset timestamps have whole-second precision, so an observation at
  most one second before the derived window start is still current; anything
  older remains ineligible.
  Never feed inherited limits back into completed samples. Account-list
  `current_window_estimate` remains independent and has no learning threshold.
- Failed optional peer reads are excluded from references; required eligible
  account read errors still fail the sample. Never disguise an account read
  error as zero usage or substitute a peer estimate for unreadable own facts.
- Mixed expiry projections skip accounts that outlived their Plan average,
  while retaining calculable accounts. All-outlived groups and otherwise missing
  lifespan/rate evidence retain null expiry amounts with distinct statuses when
  no account is calculable. Otherwise keep the known subtotal with partial status.
  ETA still uses the full known remaining
  amount and estimated accounts' deduplicated cross-group consumption. An
  eligible account awaiting its first usable quota window remains visible in
  coverage and concurrency but does not make existing group projections partial;
  it joins remaining, ETA and expiry calculations after an estimate appears.
- Sampling remains 10 seconds, list reads 30 seconds, without upstream refresh.
  Own the deduplicated quota IDs before building the buffered async stream:
  a borrowed mapped peer iterator fails `async_trait`'s `Send` generalization
  in this toolchain. Keep that ownership boundary during collection cleanup.
- Snapshot reads retain the latest completed sample across configuration
  revisions, with current group metadata, original timestamps and `refreshing`.
  Groups awaiting their first sample appear only in `pending_group_ids`; they
  do not hide other groups or fabricate zero balances. An all-pending report
  uses the Unix epoch timestamp. Deleted groups remain invalid; disabled groups
  immediately show disabled statuses. Display-only reports cannot be republished
  or evaluated for alerts. A failed manual sample rereads validated group IDs
  and returns existing data marked refreshing.
- Expiry states distinguish `lifespan_learning`, `rate_sampling` and
  `all_accounts_outlived_average`; none changes the independent ETA estimate.
  Continue using the existing latest-five valid death average and recovery
  retraction. No new learning or lifecycle tables are introduced.
- Group burn-rate inputs use a finite positive known USD subtotal even when
  other requests lack cost. A complete zero remains idle; zero known USD with
  missing costs stays unknown. Sum known account costs before this test so one
  all-missing account does not null another's known rate. Missing costs alone add
  no card warning.
- The UI uses original snapshot time plus the existing 45-second freshness
  bound. One member's earliest reset must not mask an entire valid snapshot.
  Backend window eligibility remains account-local; stale/display-only reports
  remain excluded from alert evaluation.


## 1. Scope / Trigger

Apply to quota forecasts across Store, Admin, API and the account UI. These
estimates are not balances, billing facts or scheduling inputs.

## 2. Signatures

- `GET /api/admin/accounts/quota-forecast?accountId=acct_test`
  requires `AdminAuth` and accepts only the existing `AccountIdQuery`.
- `AccountsService::quota_forecast(&ProviderAccountId)` returns
  `AccountQuotaForecastReport`.
- `AccountStore::load_quota_forecast_history(&AccountUsageWindowQuery)` returns
  `QuotaForecastHistory`.
- `ProviderAdmin::quota_forecast_observation(&ProviderDocument, &ProviderQuotaWindow)`
  projects provider-owned observations without adding provider dependencies to
  the Store or HTTP adapter.

## 3. Contracts

- Return the normal admin envelope with `accountId`, `generatedAt` and weekly
  and monthly `forecasts`. Preserve nullable estimates, `unavailableReason`,
  `lowSample`, `incompleteCost`, `incompleteTokens` and `extrapolated`.
- Load cached quota with `refresh=false`. This endpoint must not generate a
  model request, refresh credentials or mutate account state.
- Prefer the actual matching window. If only a different window is available,
  label the 7/30-day capacity extrapolation; remaining capacity still belongs
  to the source window.
- Token forecasts require a current snapshot, compatible plan/reset boundaries
  and at least five sampled percentage points. Truncated history and counter
  regressions must not silently become full-window samples. List/detail USD
  capacity remains the dynamic ratio; section 9 applies only to group monitoring.
- Missing costs and tokens have independent coverage. Missing data stays
  unknown rather than becoming zero or a false fully-covered estimate. Token
  estimates may use partial known totals with the incomplete flag. A recorded
  zero is distinct from an unknown fee. The source's displayed Token subtotal is
  also null when no tokens were observed; preserve a recorded zero and a
  positive partial subtotal instead of conflating them with all-unknown data.
- Store queries are bounded to 32 days and at most 128 provider-document
  observation points, with full-range numeric aggregates. Use one MVCC
  snapshot and equal cumulative totals for tied completion times.
- The shared inference predicate excludes OpenAI `prewarm`; health counts,
  raw records, cumulative costs and API-key budgets retain their separate
  contracts. See `account-cumulative-costs.md`, including its historical
  client-supplied-label limitation.
- The historical detail modal retains a page-scoped cache (concurrency <=2,
  <=200 entries, five-minute TTL, failure backoff 30 seconds).
  Account list refreshes only synchronize Plan/reset signatures; they never
  prefetch or periodically revalidate historical forecasts.
  The list weekly estimate instead uses `usage.costs` and `usage.quotaWindow`,
  projected together from the existing `ProviderQuota::usage_window()` owner.
  Only a live weekly source and finite positive USD cost/percent qualify:
  cost * 100 / percent, without a 5% threshold or Plan/learned fallback.
  It updates with each existing 30-second list response, independently of the
  historical detail cache. No extra Store query or upstream refresh is added.
  Manual quota refresh remains a separate existing action. Invalidate and
  synchronize account signatures before row publication to avoid a competing
  list watcher cancelling the modal refresh. Subscriber cancellation and late
  response guards must prevent account A from overwriting account B.
  Display exact-format USD text without an approximation sign; retain the
  estimate/partial/extrapolation qualification in details and title.

## 4. Validation & Error Matrix

| Input / state | Required result |
| --- | --- |
| Missing admin authentication | 401; no forecast access |
| Missing/invalid account ID or extra `refresh` parameter | 400 |
| Expired window or missing current snapshot | Null estimates with an explanation |
| Under five points of sampled progress | No new Token estimate; USD uses the independent learning thresholds |
| Missing fees but complete tokens | Token estimate may remain valid; no new USD binding |
| Some fees/tokens known, some missing | Partial Token estimate retains coverage; learned USD can still be inherited |
| Every metric is unknown or unusable | Null estimates with an explanation, never a zero-capacity claim |
| Plan/reset mismatch or decreasing counters | Reset/disqualify the sample segment |
| Store unavailable | Existing safe admin error; not a successful zero report |
| Cancel/close/switch modal | No stale account replacement or notification from cancellation |

## 5. Good / Base / Bad Cases

- Good: window inference costs are `$2`, known prewarm costs are `$3`:
  prediction uses `$2`; immutable cumulative accounting can correctly remain `$5`.
- Base: a new account inherits a matching Plan average when available; otherwise
  it remains usable while the UI shows that capacity is still unknown.
- Bad: divide permanent cumulative costs by the current quota percentage,
  or claim an extrapolated monthly estimate is an official allowance.

## 6. Tests Required

- Admin model/service: boundaries, mid-window imports, paired incremental
  samples, pending/partial costs, equal timestamps and counter resets.
- Store with real PostgreSQL: bounded observation selection, successful
  inference versus prewarm, unknown costs, health/diagnostic counts and
  permanent cumulative-cost deduplication.
- API handler: admin authentication, strict query validation and no-store;
  presenter: final DTO, nullable numbers and display formatting.
- Frontend regression and real browser: one shared modal, account switching,
  cancellation, weekly/monthly/unknown states, manual refresh, multiple
  background account refresh cycles without new forecast reads, and
  non-overflowing 320/390px layouts.
- Use `CI=true`, `CPR_TEST_DATABASE_URL` and `CPR_TEST_REDIS_URL` with isolated
  services; missing-environment early returns are not persistence verification.
- Linux test bundles from macOS must exclude AppleDouble (`._*`) metadata:
  SQLx treats stray metadata beside migrations as invalid migration filenames.
  Fix the bundle, not migrations. Remove task-owned build and browser artifacts
  after remote verification, retaining only small evidence outside the checkout.

## 7. Wrong vs Correct

Wrong: treat a zero-output response or client metadata label as sufficient
proof that a new model request is only prewarm.

Correct: classify new prewarm requests from the provider's actual
`generate=false` request semantics, while preserving raw evidence and fees.

## 8. Read-Only Group Monitor

- `/api/admin/account-groups/monitor` composes forecasts without changing
  account/credential, fingerprint, scheduler or accounting ownership. Only the
  prediction-owned learning tables (`0014`) and latest monitor snapshots (`0015`)
  are written.
- Group memberships, metadata, last-60-second completed inference fees and
  explicit banned-lifespan samples for all groups share one read-only
  repeatable-read snapshot, including the config revision.
  Reuse the member-query decoder and inference predicate. Bound each statement
  to two seconds and page readers/forecast loaders to two concurrent operations.
- Admin contributes the `admin_group_monitor` daemon under the existing runtime
  snapshot reconciliation kind. Host owns its lifecycle. Sample immediately and
  every 10 seconds without browser activity, skip missed ticks, cap a cycle at
  60 seconds, and deduplicate forecast loading across all groups. No upstream refresh.
- One sampling lock coalesces overlapping manual/background callers, including
  their errors. Ordinary reads do not hold or wait for this lock. Sampling errors
  log a safe warning and retry on the next tick; display-only failures must not
  fail gateway readiness. Host still supervises panic/restart and cancellation.
- Atomically save one latest row per group in `account_group_monitor_snapshots`;
  retain the original timestamp on failure and reject older overwrites.
  Revision-mismatched read fallback is display-only, as specified above.
  Group deletion cascades the snapshot but never Plan learning.
- Optional strict boolean `refreshForecasts` defaults to false. Ordinary/page
  polling reads persisted snapshots only. A manual true request triggers the same
  global sample and then returns the selected one-to-three groups. The previous
  300-second forecast cache is removed; persistent learning and existing upstream
  observations remain the source of truth.
- Group rates follow current account membership and can overlap for shared
  accounts. ETA uses deduplicated eligible estimated accounts' all-group
  rate; shared balances/concurrency must not be added across groups.
- Do not sum weekly/monthly extrapolated balances. Failed reads and missing
  runtime retain distinct unknown states. A missing estimate for a newly imported
  account does not hide projections from already estimated accounts; truthful
  estimated/eligible coverage remains visible. Known positive USD remains usable
  despite other missing fees; an all-missing rate is unknown, and an idle complete
  rate is not infinite ETA.
- Expiry waste is a qualified observed-lifespan estimate, never token/reset
  expiry. Use the recoverable lifecycle and original account creation-time
  contracts above; device first-seen is not the start of this lifetime.
  Missing lifespan or age beyond the mean excludes that account's waste estimate.
- Test the SQL against isolated real PostgreSQL, including completion boundaries,
  overlapping scopes, prewarm exclusion and unchanged account/audit/revision rows.

## 9. Tools-Aligned USD Learning

- Reuse `fill_missing_plan_type` / `explicit_plan_type`, cached quota and existing
  bounded inference-fee history. Preserve account subtypes instead of replacing
  them with a generic quota Plan. Arbitrary non-empty Plan names are accepted;
  `unknown` is not a Plan. No manual templates or second billing ledger.
- Persist an account/window baseline. Bind USD capacity only after both known
  cost growth >= $5 and usage growth >= 3 percentage points:
  `cost_delta / (percent_delta / 100)`. Never generate requests to reach a threshold.
- Read only settled inference costs completed no later than quota `observed_at`,
  starting at max(window start, account creation). In-flight work is excluded,
  not a global blocker. Reuse the inference predicate; no provider documents or
  cumulative-account ledger. Positive known subtotal is allowed with missing
  fees; unknown zero is not a baseline. Database errors fail sampling.
- `monitor-v2:` keys encode `[key, group, limit_id, role]` as JSON, with exact
  duration separate. Invalidation and stale-observation fencing are confined to
  the same key namespace. Never seed v2 from retired unversioned bindings.
- Use the account's bound limit first, otherwise the arithmetic mean of the
  latest at most three completed samples for Provider + case-insensitive Plan +
  window key + window minutes. One or two samples are usable but low-sample.
  Same-account/window samples replace each other; physically prune older samples.
- Account deletion cascades only personal state. Plan samples deliberately have
  no account foreign key, so they survive an empty pool and process restarts.
- Ordinary expired-window rollover retains personal capacity and starts a new
  baseline. A still-live long window explicitly falling to zero or moving its
  reset time forward clears all of that account's personal bindings to relearn.
  Plan samples remain. A real Plan/provider change also clears personal bindings.
- Transaction-scoped account/Plan advisory locks fence concurrent first writes
  and sample trimming. Reject snapshots older than the account's latest persisted
  observation even when their window has disappeared. Compare PostgreSQL timestamp
  precision; repeated observations must not rewrite state or create extra samples.
- Account-wide short windows also constrain monitor remaining USD. Use the
  tightest real source window, never sum or extrapolate remaining capacity.
  Missing/expired windows leave that account unestimated, without suppressing
  other calculable accounts in the group.
  Weekly/monthly capacity DTOs and Token predictions retain their existing format.
- Learning runs only through `AccountsService::monitor_quota`, called by the
  unattended sampler. List/detail forecast paths never write learning samples.
  Windows longer than the existing 32-day query limit remain unknown.
  New Plan names work automatically; new upstream protocols still need adaptation.
- Learning failures return a safe Admin error, not a successful zero or a fallback
  to the list USD ratio. Lifespan/expiry-waste history remains separate and reuses
  the existing five-death recovery-aware state machine.

### Regression Boundaries

The prior fixes retained HTTP/configuration-failure snapshots but did not cover
a successful response whose derived fields became null. Test transitions, not
only initial renders: no cost -> paid 0% -> first percent -> personal binding,
donor deletion -> empty pool -> new recipient, and single-window reset -> fresh
aggregate -> genuinely stale snapshot. Do not repair a null calculation by
refreshing an old timestamp, fabricating zero percent, or using it for alerts.
