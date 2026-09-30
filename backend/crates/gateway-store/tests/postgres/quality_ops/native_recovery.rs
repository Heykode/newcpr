use super::*;

#[tokio::test]
async fn native_recovery_option_edits_preserve_ownership_but_model_edits_release_it() {
    for change_model in [false, true] {
        let Some(db) = TestDatabase::create("quality_native_rule_edits").await else {
            return;
        };
        let store = policy::probe_setup(&db).await;
        let config = policy::probe_config("acct_quality_a");
        let rule = store
            .save(
                None,
                None,
                config,
                Utc::now() + Duration::hours(1),
                &context(),
            )
            .await
            .unwrap();
        scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
        let mut config = rule.config.clone();
        config.disable_excel_on_native_recovery = true;
        config.interval_seconds = Some(90);
        if change_model {
            sqlx::query("update provider_accounts set excel_models=array['fixture-model','other-model'] where id='acct_quality_a'").execute(&db.pool).await.unwrap();
            config.model = "other-model".into();
        }
        let updated = store
            .save(
                Some(&rule.id),
                Some(rule.revision),
                config,
                Utc::now() + Duration::hours(1),
                &context(),
            )
            .await
            .unwrap();
        assert!(store.claim().await.unwrap().is_none());
        let restarted = PgQualityOpsStore::new(db.pool.clone());
        scheduled_round(&restarted, &updated, &[QualityVerdict::Correct]).await;
        assert_eq!(
            policy::route(&db, "acct_quality_a").await,
            if change_model { "excel" } else { "codex" }
        );
        db.close().await;
    }
}

#[tokio::test]
async fn native_recovery_is_opt_in_owned_and_repeats_without_403_markers() {
    for enabled in [false, true] {
        let Some(db) = TestDatabase::create("quality_native_recovery").await else {
            return;
        };
        let store = policy::probe_setup(&db).await;
        let mut config = policy::probe_config("acct_quality_a");
        config.disable_excel_on_native_recovery = enabled;
        let rule = store
            .save(
                None,
                None,
                config,
                Utc::now() + Duration::hours(1),
                &context(),
            )
            .await
            .unwrap();
        assert!(
            store.claim().await.unwrap().is_none(),
            "save is not an immediate run"
        );
        let degraded = scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
        assert_eq!(degraded.action.as_deref(), Some("excel_enabled"));
        for verdict in [
            QualityVerdict::Unknown,
            QualityVerdict::RequestError,
            QualityVerdict::Incorrect,
        ] {
            scheduled_round(&store, &rule, &[verdict]).await;
            assert_eq!(policy::route(&db, "acct_quality_a").await, "excel");
        }
        let normal = scheduled_round(&store, &rule, &[QualityVerdict::Correct]).await;
        assert_eq!(
            policy::route(&db, "acct_quality_a").await,
            if enabled { "codex" } else { "excel" }
        );
        assert_eq!(
            normal.action.as_deref(),
            Some(if enabled {
                "excel_disabled_native_recovered"
            } else {
                "excel_recovery_pending"
            })
        );
        assert!(store.rules().await.unwrap()[0].config.enabled);
        if enabled {
            let clean: bool = sqlx::query_scalar("select excel_mode_disabled_at is null and excel_auto_disabled_at is null from provider_accounts where id='acct_quality_a'").fetch_one(&db.pool).await.unwrap();
            assert!(clean);
            assert_eq!(
                scheduled_round(&store, &rule, &[QualityVerdict::Incorrect])
                    .await
                    .action
                    .as_deref(),
                Some("excel_enabled")
            );
        }
        db.close().await;
    }
}

#[tokio::test]
async fn native_recovery_never_closes_unowned_or_manually_changed_excel() {
    for edit in [
        None,
        Some("update provider_accounts set enabled=false where id='acct_quality_a'"),
        Some(
            "update provider_accounts set upstream_user_id='new-identity' where id='acct_quality_a'",
        ),
        Some("update provider_accounts set excel_mode_disabled_at=now() where id='acct_quality_a'"),
        Some(
            "insert into admin_audit_events(id,actor_kind,actor_ref,action,entity_kind,entity_ref,config_revision,changed_fields,created_at) values('manual-same-value','system','system','provider_account.update','provider_account','acct_quality_a',999,array['responses_upstream'],now())",
        ),
    ] {
        let Some(db) = TestDatabase::create("quality_native_ownership").await else {
            return;
        };
        let store = policy::probe_setup(&db).await;
        let mut config = policy::probe_config("acct_quality_a");
        config.disable_excel_on_native_recovery = true;
        let rule = store
            .save(
                None,
                None,
                config,
                Utc::now() + Duration::hours(1),
                &context(),
            )
            .await
            .unwrap();
        if let Some(edit) = edit {
            scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
            sqlx::query(edit).execute(&db.pool).await.unwrap();
        } else {
            sqlx::query(
                "update provider_accounts set responses_upstream='excel' where id='acct_quality_a'",
            )
            .execute(&db.pool)
            .await
            .unwrap();
        }
        scheduled_round(&store, &rule, &[QualityVerdict::Correct]).await;
        assert_eq!(policy::route(&db, "acct_quality_a").await, "excel");
        db.close().await;
    }
}

#[tokio::test]
async fn native_recovery_is_fenced_by_edits_during_a_running_round() {
    let Some(db) = TestDatabase::create("quality_native_race").await else {
        return;
    };
    let store = policy::probe_setup(&db).await;
    let mut config = policy::probe_config("acct_quality_a");
    config.disable_excel_on_native_recovery = true;
    let rule = store
        .save(
            None,
            None,
            config,
            Utc::now() + Duration::hours(1),
            &context(),
        )
        .await
        .unwrap();
    scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
    store
        .enqueue(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    sqlx::query("update provider_accounts set enabled=false where id='acct_quality_a'")
        .execute(&db.pool)
        .await
        .unwrap();
    store
        .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Correct)])
        .await
        .unwrap();
    assert_eq!(policy::route(&db, "acct_quality_a").await, "excel");
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
    db.close().await;
}
