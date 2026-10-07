use std::collections::BTreeMap;

use super::*;
use crate::model::{
    PageSize,
    accounts::{AccountGroupFilter, AccountListQuery, AccountStatus},
};

impl QualityOpsService {
    pub async fn model_choices(
        &self,
        query: QualityModelQuery,
    ) -> Result<QualityModelPage, AdminError> {
        if query.page == 0
            || query.page > 1_000_000
            || query.statuses.len() > 5
            || query
                .statuses
                .iter()
                .any(|s| AccountStatus::parse(s).is_none())
            || (!query.account_ids.is_empty()
                && (!query.group.is_empty() || !query.statuses.is_empty()))
        {
            return Err(AdminError::invalid("模型目录范围不合法"));
        }
        if !query.account_ids.is_empty() {
            validate_monitor_accounts(&query.account_ids)?;
        }
        let group_filter = match query.group.as_str() {
            "" => None,
            "ungrouped" => Some(AccountGroupFilter::Ungrouped),
            id => Some(AccountGroupFilter::Group(
                AccountGroupId::new(id).map_err(|_| AdminError::invalid("模型目录分组不合法"))?,
            )),
        };
        let runtime = self
            .runtime
            .active_rate_limits()
            .await
            .map_err(|e| map_store_error(e, "quality model scope"))?;
        let offset = (query.page as usize - 1) * 100;
        let (accounts, has_more) = if query.account_ids.is_empty() {
            let page = self
                .accounts
                .list_accounts(
                    AccountListQuery {
                        page: query.page,
                        page_size: PageSize::new(100)
                            .map_err(|_| AdminError::internal("invalid page size"))?,
                        provider_kind: Some(
                            gateway_core::routing::ProviderKind::new("openai")
                                .map_err(|_| AdminError::internal("invalid provider"))?,
                        ),
                        group_filter,
                        search: None,
                        status: None,
                        plan_type: None,
                        sort: None,
                    },
                    runtime,
                )
                .await
                .map_err(|e| map_store_error(e, "quality model accounts"))?;
            (page.items, (offset as u64 + 100) < page.total)
        } else {
            let mut accounts = Vec::new();
            for id in query.account_ids.iter().skip(offset).take(100) {
                if let Some(account) = self
                    .accounts
                    .load_account(id, runtime.clone())
                    .await
                    .map_err(|e| map_store_error(e, "quality model account"))?
                {
                    accounts.push(account);
                }
            }
            (accounts, offset + 100 < query.account_ids.len())
        };
        let exact = query.account_ids.len() == 1;
        let mut result = QualityModelPage {
            next_page: has_more.then_some(query.page + 1),
            ..Default::default()
        };
        let mut choices = BTreeMap::new();
        for item in accounts {
            if item.account.authentication_kind != "oauth"
                || (!query.statuses.is_empty()
                    && !query
                        .statuses
                        .iter()
                        .any(|s| s == item.projection.status.as_str()))
            {
                continue;
            }
            result.matched_accounts += 1;
            let provider = self
                .providers
                .require(&item.account.provider_kind)
                .map_err(|e| map_provider_error(e, "quality model provider"))?;
            let account_id = ProviderAccountId::new(item.account.id)
                .map_err(|_| AdminError::internal("invalid account id"))?;
            match provider.quality_model_choices(&account_id, exact).await {
                Ok(models) => {
                    result.known_accounts += usize::from(!models.is_empty());
                    for mut model in models {
                        if !exact {
                            model.reasoning_efforts = None;
                        }
                        choices.entry(model.id.clone()).or_insert(model);
                    }
                }
                Err(error) if exact => {
                    return Err(map_provider_error(error, "quality model catalog"));
                }
                Err(_) => result.failed_accounts += 1,
            }
        }
        result.models = choices.into_values().collect();
        Ok(result)
    }
}
