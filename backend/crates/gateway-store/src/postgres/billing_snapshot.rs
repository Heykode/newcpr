//! Request-time billing facts, independent from mutable provider price catalogs.

use gateway_admin::model::observability::{CalculatedBillingBreakdown, CurrencyCost, UsageBilling};
use gateway_core::metering::{CalculatedCostBreakdown, CurrencyCode};
use serde_json::{Value, json};

pub(crate) fn encode(b: &CalculatedCostBreakdown) -> Value {
    json!({
        "version": 1,
        "longContextBillingApplied": b.long_context_billing_applied(),
        "input": b.input_amount().amount().to_string(),
        "output": b.output_amount().amount().to_string(),
        "cacheRead": b.cache_read_amount().amount().to_string(),
        "cacheWrite": b.cache_write_amount().amount().to_string(),
        "standard": b.standard_amount().amount().to_string(),
        "total": b.total_amount().amount().to_string(),
        "inputPrice": b.input_price_per_million().amount().to_string(),
        "outputPrice": b.output_price_per_million().amount().to_string(),
        "cacheReadPrice": b.cache_read_price_per_million().amount().to_string(),
        "cacheWritePrice": b.cache_write_price_per_million().amount().to_string(),
        "currency": b.total_amount().currency().as_str(),
        "serviceTier": b.service_tier(),
        "multiplierPercent": b.multiplier_percent(),
    })
}

pub(crate) fn decode(value: &Value) -> Option<CalculatedBillingBreakdown> {
    if value.get("version")?.as_u64()? != 1 {
        return None;
    }
    let currency = value.get("currency")?.as_str()?;
    CurrencyCode::new(currency).ok()?;
    let amount = |key| {
        Some(CurrencyCost {
            currency: currency.to_owned(),
            amount: value.get(key)?.as_str()?.parse().ok()?,
        })
    };
    Some(CalculatedBillingBreakdown {
        // Missing legacy facts do not prove which price band was selected.
        long_context_billing_applied: match value.get("longContextBillingApplied") {
            Some(value) => value.as_bool()?,
            None => false,
        },
        input_amount: amount("input")?,
        output_amount: amount("output")?,
        cache_read_amount: amount("cacheRead")?,
        cache_write_amount: amount("cacheWrite")?,
        standard_amount: amount("standard")?,
        total_amount: amount("total")?,
        input_price_per_million: amount("inputPrice")?,
        output_price_per_million: amount("outputPrice")?,
        cache_read_price_per_million: amount("cacheReadPrice")?,
        cache_write_price_per_million: amount("cacheWritePrice")?,
        service_tier: match value.get("serviceTier")? {
            Value::Null => None,
            Value::String(value) => Some(value.clone()),
            _ => return None,
        },
        multiplier_percent: value.get("multiplierPercent")?.as_u64()?.try_into().ok()?,
    })
}

pub(crate) fn prefer_saved(
    total: Option<UsageBilling>,
    snapshot: Option<&Value>,
) -> Option<UsageBilling> {
    if let Some(UsageBilling::Total { source, total }) = &total
        && source == "calculated"
        && let Some(saved) = snapshot.and_then(decode)
        && &saved.total_amount == total
    {
        return Some(UsageBilling::Calculated(Box::new(saved)));
    }
    total
}
