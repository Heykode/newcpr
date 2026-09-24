//! 统一账号目录与跨 Provider 动态分派。

use std::{collections::BTreeMap, num::NonZeroU32, sync::Arc};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use futures::StreamExt as _;
use gateway_core::{
    account::{AccountConcurrencyLimit, ProviderAccountId},
    engine::probe::{AccountProbe, AccountProbeRequest},
    routing::{ProviderKind, UpstreamModelId},
    runtime::SnapshotControl,
};

use crate::{
    model::{
        AdminError, MutationContext,
        accounts::{
            AccountConnectionTest, AccountConnectionTestEvent, AccountConnectionTestEventStream,
            AccountListQuery, AccountPageItem, AccountUpdateResult, AccountUsageWindowQuery,
            AccountsUpdateResult, BatchUpdateAccounts, TurnStateProbeOutcome, UpdateAccount,
        },
        observability::TimeRange,
        provider_credentials::{
            AccountDirectoryItem, AccountDirectoryPage, AccountExportBundle, AccountPersonalInfo,
            AccountRefreshResult, AccountUsagePeriod, ConsumeProviderResetCredit,
            PrepareCredentialRefresh, ProviderModels, ProviderProfileAvatar, ProviderQuota,
            ProviderQuotaRequest, ProviderQuotaWindow, ProviderResetCreditResult,
            ProviderResetCredits, QuotaLocalUsageAttribution,
        },
        quota_forecast::{
            AccountQuotaForecastReport, account_quota_forecasts, current_window_estimate,
            forecast_window,
        },
        quota_forecast_sampling::{QuotaForecastPoint, select_forecast_sample},
    },
    ports::{
        provider::ProviderAdminRegistry,
        store::{AccountRuntimeStore, AccountStore, SettingsStore},
    },
};

use super::{
    commit_credential_refresh, map_provider_error, map_store_error, publish_committed,
    validate_prepared_rotation,
};

fn account_health_range(now: chrono::DateTime<Utc>) -> TimeRange {
    // Align the short window to epoch five-minute boundaries so the store's
    // relative bucket indices map directly to the six UI cells.
    let current_bucket_start =
        chrono::DateTime::<Utc>::from_timestamp(now.timestamp().div_euclid(300) * 300, 0)
            .unwrap_or(now);
    TimeRange {
        start: current_bucket_start - Duration::minutes(25),
        end: current_bucket_start + Duration::minutes(5),
    }
}

fn apply_current_quota_estimates(quota: &ProviderQuota, report: &mut AccountQuotaForecastReport) {
    for (forecast, period) in report
        .forecasts
        .iter_mut()
        .zip([AccountUsagePeriod::Weekly, AccountUsagePeriod::Monthly])
    {
        forecast.estimated_usd = None;
        forecast.remaining_usd = None;
        let selected = forecast_window(quota, period);
        let Some((window, _)) = selected else {
            continue;
        };
        let Some(estimate) = current_window_estimate(window, report.generated_at) else {
            if forecast.estimated_tokens.is_none() {
                forecast
                    .unavailable_reason
                    .get_or_insert("本轮消费或已用比例不足，暂无额度估值。");
            }
            continue;
        };
        let Some(seconds) = window.window_seconds.filter(|seconds| *seconds > 0) else {
            continue;
        };
        forecast.estimated_usd =
            Some(estimate.total_usd * forecast.target_seconds as f64 / seconds as f64)
                .filter(|value| value.is_finite());
        forecast.remaining_usd = Some(estimate.remaining_usd);
        forecast.incomplete_cost = estimate.incomplete_cost;
        if let Some(source) = &mut forecast.source {
            source.usd = window.local_usage.as_ref().and_then(|usage| {
                usage
                    .costs
                    .iter()
                    .find(|cost| cost.currency.eq_ignore_ascii_case("USD"))
                    .and_then(|cost| cost.amount.to_string().parse().ok())
            });
        }
        forecast.unavailable_reason = None;
    }
}

/// 统一账号页消费的服务。
#[async_trait]
pub trait AccountsService: Send + Sync {
    async fn request_turn_state_probe(
        &self,
        _account_id: &ProviderAccountId,
        _model: &UpstreamModelId,
    ) -> Result<TurnStateProbeOutcome, AdminError> {
        Err(AdminError::invalid("当前 Provider 不支持 State 探测"))
    }

    async fn list(&self, query: AccountListQuery) -> Result<AccountDirectoryPage, AdminError>;

    async fn export(
        &self,
        context: &MutationContext,
        account_ids: Vec<ProviderAccountId>,
    ) -> Result<AccountExportBundle, AdminError>;

    async fn refresh(
        &self,
        context: &MutationContext,
        account_id: ProviderAccountId,
    ) -> Result<AccountRefreshResult, AdminError>;

