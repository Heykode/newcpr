//! Pg 观测 adapter：实现 `ObservabilityRepository` 与 `AdminObservabilityStore`。

use futures::{StreamExt, TryStreamExt, stream::BoxStream};
use gateway_admin::ports::store::UsageCalculatedBillingStream;
use gateway_core::provider_ports::ProviderCooldownPort;
use std::sync::Arc;

use super::*;

use crate::postgres::{ProviderAccountStatus, load_account_statuses, load_rate_limited_until};

use crate::redis::{CredentialLeaseRepository as _, RedisCredentialLeaseRepository};

#[derive(Clone)]
pub struct PgObservabilityRepository {
    pool: PgPool,
    cooldowns: Option<Arc<dyn ProviderCooldownPort>>,
    query_budget: ObservabilityQueryBudget,
}

impl PgObservabilityRepository {
    #[must_use]
    pub fn new(
        pool: PgPool,
        cooldowns: Option<Arc<dyn ProviderCooldownPort>>,
        query_budget: ObservabilityQueryBudget,
    ) -> Self {
        Self {
            pool,
            cooldowns,
            query_budget,
        }
    }

    /// 从账号事实和当前冷却一次性派生 Dashboard 五态；不在 SQL 中复制状态机。
    async fn account_status_snapshot(
        &self,
        observed_at: DateTime<Utc>,
    ) -> StoreResult<(ProviderAccountMetrics, Vec<ProviderAccountStatus>)> {
        let accounts = self
            .query_budget
            .run(
                &self.pool,
                "load dashboard provider accounts",
                |mut connection| async move { load_account_statuses(&mut *connection).await },
            )
            .await?;
        let now = observed_at.into();
        let rate_limited_until =
            load_rate_limited_until(self.cooldowns.as_deref(), &accounts, now).await;
        let mut metrics = ProviderAccountMetrics {
            total: u64::try_from(accounts.len()).unwrap_or(u64::MAX),
            ..ProviderAccountMetrics::default()
        };
        let mut normal_accounts = Vec::new();
        for mut account in accounts {
            // Dashboard capacity remains an enabled-account view, unlike directory status.
            if !account.facts.enabled {
                metrics.disabled = metrics.disabled.saturating_add(1);
                continue;
            }
            let runtime = rate_limited_until.get(&account.id).copied();
            account.facts.rate_limited_until = runtime.map(|(until, _)| until);
            let mut projection = gateway_core::account::resolve_account_status(&account.facts, now);
            if runtime.is_some_and(|(_, protection)| protection)
                && projection.status != gateway_core::account::AccountStatus::Error
            {
                projection.status = gateway_core::account::AccountStatus::RateLimited;
            }
            match projection.status {
                gateway_core::account::AccountStatus::Normal => {
                    metrics.normal = metrics.normal.saturating_add(1);
                    normal_accounts.push(account);
                }
                gateway_core::account::AccountStatus::QuotaExhausted => {
                    metrics.quota_exhausted = metrics.quota_exhausted.saturating_add(1);
                }
                gateway_core::account::AccountStatus::RateLimited => {
                    metrics.rate_limited = metrics.rate_limited.saturating_add(1);
                }
                gateway_core::account::AccountStatus::Disabled => {
                    metrics.disabled = metrics.disabled.saturating_add(1);
                }
                gateway_core::account::AccountStatus::Error => {
                    metrics.error = metrics.error.saturating_add(1);
                }
            }
        }
        Ok((metrics, normal_accounts))
    }
}

/// `gateway-admin` 观测端口的 PostgreSQL adapter。
///
/// SQL 查询与内部投影继续由 [`PgObservabilityRepository`] 唯一拥有；本类型只负责
/// Admin UTC 领域模型与持久化投影之间的无格式化转换。
#[derive(Clone)]
pub struct PgAdminObservabilityStore {
    repository: PgObservabilityRepository,
    runtime_signals: Option<RedisCredentialLeaseRepository>,
}

impl PgAdminObservabilityStore {
    #[must_use]
    pub fn new(
        pool: PgPool,
        runtime_signals: Option<RedisCredentialLeaseRepository>,
        cooldowns: Option<Arc<dyn ProviderCooldownPort>>,
        query_budget: ObservabilityQueryBudget,
    ) -> Self {
        Self {
            repository: PgObservabilityRepository::new(pool, cooldowns, query_budget),
            runtime_signals,
        }
    }
}

