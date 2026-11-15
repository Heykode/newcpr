//! Metric selection follows upstream; native billing remains provider-owned.
use super::*;
use crate::model::observability::{
    UsageCacheEfficiency, UsageCostComparison, UsageCostSummary, UsageInsightData,
    UsageMetricPoint, UsageMetricSeries, UsageThroughputPercentiles,
};

impl DefaultObservabilityService {
    pub(super) async fn load_usage_metric(
        &self,
        range: TimeRange,
        filter: UsageFilter,
        metric: UsageInsightMetric,
    ) -> Result<UsageInsight, AdminError> {
        tokio::time::timeout(
            self.store.query_timeout(),
            self.load_usage_metric_within_budget(range, filter, metric),
        )
        .await
        .map_err(|error| {
            AdminError::new(
                crate::model::AdminErrorKind::Unavailable,
                "usage metric execution budget is exhausted",
            )
            .with_source(error)
        })?
    }

    async fn load_usage_metric_within_budget(
        &self,
        range: TimeRange,
        filter: UsageFilter,
        metric: UsageInsightMetric,
    ) -> Result<UsageInsight, AdminError> {
        let observation = self
            .store
            .usage_metric_observation(range, filter.clone(), metric)
            .await
            .map_err(|error| map_store_error(error, "usage metric"))?;
        let costs = projected_cost_scenarios(observation.costs)?;
        let insights = build_usage_insights(
            observation.overview,
            observation.trend,
            costs,
            range,
            observation.concurrency,
        )?;
        Ok(select_usage_metric(insights, metric))
    }
}

fn select_usage_metric(insights: UsageInsights, metric: UsageInsightMetric) -> UsageInsight {
    let activity = insights
        .health
        .points
        .iter()
        .map(|point| (point.bucket_start, point.total_requests))
        .collect::<BTreeMap<_, _>>();
    let data = match metric {
        UsageInsightMetric::Health => UsageInsightData::Health(insights.health),
        UsageInsightMetric::Latency => UsageInsightData::Latency(metric_series(
            insights.performance.latency_percentiles,
            insights
                .performance
                .points
                .into_iter()
                .map(|point| (point.bucket_start, point.latency_percentiles)),
            &activity,
        )),
        UsageInsightMetric::FirstToken => UsageInsightData::FirstToken(metric_series(
            insights.performance.first_token_latency_percentiles,
            insights
                .performance
                .points
                .into_iter()
                .map(|point| (point.bucket_start, point.first_token_latency_percentiles)),
            &activity,
        )),
        UsageInsightMetric::Throughput => UsageInsightData::Throughput(metric_series(
            UsageThroughputPercentiles {
                p10: insights.performance.output_throughput_p10,
                p50: insights.performance.output_throughput_p50,
                p90: insights.performance.output_throughput_p90,
            },
            insights.performance.points.into_iter().map(|point| {
                (
                    point.bucket_start,
                    UsageThroughputPercentiles {
                        p10: point.output_throughput_p10,
                        p50: point.output_throughput_p50,
                        p90: point.output_throughput_p90,
                    },
                )
            }),
            &activity,
        )),
        UsageInsightMetric::Load => UsageInsightData::Load(metric_series(
            insights.performance.load,
            insights
                .performance
                .points
                .into_iter()
                .map(|point| (point.bucket_start, point.load)),
            &activity,
        )),
        UsageInsightMetric::Scheduling => {
            UsageInsightData::Scheduling(Box::new(insights.performance))
        }
        UsageInsightMetric::Cost => UsageInsightData::Cost(metric_series(
            cost_summary(&insights.cost),
            insights.cost.points.into_iter().map(|point| {
                (
                    point.bucket_start,
                    UsageCostComparison {
                        estimated_cost: point.estimated_cost,
                        no_cache_cost: point.no_cache_cost,
                    },
                )
            }),
            &activity,
        )),
        UsageInsightMetric::Savings => UsageInsightData::Savings(metric_series(
            cost_summary(&insights.cost),
            insights
                .cost
                .points
                .into_iter()
                .map(|point| (point.bucket_start, point.cache_savings)),
            &activity,
        )),
        UsageInsightMetric::Cache => UsageInsightData::Cache(metric_series(
            cost_summary(&insights.cost),
            insights.cost.points.into_iter().map(|point| {
                (
                    point.bucket_start,
                    UsageCacheEfficiency {
                        input_tokens: point.input_tokens,
                        cached_token_rate: point.cached_token_rate,
                        cache_hit_request_rate: point.cache_hit_request_rate,
                    },
                )
            }),
            &activity,
        )),
    };
    UsageInsight {
        analytics: insights.analytics,
        granularity: insights.granularity,
        data,
    }
}

fn cost_summary(cost: &UsageInsightsCost) -> UsageCostSummary {
    UsageCostSummary {
        estimated_cost: cost.estimated_cost.clone(),
        cache_savings: cost.cache_savings.clone(),
        tier_premium: cost.tier_premium.clone(),
        cost_per_successful_request: cost.cost_per_successful_request.clone(),
        coverage: cost.coverage.clone(),
    }
}

fn metric_series<S, P>(
    summary: S,
    points: impl Iterator<Item = (DateTime<Utc>, P)>,
    activity: &BTreeMap<DateTime<Utc>, u64>,
) -> UsageMetricSeries<S, P> {
    UsageMetricSeries {
        summary,
        points: points
            .map(|(bucket_start, value)| UsageMetricPoint {
                bucket_start,
                request_count: activity.get(&bucket_start).copied().unwrap_or_default(),
                value,
            })
            .collect(),
    }
}
