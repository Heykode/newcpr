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

#[tokio::test]
async fn reset_history_pages_all_confirmed_batches_and_searches_literal_account_fields() {
    let Some(db) = TestDatabase::create("reset_history").await else {
        return;
    };
    seed(&db).await;
    let store = PgResetCreditsStore(db.pool.clone());
    sqlx::query("update provider_accounts set custom_name='100%_fixture' where id='acct_reset_1'")
        .execute(&db.pool)
        .await
        .unwrap();
    for n in 0..35 {
        let mut value = batch(1);
        value.confirmed = true;
        value.created_at = Utc::now() - Duration::seconds(40 - n);
        value.items[0].status = ResetItemStatus::Succeeded;
        store.save_preview(value).await.unwrap();
    }
    store.save_preview(batch(1)).await.unwrap();
    let first = store
        .history(ResetHistoryQuery {
            page: 1,
            search: "%_".into(),
            before: None,
        })
        .await
        .unwrap();
    assert_eq!(first.items.len(), 10);
    assert!(first.has_more);
    assert_eq!(first.account_names["acct_reset_1"], "100%_fixture");
    let mut added = batch(1);
    added.confirmed = true;
    store.save_preview(added).await.unwrap();
    let mut ids: std::collections::BTreeSet<_> = first.items.iter().map(|batch| batch.id).collect();
    for page in 2..=4 {
        let next = store
            .history(ResetHistoryQuery {
                page,
                search: "".into(),
                before: Some(first.before),
            })
            .await
            .unwrap();
        assert_eq!(next.has_more, page < 4);
        for batch in next.items {
            assert!(ids.insert(batch.id));
        }
    }
    assert_eq!(ids.len(), 35);
    assert!(
        store
            .history(ResetHistoryQuery {
                page: 1,
                search: "missing".into(),
                before: None
            })
            .await
            .unwrap()
            .items
            .is_empty()
    );
    sqlx::query("delete from provider_accounts where id='acct_reset_1'")
        .execute(&db.pool)
        .await
        .unwrap();
    let deleted = store
        .history(ResetHistoryQuery {
            page: 1,
            search: "acct_reset_1".into(),
            before: None,
        })
        .await
        .unwrap();
    assert_eq!(deleted.items.len(), 10);
    assert!(deleted.account_names.is_empty());
    db.close().await;
}

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