impl PgObservabilityRepository {
    async fn dashboard_summary_with_accounts(
        &self,
        range: ObservabilityRange,
        observed_at: DateTime<Utc>,
    ) -> StoreResult<(DashboardObservation, Vec<ProviderAccountStatus>)> {
        let filter = UsageRecordFilter::default();
        let account_usage_query =
            ProviderAccountUsageQuery::recent(range, DASHBOARD_ACCOUNT_LIMIT)?
                .with_hourly_request_buckets()?;
        let recent_query = UsageRecordQuery {
            record_status: admin_observability::UsageRecordStatus::Success,
            range,
            filter: UsageRecordFilter {
                outcome: Some("succeeded".to_owned()),
                ..UsageRecordFilter::default()
            },
            current_page: 1,
            page_size: ObservabilityPageSize::new(10)?,
        };
        // 每条 SQL 独立取一个全局观测槽位，避免整包预留造成队头阻塞。
        let (totals, (provider_accounts, normal_accounts)) = futures::try_join!(
            self.query_budget.run(
                &self.pool,
                "load dashboard lifetime totals",
                |mut connection| async move { dashboard_totals(&mut connection).await }
            ),
            self.account_status_snapshot(observed_at),
        )?;
        let (trend, account_usage, recent_requests) = futures::try_join!(
            self.query_budget.run(
                &self.pool,
                "load dashboard request trend",
                |mut connection| async move {
                    dashboard_request_metric_series(&mut connection, range, &filter).await
                },
            ),
            self.query_budget.run(
                &self.pool,
                "load dashboard account usage",
                |mut connection| async move {
                    provider_account_usage(&mut connection, account_usage_query).await
                },
            ),
            self.query_budget.run(
                &self.pool,
                "load dashboard recent requests",
                |mut connection| async move {
                    list_usage_record_items(&mut connection, &recent_query, None).await
                }
            ),
        )?;
        Ok((
            DashboardObservation {
                range,
                totals,
                provider_accounts,
                trend,
                account_usage,
                recent_requests,
            },
            normal_accounts,
        ))
    }
}

impl PgAdminObservabilityStore {
    async fn runtime_slots_for_accounts(
        &self,
        normal_accounts: &[ProviderAccountStatus],
    ) -> AdminStoreResult<Option<admin_observability::DashboardRuntimeSlots>> {
        let inherited_accounts = u64::try_from(
            normal_accounts
                .iter()
                .filter(|account| account.concurrency_limit.is_none())
                .count(),
        )
        .map_err(|_| observability_error(invalid("inherited account count overflows u64")))?;
        let overridden_slots = normal_accounts.iter().fold(0_u64, |total, account| {
            total.saturating_add(
                account
                    .concurrency_limit
                    .map_or(0, |limit| u64::from(limit.get())),
            )
        });
        if normal_accounts.is_empty() {
            return Ok(Some(admin_observability::DashboardRuntimeSlots {
                inherited_accounts,
                overridden_slots,
                used_slots: Some(0),
            }));
        }
        let normal_account_ids = normal_accounts
            .iter()
            .map(|account| account.id.clone())
            .collect::<Vec<_>>();
        let Some(runtime_signals) = &self.runtime_signals else {
            return Ok(Some(admin_observability::DashboardRuntimeSlots {
                inherited_accounts,
                overridden_slots,
                used_slots: None,
            }));
        };
        let signals = match runtime_signals
            .credential_runtime_signals(&normal_account_ids)
            .await
        {
            Ok(signals) => signals,
            Err(_) => {
                return Ok(Some(admin_observability::DashboardRuntimeSlots {
                    inherited_accounts,
                    overridden_slots,
                    used_slots: None,
                }));
            }
        };
        let used_slots = signals.into_iter().fold(0_u64, |total, signal| {
            total.saturating_add(u64::from(signal.in_flight))
        });
        Ok(Some(admin_observability::DashboardRuntimeSlots {
            inherited_accounts,
            overridden_slots,
            used_slots: Some(used_slots),
        }))
    }
}

#[async_trait]
impl ObservabilityRepository for PgObservabilityRepository {
    async fn usage_concurrency(
        &self,
        range: ObservabilityRange,
        filter: UsageRecordFilter,
    ) -> StoreResult<Vec<admin_observability::UsageConcurrencyPoint>> {
        self.query_budget
            .run(
                &self.pool,
                "load usage concurrency",
                |mut connection| async move {
                    request_concurrency(&mut connection, range, &filter).await
                },
            )
            .await
    }

