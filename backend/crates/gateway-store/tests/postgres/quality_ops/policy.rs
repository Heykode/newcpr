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

pub(super) fn probe_config(account: &str) -> QualityRuleConfig {
    let mut config = config(account);
    config.detection_mode = QualityDetectionMode::StateProbe;
    config.failure_action = QualityFailureAction::EnableExcel;
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
async fn legacy_enable_excel_actions_require_manual_route_switching() {
    for detection_mode in [
        QualityDetectionMode::Answer,
        QualityDetectionMode::StateProbe,
    ] {
        for route in ["codex", "excel"] {
            let Some(db) = TestDatabase::create("quality_manual_route").await else {
                return;
            };
            let store = setup(&db).await;
            sqlx::query(
                "update provider_accounts set responses_upstream=$1 where id='acct_quality_a'",
            )
            .bind(route)
            .execute(&db.pool)
            .await
            .unwrap();
            let mut cfg = probe_config("acct_quality_a");
            cfg.detection_mode = detection_mode;
            cfg.disable_excel_on_native_recovery = true;
            let rule = store
                .save(None, None, cfg, Utc::now(), &context())
                .await
                .unwrap();
            for verdict in [
                QualityVerdict::Incorrect,
                QualityVerdict::Correct,
                QualityVerdict::Unknown,
                QualityVerdict::RequestError,
            ] {
                let incorrect = matches!(verdict, QualityVerdict::Incorrect);
                let run = scheduled_round(&store, &rule, &[verdict]).await;
                if incorrect {
                    assert_eq!(
                        run.action.as_deref(),
                        Some("route_switch_requires_confirmation")
                    );
                }
                let actual: String = sqlx::query_scalar(
                    "select responses_upstream from provider_accounts where id='acct_quality_a'",
                )
                .fetch_one(&db.pool)
                .await
                .unwrap();
                assert_eq!(actual, route);
                assert!(enabled(&db).await);
                assert!(store.rules().await.unwrap()[0].config.enabled);
            }
            db.close().await;
        }
    }
}

#[tokio::test]
async fn legacy_excel_threshold_keeps_evidence_without_switching_the_route() {
    let Some(db) = TestDatabase::create("quality_manual_threshold").await else {
        return;
    };
    let store = setup(&db).await;
    let mut cfg = probe_config("acct_quality_a");
    cfg.excel_failure_threshold = 2;
    let rule = store
        .save(None, None, cfg, Utc::now(), &context())
        .await
        .unwrap();
    for (verdict, expected) in [
        (QualityVerdict::Incorrect, "excel_threshold_pending"),
        (
            QualityVerdict::Incorrect,
            "route_switch_requires_confirmation",
        ),
        (QualityVerdict::Correct, "excel_streak_reset"),
    ] {
        let restarted = PgQualityOpsStore::new(db.pool.clone());
        let run = scheduled_round(&restarted, &rule, &[verdict]).await;
        assert_eq!(run.action.as_deref(), Some(expected));
        assert_eq!(route(&db, "acct_quality_a").await, "codex");
    }
    db.close().await;
}

#[tokio::test]
async fn route_action_inflight_configuration_edits_are_still_fenced() {
    let Some(db) = TestDatabase::create("quality_manual_route_fence").await else {
        return;
    };
    let store = setup(&db).await;
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
    let claim = store.claim().await.unwrap().unwrap();
    sqlx::query("update provider_accounts set model_access_json='{\"mode\":\"denylist\",\"models\":[\"fixture-model\"]}' where id='acct_quality_a'")
        .execute(&db.pool)
        .await
        .unwrap();
    store
        .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    let run = store.detail(&claim.run_id).await.unwrap().unwrap();
    assert_eq!(
        run.action.as_deref(),
        Some("excel_blocked_configuration_changed")
    );
    assert_eq!(route(&db, "acct_quality_a").await, "codex");
    assert!(
        store
            .rules()
            .await
            .unwrap()
            .iter()
            .any(|item| item.id == rule.id)
    );
    db.close().await;
}
