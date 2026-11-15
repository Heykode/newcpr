//! Admin 账号列表、用量与成本的领域映射。

use super::*;

pub(crate) fn admin_account_record(
    summary: ProviderAccountSummary,
) -> AdminStoreResult<AccountRecord> {
    Ok(AccountRecord {
        quota_paused: summary.quota_paused,
        quota_limits: summary.quota_limits,
        auto_use_quota_credits: summary.auto_use_quota_credits,
        outbound_proxy_id: summary.outbound_proxy_id,
        outbound_proxy_enabled: summary.outbound_proxy_enabled,
        custom_name: summary.custom_name,
        outbound_proxy: summary.outbound_proxy,
        id: summary.id,
        provider_kind: ProviderKind::new(summary.provider_kind).map_err(|_| {
            AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                ENTITY,
                "persisted Provider kind is invalid",
            )
        })?,
        groups: Vec::new(),
        name: summary.name,
        email: summary.email,
        upstream_user_id: summary.upstream_user_id,
        upstream_account_id: summary.upstream_account_id,
        plan_type: summary.plan_type,
        authentication_kind: summary.authentication_kind,
        credential_revision: admin_revision(summary.credential_revision)?,
        turn_state_binding_revision: admin_revision(summary.turn_state_binding_revision)?,
        relogin_count: summary.relogin_count,
        last_relogin_at: summary.last_relogin_at,
        has_refresh_token: summary.has_refresh_token,
        access_token_expires_at: summary.access_token_expires_at,
        next_refresh_at: summary.next_refresh_at,
        enabled: summary.enabled,
        turn_state_injection_enabled: summary.turn_state_injection_enabled,
        responses_upstream: summary.responses_upstream,
        excel_models: summary.excel_models,
        excel_models_follow_global: summary.excel_models_follow_global,
        excel_cache_creation_as_input: summary.excel_cache_creation_as_input,
        excel_ignore_encrypted_content: summary.excel_ignore_encrypted_content,
        request_proxy_source: summary.request_proxy_source,
        excel_auto_disable_on_403: summary.excel_auto_disable_on_403,
        excel_403_action: summary.excel_403_action,
        excel_auto_disabled_at: summary.excel_auto_disabled_at,
        excel_403_warning_at: summary.excel_403_warning_at,
        quality_monitoring: None,
        excel_mode_disabled_at: summary.excel_mode_disabled_at,
        effective_excel_models: summary.effective_excel_models,
        concurrency_limit: summary.concurrency_limit,
        weight: summary.weight,
        model_access: summary.model_access,
        credential_state: summary.credential_state,
        credential_observed_at: summary.credential_observed_at,
        quota: summary.quota,
        last_error_reason: summary.last_error_reason,
        last_error_message: summary.last_error_message,
        created_at: summary.created_at,
        updated_at: summary.updated_at,
    })
}

pub(crate) fn prepared_account(
    credential: PreparedCredentialCreate,
) -> StoreResult<NewProviderAccount> {
    Ok(NewProviderAccount {
        outbound_proxy: credential.outbound_proxy,
        id: credential.account_id.as_str().to_owned(),
        provider_kind: credential.provider_kind.as_str().to_owned(),
        name: credential.name,
        email: credential.email,
        upstream_user_id: credential.upstream_user_id,
        upstream_account_id: credential.upstream_account_id,
        plan_type: credential.plan_type,
        authentication_kind: credential.authentication_kind,
        provider_credentials_json: provider_document_json(credential.provider_material)?,
        has_refresh_token: credential.has_refresh_token,
        access_token_expires_at: credential.access_token_expires_at,
        next_refresh_at: credential.next_refresh_at,
        enabled: credential.enabled,
        concurrency_limit: None,
        weight: AccountWeight::DEFAULT,
        model_access: credential.model_access,
        credential_state: credential.credential_state,
        credential_observed_at: credential.credential_observed_at,
    })
}