    async fn recover(
        &self,
        context: &MutationContext,
        account_id: ProviderAccountId,
    ) -> Result<AccountRefreshResult, AdminError>;

    async fn update(
        &self,
        context: &MutationContext,
        command: UpdateAccount,
    ) -> Result<AccountUpdateResult, AdminError>;

    async fn batch_update(
        &self,
        context: &MutationContext,
        command: BatchUpdateAccounts,
    ) -> Result<AccountsUpdateResult, AdminError>;

    async fn quota(
        &self,
        account_id: &ProviderAccountId,
        refresh: bool,
    ) -> Result<AccountDirectoryItem, AdminError>;

    async fn quota_forecast(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<AccountQuotaForecastReport, AdminError>;

    /// Cached provider windows plus the same local window usage used by the account list.
    async fn current_quota(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<ProviderQuota, AdminError>;

    async fn personal_info(
        &self,
        _account_id: &ProviderAccountId,
    ) -> Result<AccountPersonalInfo, AdminError> {
        Err(AdminError::invalid("当前 Provider 不支持个人信息"))
    }

    async fn profile_avatar(
        &self,
        _account_id: &ProviderAccountId,
    ) -> Result<ProviderProfileAvatar, AdminError> {
        Err(AdminError::invalid("当前 Provider 不支持账号头像"))
    }

    async fn reset_credits(
        &self,
        _context: &MutationContext,
        _account_id: ProviderAccountId,
    ) -> Result<ProviderResetCredits, AdminError> {
        Err(AdminError::invalid("当前 Provider 不支持重置额度"))
    }

    async fn consume_reset_credit(
        &self,
        _context: &MutationContext,
        _command: ConsumeProviderResetCredit,
    ) -> Result<ProviderResetCreditResult, AdminError> {
        Err(AdminError::invalid("当前 Provider 不支持重置额度"))
    }

    async fn models(
        &self,
        account_id: &ProviderAccountId,
        refresh: bool,
    ) -> Result<ProviderModels, AdminError>;

    async fn model_catalog_document(
        &self,
        _account_id: &ProviderAccountId,
    ) -> Result<crate::model::provider_credentials::ProviderModelCatalogDocument, AdminError> {
        Err(AdminError::invalid("当前 Provider 不支持原生模型目录导出"))
    }

    async fn test_connection(
        &self,
        command: AccountConnectionTest,
    ) -> Result<AccountConnectionTestEventStream, AdminError>;
}

pub(crate) struct DefaultAccountsService {
    accounts: Arc<dyn AccountStore>,
    account_runtime: Arc<dyn AccountRuntimeStore>,
    settings: Arc<dyn SettingsStore>,
    providers: ProviderAdminRegistry,
    snapshot: Arc<dyn SnapshotControl>,
    probe: Arc<dyn AccountProbe>,
    reset_credit_locks:
        Arc<futures::lock::Mutex<BTreeMap<ProviderAccountId, Arc<futures::lock::Mutex<()>>>>>,
}

impl DefaultAccountsService {
    #[must_use]
    pub(crate) fn new(
        accounts: Arc<dyn AccountStore>,
        account_runtime: Arc<dyn AccountRuntimeStore>,
        settings: Arc<dyn SettingsStore>,
        providers: ProviderAdminRegistry,
        snapshot: Arc<dyn SnapshotControl>,
        probe: Arc<dyn AccountProbe>,
    ) -> Self {
        Self {
            accounts,
            account_runtime,
            settings,
            providers,
            snapshot,
            probe,
            reset_credit_locks: Arc::new(futures::lock::Mutex::new(BTreeMap::new())),
        }
    }

    async fn default_concurrency_limit(&self) -> Result<NonZeroU32, AdminError> {
        let settings = self
            .settings
            .load_runtime_settings()
            .await
            .map_err(|error| map_store_error(error, "runtime settings"))?;
        NonZeroU32::new(settings.max_concurrent_per_account)
            .ok_or_else(|| AdminError::internal("运行时账号并发上限不合法"))
    }

    async fn reset_credit_lock(
        &self,
        account_id: &ProviderAccountId,
    ) -> Arc<futures::lock::Mutex<()>> {
        let mut locks = self.reset_credit_locks.lock().await;
        Arc::clone(
            locks
                .entry(account_id.clone())
                .or_insert_with(|| Arc::new(futures::lock::Mutex::new(()))),
        )
    }

    async fn load_account(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<AccountPageItem, AdminError> {
        let runtime = self
            .account_runtime
            .account_runtime(&[account_id.as_str().to_owned()])
            .await
            .map_err(|error| map_store_error(error, "account runtime"))?;
        self.accounts
            .load_account(account_id.as_str(), runtime)
            .await
            .map_err(|error| map_store_error(error, "provider account"))?
            .ok_or_else(|| AdminError::not_found("Provider 账号不存在"))
    }

    async fn provider_for_account(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<
        (
            AccountPageItem,
            Arc<dyn crate::ports::provider::ProviderAdmin>,
        ),
        AdminError,
    > {
        let item = self.load_account(account_id).await?;
        let provider = self
            .providers
            .require(&item.account.provider_kind)
            .map_err(|error| map_provider_error(error, "provider account"))?;
        Ok((item, provider))
    }

    async fn attach_quota_local_usage(
        &self,
        accounts: &[AccountPageItem],
        quotas: &mut [ProviderQuota],
    ) -> Result<(), AdminError> {
        let windows = accounts
            .iter()
            .zip(quotas.iter())
            .flat_map(|(item, quota)| {
                quota
                    .windows
                    .iter()
                    .filter(|window| window.local_usage.is_none())
                    .filter_map(|window| quota_usage_window(&item.account.id, window))
            })
            .collect::<Vec<_>>();
        if windows.is_empty() {
            return Ok(());
        }
        let usage_by_window = self
            .accounts
            .load_account_usage_by_windows(&windows)
            .await
            .map_err(|error| map_store_error(error, "quota window usage"))?
            .into_iter()
            .map(|result| ((result.account_id, result.key), result.usage))
            .collect::<BTreeMap<_, _>>();
        for (item, quota) in accounts.iter().zip(quotas) {
            for window in &mut quota.windows {
                if window.local_usage.is_none()
                    && window.local_usage_attribution == QuotaLocalUsageAttribution::AccountWide
                {
                    let key = (item.account.id.clone(), window.key.clone());
                    if let Some(usage) = usage_by_window.get(&key) {
                        window.local_usage = Some(usage.clone());
                    }
                }
            }
        }
        Ok(())
    }

    async fn load_directory_item(
        &self,
        account_id: &ProviderAccountId,
        refresh_quota: bool,
    ) -> Result<AccountDirectoryItem, AdminError> {
        let (stored, provider) = self.provider_for_account(account_id).await?;
        let account = &stored.account;
        let now = Utc::now();
        let rolling_range = TimeRange {
            start: now - Duration::hours(24),
            end: now,
        };
        let ids = vec![account.id.clone()];
        let (rolling_usage, health_usage, mut cumulative_costs) = futures::try_join!(
            self.accounts.load_account_usage(rolling_range, &ids),
            self.accounts
                .load_account_health_timeline(account_health_range(now), &ids),
            self.accounts.load_account_cumulative_costs(&ids),
        )
        .map_err(|error| map_store_error(error, "account usage"))?;
        let rolling_usage = rolling_usage.into_iter().next();
        let health_timeline = health_usage
            .into_iter()
            .next()
            .as_ref()
            .map(|usage| usage.request_buckets.clone())
            .unwrap_or_default();
        let mut quota = provider
            .quota(ProviderQuotaRequest {
                account_id: account_id.clone(),
                refresh: refresh_quota,
                rolling_usage: rolling_usage.clone(),
            })
            .await
            .map_err(|error| map_provider_error(error, "provider quota"))?;
        let mut stored = if refresh_quota {
            self.load_account(account_id).await?
        } else {
            stored
        };
        self.attach_quota_local_usage(
            std::slice::from_ref(&stored),
            std::slice::from_mut(&mut quota),
        )
        .await?;
        let usage = quota
            .usage_window()
            .and_then(|(window, _)| window.local_usage.clone());
        let default_concurrency = self.default_concurrency_limit().await?;
        Ok(AccountDirectoryItem {
            turn_state: self
                .accounts
                .load_turn_state_status(std::slice::from_ref(&stored.account.id))
                .await
                .unwrap_or_default()
                .remove(&stored.account.id)
                .map(|mut state| {
                    if stored.projection.status != gateway_core::account::AccountStatus::Normal {
                        state.ready_models.clear();
                    }
                    state
                }),
            effective_concurrency_limit: stored
                .account
                .concurrency_limit
                .map_or(default_concurrency, AccountConcurrencyLimit::into_non_zero),
            in_flight: stored.in_flight,
            health_timeline,
            cumulative_costs: cumulative_costs
                .remove(account_id.as_str())
                .unwrap_or_default(),
            plan_type_display: self.providers.resolve_account_plan(
                stored.account.provider_kind.as_str(),
                &mut stored.account.plan_type,
                Some(&quota),
            ),
            projection: stored.projection,
            usage,
            account: stored.account,
            quota,
        })
    }
}

#[async_trait]
impl AccountsService for DefaultAccountsService {
    async fn list(&self, query: AccountListQuery) -> Result<AccountDirectoryPage, AdminError> {
        let runtime = self
            .account_runtime
            .active_rate_limits()
            .await
            .map_err(|error| map_store_error(error, "account runtime"))?;
        let mut page = self
            .accounts
            .list_accounts(query, runtime)
            .await
            .map_err(|error| map_store_error(error, "account directory"))?;
        let page_ids = page
            .items
            .iter()
            .map(|item| item.account.id.clone())
            .collect::<Vec<_>>();
        if !page_ids.is_empty()
            && let Ok(runtime) = self.account_runtime.account_runtime(&page_ids).await
        {
            for item in &mut page.items {
                item.in_flight = runtime
                    .in_flight
                    .as_ref()
                    .map(|values| values.get(&item.account.id).copied().unwrap_or(0));
            }
        }
        let now = Utc::now();
        let rolling_range = TimeRange {
            start: now - Duration::hours(24),
            end: now,
        };
        let ids = page
            .items
            .iter()
            .map(|item| item.account.id.clone())
            .collect::<Vec<_>>();
        let (rolling_usage, health_usage, mut cumulative_costs) = futures::try_join!(
            self.accounts.load_account_usage(rolling_range, &ids),
            self.accounts
                .load_account_health_timeline(account_health_range(now), &ids),
            self.accounts.load_account_cumulative_costs(&ids),
        )
        .map_err(|error| map_store_error(error, "account usage"))?;
        let rolling_usage = rolling_usage
            .into_iter()
            .map(|usage| (usage.account_id.clone(), usage))
            .collect::<BTreeMap<_, _>>();
        let health_usage = health_usage
            .into_iter()
            .map(|usage| (usage.account_id.clone(), usage))
            .collect::<BTreeMap<_, _>>();
        let mut quotas = futures::future::join_all(page.items.iter().map(|item| async {
            let account = &item.account;
            let account_id = ProviderAccountId::new(account.id.clone())
                .map_err(|_| AdminError::invalid("Provider 账号 ID 不合法"))?;
            // 单个账号的 quota 投影失败（Provider 未注册或 quota 读取失败）不拖垮整页：
            // 该账号降级为空额度投影，其余账号与页面状态照常返回。
            let provider = match self.providers.require(&account.provider_kind) {
                Ok(provider) => provider,
                Err(error) => {
                    tracing::warn!(
                        account_id = %account.id,
                        error = %error,
                        "account directory provider is not registered; showing empty quota"
                    );
                    return Ok(empty_quota());
                }
            };
            match provider
                .quota(ProviderQuotaRequest {
                    account_id,
                    refresh: false,
                    rolling_usage: rolling_usage.get(&account.id).cloned(),
                })
                .await
            {
                Ok(quota) => Ok(quota),
                Err(error) => {
                    tracing::warn!(
                        account_id = %account.id,
                        error = %error,
                        "account directory quota projection failed; showing empty quota"
                    );
                    Ok(empty_quota())
                }
            }
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, AdminError>>()?;
        self.attach_quota_local_usage(&page.items, &mut quotas)
            .await?;
        let default_concurrency = self.default_concurrency_limit().await?;
        let mut turn_states = self
            .accounts
            .load_turn_state_status(&ids)
            .await
            .unwrap_or_default();
        let items = page
            .items
            .into_iter()
            .zip(quotas)
            .map(|(mut item, quota)| {
                let health_timeline = health_usage
                    .get(&item.account.id)
                    .map(|usage| usage.request_buckets.clone())
                    .unwrap_or_default();
                let usage = quota
                    .usage_window()
                    .and_then(|(window, _)| window.local_usage.clone());
                AccountDirectoryItem {
                    turn_state: turn_states.remove(&item.account.id).map(|mut state| {
                        if item.projection.status != gateway_core::account::AccountStatus::Normal {
                            state.ready_models.clear();
                        }
                        state
                    }),
                    effective_concurrency_limit: item
                        .account
                        .concurrency_limit
                        .map_or(default_concurrency, AccountConcurrencyLimit::into_non_zero),
                    in_flight: item.in_flight,
                    health_timeline,
                    cumulative_costs: cumulative_costs
                        .remove(&item.account.id)
                        .unwrap_or_default(),
                    plan_type_display: self.providers.resolve_account_plan(
                        item.account.provider_kind.as_str(),
                        &mut item.account.plan_type,
                        Some(&quota),
                    ),
                    usage,
                    account: item.account,
                    projection: item.projection,
                    quota,
                }
            })
            .collect();
        Ok(AccountDirectoryPage {
            config_revision: page.config_revision,
            items,
            total: page.total,
            summary: page.summary,
        })
    }

    async fn export(
        &self,
        context: &MutationContext,
        account_ids: Vec<ProviderAccountId>,
    ) -> Result<AccountExportBundle, AdminError> {
        if account_ids.is_empty() || account_ids.len() > 200 {
            return Err(AdminError::invalid("账号导出数量必须在 1 到 200 之间"));
        }
        let exported_ids = account_ids.clone();
        let mut grouped = BTreeMap::<ProviderKind, Vec<ProviderAccountId>>::new();
        for account_id in account_ids {
            let account = self.load_account(&account_id).await?;
            grouped
                .entry(account.account.provider_kind)
                .or_default()
                .push(account_id);
        }
        if grouped.values().any(|ids| {
            let unique = ids.iter().collect::<std::collections::BTreeSet<_>>();
            unique.len() != ids.len()
        }) {
            return Err(AdminError::invalid("账号导出列表包含重复 ID"));
        }
        let mut documents = Vec::with_capacity(grouped.len());
        for (provider_kind, ids) in grouped {
            let provider = self
                .providers
                .require(&provider_kind)
                .map_err(|error| map_provider_error(error, "provider account export"))?;
            let credentials = self
                .accounts
                .load_credentials_for_export(&provider_kind, &ids)
                .await
                .map_err(|error| map_store_error(error, "provider account export"))?;
            documents.push(
                provider
                    .export_credentials(credentials)
                    .await
                    .map_err(|error| map_provider_error(error, "provider account export"))?,
            );
        }
        self.accounts
            .record_credential_export(&exported_ids, context)
            .await
            .map_err(|error| map_store_error(error, "provider account export audit"))?;
        Ok(AccountExportBundle {
            exported_at: Utc::now(),
            documents,
        })
    }

    async fn refresh(
        &self,
        context: &MutationContext,
        account_id: ProviderAccountId,
    ) -> Result<AccountRefreshResult, AdminError> {
        let (stored, provider) = self.provider_for_account(&account_id).await?;
        let account = stored.account;
        let prepared = provider
            .prepare_refresh(PrepareCredentialRefresh {
                account: account.clone(),
            })
            .await
            .map_err(|error| map_provider_error(error, "provider credential refresh"))?;
        validate_prepared_rotation(&account, &prepared, "provider credential refresh")?;
        let result = commit_credential_refresh(
            self.accounts.as_ref(),
            prepared,
            context,
            "provider credential refresh",
        )
        .await?;
        provider
            .account_facts_changed(std::slice::from_ref(&result.account_id))
            .await;
        publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
        let account = self.load_directory_item(&result.account_id, false).await?;
        Ok(AccountRefreshResult {
            config_revision: result.config_revision,
            account,
        })
    }

    async fn recover(
        &self,
        context: &MutationContext,
        account_id: ProviderAccountId,
    ) -> Result<AccountRefreshResult, AdminError> {
        let (stored, provider) = self.provider_for_account(&account_id).await?;
        let config_revision = if stored.account.enabled {
            self.accounts
                .recover_account(&account_id, context)
                .await
                .map_err(|error| map_store_error(error, "provider account recovery"))?
                .config_revision
        } else {
            // 停用只表示不参与调度，重新启用不能抹除已观测的额度、凭据或冷却事实。
            self.accounts
                .batch_update_accounts(
                    BatchUpdateAccounts {
                        model_access: None,
                        custom_name: None,
                        account_ids: vec![account_id.to_string()],
                        enabled: Some(true),
                        turn_state_injection_enabled: None,
                        concurrency_limit: None,
                        weight: None,
                        group_ids: None,
                        outbound_proxy: None,
                    },
                    context,
                )
                .await
                .map_err(|error| map_store_error(error, "enable provider account"))?
                .config_revision
        };
        provider
            .account_facts_changed(std::slice::from_ref(&account_id))
            .await;
        publish_committed(self.snapshot.as_ref(), config_revision).await?;
        let account = self.load_directory_item(&account_id, false).await?;
        Ok(AccountRefreshResult {
            config_revision,
            account,
        })
    }

    async fn update(
        &self,
        context: &MutationContext,
        mut command: UpdateAccount,
    ) -> Result<AccountUpdateResult, AdminError> {
        command.custom_name = command
            .custom_name
            .map(|value| crate::model::accounts::normalize_custom_name(value.as_deref()))
            .transpose()?;
        let account_id = ProviderAccountId::new(command.account_id.clone())
            .map_err(|_| AdminError::invalid("Provider 账号 ID 不合法"))?;
        let (_, provider) = self.provider_for_account(&account_id).await?;
        let enabled = command.enabled;
        let result = self
            .accounts
            .update_account(command, context)
            .await
            .map_err(|error| map_store_error(error, "provider account"))?;
        if !enabled {
            provider.account_unavailable(&account_id).await;
        }
        provider
            .account_facts_changed(std::slice::from_ref(&account_id))
            .await;
        publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
        Ok(result)
    }

    async fn batch_update(
        &self,
        context: &MutationContext,
        mut command: BatchUpdateAccounts,
    ) -> Result<AccountsUpdateResult, AdminError> {
        command.custom_name = command
            .custom_name
            .map(|value| crate::model::accounts::normalize_custom_name(value.as_deref()))
            .transpose()?;
        let account_ids = command
            .account_ids
            .iter()
            .map(|id| {
                ProviderAccountId::new(id.clone())
                    .map_err(|_| AdminError::invalid("Provider 账号 ID 不合法"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut providers = BTreeMap::<
            ProviderKind,
            (
                Arc<dyn crate::ports::provider::ProviderAdmin>,
                Vec<ProviderAccountId>,
            ),
        >::new();
        for account_id in &account_ids {
            let (item, provider) = self.provider_for_account(account_id).await?;
            if command.turn_state_injection_enabled == Some(true)
                && item.account.provider_kind.as_str() != "openai"
            {
                return Err(AdminError::invalid(
                    "State 开关仅支持 OpenAI 账号，请调整选择",
                ));
            }
            providers
                .entry(item.account.provider_kind)
                .or_insert_with(|| (provider, Vec::new()))
                .1
                .push(account_id.clone());
        }
        let enabled = command.enabled;
        let result = self
            .accounts
            .batch_update_accounts(command, context)
            .await
            .map_err(|error| map_store_error(error, "provider accounts"))?;
        for (provider, provider_ids) in providers.values() {
            if enabled == Some(false) {
                for account_id in provider_ids {
                    provider.account_unavailable(account_id).await;
                }
            }
            provider.account_facts_changed(provider_ids).await;
        }
        publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
        Ok(result)
    }

    async fn quota(
        &self,
        account_id: &ProviderAccountId,
        refresh: bool,
    ) -> Result<AccountDirectoryItem, AdminError> {
        self.load_directory_item(account_id, refresh).await
    }

    async fn current_quota(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<ProviderQuota, AdminError> {
        let (stored, provider) = self.provider_for_account(account_id).await?;
        let mut quota = provider
            .quota(ProviderQuotaRequest {
                account_id: account_id.clone(),
                refresh: false,
                rolling_usage: None,
            })
            .await
            .map_err(|error| map_provider_error(error, "current quota snapshot"))?;
        self.attach_quota_local_usage(
            std::slice::from_ref(&stored),
            std::slice::from_mut(&mut quota),
        )
        .await?;
        Ok(quota)
    }

    async fn quota_forecast(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<AccountQuotaForecastReport, AdminError> {
        let (stored, provider) = self.provider_for_account(account_id).await?;
        let mut quota = provider
            .quota(ProviderQuotaRequest {
                account_id: account_id.clone(),
                refresh: false,
                rolling_usage: None,
            })
            .await
            .map_err(|error| map_provider_error(error, "forecast quota snapshot"))?;
        let now = Utc::now();
        let mut samples = Vec::new();
        for window in quota.windows.iter().filter(|window| {
            window.local_usage_attribution == QuotaLocalUsageAttribution::AccountWide
                && window
                    .window_seconds
                    .is_some_and(|seconds| seconds <= 32 * 86_400)
        }) {
            let (Some(mut query), Some(observed), Some(percent)) = (
                quota_usage_window(account_id.as_str(), window),
                quota.observed_at,
                window.used_percent,
            ) else {
                continue;
            };
            if observed < query.range.start
                || observed > now
                || now >= query.range.end
                || !percent.is_finite()
                || !(0.0..=100.0).contains(&percent)
            {
                continue;
            }
            let reset_at = query.range.end;
            query.range.start = query.range.start.max(stored.account.created_at);
            if query.range.start >= observed {
                continue;
            }
            query.range.end = observed;
            let history = self
                .accounts
                .load_quota_forecast_history(&query)
                .await
                .map_err(|error| map_store_error(error, "forecast paired usage"))?;
            let mut points = Vec::new();
            let mut interrupted = false;
            for point in history.points {
                let Some(fact) =
                    provider.quota_forecast_observation(&point.provider_observation, window)
                else {
                    continue;
                };
                let same_plan = quota
                    .plan_type
                    .as_deref()
                    .zip(fact.plan_type.as_deref())
                    .is_some_and(|(current, previous)| current.eq_ignore_ascii_case(previous));
                // 仅容纳已观测到的秒级量化抖动，不用宽时间容差合并实际重置。
                // 不匹配的段截断基线；之后的有效观测可以重新积累。
                if !same_plan || (fact.reset_at - reset_at).abs() > Duration::seconds(2) {
                    points.clear();
                    interrupted = true;
                    continue;
                }
                points.push(QuotaForecastPoint {
                    observed_at: point.completed_at,
                    used_percent: fact.used_percent,
                    usage: point.usage,
                });
            }
            let sample = select_forecast_sample(
                window.key.clone(),
                query.range.start,
                QuotaForecastPoint {
                    observed_at: observed,
                    used_percent: percent,
                    usage: history.usage,
                },
                points,
                history.pending_request_count,
                interrupted,
            );
            samples.push(sample);
        }
        self.attach_quota_local_usage(
            std::slice::from_ref(&stored),
            std::slice::from_mut(&mut quota),
        )
        .await?;
        let mut report = AccountQuotaForecastReport {
            account_id: account_id.to_string(),
            generated_at: now,
            forecasts: account_quota_forecasts(&quota, stored.account.created_at, now, &samples),
            learned_windows: Vec::new(),
        };
        apply_current_quota_estimates(&quota, &mut report);
        Ok(report)
    }

    async fn personal_info(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<AccountPersonalInfo, AdminError> {
        let (initial, provider) = self.provider_for_account(account_id).await?;
        // 两项读取相互独立；不因其中一项失败而取消另一项，也不触发凭据或额度刷新。
        let (profile, subscription) = futures::join!(
            provider.profile_statistics(account_id),
            provider.subscription(account_id),
        );
        let profile =
            profile.map_err(|error| map_provider_error(error, "provider profile statistics"));
        let subscription = subscription
            .map_err(|error| map_provider_error(error, "provider subscription"))
            .ok()
            .flatten();

        // 汇聚等待期间发生重新授权、换绑或删除时，不返回混合身份的数据。
        let current = self.load_account(account_id).await?;
        if current.account.provider_kind != initial.account.provider_kind
            || current.account.credential_revision != initial.account.credential_revision
            || current.account.upstream_user_id != initial.account.upstream_user_id
            || current.account.upstream_account_id != initial.account.upstream_account_id
        {
            return Err(AdminError::conflict("账号身份已变化，请刷新信息后重试"));
        }
        Ok(AccountPersonalInfo {
            profile,
            subscription,
        })
    }

    async fn profile_avatar(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<ProviderProfileAvatar, AdminError> {
        let (_, provider) = self.provider_for_account(account_id).await?;
        provider
            .profile_avatar(account_id)
            .await
            .map_err(|error| map_provider_error(error, "provider profile avatar"))
    }

    async fn reset_credits(
        &self,
        context: &MutationContext,
        account_id: ProviderAccountId,
    ) -> Result<ProviderResetCredits, AdminError> {
        let (_, provider) = self.provider_for_account(&account_id).await?;
        match provider.reset_credits(&account_id).await {
            Ok(credits) => Ok(credits),
            Err(error)
                if error.kind()
                    == crate::ports::provider::ProviderAdminErrorKind::CredentialRefreshRequired =>
            {
                self.refresh(context, account_id.clone()).await?;
                let (_, provider) = self.provider_for_account(&account_id).await?;
                provider
                    .reset_credits(&account_id)
                    .await
                    .map_err(map_reset_credits_error_after_refresh)
            }
            Err(error) => Err(map_provider_error(error, "provider reset credits")),
        }
    }

    async fn consume_reset_credit(
        &self,
        context: &MutationContext,
        command: ConsumeProviderResetCredit,
    ) -> Result<ProviderResetCreditResult, AdminError> {
        let account_id = command.account_id.clone();
        // 覆盖 credential refresh + 同键重试的完整账号级临界区，避免 401 两次调用
        // 之间插入另一笔不可逆消费。
        let lock = self.reset_credit_lock(&account_id).await;
        let _guard = lock.lock().await;
        let (_, provider) = self.provider_for_account(&account_id).await?;
        match provider.consume_reset_credit(command.clone()).await {
            Ok(result) => Ok(result),
            Err(error)
                if error.kind()
                    == crate::ports::provider::ProviderAdminErrorKind::CredentialRefreshRequired =>
            {
                self.refresh(context, account_id.clone()).await?;
                let (_, provider) = self.provider_for_account(&account_id).await?;
                provider
                    .consume_reset_credit(command)
                    .await
                    .map_err(map_reset_credits_error_after_refresh)
            }
            Err(error) => Err(map_provider_error(error, "provider reset-credit consume")),
        }
    }

    async fn models(
        &self,
        account_id: &ProviderAccountId,
        refresh: bool,
    ) -> Result<ProviderModels, AdminError> {
        let (_, provider) = self.provider_for_account(account_id).await?;
        provider
            .models(account_id, refresh)
            .await
            .map_err(|error| map_provider_error(error, "provider model catalog"))
    }

    async fn request_turn_state_probe(
        &self,
        account_id: &ProviderAccountId,
        model: &UpstreamModelId,
    ) -> Result<TurnStateProbeOutcome, AdminError> {
        let (_, provider) = self.provider_for_account(account_id).await?;
        provider
            .request_turn_state_probe(account_id, model)
            .await
            .map_err(|error| map_provider_error(error, "provider turn state probe"))
    }

    async fn model_catalog_document(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<crate::model::provider_credentials::ProviderModelCatalogDocument, AdminError> {
        let (_, provider) = self.provider_for_account(account_id).await?;
        provider
            .model_catalog_document(account_id)
            .await
            .map_err(|error| map_provider_error(error, "provider native model catalog"))
    }

    async fn test_connection(
        &self,
        command: AccountConnectionTest,
    ) -> Result<AccountConnectionTestEventStream, AdminError> {
        let AccountConnectionTest {
            account_id,
            upstream_model,
            endpoint,
            input_text,
            stream,
        } = command;
        let (stored, provider) = self.provider_for_account(&account_id).await?;
        let account = stored.account;
        let model = upstream_model.as_str().to_owned();
        let operation = provider
            .connection_test_operation_with_options(&upstream_model, &input_text, endpoint, stream)
            .map_err(|error| map_provider_error(error, "provider connection test"))?;
        let initial = vec![
            AccountConnectionTestEvent::Started {
                model: model.clone(),
                endpoint,
            },
            AccountConnectionTestEvent::Request {
                model,
                input_text,
                endpoint,
                stream,
                store: false,
            },
        ];
        let probe = Arc::clone(&self.probe);
        let terminal = futures::stream::once(async move {
            let result = probe
                .probe(AccountProbeRequest {
                    account_id,
                    provider_kind: account.provider_kind,
                    upstream_model,
                    operation,
                })
                .await;
            match result {
                Ok(result) => {
                    let text = if stream || result.text.is_empty() {
                        result.text
                    } else {
                        vec![result.text.concat()]
                    };
                    text.into_iter()
                        .map(|text| AccountConnectionTestEvent::Content { text })
                        .chain(std::iter::once(AccountConnectionTestEvent::Completed {
                            upstream_response_model: result.upstream_response_model,
                        }))
                        .collect()
                }
                Err(error) => {
                    let upstream_status = error
                        .upstream_response()
                        .map(gateway_core::engine::probe::AccountProbeUpstreamResponse::status);
                    let upstream_content_type = error
                        .upstream_response()
                        .and_then(|response| response.content_type())
                        .and_then(|value| std::str::from_utf8(value).ok())
                        .map(ToOwned::to_owned);
                    let upstream_body = error
                        .upstream_response()
                        .map(|response| String::from_utf8_lossy(response.body()).into_owned());
                    let message = error.client_message().to_owned();
                    vec![AccountConnectionTestEvent::Failed {
                        source: error.source(),
                        gateway_error_code: error.kind(),
                        send_state: error.send_state(),
                        message,
                        provider_error_code: error.client_error_code().map(ToOwned::to_owned),
                        provider_error_type: error.client_error_type().map(ToOwned::to_owned),
                        upstream_status,
                        upstream_content_type,
                        upstream_body,
                    }]
                }
            }
        })
        .flat_map(futures::stream::iter);
        Ok(Box::pin(futures::stream::iter(initial).chain(terminal)))
    }
}

fn map_reset_credits_error_after_refresh(
    error: crate::ports::provider::ProviderAdminError,
) -> AdminError {
    if error.kind() == crate::ports::provider::ProviderAdminErrorKind::CredentialRefreshRequired {
        return AdminError::bad_gateway("上游服务拒绝了刷新后的凭据");
    }
    map_provider_error(error, "provider reset credits")
}

/// 账号目录中单个账号 quota 读取失败时使用的空额度投影。
fn empty_quota() -> ProviderQuota {
    ProviderQuota {
        plan_type: None,
        observed_at: None,
        refresh_token_expires_at: None,
        windows: Vec::new(),
        limit_reached: false,
        provider_data: None,
    }
}

fn quota_usage_window(
    account_id: &str,
    window: &ProviderQuotaWindow,
) -> Option<AccountUsageWindowQuery> {
    if window.local_usage_attribution != QuotaLocalUsageAttribution::AccountWide {
        return None;
    }
    let reset_at = window.reset_at?;
    let seconds = i64::try_from(window.window_seconds?).ok()?;
    let start = reset_at.checked_sub_signed(Duration::try_seconds(seconds)?)?;
    let range = TimeRange::new(start, reset_at).ok()?;
    // 上游百分比以该 reset 边界定义；以当前时间回推会让本地 Token 属于另一窗口。
    Some(AccountUsageWindowQuery {
        account_id: account_id.to_owned(),
        key: window.key.clone(),
        range,
    })
}