async fn auto_setup(db: &TestDatabase) -> PgResetCreditsStore {
    seed(db).await;
    let store = PgResetCreditsStore(db.pool.clone());
    let default = store.auto_policy("acct_reset_1").await.unwrap();
    assert!(!default.config.enabled);
    assert_eq!(default.revision, 0);
    assert!(store.claim_auto_check().await.unwrap().is_none());
    let policy = store
        .save_auto_policy(
            "acct_reset_1",
            0,
            AutoResetConfig {
                enabled: true,
                ..Default::default()
            },
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(policy.revision, 1);
    assert!(
        store
            .save_auto_policy("acct_reset_1", 0, policy.config, &context())
            .await
            .is_err()
    );
    store
}

fn evidence(check: &AutoResetCheck) -> AutoResetObservation {
    AutoResetObservation {
        started_at: Utc::now().max(check.started_at),
        observed_at: Utc::now().max(check.started_at),
        triggered: vec![AutoResetWindow {
            seconds: 18_000,
            reset_at: Utc::now() + Duration::hours(1),
        }],
        all_below: false,
    }
}
async fn enqueue_auto(
    store: &PgResetCreditsStore,
    check: &AutoResetCheck,
    observation: AutoResetObservation,
) {
    store
        .finish_auto_check(
            check,
            Some(observation),
            batch(1).items[0].credit.clone(),
            "fixture",
        )
        .await
        .unwrap();
}
async fn due(db: &TestDatabase) {
    sqlx::query("update account_auto_reset_policies set next_check_at=now()-interval '1 second'")
        .execute(&db.pool)
        .await
        .unwrap();
}
fn automatic_command(item: &ResetBatchItem) -> ConsumeProviderResetCredit {
    ConsumeProviderResetCredit {
        account_id: ProviderAccountId::new(item.account_id.clone()).unwrap(),
        credit_id: item.credit.as_ref().map(|credit| credit.id.clone()),
        redeem_request_id: item.redeem_request_id,
    }
}

#[tokio::test]
async fn automatic_policy_and_check_leases_are_durable_and_identity_bound() {
    let Some(db) = TestDatabase::create("auto_policy").await else {
        return;
    };
    let a = auto_setup(&db).await;
    let b = PgResetCreditsStore(db.pool.clone());
    let (first, second) = tokio::join!(a.claim_auto_check(), b.claim_auto_check());
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(
        usize::from(first.is_some()) + usize::from(second.is_some()),
        1
    );
    let check = first.or(second).unwrap();
    sqlx::query(
        "update provider_accounts set credential_observed_at=now(),updated_at=now() where id='acct_reset_1'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(
        a.auto_policy("acct_reset_1").await.unwrap().config.enabled,
        "token rotation preserves authorization"
    );
    sqlx::query(
        "update provider_accounts set upstream_user_id='new-identity' where id='acct_reset_1'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(!b.auto_policy("acct_reset_1").await.unwrap().config.enabled);
    enqueue_auto(&a, &check, evidence(&check)).await;
    assert!(a.batches().await.unwrap().is_empty());
    due(&db).await;
    assert!(a.claim_auto_check().await.unwrap().is_none());
    sqlx::query("update provider_accounts set responses_upstream='excel' where id='acct_reset_1'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        a.save_auto_policy(
            "acct_reset_1",
            1,
            AutoResetConfig {
                enabled: true,
                ..Default::default()
            },
            &context()
        )
        .await
        .is_err()
    );
    db.close().await;
}

#[tokio::test]
async fn automatic_and_manual_consumption_share_admission_and_stale_observation_fences() {
    let Some(db) = TestDatabase::create("auto_manual").await else {
        return;
    };
    let store = auto_setup(&db).await;
    let check = store.claim_auto_check().await.unwrap().unwrap();
    let observation = evidence(&check);
    enqueue_auto(&store, &check, observation.clone()).await;
    let (batch, item) = store.claim().await.unwrap().unwrap();
    assert!(matches!(
        batch.context.unwrap().actor,
        MutationActor::System
    ));
    assert!(
        store
            .auto_execution(item.redeem_request_id)
            .await
            .unwrap()
            .is_some()
    );
    let manual = command("manual-card");
    let ResetConsumePermit::Execute(claim) =
        store.begin_consume(&manual, &context()).await.unwrap()
    else {
        panic!()
    };
    let success = ProviderResetCreditResult {
        code: "reset".into(),
        credit: None,
    };
    store
        .finish_consume(&manual, claim, Some(&success))
        .await
        .unwrap();
    assert!(
        store
            .begin_consume(&automatic_command(&item), &context())
            .await
            .is_err(),
        "a completed manual reset invalidates earlier automatic evidence"
    );
    assert!(store.auto_execution(item.redeem_request_id).await.is_err());
    db.close().await;
}

#[tokio::test]
async fn automatic_unknown_result_blocks_new_card_and_only_explicit_original_can_continue() {
    let Some(db) = TestDatabase::create("auto_unknown").await else {
        return;
    };
    let store = auto_setup(&db).await;
    let check = store.claim_auto_check().await.unwrap().unwrap();
    enqueue_auto(&store, &check, evidence(&check)).await;
    let (batch, mut item) = store.claim().await.unwrap().unwrap();
    let cmd = automatic_command(&item);
    let ResetConsumePermit::Execute(claim) = store.begin_consume(&cmd, &context()).await.unwrap()
    else {
        panic!()
    };
    store.finish_consume(&cmd, claim, None).await.unwrap();
    item.status = ResetItemStatus::Unknown;
    store.finish_item(batch.id, item.clone()).await.unwrap();
    due(&db).await;
    assert!(store.claim_auto_check().await.unwrap().is_none());
    assert!(
        store
            .begin_consume(&command("another-card"), &context())
            .await
            .is_err()
    );
    store
        .save_auto_policy("acct_reset_1", 1, AutoResetConfig::default(), &context())
        .await
        .unwrap();
    sqlx::query("update account_reset_consumptions set updated_at=now()-interval '181 seconds'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        matches!(
            store.begin_consume(&cmd, &context()).await.unwrap(),
            ResetConsumePermit::Execute(_)
        ),
        "manual resolution retains original request even after disabling automation"
    );
    db.close().await;
}

#[tokio::test]
async fn automatic_disable_invalidates_queued_and_already_claimed_unsent_work() {
    for claimed in [false, true] {
        let Some(db) = TestDatabase::create("auto_disable").await else {
            return;
        };
        let store = auto_setup(&db).await;
        let check = store.claim_auto_check().await.unwrap().unwrap();
        enqueue_auto(&store, &check, evidence(&check)).await;
        let item = if claimed {
            Some(store.claim().await.unwrap().unwrap().1)
        } else {
            None
        };
        store
            .save_auto_policy("acct_reset_1", 1, AutoResetConfig::default(), &context())
            .await
            .unwrap();
        if let Some(item) = item {
            assert!(
                store
                    .begin_consume(&automatic_command(&item), &context())
                    .await
                    .is_err()
            );
        } else {
            assert!(store.claim().await.unwrap().is_none());
            assert_eq!(
                store.batches().await.unwrap()[0].items[0].status,
                ResetItemStatus::Skipped
            );
        }
        db.close().await;
    }
}

