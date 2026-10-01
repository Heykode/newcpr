use super::TestDatabase;
use chrono::{Duration, Utc};
use gateway_admin::{
    model::{
        MutationActor, MutationContext,
        provider_credentials::{
            ConsumeProviderResetCredit, ProviderResetCredit, ProviderResetCreditResult,
        },
        reset_credits::*,
    },
    ports::reset_credits::ResetCreditsStore,
};
use gateway_core::account::ProviderAccountId;
use gateway_store::postgres::PgResetCreditsStore;
use uuid::Uuid;

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "reset-fixture".into(),
    }
}
async fn seed(db: &TestDatabase) {
    sqlx::query("insert into provider_accounts (
        id,provider_kind,name,upstream_user_id,authentication_kind,provider_credentials_json,
        has_refresh_token,credential_observed_at,created_at,updated_at)
        select 'acct_reset_'||n,'openai','Fixture','fixture-user-'||n,'oauth','{}',false,now(),now(),now()
        from generate_series(1,5) n").execute(&db.pool).await.unwrap();
}
fn command(card: &str) -> ConsumeProviderResetCredit {
    ConsumeProviderResetCredit {
        account_id: ProviderAccountId::new("acct_reset_1").unwrap(),
        credit_id: Some(card.into()),
        redeem_request_id: Uuid::new_v4(),
    }
}
fn batch(count: usize) -> ResetBatch {
    ResetBatch {
        id: Uuid::new_v4(),
        created_at: Utc::now(),
        confirmed: false,
        reset_type: None,
        context: None,
        items: (1..=count)
            .map(|n| ResetBatchItem {
                account_id: format!("acct_reset_{n}"),
                available_count: Some(2),
                credit: Some(ProviderResetCredit {
                    id: format!("card-{n}"),
                    status: Some("available".into()),
                    title: None,
                    expires_at: Some(Utc::now() + Duration::hours(1)),
                    reset_type: Some("codex".into()),
                }),
                redeem_request_id: Uuid::new_v4(),
                status: ResetItemStatus::Ready,
                message: String::new(),
                updated_at: Utc::now(),
                claim_id: None,
                retry: false,
            })
            .collect(),
    }
}

#[tokio::test]
async fn reset_credits_durable_fence_replays_result_and_never_uses_next_card() {
    let Some(db) = TestDatabase::create("reset_fence").await else {
        return;
    };
    seed(&db).await;
    let a = PgResetCreditsStore(db.pool.clone());
    let b = PgResetCreditsStore(db.pool.clone());
    let cmd = command("soon");
    let ctx = context();
    let (first, duplicate) = tokio::join!(a.begin_consume(&cmd, &ctx), b.begin_consume(&cmd, &ctx));
    assert_eq!(
        usize::from(first.is_ok()) + usize::from(duplicate.is_ok()),
        1
    );
    let permit = first.or(duplicate).unwrap();
    let ResetConsumePermit::Execute(claim) = permit else {
        panic!("must acquire once")
    };
    assert!(a.begin_consume(&command("later"), &ctx).await.is_err());
    a.finish_consume(&cmd, claim, None).await.unwrap();
    let inventory = b.inventories(&["acct_reset_1".into()]).await.unwrap();
    assert_eq!(
        inventory[0].pending.as_ref().unwrap().redeem_request_id,
        cmd.redeem_request_id
    );
    assert!(
        b.begin_consume(&cmd, &ctx).await.is_err(),
        "protect still-running upstream attempt"
    );
    sqlx::query("update account_reset_consumptions set updated_at=now()-interval '181 seconds'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        b.begin_consume(&command("later"), &ctx).await.is_err(),
        "unknown result blocks new ID after restart too"
    );
    let ResetConsumePermit::Execute(retry_claim) = b.begin_consume(&cmd, &ctx).await.unwrap()
    else {
        panic!("original retry")
    };
    let success = ProviderResetCreditResult {
        code: "reset".into(),
        credit: None,
    };
    a.finish_consume(&cmd, claim, Some(&success)).await.unwrap();
    assert!(
        b.begin_consume(&cmd, &ctx).await.is_err(),
        "stale claimant cannot finish a new attempt"
    );
    b.finish_consume(&cmd, retry_claim, Some(&success))
        .await
        .unwrap();
    assert!(matches!(
        a.begin_consume(&cmd, &ctx).await.unwrap(),
        ResetConsumePermit::Completed(_)
    ));
    assert!(
        a.begin_consume(&command("soon"), &ctx).await.is_err(),
        "same card cannot be consumed with a new ID"
    );
    let mut changed = cmd.clone();
    changed.credit_id = Some("later".into());
    assert!(a.begin_consume(&changed, &ctx).await.is_err());
    db.close().await;
}

