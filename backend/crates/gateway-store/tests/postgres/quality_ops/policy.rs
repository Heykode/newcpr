use super::*;

async fn probe_setup(db: &TestDatabase) -> PgQualityOpsStore {
    let store = setup(db).await;
    sqlx::query("update provider_accounts set excel_models_follow_global=false,excel_models=array['fixture-model']")
        .execute(&db.pool).await.unwrap();
    store
}

fn probe_config(account: &str) -> QualityRuleConfig {
    let mut config = config(account);
    config.detection_mode = QualityDetectionMode::StateProbe;
    config.failure_action = QualityFailureAction::EnableExcel;
    config
}

async fn route(db: &TestDatabase, account: &str) -> String {
    sqlx::query_scalar("select responses_upstream from provider_accounts where id=$1")
        .bind(account)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn quality_probe_excel_action_is_atomic_and_independent_between_accounts() {
    let Some(db) = TestDatabase::create("quality_probe_atomic").await else {
        return;
    };
    let store = probe_setup(&db).await;
    for id in ["acct_quality_a", "acct_quality_b"] {
        store
            .save(None, None, probe_config(id), Utc::now(), &context())
            .await
            .unwrap();
    }
    let first = store.claim().await.unwrap().unwrap();
    let second = store.claim().await.unwrap().unwrap();
    for claim in [first, second] {
        store
            .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
            .await
            .unwrap();
        assert_eq!(route(&db, &claim.rule.config.account_id).await, "excel");
        let rule = store
            .rules()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.id == claim.rule.id)
            .unwrap();
        assert!(!rule.config.enabled);
        assert!(!rule.pending);
        assert!(!rule.running);
        assert_eq!(rule.revision, claim.rule.revision + 1);
        let run = store.detail(&claim.run_id).await.unwrap().unwrap();
        assert_eq!(run.status, "incorrect");
        assert_eq!(run.action.as_deref(), Some("excel_enabled_probe_paused"));
        assert_eq!(run.detection_mode, QualityDetectionMode::StateProbe);
    }
    assert!(store.claim().await.unwrap().is_none());
    // Neither a manual switch-off nor a 403 switch-off may resurrect a paused rule.
    sqlx::query(
        "update provider_accounts set responses_upstream='codex',excel_mode_disabled_at=now()",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(store.claim().await.unwrap().is_none());
    db.close().await;
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
    // Enabling Excel while queued/running invalidates the lease and late actions.
    let claim = store.claim().await.unwrap().unwrap();
    sqlx::query(
        "update provider_accounts set responses_upstream='excel' where id='acct_quality_a'",
    )
    .execute(&db.pool)
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
    assert!(!store.rules().await.unwrap()[0].config.enabled);
    assert!(
        store
            .enqueue(&rule.id, rule.revision, &context())
            .await
            .is_err()
    );
    db.close().await;
}

#[tokio::test]
async fn quality_excel_actions_respect_policy_changes_identity_and_current_availability() {
    let Some(db) = TestDatabase::create("quality_excel_guards").await else {
        return;
    };
    let store = probe_setup(&db).await;
    let edits = [
        "update provider_accounts set enabled=false where id=$1",
        "update provider_accounts set excel_mode_disabled_at=now() where id=$1",
        "update provider_accounts set excel_models=array['other-model'] where id=$1",
        "update provider_accounts set upstream_user_id='different' where id=$1",
        "update provider_accounts set access_token_expires_at=now()-interval '1 minute' where id=$1",
        "insert into admin_audit_events(id,actor_kind,actor_ref,action,entity_kind,entity_ref,
         config_revision,changed_fields,created_at)
         values('probe-manual-override','system','system','provider_account.update',
         'provider_account',$1,99,array['responses_upstream'],now())",
    ];
    for (index, edit) in edits.iter().enumerate() {
        let id = format!(
            "acct_quality_{}",
            char::from(b'a' + u8::try_from(index).unwrap())
        );
        store
            .save(None, None, probe_config(&id), Utc::now(), &context())
            .await
            .unwrap();
        let claim = store.claim().await.unwrap().unwrap();
        sqlx::query(*edit)
            .bind(&id)
            .execute(&db.pool)
            .await
            .unwrap();
        store
            .finish(
                &claim,
                Utc::now() + Duration::hours(6),
                vec![answer(QualityVerdict::Incorrect)],
            )
            .await
            .unwrap();
        assert_eq!(route(&db, &id).await, "codex");
        assert!(
            store
                .detail(&claim.run_id)
                .await
                .unwrap()
                .unwrap()
                .action
                .is_some()
        );
    }
    db.close().await;
}

