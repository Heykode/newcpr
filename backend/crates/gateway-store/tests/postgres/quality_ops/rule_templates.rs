use super::*;

fn target(account: &str, rule: Option<&QualityRule>) -> QualityTemplateTarget {
    QualityTemplateTarget {
        account_id: account.into(),
        rule_id: rule.map(|rule| rule.id.clone()),
        revision: rule.map(|rule| rule.revision),
    }
}

async fn catalog(store: &PgQualityOpsStore) -> QualityRuleTemplate {
    store
        .save_template(
            None,
            None,
            "Monitoring template".into(),
            config(""),
            &context(),
        )
        .await
        .unwrap()
}

async fn accounts_snapshot(db: &TestDatabase) -> serde_json::Value {
    sqlx::query_scalar("select jsonb_agg(to_jsonb(a) order by id) from provider_accounts a")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn seconds_templates_round_trip_without_changing_legacy_schedules() {
    let Some(db) = TestDatabase::create("quality_seconds").await else {
        return;
    };
    let store = setup(&db).await;
    let legacy = catalog(&store).await;
    let mut seconds = config("");
    seconds.interval_seconds = Some(17);
    let template = store
        .save_template(None, None, "Seconds".into(), seconds, &context())
        .await
        .unwrap();
    let next = Utc::now() + Duration::seconds(17);
    let rule = store
        .apply_template(&template, &target("acct_quality_a", None), next, &context())
        .await
        .unwrap();
    assert_eq!(rule.config.interval_seconds, Some(17));
    assert!(!rule.pending);
    let loaded = store.rules().await.unwrap();
    assert_eq!(loaded[0].config.interval_seconds, Some(17));
    assert!(loaded[0].next_run_at <= Utc::now());
    let claim = store.claim().await.unwrap().unwrap();
    store
        .finish(&claim, next, vec![answer(QualityVerdict::Correct)])
        .await
        .unwrap();
    assert!(store.claim().await.unwrap().is_none());
    assert_eq!(
        store.rules().await.unwrap()[0]
            .next_run_at
            .timestamp_millis(),
        next.timestamp_millis()
    );
    let templates = store.templates().await.unwrap();
    let old = templates.iter().find(|item| item.id == legacy.id).unwrap();
    assert_eq!(old.config.interval_seconds, None);
    assert_eq!(old.config.cron, legacy.config.cron);
    assert_eq!(old.config.timezone, legacy.config.timezone);
    db.close().await;
}

#[tokio::test]
async fn monitoring_is_projected_in_account_list_and_detail_without_changing_account_settings() {
    use gateway_admin::{
        model::{
            PageSize,
            accounts::{AccountListQuery, AccountRuntimeSnapshot},
        },
        ports::store::AccountStore as _,
    };
    let Some(db) = TestDatabase::create("quality_monitor_projection").await else {
        return;
    };
    let store = setup(&db).await;
    let accounts = super::super::admin_account_store(&db.pool);
    let template = catalog(&store).await;
    let rule = store
        .apply_template(
            &template,
            &target("acct_quality_a", None),
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    let listed = accounts
        .list_accounts(
            AccountListQuery {
                page: 1,
                page_size: PageSize::new(20).unwrap(),
                provider_kind: None,
                group_filter: None,
                search: None,
                status: None,
                plan_type: None,
                sort: None,
            },
            AccountRuntimeSnapshot::default(),
        )
        .await
        .unwrap();
    let projected = listed
        .items
        .iter()
        .find(|item| item.account.id.as_str() == "acct_quality_a")
        .unwrap();
    assert_eq!(
        projected
            .account
            .quality_monitoring
            .as_ref()
            .unwrap()
            .rule_id,
        rule.id
    );
    assert!(
        listed
            .items
            .iter()
            .filter(|item| item.account.id.as_str() != "acct_quality_a")
            .all(|item| item.account.quality_monitoring.is_none())
    );
    let loaded = accounts
        .load_account("acct_quality_a", AccountRuntimeSnapshot::default())
        .await
        .unwrap()
        .unwrap();
    assert!(loaded.account.quality_monitoring.as_ref().unwrap().enabled);
    store
        .delete(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    let loaded = accounts
        .load_account("acct_quality_a", AccountRuntimeSnapshot::default())
        .await
        .unwrap()
        .unwrap();
    assert!(loaded.account.quality_monitoring.is_none());
    db.close().await;
}

#[tokio::test]
async fn rule_template_application_preserves_accounts_and_history_and_survives_catalog_delete() {
    let Some(db) = TestDatabase::create("quality_catalog_history").await else {
        return;
    };
    let store = setup(&db).await;
    let before = accounts_snapshot(&db).await;
    let template = catalog(&store).await;
    assert_eq!(store.templates().await.unwrap().len(), 1);
    assert!(
        serde_json::to_value(&template.config)
            .unwrap()
            .get("accountId")
            .is_none()
    );
    assert!(store.rules().await.unwrap().is_empty());
    let next = Utc::now() + Duration::hours(6);
    let mut forged = template.clone();
    forged.config.model = "untrusted-model".into();
    forged.name = "untrusted-name".into();
    let original = store
        .apply_template(&forged, &target("acct_quality_a", None), next, &context())
        .await
        .unwrap();
    assert_eq!(original.config.model, template.config.model);
    assert_eq!(
        original.source_template.as_ref().unwrap().name,
        template.name
    );
    let second = store
        .apply_template(&template, &target("acct_quality_b", None), next, &context())
        .await
        .unwrap();
    assert_ne!(original.id, second.id);
    assert!(!original.pending);
    assert!(original.next_run_at <= Utc::now());
    assert!(second.next_run_at <= Utc::now());
    assert_eq!(accounts_snapshot(&db).await, before);
    let old_run = scheduled_round(&store, &original, &[QualityVerdict::Correct]).await;
    let mut changed = template.config.clone();
    changed.enabled = false;
    changed.prompt = "new question".into();
    let updated = store
        .save_template(
            Some(&template.id),
            Some(template.revision),
            "Updated template".into(),
            changed,
            &context(),
        )
        .await
        .unwrap();
    assert!(
        store
            .rules()
            .await
            .unwrap()
            .iter()
            .all(|rule| rule.config.enabled),
        "catalog edits must not change live rules"
    );
    let replaced = store
        .apply_template(
            &updated,
            &target("acct_quality_a", Some(&original)),
            next,
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(original.id, replaced.id);
    assert!(!replaced.config.enabled);
    assert_eq!(store.runs(&replaced.id).await.unwrap().len(), 1);
    assert_eq!(
        store
            .detail(&old_run.id)
            .await
            .unwrap()
            .unwrap()
            .config
            .unwrap()
            .prompt,
        original.config.prompt
    );
    store
        .delete_template(&updated.id, updated.revision, &context())
        .await
        .unwrap();
    assert!(store.templates().await.unwrap().is_empty());
    assert_eq!(store.rules().await.unwrap().len(), 2);
    let monitors = store
        .monitoring(&[
            "acct_quality_a".into(),
            "acct_quality_b".into(),
            "acct_quality_c".into(),
        ])
        .await
        .unwrap();
    assert_eq!(monitors.len(), 2);
    assert!(!monitors["acct_quality_a"].enabled);
    assert_eq!(
        monitors["acct_quality_a"]
            .source_template
            .as_ref()
            .unwrap()
            .revision,
        updated.revision
    );
    assert!(monitors["acct_quality_b"].enabled);
    assert_eq!(accounts_snapshot(&db).await, before);
    db.close().await;
}

#[tokio::test]
async fn rule_templates_reject_stale_versions_and_wrong_targets_and_preserve_probe_guards() {
    let Some(db) = TestDatabase::create("quality_catalog_fences").await else {
        return;
    };
    let store = setup(&db).await;
    let template = catalog(&store).await;
    assert!(
        store
            .save_template(
                None,
                None,
                "Bound".into(),
                config("acct_quality_a"),
                &context()
            )
            .await
            .is_err()
    );
    let next = Utc::now() + Duration::hours(6);
    let original = store
        .apply_template(&template, &target("acct_quality_a", None), next, &context())
        .await
        .unwrap();
    assert!(
        store
            .apply_template(&template, &target("acct_quality_a", None), next, &context())
            .await
            .is_err()
    );
    assert!(
        store
            .apply_template(
                &template,
                &target("acct_quality_b", Some(&original)),
                next,
                &context()
            )
            .await
            .is_err()
    );
    let updated = store
        .apply_template(
            &template,
            &target("acct_quality_a", Some(&original)),
            next,
            &context(),
        )
        .await
        .unwrap();
    assert!(
        store
            .apply_template(
                &template,
                &target("acct_quality_a", Some(&original)),
                next,
                &context()
            )
            .await
            .is_err()
    );
    let mut probe = template.config.clone();
    probe.detection_mode = QualityDetectionMode::StateProbe;
    let new_template = store
        .save_template(
            Some(&template.id),
            Some(template.revision),
            template.name.clone(),
            probe,
            &context(),
        )
        .await
        .unwrap();
    assert!(
        store
            .save_template(
                Some(&template.id),
                Some(template.revision),
                template.name.clone(),
                template.config.clone(),
                &context()
            )
            .await
            .is_err()
    );
    assert!(
        store
            .delete_template(&template.id, template.revision, &context())
            .await
            .is_err()
    );
    assert!(
        store
            .apply_template(
                &template,
                &target("acct_quality_a", Some(&updated)),
                next,
                &context()
            )
            .await
            .is_err()
    );
    sqlx::query(
        "update provider_accounts set responses_upstream='excel' where id='acct_quality_b'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(
        store
            .apply_template(
                &new_template,
                &target("acct_quality_b", None),
                next,
                &context()
            )
            .await
            .is_ok()
    );
    assert!(
        store
            .apply_template(
                &new_template,
                &target("acct_quality_c", None),
                next,
                &context()
            )
            .await
            .is_ok()
    );
    assert_eq!(store.rules().await.unwrap().len(), 3);
    store
        .delete_template(&new_template.id, new_template.revision, &context())
        .await
        .unwrap();
    assert!(
        store
            .apply_template(
                &new_template,
                &target("acct_quality_d", None),
                next,
                &context()
            )
            .await
            .is_err()
    );
    db.close().await;
}

#[tokio::test]
async fn monitoring_tracks_queue_lease_pause_and_deletion_and_fences_old_results() {
    let Some(db) = TestDatabase::create("quality_monitor_states").await else {
        return;
    };
    let store = setup(&db).await;
    let original = store
        .save(
            None,
            None,
            config("acct_quality_a"),
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    let ids = ["acct_quality_a".into()];
    let monitor = store
        .monitoring(&ids)
        .await
        .unwrap()
        .remove("acct_quality_a")
        .unwrap();
    assert!(monitor.enabled);
    assert!(!monitor.running && !monitor.pending);
    assert!(monitor.source_template.is_none(), "legacy NULL must decode");
    store
        .enqueue(&original.id, original.revision, &context())
        .await
        .unwrap();
    assert!(store.monitoring(&ids).await.unwrap()["acct_quality_a"].pending);
    let claim = store.claim().await.unwrap().unwrap();
    assert!(store.monitoring(&ids).await.unwrap()["acct_quality_a"].running);
    let mut paused_config = config("");
    paused_config.enabled = false;
    let template = store
        .save_template(None, None, "Paused".into(), paused_config, &context())
        .await
        .unwrap();
    let paused = store
        .apply_template(
            &template,
            &target("acct_quality_a", Some(&original)),
            Utc::now() + Duration::hours(6),
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
    let monitor = &store.monitoring(&ids).await.unwrap()["acct_quality_a"];
    assert!(!monitor.enabled && !monitor.running && !monitor.pending);
    assert_eq!(monitor.revision, paused.revision);
    store
        .delete(&paused.id, paused.revision, &context())
        .await
        .unwrap();
    assert!(store.monitoring(&ids).await.unwrap().is_empty());
    db.close().await;
}
