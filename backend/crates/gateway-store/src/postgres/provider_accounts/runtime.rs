//! 持久账号事实与可丢失冷却事实的统一状态投影。

use std::{collections::BTreeMap, time::SystemTime};

use futures::future::join_all;
use gateway_core::{
    account::{
        AccountStatusFacts, AccountStatusProjection, ProviderAccountId, resolve_account_status,
    },
    provider_ports::ProviderCooldownPort,
};

use super::ProviderAccountSummary;

#[must_use]
pub(crate) fn account_status_projection(
    account: &ProviderAccountSummary,
    now: SystemTime,
    rate_limited_until: Option<SystemTime>,
) -> AccountStatusProjection {
    let mut facts = account.active_status_facts();
    if account.responses_upstream != gateway_core::account::ResponsesUpstream::Excel {
        facts.rate_limited_until = rate_limited_until;
    }
    resolve_account_status(&facts, now)
}

impl ProviderAccountSummary {
    pub(crate) fn active_status_facts(&self) -> AccountStatusFacts {
        let account = self;
        if account.responses_upstream == gateway_core::account::ResponsesUpstream::Excel {
            let excel = account.excel_credential.as_ref();
            return AccountStatusFacts {
                quota_paused: account.quota_paused,
                enabled: account.enabled && !excel.is_some_and(|e| e.suspended),
                credential_state: excel
                    .map_or(gateway_core::account::CredentialState::Invalid, |e| e.state),
                access_token_expires_at: excel.and_then(|e| e.access_token_expires_at),
                quota: excel.map_or_else(gateway_core::account::QuotaState::unknown, |e| e.quota),
                rate_limited_until: None,
                last_error_reason: excel.and_then(|e| e.last_error_reason),
                last_error_message: excel.map_or_else(
                    || Some("Excel 凭据尚未获取，请重新授权 Excel".into()),
                    |e| e.last_error_message.clone(),
                ),
            };
        }
        AccountStatusFacts {
            quota_paused: account.quota_paused,
            enabled: account.enabled,
            credential_state: account.credential_state,
            access_token_expires_at: account.access_token_expires_at.map(Into::into),
            quota: account.quota,
            rate_limited_until: None,
            last_error_reason: account.last_error_reason,
            last_error_message: account.last_error_message.clone(),
        }
    }
}

/// 尽力读取账号当前有效的 429 冷却。
///
/// 冷却属于可丢失的账号级运行时事实：端口不可用、单条读取失败或脏账号 ID
/// 都视为没有冷却，不污染持久账号状态。凭据轮换不会解除上游对同一账号的限流。
pub(crate) async fn load_rate_limited_until(
    cooldowns: Option<&dyn ProviderCooldownPort>,
    accounts: &[ProviderAccountStatus],
    now: SystemTime,
) -> BTreeMap<String, (SystemTime, bool)> {
    let Some(cooldowns) = cooldowns else {
        return BTreeMap::new();
    };
    let mut result = BTreeMap::new();
    for batch in accounts.chunks(crate::redis::ACCOUNT_STATE_READ_CONCURRENCY) {
        let reads = batch.iter().map(|account| async move {
            if account.excel_entry {
                return None;
            }
            let account_id = ProviderAccountId::new(account.id.clone()).ok()?;
            let cooldown = cooldowns
                .read(&account_id)
                .await
                .ok()
                .flatten()
                .map(|state| state.until());
            let protection = cooldowns
                .read_protection(&account_id, account.revision)
                .await
                .ok()
                .flatten()
                .map(|state| state.scheduling_until(now));
            cooldown
                .max(protection)
                .filter(|until| *until > now)
                .map(|until| (account.id.clone(), (until, protection.is_some())))
        });
        result.extend(join_all(reads).await.into_iter().flatten());
    }
    result
}

/// Dashboard does not need identity documents, model lists or proxy locations.
pub(crate) struct ProviderAccountStatus {
    pub excel_entry: bool,
    pub revision: gateway_core::account::CredentialRevision,
    pub id: String,
    pub concurrency_limit: Option<gateway_core::account::AccountConcurrencyLimit>,
    pub facts: AccountStatusFacts,
}

pub(crate) async fn load_account_statuses<'e>(
    pool: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
) -> crate::StoreResult<Vec<ProviderAccountStatus>> {
    use super::rows::{account_status_facts_from_row, get};
    let rows = sqlx::query(
        "select a.id,a.credential_revision,a.quota_paused,a.concurrency_limit,a.responses_upstream='excel' as excel_entry,
           a.enabled and not (a.responses_upstream='excel' and coalesce(x.suspended,false)) as enabled,
           case when a.responses_upstream='excel' then coalesce(x.credential_state,'invalid') else a.credential_state end as credential_state,
           case when a.responses_upstream='excel' then x.access_token_expires_at else a.access_token_expires_at end as access_token_expires_at,
           case when a.responses_upstream='excel' then coalesce(x.quota_access_state,'unknown') else a.quota_access_state end as quota_access_state,
           case when a.responses_upstream='excel' then x.quota_evidence else a.quota_evidence end as quota_evidence,
           case when a.responses_upstream='excel' then x.quota_access_observed_at else a.quota_access_observed_at end as quota_access_observed_at,
           case when a.responses_upstream='excel' then x.quota_reset_at else a.quota_reset_at end as quota_reset_at,
           case when a.responses_upstream='excel' then x.last_error_reason else a.last_error_reason end as last_error_reason,
           case when a.responses_upstream='excel' then x.last_error_message else a.last_error_message end as last_error_message
         from provider_accounts a left join openai_excel_credentials x on x.account_id=a.id",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| crate::postgres_unavailable("load account status facts"))?;
    rows.iter()
        .map(|row| {
            let concurrency_limit = get::<Option<i64>>(row, "concurrency_limit")?
                .map(|value| {
                    u32::try_from(value)
                        .ok()
                        .and_then(gateway_core::account::AccountConcurrencyLimit::new)
                        .ok_or_else(|| super::invalid("invalid concurrency_limit"))
                })
                .transpose()?;
            Ok(ProviderAccountStatus {
                excel_entry: get(row, "excel_entry")?,
                revision: gateway_core::account::CredentialRevision::new(super::to_u64(get(
                    row,
                    "credential_revision",
                )?)?)
                .map_err(|_| super::invalid("invalid credential revision"))?,
                id: get(row, "id")?,
                concurrency_limit,
                facts: account_status_facts_from_row(row)?,
            })
        })
        .collect()
}