    async fn dashboard_summary(
        &self,
        range: ObservabilityRange,
        observed_at: DateTime<Utc>,
    ) -> StoreResult<DashboardObservation> {
        self.dashboard_summary_with_accounts(range, observed_at)
            .await
            .map(|(observation, _)| observation)
    }

    async fn dashboard_trend(
        &self,
        range: ObservabilityRange,
    ) -> StoreResult<Vec<RequestMetricPoint>> {
        self.query_budget
            .run(
                &self.pool,
                "load dashboard request trend",
                |mut connection| async move {
                    dashboard_request_metric_series(
                        &mut connection,
                        range,
                        &UsageRecordFilter::default(),
                    )
                    .await
                },
            )
            .await
    }

    async fn usage_trend(
        &self,
        range: ObservabilityRange,
        filter: UsageRecordFilter,
    ) -> StoreResult<Vec<RequestMetricPoint>> {
        self.query_budget
            .run(
                &self.pool,
                "load usage request trend",
                |mut connection| async move {
                    request_metric_series(&mut connection, range, &filter).await
                },
            )
            .await
    }

    fn usage_calculated_billing_facts(
        &self,
        range: ObservabilityRange,
        filter: UsageRecordFilter,
    ) -> BoxStream<'_, StoreResult<CalculatedUsageBillingFact>> {
        self.query_budget.run_stream(
            &self.pool,
            "load calculated usage billing facts",
            move |connection| calculated_usage_billing_facts(connection, range, filter),
        )
    }

    async fn provider_account_usage(
        &self,
        query: ProviderAccountUsageQuery,
    ) -> StoreResult<Vec<ProviderAccountUsageObservation>> {
        self.query_budget
            .run(
                &self.pool,
                "load provider account usage",
                |mut connection| async move { provider_account_usage(&mut connection, query).await },
            )
            .await
    }

    async fn list_usage_records(&self, query: UsageRecordQuery) -> StoreResult<UsageRecordPage> {
        self.query_budget
            .run(
                &self.pool,
                "list usage records",
                |mut connection| async move { list_usage_records(&mut connection, query).await },
            )
            .await
    }

    async fn usage_record_detail(&self, request_id: &str) -> StoreResult<UsageRecordDetail> {
        self.query_budget
            .run(
                &self.pool,
                "load usage record detail",
                |mut connection| async move { usage_record_detail(&mut connection, request_id).await },
            )
            .await
    }

    async fn usage_summary(
        &self,
        range: ObservabilityRange,
        filter: UsageRecordFilter,
    ) -> StoreResult<UsageOverview> {
        filter.validate()?;
        let filter = &filter;
        let requests =
            self.query_budget
                .run(
                    &self.pool,
                    "load usage request metrics",
                    |mut connection| async move {
                        request_metrics(&mut connection, range, filter).await
                    },
                )
                .await?;
        let attempts =
            self.query_budget
                .run(
                    &self.pool,
                    "load usage attempt metrics",
                    |mut connection| async move {
                        attempt_metrics(&mut connection, range, filter).await
                    },
                )
                .await?;
        let providers = self
            .query_budget
            .run(
                &self.pool,
                "load usage provider observations",
                |mut connection| async move {
                    provider_observations(&mut connection, range, filter).await
                },
            )
            .await?;
        Ok(UsageOverview {
            range,
            requests,
            attempts,
            providers,
        })
    }

    async fn usage_diagnostics(
        &self,
        range: ObservabilityRange,
        filter: UsageRecordFilter,
        dimension: DiagnosticDimension,
        page: Option<DiagnosticPageQuery>,
    ) -> StoreResult<DiagnosticObservationPage> {
        self.query_budget
            .run(
                &self.pool,
                "load usage diagnostics",
                |mut connection| async move {
                    usage_diagnostics(&mut connection, range, &filter, dimension, page).await
                },
            )
            .await
    }

    async fn list_ops_errors(&self, query: OpsErrorQuery) -> StoreResult<OpsErrorPage> {
        self.query_budget
            .run(&self.pool, "list ops errors", |mut connection| async move {
                list_ops_errors(&mut connection, query).await
            })
            .await
    }
}

#[async_trait]
impl AdminObservabilityStore for PgAdminObservabilityStore {
    fn query_timeout(&self) -> std::time::Duration {
        self.repository.query_budget.query_timeout()
    }

