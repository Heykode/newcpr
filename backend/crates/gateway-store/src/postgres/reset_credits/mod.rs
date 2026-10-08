use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use gateway_admin::{
    model::{
        MutationContext,
        provider_credentials::{ConsumeProviderResetCredit, ProviderResetCreditResult},
        reset_credits::*,
    },
    ports::{
        reset_credits::ResetCreditsStore,
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
    },
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

mod automatic;

pub struct PgResetCreditsStore(pub PgPool);

fn unavailable(_: impl std::fmt::Display) -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "reset credits",
        "重置任务存储暂不可用",
    )
}
fn conflict(message: &str) -> AdminStoreError {
    AdminStoreError::new(AdminStoreErrorKind::Conflict, "reset credits", message)
}
fn encode(value: &impl serde::Serialize) -> AdminStoreResult<Value> {
    serde_json::to_value(value).map_err(unavailable)
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> AdminStoreResult<T> {
    serde_json::from_value(value).map_err(unavailable)
}

impl PgResetCreditsStore {
    async fn identity(tx: &mut Transaction<'_, Postgres>, id: &str) -> AdminStoreResult<Value> {
        sqlx::query_scalar(
            "select jsonb_build_array(provider_kind,upstream_user_id,upstream_account_id,extract(epoch from created_at))
             from provider_accounts where id=$1",
        ).bind(id).fetch_optional(&mut **tx).await.map_err(unavailable)?
            .ok_or_else(|| conflict("账号已删除"))
    }
    // Only database state transitions share this short transaction lock. Never hold it over HTTP.
    async fn locked(&self) -> AdminStoreResult<Transaction<'_, Postgres>> {
        let mut tx = self.0.begin().await.map_err(unavailable)?;
        sqlx::query("set local lock_timeout='3s'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("select pg_advisory_xact_lock(82061001)")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        Ok(tx)
    }

    async fn read_batch(
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
    ) -> AdminStoreResult<ResetBatch> {
        let row: Option<(Value, Option<Value>)> =
            sqlx::query_as("select document,actor from account_reset_batches where id=$1")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(unavailable)?;
        let (document, actor) = row.ok_or_else(|| conflict("重置任务不存在"))?;
        let mut batch: ResetBatch = decode(document)?;
        batch.context = actor.map(decode).transpose()?;
        Ok(batch)
    }

    async fn write_batch(
        tx: &mut Transaction<'_, Postgres>,
        batch: &ResetBatch,
    ) -> AdminStoreResult<()> {
        sqlx::query("update account_reset_batches set document=$2,actor=$3 where id=$1")
            .bind(batch.id)
            .bind(encode(batch)?)
            .bind(batch.context.as_ref().map(encode).transpose()?)
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    async fn audit(
        tx: &mut Transaction<'_, Postgres>,
        context: &MutationContext,
        action: &str,
        id: &str,
    ) -> AdminStoreResult<()> {
        super::insert_admin_audit_event(
            tx,
            crate::mutation_audit(context, action, "reset_credits", id, vec![]),
        )
        .await
        .map_err(unavailable)
    }

    async fn pending(&self, ids: &[String]) -> AdminStoreResult<Vec<(String, ResetPending)>> {
        let rows: Vec<(String, Uuid, Option<String>, DateTime<Utc>)> = sqlx::query_as(
            "select account_id,request_id,credit_id,updated_at + interval '180 seconds'
             from account_reset_consumptions where account_id=any($1) and status in ('running','unknown')",
        ).bind(ids).fetch_all(&self.0).await.map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(account_id, redeem_request_id, credit_id, retry_after)| {
                (
                    account_id,
                    ResetPending {
                        redeem_request_id,
                        credit_id,
                        retry_after,
                    },
                )
            })
            .collect())
    }
}

