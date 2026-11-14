use super::*;

#[tokio::test]
async fn native_quality_recovery_only_releases_legacy_ownership() {
    for has_excel in [false, true] {
        for restore_enabled in [false, true] {
            let Some(db) = TestDatabase::create("quality_release_legacy_excel").await else {
                return;
            };
            let store = setup(&db).await;
            sqlx::query(
                "update provider_accounts set responses_upstream='excel' where id='acct_quality_a'",
            )
            .execute(&db.pool)
            .await
            .unwrap();
            if has_excel {
                sqlx::query("insert into openai_excel_credentials(account_id,upstream_user_id,credential_json,credential_observed_at,suspended) values('acct_quality_a','acct_quality_a','{}',now(),true)")
                    .execute(&db.pool).await.unwrap();
            }
            let mut cfg = policy::probe_config("acct_quality_a");
            cfg.disable_excel_on_native_recovery = restore_enabled;
            let rule = store
                .save(None, None, cfg, Utc::now(), &context())
                .await
                .unwrap();
            sqlx::query("update quality_rules set recovery=$2 where id=$1")
                .bind(&rule.id)
                .bind(serde_json::json!({"excel_owner":{"legacy":true},"excel_pass_streak":1}))
                .execute(&db.pool)
                .await
                .unwrap();
            let run = scheduled_round(&store, &rule, &[QualityVerdict::Correct]).await;
            assert_eq!(run.action.as_deref(), Some("excel_recovery_released"));
            assert_eq!(policy::route(&db, "acct_quality_a").await, "excel");
            assert!(enabled(&db).await);
            let clean: bool = sqlx::query_scalar("select not (recovery ? 'excel_owner') and not (recovery ? 'excel_pass_streak') from quality_rules where id=$1")
                .bind(&rule.id).fetch_one(&db.pool).await.unwrap();
            assert!(clean);
            if has_excel {
                let suspended: bool = sqlx::query_scalar("select suspended from openai_excel_credentials where account_id='acct_quality_a'").fetch_one(&db.pool).await.unwrap();
                assert!(suspended);
            }
            db.close().await;
        }
    }
}
