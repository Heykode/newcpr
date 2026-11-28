use super::*;

#[tokio::test]
async fn quality_pause_threshold_is_durable_and_legacy_excel_value_is_not_reinterpreted() {
    use QualityVerdict::{Correct, Incorrect, RequestError, Unknown};
    let Some(db) = TestDatabase::create("quality_pause_threshold").await else {
        return;
    };
    let store = setup(&db).await;
    let mut config = config("acct_quality_a");
    config.failure_action = QualityFailureAction::DisableScheduling;
    config.failure_threshold = Some(3);
    config.auto_restore = true;
    let rule = store
        .save(None, None, config, Utc::now(), &context())
        .await
        .unwrap();
    for (verdict, count) in [
        (Incorrect, 1),
        (Correct, 0),
        (Incorrect, 1),
        (Unknown, 1),
        (RequestError, 1),
        (Incorrect, 2),
    ] {
        let store = PgQualityOpsStore::new(db.pool.clone());
        scheduled_round(&store, &rule, &[verdict]).await;
        assert!(enabled(&db).await);
        assert_eq!(store.rules().await.unwrap()[0].excel_failure_streak, count);
    }
    let run = scheduled_round(&store, &rule, &[Incorrect]).await;
    assert_eq!(run.action.as_deref(), Some("scheduling_paused"));
    assert!(!enabled(&db).await);
    assert_eq!(
        scheduled_round(&store, &rule, &[Correct])
            .await
            .action
            .as_deref(),
        Some("restored")
    );
    assert!(enabled(&db).await);
    let mut config = rule.config.clone();
    config.failure_threshold = None;
    config.excel_failure_threshold = 9;
    let rule = store
        .save(
            Some(&rule.id),
            Some(rule.revision),
            config,
            Utc::now(),
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[Incorrect])
            .await
            .action
            .as_deref(),
        Some("scheduling_paused")
    );
    db.close().await;
}

