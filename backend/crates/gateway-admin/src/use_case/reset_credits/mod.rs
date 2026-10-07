use super::{accounts::AccountsService, map_store_error};
use crate::{
    model::{
        AdminError, AdminErrorKind, MutationContext,
        provider_credentials::ConsumeProviderResetCredit, reset_credits::*,
    },
    ports::reset_credits::ResetCreditsStore,
};
use chrono::Utc;
use futures::{StreamExt, stream};
use gateway_core::account::ProviderAccountId;
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use uuid::Uuid;

mod automatic;

pub struct ResetCreditsService {
    pub(crate) accounts: Arc<dyn AccountsService>,
    pub(crate) store: Option<Arc<dyn ResetCreditsStore>>,
}

impl ResetCreditsService {
    fn store(&self) -> Result<&dyn ResetCreditsStore, AdminError> {
        self.store
            .as_deref()
            .ok_or_else(|| AdminError::unavailable("重置任务存储未配置"))
    }

    fn ids(ids: Vec<String>) -> Result<Vec<String>, AdminError> {
        let ids: BTreeSet<_> = ids.into_iter().collect();
        if ids.is_empty() || ids.len() > 500 {
            return Err(AdminError::invalid("请选择 1 至 500 个账号"));
        }
        for id in &ids {
            ProviderAccountId::new(id.clone())
                .map_err(|_| AdminError::invalid("账号标识不合法"))?;
        }
        Ok(ids.into_iter().collect())
    }

    pub async fn inventories(&self, ids: Vec<String>) -> Result<Vec<ResetInventory>, AdminError> {
        self.store()?
            .inventories(&Self::ids(ids)?)
            .await
            .map_err(|e| map_store_error(e, "reset credits"))
    }

    pub async fn refresh(
        &self,
        ids: Vec<String>,
        context: &MutationContext,
    ) -> Result<Vec<ResetInventory>, AdminError> {
        let ids = Self::ids(ids)?;
        let results = stream::iter(ids.iter().cloned().map(|id| async move {
            let account_id = ProviderAccountId::new(id.clone())
                .map_err(|_| AdminError::invalid("账号标识不合法"))?;
            let checked_at = Utc::now();
            let result = tokio::time::timeout(
                Duration::from_secs(45),
                self.accounts.reset_credits(context, account_id),
            )
            .await;
            let (credits, error) = match result {
                Ok(Ok(credits)) => (Some(credits), None),
                Ok(Err(e)) => (None, Some(e.message().to_owned())),
                Err(_) => (None, Some("重置次数查询超时".to_owned())),
            };
            self.store()?
                .save_inventory(ResetInventory {
                    account_id: id,
                    checked_at,
                    credits,
                    error,
                    pending: None,
                })
                .await
                .map_err(|e| map_store_error(e, "reset credits"))
        }))
        .buffer_unordered(3)
        .collect::<Vec<_>>()
        .await;
        for result in results {
            result?;
        }
        self.inventories(ids).await
    }