#[async_trait]
impl ResetCreditsStore for PgResetCreditsStore {
    async fn auto_policy(&self, account_id: &str) -> AdminStoreResult<AutoResetPolicy> {
        self.auto_policy_inner(account_id).await
    }
    async fn save_auto_policy(
        &self,
        account_id: &str,
        revision: i64,
        config: AutoResetConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<AutoResetPolicy> {
        self.save_auto_inner(account_id, revision, config, context)
            .await
    }
    async fn claim_auto_check(&self) -> AdminStoreResult<Option<AutoResetCheck>> {
        self.claim_auto_inner().await
    }
    async fn finish_auto_check(
        &self,
        check: &AutoResetCheck,
        observation: Option<AutoResetObservation>,
        credit: Option<gateway_admin::model::provider_credentials::ProviderResetCredit>,
        message: &str,
    ) -> AdminStoreResult<()> {
        self.finish_auto_inner(check, observation, credit, message)
            .await
    }
    async fn auto_execution(
        &self,
        request: Uuid,
    ) -> AdminStoreResult<Option<(AutoResetConfig, AutoResetObservation)>> {
        let mut tx = self.locked().await?;
        Self::validate_auto_send(&mut tx, request, false).await?;
        let row: Option<(Value, Value)> = sqlx::query_as(
            "select p.config,j.observation from account_auto_reset_jobs j
            join account_auto_reset_policies p on p.account_id=j.account_id where j.request_id=$1",
        )
        .bind(request)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        row.map(|(config, observation)| Ok((decode(config)?, decode(observation)?)))
            .transpose()
    }
    async fn inventories(&self, ids: &[String]) -> AdminStoreResult<Vec<ResetInventory>> {
        let values: Vec<Value> = sqlx::query_scalar(
            "select document from account_reset_inventories where account_id=any($1)",
        )
        .bind(ids)
        .fetch_all(&self.0)
        .await
        .map_err(unavailable)?;
        let mut rows: Vec<ResetInventory> = values
            .into_iter()
            .map(decode)
            .collect::<AdminStoreResult<_>>()?;
        for (id, pending) in self.pending(ids).await? {
            if let Some(row) = rows.iter_mut().find(|row| row.account_id == id) {
                row.pending = Some(pending);
            } else {
                rows.push(ResetInventory {
                    account_id: id,
                    checked_at: Utc::now(),
                    credits: None,
                    error: None,
                    pending: Some(pending),
                });
            }
        }
        Ok(rows)
    }

    async fn save_inventory(&self, mut inventory: ResetInventory) -> AdminStoreResult<()> {
        inventory.pending = None;
        sqlx::query("insert into account_reset_inventories(account_id,checked_at,document) values($1,$2,$3)
            on conflict(account_id) do update set checked_at=excluded.checked_at,document=excluded.document
            where account_reset_inventories.checked_at <= excluded.checked_at")
            .bind(&inventory.account_id).bind(inventory.checked_at).bind(encode(&inventory)?)
            .execute(&self.0).await.map_err(unavailable)?;
        Ok(())
    }

    async fn save_preview(&self, batch: ResetBatch) -> AdminStoreResult<ResetBatch> {
        let mut tx = self.locked().await?;
        let mut identities = serde_json::Map::new();
        for item in &batch.items {
            identities.insert(
                item.account_id.clone(),
                Self::identity(&mut tx, &item.account_id).await?,
            );
        }
        sqlx::query("insert into account_reset_batches(id,created_at,document,identities) values($1,$2,$3,$4)")
            .bind(batch.id).bind(batch.created_at).bind(encode(&batch)?).bind(Value::Object(identities))
            .execute(&mut *tx).await.map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(batch)
    }

    async fn batches(&self) -> AdminStoreResult<Vec<ResetBatch>> {
        let values: Vec<Value> = sqlx::query_scalar(
            "select document from account_reset_batches where document->>'confirmed'='true' and
             (id in (select id from account_reset_batches where document->>'confirmed'='true' order by created_at desc limit 30)
              or document->'items' @> '[{\"status\":\"queued\"}]'
              or document->'items' @> '[{\"status\":\"running\"}]'
              or document->'items' @> '[{\"status\":\"unknown\"}]')
             order by created_at desc",
        )
        .fetch_all(&self.0)
        .await
        .map_err(unavailable)?;
        values.into_iter().map(decode).collect()
    }

    async fn history(&self, query: ResetHistoryQuery) -> AdminStoreResult<ResetHistoryPage> {
        query.validate().map_err(unavailable)?;
        let before = query.before.unwrap_or_else(Utc::now);
        let values: Vec<Value> = sqlx::query_scalar(
            "select b.document from account_reset_batches b
             where b.document->>'confirmed'='true' and b.created_at <= $1
             and ($2='' or exists (
               select 1 from jsonb_array_elements(b.document->'items') i
               left join provider_accounts a on a.id=i->>'accountId'
               where strpos(lower(concat_ws(' ',i->>'accountId',a.name,a.custom_name,a.email)),lower($2))>0))
             order by b.created_at desc,b.id desc limit 11 offset $3"
        ).bind(before).bind(query.search.trim()).bind(i64::from(query.page - 1) * 10)
            .fetch_all(&self.0).await.map_err(unavailable)?;
        let has_more = values.len() > 10;
        let items: Vec<ResetBatch> = values
            .into_iter()
            .take(10)
            .map(decode)
            .collect::<AdminStoreResult<_>>()?;
        let ids: Vec<_> = items
            .iter()
            .flat_map(|batch| batch.items.iter().map(|item| item.account_id.as_str()))
            .collect();
        let names: Vec<(String, String)> = sqlx::query_as(
            "select id,coalesce(nullif(custom_name,''),nullif(email,''),name,id)
             from provider_accounts where id=any($1::text[])",
        )
        .bind(&ids)
        .fetch_all(&self.0)
        .await
        .map_err(unavailable)?;
        Ok(ResetHistoryPage {
            items,
            account_names: names.into_iter().collect(),
            before,
            has_more,
        })
    }

    async fn confirm(&self, id: Uuid, context: &MutationContext) -> AdminStoreResult<ResetBatch> {
        let mut tx = self.locked().await?;
        let mut batch = Self::read_batch(&mut tx, id).await?;
        if !batch.confirmed {
            if Utc::now() - batch.created_at > Duration::minutes(5) {
                return Err(conflict("预览已过期，请重新查询后确认"));
            }
            batch.confirmed = true;
            batch.context = Some(context.clone());
            for item in &mut batch.items {
                if item.status == ResetItemStatus::Ready {
                    item.status = ResetItemStatus::Queued;
                    item.updated_at = Utc::now();
                }
            }
            Self::write_batch(&mut tx, &batch).await?;
            Self::audit(&mut tx, context, "reset_credits.confirm", &id.to_string()).await?;
        }
        tx.commit().await.map_err(unavailable)?;
        Ok(batch)
    }

    async fn retry(
        &self,
        id: Uuid,
        account_id: &str,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        let mut tx = self.locked().await?;
        let mut batch = Self::read_batch(&mut tx, id).await?;
        let item = batch
            .items
            .iter_mut()
            .find(|item| item.account_id == account_id)
            .ok_or_else(|| conflict("任务中没有该账号"))?;
        if item.status != ResetItemStatus::Unknown {
            return Err(conflict("只能继续确认结果未知的原操作"));
        }
        if Utc::now() - item.updated_at < Duration::seconds(180) {
            return Err(conflict("原请求仍在保护期，请稍后继续确认"));
        }
        item.retry = true;
        item.status = ResetItemStatus::Queued;
        batch.context = Some(context.clone());
        Self::write_batch(&mut tx, &batch).await?;
        Self::audit(
            &mut tx,
            context,
            "reset_credits.retry_original",
            &id.to_string(),
        )
        .await?;
        tx.commit().await.map_err(unavailable)
    }

    async fn claim(&self) -> AdminStoreResult<Option<(ResetBatch, ResetBatchItem)>> {
        let mut tx = self.locked().await?;
        let ids: Vec<Uuid> = sqlx::query_scalar(
            "select id from account_reset_batches where document->>'confirmed'='true'
             and (document->'items' @> '[{\"status\":\"queued\"}]'
               or document->'items' @> '[{\"status\":\"running\"}]') order by created_at",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        let mut active = Vec::new();
        let mut running = 0;
        for id in ids {
            let mut batch = Self::read_batch(&mut tx, id).await?;
            let mut changed = false;
            for item in &mut batch.items {
                if item.status == ResetItemStatus::Running {
                    if Utc::now() - item.updated_at > Duration::seconds(180) {
                        item.status = ResetItemStatus::Unknown;
                        item.message = "执行中断，结果待确认；不会自动换卡重试".into();
                        changed = true;
                    } else {
                        running += 1;
                    }
                }
            }
            if changed {
                Self::write_batch(&mut tx, &batch).await?;
            }
            active.push(batch);
        }
        if running < 3 {
            for mut batch in active {
                if let Some(item) = batch
                    .items
                    .iter_mut()
                    .find(|i| i.status == ResetItemStatus::Queued)
                {
                    let identities: Value = sqlx::query_scalar(
                        "select identities from account_reset_batches where id=$1",
                    )
                    .bind(batch.id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(unavailable)?;
                    if Self::identity(&mut tx, &item.account_id)
                        .await
                        .ok()
                        .as_ref()
                        != identities.get(&item.account_id)
                    {
                        item.status = ResetItemStatus::Skipped;
                        item.message = "账号已删除或身份变化，本次未消费".into();
                        Self::write_batch(&mut tx, &batch).await?;
                        continue;
                    }
                    if Self::validate_auto_send(&mut tx, item.redeem_request_id, item.retry)
                        .await
                        .is_err()
                    {
                        item.status = ResetItemStatus::Skipped;
                        item.message = "自动重置授权或观测已变化，本次未消费".into();
                        Self::write_batch(&mut tx, &batch).await?;
                        continue;
                    }
                    item.status = ResetItemStatus::Running;
                    item.claim_id = Some(Uuid::new_v4());
                    item.updated_at = Utc::now();
                    let item = item.clone();
                    Self::write_batch(&mut tx, &batch).await?;
                    tx.commit().await.map_err(unavailable)?;
                    return Ok(Some((batch, item)));
                }
            }
        }
        tx.commit().await.map_err(unavailable)?;
        Ok(None)
    }

    async fn finish_item(&self, id: Uuid, mut item: ResetBatchItem) -> AdminStoreResult<()> {
        let mut tx = self.locked().await?;
        let mut batch = Self::read_batch(&mut tx, id).await?;
        if let Some(current) = batch.items.iter_mut().find(|i| {
            i.account_id == item.account_id
                && i.claim_id == item.claim_id
                && i.status == ResetItemStatus::Running
        }) {
            item.updated_at = Utc::now();
            sqlx::query("update account_auto_reset_policies p set message=$2 from account_auto_reset_jobs j
                where j.request_id=$1 and p.account_id=j.account_id and p.revision=j.policy_revision")
                .bind(item.redeem_request_id).bind(&item.message).execute(&mut *tx).await.map_err(unavailable)?;
            *current = item;
            Self::write_batch(&mut tx, &batch).await?;
        }
        tx.commit().await.map_err(unavailable)
    }

    async fn begin_consume(
        &self,
        command: &ConsumeProviderResetCredit,
        context: &MutationContext,
    ) -> AdminStoreResult<ResetConsumePermit> {
        let mut tx = self.locked().await?;
        let identity = Self::identity(&mut tx, command.account_id.as_str()).await?;
        Self::validate_auto_send(&mut tx, command.redeem_request_id, true).await?;
        let planned: Option<Value> = sqlx::query_scalar(
            "select identities->($2::text) from account_reset_batches
             where document->'items' @> jsonb_build_array(jsonb_build_object('redeemRequestId',$1::text))",
        ).bind(command.redeem_request_id.to_string()).bind(command.account_id.as_str())
            .fetch_optional(&mut *tx).await.map_err(unavailable)?;
        if planned.is_some_and(|planned| planned != identity) {
            return Err(conflict("账号身份与已确认的重置任务不一致"));
        }
        let row: Option<(String, Option<String>, String, DateTime<Utc>, Option<Value>, Value)> = sqlx::query_as(
            "select account_id,credit_id,status,updated_at,result,identity from account_reset_consumptions where request_id=$1",
        ).bind(command.redeem_request_id).fetch_optional(&mut *tx).await.map_err(unavailable)?;
        if let Some((account, credit, status, updated_at, result, saved_identity)) = row {
            if account != command.account_id.as_str()
                || credit != command.credit_id
                || saved_identity != identity
            {
                return Err(conflict("请求标识已绑定其他账号或卡片"));
            }
            if status == "completed" {
                return Ok(ResetConsumePermit::Completed(decode(
                    result.ok_or_else(|| unavailable("missing result"))?,
                )?));
            }
            if Utc::now() - updated_at < Duration::seconds(180) {
                return Err(conflict("原重置请求仍在保护期，请稍后继续确认"));
            }
        } else {
            let blocked: bool = sqlx::query_scalar(
                "select exists(select 1 from account_reset_consumptions where account_id=$1 and
                 (status in ('running','unknown') or (credit_id=$2 and result->>'code' in ('reset','already_redeemed'))))",
            ).bind(command.account_id.as_str()).bind(&command.credit_id)
                .fetch_one(&mut *tx).await.map_err(unavailable)?;
            if blocked {
                return Err(conflict("该账号有待确认操作或该卡已消费，不能另起重置"));
            }
        }
        let claim = Uuid::new_v4();
        sqlx::query("insert into account_reset_consumptions(request_id,account_id,credit_id,status,claim_id,identity)
            values($1,$2,$3,'running',$4,$5) on conflict(request_id) do update set status='running',claim_id=$4,updated_at=now()")
            .bind(command.redeem_request_id).bind(command.account_id.as_str()).bind(&command.credit_id).bind(claim).bind(identity)
            .execute(&mut *tx).await.map_err(unavailable)?;
        // Queuing is reversible; fence the window only when the durable consume permit is issued.
        sqlx::query(
            "update account_auto_reset_policies p set last_trigger=j.observation->'triggered'
            from account_auto_reset_jobs j where j.request_id=$1 and p.account_id=j.account_id
            and p.revision=j.policy_revision",
        )
        .bind(command.redeem_request_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        Self::audit(
            &mut tx,
            context,
            "reset_credits.consume",
            command.account_id.as_str(),
        )
        .await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(ResetConsumePermit::Execute(claim))
    }

    async fn finish_consume(
        &self,
        command: &ConsumeProviderResetCredit,
        claim: Uuid,
        result: Option<&ProviderResetCreditResult>,
    ) -> AdminStoreResult<()> {
        sqlx::query(
            "update account_reset_consumptions set status=$3,result=$4,updated_at=now()
            where request_id=$1 and claim_id=$2 and status='running'",
        )
        .bind(command.redeem_request_id)
        .bind(claim)
        .bind(if result.is_some() {
            "completed"
        } else {
            "unknown"
        })
        .bind(result.map(encode).transpose()?)
        .execute(&self.0)
        .await
        .map_err(unavailable)?;
        Ok(())
    }
}
