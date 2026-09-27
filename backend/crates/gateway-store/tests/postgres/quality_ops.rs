use super::TestDatabase;
use chrono::{Duration, Utc};
use gateway_admin::{
    model::{MutationActor, MutationContext, quality_ops::*},
    ports::quality_ops::QualityOpsStore,
};
use gateway_store::postgres::quality_ops::PgQualityOpsStore;

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "quality-fixture".into(),
    }
}

fn config(account: &str) -> QualityRuleConfig {
    QualityRuleConfig {
        account_id: account.into(),
        model: "fixture-model".into(),
        enabled: true,
        cron: "0 */6 * * *".into(),
        timezone: "UTC".into(),
        repetitions: 1,
        prompt: "fixture question".into(),
        reference_answer: "fixture answer".into(),
        reasoning_effort: None,
        judge_group_id: "fixture-group".into(),
        judge_model: "fixture-judge".into(),
        judge_prompt: "compare".into(),
    }
}

async fn setup(db: &TestDatabase) -> PgQualityOpsStore {
    for id in ["quality-a", "quality-b", "quality-c"] {
        sqlx::query(
            "insert into provider_accounts(
            id,provider_kind,name,upstream_user_id,authentication_kind,provider_credentials_json,
            has_refresh_token,credential_observed_at,created_at,updated_at)
            values($1,'openai',$1,$1,'oauth','{}',false,now(),now(),now())",
        )
        .bind(id)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    PgQualityOpsStore::new(db.pool.clone())
}

fn answer(verdict: QualityVerdict) -> QualityAnswer {
    QualityAnswer {
        index: 1,
        answer: "fixture answer".into(),
        verdict,
        reason: "fixture reason".into(),
        elapsed_ms: 20,
        returned_model: Some("fixture-model".into()),
        judge_account_id: Some("quality-b".into()),
    }
}

#[tokio::test]
async fn quality_claims_are_globally_bounded_and_results_are_lazy() {
    let Some(db) = TestDatabase::create("quality_claims").await else {
        return;
    };
    let store = setup(&db).await;
    for id in ["quality-a", "quality-b", "quality-c"] {
        store
            .save(
                None,
                None,
                config(id),
                Utc::now() - Duration::minutes(1),
                &context(),
            )
            .await
            .unwrap();
    }
    let (a, b, c) = tokio::join!(store.claim(), store.claim(), store.claim());
    let claims = [a.unwrap(), b.unwrap(), c.unwrap()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(claims.len(), 2);
    assert_ne!(claims[0].rule.id, claims[1].rule.id);
    assert!(store.claim().await.unwrap().is_none());
    store
        .finish(
            &claims[0],
            Utc::now() + Duration::hours(6),
            vec![answer(QualityVerdict::Correct)],
        )
        .await
        .unwrap();
    let history = store.runs(&claims[0].rule.id).await.unwrap();
    assert_eq!(history[0].correct, 1);
    assert!(history[0].answers.is_empty());
    assert_eq!(
        store
            .detail(&claims[0].run_id)
            .await
            .unwrap()
            .unwrap()
            .answers
            .len(),
        1
    );
    assert!(store.claim().await.unwrap().is_some());
    db.close().await;
}

#[tokio::test]
async fn quality_pause_fences_late_completion_and_manual_run_is_deduplicated() {
    let Some(db) = TestDatabase::create("quality_pause").await else {
        return;
    };
    let store = setup(&db).await;
    let original = store
        .save(None, None, config("quality-a"), Utc::now(), &context())
        .await
        .unwrap();
    assert!(
        store
            .save(None, None, config("quality-a"), Utc::now(), &context())
            .await
            .is_err()
    );
    let claim = store.claim().await.unwrap().unwrap();
    let mut changed = original.config.clone();
    changed.enabled = false;
    let updated = store
        .save(
            Some(&original.id),
            Some(original.revision),
            changed,
            Utc::now(),
            &context(),
        )
        .await
        .unwrap();
    assert!(!store.current(&claim).await.unwrap());
    store
        .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    assert_eq!(
        store.detail(&claim.run_id).await.unwrap().unwrap().status,
        "cancelled"
    );
    assert!(store.claim().await.unwrap().is_none());
    store
        .enqueue(&updated.id, updated.revision, &context())
        .await
        .unwrap();
    assert!(
        store
            .enqueue(&updated.id, updated.revision, &context())
            .await
            .is_err()
    );
    let manual = store.claim().await.unwrap().unwrap();
    assert!(store.current(&manual).await.unwrap());
    assert!(
        store
            .delete(&updated.id, original.revision, &context())
            .await
            .is_err()
    );
    store
        .delete(&updated.id, updated.revision, &context())
        .await
        .unwrap();
    assert!(!store.current(&manual).await.unwrap());
    assert!(store.detail(&manual.run_id).await.unwrap().is_none());
    db.close().await;
}

#[tokio::test]
async fn quality_expired_lease_recovers_and_errors_do_not_count_as_incorrect() {
    let Some(db) = TestDatabase::create("quality_recovery").await else {
        return;
    };
    let store = setup(&db).await;
    store
        .save(None, None, config("quality-a"), Utc::now(), &context())
        .await
        .unwrap();
    let old = store.claim().await.unwrap().unwrap();
    sqlx::query("update quality_rules set lease_until=now()-interval '1 second'")
        .execute(&db.pool)
        .await
        .unwrap();
    let new = store.claim().await.unwrap().unwrap();
    assert_ne!(old.lease_token, new.lease_token);
    assert!(!store.current(&old).await.unwrap());
    assert_eq!(
        store.detail(&old.run_id).await.unwrap().unwrap().status,
        "interrupted"
    );
    store
        .finish(
            &new,
            Utc::now() + Duration::hours(6),
            vec![answer(QualityVerdict::RequestError)],
        )
        .await
        .unwrap();
    let result = store.detail(&new.run_id).await.unwrap().unwrap();
    assert_eq!(result.request_errors, 1);
    assert_eq!(result.incorrect, 0);
    sqlx::query("update quality_runs set started_at=now()-interval '8 days'")
        .execute(&db.pool)
        .await
        .unwrap();
    store.cleanup().await.unwrap();
    assert!(store.runs(&new.rule.id).await.unwrap().is_empty());
    db.close().await;
}