#[tokio::test]
async fn quality_answer_mode_enables_excel_without_pausing_and_legacy_json_stays_readable() {
    let Some(db) = TestDatabase::create("quality_excel_answer").await else {
        return;
    };
    let store = probe_setup(&db).await;
    let mut config = config("acct_quality_a");
    config.failure_action = QualityFailureAction::EnableExcel;
    let rule = store
        .save(None, None, config, Utc::now(), &context())
        .await
        .unwrap();
    sqlx::query("update quality_rules set config=config-'detectionMode'-'excelFailureThreshold' where id=$1")
        .bind(&rule.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    assert_eq!(
        claim.rule.config.detection_mode,
        QualityDetectionMode::Answer
    );
    assert_eq!(claim.rule.config.excel_failure_threshold, 1);
    store
        .finish(
            &claim,
            Utc::now() + Duration::hours(6),
            vec![answer(QualityVerdict::Incorrect)],
        )
        .await
        .unwrap();
    assert_eq!(route(&db, "acct_quality_a").await, "excel");
    assert!(store.rules().await.unwrap()[0].config.enabled);
    assert_eq!(
        store
            .detail(&claim.run_id)
            .await
            .unwrap()
            .unwrap()
            .action
            .as_deref(),
        Some("excel_enabled")
    );
    db.close().await;
}

#[tokio::test]
async fn quality_excel_threshold_counts_confirmed_rounds_and_survives_store_restart() {
    let Some(db) = TestDatabase::create("quality_excel_streak").await else {
        return;
    };
    let store = probe_setup(&db).await;
    let mut config = probe_config("acct_quality_a");
    config.excel_failure_threshold = 3;
    store
        .save(None, None, config, Utc::now(), &context())
        .await
        .unwrap();
    let verdicts = [
        (QualityVerdict::Incorrect, 1),
        (QualityVerdict::Incorrect, 2),
        (QualityVerdict::Correct, 0),
        (QualityVerdict::Incorrect, 1),
        (QualityVerdict::Unknown, 1),
        (QualityVerdict::RequestError, 1),
        (QualityVerdict::Incorrect, 2),
        (QualityVerdict::Incorrect, 3),
    ];
    for (verdict, count) in verdicts {
        let store = PgQualityOpsStore::new(db.pool.clone());
        let claim = store.claim().await.unwrap().unwrap();
        store
            .finish(&claim, Utc::now(), vec![answer(verdict)])
            .await
            .unwrap();
        let rule = store.rules().await.unwrap().remove(0);
        assert_eq!(rule.excel_failure_streak, count);
        assert_eq!(
            route(&db, "acct_quality_a").await,
            if count == 3 { "excel" } else { "codex" }
        );
        assert_eq!(rule.config.enabled, count != 3);
        // A duplicated completion cannot count the same round twice.
        store
            .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
            .await
            .unwrap();
        assert_eq!(store.rules().await.unwrap()[0].excel_failure_streak, count);
    }
    assert!(store.claim().await.unwrap().is_none());
    db.close().await;
}

#[tokio::test]
async fn quality_excel_threshold_edits_reset_progress_and_fence_inflight_results() {
    let Some(db) = TestDatabase::create("quality_excel_streak_edit").await else {
        return;
    };
    let store = probe_setup(&db).await;
    let mut config = probe_config("acct_quality_a");
    config.excel_failure_threshold = 2;
    store
        .save(None, None, config, Utc::now(), &context())
        .await
        .unwrap();
    let first = store.claim().await.unwrap().unwrap();
    store
        .finish(&first, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    let stale = store.claim().await.unwrap().unwrap();
    let mut rule = store.rules().await.unwrap().remove(0);
    assert_eq!(rule.excel_failure_streak, 1);
    rule.config.excel_failure_threshold = 3;
    let saved = store
        .save(
            Some(&rule.id),
            Some(rule.revision),
            rule.config,
            Utc::now(),
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(saved.excel_failure_streak, 0);
    store
        .finish(&stale, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    assert_eq!(store.rules().await.unwrap()[0].excel_failure_streak, 0);
    assert_eq!(
        store.detail(&stale.run_id).await.unwrap().unwrap().status,
        "cancelled"
    );
    let fresh = store.claim().await.unwrap().unwrap();
    store
        .finish(&fresh, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    assert_eq!(store.rules().await.unwrap()[0].excel_failure_streak, 1);
    assert_eq!(route(&db, "acct_quality_a").await, "codex");
    db.close().await;
}

#[tokio::test]
async fn quality_excel_threshold_does_not_mix_changed_account_evidence() {
    let Some(db) = TestDatabase::create("quality_excel_streak_scope").await else {
        return;
    };
    let store = probe_setup(&db).await;
    let mut config = probe_config("acct_quality_a");
    config.excel_failure_threshold = 2;
    store
        .save(None, None, config, Utc::now(), &context())
        .await
        .unwrap();
    for replacement in ["identity_b", "identity_c"] {
        let first = store.claim().await.unwrap().unwrap();
        store
            .finish(&first, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
            .await
            .unwrap();
        assert_eq!(store.rules().await.unwrap()[0].excel_failure_streak, 1);
        sqlx::query("update provider_accounts set upstream_user_id=$1 where id='acct_quality_a'")
            .bind(replacement)
            .execute(&db.pool)
            .await
            .unwrap();
    }
    let claim = store.claim().await.unwrap().unwrap();
    sqlx::query(
        "update provider_accounts set excel_mode_disabled_at=now() where id='acct_quality_a'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    store
        .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    assert_eq!(route(&db, "acct_quality_a").await, "codex");
    assert_eq!(
        store
            .detail(&claim.run_id)
            .await
            .unwrap()
            .unwrap()
            .action
            .as_deref(),
        Some("excel_blocked_configuration_changed")
    );
    for _ in 0..2 {
        let claim = store.claim().await.unwrap().unwrap();
        store
            .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
            .await
            .unwrap();
    }
    assert_eq!(route(&db, "acct_quality_a").await, "codex");
    assert_eq!(
        store.rules().await.unwrap()[0].last_action.as_deref(),
        Some("excel_blocked_403")
    );
    db.close().await;
}

#[tokio::test]
async fn quality_excel_threshold_counts_one_round_not_parallel_samples() {
    let Some(db) = TestDatabase::create("quality_excel_streak_samples").await else {
        return;
    };
    let store = probe_setup(&db).await;
    let mut config = config("acct_quality_a");
    config.repetitions = 3;
    config.failure_action = QualityFailureAction::EnableExcel;
    config.excel_failure_threshold = 2;
    store
        .save(None, None, config, Utc::now(), &context())
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    store
        .finish(
            &claim,
            Utc::now(),
            (0..3).map(|_| answer(QualityVerdict::Incorrect)).collect(),
        )
        .await
        .unwrap();
    let rule = store.rules().await.unwrap().remove(0);
    assert_eq!(rule.excel_failure_streak, 1);
    assert_eq!(rule.last_action.as_deref(), Some("excel_threshold_pending"));
    assert_eq!(route(&db, "acct_quality_a").await, "codex");
    let second = store.claim().await.unwrap().unwrap();
    store
        .finish(
            &second,
            Utc::now(),
            (0..3).map(|_| answer(QualityVerdict::Incorrect)).collect(),
        )
        .await
        .unwrap();
    assert_eq!(route(&db, "acct_quality_a").await, "excel");
    assert!(store.rules().await.unwrap()[0].config.enabled);
    let recovered = store.claim().await.unwrap().unwrap();
    store
        .finish(
            &recovered,
            Utc::now(),
            (0..3).map(|_| answer(QualityVerdict::Correct)).collect(),
        )
        .await
        .unwrap();
    assert_eq!(store.rules().await.unwrap()[0].excel_failure_streak, 0);
    assert_eq!(route(&db, "acct_quality_a").await, "excel");
    db.close().await;
}
