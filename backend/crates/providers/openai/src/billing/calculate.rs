//! OpenAI Token、缓存、托管工具和服务档位的费用计算

use gateway_core::metering::{
    CalculatedCostAmounts, CalculatedCostBreakdown, CalculatedCostRates, CurrencyCode, Decimal,
    Money,
};
use gateway_protocol::openai::events::TokenUsage;
use serde_json::Value;

use super::catalog::{
    ModelPricing, PricingTier, TokenRates, cache_write_rate, model_matches_rule, model_pricing,
    normalize_model_name, pricing_model_name,
};

const LONG_CONTEXT_THRESHOLD: u64 = 272_000;
const WEB_SEARCH_CALL_TICKS: u128 = 100_000_000;
const WEB_SEARCH_PREVIEW_NON_REASONING_CALL_TICKS: u128 = 250_000_000;
const FILE_SEARCH_CALL_TICKS: u128 = 25_000_000;

/// OpenAI 公开 Token 价格计算所需的单次用量事实
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenAiBillingUsage {
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    cache_write_tokens: u64,
    image_input_tokens: u64,
    image_output_tokens: u64,
    web_search_calls: u64,
    file_search_calls: u64,
    web_search_pricing: Option<WebSearchPricing>,
}

impl OpenAiBillingUsage {
    /// 构造不含托管工具费用的 Token 用量
    #[must_use]
    pub const fn new(
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
        cache_write_tokens: u64,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cached_tokens,
            cache_write_tokens,
            image_input_tokens: 0,
            image_output_tokens: 0,
            web_search_calls: 0,
            file_search_calls: 0,
            web_search_pricing: None,
        }
    }

    pub(crate) const fn with_web_search_calls(
        mut self,
        calls: u64,
        pricing: Option<WebSearchPricing>,
    ) -> Self {
        self.web_search_calls = calls;
        self.web_search_pricing = pricing;
        self
    }

    pub(crate) const fn with_file_search_calls(mut self, calls: u64) -> Self {
        self.file_search_calls = calls;
        self
    }
}

