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
async fn native_recovery_closes_template_owned_excel_without_reverting_other_settings() {
    let Some(db) = TestDatabase::create("quality_template_native_recovery").await else {
        return;
    };
    let store = setup(&db).await;
    let template = template(&db).await;
    let mut config = config("acct_quality_a");
    config.detection_mode = QualityDetectionMode::StateProbe;
    config.failure_action = QualityFailureAction::ApplyAccountTemplate;
    config.failure_template = Some(template);
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
    let applied = snapshot(&db).await;
    assert_eq!(applied["account"]["responses_upstream"], "excel");
    let run = scheduled_round(&store, &rule, &[QualityVerdict::Correct]).await;
    assert_eq!(
        run.action.as_deref(),
        Some("excel_disabled_native_recovered")
    );
    let mut restored = snapshot(&db).await;
    assert_eq!(restored["account"]["responses_upstream"], "codex");
    restored["account"]["responses_upstream"] = applied["account"]["responses_upstream"].clone();
    restored["account"]["updated_at"] = applied["account"]["updated_at"].clone();
    assert_eq!(restored, applied, "only route and timestamp may change");
    db.close().await;
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
