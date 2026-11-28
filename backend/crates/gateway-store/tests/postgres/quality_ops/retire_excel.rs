use super::*;
use serde_json::{Value, json};

const RETIRED: &[&str] = &[
    "responsesUpstream",
    "excelModels",
    "excelModelsFollowGlobal",
    "excelCacheCreationAsInput",
    "excelIgnoreEncryptedContent",
    "excelAutoDisableOn403",
    "excel403Action",
    "excelRecovery",
];

async fn all_accounts(db: &TestDatabase) -> Value {
    sqlx::query_scalar("select jsonb_agg(to_jsonb(a) order by id) from provider_accounts a")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn retirement_migration_preserves_native_accounts_templates_and_references() {
    let old = sqlx::migrate::Migrator {
        migrations: super::super::TEST_MIGRATOR
            .iter()
            .filter(|m| m.version < 105)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let Some(db) = TestDatabase::create_with_migrator("retire_excel", &old).await else {
        return;
    };
    let store = setup(&db).await;
    let template = super::templates::template(&db).await;
    let rule = super::templates::save(&store, &template, 3).await;
    let clean_template = serde_json::to_value(&template.config).unwrap();
    let mut legacy_template = clean_template.clone();
    for key in RETIRED {
        legacy_template[*key] = json!("retired");
    }
    legacy_template["responsesUpstream"] = json!("excel");
    legacy_template["excelRecovery"] = json!({"enabled":true,"intervalMinutes":10});
    sqlx::query("update account_relogin_templates set config=$1 where id=$2")
        .bind(&legacy_template)
        .bind(&template.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let clean_quality = serde_json::to_value(&rule.config).unwrap();
    let mut legacy_quality = clean_quality.clone();
    legacy_quality["failureTemplate"]["config"] = legacy_template.clone();
    legacy_quality["disableExcelOnNativeRecovery"] = json!(true);
    legacy_quality["excelRecoveryThreshold"] = json!(4);
    let ownership = json!({"excel_streak":{"count":2,"scope":"ordinary","identity":[null,null]},"pause_owner":"ordinary"});
    sqlx::query("update quality_rules set config=$1,recovery=$2 where id=$3")
        .bind(&legacy_quality)
        .bind(&ownership)
        .bind(&rule.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let mut global_quality = legacy_quality.clone();
    global_quality["accountId"] = json!("");
    sqlx::query("insert into quality_rule_templates(id,name,revision,config) values('ordinary-quality','Ordinary',7,$1)")
        .bind(&global_quality).execute(&db.pool).await.unwrap();
    sqlx::query("insert into quality_group_rules(id,name,revision,filter,config) values('ordinary-group','Ordinary',8,'{}',$1)")
        .bind(&global_quality).execute(&db.pool).await.unwrap();
    let mut no_action = config("acct_quality_b");
    no_action.enabled = false;
    let retired_rule = store
        .save(None, None, no_action, Utc::now(), &context())
        .await
        .unwrap();
    sqlx::query("update quality_rules set config=jsonb_set(config,'{failureAction}','\"enable_excel\"'),recovery=$1 where id=$2")
        .bind(json!({"excel_owner":{},"excel_pass_streak":{},"excel_streak":{"count":1,"scope":[],"identity":[null,null]},"pause_owner":"keep"}))
        .bind(&retired_rule.id).execute(&db.pool).await.unwrap();
    let legacy_material = json!({"note":{"excelModels":"keep arbitrary user data"},
        "enrollment":{"id":template.id,"revision":1,"config":legacy_template},
        "enrollment_plan":{"enrollment":{"id":template.id,"revision":1,"config":legacy_template}}});
    sqlx::query("insert into account_relogin_entries(id,email,revision,material) values('retirement','retirement@example.com',9,$1)")
        .bind(&legacy_material).execute(&db.pool).await.unwrap();
    sqlx::query(
        "insert into account_excel_recovery(account_id,enabled) values('acct_quality_a',true)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "update excel_recovery_slots set lease_id='legacy',lease_until=now()+interval '1 hour'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("update runtime_settings set request_tuning_json=$1")
        .bind(json!({"maxAccountSwitches":7,"websocketMaxRetries":3,"excelImageTransport":{"mode":"native"},"excelImageRelayBytes":123}))
        .execute(&db.pool).await.unwrap();
    let before = all_accounts(&db).await;
    let run = scheduled_round(&store, &rule, &[QualityVerdict::Unknown]).await;
    // A pre-upgrade run keeps the settings in force at the time, independently
    // of what the live rule is allowed to do after the upgrade.
    let mut recorded = clean_quality.clone();
    recorded["disableExcelOnNativeRecovery"] = json!(true);
    recorded["excelRecoveryThreshold"] = json!(4);
    recorded["failureTemplate"]["config"]["responsesUpstream"] = json!("excel");
    recorded["failureTemplate"]["config"]["excelModels"] = json!(["model-a"]);
    recorded["failureTemplate"]["config"]["excelModelsFollowGlobal"] = json!(false);
    recorded["failureTemplate"]["config"]["excelCacheCreationAsInput"] = json!(true);
    recorded["failureTemplate"]["config"]["excelIgnoreEncryptedContent"] = json!(true);
    recorded["failureTemplate"]["config"]["excelAutoDisableOn403"] = json!(false);
    recorded["failureTemplate"]["config"]["excel403Action"] = json!("disable_excel");
    recorded["failureTemplate"]["config"]["excelRecovery"] =
        json!({"enabled":true,"intervalMinutes":10});
    sqlx::query("update quality_runs set config=$1 where id=$2")
        .bind(&recorded)
        .bind(&run.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let mut recorded_action = serde_json::to_value(&retired_rule.config).unwrap();
    recorded_action["failureAction"] = json!("enable_excel");
    sqlx::query("insert into quality_runs(id,rule_id,rule_revision,account_id,model,config,status)
        select 'retired-action-history',id,revision,account_id,config->>'model',$2,'correct' from quality_rules where id=$1")
        .bind(&retired_rule.id).bind(&recorded_action).execute(&db.pool).await.unwrap();
    let history: Value = sqlx::query_scalar("select to_jsonb(r) from quality_runs r where id=$1")
        .bind(&run.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    // Restore a valid shared threshold owner after the non-mutating evidence round.
    sqlx::query("update quality_rules set recovery=$1 where id=$2")
        .bind(&ownership)
        .bind(&rule.id)
        .execute(&db.pool)
        .await
        .unwrap();
    super::super::TEST_MIGRATOR.run(&db.pool).await.unwrap();
    super::super::TEST_MIGRATOR.run(&db.pool).await.unwrap();
    assert_eq!(
        all_accounts(&db).await,
        before,
        "account rows must remain byte-equivalent JSON"
    );
    let (id, revision, config): (String, i64, Value) =
        sqlx::query_as("select id,revision,config from account_relogin_templates")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        (id, revision, config),
        (template.id.clone(), 1, clean_template.clone())
    );
    let (revision, config, recovery): (i64, Value, Value) =
        sqlx::query_as("select revision,config,recovery from quality_rules where id=$1")
            .bind(&rule.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(revision, rule.revision);
    assert_eq!(config, clean_quality);
    assert_eq!(
        recovery, ownership,
        "shared quality ownership and streak survive"
    );
    let (action, recovery): (String, Value) =
        sqlx::query_as("select config->>'failureAction',recovery from quality_rules where id=$1")
            .bind(&retired_rule.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(action, "none");
    assert_eq!(recovery, json!({"pause_owner":"keep"}));
    let mut expected_global = clean_quality;
    expected_global["accountId"] = json!("");
    let (revision, config): (i64, Value) = sqlx::query_as(
        "select revision,config from quality_rule_templates where id='ordinary-quality'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!((revision, config), (7, expected_global.clone()));
    let (revision, config): (i64, Value) =
        sqlx::query_as("select revision,config from quality_group_rules where id='ordinary-group'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!((revision, config), (8, expected_global));
    let mut expected_material = legacy_material;
    expected_material["enrollment"]["config"] = clean_template.clone();
    expected_material["enrollment_plan"]["enrollment"]["config"] = clean_template;
    let (revision, material): (i64, Value) = sqlx::query_as(
        "select revision,material from account_relogin_entries where id='retirement'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!((revision, material), (9, expected_material));
    let tuning: Value = sqlx::query_scalar("select request_tuning_json from runtime_settings")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        tuning,
        json!({"maxAccountSwitches":7,"websocketMaxRetries":3})
    );
    let count: i64 = sqlx::query_scalar("select count(*) from account_excel_recovery")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let count:i64=sqlx::query_scalar("select count(*) from excel_recovery_slots where lease_id is not null or lease_until is not null")
        .fetch_one(&db.pool).await.unwrap();
    assert_eq!(count, 0);
    let count:i64=sqlx::query_scalar("select count(*) from pg_trigger where tgrelid='provider_accounts'::regclass and tgname='invalidate_excel_recovery'")
        .fetch_one(&db.pool).await.unwrap();
    assert_eq!(count, 0);
    let retained: Value = sqlx::query_scalar("select to_jsonb(r) from quality_runs r where id=$1")
        .bind(&run.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(retained, history);
    let historical = store
        .detail("retired-action-history")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(historical.config.unwrap()).unwrap(),
        recorded_action
    );
    let detail = store.detail(&run.id).await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(detail.config.unwrap()).unwrap(),
        recorded
    );
    assert!(
        store
            .runs(&rule.id)
            .await
            .unwrap()
            .iter()
            .all(|item| item.config.is_none())
    );
    let mut malformed = recorded;
    malformed["failureTemplate"]["config"]["excelRecovery"]["unexpected"] = json!(true);
    assert!(serde_json::from_value::<QualityRunConfig>(malformed).is_err());
    // The shared template reference remains usable after the migration.
    let current = store
        .rules()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == rule.id)
        .unwrap();
    store
        .save(
            Some(&current.id),
            Some(current.revision),
            current.config,
            Utc::now(),
            &context(),
        )
        .await
        .unwrap();
    db.close().await;
}

#[tokio::test]
async fn legacy_template_ownership_restores_native_settings_but_respects_manual_changes() {
    for manual_change in [false, true] {
        let Some(db) = TestDatabase::create("retire_owned_settings").await else {
            return;
        };
        let store = setup(&db).await;
        let template = super::templates::template(&db).await;
        let original = all_accounts(&db).await;
        let mut cfg = config("acct_quality_a");
        cfg.failure_action = QualityFailureAction::ApplyAccountTemplate;
        cfg.failure_template = Some(template);
        cfg.auto_restore = true;
        let rule = store
            .save(None, None, cfg, Utc::now(), &context())
            .await
            .unwrap();
        assert_eq!(
            scheduled_round(&store, &rule, &[QualityVerdict::Incorrect])
                .await
                .action
                .as_deref(),
            Some("template_applied")
        );
        let mut recovery: Value =
            sqlx::query_scalar("select recovery from quality_rules where id=$1")
                .bind(&rule.id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        for phase in ["before", "after"] {
            recovery["template_owner"][phase]["excel_recovery"] = json!({"enabled":true});
            recovery["template_owner"][phase]["account"]["responses_upstream"] = json!("excel");
            recovery["template_owner"][phase]["account"]["excel_models"] = json!(["retired"]);
        }
        sqlx::query("update quality_rules set recovery=$1 where id=$2")
            .bind(recovery)
            .bind(&rule.id)
            .execute(&db.pool)
            .await
            .unwrap();
        if manual_change {
            sqlx::query("update provider_accounts set weight=31 where id='acct_quality_a'")
                .execute(&db.pool)
                .await
                .unwrap();
        }
        let result = scheduled_round(&store, &rule, &[QualityVerdict::Correct]).await;
        assert_eq!(
            result.action.as_deref(),
            Some(if manual_change {
                "template_recovery_released"
            } else {
                "template_restored"
            })
        );
        let after = all_accounts(&db).await;
        assert_eq!(
            after[0]["weight"],
            if manual_change {
                json!(31)
            } else {
                original[0]["weight"].clone()
            }
        );
        for field in [
            "provider_credentials_json",
            "credential_revision",
            "upstream_user_id",
            "responses_upstream",
        ] {
            assert_eq!(after[0][field], original[0][field], "preserve {field}");
        }
        db.close().await;
    }
}
