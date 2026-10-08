use super::*;
use gateway_admin::model::relogin_templates::ReloginTemplate;
use serde_json::{Value, json};

const GROUP: &str = "grp_00000000000000000000000000000001";

async fn template(db: &TestDatabase) -> ReloginTemplate {
    sqlx::query("insert into account_groups(id,name,color,enabled,created_at,updated_at) values($1,'Template group','#FFFFFFFF',true,now(),now())")
        .bind(GROUP).execute(&db.pool).await.unwrap();
    let template: ReloginTemplate = serde_json::from_value(json!({
        "id": "quality-template", "revision": 1,
        "config": {
            "name": "Excel remediation", "enabled": true, "concurrencyLimit": 7,
            "weight": 19, "groupIds": [GROUP], "outboundProxyId": null,
            "responsesUpstream": "excel", "excelModelsFollowGlobal": false,
            "excelModels": ["fixture-model"], "excelCacheCreationAsInput": false,
            "excelIgnoreEncryptedContent": true, "excel403Action": "pause_account",
            "egressMode": "unchanged"
        }
    }))
    .unwrap();
    sqlx::query(
        "insert into account_relogin_templates(id,revision,name,config) values($1,$2,$3,$4)",
    )
    .bind(&template.id)
    .bind(template.revision as i64)
    .bind(&template.config.name)
    .bind(serde_json::to_value(&template.config).unwrap())
    .execute(&db.pool)
    .await
    .unwrap();
    template
}

