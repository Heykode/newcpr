//! 有界统计缓存：身份和完整筛选参与键，统计发布代次使旧快照失效

use crate::model::observability::*;
use crate::ports::store::{
    AdminStoreError, AdminStoreErrorKind, AdminStoreResult, ObservabilityStore,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::{collections::HashMap, future::Future, hash::Hash, sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::Instant};

const CAPACITY: usize = 64;
const TTL: Duration = Duration::from_secs(15);
const MAX_STALE: Duration = Duration::from_secs(60);

type Slot<T> = Arc<Mutex<Option<Cached<T>>>>;
struct Cached<T> {
    value: T,
    created: Instant,
}
struct Entry<T> {
    slot: Slot<T>,
    touched: Instant,
}
struct Cache<K, T> {
    entries: Mutex<HashMap<K, Entry<T>>>,
}
impl<K, T> Default for Cache<K, T> {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

trait AnalyticsValue: Clone {
    fn metadata(&mut self) -> &mut AnalyticsMetadata;
}
impl AnalyticsValue for DashboardObservation {
    fn metadata(&mut self) -> &mut AnalyticsMetadata {
        &mut self.analytics
    }
}
impl AnalyticsValue for UsageOverview {
    fn metadata(&mut self) -> &mut AnalyticsMetadata {
        &mut self.analytics
    }
}
impl AnalyticsValue for UsageInsightsObservation {
    fn metadata(&mut self) -> &mut AnalyticsMetadata {
        &mut self.overview.analytics
    }
}
impl AnalyticsValue for (DashboardObservation, Option<DashboardRuntimeSlots>) {
    fn metadata(&mut self) -> &mut AnalyticsMetadata {
        &mut self.0.analytics
    }
}

impl<K: Eq + Hash, T: AnalyticsValue> Cache<K, T> {
    async fn get<F, Fut>(
        &self,
        key: K,
        store: &dyn ObservabilityStore,
        ttl: Duration,
        load: F,
    ) -> AdminStoreResult<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = AdminStoreResult<T>>,
    {
        // 刷新失败后的同键等待也计入预算，避免串行重试把延迟放大为队列长度倍数
        tokio::time::timeout(
            store.query_timeout(),
            self.get_within_budget(key, store, ttl, load),
        )
        .await
        .map_err(|source| {
            AdminStoreError::new(
                AdminStoreErrorKind::Unavailable,
                "observability cache",
                "observability cache execution budget is exhausted",
            )
            .with_source(source)
        })?
    }

    async fn get_within_budget<F, Fut>(
        &self,
        key: K,
        store: &dyn ObservabilityStore,
        ttl: Duration,
        load: F,
    ) -> AdminStoreResult<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = AdminStoreResult<T>>,
    {
        let slot = {
            let mut entries = self.entries.lock().await;
            if !entries.contains_key(&key) && entries.len() >= CAPACITY {
                let oldest = entries
                    .iter()
                    .filter(|(_, entry)| Arc::strong_count(&entry.slot) == 1)
                    .min_by_key(|(_, entry)| entry.touched)
                    .map(|(_, entry)| entry.touched);
                if let Some(oldest) = oldest {
                    entries.retain(|_, entry| entry.touched != oldest);
                }
            }
            if !entries.contains_key(&key) && entries.len() >= CAPACITY {
                drop(entries);
                return load().await;
            }
            let entry = entries.entry(key).or_insert_with(|| Entry {
                slot: Arc::new(Mutex::new(None)),
                touched: Instant::now(),
            });
            entry.touched = Instant::now();
            entry.slot.clone()
        };
        // 同键刷新只执行一次；失败不会缓存成成功，也不延长旧结果寿命
        let mut cached = slot.lock().await;
        // 等锁期间代次可能推进；取得同键锁后再核对，失败时也不能回用已被新统计替代的快照
        let generation = store.analytics_cache_generation().await?;
        if let Some(entry) = cached.as_mut()
            && entry.value.metadata().generation == generation
            && entry.created.elapsed() < ttl
        {
            return Ok(with_age(entry.value.clone(), false));
        }
        match load().await {
            Ok(value) => {
                *cached = Some(Cached {
                    value: value.clone(),
                    created: Instant::now(),
                });
                Ok(value)
            }
            Err(error) => {
                if ttl == TTL
                    && error.kind() == AdminStoreErrorKind::Unavailable
                    && let Some(entry) = cached.as_mut()
                    && entry.created.elapsed() <= MAX_STALE
                    && Utc::now()
                        .signed_duration_since(entry.value.metadata().generated_at)
                        .to_std()
                        .unwrap_or_default()
                        <= MAX_STALE
                    && entry.value.metadata().state != AnalyticsState::Building
                    && entry.value.metadata().generation
                        == store.analytics_cache_generation().await?
                {
                    return Ok(with_age(entry.value.clone(), true));
                }
                Err(error)
            }
        }
    }
}

fn with_age<T: AnalyticsValue>(mut value: T, stale: bool) -> T {
    let metadata = value.metadata();
    metadata.lag_seconds = metadata.lag_seconds.saturating_add(
        Utc::now()
            .signed_duration_since(metadata.generated_at)
            .num_seconds()
            .max(0) as u64,
    );
    if stale {
        metadata.state = AnalyticsState::Stale;
    }
    value
}

impl AnalyticsValue for (DiagnosticObservationPage, AnalyticsMetadata) {
    fn metadata(&mut self) -> &mut AnalyticsMetadata {
        &mut self.1
    }
}

type UsageKey = (TimeRange, UsageFilter);
pub(crate) struct CachedObservabilityStore {
    inner: Arc<dyn ObservabilityStore>,
    dashboard: Cache<TimeRange, (DashboardObservation, Option<DashboardRuntimeSlots>)>,
    summaries: Cache<UsageKey, UsageOverview>,
    insights: Cache<UsageKey, UsageInsightsObservation>,
    metrics: Cache<(UsageKey, UsageInsightMetric), UsageInsightsObservation>,
    diagnostics:
        Cache<(UsageKey, DiagnosticDimension), (DiagnosticObservationPage, AnalyticsMetadata)>,
}
impl CachedObservabilityStore {
    pub(crate) fn new(inner: Arc<dyn ObservabilityStore>) -> Self {
        Self {
            inner,
            dashboard: Cache::default(),
            summaries: Cache::default(),
            insights: Cache::default(),
            metrics: Cache::default(),
            diagnostics: Cache::default(),
        }
    }
}
#[async_trait]
impl ObservabilityStore for CachedObservabilityStore {
    fn query_timeout(&self) -> Duration {
        self.inner.query_timeout()
    }
    async fn analytics_cache_generation(&self) -> AdminStoreResult<u64> {
        self.inner.analytics_cache_generation().await
    }
    async fn dashboard_summary(
        &self,
        range: TimeRange,
        observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<DashboardObservation> {
        self.dashboard_with_runtime_slots(range, observed_at)
            .await
            .map(|(value, _)| value)
    }
    async fn dashboard_with_runtime_slots(
        &self,
        range: TimeRange,
        observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<(DashboardObservation, Option<DashboardRuntimeSlots>)> {
        self.dashboard
            .get(range, self.inner.as_ref(), Duration::from_secs(2), || {
                self.inner.dashboard_with_runtime_slots(range, observed_at)
            })
            .await
    }
    async fn dashboard_runtime_slots(
        &self,
        observed_at: DateTime<Utc>,
    ) -> AdminStoreResult<Option<DashboardRuntimeSlots>> {
        self.inner.dashboard_runtime_slots(observed_at).await
    }
    async fn dashboard_card_metrics(&self, range: TimeRange) -> AdminStoreResult<RequestMetrics> {
        self.inner.dashboard_card_metrics(range).await
    }
    async fn dashboard_trend(&self, range: TimeRange) -> AdminStoreResult<Vec<RequestMetricPoint>> {
        self.inner.dashboard_trend(range).await
    }
    async fn client_key_statistics(
        &self,
        range: TimeRange,
        id: &gateway_core::policy::ClientApiKeyId,
        query: crate::model::observability::KeyUsageMetric,
    ) -> AdminStoreResult<crate::model::observability::KeyStatisticsOverview> {
        self.inner.client_key_statistics(range, id, query).await
    }

    async fn client_key_models(
        &self,
        range: TimeRange,
        id: &gateway_core::policy::ClientApiKeyId,
    ) -> AdminStoreResult<Vec<crate::model::observability::KeyModelRow>> {
        self.inner.client_key_models(range, id).await
    }

    async fn usage_trend(
        &self,
        range: TimeRange,
        filter: UsageFilter,
    ) -> AdminStoreResult<Vec<RequestMetricPoint>> {
        self.inner.usage_trend(range, filter).await
    }
    async fn usage_concurrency(
        &self,
        range: TimeRange,
        filter: UsageFilter,
    ) -> AdminStoreResult<Vec<UsageConcurrencyPoint>> {
        self.inner.usage_concurrency(range, filter).await
    }
    async fn usage_summary(
        &self,
        range: TimeRange,
        filter: UsageFilter,
    ) -> AdminStoreResult<UsageOverview> {
        self.summaries
            .get((range, filter.clone()), self.inner.as_ref(), TTL, || {
                self.inner.usage_summary(range, filter)
            })
            .await
    }
    async fn usage_insights_observation(
        &self,
        range: TimeRange,
        filter: UsageFilter,
    ) -> AdminStoreResult<UsageInsightsObservation> {
        self.insights
            .get((range, filter.clone()), self.inner.as_ref(), TTL, || {
                self.inner.usage_insights_observation(range, filter)
            })
            .await
    }
    async fn usage_metric_observation(
        &self,
        range: TimeRange,
        filter: UsageFilter,
        metric: UsageInsightMetric,
    ) -> AdminStoreResult<UsageInsightsObservation> {
        self.metrics
            .get(
                ((range, filter.clone()), metric),
                self.inner.as_ref(),
                TTL,
                || self.inner.usage_metric_observation(range, filter, metric),
            )
            .await
    }
    fn usage_calculated_billing_facts(
        &self,
        range: TimeRange,
        filter: UsageFilter,
    ) -> crate::ports::store::UsageCalculatedBillingStream<'_> {
        self.inner.usage_calculated_billing_facts(range, filter)
    }
    async fn usage_diagnostics(
        &self,
        range: TimeRange,
        filter: UsageFilter,
        dimension: DiagnosticDimension,
        page: Option<DiagnosticPageQuery>,
    ) -> AdminStoreResult<DiagnosticObservationPage> {
        if dimension == DiagnosticDimension::KeyModel || page.is_some() {
            return self
                .inner
                .usage_diagnostics(range, filter, dimension, page)
                .await;
        }
        let (mut result, metadata) = self
            .diagnostics
            .get(
                ((range, filter.clone()), dimension),
                self.inner.as_ref(),
                TTL,
                || async {
                    let result = self
                        .inner
                        .usage_diagnostics(range, filter, dimension, page)
                        .await?;
                    let metadata = result.analytics.clone().ok_or_else(|| {
                        AdminStoreError::new(
                            AdminStoreErrorKind::Unavailable,
                            "observability cache",
                            "native diagnostics metadata is missing",
                        )
                    })?;
                    Ok((result, metadata))
                },
            )
            .await?;
        result.analytics = Some(metadata);
        Ok(result)
    }
    async fn list_usage_records(&self, query: UsageQuery) -> AdminStoreResult<UsagePage> {
        self.inner.list_usage_records(query).await
    }
    async fn usage_record_detail(&self, request_id: &str) -> AdminStoreResult<UsageDetail> {
        self.inner.usage_record_detail(request_id).await
    }
    async fn list_ops_errors(&self, query: OpsErrorQuery) -> AdminStoreResult<OpsErrorPage> {
        self.inner.list_ops_errors(query).await
    }
    async fn diagnostic_errors(
        &self,
        target: &crate::model::diagnostics::DiagnosticTarget,
        limit: u16,
    ) -> AdminStoreResult<Vec<OpsError>> {
        self.inner.diagnostic_errors(target, limit).await
    }
    async fn account_filter_options(
        &self,
        range: TimeRange,
        search: &str,
    ) -> AdminStoreResult<Vec<AccountFilterOption>> {
        self.inner.account_filter_options(range, search).await
    }
}