    pub async fn preview(
        &self,
        ids: Vec<String>,
        reset_type: Option<String>,
        context: &MutationContext,
    ) -> Result<ResetBatch, AdminError> {
        let ids = Self::ids(ids)?;
        if reset_type
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 128)
        {
            return Err(AdminError::invalid("重置类型不合法"));
        }
        let inventories = self.refresh(ids.clone(), context).await?;
        let now = Utc::now();
        let items = ids
            .into_iter()
            .map(|id| {
                let inventory = inventories.iter().find(|i| i.account_id == id);
                let selection = match inventory {
                    Some(i) if i.pending.is_some() => Err(AdminError::conflict(
                        "有待确认的重置操作，请先继续确认原操作",
                    )),
                    Some(i) if i.error.is_some() => Err(AdminError::invalid(
                        i.error.as_deref().unwrap_or("查询失败"),
                    )),
                    Some(i) => i
                        .credits
                        .as_ref()
                        .ok_or_else(|| AdminError::invalid("尚未取得重置卡"))
                        .and_then(|credits| earliest_credit(credits, reset_type.as_deref(), now)),
                    None => Err(AdminError::invalid("未取得账号重置次数")),
                };
                let (credit, status, message) = match selection {
                    Ok(credit) => (Some(credit), ResetItemStatus::Ready, "等待确认".into()),
                    Err(e) => (None, ResetItemStatus::Skipped, e.message().to_owned()),
                };
                ResetBatchItem {
                    account_id: id,
                    available_count: inventory
                        .and_then(|i| i.credits.as_ref())
                        .map(|i| i.available_count),
                    credit,
                    redeem_request_id: Uuid::new_v4(),
                    status,
                    message,
                    updated_at: now,
                    claim_id: None,
                    retry: false,
                }
            })
            .collect();
        self.store()?
            .save_preview(ResetBatch {
                id: Uuid::new_v4(),
                created_at: now,
                confirmed: false,
                reset_type,
                items,
                context: None,
            })
            .await
            .map_err(|e| map_store_error(e, "reset credits"))
    }

    pub async fn batches(&self) -> Result<Vec<ResetBatch>, AdminError> {
        self.store()?
            .batches()
            .await
            .map_err(|e| map_store_error(e, "reset credits"))
    }
    pub async fn confirm(
        &self,
        id: Uuid,
        context: &MutationContext,
    ) -> Result<ResetBatch, AdminError> {
        self.store()?
            .confirm(id, context)
            .await
            .map_err(|e| map_store_error(e, "reset credits"))
    }
    pub async fn retry(
        &self,
        id: Uuid,
        account_id: &str,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        self.store()?
            .retry(id, account_id, context)
            .await
            .map_err(|e| map_store_error(e, "reset credits"))
    }

    pub(crate) async fn run_one(&self) -> Result<(), AdminError> {
        let Some(store) = self.store.as_deref() else {
            return Ok(());
        };
        let Some((batch, mut item)) = store
            .claim()
            .await
            .map_err(|e| map_store_error(e, "reset credits"))?
        else {
            return Ok(());
        };
        let result =
            tokio::time::timeout(Duration::from_secs(100), self.execute(&batch, &item)).await;
        match result {
            Ok(Ok((status, message))) => {
                item.status = status;
                item.message = message;
            }
            Ok(Err(e)) => {
                item.status = if e.kind() == AdminErrorKind::UpstreamResultUnknown {
                    ResetItemStatus::Unknown
                } else {
                    ResetItemStatus::Failed
                };
                item.message = e.message().to_owned();
            }
            Err(_) => {
                item.status = ResetItemStatus::Unknown;
                item.message = "执行超时，结果待确认；不会自动换卡重试".into();
            }
        }
        // Readback failure must never turn a confirmed consumption into a retryable failure.
        if matches!(
            item.status,
            ResetItemStatus::Succeeded | ResetItemStatus::Skipped
        ) && let Some(context) = batch.context.as_ref()
        {
            if !matches!(tokio::time::timeout(
                Duration::from_secs(15),
                self.refresh(vec![item.account_id.clone()], context),
            ).await, Ok(Ok(rows)) if rows.iter().all(|row| row.error.is_none()))
            {
                item.message.push_str("；次数刷新失败");
            }
            if item.status == ResetItemStatus::Succeeded
                && let Ok(id) = ProviderAccountId::new(item.account_id.clone())
                && !matches!(
                    tokio::time::timeout(Duration::from_secs(15), self.accounts.quota(&id, true))
                        .await,
                    Ok(Ok(_))
                )
            {
                item.message.push_str("；额度刷新失败");
            }
        }
        store
            .finish_item(batch.id, item)
            .await
            .map_err(|e| map_store_error(e, "reset credits"))
    }

    async fn execute(
        &self,
        batch: &ResetBatch,
        item: &ResetBatchItem,
    ) -> Result<(ResetItemStatus, String), AdminError> {
        let context = batch
            .context
            .as_ref()
            .ok_or_else(|| AdminError::internal("重置任务缺少审计上下文"))?;
        let account_id = ProviderAccountId::new(item.account_id.clone())
            .map_err(|_| AdminError::invalid("账号标识不合法"))?;
        let credit = item
            .credit
            .as_ref()
            .ok_or_else(|| AdminError::invalid("未选定重置卡"))?;
        if !item.retry {
            if let Some((config, previous)) = self
                .store()?
                .auto_execution(item.redeem_request_id)
                .await
                .map_err(|e| map_store_error(e, "auto reset admission"))?
            {
                let current = self.fresh_auto_observation(&account_id, &config).await?;
                if !current.overlaps(&previous.triggered) {
                    return Ok((
                        ResetItemStatus::Skipped,
                        "额度窗口已恢复或变化，本次未消费".into(),
                    ));
                }
            }
            let current = self
                .accounts
                .reset_credits(context, account_id.clone())
                .await?;
            match earliest_credit(&current, batch.reset_type.as_deref(), Utc::now()) {
                Ok(selected) if &selected == credit => {}
                _ => {
                    return Ok((
                        ResetItemStatus::Skipped,
                        "卡片库存或有效期已变化，请重新预览；本次未消费".into(),
                    ));
                }
            }
        }
        let result = self
            .accounts
            .consume_reset_credit(
                context,
                ConsumeProviderResetCredit {
                    account_id,
                    credit_id: Some(credit.id.clone()),
                    redeem_request_id: item.redeem_request_id,
                },
            )
            .await?;
        let succeeded = result.code == "reset" || (result.code == "already_redeemed" && item.retry);
        let message = match result.code.as_str() {
            "reset" => "额度已重置",
            "already_redeemed" if item.retry => "原重置操作已完成",
            "already_redeemed" => "该卡已被使用，本次未重复消费",
            "no_credit" => "没有可用重置次数",
            "nothing_to_reset" => "当前额度无需重置",
            _ => "上游未执行重置",
        }
        .to_owned();
        Ok((
            if succeeded {
                ResetItemStatus::Succeeded
            } else {
                ResetItemStatus::Skipped
            },
            message,
        ))
    }
}
