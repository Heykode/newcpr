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
    assert_eq!(initial, 5);
    db.close().await;
}