async fn save(store: &PgQualityOpsStore, template: &ReloginTemplate, threshold: u8) -> QualityRule {
    let mut config = config("acct_quality_a");
    config.failure_action = QualityFailureAction::ApplyAccountTemplate;
    config.failure_template = Some(template.clone());
    config.excel_failure_threshold = threshold;
    store
        .save(
            None,
            None,
            config,
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap()
}

async fn snapshot(db: &TestDatabase) -> Value {
    sqlx::query_scalar("select jsonb_build_object('account',to_jsonb(a),
        'groups',(select coalesce(jsonb_agg(g order by g.account_group_id),'[]') from account_group_accounts g where g.provider_account_id=a.id),
        'egress',(select to_jsonb(o) from provider_egress_account_overrides o where o.provider_account_id=a.id)) from provider_accounts a where a.id='acct_quality_a'")
        .fetch_one(&db.pool).await.unwrap()
}

#[tokio::test]
async fn template_model_access_preserves_omission_and_applies_explicit_policy() {
    for policy in [
        None,
        Some(json!({"mode":"all","models":[]})),
        Some(json!({"mode":"allowlist","models":["fixture-model","model-a"]})),
        Some(json!({"mode":"denylist","models":["model-a"]})),
    ] {
        let Some(db) = TestDatabase::create("quality_template_model_access").await else {
            return;
        };
        let store = setup(&db).await;
        let original = json!({"mode":"denylist","models":["model-original"]});
        sqlx::query("update provider_accounts set model_access_json=$1 where id='acct_quality_a'")
            .bind(&original)
            .execute(&db.pool)
            .await
            .unwrap();
        let mut authoritative = template(&db).await;
        authoritative.config.model_access = policy
            .clone()
            .map(|value| serde_json::from_value(value).unwrap());
        sqlx::query("update account_relogin_templates set config=$1 where id=$2")
            .bind(serde_json::to_value(&authoritative.config).unwrap())
            .bind(&authoritative.id)
            .execute(&db.pool)
            .await
            .unwrap();
        let before = snapshot(&db).await;
        let rule = save(&store, &authoritative, 1).await;
        assert_eq!(
            snapshot(&db).await,
            before,
            "saving a rule does not mutate accounts"
        );
        let run = scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
        assert_eq!(run.action.as_deref(), Some("template_applied"));
        let after = snapshot(&db).await;
        assert_eq!(
            after["account"]["model_access_json"],
            policy.unwrap_or(original)
        );
        for field in [
            "provider_credentials_json",
            "credential_revision",
            "upstream_user_id",
            "upstream_account_id",
        ] {
            assert_eq!(
                after["account"][field], before["account"][field],
                "preserve {field}"
            );
        }
        db.close().await;
    }
}

#[tokio::test]
async fn quality_template_preserves_account_outbound_when_requested() {
    let Some(db) = TestDatabase::create("quality_template_preserve_proxy").await else {
        return;
    };
    let store = setup(&db).await;
    sqlx::query("update provider_accounts set outbound_proxy_url='http://proxy.example:8080' where id='acct_quality_a'")
        .execute(&db.pool).await.unwrap();
    let mut authoritative = template(&db).await;
    authoritative.config.preserve_outbound_proxy = true;
    authoritative.config.egress_mode = None;
    sqlx::query("update account_relogin_templates set config=$1 where id=$2")
        .bind(serde_json::to_value(&authoritative.config).unwrap())
        .bind(&authoritative.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let before = snapshot(&db).await;
    let rule = save(&store, &authoritative, 1).await;
    let run = scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
    assert_eq!(run.action.as_deref(), Some("template_applied"));
    let after = snapshot(&db).await;
    for field in [
        "outbound_proxy_url",
        "outbound_proxy_id",
        "request_proxy_source",
        "provider_credentials_json",
        "credential_revision",
    ] {
        assert_eq!(
            after["account"][field], before["account"][field],
            "must preserve {field}"
        );
    }
    assert_eq!(after["egress"], before["egress"]);
    db.close().await;
}

#[tokio::test]
async fn template_threshold_snapshot_and_complete_application_are_atomic() {
    use QualityVerdict::{Correct, Incorrect, RequestError, Unknown};
    let Some(db) = TestDatabase::create("quality_template_apply").await else {
        return;
    };
    let store = setup(&db).await;
    let mut authoritative = template(&db).await;
    authoritative.config.request_proxy_source =
        Some(gateway_core::account::RequestProxySource::Mihomo);
    sqlx::query("update account_relogin_templates set config=$1 where id=$2")
        .bind(serde_json::to_value(&authoritative.config).unwrap())
        .bind(&authoritative.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let mut forged = authoritative.clone();
    forged.config.enabled = false;
    forged.config.weight = 90;
    let before_save = snapshot(&db).await;
    let rule = save(&store, &forged, 2).await;
    assert_eq!(rule.config.failure_template.as_ref(), Some(&authoritative));
    let before = snapshot(&db).await;
    assert_eq!(before["account"], before_save["account"]);
    assert!(
        rule.next_run_at <= Utc::now(),
        "new monitoring is immediately due"
    );
    for verdict in [RequestError, Unknown] {
        assert!(
            scheduled_round(&store, &rule, &[verdict])
                .await
                .action
                .is_none()
        );
    }
    assert_eq!(
        scheduled_round(&store, &rule, &[Incorrect])
            .await
            .action
            .as_deref(),
        Some("excel_threshold_pending")
    );
    assert_eq!(snapshot(&db).await, before);
    assert_eq!(
        scheduled_round(&store, &rule, &[Correct])
            .await
            .action
            .as_deref(),
        Some("excel_streak_reset")
    );
    scheduled_round(&store, &rule, &[Incorrect]).await;
    store
        .enqueue(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    store
        .finish(
            &claim,
            Utc::now() + Duration::hours(6),
            vec![answer(Incorrect)],
        )
        .await
        .unwrap();
    let run = store.detail(&claim.run_id).await.unwrap().unwrap();
    assert_eq!(run.action.as_deref(), Some("template_applied"));
    assert_eq!(
        run.config.unwrap().failure_template.as_ref(),
        Some(&authoritative)
    );
    let applied = snapshot(&db).await;
    let account = &applied["account"];
    assert_eq!(account["enabled"], true);
    assert_eq!(account["concurrency_limit"], 7);
    assert_eq!(account["weight"], 19);
    assert_eq!(account["responses_upstream"], "excel");
    assert_eq!(account["excel_models"], json!(["fixture-model"]));
    assert_eq!(account["excel_models_follow_global"], false);
    assert_eq!(account["excel_cache_creation_as_input"], false);
    assert_eq!(account["excel_ignore_encrypted_content"], true);
    assert_eq!(account["request_proxy_source"], "mihomo");
    assert_eq!(account["excel_403_action"], "pause_account");
    assert_eq!(applied["groups"][0]["account_group_id"], GROUP);
    assert_eq!(applied["egress"]["mode"], "unchanged");
    assert_eq!(
        account["provider_credentials_json"],
        before["account"]["provider_credentials_json"]
    );
    assert_eq!(
        account["credential_revision"],
        before["account"]["credential_revision"]
    );
    assert!(
        store.rules().await.unwrap()[0].config.enabled,
        "answer rule stays enabled"
    );
    assert_eq!(store.rules().await.unwrap()[0].excel_failure_streak, 0);
    store
        .finish(&claim, Utc::now(), vec![answer(Incorrect)])
        .await
        .unwrap();
    assert_eq!(
        snapshot(&db).await,
        applied,
        "duplicate finish must not republish"
    );
    let audits: i64 = sqlx::query_scalar("select count(*) from admin_audit_events where action='quality_rule.apply_account_template'")
        .fetch_one(&db.pool).await.unwrap();
    assert_eq!(audits, 1);
    db.close().await;
}

#[tokio::test]
async fn stale_templates_and_unavailable_accounts_never_partially_apply() {
    let Some(db) = TestDatabase::create("quality_template_guards").await else {
        return;
    };
    let store = setup(&db).await;
    let template = template(&db).await;
    let mut rule = save(&store, &template, 1).await;
    let cases = [
        (
            "credential",
            "update provider_accounts set credential_state='expired' where id='acct_quality_a'",
            "template_blocked_account",
        ),
        (
            "quota",
            "update provider_accounts set quota_access_state='exhausted',quota_evidence='usage_limit_reached',quota_access_observed_at=now(),updated_at=now() where id='acct_quality_a'",
            "template_blocked_account",
        ),
        (
            "expiry",
            "update provider_accounts set access_token_expires_at=now()-interval '1 minute' where id='acct_quality_a'",
            "template_blocked_account",
        ),
        (
            "403 pause",
            "update provider_accounts set excel_auto_disabled_at=now() where id='acct_quality_a'",
            "template_blocked_account",
        ),
        (
            "403 mode",
            "update provider_accounts set excel_mode_disabled_at=now() where id='acct_quality_a'",
            "excel_blocked_configuration_changed",
        ),
        (
            "paused",
            "update provider_accounts set enabled=false where id='acct_quality_a'",
            "excel_blocked_configuration_changed",
        ),
        (
            "identity",
            "update provider_accounts set upstream_user_id='replaced' where id='acct_quality_a'",
            "excel_blocked_configuration_changed",
        ),
        (
            "template edit",
            "update account_relogin_templates set revision=2 where id='quality-template'",
            "template_unavailable",
        ),
        (
            "missing group",
            "delete from account_groups",
            "template_blocked_references",
        ),
    ];
    for (label, mutation, expected) in cases {
        store
            .enqueue(&rule.id, rule.revision, &context())
            .await
            .unwrap();
        let claim = store.claim().await.unwrap().unwrap();
        sqlx::query(mutation).execute(&db.pool).await.unwrap();
        let before = snapshot(&db).await;
        store
            .finish(
                &claim,
                Utc::now() + Duration::hours(6),
                vec![answer(QualityVerdict::Incorrect)],
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .detail(&claim.run_id)
                .await
                .unwrap()
                .unwrap()
                .action
                .as_deref(),
            Some(expected),
            "{label}"
        );
        assert_eq!(snapshot(&db).await, before, "{label}");
        sqlx::query("update provider_accounts set credential_state='ready',quota_access_state='unknown',quota_evidence=null,quota_access_observed_at=null,access_token_expires_at=null,excel_auto_disabled_at=null,excel_mode_disabled_at=null,enabled=true,upstream_user_id=id where id='acct_quality_a'")
            .execute(&db.pool).await.unwrap();
        sqlx::query("update account_relogin_templates set revision=1")
            .execute(&db.pool)
            .await
            .unwrap();
        sqlx::query("insert into account_groups(id,name,color,enabled,created_at,updated_at) values($1,'Template group','#FFFFFFFF',true,now(),now()) on conflict do nothing")
            .bind(GROUP).execute(&db.pool).await.unwrap();
        rule = store
            .save(
                Some(&rule.id),
                Some(rule.revision),
                rule.config.clone(),
                Utc::now() + Duration::hours(6),
                &context(),
            )
            .await
            .unwrap();
    }
    store
        .enqueue(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    sqlx::query("delete from account_relogin_templates")
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
    assert_eq!(
        store
            .detail(&claim.run_id)
            .await
            .unwrap()
            .unwrap()
            .action
            .as_deref(),
        Some("template_unavailable")
    );
    assert!(
        store
            .save(
                Some(&rule.id),
                Some(rule.revision),
                rule.config,
                Utc::now(),
                &context()
            )
            .await
            .is_err()
    );
    db.close().await;
}

#[tokio::test]
async fn template_controls_codex_and_scheduling_and_probe_lifecycle() {
    let Some(db) = TestDatabase::create("quality_template_modes").await else {
        return;
    };
    let store = setup(&db).await;
    let mut template = template(&db).await;
    let mut config = config("acct_quality_a");
    config.detection_mode = QualityDetectionMode::StateProbe;
    config.failure_action = QualityFailureAction::ApplyAccountTemplate;
    config.failure_template = Some(template.clone());
    let rule = store
        .save(
            None,
            None,
            config,
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[QualityVerdict::Incorrect])
            .await
            .action
            .as_deref(),
        Some("template_applied")
    );
    let paused = store.rules().await.unwrap().remove(0);
    assert!(paused.config.enabled);
    assert!(!paused.pending);
    assert!(!paused.running);
    template.config.responses_upstream = Some(gateway_core::account::ResponsesUpstream::Codex);
    template.config.enabled = false;
    template.revision = 2;
    sqlx::query("update account_relogin_templates set revision=2,config=$1")
        .bind(serde_json::to_value(&template.config).unwrap())
        .execute(&db.pool)
        .await
        .unwrap();
    let mut config = paused.config.clone();
    config.enabled = true;
    config.detection_mode = QualityDetectionMode::Answer;
    config.failure_template = Some(template);
    let rule = store
        .save(
            Some(&paused.id),
            Some(paused.revision),
            config,
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[QualityVerdict::Incorrect])
            .await
            .action
            .as_deref(),
        Some("template_applied")
    );
    let account = snapshot(&db).await;
    assert_eq!(account["account"]["responses_upstream"], "codex");
    assert_eq!(account["account"]["excel_ignore_encrypted_content"], false);
    assert_eq!(account["account"]["excel_403_action"], "none");
    assert_eq!(account["account"]["enabled"], false);
    db.close().await;
}

#[tokio::test]
async fn template_recovery_restores_complete_configuration_and_preserves_credentials() {
    for mode in [
        QualityDetectionMode::Answer,
        QualityDetectionMode::StateProbe,
    ] {
        let Some(db) = TestDatabase::create("quality_template_restore").await else {
            return;
        };
        let store = setup(&db).await;
        let mut template = template(&db).await;
        template.config.enabled = false;
        template.config.excel_recovery =
            Some(gateway_admin::model::excel_recovery::ExcelRecoveryConfig {
                enabled: true,
                interval_minutes: 10,
            });
        sqlx::query("update account_relogin_templates set config=$1")
            .bind(serde_json::to_value(&template.config).unwrap())
            .execute(&db.pool)
            .await
            .unwrap();
        let before = snapshot(&db).await;
        let mut config = config("acct_quality_a");
        config.detection_mode = mode;
        config.failure_action = QualityFailureAction::ApplyAccountTemplate;
        config.failure_template = Some(template);
        config.auto_restore = true;
        let rule = store
            .save(None, None, config, Utc::now(), &context())
            .await
            .unwrap();
        assert_eq!(
            scheduled_round(&store, &rule, &[QualityVerdict::Incorrect])
                .await
                .action
                .as_deref(),
            Some("template_applied")
        );
        let recovery: (bool, i32) = sqlx::query_as("select enabled,interval_minutes from account_excel_recovery where account_id='acct_quality_a'").fetch_one(&db.pool).await.unwrap();
        assert_eq!(recovery, (true, 10));
        assert_eq!(snapshot(&db).await["account"]["enabled"], false);
        assert!(
            store.rules().await.unwrap()[0].config.enabled,
            "keep probing paused account"
        );
        assert_eq!(
            scheduled_round(&store, &rule, &[QualityVerdict::Incorrect])
                .await
                .action
                .as_deref(),
            Some("already_applied")
        );
        for verdict in [QualityVerdict::Unknown, QualityVerdict::RequestError] {
            assert!(
                scheduled_round(&store, &rule, &[verdict])
                    .await
                    .action
                    .is_none()
            );
            assert_eq!(snapshot(&db).await["account"]["enabled"], false);
        }
        // A credential refresh after the template must survive configuration recovery.
        sqlx::query("update provider_accounts set credential_revision=credential_revision+1 where id='acct_quality_a'").execute(&db.pool).await.unwrap();
        assert_eq!(
            scheduled_round(&store, &rule, &[QualityVerdict::Correct])
                .await
                .action
                .as_deref(),
            Some("template_restored")
        );
        let mut restored = snapshot(&db).await;
        assert_eq!(
            restored["account"]["credential_revision"].as_i64(),
            before["account"]["credential_revision"]
                .as_i64()
                .map(|v| v + 1)
        );
        restored["account"]["credential_revision"] =
            before["account"]["credential_revision"].clone();
        restored["account"]["updated_at"] = before["account"]["updated_at"].clone();
        assert_eq!(restored, before);
        let count: i64 = sqlx::query_scalar(
            "select count(*) from account_excel_recovery where account_id='acct_quality_a'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
        assert!(
            scheduled_round(&store, &rule, &[QualityVerdict::Correct])
                .await
                .action
                .is_none()
        );
        db.close().await;
    }
}

#[tokio::test]
async fn template_recovery_is_opt_in_and_ignores_legacy_excel_only_recovery() {
    let Some(db) = TestDatabase::create("quality_template_no_restore").await else {
        return;
    };
    let store = setup(&db).await;
    let template = template(&db).await;
    let mut config = config("acct_quality_a");
    config.detection_mode = QualityDetectionMode::StateProbe;
    config.failure_action = QualityFailureAction::ApplyAccountTemplate;
    config.failure_template = Some(template);
    config.disable_excel_on_native_recovery = true; // persisted legacy config
    let rule = store
        .save(None, None, config, Utc::now(), &context())
        .await
        .unwrap();
    scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
    let applied = snapshot(&db).await;
    assert_eq!(
        scheduled_round(&store, &rule, &[QualityVerdict::Correct])
            .await
            .action
            .as_deref(),
        Some("already_applied")
    );
    assert_eq!(snapshot(&db).await, applied);
    db.close().await;
}

#[tokio::test]
async fn template_recovery_does_not_override_manual_edits_or_independent_protection() {
    for (label, change, expected) in [
        (
            "manual_same_value",
            "insert into admin_audit_events(id,actor_kind,actor_ref,action,entity_kind,entity_ref,config_revision,changed_fields,created_at) values('manual-same-value','system','system','provider_account.update','provider_account','acct_quality_a',999,array['responses_upstream'],now())",
            "template_recovery_released",
        ),
        (
            "edit",
            "update provider_accounts set weight=31 where id='acct_quality_a'",
            "template_recovery_released",
        ),
        (
            "quota",
            "update provider_accounts set quota_access_state='exhausted',quota_evidence='usage_limit_reached',quota_access_observed_at=now(),updated_at=now() where id='acct_quality_a'",
            "template_restore_blocked",
        ),
        (
            "expired",
            "update provider_accounts set credential_state='expired' where id='acct_quality_a'",
            "template_restore_blocked",
        ),
        (
            "protection",
            "update provider_accounts set excel_auto_disabled_at=now() where id='acct_quality_a'",
            "template_restore_blocked",
        ),
        (
            "identity",
            "update provider_accounts set upstream_user_id='changed-user' where id='acct_quality_a'",
            "template_recovery_released",
        ),
    ] {
        let Some(db) = TestDatabase::create(label).await else {
            return;
        };
        let store = setup(&db).await;
        let template = template(&db).await;
        let mut rule = save(&store, &template, 1).await;
        rule.config.auto_restore = true;
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
        scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
        sqlx::query(sqlx::AssertSqlSafe(change))
            .execute(&db.pool)
            .await
            .unwrap();
        let before = snapshot(&db).await;
        assert_eq!(
            scheduled_round(&store, &rule, &[QualityVerdict::Correct])
                .await
                .action
                .as_deref(),
            Some(expected),
            "{label}"
        );
        assert_eq!(snapshot(&db).await, before, "{label}");
        db.close().await;
    }
}

#[tokio::test]
async fn proxy_conflicts_roll_back_all_template_changes_but_keep_the_result() {
    let Some(db) = TestDatabase::create("quality_template_rollback").await else {
        return;
    };
    let store = setup(&db).await;
    let mut template = template(&db).await;
    sqlx::query("insert into outbound_proxies(id,name,proxy_url,last_test_success) values('proxy-template','Template proxy','http://127.0.0.1:18080',true)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("insert into provider_egress_account_overrides(provider_account_id,mode) values('acct_quality_a','random_ipv6_reuse')")
        .execute(&db.pool).await.unwrap();
    template.config.outbound_proxy_id = Some("proxy-template".into());
    template.config.egress_mode = None;
    sqlx::query("update account_relogin_templates set config=$1")
        .bind(serde_json::to_value(&template.config).unwrap())
        .execute(&db.pool)
        .await
        .unwrap();
    let rule = save(&store, &template, 1).await;
    let before = snapshot(&db).await;
    let egress_revision: i64 =
        sqlx::query_scalar("select revision from provider_egress_settings where id=1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let run = scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
    assert_eq!(run.status, "incorrect");
    assert_eq!(run.action.as_deref(), Some("template_blocked_settings"));
    assert_eq!(
        snapshot(&db).await,
        before,
        "Excel, groups, proxy, weight and revision must all roll back"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select revision from provider_egress_settings where id=1")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        egress_revision
    );
    sqlx::query("update outbound_proxies set last_test_success=false where id='proxy-template'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[QualityVerdict::Incorrect])
            .await
            .action
            .as_deref(),
        Some("template_blocked_references")
    );
    assert_eq!(snapshot(&db).await, before);
    sqlx::query("update outbound_proxies set last_test_success=true where id='proxy-template'")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("delete from provider_egress_account_overrides")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[QualityVerdict::Incorrect])
            .await
            .action
            .as_deref(),
        Some("template_applied")
    );
    assert_eq!(
        snapshot(&db).await["account"]["outbound_proxy_id"],
        "proxy-template"
    );
    db.close().await;
}

#[tokio::test]
async fn template_egress_modes_keep_preserve_inherit_and_explicit_semantics() {
    use gateway_core::provider_ports::egress::EgressMode;
    let Some(db) = TestDatabase::create("quality_template_egress").await else {
        return;
    };
    let store = setup(&db).await;
    let mut template = template(&db).await;
    let mut rule = save(&store, &template, 1).await;
    let modes = [
        (None, Some("random_ipv6_reuse")),
        (Some(None), None),
        (Some(Some(EgressMode::Unchanged)), Some("unchanged")),
        (
            Some(Some(EgressMode::FixedIpv6Reuse)),
            Some("fixed_ipv6_reuse"),
        ),
        (
            Some(Some(EgressMode::RandomIpv6Reuse)),
            Some("random_ipv6_reuse"),
        ),
        (
            Some(Some(EgressMode::FixedIpv6Fresh)),
            Some("fixed_ipv6_fresh"),
        ),
        (
            Some(Some(EgressMode::RandomIpv6Fresh)),
            Some("random_ipv6_fresh"),
        ),
    ];
    for (mode, expected) in modes {
        sqlx::query("insert into provider_egress_account_overrides(provider_account_id,mode) values('acct_quality_a','random_ipv6_reuse') on conflict(provider_account_id) do update set mode=excluded.mode")
            .execute(&db.pool).await.unwrap();
        template.revision += 1;
        template.config.egress_mode = mode;
        sqlx::query("update account_relogin_templates set config=$1,revision=$2")
            .bind(serde_json::to_value(&template.config).unwrap())
            .bind(template.revision as i64)
            .execute(&db.pool)
            .await
            .unwrap();
        let mut config = rule.config.clone();
        config.failure_template = Some(template.clone());
        rule = store
            .save(
                Some(&rule.id),
                Some(rule.revision),
                config,
                Utc::now() + Duration::hours(6),
                &context(),
            )
            .await
            .unwrap();
        assert_eq!(
            scheduled_round(&store, &rule, &[QualityVerdict::Incorrect])
                .await
                .action
                .as_deref(),
            Some("template_applied")
        );
        let value: Option<String> = sqlx::query_scalar("select mode from provider_egress_account_overrides where provider_account_id='acct_quality_a'").fetch_optional(&db.pool).await.unwrap();
        assert_eq!(value.as_deref(), expected);
    }
    assert!(sqlx::query_scalar::<_, Option<String>>("select mode from provider_egress_account_overrides where provider_account_id='acct_quality_b'")
        .fetch_optional(&db.pool).await.unwrap().is_none(), "unrelated accounts are untouched");
    db.close().await;
}

#[tokio::test]
async fn template_vetoes_preexisting_403_and_stale_claims_but_allows_independent_actions() {
    let Some(db) = TestDatabase::create("quality_template_fences").await else {
        return;
    };
    let store = setup(&db).await;
    let template = template(&db).await;
    let rule = save(&store, &template, 1).await;
    sqlx::query(
        "update provider_accounts set excel_mode_disabled_at=now() where id='acct_quality_a'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let before = snapshot(&db).await;
    assert_eq!(
        scheduled_round(&store, &rule, &[QualityVerdict::Incorrect])
            .await
            .action
            .as_deref(),
        Some("excel_blocked_403")
    );
    assert_eq!(snapshot(&db).await, before);
    sqlx::query(
        "update provider_accounts set excel_mode_disabled_at=null where id='acct_quality_a'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    store
        .enqueue(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    let rule = store
        .save(
            Some(&rule.id),
            Some(rule.revision),
            rule.config.clone(),
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    let before = snapshot(&db).await;
    store
        .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    assert_eq!(snapshot(&db).await, before);
    assert_eq!(
        store.detail(&claim.run_id).await.unwrap().unwrap().status,
        "cancelled"
    );
    let mut other = rule.config.clone();
    other.account_id = "acct_quality_b".into();
    let other = store
        .save(
            None,
            None,
            other,
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    for rule in [&rule, &other] {
        store
            .enqueue(&rule.id, rule.revision, &context())
            .await
            .unwrap();
    }
    let first = store.claim().await.unwrap().unwrap();
    let second = store.claim().await.unwrap().unwrap();
    let results = futures::future::join_all([&first, &second].map(|claim| {
        store.finish(
            claim,
            Utc::now() + Duration::hours(6),
            vec![answer(QualityVerdict::Incorrect)],
        )
    }))
    .await;
    for result in results {
        result.unwrap();
    }
    for claim in [first, second] {
        assert_eq!(
            store
                .detail(&claim.run_id)
                .await
                .unwrap()
                .unwrap()
                .action
                .as_deref(),
            Some("template_applied")
        );
    }
    db.close().await;
}

#[tokio::test]
async fn template_recovery_waits_for_missing_references_and_retains_existing_recovery_settings() {
    let Some(db) = TestDatabase::create("quality_restore_refs").await else {
        return;
    };
    let store = setup(&db).await;
    sqlx::query("insert into account_groups(id,name,color,enabled,created_at,updated_at) values('grp_00000000000000000000000000000002','Original group','#FFFFFFFF',true,now(),now())")
        .execute(&db.pool).await.unwrap();
    sqlx::query("insert into account_group_accounts(provider_account_id,account_group_id,created_at) values('acct_quality_a','grp_00000000000000000000000000000002',now())")
        .execute(&db.pool).await.unwrap();
    sqlx::query("update provider_accounts set outbound_proxy_url='http://proxy.example:8080' where id='acct_quality_a'")
        .execute(&db.pool).await.unwrap();
    sqlx::query("insert into account_excel_recovery(account_id,enabled,interval_minutes,next_probe_at) values('acct_quality_a',false,17,now())")
        .execute(&db.pool).await.unwrap();
    let before = snapshot(&db).await;
    let mut template = template(&db).await;
    template.config.excel_recovery =
        Some(gateway_admin::model::excel_recovery::ExcelRecoveryConfig {
            enabled: true,
            interval_minutes: 10,
        });
    sqlx::query("update account_relogin_templates set config=$1")
        .bind(serde_json::to_value(&template.config).unwrap())
        .execute(&db.pool)
        .await
        .unwrap();
    let mut rule = save(&store, &template, 1).await;
    rule.config.auto_restore = true;
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
    scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
    let applied = snapshot(&db).await;
    sqlx::query("delete from account_groups where id='grp_00000000000000000000000000000002'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[QualityVerdict::Correct])
            .await
            .action
            .as_deref(),
        Some("template_restore_blocked")
    );
    assert_eq!(snapshot(&db).await, applied);
    sqlx::query("insert into account_groups(id,name,color,enabled,created_at,updated_at) values('grp_00000000000000000000000000000002','Original group','#FFFFFFFF',true,now(),now())")
        .execute(&db.pool).await.unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[QualityVerdict::Correct])
            .await
            .action
            .as_deref(),
        Some("template_restored")
    );
    let mut restored = snapshot(&db).await;
    restored["account"]["updated_at"] = before["account"]["updated_at"].clone();
    assert_eq!(restored, before);
    let recovered: (bool, i32, i64) = sqlx::query_as("select enabled,interval_minutes,generation from account_excel_recovery where account_id='acct_quality_a'")
        .fetch_one(&db.pool).await.unwrap();
    assert_eq!((recovered.0, recovered.1), (false, 17));
    assert!(
        recovered.2 > 1,
        "invalidate probes claimed before configuration restoration"
    );
    db.close().await;
}

#[tokio::test]
async fn template_recovery_is_fenced_by_rule_changes_and_edits_during_a_probe() {
    for during_probe in [false, true] {
        let Some(db) = TestDatabase::create("quality_restore_fence").await else {
            return;
        };
        let store = setup(&db).await;
        let template = template(&db).await;
        let mut rule = save(&store, &template, 1).await;
        rule.config.auto_restore = true;
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
        scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
        store
            .enqueue(&rule.id, rule.revision, &context())
            .await
            .unwrap();
        let claim = store.claim().await.unwrap().unwrap();
        if during_probe {
            sqlx::query("update provider_accounts set enabled=false where id='acct_quality_a'")
                .execute(&db.pool)
                .await
                .unwrap();
        } else {
            rule.config.model = "changed-model".into();
            // Drop the remediation action so the replacement model needs no Excel validation.
            rule.config.failure_action = QualityFailureAction::None;
            rule.config.failure_template = None;
            store
                .save(
                    Some(&rule.id),
                    Some(rule.revision),
                    rule.config.clone(),
                    Utc::now(),
                    &context(),
                )
                .await
                .unwrap();
        }
        let before = snapshot(&db).await;
        store
            .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Correct)])
            .await
            .unwrap();
        assert_eq!(
            snapshot(&db).await,
            before,
            "stale probe cannot restore configuration"
        );
        db.close().await;
    }
}

#[tokio::test]
async fn template_overload_triggers_after_two_rounds_in_both_modes_and_restores() {
    use QualityVerdict::{Correct, Incorrect, Overloaded, RequestError, Unknown};
    for mode in [
        QualityDetectionMode::Answer,
        QualityDetectionMode::StateProbe,
    ] {
        for second in [Overloaded, Incorrect] {
            let Some(db) = TestDatabase::create("quality_overload_template").await else {
                return;
            };
            let store = setup(&db).await;
            let template = template(&db).await;
            let mut cfg = config("acct_quality_a");
            cfg.detection_mode = mode;
            cfg.failure_action = QualityFailureAction::ApplyAccountTemplate;
            cfg.failure_template = Some(template);
            cfg.excel_failure_threshold = 5;
            cfg.auto_restore = true;
            let rule = store
                .save(None, None, cfg, Utc::now(), &context())
                .await
                .unwrap();
            // Unknown errors hold evidence; a healthy round starts a new sequence.
            for verdict in [Overloaded, Unknown, RequestError, Correct, Overloaded] {
                let run = scheduled_round(&store, &rule, &[verdict]).await;
                assert!(run.action.is_none());
                assert_eq!(snapshot(&db).await["account"]["weight"], 1);
            }
            let restarted = PgQualityOpsStore::new(db.pool.clone());
            let run = scheduled_round(&restarted, &rule, &[second]).await;
            assert_eq!(run.status, "incorrect");
            assert_eq!(run.action.as_deref(), Some("template_applied"));
            assert_eq!(snapshot(&db).await["account"]["weight"], 19);
            // Overload does not restore settings; an entirely healthy round does.
            scheduled_round(&restarted, &rule, &[Overloaded]).await;
            assert_eq!(snapshot(&db).await["account"]["weight"], 19);
            let restored = scheduled_round(&restarted, &rule, &[Correct]).await;
            assert_eq!(restored.action.as_deref(), Some("template_restored"));
            assert_eq!(snapshot(&db).await["account"]["weight"], 1);
            db.close().await;
        }
    }
}

#[tokio::test]
async fn template_overload_is_one_round_and_cannot_bypass_account_protection() {
    use QualityVerdict::{Incorrect, Overloaded};
    let Some(db) = TestDatabase::create("quality_overload_protection").await else {
        return;
    };
    let store = setup(&db).await;
    let template = template(&db).await;
    let mut cfg = config("acct_quality_a");
    cfg.repetitions = 3;
    cfg.failure_action = QualityFailureAction::ApplyAccountTemplate;
    cfg.failure_template = Some(template);
    cfg.excel_failure_threshold = 1;
    let rule = store
        .save(None, None, cfg, Utc::now(), &context())
        .await
        .unwrap();
    let first = scheduled_round(&store, &rule, &[Overloaded, Overloaded, Overloaded]).await;
    assert_eq!(first.status, "overloaded");
    assert_eq!(first.request_errors, 3);
    assert!(first.action.is_none());
    assert_eq!(snapshot(&db).await["account"]["weight"], 1);
    sqlx::query(
        "update provider_accounts set quota_access_state='exhausted',quota_evidence='usage_limit_reached',
         quota_access_observed_at=now(),updated_at=now() where id='acct_quality_a'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let second = scheduled_round(&store, &rule, &[Overloaded, Overloaded, Incorrect]).await;
    assert_eq!(second.status, "incorrect");
    assert_eq!(second.action.as_deref(), Some("template_blocked_account"));
    assert_eq!(snapshot(&db).await["account"]["weight"], 1);
    db.close().await;
}
