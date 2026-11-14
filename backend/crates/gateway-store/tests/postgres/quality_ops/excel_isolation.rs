use super::*;

async fn excel_account(db: &TestDatabase) {
    sqlx::query("update provider_accounts set responses_upstream='excel',credential_state='expired',last_error_message='codex-fixture' where id='acct_quality_a'")
        .execute(&db.pool).await.unwrap();
    sqlx::query("insert into openai_excel_credentials(account_id,upstream_user_id,credential_json,credential_observed_at,access_token_expires_at)
        values('acct_quality_a','acct_quality_a','{}',now(),now()+interval '1 hour')")
        .execute(&db.pool).await.unwrap();
}

#[tokio::test]
async fn excel_quality_pause_and_restore_never_change_codex_or_shared_enable() {
    let Some(db) = TestDatabase::create("excel_quality_isolation").await else {
        return;
    };
    let store = setup(&db).await;
    excel_account(&db).await;
    let mut cfg = config("acct_quality_a");
    cfg.failure_action = QualityFailureAction::DisableScheduling;
    cfg.failure_threshold = Some(1);
    cfg.auto_restore = true;
    let rule = store
        .save(None, None, cfg, Utc::now(), &context())
        .await
        .unwrap();
    for (verdict, action, suspended) in [
        (QualityVerdict::Incorrect, "excel_scheduling_paused", true),
        (QualityVerdict::Correct, "excel_scheduling_restored", false),
    ] {
        let run = scheduled_round(&store, &rule, &[verdict]).await;
        assert_eq!(run.action.as_deref(), Some(action));
        let state: (bool, String, Option<String>, String, bool) = sqlx::query_as("select a.enabled,a.credential_state,a.last_error_message,a.responses_upstream,e.suspended from provider_accounts a join openai_excel_credentials e on e.account_id=a.id where a.id='acct_quality_a'")
            .fetch_one(&db.pool).await.unwrap();
        assert_eq!(
            state,
            (
                true,
                "expired".into(),
                Some("codex-fixture".into()),
                "excel".into(),
                suspended
            )
        );
    }
    scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
    sqlx::query("update provider_accounts set enabled=false where id='acct_quality_a'")
        .execute(&db.pool)
        .await
        .unwrap();
    scheduled_round(&store, &rule, &[QualityVerdict::Correct]).await;
    assert!(!enabled(&db).await);
    let suspended: bool = sqlx::query_scalar(
        "select suspended from openai_excel_credentials where account_id='acct_quality_a'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(suspended);
    db.close().await;
}

#[tokio::test]
async fn excel_quality_cannot_remove_shared_groups() {
    let Some(db) = TestDatabase::create("excel_quality_shared_groups").await else {
        return;
    };
    let store = setup(&db).await;
    excel_account(&db).await;
    let group = "grp_00000000000000000000000000000001";
    sqlx::query("insert into account_groups(id,name,color,enabled,created_at,updated_at) values($1,$1,'#FFFFFFFF',true,now(),now())").bind(group).execute(&db.pool).await.unwrap();
    sqlx::query("insert into account_group_accounts values($1,'acct_quality_a',now())")
        .bind(group)
        .execute(&db.pool)
        .await
        .unwrap();
    let mut cfg = config("acct_quality_a");
    cfg.failure_action = QualityFailureAction::RemoveGroups;
    cfg.failure_group_ids = vec![group.into()];
    let rule = store
        .save(None, None, cfg, Utc::now(), &context())
        .await
        .unwrap();
    let run = scheduled_round(&store, &rule, &[QualityVerdict::Incorrect]).await;
    assert_eq!(run.action.as_deref(), Some("excel_shared_action_blocked"));
    let count: i64 = sqlx::query_scalar(
        "select count(*) from account_group_accounts where provider_account_id='acct_quality_a'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    db.close().await;
}
