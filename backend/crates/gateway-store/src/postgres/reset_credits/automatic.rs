use super::*;
use gateway_admin::model::{AdminError, MutationActor};

#[derive(sqlx::FromRow)]
struct AutoPolicyRow {
    revision: i64,
    config: Value,
    identity: Value,
    checked_at: Option<DateTime<Utc>>,
    message: Option<String>,
}

impl PgResetCreditsStore {
    async fn auto_eligible(tx: &mut Transaction<'_, Postgres>, id: &str) -> AdminStoreResult<bool> {
        sqlx::query_scalar(
            "select exists(select 1 from provider_accounts where id=$1 and provider_kind='openai'
            and authentication_kind='oauth' and responses_upstream='codex')",
        )
        .bind(id)
        .fetch_one(&mut **tx)
        .await
        .map_err(unavailable)
    }

    async fn read_auto(
        tx: &mut Transaction<'_, Postgres>,
        id: &str,
    ) -> AdminStoreResult<AutoResetPolicy> {
        let identity = Self::identity(tx, id).await?;
        let row: Option<AutoPolicyRow> = sqlx::query_as(
            "select revision,config,identity,checked_at,message from account_auto_reset_policies where account_id=$1")
            .bind(id).fetch_optional(&mut **tx).await.map_err(unavailable)?;
        let Some(row) = row else {
            return Ok(AutoResetPolicy {
                account_id: id.to_owned(),
                revision: 0,
                config: AutoResetConfig::default(),
                checked_at: None,
                message: None,
            });
        };
        let mut config: AutoResetConfig = decode(row.config)?;
        let mut message = row.message;
        if identity != row.identity {
            config.enabled = false;
            message = Some("账号身份已变化，自动重置未授权".into());
        }
        Ok(AutoResetPolicy {
            account_id: id.to_owned(),
            revision: row.revision,
            config,
            checked_at: row.checked_at,
            message,
        })
    }

    pub(super) async fn auto_policy_inner(&self, id: &str) -> AdminStoreResult<AutoResetPolicy> {
        let mut tx = self.locked().await?;
        let policy = Self::read_auto(&mut tx, id).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(policy)
    }

    pub(super) async fn save_auto_inner(
        &self,
        id: &str,
        revision: i64,
        config: AutoResetConfig,
        context: &MutationContext,
    ) -> AdminStoreResult<AutoResetPolicy> {
        config
            .validate()
            .map_err(|e: AdminError| conflict(e.message()))?;
        let mut tx = self.locked().await?;
        if config.enabled && !Self::auto_eligible(&mut tx, id).await? {
            return Err(conflict("自动重置仅适用于原生 Codex OAuth 账号"));
        }
        let current = Self::read_auto(&mut tx, id).await?;
        if current.revision != revision || revision >= 9_007_199_254_740_990 {
            return Err(conflict("自动重置设置已变化，请刷新后保存"));
        }
        let identity = Self::identity(&mut tx, id).await?;
        sqlx::query("insert into account_auto_reset_policies(account_id,revision,config,identity) values($1,$2,$3,$4)
            on conflict(account_id) do update set revision=$2,config=$3,identity=$4,next_check_at=now(),check_claim=null,check_until=null")
            .bind(id).bind(revision+1).bind(encode(&config)?).bind(identity).execute(&mut *tx).await.map_err(unavailable)?;
        Self::audit(&mut tx, context, "reset_credits.auto_policy", id).await?;
        let result = Self::read_auto(&mut tx, id).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(result)
    }

    pub(super) async fn claim_auto_inner(&self) -> AdminStoreResult<Option<AutoResetCheck>> {
        let mut tx = self.locked().await?;
        let active: i64 = sqlx::query_scalar(
            "select count(*) from account_auto_reset_policies where check_until>now()",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        if active >= 3 {
            return Ok(None);
        }
        let id: Option<String> = sqlx::query_scalar("select p.account_id from account_auto_reset_policies p
            join provider_accounts a on a.id=p.account_id
            where p.config->>'enabled'='true' and p.next_check_at<=now() and (p.check_until is null or p.check_until<=now())
            and (p.config->>'fiveHourUsedMillis'<>'0' or p.config->>'sevenDayUsedMillis'<>'0')
            and a.provider_kind='openai' and a.authentication_kind='oauth' and a.responses_upstream='codex'
            and p.identity=jsonb_build_array(a.provider_kind,a.upstream_user_id,a.upstream_account_id,extract(epoch from a.created_at))
            and not exists(select 1 from account_reset_consumptions c where c.account_id=p.account_id and c.status in ('running','unknown'))
            and not exists(select 1 from account_reset_batches b, jsonb_array_elements(b.document->'items') item
                where item->>'accountId'=p.account_id and item->>'status' in ('queued','running','unknown'))
            order by p.next_check_at,p.account_id limit 1")
            .fetch_optional(&mut *tx).await.map_err(unavailable)?;
        let Some(id) = id else {
            return Ok(None);
        };
        let claim_id = Uuid::new_v4();
        let started_at: DateTime<Utc> = sqlx::query_scalar("update account_auto_reset_policies set check_claim=$2,
            check_until=now()+interval '120 seconds',next_check_at=now()+interval '60 seconds' where account_id=$1 returning now()")
            .bind(&id).bind(claim_id).fetch_one(&mut *tx).await.map_err(unavailable)?;
        let policy = Self::read_auto(&mut tx, &id).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(Some(AutoResetCheck {
            policy,
            claim_id,
            started_at,
        }))
    }

    pub(super) async fn finish_auto_inner(
        &self,
        check: &AutoResetCheck,
        observation: Option<AutoResetObservation>,
        credit: Option<gateway_admin::model::provider_credentials::ProviderResetCredit>,
        message: &str,
    ) -> AdminStoreResult<()> {
        let mut tx = self.locked().await?;
        let id = &check.policy.account_id;
        let row: Option<(Value, Value)> = sqlx::query_as("select identity,last_trigger from account_auto_reset_policies
            where account_id=$1 and revision=$2 and check_claim=$3 and check_until>now() and config->>'enabled'='true'")
            .bind(id).bind(check.policy.revision).bind(check.claim_id).fetch_optional(&mut *tx).await.map_err(unavailable)?;
        let Some((identity, last_trigger)) = row else {
            return Ok(());
        };
        if Self::identity(&mut tx, id).await? != identity
            || !Self::auto_eligible(&mut tx, id).await?
        {
            return Ok(());
        }
        let mut previous: Vec<AutoResetWindow> = decode(last_trigger)?;
        let mut message = message.to_owned();
        if let Some(observation) = observation {
            let blocked: bool = sqlx::query_scalar(
                "select exists(select 1 from account_reset_consumptions where account_id=$1
                and (status in ('running','unknown') or updated_at >= $2))",
            )
            .bind(id)
            .bind(observation.started_at)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
            let fresh = observation.started_at >= check.started_at
                && observation.observed_at >= observation.started_at
                && observation.observed_at <= Utc::now()
                && Utc::now() - observation.observed_at <= Duration::seconds(90);
            if blocked || !fresh {
                message = "额度观测已失效或有其他重置操作，本次未消费".into();
            } else if observation.all_below {
                previous.clear();
            } else if observation.overlaps(&previous) {
                message = "等待已重置窗口的新用量，未重复消费".into();
            } else if !observation.triggered.is_empty()
                && let Some(credit) = credit
            {
                let now = Utc::now();
                let item = ResetBatchItem {
                    account_id: id.clone(),
                    available_count: None,
                    credit: Some(credit),
                    redeem_request_id: Uuid::new_v4(),
                    status: ResetItemStatus::Queued,
                    message: "自动重置已排队".into(),
                    updated_at: now,
                    claim_id: None,
                    retry: false,
                };
                let context = MutationContext {
                    actor: MutationActor::System,
                    request_id: format!("auto-reset-{}", item.redeem_request_id),
                };
                let batch = ResetBatch {
                    id: Uuid::new_v4(),
                    created_at: now,
                    confirmed: true,
                    reset_type: item.credit.as_ref().and_then(|c| c.reset_type.clone()),
                    items: vec![item.clone()],
                    context: Some(context.clone()),
                };
                sqlx::query("insert into account_reset_batches(id,created_at,document,actor,identities) values($1,$2,$3,$4,$5)")
                    .bind(batch.id).bind(now).bind(encode(&batch)?).bind(encode(&context)?)
                    .bind(serde_json::json!({ id: identity })).execute(&mut *tx).await.map_err(unavailable)?;
                sqlx::query("insert into account_auto_reset_jobs(request_id,batch_id,account_id,policy_revision,observation) values($1,$2,$3,$4,$5)")
                    .bind(item.redeem_request_id).bind(batch.id).bind(id).bind(check.policy.revision).bind(encode(&observation)?)
                    .execute(&mut *tx).await.map_err(unavailable)?;
                Self::audit(&mut tx, &context, "reset_credits.auto_queued", id).await?;
                message = "自动重置已排队".into();
            }
        }
        sqlx::query("update account_auto_reset_policies set check_claim=null,check_until=null,checked_at=now(),
            next_check_at=now()+interval '60 seconds',message=$2,last_trigger=$3 where account_id=$1")
            .bind(id).bind(message).bind(encode(&previous)?).execute(&mut *tx).await.map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)
    }

    // Rechecked under the same short ledger lock as manual admission, immediately before sending.
    pub(super) async fn validate_auto_send(
        tx: &mut Transaction<'_, Postgres>,
        request: Uuid,
        retry: bool,
    ) -> AdminStoreResult<()> {
        let job: Option<(String, i64, Value)> = sqlx::query_as("select account_id,policy_revision,observation from account_auto_reset_jobs where request_id=$1")
            .bind(request).fetch_optional(&mut **tx).await.map_err(unavailable)?;
        let Some((id, revision, observation)) = job else {
            return Ok(());
        };
        // An explicit continuation resolves the original irreversible operation, even after disabling automation.
        if retry {
            let exists: bool = sqlx::query_scalar(
                "select exists(select 1 from account_reset_consumptions where request_id=$1)",
            )
            .bind(request)
            .fetch_one(&mut **tx)
            .await
            .map_err(unavailable)?;
            if exists {
                return Ok(());
            }
        }
        let observation: AutoResetObservation = decode(observation)?;
        let policy = Self::read_auto(tx, &id).await?;
        if !policy.config.enabled
            || policy.revision != revision
            || !Self::auto_eligible(tx, &id).await?
            || Utc::now() - observation.observed_at > Duration::seconds(90)
            || observation
                .triggered
                .iter()
                .any(|w| w.reset_at <= Utc::now())
        {
            return Err(conflict("自动重置授权或额度观测已失效，本次未消费"));
        }
        let changed: bool = sqlx::query_scalar(
            "select exists(select 1 from account_reset_consumptions where account_id=$1
            and request_id<>$2 and (status in ('running','unknown') or updated_at >= $3))",
        )
        .bind(&id)
        .bind(request)
        .bind(observation.started_at)
        .fetch_one(&mut **tx)
        .await
        .map_err(unavailable)?;
        if changed {
            return Err(conflict("观测后已有其他重置操作，本次未消费"));
        }
        Ok(())
    }
}
