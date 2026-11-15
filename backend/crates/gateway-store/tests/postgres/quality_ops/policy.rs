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
        assert!(rule.config.enabled);
        assert!(!rule.pending);
        assert!(!rule.running);
        assert_eq!(rule.revision, claim.rule.revision);
        let run = store.detail(&claim.run_id).await.unwrap().unwrap();
        assert_eq!(run.status, "incorrect");
        assert_eq!(run.action.as_deref(), Some("excel_enabled"));
        assert_eq!(run.detection_mode, QualityDetectionMode::StateProbe);
    }
    let continuing = store
        .claim()
        .await
        .unwrap()
        .expect("Excel keeps native monitoring active");
    store
        .finish(
            &continuing,
            Utc::now(),
            vec![answer(QualityVerdict::Correct)],
        )
        .await
        .unwrap();
    // A 403 switch-off remains protected from being automatically re-enabled.
    sqlx::query(
        "update provider_accounts set responses_upstream='codex',excel_mode_disabled_at=now()",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    store
        .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    assert_eq!(route(&db, &claim.rule.config.account_id).await, "codex");
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
    // Manual route changes keep detection alive but fence this round's actions.
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
        Some("excel_blocked_configuration_changed")
    );
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
        let expected_streak = if count == 3 { 0 } else { count };
        assert_eq!(rule.excel_failure_streak, expected_streak);
        assert_eq!(
            route(&db, "acct_quality_a").await,
            if count == 3 { "excel" } else { "codex" }
        );
        assert!(rule.config.enabled);
        // A duplicated completion cannot count the same round twice.
        store
            .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
            .await
            .unwrap();
        assert_eq!(
            store.rules().await.unwrap()[0].excel_failure_streak,
            expected_streak
        );
    }
    assert!(store.claim().await.unwrap().is_some());
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
