use super::*;

#[tokio::test]
async fn quality_overload_counts_rounds_and_preserves_normal_reset_with_record_only() {
    use QualityVerdict::{Correct, Incorrect, Overloaded, RequestError, Unknown};
    let Some(db) = TestDatabase::create("quality_overload_rounds").await else {
        return;
    };
    let store = setup(&db).await;
    let mut cfg = config("acct_quality_a");
    cfg.repetitions = 2;
    let rule = store
        .save(None, None, cfg, Utc::now(), &context())
        .await
        .unwrap();
    for (verdicts, expected) in [
        (vec![Overloaded, Overloaded], "overloaded"),
        (vec![Unknown, RequestError], "request_error"),
        (vec![Overloaded, Correct], "incorrect"),
        (vec![Correct, Correct], "correct"),
        (vec![Overloaded, Correct], "overloaded"),
        (vec![Correct, Correct], "correct"),
        (vec![Incorrect, Correct], "incorrect"),
        (vec![Overloaded, Correct], "incorrect"),
    ] {
        let run = scheduled_round(&store, &rule, &verdicts).await;
        assert_eq!(run.status, expected);
        assert!(run.action.is_none());
        assert!(enabled(&db).await);
    }
    db.close().await;
}

#[tokio::test]
async fn quality_overload_duplicate_completion_edits_and_identity_changes_are_fenced() {
    use QualityVerdict::Overloaded;
    let Some(db) = TestDatabase::create("quality_overload_fences").await else {
        return;
    };
    let store = setup(&db).await;
    let rule = store
        .save(None, None, config("acct_quality_a"), Utc::now(), &context())
        .await
        .unwrap();
    let first = store.claim().await.unwrap().unwrap();
    store
        .finish(&first, Utc::now(), vec![answer(Overloaded)])
        .await
        .unwrap();
    store
        .finish(&first, Utc::now(), vec![answer(Overloaded)])
        .await
        .unwrap();
    assert_eq!(
        store.detail(&first.run_id).await.unwrap().unwrap().status,
        "overloaded"
    );
    let count: i32 = sqlx::query_scalar(
        "select (recovery#>>'{overload_streak,count}')::int from quality_rules where id=$1",
    )
    .bind(&rule.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let stale = store.claim().await.unwrap().unwrap();
    let edited = store
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
        .finish(&stale, Utc::now(), vec![answer(Overloaded)])
        .await
        .unwrap();
    assert_eq!(
        store.detail(&stale.run_id).await.unwrap().unwrap().status,
        "cancelled"
    );
    assert_eq!(
        scheduled_round(&store, &edited, &[Overloaded]).await.status,
        "overloaded"
    );
    sqlx::query("update provider_accounts set upstream_user_id='replacement-identity' where id='acct_quality_a'")
        .execute(&db.pool).await.unwrap();
    assert_eq!(
        scheduled_round(&store, &edited, &[Overloaded]).await.status,
        "overloaded"
    );
    // A changed policy cannot retain one round from the old account conditions.
    sqlx::query(
        "update provider_accounts set responses_upstream='excel' where id='acct_quality_a'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        scheduled_round(&store, &edited, &[Overloaded]).await.status,
        "overloaded"
    );
    assert_eq!(
        scheduled_round(&store, &edited, &[Overloaded]).await.status,
        "incorrect"
    );
    db.close().await;
}

#[tokio::test]
async fn quality_overload_reuses_scheduling_action_and_keeps_explicit_error_behavior() {
    use QualityVerdict::{Correct, Incorrect, Overloaded};
    let Some(db) = TestDatabase::create("quality_overload_pause").await else {
        return;
    };
    let store = setup(&db).await;
    let mut cfg = config("acct_quality_a");
    cfg.failure_action = QualityFailureAction::DisableScheduling;
    cfg.failure_threshold = Some(5);
    cfg.auto_restore = true;
    let rule = store
        .save(None, None, cfg, Utc::now(), &context())
        .await
        .unwrap();
    assert!(
        scheduled_round(&store, &rule, &[Overloaded])
            .await
            .action
            .is_none()
    );
    assert!(enabled(&db).await);
    assert_eq!(
        scheduled_round(&store, &rule, &[Overloaded])
            .await
            .action
            .as_deref(),
        Some("scheduling_paused")
    );
    assert!(!enabled(&db).await);
    assert_eq!(
        scheduled_round(&store, &rule, &[Correct])
            .await
            .action
            .as_deref(),
        Some("restored")
    );
    assert!(enabled(&db).await);
    // Pure explicit failures retain the configured threshold after restoration.
    assert_eq!(
        scheduled_round(&store, &rule, &[Incorrect])
            .await
            .action
            .as_deref(),
        Some("excel_threshold_pending")
    );
    assert!(enabled(&db).await);
    // Omitted thresholds still preserve the legacy single-failure timing.
    let mut cfg = rule.config.clone();
    cfg.failure_threshold = None;
    let rule = store
        .save(
            Some(&rule.id),
            Some(rule.revision),
            cfg,
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
