//! 账号额度的只读容量估算，不参与金额结算或账号调度。

use chrono::{DateTime, Duration, Utc};

use super::provider_credentials::{AccountUsagePeriod, ProviderQuota, ProviderQuotaWindow};
use super::quota_forecast_sampling::{QuotaForecastMethod, QuotaForecastSample};

const DAY_SECONDS: u64 = 86_400;
const MIN_USED_PERCENT: f64 = 5.0;
const LOW_SAMPLE_PERCENT: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurrentQuotaEstimate {
    pub total_usd: f64,
    pub remaining_usd: f64,
    pub incomplete_cost: bool,
}

/// 复用同一窗口的消费与百分比；不读取个人学习值或 Plan 默认值。
#[must_use]
pub fn current_window_estimate(
    window: &ProviderQuotaWindow,
    now: DateTime<Utc>,
) -> Option<CurrentQuotaEstimate> {
    if window.local_usage_attribution
        != super::provider_credentials::QuotaLocalUsageAttribution::AccountWide
        || window.reset_at? <= now
    {
        return None;
    }
    let seconds = i64::try_from(window.window_seconds?).ok()?;
    let start = window
        .reset_at?
        .checked_sub_signed(Duration::try_seconds(seconds)?)?;
    if seconds <= 0 || start > now {
        return None;
    }
    let percent = window.used_percent?;
    let usage = window.local_usage.as_ref()?;
    let cost = usage
        .costs
        .iter()
        .find(|cost| cost.currency.eq_ignore_ascii_case("USD"))?
        .amount
        .to_string()
        .parse::<f64>()
        .ok()?;
    if !percent.is_finite() || percent <= 0.0 || !cost.is_finite() || cost <= 0.0 {
        return None;
    }
    let total_usd = cost * 100.0 / percent;
    let remaining_usd = (total_usd - cost).max(0.0);
    (total_usd.is_finite() && total_usd > 0.0 && remaining_usd.is_finite()).then_some(
        CurrentQuotaEstimate {
            total_usd,
            remaining_usd,
            incomplete_cost: usage.cost_coverage.unavailable_count > 0,
        },
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountQuotaForecastReport {
    pub account_id: String,
    pub generated_at: DateTime<Utc>,
    pub forecasts: [AccountQuotaForecast; 2],
    /// Internal monitor inputs; not additional weekly/monthly API balances.
    pub learned_windows: Vec<super::quota_learning::LearnedQuotaWindow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountQuotaForecast {
    pub period: AccountUsagePeriod,
    pub target_seconds: u64,
    pub extrapolated: bool,
    pub source: Option<QuotaForecastSource>,
    pub unavailable_reason: Option<&'static str>,
    pub low_sample: bool,
    pub incomplete_cost: bool,
    pub incomplete_tokens: bool,
    pub estimated_tokens: Option<u64>,
    pub estimated_usd: Option<f64>,
    /// 剩余估算始终属于源窗口，不随目标周期折算。
    pub remaining_tokens: Option<u64>,
    pub remaining_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaForecastSource {
    pub label: String,
    pub used_percent: Option<f64>,
    pub observed_at: Option<DateTime<Utc>>,
    pub reset_at: DateTime<Utc>,
    pub tokens: Option<u64>,
    pub usd: Option<f64>,
}

/// 优先预测真实的对应窗口；缺少对应周期时只给出明确标识的 7/30 天容量折算。
/// 本地日志不能证明站外消耗或完整留存，因此即使样本充足也不声称官方额度。
#[must_use]
pub fn account_quota_forecasts(
    quota: &ProviderQuota,
    account_added_at: DateTime<Utc>,
    now: DateTime<Utc>,
    samples: &[QuotaForecastSample],
) -> [AccountQuotaForecast; 2] {
    [AccountUsagePeriod::Weekly, AccountUsagePeriod::Monthly].map(|period| {
        let selected = forecast_window(quota, period);
        let mut forecast = AccountQuotaForecast {
            period,
            target_seconds: match period {
                AccountUsagePeriod::Weekly => 7 * DAY_SECONDS,
                AccountUsagePeriod::Monthly => 30 * DAY_SECONDS,
            },
            extrapolated: false,
            source: None,
            unavailable_reason: Some("没有可统计的周/月额度窗口，请先刷新账号额度。"),
            low_sample: false,
            incomplete_cost: false,
            incomplete_tokens: false,
            estimated_tokens: None,
            estimated_usd: None,
            remaining_tokens: None,
            remaining_usd: None,
        };
        if let Some((window, source_period)) = selected {
            forecast.project(
                window,
                source_period,
                quota.observed_at,
                account_added_at,
                now,
                samples.iter().find(|sample| sample.key == window.key),
            );
        }
        forecast
    })
}

pub(crate) fn forecast_window(
    quota: &ProviderQuota,
    period: AccountUsagePeriod,
) -> Option<(&ProviderQuotaWindow, AccountUsagePeriod)> {
    quota
        .usage_windows()
        .filter(|(window, _)| window.local_usage.is_some())
        .find(|(_, source_period)| *source_period == period)
        .or_else(|| quota.usage_window())
        .or_else(|| {
            quota
                .usage_windows()
                .find(|(_, source_period)| *source_period == period)
        })
        .or_else(|| quota.usage_windows().min_by_key(|(_, period)| *period))
}

impl AccountQuotaForecast {
    fn project(
        &mut self,
        window: &ProviderQuotaWindow,
        source_period: AccountUsagePeriod,
        observed_at: Option<DateTime<Utc>>,
        account_added_at: DateTime<Utc>,
        now: DateTime<Utc>,
        sample: Option<&QuotaForecastSample>,
    ) {
        let (Some(seconds), Some(reset_at)) = (window.window_seconds, window.reset_at) else {
            return;
        };
        self.extrapolated = source_period != self.period;
        if !self.extrapolated {
            self.target_seconds = seconds;
        }
        let percent = window
            .used_percent
            .filter(|p| p.is_finite() && (0.0..=100.0).contains(p));
        let usage = sample.map(|sample| &sample.usage);
        let tokens = usage
            .filter(|usage| usage.tokens > 0 || usage.missing_token_count < usage.request_count)
            .map(|usage| usage.tokens);
        let usd = usage
            .filter(|usage| usage.known_cost_count > 0)
            .map(|usage| usage.usd)
            .filter(|value| value.is_finite() && *value >= 0.0);
        let start = i64::try_from(seconds)
            .ok()
            .and_then(Duration::try_seconds)
            .and_then(|duration| reset_at.checked_sub_signed(duration));
        self.source = Some(QuotaForecastSource {
            label: window.label.clone(),
            used_percent: percent,
            observed_at,
            reset_at,
            tokens,
            usd,
        });
        self.incomplete_cost = usage.is_none_or(|usage| {
            usage.unavailable_cost_count > 0
                || usage.known_cost_count == 0
                || usage.known_cost_count != usage.request_count
        });
        self.incomplete_tokens = usage.is_some_and(|usage| usage.missing_token_count > 0);
        let method = sample.map_or(QuotaForecastMethod::Cumulative, |sample| sample.method);
        let Some(start) = start.filter(|start| *start <= now && now < reset_at) else {
            self.unavailable_reason = Some("额度窗口已过期或边界无效，请刷新账号额度后重试。");
            return;
        };
        if !observed_at.is_some_and(|observed| start <= observed && observed <= now) {
            self.unavailable_reason = Some("缺少本周期的额度快照，请先刷新账号额度。");
            return;
        }
        if sample.is_some_and(|sample| sample.discontinuous) {
            self.unavailable_reason =
                Some("额度观测出现回落或累计记录不连续，正在重新积累配对样本。");
            return;
        }
        if account_added_at > start && method == QuotaForecastMethod::Cumulative {
            self.unavailable_reason = Some(
                "本周期开始时的记录不完整，正在积累至少 5 个百分点的配对观测，无需等待下次重置。",
            );
            return;
        }
        let Some(percent) = percent else {
            self.unavailable_reason = Some("已用比例未知，请刷新额度后查看预测。");
            return;
        };
        if usage.is_none_or(|usage| usage.request_count == 0) {
            self.unavailable_reason = Some("本周期没有网关用量记录，暂时无法预测额度。");
            return;
        }
        let Some(sample) = sample.filter(|sample| {
            sample.end_at == observed_at.unwrap_or(now)
                && start <= sample.start_at
                && sample.start_at <= sample.end_at
                && sample.sampled_percent.is_finite()
                && sample.sampled_percent >= MIN_USED_PERCENT
                && sample.sampled_percent <= percent
        }) else {
            self.unavailable_reason =
                Some("有效额度进度不足 5 个百分点或采样边界无效，请继续积累用量。");
            return;
        };
        self.low_sample = sample.sampled_percent < LOW_SAMPLE_PERCENT
            || (method == QuotaForecastMethod::Incremental && sample.block_count < 2);
        // 漏记和个别缺失只影响精度，仍按已记录数值估算，不按请求数补齐未知消耗。
        // 预测是近似展示值；不复用为账单金额，也不把月折算当成自然月或额外余额。
        let capacity_factor = 100.0 / sample.sampled_percent;
        let factor = capacity_factor * self.target_seconds as f64 / seconds as f64;
        let tokens = tokens.filter(|tokens| *tokens > 0);
        self.estimated_tokens = tokens.and_then(|value| estimate_tokens(value, factor));
        self.estimated_usd = usd.and_then(|value| estimate(value, factor));
        let remaining_factor = (100.0 - percent) / sample.sampled_percent;
        self.remaining_tokens = tokens.and_then(|value| estimate_tokens(value, remaining_factor));
        self.remaining_usd = usd.and_then(|value| estimate(value, remaining_factor));
        self.unavailable_reason = if self.estimated_tokens.is_none() && self.estimated_usd.is_none()
        {
            Some("本周期暂无可用于估算的 Token 或费用数据，请积累用量后重试。")
        } else {
            None
        };
    }
}

fn estimate(value: f64, factor: f64) -> Option<f64> {
    let estimate = value * factor;
    (estimate.is_finite() && estimate >= 0.0).then_some(estimate)
}

fn estimate_tokens(value: u64, factor: f64) -> Option<u64> {
    estimate(value as f64, factor)
        .filter(|value| value.round() < u64::MAX as f64)
        .map(|value| value.round() as u64)
}