    async fn usage_metric_observation(
        &self,
        range: admin_observability::TimeRange,
        filter: admin_observability::UsageFilter,
        metric: admin_observability::UsageInsightMetric,
    ) -> AdminStoreResult<admin_observability::UsageInsightsObservation> {
        use admin_observability::UsageInsightMetric as M;
        use sqlx::Connection as _;
        let range = store_range(range)?;
        let filter = store_usage_filter(filter);
        self.repository
            .query_budget
            .run(
                &self.repository.pool,
                "load selected usage metric",
                |mut connection| async move {
                    let mut tx = connection.begin().await.map_err(|error| {
                        postgres_unavailable("begin usage snapshot").with_source(error)
                    })?;
                    sqlx::query("set transaction isolation level repeatable read, read only")
                        .execute(&mut *tx)
                        .await
                        .map_err(|error| {
                            postgres_unavailable("set usage snapshot").with_source(error)
                        })?;
                    let costs = matches!(metric, M::Cost | M::Savings | M::Cache);
                    let requests =
                        request_metrics_selected(&mut tx, range, &filter, Some(metric)).await?;
                    let trend = request_metric_series_selected(
                        &mut tx,
                        range,
                        &filter,
                        costs,
                        Some(metric),
                    )
                    .await?;
                    let attempts = if costs {
                        attempt_metrics(&mut tx, range, &filter).await?
                    } else {
                        AttemptMetrics::default()
                    };
                    let concurrency = if metric == M::Load {
                        request_concurrency(&mut tx, range, &filter).await?
                    } else {
                        Vec::new()
                    };
                    tx.commit().await.map_err(|error| {
                        postgres_unavailable("finish usage snapshot").with_source(error)
                    })?;
                    Ok((
                        UsageOverview {
                            range,
                            requests,
                            attempts,
                            providers: Vec::new(),
                        },
                        trend,
                        concurrency,
                    ))
                },
            )
            .await
            .map_err(observability_error)
            .and_then(|(overview, trend, concurrency)| {
                Ok(admin_observability::UsageInsightsObservation {
                    overview: admin_usage_overview(overview)?,
                    trend: trend
                        .into_iter()
                        .map(admin_request_metric_point)
                        .collect::<AdminStoreResult<_>>()?,
                    concurrency,
                })
            })
    }

    async fn usage_concurrency(
        &self,
        range: admin_observability::TimeRange,
        filter: admin_observability::UsageFilter,
    ) -> AdminStoreResult<Vec<admin_observability::UsageConcurrencyPoint>> {
        self.repository
            .usage_concurrency(store_range(range)?, store_usage_filter(filter))
            .await
            .map_err(observability_error)
    }