#[tokio::test]
async fn automatic_success_with_unchanged_upstream_evidence_never_spends_next_card() {
    let Some(db) = TestDatabase::create("auto_cycle").await else {
        return;
    };
    let store = auto_setup(&db).await;
    let check = store.claim_auto_check().await.unwrap().unwrap();
    let observation = evidence(&check);
    enqueue_auto(&store, &check, observation.clone()).await;
    let (batch, mut item) = store.claim().await.unwrap().unwrap();
    let cmd = automatic_command(&item);
    let ResetConsumePermit::Execute(claim) = store.begin_consume(&cmd, &context()).await.unwrap()
    else {
        panic!()
    };
    store
        .finish_consume(
            &cmd,
            claim,
            Some(&ProviderResetCreditResult {
                code: "reset".into(),
                credit: None,
            }),
        )
        .await
        .unwrap();
    item.status = ResetItemStatus::Succeeded;
    item.message = "success; readback failed".into();
    store.finish_item(batch.id, item).await.unwrap();
    due(&db).await;
    let check = store.claim_auto_check().await.unwrap().unwrap();
    let mut unchanged = evidence(&check);
    unchanged.triggered = observation.triggered.clone();
    enqueue_auto(&store, &check, unchanged).await;
    assert_eq!(
        store.batches().await.unwrap().len(),
        1,
        "even a new timestamp does not rearm an unchanged exhausted window"
    );
    due(&db).await;
    let check = store.claim_auto_check().await.unwrap().unwrap();
    let mut below = evidence(&check);
    below.triggered.clear();
    below.all_below = true;
    store
        .finish_auto_check(&check, Some(below), None, "below threshold")
        .await
        .unwrap();
    due(&db).await;
    let check = store.claim_auto_check().await.unwrap().unwrap();
    enqueue_auto(&store, &check, evidence(&check)).await;
    assert_eq!(
        store.batches().await.unwrap().len(),
        2,
        "confirmed recovery permits a later consumption cycle"
    );
    db.close().await;
}

#[tokio::test]
async fn automatic_unsent_cancellation_does_not_mark_the_window_as_consumed() {
    let Some(db) = TestDatabase::create("auto_unsent").await else {
        return;
    };
    let store = auto_setup(&db).await;
    let check = store.claim_auto_check().await.unwrap().unwrap();
    let observation = evidence(&check);
    enqueue_auto(&store, &check, observation.clone()).await;
    store
        .save_auto_policy("acct_reset_1", 1, AutoResetConfig::default(), &context())
        .await
        .unwrap();
    assert!(store.claim().await.unwrap().is_none());
    assert_eq!(
        store.batches().await.unwrap()[0].items[0].status,
        ResetItemStatus::Skipped
    );
    store
        .save_auto_policy(
            "acct_reset_1",
            2,
            AutoResetConfig {
                enabled: true,
                ..Default::default()
            },
            &context(),
        )
        .await
        .unwrap();
    let check = store.claim_auto_check().await.unwrap().unwrap();
    let mut fresh = evidence(&check);
    fresh.triggered = observation.triggered;
    enqueue_auto(&store, &check, fresh).await;
    assert_eq!(
        store.batches().await.unwrap().len(),
        2,
        "an unsent cancellation must not suppress a fresh check of the same window"
    );
    assert!(store.claim().await.unwrap().is_some());
    db.close().await;
}

#[tokio::test]
async fn automatic_zero_thresholds_do_not_claim_checks() {
    let Some(db) = TestDatabase::create("auto_zero").await else {
        return;
    };
    let store = auto_setup(&db).await;
    store
        .save_auto_policy(
            "acct_reset_1",
            1,
            AutoResetConfig {
                enabled: true,
                five_hour_used_millis: 0,
                seven_day_used_millis: 0,
            },
            &context(),
        )
        .await
        .unwrap();
    assert!(store.claim_auto_check().await.unwrap().is_none());
    db.close().await;
}

#[tokio::test]
async fn automatic_stale_checks_and_pre_refresh_manual_actions_cannot_enqueue() {
    let Some(db) = TestDatabase::create("auto_stale").await else {
        return;
    };
    let store = auto_setup(&db).await;
    let check = store.claim_auto_check().await.unwrap().unwrap();
    let observation = evidence(&check);
    let cmd = command("manual");
    let ResetConsumePermit::Execute(claim) = store.begin_consume(&cmd, &context()).await.unwrap()
    else {
        panic!()
    };
    store
        .finish_consume(
            &cmd,
            claim,
            Some(&ProviderResetCreditResult {
                code: "reset".into(),
                credit: None,
            }),
        )
        .await
        .unwrap();
    enqueue_auto(&store, &check, observation).await;
    assert!(store.batches().await.unwrap().is_empty());
    due(&db).await;
    let check = store.claim_auto_check().await.unwrap().unwrap();
    store
        .save_auto_policy("acct_reset_1", 1, AutoResetConfig::default(), &context())
        .await
        .unwrap();
    enqueue_auto(&store, &check, evidence(&check)).await;
    assert!(store.batches().await.unwrap().is_empty());
    db.close().await;
}