#[tokio::test]
async fn quality_remove_threshold_preserves_other_groups_and_fences_duplicate_or_stale_rounds() {
    let Some(db) = TestDatabase::create("quality_remove_threshold").await else {
        return;
    };
    let store = setup(&db).await;
    let group = "grp_00000000000000000000000000000001";
    sqlx::query("insert into account_groups(id,name,color,enabled,created_at,updated_at) values($1,$1,'#FFFFFFFF',true,now(),now())")
        .bind(group).execute(&db.pool).await.unwrap();
    sqlx::query("insert into account_group_accounts values($1,'acct_quality_a',now())")
        .bind(group)
        .execute(&db.pool)
        .await
        .unwrap();
    let mut config = config("acct_quality_a");
    config.failure_action = QualityFailureAction::RemoveGroups;
    config.failure_group_ids = vec![group.into()];
    config.failure_threshold = Some(2);
    config.auto_restore = true;
    let mut rule = store
        .save(None, None, config, Utc::now(), &context())
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    for _ in 0..2 {
        store
            .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
            .await
            .unwrap();
        assert_eq!(store.rules().await.unwrap()[0].excel_failure_streak, 1);
    }
    let stale = store.claim().await.unwrap().unwrap();
    rule = store
        .save(
            Some(&rule.id),
            Some(rule.revision),
            rule.config.clone(),
            Utc::now(),
            &context(),
        )
        .await
        .unwrap();
    store
        .finish(&stale, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    assert_eq!(store.rules().await.unwrap()[0].excel_failure_streak, 0);
    for expected in ["excel_threshold_pending", "groups_removed"] {
        assert_eq!(
            scheduled_round(&store, &rule, &[QualityVerdict::Incorrect])
                .await
                .action
                .as_deref(),
            Some(expected)
        );
    }
    assert!(enabled(&db).await);
    let count: i64 = sqlx::query_scalar(
        "select count(*) from account_group_accounts where provider_account_id='acct_quality_a'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        scheduled_round(&store, &rule, &[QualityVerdict::Correct])
            .await
            .action
            .as_deref(),
        Some("restored")
    );
    let count: i64 = sqlx::query_scalar(
        "select count(*) from account_group_accounts where provider_account_id='acct_quality_a'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    db.close().await;
}

pub(super) async fn probe_setup(db: &TestDatabase) -> PgQualityOpsStore {
    let store = setup(db).await;
    sqlx::query("update provider_accounts set excel_models_follow_global=false,excel_models=array['fixture-model']")
        .execute(&db.pool).await.unwrap();
    store
}

pub(super) fn probe_config(account: &str) -> QualityRuleConfig {
    let mut config = config(account);
    config.detection_mode = QualityDetectionMode::StateProbe;
    config.failure_action = QualityFailureAction::None;
    config
}

pub(super) async fn route(db: &TestDatabase, account: &str) -> String {
    sqlx::query_scalar("select responses_upstream from provider_accounts where id=$1")
        .bind(account)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn quality_probe_unknown_and_errors_never_change_route_or_pause_rule() {
    let Some(db) = TestDatabase::create("quality_probe_unknown").await else {
        return;
    };
    let store = probe_setup(&db).await;
    let rule = store
        .save(
            None,
            None,
            probe_config("acct_quality_a"),
            Utc::now(),
            &context(),
        )
        .await
        .unwrap();
    for verdict in [
        QualityVerdict::Unknown,
        QualityVerdict::RequestError,
        QualityVerdict::Correct,
    ] {
        let claim = store.claim().await.unwrap().unwrap();
        store
            .finish(&claim, Utc::now(), vec![answer(verdict)])
            .await
            .unwrap();
        assert_eq!(route(&db, "acct_quality_a").await, "codex");
        assert!(store.rules().await.unwrap()[0].config.enabled);
        assert!(
            store
                .detail(&claim.run_id)
                .await
                .unwrap()
                .unwrap()
                .action
                .is_none()
        );
    }
    // Record-only probes retain legacy routes and never perform a route action.
    let claim = store.claim().await.unwrap().unwrap();
    sqlx::query(
        "update provider_accounts set responses_upstream='excel' where id='acct_quality_a'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(store.current(&claim).await.unwrap());
    store
        .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    assert_eq!(
        store.detail(&claim.run_id).await.unwrap().unwrap().status,
        "incorrect"
    );
    assert_eq!(
        store
            .detail(&claim.run_id)
            .await
            .unwrap()
            .unwrap()
            .action
            .as_deref(),
        None
    );
    assert_eq!(route(&db, "acct_quality_a").await, "excel");
    assert!(store.rules().await.unwrap()[0].config.enabled);
    assert!(
        store
            .enqueue(&rule.id, rule.revision, &context())
            .await
            .is_ok()
    );
    db.close().await;
}

#[tokio::test]
async fn quality_paused_manual_probes_keep_excel_route_and_remain_paused_after_run() {
    let Some(db) = TestDatabase::create("quality_manual_probe").await else {
        return;
    };
    let store = probe_setup(&db).await;
    // Excel may be enabled before enqueue, before claim, or during a run.
    for stage in 0..3 {
        sqlx::query(
            "update provider_accounts set responses_upstream='codex' where id='acct_quality_a'",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        let mut config = probe_config("acct_quality_a");
        config.enabled = false;
        let rule = store
            .save(None, None, config, Utc::now(), &context())
            .await
            .unwrap();
        if stage > 0 {
            store
                .enqueue(&rule.id, rule.revision, &context())
                .await
                .unwrap();
        }
        let claim = if stage == 2 {
            Some(store.claim().await.unwrap().unwrap())
        } else {
            None
        };
        sqlx::query(
            "update provider_accounts set responses_upstream='excel' where id='acct_quality_a'",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        if stage == 0 {
            store
                .enqueue(&rule.id, rule.revision, &context())
                .await
                .unwrap();
        }
        let claim = match claim {
            Some(claim) => claim,
            None => store.claim().await.unwrap().unwrap(),
        };
        assert!(store.current(&claim).await.unwrap());
        store
            .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Correct)])
            .await
            .unwrap();
        assert_eq!(route(&db, "acct_quality_a").await, "excel");
        assert!(store.claim().await.unwrap().is_none());
        let paused = store.rules().await.unwrap().remove(0);
        assert!(!paused.config.enabled);
        assert!(!paused.pending);
        assert!(!paused.running);
        // Guard checks must not repeatedly change the revision of an idle paused rule.
        assert_eq!(store.rules().await.unwrap()[0].revision, paused.revision);
        store
            .delete(&paused.id, paused.revision, &context())
            .await
            .unwrap();
    }
    db.close().await;
}