    async fn diagnostic_errors(
        &self,
        target: &gateway_admin::model::diagnostics::DiagnosticTarget,
        limit: u16,
    ) -> AdminStoreResult<Vec<admin_observability::OpsError>> {
        let target = target.clone();
        self.repository.query_budget.run(&self.repository.pool, "export diagnostic errors", |mut connection| async move {
            diagnostic_errors(&mut connection, &target, limit).await
        }).await.map(|records| records.into_iter().map(admin_ops_error).collect()).map_err(observability_error)
    }
    async fn account_filter_options(
        &self,
        range: admin_observability::TimeRange,
        search: &str,
    ) -> AdminStoreResult<Vec<admin_observability::AccountFilterOption>> {
        let range = store_range(range)?;
        self.repository
            .query_budget
            .run(
                &self.repository.pool,
                "account filter options",
                |mut connection| async move {
                    account_filter_options(&mut connection, range, search).await
                },
            )
            .await
            .map_err(observability_error)
    }
    async fn dashboard_summary(
        &self,
        range: admin_observability::TimeRange,
        observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<admin_observability::DashboardObservation> {
        let observation = self
            .repository
            .dashboard_summary(store_range(range)?, observed_at)
            .await
            .map_err(observability_error)?;
        admin_dashboard_observation(observation)
    }

    async fn dashboard_with_runtime_slots(
        &self,
        range: admin_observability::TimeRange,
        observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<(
        admin_observability::DashboardObservation,
        Option<admin_observability::DashboardRuntimeSlots>,
    )> {
        let (observation, accounts) = self
            .repository
            .dashboard_summary_with_accounts(store_range(range)?, observed_at)
            .await
            .map_err(observability_error)?;
        let slots = self.runtime_slots_for_accounts(&accounts).await?;
        Ok((admin_dashboard_observation(observation)?, slots))
    }

    async fn dashboard_card_metrics(
        &self,
        range: admin_observability::TimeRange,
    ) -> AdminStoreResult<admin_observability::RequestMetrics> {
        let range = store_range(range)?;
        let metrics = self.repository.query_budget.run(
            &self.repository.pool,
            "load dashboard comparison counters",
            |mut connection| async move { dashboard_card_metrics(&mut connection, range).await },
        ).await.map_err(observability_error)?;
        admin_request_metrics(metrics)
    }

    async fn dashboard_runtime_slots(
        &self,
        observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<Option<admin_observability::DashboardRuntimeSlots>> {
        let (_, accounts) = self
            .repository
            .account_status_snapshot(observed_at)
            .await
            .map_err(observability_error)?;
        self.runtime_slots_for_accounts(&accounts).await
    }

    async fn dashboard_trend(
        &self,
        range: admin_observability::TimeRange,
    ) -> AdminStoreResult<Vec<admin_observability::RequestMetricPoint>> {
        self.repository
            .dashboard_trend(store_range(range)?)
            .await
            .map_err(observability_error)?
            .into_iter()
            .map(admin_request_metric_point)
            .collect()
    }

    async fn usage_trend(
        &self,
        range: admin_observability::TimeRange,
        filter: admin_observability::UsageFilter,
    ) -> AdminStoreResult<Vec<admin_observability::RequestMetricPoint>> {
        self.repository
            .usage_trend(store_range(range)?, store_usage_filter(filter))
            .await
            .map_err(observability_error)?
            .into_iter()
            .map(admin_request_metric_point)
            .collect()
    }

    fn usage_calculated_billing_facts(
        &self,
        range: admin_observability::TimeRange,
        filter: admin_observability::UsageFilter,
    ) -> UsageCalculatedBillingStream<'_> {
        let range = match store_range(range) {
            Ok(range) => range,
            Err(error) => return Box::pin(futures::stream::once(async { Err(error) })),
        };
        self.repository
            .usage_calculated_billing_facts(range, store_usage_filter(filter))
            .map_err(observability_error)
            .map(|fact| fact.and_then(admin_calculated_usage_billing_fact))
            .boxed()
    }

    async fn list_usage_records(
        &self,
        query: admin_observability::UsageQuery,
    ) -> AdminStoreResult<admin_observability::UsagePage> {
        let page = self
            .repository
            .list_usage_records(store_usage_query(query)?)
            .await
            .map_err(observability_error)?;
        admin_usage_page(page)
    }

    async fn usage_record_detail(
        &self,
        request_id: &str,
    ) -> AdminStoreResult<admin_observability::UsageDetail> {
        let detail = self
            .repository
            .usage_record_detail(request_id)
            .await
            .map_err(observability_error)?;
        admin_usage_detail(detail)
    }

    async fn usage_summary(
        &self,
        range: admin_observability::TimeRange,
        filter: admin_observability::UsageFilter,
    ) -> AdminStoreResult<admin_observability::UsageOverview> {
        let overview = self
            .repository
            .usage_summary(store_range(range)?, store_usage_filter(filter))
            .await
            .map_err(observability_error)?;
        admin_usage_overview(overview)
    }

    async fn usage_diagnostics(
        &self,
        range: admin_observability::TimeRange,
        filter: admin_observability::UsageFilter,
        dimension: admin_observability::DiagnosticDimension,
        page: Option<admin_observability::DiagnosticPageQuery>,
    ) -> AdminStoreResult<admin_observability::DiagnosticObservationPage> {
        let result = self
            .repository
            .usage_diagnostics(
                store_range(range)?,
                store_usage_filter(filter),
                store_diagnostic_dimension(dimension),
                page.map(|page| DiagnosticPageQuery {
                    current_page: page.current_page,
                    page_size: page.page_size.get(),
                }),
            )
            .await
            .map_err(observability_error)?;
        let items = result
            .items
            .into_iter()
            .map(admin_diagnostic_observation)
            .collect::<AdminStoreResult<Vec<_>>>()?;
        Ok(admin_observability::DiagnosticObservationPage {
            total_request_count: result.total_request_count,
            items,
            current_page: result.current_page,
            page_size: result.page_size,
            has_more: result.has_more,
        })
    }

    async fn list_ops_errors(
        &self,
        query: admin_observability::OpsErrorQuery,
    ) -> AdminStoreResult<admin_observability::OpsErrorPage> {
        let page = self
            .repository
            .list_ops_errors(store_ops_error_query(query)?)
            .await
            .map_err(observability_error)?;
        admin_ops_error_page(page)
    }
}

pub(crate) fn observability_error(
    error: StoreError,
) -> gateway_admin::ports::store::AdminStoreError {
    admin_store_error("observability", error)
}