impl From<TokenUsage> for OpenAiBillingUsage {
    fn from(usage: TokenUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_tokens: usage.cached_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            image_input_tokens: usage.image_input_tokens,
            image_output_tokens: usage.image_output_tokens,
            web_search_calls: 0,
            file_search_calls: 0,
            web_search_pricing: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebSearchPricing {
    Standard,
    PreviewNonReasoning,
}

impl WebSearchPricing {
    const fn price_per_call_ticks(self) -> u128 {
        match self {
            Self::Standard => WEB_SEARCH_CALL_TICKS,
            Self::PreviewNonReasoning => WEB_SEARCH_PREVIEW_NON_REASONING_CALL_TICKS,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct TokenAmounts {
    pub(super) input_ticks: u128,
    pub(super) output_ticks: u128,
    pub(super) cache_read_ticks: u128,
    pub(super) cache_write_ticks: u128,
    pub(super) total_ticks: u128,
}

/// 按 OpenAI Provider 当前受控价格规则计算费用明细
#[must_use]
pub fn openai_billing_breakdown(
    model: &str,
    usage: OpenAiBillingUsage,
    service_tier: Option<&str>,
) -> Option<CalculatedCostBreakdown> {
    token_billing_breakdown_with_context(
        model_pricing(model),
        usage,
        service_tier,
        usage.input_tokens > LONG_CONTEXT_THRESHOLD,
        None,
    )
}

/// 使用请求开始时冻结的价格覆盖，不读取管理存储
#[must_use]
pub(crate) fn openai_billing_breakdown_with_override(
    model: &str,
    usage: OpenAiBillingUsage,
    service_tier: Option<&str>,
    pricing: Option<&gateway_core::metering::ModelPriceOverride>,
) -> Option<CalculatedCostBreakdown> {
    token_billing_breakdown_with_context(
        model_pricing(model),
        usage,
        service_tier,
        usage.input_tokens > LONG_CONTEXT_THRESHOLD,
        pricing,
    )
}

fn token_billing_breakdown_with_context(
    pricing: Option<ModelPricing>,
    usage: OpenAiBillingUsage,
    service_tier: Option<&str>,
    long_context: bool,
    custom: Option<&gateway_core::metering::ModelPriceOverride>,
) -> Option<CalculatedCostBreakdown> {
    if usage.cached_tokens.checked_add(usage.cache_write_tokens)? > usage.input_tokens
        || usage.image_input_tokens > 0
        || usage.image_output_tokens > 0
    {
        return None;
    }
    let tool_ticks = web_search_amount_ticks(usage)?
        .checked_add(u128::from(usage.file_search_calls).checked_mul(FILE_SEARCH_CALL_TICKS)?)?;
    let normalized_tier = normalize_service_tier(service_tier);
    let tier = pricing_tier(normalized_tier.as_deref())?;
    let (standard_rates, _) =
        effective_rates(pricing, custom, PricingTier::Standard, long_context)?;
    let (selected_rates, long_context_billing_applied) =
        effective_rates(pricing, custom, tier, long_context)?;
    let cache_write_percent = pricing.map_or(0, |pricing| pricing.cache_write_percent);
    let mut standard = token_amounts(
        standard_rates,
        cache_write_percent,
        usage.input_tokens,
        usage.output_tokens,
        usage.cached_tokens,
        usage.cache_write_tokens,
    )?;
    let mut selected = token_amounts(
        selected_rates,
        cache_write_percent,
        usage.input_tokens,
        usage.output_tokens,
        usage.cached_tokens,
        usage.cache_write_tokens,
    )?;
    standard.total_ticks = standard.total_ticks.checked_add(tool_ticks)?;
    selected.total_ticks = selected.total_ticks.checked_add(tool_ticks)?;
    let multiplier_percent =
        effective_multiplier_percent(selected.total_ticks, standard.total_ticks)?;
    let cache_write_rate = cache_write_rate(selected_rates, cache_write_percent)?;

    CalculatedCostBreakdown::new(
        CalculatedCostAmounts::new(
            usd_money(selected.input_ticks)?,
            usd_money(selected.output_ticks)?,
            usd_money(selected.cache_read_ticks)?,
            usd_money(selected.cache_write_ticks)?,
            usd_money(standard.total_ticks)?,
            usd_money(selected.total_ticks)?,
        ),
        CalculatedCostRates::new(
            usd_price_per_million(selected_rates.input_ticks)?,
            usd_price_per_million(selected_rates.output_ticks)?,
            usd_price_per_million(selected_rates.cache_read_ticks)?,
            usd_price_per_million(cache_write_rate)?,
        ),
        Some(normalized_tier.unwrap_or_else(|| "default".to_owned())),
        multiplier_percent,
    )
    .with_long_context_billing(long_context_billing_applied)
    .with_custom_multiplier(custom.map_or(10_000, |pricing| pricing.multiplier_bps))
}

fn effective_rates(
    default: Option<ModelPricing>,
    custom: Option<&gateway_core::metering::ModelPriceOverride>,
    tier: PricingTier,
    long_context: bool,
) -> Option<(TokenRates, bool)> {
    let long_band = match tier {
        PricingTier::Standard => "long_standard",
        PricingTier::Fast => "long_fast",
        PricingTier::Flex => "long_flex",
        PricingTier::Ultrafast => "long_ultrafast",
    };
    let uses_long = long_context
        && (default.is_some_and(|p| p.long_standard.is_configured() || p.unpriced_long_context)
            || custom.is_some_and(|p| p.bands.contains_key(long_band)));
    let band = match (tier, uses_long) {
        (PricingTier::Standard, false) => "standard",
        (PricingTier::Fast, false) => "fast",
        (PricingTier::Flex, false) => "flex",
        (PricingTier::Ultrafast, false) => "ultrafast",
        (PricingTier::Standard, true) => "long_standard",
        (PricingTier::Fast, true) => "long_fast",
        (PricingTier::Flex, true) => "long_flex",
        (PricingTier::Ultrafast, true) => "long_ultrafast",
    };
    if let Some(rates) = custom.and_then(|p| p.bands.get(band)) {
        return Some((
            TokenRates {
                input_ticks: rates.input.ticks_per_token(),
                output_ticks: rates.output.ticks_per_token(),
                cache_read_ticks: rates.cache_read.ticks_per_token(),
                cache_write_ticks: Some(rates.cache_write.ticks_per_token()),
                explicit_cache: true,
            },
            uses_long,
        ));
    }
    let default = default?;
    Some((
        default.rates(tier, long_context)?,
        long_context && default.long_standard.is_configured(),
    ))
}

pub(crate) fn web_search_pricing(model: &str, tools: Option<&[Value]>) -> Option<WebSearchPricing> {
    let mut standard = false;
    let mut preview = false;
    for tool_type in tools
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("type").and_then(Value::as_str))
    {
        if tool_type == "web_search_preview" || tool_type.starts_with("web_search_preview_") {
            preview = true;
        } else if tool_type == "web_search" || tool_type.starts_with("web_search_") {
            standard = true;
        }
    }
    match (standard, preview) {
        // 这两个模型将非预览搜索内容按固定 8K 输入块计费
        // 响应缺少足够明细，无法排除与普通输入重复计费，因此保留为未定价
        (true, false) if fixed_block_web_search_model(model) => None,
        (true, false) => Some(WebSearchPricing::Standard),
        (false, true) if reasoning_model(model) => Some(WebSearchPricing::Standard),
        (false, true) => Some(WebSearchPricing::PreviewNonReasoning),
        (false, false) | (true, true) => None,
    }
}

fn fixed_block_web_search_model(model: &str) -> bool {
    let normalized = normalize_model_name(model);
    ["gpt-4o-mini", "gpt-4.1-mini"]
        .iter()
        .any(|rule| model_matches_rule(&normalized, rule))
}

fn reasoning_model(model: &str) -> bool {
    let normalized = normalize_model_name(model);
    let model = pricing_model_name(&normalized);
    model.starts_with("gpt-5")
        || matches!(
            model,
            "gpt-6.2-astra"
                | "gpt-6.2-sol"
                | "gpt-6-astra"
                | "gpt-6.1-sol"
                | "gpt-6.1-terra"
                | "gpt-6-sol"
                | "gpt-6-luna"
        )
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
}

fn pricing_tier(service_tier: Option<&str>) -> Option<PricingTier> {
    match service_tier {
        None | Some("default" | "standard") => Some(PricingTier::Standard),
        Some("flex") => Some(PricingTier::Flex),
        Some("fast" | "priority") => Some(PricingTier::Fast),
        Some("ultrafast") => Some(PricingTier::Ultrafast),
        Some(_) => None,
    }
}

fn web_search_amount_ticks(usage: OpenAiBillingUsage) -> Option<u128> {
    if usage.web_search_calls == 0 {
        return Some(0);
    }
    u128::from(usage.web_search_calls).checked_mul(usage.web_search_pricing?.price_per_call_ticks())
}

pub(super) fn token_amounts(
    rates: TokenRates,
    cache_write_percent: u32,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    cache_write_tokens: u64,
) -> Option<TokenAmounts> {
    if cached_tokens.checked_add(cache_write_tokens)? > input_tokens {
        return None;
    }
    let billed_cache_read = if rates.explicit_cache || rates.cache_read_ticks > 0 {
        cached_tokens
    } else {
        0
    };
    let cache_write_rate = cache_write_rate(rates, cache_write_percent)?;
    let billed_cache_write = if rates.explicit_cache || cache_write_rate > 0 {
        cache_write_tokens
    } else {
        0
    };
    let uncached_input = input_tokens
        .checked_sub(billed_cache_read)?
        .checked_sub(billed_cache_write)?;
    let input_ticks = u128::from(uncached_input).checked_mul(rates.input_ticks)?;
    let output_ticks = u128::from(output_tokens).checked_mul(rates.output_ticks)?;
    let cache_read_ticks = u128::from(billed_cache_read).checked_mul(rates.cache_read_ticks)?;
    let cache_write_ticks = u128::from(billed_cache_write).checked_mul(cache_write_rate)?;
    let total_ticks = input_ticks
        .checked_add(output_ticks)?
        .checked_add(cache_read_ticks)?
        .checked_add(cache_write_ticks)?;
    Some(TokenAmounts {
        input_ticks,
        output_ticks,
        cache_read_ticks,
        cache_write_ticks,
        total_ticks,
    })
}

fn effective_multiplier_percent(total: u128, standard: u128) -> Option<u32> {
    if standard == 0 {
        return Some(100);
    }
    let rounded = total
        .checked_mul(100)?
        .checked_add(standard / 2)?
        .checked_div(standard)?;
    u32::try_from(rounded).ok()
}

/// 规范化请求或响应携带的服务档位，供观测与计费共用
pub(crate) fn normalize_service_tier(service_tier: Option<&str>) -> Option<String> {
    service_tier
        .map(str::trim)
        .filter(|value| {
            !value.is_empty() && value.len() <= 64 && !value.chars().any(char::is_control)
        })
        .map(str::to_ascii_lowercase)
}

pub(super) fn usd_money(ticks: u128) -> Option<Money> {
    Some(Money::new(
        Decimal::from_scaled(ticks).ok()?,
        CurrencyCode::new("USD").ok()?,
    ))
}

pub(super) fn usd_price_per_million(per_token_ticks: u128) -> Option<Money> {
    usd_money(per_token_ticks.checked_mul(1_000_000)?)
}

#[cfg(test)]
mod tests {
    use crate::billing::{
        OpenAiBillingUsage, openai_billing_breakdown, openai_billing_breakdown_with_override,
    };
    use gateway_core::metering::ModelPriceOverride;
    use serde_json::json;

    fn billing_usage(
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_write_input_tokens: u64,
    ) -> OpenAiBillingUsage {
        OpenAiBillingUsage::new(
            input_tokens,
            output_tokens,
            cached_input_tokens,
            cache_write_input_tokens,
        )
    }

    #[test]
    fn astra_billing_should_preserve_components_across_tiers_and_context_boundary() {
        // 每百万 token 的美元价格，依次为输入、缓存读取、缓存写入与输出
        for (tier, input, expected, multiplier) in [
            (None, 272_000, ["10", "1", "12.5", "50"], 100),
            (None, 272_001, ["20", "2", "25", "75"], 100),
            (Some("flex"), 272_000, ["5", "0.5", "6.25", "25"], 50),
            (Some("flex"), 272_001, ["10", "1", "12.5", "37.5"], 50),
            (Some("fast"), 272_000, ["20", "2", "25", "100"], 200),
            (Some("fast"), 272_001, ["40", "4", "50", "150"], 200),
            (Some("priority"), 272_001, ["40", "4", "50", "150"], 200),
        ] {
            let breakdown =
                openai_billing_breakdown("gpt-6-astra", billing_usage(input, 5, 20, 10), tier)
                    .expect("published Astra pricing");
            let prices = [
                breakdown.input_price_per_million(),
                breakdown.cache_read_price_per_million(),
                breakdown.cache_write_price_per_million(),
                breakdown.output_price_per_million(),
            ]
            .map(|price| {
                price
                    .amount()
                    .to_string()
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_owned()
            });
            assert_eq!(prices, expected, "tier={tier:?}, input={input}");
            assert_eq!(breakdown.multiplier_percent(), multiplier);
        }
        let breakdown =
            openai_billing_breakdown("gpt-6-astra", billing_usage(100, 5, 20, 10), None)
                .expect("Astra component pricing");
        assert_eq!(
            (
                breakdown.input_amount().amount().scaled(),
                breakdown.output_amount().amount().scaled(),
                breakdown.cache_read_amount().amount().scaled(),
                breakdown.cache_write_amount().amount().scaled(),
                breakdown.total_amount().amount().scaled(),
            ),
            (7_000_000, 2_500_000, 200_000, 1_250_000, 10_950_000),
        );
    }

    #[test]
    fn gpt_6_models_should_use_published_prices_across_tiers_and_context_boundary() {
        // 每档依次核对每百万 Token 的输入、缓存读取、缓存写入和输出价格
        for (model, cases) in [
            (
                "gpt-6.2-astra",
                [
                    (None, 272_000, ["8", "0.4", "10", "40"]),
                    (None, 272_001, ["16", "0.8", "20", "60"]),
                    (Some("fast"), 272_000, ["16", "0.8", "20", "80"]),
                    (Some("priority"), 272_001, ["32", "1.6", "40", "120"]),
                    (Some("ultrafast"), 272_000, ["48", "2.4", "60", "240"]),
                    (Some("ultrafast"), 272_001, ["96", "4.8", "120", "360"]),
                ],
            ),
            (
                "gpt-6.2-sol",
                [
                    (None, 272_000, ["2", "0.1", "2.5", "10"]),
                    (None, 272_001, ["4", "0.2", "5", "15"]),
                    (Some("fast"), 272_000, ["4", "0.2", "5", "20"]),
                    (Some("priority"), 272_001, ["8", "0.4", "10", "30"]),
                    (Some("ultrafast"), 272_000, ["12", "0.6", "15", "60"]),
                    (Some("ultrafast"), 272_001, ["24", "1.2", "30", "90"]),
                ],
            ),
            (
                "gpt-6.1-sol",
                [
                    (None, 272_000, ["2", "0.1", "2.5", "10"]),
                    (None, 272_001, ["4", "0.2", "5", "15"]),
                    (Some("flex"), 272_000, ["1", "0.05", "1.25", "5"]),
                    (Some("flex"), 272_001, ["2", "0.1", "2.5", "7.5"]),
                    (Some("fast"), 272_000, ["4", "0.2", "5", "20"]),
                    (Some("priority"), 272_001, ["8", "0.4", "10", "30"]),
                ],
            ),
            (
                "gpt-6.1-terra",
                [
                    (None, 272_000, ["1", "0.05", "1.25", "5"]),
                    (None, 272_001, ["2", "0.1", "2.5", "7.5"]),
                    (Some("flex"), 272_000, ["0.5", "0.025", "0.625", "2.5"]),
                    (Some("flex"), 272_001, ["1", "0.05", "1.25", "3.75"]),
                    (Some("fast"), 272_000, ["2", "0.1", "2.5", "10"]),
                    (Some("priority"), 272_001, ["4", "0.2", "5", "15"]),
                ],
            ),
            (
                "gpt-6-sol",
                [
                    (None, 272_000, ["2", "0.2", "2.5", "10"]),
                    (None, 272_001, ["4", "0.4", "5", "15"]),
                    (Some("flex"), 272_000, ["1", "0.1", "1.25", "5"]),
                    (Some("flex"), 272_001, ["2", "0.2", "2.5", "7.5"]),
                    (Some("fast"), 272_000, ["4", "0.4", "5", "20"]),
                    (Some("priority"), 272_001, ["8", "0.8", "10", "30"]),
                ],
            ),
            (
                "gpt-6-luna",
                [
                    (None, 272_000, ["0.1", "0.01", "0.125", "0.5"]),
                    (None, 272_001, ["0.2", "0.02", "0.25", "0.75"]),
                    (Some("flex"), 272_000, ["0.05", "0.005", "0.0625", "0.25"]),
                    (Some("flex"), 272_001, ["0.1", "0.01", "0.125", "0.375"]),
                    (Some("fast"), 272_000, ["0.2", "0.02", "0.25", "1"]),
                    (Some("priority"), 272_001, ["0.4", "0.04", "0.5", "1.5"]),
                ],
            ),
        ] {
            for (tier, input, expected) in cases {
                let breakdown =
                    openai_billing_breakdown(model, billing_usage(input, 5, 20, 10), tier)
                        .expect("published GPT-6 pricing");
                let actual = [
                    breakdown.input_price_per_million(),
                    breakdown.cache_read_price_per_million(),
                    breakdown.cache_write_price_per_million(),
                    breakdown.output_price_per_million(),
                ]
                .map(|price| price.amount().canonical());
                assert_eq!(actual, expected, "{model} {tier:?} {input}");
                assert_eq!(
                    breakdown.long_context_billing_applied(),
                    input > 272_000,
                    "{model} {tier:?} {input}"
                );
            }
        }
    }

    #[test]
    fn ultrafast_billing_uses_published_prices_across_the_context_boundary() {
        // 每档依次核对每百万 Token 的输入、缓存读取、缓存写入和输出价格
        for (model, input, expected, total) in [
            ("gpt-6-astra", 272_000, ["60", "6", "75", "300"], "16.32057"),
            (
                "gpt-6-astra",
                272_001,
                ["120", "12", "150", "450"],
                "32.64051",
            ),
            (
                "gpt-6.1-sol",
                272_000,
                ["12", "0.6", "15", "60"],
                "3.264102",
            ),
            (
                "gpt-6.1-sol",
                272_001,
                ["24", "1.2", "30", "90"],
                "6.528078",
            ),
        ] {
            let breakdown =
                openai_billing_breakdown(model, billing_usage(input, 5, 20, 10), Some("ultrafast"))
                    .expect("published Ultrafast pricing");
            let prices = [
                breakdown.input_price_per_million(),
                breakdown.cache_read_price_per_million(),
                breakdown.cache_write_price_per_million(),
                breakdown.output_price_per_million(),
            ]
            .map(|price| price.amount().canonical());
            assert_eq!(prices, expected, "{model} {input}");
            assert_eq!(breakdown.total_amount().amount().canonical(), total);
            assert_eq!(breakdown.service_tier(), Some("ultrafast"));
            assert_eq!(breakdown.multiplier_percent(), 600);
            assert_eq!(breakdown.long_context_billing_applied(), input > 272_000);
        }
    }

    #[test]
    fn billing_breakdown_should_preserve_input_output_and_cache_components() {
        let breakdown =
            openai_billing_breakdown("gpt-5.6-sol", billing_usage(100, 5, 20, 10), None)
                .expect("known model pricing");

        assert_eq!(breakdown.input_amount().amount().scaled(), 3_500_000);
        assert_eq!(breakdown.output_amount().amount().scaled(), 1_500_000);
        assert_eq!(breakdown.cache_read_amount().amount().scaled(), 100_000);
        assert_eq!(breakdown.cache_write_amount().amount().scaled(), 625_000);
        assert_eq!(breakdown.total_amount().amount().scaled(), 5_725_000);
        assert_eq!(breakdown.service_tier(), Some("default"));
        assert_eq!(breakdown.multiplier_percent(), 100);
    }

    #[test]
    fn billing_breakdown_should_use_latest_gpt_5_6_and_cached_input_prices() {
        let terra = openai_billing_breakdown("gpt-5.6-terra", billing_usage(1, 1, 0, 0), None)
            .expect("gpt-5.6-terra standard pricing");
        assert_eq!(terra.total_amount().amount().scaled(), 140_000);

        let terra_fast =
            openai_billing_breakdown("gpt-5.6-terra", billing_usage(1, 1, 0, 0), Some("fast"))
                .expect("gpt-5.6-terra fast pricing");
        assert_eq!(terra_fast.total_amount().amount().scaled(), 280_000);

        let terra_long =
            openai_billing_breakdown("gpt-5.6-terra", billing_usage(272_001, 0, 0, 0), None)
                .expect("gpt-5.6-terra long-context pricing");
        assert_eq!(terra_long.total_amount().amount().scaled(), 10_880_040_000);

        let luna = openai_billing_breakdown("gpt-5.6-luna", billing_usage(1, 1, 0, 0), None)
            .expect("gpt-5.6-luna standard pricing");
        assert_eq!(luna.total_amount().amount().scaled(), 14_000);

        let gpt_4o = openai_billing_breakdown("gpt-4o", billing_usage(1, 1, 1, 0), None)
            .expect("gpt-4o cached-input pricing");
        assert_eq!(gpt_4o.total_amount().amount().scaled(), 112_500);

        let gpt_4o_mini = openai_billing_breakdown("gpt-4o-mini", billing_usage(1, 1, 1, 0), None)
            .expect("gpt-4o-mini cached-input pricing");
        assert_eq!(gpt_4o_mini.total_amount().amount().scaled(), 6_750);
    }

    #[test]
    fn billing_breakdown_should_apply_fast_and_flex_tiers_without_guessing_unknown_models() {
        let fast = openai_billing_breakdown("gpt-5.4", billing_usage(1, 1, 0, 0), Some("fast"))
            .expect("fast pricing");
        let flex = openai_billing_breakdown("gpt-5.4", billing_usage(1, 1, 0, 0), Some("flex"))
            .expect("flex pricing");

        assert_eq!(fast.total_amount().amount().scaled(), 350_000);
        assert_eq!(fast.service_tier(), Some("fast"));
        assert_eq!(fast.multiplier_percent(), 200);
        assert!(!fast.long_context_billing_applied());
        assert_eq!(flex.total_amount().amount().scaled(), 87_500);
        assert_eq!(flex.multiplier_percent(), 50);
        assert!(
            openai_billing_breakdown("unknown-model", billing_usage(1, 1, 0, 0), None).is_none()
        );
    }

    #[test]
    fn billing_breakdown_should_use_official_fast_long_context_prices() {
        let breakdown =
            openai_billing_breakdown("gpt-5.6-sol", billing_usage(272_001, 1, 1, 1), Some("fast"))
                .expect("gpt-5.6-sol fast long-context pricing");

        assert_eq!(
            (
                breakdown.input_price_per_million().amount().scaled(),
                breakdown.cache_read_price_per_million().amount().scaled(),
                breakdown.cache_write_price_per_million().amount().scaled(),
                breakdown.output_price_per_million().amount().scaled(),
            ),
            (
                200_000_000_000,
                20_000_000_000,
                250_000_000_000,
                900_000_000_000,
            )
        );
    }

    #[test]
    fn billing_breakdown_should_fail_closed_for_unpublished_prices() {
        assert_eq!(
            (
                openai_billing_breakdown("gpt-5.5", billing_usage(272_001, 1, 0, 0), Some("fast"))
                    .is_none(),
                openai_billing_breakdown("gpt-5.5-pro", billing_usage(1, 1, 0, 0), Some("fast"))
                    .is_none(),
                openai_billing_breakdown("gpt-5.3-codex-spark", billing_usage(1, 1, 0, 0), None)
                    .is_none(),
                openai_billing_breakdown("gpt-4o", billing_usage(1, 1, 0, 0), Some("flex"))
                    .is_none(),
            ),
            (true, true, true, true)
        );
    }

    #[test]
    fn billing_breakdown_should_use_official_gpt_4o_fast_prices() {
        let breakdown =
            openai_billing_breakdown("gpt-4o", billing_usage(1, 1, 1, 0), Some("priority"))
                .expect("gpt-4o fast pricing");

        assert_eq!(
            (
                breakdown.cache_read_price_per_million().amount().scaled(),
                breakdown.output_price_per_million().amount().scaled(),
                breakdown.standard_amount().amount().scaled(),
                breakdown.total_amount().amount().scaled(),
                breakdown.multiplier_percent(),
            ),
            (21_250_000_000, 170_000_000_000, 112_500, 191_250, 170,)
        );
    }

    #[test]
    fn billing_breakdown_should_use_published_flex_cache_price() {
        let breakdown =
            openai_billing_breakdown("gpt-5.4", billing_usage(1, 0, 1, 0), Some("flex"))
                .expect("gpt-5.4 flex pricing");

        assert_eq!(
            breakdown.cache_read_price_per_million().amount().scaled(),
            1_300_000_000
        );
    }

    #[test]
    fn billing_breakdown_should_switch_only_after_the_long_context_threshold() {
        let boundary = openai_billing_breakdown("gpt-5.4", billing_usage(272_000, 0, 0, 0), None)
            .expect("short-context boundary");
        let long = openai_billing_breakdown("gpt-5.4", billing_usage(272_001, 0, 0, 0), None)
            .expect("long-context pricing");

        assert!(!boundary.long_context_billing_applied());
        assert!(long.long_context_billing_applied());
        assert!(
            !openai_billing_breakdown("gpt-4o", billing_usage(272_001, 0, 0, 0), None)
                .expect("model without long-context pricing")
                .long_context_billing_applied()
        );

        assert_eq!(
            boundary.input_price_per_million().amount().scaled(),
            25_000_000_000
        );
        assert_eq!(
            long.input_price_per_million().amount().scaled(),
            50_000_000_000
        );
        assert!(openai_billing_breakdown("gpt-5.4", billing_usage(1, 0, 2, 0), None).is_none());
    }

    #[test]
    fn billing_should_use_explicit_variant_prices_and_verified_snapshots() {
        // 每个用例包含一个普通输入 token 和一个输出 token，金额单位为 USD ticks
        for (model, expected) in [
            ("gpt-5.6-cyber", 875_000),
            ("gpt-5.5-cyber", 875_000),
            ("gpt-daybreak-red-latest", 875_000),
            ("gpt-daybreak-blue-latest", 350_000),
            ("chat-latest", 350_000),
            ("gpt-5.4-2026-03-05", 175_000),
            (" OpenAI/GPT-4o-2024-08-06 ", 125_000),
            ("gpt-4o-2024-05-13", 200_000),
            ("gpt-3.5-turbo-0125", 20_000),
            ("gpt-3.5-turbo-1106", 30_000),
        ] {
            let billing =
                openai_billing_breakdown(model, billing_usage(1, 1, 0, 0), None).expect(model);
            assert_eq!(
                billing.total_amount().amount().scaled(),
                expected,
                "{model}"
            );
        }
    }

    #[test]
    fn billing_should_not_price_shutdown_models_or_restore_them_through_aliases() {
        // 官方关闭日期已过；同族仍在服务的基础型号不能为这些旧型号兜底
        for model in [
            "gpt-4-0314",
            " OpenAI/GPT-4-0314 ",
            "gpt-5-codex",
            "gpt-5.1-codex",
            "gpt-5.1-codex-max",
            "gpt-5.1-codex-mini",
            "gpt-5.2-codex",
            "gpt-5-chat-latest",
            "gpt-5.1-chat-latest",
            "gpt-5.2-chat-latest",
            "gpt-5.3-chat-latest",
        ] {
            for tier in [None, Some("flex"), Some("fast")] {
                assert!(
                    openai_billing_breakdown(model, billing_usage(100, 10, 20, 0), tier).is_none(),
                    "{model} {tier:?}"
                );
            }
        }
    }

    #[test]
    fn billing_should_keep_deprecated_models_before_their_shutdown_dates() {
        // 截至 2026-09-13，这些型号尚未到官方关闭日期，仍按已公布单价计费
        for model in [
            "gpt-3.5-turbo-instruct",
            "gpt-3.5-turbo-1106",
            "gpt-4-0613",
            "gpt-4-turbo-2024-04-09",
            "gpt-4o-2024-05-13",
            "gpt-4.1-nano",
            "o1",
            "o1-pro",
            "o3-mini",
            "o4-mini",
        ] {
            assert!(
                openai_billing_breakdown(model, billing_usage(100, 10, 20, 0), None).is_some(),
                "{model}"
            );
        }
    }

    #[test]
    fn billing_should_not_inherit_prices_for_unknown_models_or_tiers() {
        for model in [
            "gpt-6-astra-future",
            "gpt-6-astra-2099-01-01",
            "gpt-5.6-sol-wm",
            "gpt-5.6-cyber-future",
            "gpt-5.4-cyber",
            "gpt-5.3-codex-spark",
            "gpt-4.5-preview",
            "gpt-4-32k",
            "gpt-5.4:custom",
            "gpt-5.4.1",
        ] {
            assert!(
                openai_billing_breakdown(model, billing_usage(1, 1, 0, 0), None).is_none(),
                "{model}"
            );
        }
        for (model, input, tier) in [
            ("gpt-5.6-cyber", 272_001, None),
            ("gpt-5.5-cyber", 272_001, None),
            ("gpt-5.6-cyber", 1, Some("fast")),
            ("gpt-5.3-codex", 1, Some("flex")),
            ("gpt-6.2-astra", 1, Some("flex")),
            ("gpt-6.2-astra", 272_001, Some("flex")),
            ("gpt-6.2-sol", 1, Some("flex")),
            ("gpt-6.2-sol", 272_001, Some("flex")),
            ("gpt-6-astra", 1, Some("auto")),
            ("gpt-6-astra", 1, Some("future")),
            ("gpt-6.1-terra", 1, Some("ultrafast")),
            ("gpt-6.1-terra", 272_001, Some("ultrafast")),
            ("gpt-6-sol", 1, Some("ultrafast")),
            ("gpt-6-sol", 272_001, Some("ultrafast")),
            ("gpt-6-luna", 1, Some("ultrafast")),
            ("gpt-6-luna", 272_001, Some("ultrafast")),
            ("gpt-5.4", 1, Some("ultrafast")),
        ] {
            assert!(
                openai_billing_breakdown(model, billing_usage(input, 1, 0, 0), tier).is_none(),
                "{model} {tier:?}"
            );
        }
    }

    #[test]
    fn billing_should_reject_overlapping_or_overflowing_cache_counts() {
        for (input, cached, written) in [(100, 20, 90), (100, 0, 101), (u64::MAX, u64::MAX, 1)] {
            assert!(
                openai_billing_breakdown(
                    "gpt-6-astra",
                    billing_usage(input, 0, cached, written),
                    None
                )
                .is_none()
            );
        }
        let cyber = openai_billing_breakdown("gpt-5.6-cyber", billing_usage(100, 10, 20, 10), None)
            .expect("已公开的 Cyber 缓存写价格");
        assert_eq!(
            cyber.cache_write_price_per_million().amount().to_string(),
            "15.6250000000"
        );
        assert_eq!(cyber.total_amount().amount().scaled(), 18_062_500);
    }

    #[test]
    fn custom_price_has_exact_cache_and_multiplier_math() {
        let custom: ModelPriceOverride =
            serde_json::from_value(json!({"multiplierBps":12500,"bands":{
                "standard":{"input":"2","output":"10","cacheRead":"0","cacheWrite":"3"}
            }}))
            .unwrap();
        let breakdown = openai_billing_breakdown_with_override(
            "custom-model",
            billing_usage(100, 10, 20, 10),
            None,
            Some(&custom),
        )
        .unwrap();
        assert_eq!(breakdown.input_amount().amount().canonical(), "0.000175");
        assert_eq!(breakdown.output_amount().amount().canonical(), "0.000125");
        assert_eq!(breakdown.cache_read_amount().amount().canonical(), "0");
        assert_eq!(
            breakdown.cache_write_amount().amount().canonical(),
            "0.0000375"
        );
        assert_eq!(breakdown.total_amount().amount().canonical(), "0.0003375");
        assert_eq!(breakdown.custom_multiplier_bps(), 12500);
        assert!(
            openai_billing_breakdown_with_override(
                "custom-model",
                billing_usage(100, 10, 0, 0),
                Some("flex"),
                Some(&custom)
            )
            .is_none()
        );
    }

    #[test]
    fn a_custom_long_tier_does_not_require_a_long_standard_band() {
        for (tier, band) in [("priority", "long_fast"), ("ultrafast", "long_ultrafast")] {
            let pricing = serde_json::from_value(json!({"multiplierBps":10000,"bands":{
                "standard":{"input":"2","output":"10","cacheRead":"0","cacheWrite":"0"},
                (band):{"input":"8","output":"40","cacheRead":"0","cacheWrite":"0"}
            }}))
            .unwrap();
            let result = openai_billing_breakdown_with_override(
                "custom-model",
                billing_usage(300_000, 0, 0, 0),
                Some(tier),
                Some(&pricing),
            )
            .unwrap();
            assert_eq!(result.total_amount().amount().canonical(), "2.4");
            assert_eq!(result.standard_amount().amount().canonical(), "0.6");
            assert!(result.long_context_billing_applied());
        }
    }

    #[test]
    fn multiplier_only_preserves_builtin_tiers_and_unknown_models_remain_unknown() {
        let custom: ModelPriceOverride =
            serde_json::from_value(json!({"multiplierBps":20000,"bands":{}})).unwrap();
        for tier in [None, Some("priority"), Some("flex"), Some("ultrafast")] {
            for input in [100, 272_001] {
                let usage = billing_usage(input, 10, 20, 5);
                let original = openai_billing_breakdown("gpt-6-astra", usage, tier).unwrap();
                let adjusted = openai_billing_breakdown_with_override(
                    "gpt-6-astra",
                    usage,
                    tier,
                    Some(&custom),
                )
                .unwrap();
                assert_eq!(
                    adjusted.total_amount().amount().scaled(),
                    original.total_amount().amount().scaled() * 2
                );
                assert_eq!(adjusted.multiplier_percent(), original.multiplier_percent());
                assert_eq!(adjusted.long_context_billing_applied(), input > 272_000);
            }
        }
        assert!(
            openai_billing_breakdown_with_override(
                "unknown-model",
                billing_usage(100, 10, 0, 0),
                None,
                Some(&custom)
            )
            .is_none()
        );
    }
}