pub(crate) fn provider_document_json(document: ProviderDocument) -> StoreResult<JsonObject> {
    JsonObject::try_from_value(
        ENTITY,
        serde_json::Value::Object(document.into_provider_data().into_inner()),
        CREDENTIALS_MAX_BYTES,
    )
}

pub(crate) fn admin_account_usage(
    usage: ProviderAccountUsageObservation,
) -> AdminStoreResult<AccountUsage> {
    Ok(AccountUsage {
        account_id: usage.account_id,
        request_count: usage.request_count,
        success_count: usage.success_count,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_tokens: usage.cached_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        image_input_tokens: usage.image_input_tokens,
        image_output_tokens: usage.image_output_tokens,
        image_request_count: usage.image_request_count,
        image_request_failed_count: usage.image_request_failed_count,
        total_tokens: usage.total_tokens,
        cost_coverage: admin_account_cost_coverage(usage.cost_coverage),
        costs: admin_account_costs(usage.costs)?,
        last_used_at: usage.last_used_at,
        request_buckets: usage
            .request_buckets
            .into_iter()
            .map(|bucket| AccountRequestBucket {
                bucket_start: bucket.bucket_start,
                request_count: bucket.request_count,
                success_count: bucket.success_count,
                error_count: bucket.error_count,
                non_completion_count: bucket.non_completion_count,
            })
            .collect(),
        models: usage
            .models
            .into_iter()
            .map(admin_account_model_usage)
            .collect::<AdminStoreResult<Vec<_>>>()?,
    })
}

pub(crate) fn window_usage_count(
    row: &sqlx::postgres::PgRow,
    column: &'static str,
) -> AdminStoreResult<u64> {
    let value = window_usage_value::<i64>(row, column)?;
    u64::try_from(value).map_err(|_| invalid_window_usage(column))
}

pub(crate) fn window_usage_value<T>(
    row: &sqlx::postgres::PgRow,
    column: &'static str,
) -> AdminStoreResult<T>
where
    for<'row> T: sqlx::Decode<'row, Postgres> + sqlx::Type<Postgres>,
{
    row.try_get(column)
        .map_err(|_| invalid_window_usage(column))
}

pub(crate) fn invalid_window_usage(column: &'static str) -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Invalid,
        ENTITY,
        format!("invalid quota window usage column: {column}"),
    )
}

pub(crate) fn admin_account_model_usage(
    usage: ProviderAccountModelUsageObservation,
) -> AdminStoreResult<AccountModelUsage> {
    Ok(AccountModelUsage {
        model: usage.model,
        health: usage.health,
        request_count: usage.request_count,
        success_count: usage.success_count,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_tokens: usage.cached_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        image_input_tokens: usage.image_input_tokens,
        image_output_tokens: usage.image_output_tokens,
        image_request_count: usage.image_request_count,
        image_request_failed_count: usage.image_request_failed_count,
        total_tokens: usage.total_tokens,
        cost_coverage: admin_account_cost_coverage(usage.cost_coverage),
        costs: admin_account_costs(usage.costs)?,
        last_used_at: usage.last_used_at,
    })
}

pub(crate) const fn admin_account_cost_coverage(
    coverage: crate::postgres::CostCoverage,
) -> AdminCostCoverage {
    AdminCostCoverage {
        provider_reported_count: coverage.provider_reported_count,
        calculated_count: coverage.calculated_count,
        partial_count: 0,
        unavailable_count: coverage.unavailable_count,
        not_billable_count: 0,
    }
}

pub(crate) fn admin_account_costs(
    costs: Vec<CurrencyCostTotal>,
) -> AdminStoreResult<Vec<AccountCost>> {
    costs
        .into_iter()
        .map(|cost| {
            let amount = AdminDecimalAmount::from_str(cost.amount.as_str()).map_err(|_| {
                AdminStoreError::new(
                    AdminStoreErrorKind::Invalid,
                    ENTITY,
                    "persisted account cost is invalid",
                )
            })?;
            Ok(AccountCost {
                currency: cost.currency,
                amount,
            })
        })
        .collect()
}
