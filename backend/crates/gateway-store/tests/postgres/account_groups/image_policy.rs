use super::*;
use gateway_admin::model::account_groups::UpdateAccountGroup;

#[tokio::test]
async fn image_policy_upgrade_preserves_saved_concurrency_and_existing_groups() {
    let old = sqlx::migrate::Migrator {
        migrations: super::super::TEST_MIGRATOR
            .iter()
            .filter(|migration| migration.version < 73)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    for (revision, saved, expected) in [(1_i64, 3_i64, 3_i64), (2, 3, 3), (1, 9, 9)] {
        let Some(db) = TestDatabase::create_with_migrator("image_upgrade", &old).await else {
            return;
        };
        sqlx::query("update runtime_settings set config_revision=$1, max_concurrent_per_account=$2 where id=1")
            .bind(revision).bind(saved).execute(&db.pool).await.unwrap();
        sqlx::query("insert into account_groups(id,name,color,created_at,updated_at) values ('grp_00000000000000000000000000000001','Existing','#2563EBFF',now(),now())")
            .execute(&db.pool)
            .await
            .unwrap();
        super::super::TEST_MIGRATOR.run(&db.pool).await.unwrap();
        super::super::TEST_MIGRATOR.run(&db.pool).await.unwrap();
        let actual: i64 = sqlx::query_scalar(
            "select max_concurrent_per_account from runtime_settings where id=1",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(actual, expected);
        let allowed: bool = sqlx::query_scalar(
            "select allow_image_generation from account_groups where id='grp_00000000000000000000000000000001'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(allowed);
        db.close().await;
    }
}

#[tokio::test]
async fn image_permission_roundtrips_and_legacy_update_preserves_it() {
    let Some(db) = TestDatabase::create("group_image_policy").await else {
        return;
    };
    let repository = PgAccountGroupRepository::new(db.pool.clone());
    let id = group_id(MIXED_GROUP);
    let created = repository
        .create_account_group(
            NewAccountGroup {
                allow_subagents: true,
                model_access: Default::default(),

                id: id.clone(),
                name: "Image policy".into(),
                description: None,
                color: group_color("#2563EBFF"),
                fast_mode: FastMode::Default,
                disable_fast: false,
                allow_image_generation: false,
            },
            &context("create-images"),
        )
        .await
        .unwrap();
    assert!(!created.record.unwrap().allow_image_generation);
    for (value, expected) in [
        (None, false),
        (Some(true), true),
        (None, true),
        (Some(false), false),
    ] {
        let updated = repository
            .update_account_group(
                UpdateAccountGroup {
                    allow_subagents: Default::default(),
                    model_access: Default::default(),

                    id: id.clone(),
                    name: "Image policy".into(),
                    description: None,
                    color: group_color("#2563EBFF"),
                    fast_mode: None,
                    disable_fast: None,
                    allow_image_generation: value,
                },
                &context("update-images"),
            )
            .await
            .unwrap();
        assert_eq!(updated.record.unwrap().allow_image_generation, expected);
    }
    let initial: i64 =
        sqlx::query_scalar("select max_concurrent_per_account from runtime_settings where id=1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(initial, 3);
    db.close().await;
}

#[tokio::test]
async fn group_policies_preserve_omitted_values_and_publish_snapshot_facts() {
    use gateway_admin::model::account_groups::UpdateAccountGroup;
    use gateway_store::postgres::{PgRuntimeSnapshotRepository, RuntimeSnapshotRepository};
    let Some(database) = TestDatabase::create("image_generation_group").await else {
        return;
    };
    let repository = PgAccountGroupRepository::new(database.pool.clone());
    let id = group_id(MIXED_GROUP);
    let denylist = gateway_core::account::AccountModelAccess::new(
        gateway_core::account::AccountModelAccessMode::Denylist,
        vec!["model-a".to_owned()],
    )
    .unwrap();
    repository
        .create_account_group(
            NewAccountGroup {
                allow_image_generation: false,
                allow_subagents: false,
                model_access: denylist.clone(),
                id: id.clone(),
                name: "Fast policy".to_owned(),
                description: None,
                color: group_color("#2563EBFF"),
                fast_mode: FastMode::Ultrafast,
                disable_fast: false,
            },
            &context("create-fast"),
        )
        .await
        .unwrap();
    for (value, expected) in [
        (None, false),
        (Some(true), true),
        (None, true),
        (Some(false), false),
    ] {
        let mutation = repository
            .update_account_group(
                UpdateAccountGroup {
                    allow_image_generation: value,
                    allow_subagents: value,
                    model_access: value.map(|allowed| {
                        if allowed {
                            Default::default()
                        } else {
                            denylist.clone()
                        }
                    }),
                    id: id.clone(),
                    name: "Renamed policy".to_owned(),
                    description: None,
                    color: group_color("#2563EBFF"),
                    fast_mode: None,
                    disable_fast: None,
                },
                &context("update-fast"),
            )
            .await
            .unwrap();
        let record = mutation.record.unwrap();
        assert_eq!(record.allow_image_generation, expected);
        assert_eq!(record.allow_subagents, expected);
        let expected_models = if expected {
            Default::default()
        } else {
            denylist.clone()
        };
        assert_eq!(record.model_access, expected_models);
        let snapshot = PgRuntimeSnapshotRepository::new(database.pool.clone())
            .load_runtime_snapshot()
            .await
            .unwrap();
        assert_eq!(
            snapshot.config_revision.get(),
            mutation.config_revision.get()
        );
        let group = snapshot
            .account_groups
            .iter()
            .find(|group| group.id == id)
            .unwrap();
        assert_eq!(group.allow_image_generation, expected);
        assert_eq!(group.allow_subagents, expected);
        assert_eq!(group.model_access, expected_models);
    }
    database.close().await;
}

#[tokio::test]
async fn group_policy_defaults_apply_to_groups_created_without_explicit_policies() {
    let Some(database) = TestDatabase::create("group_image_default").await else {
        return;
    };
    sqlx::query("insert into account_groups (id, name, color, created_at, updated_at) values ($1, $1, '#2563EBFF', now(), now())")
        .bind(EMPTY_GROUP).execute(&database.pool).await.unwrap();
    let (images, subagents, models) = sqlx::query_as::<_, (bool, bool, sqlx::types::Json<gateway_core::account::AccountModelAccess>)>(
        "select allow_image_generation, allow_subagents, model_access_json from account_groups where id = $1",
    )
    .bind(EMPTY_GROUP)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(images);
    assert!(subagents);
    assert_eq!(models.0, gateway_core::account::AccountModelAccess::all());
    database.close().await;
}

#[tokio::test]
async fn v3540_upgrade_preserves_existing_settings_groups_keys_and_budget() {
    let old = sqlx::migrate::Migrator {
        migrations: super::super::TEST_MIGRATOR
            .iter()
            .filter(|m| m.version <= 97)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let Some(db) = TestDatabase::create_with_migrator("v3540_upgrade", &old).await else {
        return;
    };
    sqlx::query("insert into account_groups(id,name,color,fast_mode,disable_fast,created_at,updated_at) values ($1,'Existing','#2563EBFF','disabled',true,now(),now())")
        .bind(MIXED_GROUP).execute(&db.pool).await.unwrap();
    seed_account(&db.pool, "acct_existing", "openai", "Existing account").await;
    assign_accounts(&db.pool, MIXED_GROUP, &["acct_existing"]).await;
    sqlx::query("insert into client_api_keys(id,name,key,created_at,updated_at) values('key_existing','Existing key',$1,now(),now())")
        .bind(format!("sk_{}", "q".repeat(43))).execute(&db.pool).await.unwrap();
    sqlx::query("insert into client_api_key_groups(client_api_key_id,account_group_id,created_at) values('key_existing',$1,now())")
        .bind(MIXED_GROUP).execute(&db.pool).await.unwrap();
    sqlx::query("insert into client_key_budget_windows(client_api_key_id,daily_start,daily_end,weekly_start,weekly_end,daily_used_usd,weekly_used_usd) values('key_existing',now(),now()+interval '1 day',now(),now()+interval '7 days',2.5,9.5)")
        .execute(&db.pool).await.unwrap();
    let tables = [
        "account_groups",
        "runtime_settings",
        "provider_accounts",
        "client_api_keys",
        "client_api_key_groups",
        "account_group_accounts",
        "client_key_budget_windows",
    ];
    let mut before = Vec::<serde_json::Value>::new();
    for table in tables {
        before.push(
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "select coalesce(jsonb_agg(to_jsonb(t)),'[]') from {table} t"
            )))
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        );
    }
    super::super::TEST_MIGRATOR.run(&db.pool).await.unwrap();
    super::super::TEST_MIGRATOR.run(&db.pool).await.unwrap();
    for (table, expected) in tables.into_iter().zip(before) {
        let remove = match table {
            "account_groups" => " - 'allow_subagents' - 'model_access_json'",
            "runtime_settings" => {
                " - 'openai_oauth_guardian_review_model' - 'openai_request_country_json'"
            }
            "client_key_budget_windows" => " - 'revision' - 'last_operation_request'",
            _ => "",
        };
        let actual: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select coalesce(jsonb_agg(to_jsonb(t){remove}),'[]') from {table} t"
        )))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(actual, expected, "existing data in {table}");
    }
    let defaults: (bool, serde_json::Value, bool, serde_json::Value) = sqlx::query_as("select g.allow_subagents,g.model_access_json,s.openai_oauth_guardian_review_model,s.openai_request_country_json from account_groups g cross join runtime_settings s")
        .fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        defaults,
        (
            true,
            serde_json::json!({"mode":"all","models":[]}),
            true,
            serde_json::json!({"mode":"follow_location"})
        )
    );
    db.close().await;
}