#[tokio::test]
async fn reset_credits_confirmation_is_idempotent_and_global_worker_cap_is_three() {
    let Some(db) = TestDatabase::create("reset_batch").await else {
        return;
    };
    seed(&db).await;
    let a = PgResetCreditsStore(db.pool.clone());
    let b = PgResetCreditsStore(db.pool.clone());
    let preview = a.save_preview(batch(5)).await.unwrap();
    assert!(a.claim().await.unwrap().is_none(), "preview cannot consume");
    let confirmed = a.confirm(preview.id, &context()).await.unwrap();
    assert_eq!(confirmed.items.len(), 5);
    assert_eq!(
        a.confirm(preview.id, &context()).await.unwrap().items[0].redeem_request_id,
        confirmed.items[0].redeem_request_id
    );
    let first = a.claim().await.unwrap().unwrap();
    assert!(b.claim().await.unwrap().is_some());
    assert!(a.claim().await.unwrap().is_some());
    assert!(b.claim().await.unwrap().is_none());
    let mut item = first.1;
    item.status = ResetItemStatus::Succeeded;
    a.finish_item(first.0.id, item).await.unwrap();
    assert!(b.claim().await.unwrap().is_some());
    assert_eq!(a.batches().await.unwrap().len(), 1);
    let result = a.confirm(preview.id, &context()).await.unwrap();
    assert_eq!(
        result.items[0].status,
        ResetItemStatus::Succeeded,
        "duplicate confirmation cannot reset progress"
    );
    db.close().await;
}

#[tokio::test]
async fn reset_credits_expired_claims_become_unknown_not_automatically_replayed() {
    let Some(db) = TestDatabase::create("reset_crash").await else {
        return;
    };
    seed(&db).await;
    let store = PgResetCreditsStore(db.pool.clone());
    let preview = store.save_preview(batch(1)).await.unwrap();
    store.confirm(preview.id, &context()).await.unwrap();
    let (_, mut item) = store.claim().await.unwrap().unwrap();
    item.updated_at = Utc::now() - Duration::seconds(181);
    let mut persisted = store.batches().await.unwrap().remove(0);
    persisted.items[0] = item.clone();
    sqlx::query("update account_reset_batches set document=$2 where id=$1")
        .bind(preview.id)
        .bind(serde_json::to_value(persisted).unwrap())
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(store.claim().await.unwrap().is_none());
    assert_eq!(
        store.batches().await.unwrap()[0].items[0].status,
        ResetItemStatus::Unknown
    );
    store
        .retry(preview.id, "acct_reset_1", &context())
        .await
        .unwrap();
    let (_, retried) = store.claim().await.unwrap().unwrap();
    assert_eq!(retried.redeem_request_id, item.redeem_request_id);
    assert_eq!(retried.credit, item.credit);
    assert!(retried.retry);
    db.close().await;
}

#[tokio::test]
async fn reset_credits_preview_and_original_retry_are_fenced_against_identity_changes() {
    let Some(db) = TestDatabase::create("reset_identity").await else {
        return;
    };
    seed(&db).await;
    let store = PgResetCreditsStore(db.pool.clone());
    let preview = store.save_preview(batch(1)).await.unwrap();
    let cmd = command("soon");
    let ResetConsumePermit::Execute(claim) = store.begin_consume(&cmd, &context()).await.unwrap()
    else {
        panic!()
    };
    store.finish_consume(&cmd, claim, None).await.unwrap();
    sqlx::query(
        "update provider_accounts set upstream_user_id='different-fixture' where id='acct_reset_1'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("update account_reset_consumptions set updated_at=now()-interval '181 seconds'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(store.begin_consume(&cmd, &context()).await.is_err());
    store.confirm(preview.id, &context()).await.unwrap();
    assert!(store.claim().await.unwrap().is_none());
    assert_eq!(
        store.batches().await.unwrap()[0].items[0].status,
        ResetItemStatus::Skipped
    );
    db.close().await;
}
