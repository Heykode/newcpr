use super::*;
use crate::model::{
    MutationActor, PageSize,
    accounts::{AccountGroupFilter, AccountListQuery, AccountStatus},
};

impl QualityOpsService {
    pub async fn group_rules(&self) -> Result<Vec<QualityGroupRule>, AdminError> {
        self.store()?
            .group_rules()
            .await
            .map_err(|e| map_store_error(e, "quality groups"))
    }

    pub async fn save_group_rule(
        &self,
        id: Option<&str>,
        revision: Option<i64>,
        name: String,
        filter: QualityGroupFilter,
        config: QualityRuleConfig,
        context: &MutationContext,
    ) -> Result<QualityGroupUpdate, AdminError> {
        if id.is_some() != revision.is_some() || revision.is_some_and(|n| n < 1) {
            return Err(AdminError::invalid("编辑分组规则必须提供有效版本"));
        }
        let name = name.trim().to_owned();
        if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
            return Err(AdminError::invalid("分组规则名称必须为 1–128 个字符"));
        }
        if !config.account_id.is_empty() {
            return Err(AdminError::invalid("分组规则不能绑定单个账号"));
        }
        validate_config(&config, false)?;
        if (!filter.group.is_empty()
            && filter.group != "ungrouped"
            && AccountGroupId::new(filter.group.as_str()).is_err())
            || filter.statuses.len() > 5
            || filter
                .statuses
                .iter()
                .any(|s| AccountStatus::parse(s).is_none())
            || filter
                .statuses
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != filter.statuses.len()
        {
            return Err(AdminError::invalid("请选择有效的被测分组和账号状态"));
        }
        let group = self
            .store()?
            .save_group_rule(id, revision, name, filter, config, context)
            .await
            .map_err(|e| map_store_error(e, "quality group"))?;
        // The parent is already committed. Report sync failure without implying rollback.
        let sync = self
            .sync_group_rule(&group, context)
            .await
            .unwrap_or(QualityGroupSync {
                failed: 1,
                ..Default::default()
            });
        let group = self
            .group_rules()
            .await
            .ok()
            .and_then(|groups| groups.into_iter().find(|g| g.id == group.id))
            .unwrap_or(group);
        Ok(QualityGroupUpdate { group, sync })
    }

    pub async fn delete_group_rule(
        &self,
        id: &str,
        revision: i64,
        delete_rules: bool,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        self.store()?
            .delete_group_rule(id, revision, delete_rules, context)
            .await
            .map_err(|e| map_store_error(e, "quality group"))
    }

    async fn apply_group_target(
        &self,
        group: &QualityGroupRule,
        target: QualityTemplateTarget,
        context: &MutationContext,
    ) -> Result<bool, AdminError> {
        let mut config = group.config.clone();
        config.account_id.clone_from(&target.account_id);
        self.validate_target(&config).await?;
        self.store()?
            .apply_group_rule(group, &target, next_run(&config, Utc::now())?, context)
            .await
            .map_err(|e| map_store_error(e, "quality group target"))
    }

    async fn sync_group_rule(
        &self,
        group: &QualityGroupRule,
        context: &MutationContext,
    ) -> Result<QualityGroupSync, AdminError> {
        let mut result = QualityGroupSync::default();
        let mut after = String::new();
        loop {
            let targets = self
                .store()?
                .group_targets(&group.id, &after)
                .await
                .map_err(|e| map_store_error(e, "quality group targets"))?;
            if targets.is_empty() {
                break;
            }
            for target in targets {
                after.clone_from(&target.account_id);
                match self.apply_group_target(group, target, context).await {
                    Ok(true) => result.updated += 1,
                    Ok(false) => {}
                    Err(_) => result.failed += 1,
                }
            }
        }
        if group.config.enabled {
            let runtime = self
                .runtime
                .active_rate_limits()
                .await
                .map_err(|e| map_store_error(e, "quality account states"))?;
            let statuses = if group.filter.statuses.is_empty() {
                vec![None]
            } else {
                group
                    .filter
                    .statuses
                    .iter()
                    .map(|s| AccountStatus::parse(s))
                    .collect()
            };
            for status in statuses {
                let mut page = 1;
                loop {
                    let group_filter = match group.filter.group.as_str() {
                        "" => None,
                        "ungrouped" => Some(AccountGroupFilter::Ungrouped),
                        id => Some(AccountGroupFilter::Group(
                            AccountGroupId::new(id)
                                .map_err(|_| AdminError::invalid("被测分组不合法"))?,
                        )),
                    };
                    let accounts = self
                        .accounts
                        .list_accounts(
                            AccountListQuery {
                                page,
                                page_size: PageSize::new(100).map_err(|_| {
                                    AdminError::internal("invalid quality page size")
                                })?,
                                provider_kind: Some(
                                    gateway_core::routing::ProviderKind::new("openai")
                                        .map_err(|_| AdminError::internal("invalid provider"))?,
                                ),
                                group_filter,
                                search: None,
                                status,
                                plan_type: None,
                                sort: None,
                            },
                            runtime.clone(),
                        )
                        .await
                        .map_err(|e| map_store_error(e, "quality matching accounts"))?;
                    let count = accounts.items.len();
                    for item in accounts.items {
                        if item.account.authentication_kind != "oauth" {
                            continue;
                        }
                        let target = QualityTemplateTarget {
                            account_id: item.account.id.to_string(),
                            rule_id: None,
                            revision: None,
                        };
                        // An existing rule belongs to its current owner, never to a new matching group.
                        if item.account.quality_monitoring.is_some() {
                            continue;
                        }
                        match self.apply_group_target(group, target, context).await {
                            Ok(true) => result.created += 1,
                            Ok(false) => {}
                            Err(_) => result.failed += 1,
                        }
                    }
                    if count < 100 {
                        break;
                    }
                    page = page
                        .checked_add(1)
                        .ok_or_else(|| AdminError::internal("quality page overflow"))?;
                    tokio::task::yield_now().await;
                }
            }
        }
        self.store()?
            .mark_group_synced(&group.id, group.revision)
            .await
            .map_err(|e| map_store_error(e, "quality group sync"))?;
        Ok(result)
    }

    pub(crate) async fn sync_groups(&self) -> Result<(), AdminError> {
        if self.store.is_none() {
            return Ok(());
        }
        let context = MutationContext {
            actor: MutationActor::System,
            request_id: "quality-group-sync".into(),
        };
        for group in self.group_rules().await? {
            match self.sync_group_rule(&group, &context).await {
                Ok(result) if result.failed > 0 => tracing::warn!(
                    failed = result.failed,
                    "quality group enrollment partially failed; pending targets will be retried"
                ),
                Err(_) => {
                    tracing::warn!("quality group enrollment failed; existing checks unchanged")
                }
                Ok(_) => {}
            }
        }
        Ok(())
    }
}
