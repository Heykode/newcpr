use chrono::{TimeDelta, Utc};
use gateway_admin::{
    model::codex_quota_overdraft::CodexQuotaOverdraftOutcome, ports::store::AccountStore,
};

use super::{TestDatabase, admin_account_store};

#[tokio::test]
async fn overdraft_claims_are_single_owner_revision_fenced_and_recover_with_quota() {
    let Some(database) = TestDatabase::create("codex_quota_overdraft").await else {
        return;
    };
    sqlx::query(
        "update runtime_settings set codex_quota_overdraft_enabled=true,
         codex_quota_overdraft_probe_model='gpt-6-astra',config_revision=config_revision+1 where id=1",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let reset_at = Utc::now() + TimeDelta::hours(5);
    sqlx::query(
        "insert into provider_accounts(
           id,provider_kind,name,upstream_user_id,upstream_account_id,authentication_kind,
           provider_credentials_json,has_refresh_token,access_token_expires_at,credential_state,
           credential_observed_at,quota_access_state,quota_evidence,quota_access_observed_at,quota_reset_at,
           created_at,updated_at)
         values('acct_overdraft_test','openai','test','user-test','workspace-test','oauth','{}'::jsonb,true,
           now()+interval '1 hour','ready',now(),'exhausted','usage_limit_reached',now(),$1,now(),now())",
    )
    .bind(reset_at)
    .execute(&database.pool)
    .await
    .unwrap();
    let store = admin_account_store(&database.pool);
    let first = store
        .claim_codex_quota_overdraft()
        .await
        .unwrap()
        .expect("first claim");
    assert_eq!(first.attempts, 1);
    assert!(store.claim_codex_quota_overdraft().await.unwrap().is_none());
    assert!(
        store
            .finish_codex_quota_overdraft(
                &first,
                CodexQuotaOverdraftOutcome::Inconclusive {
                    reason_code: "upstream_unavailable".to_owned(),
                    tested_at: Utc::now(),
                    retry_at: Utc::now() - TimeDelta::seconds(1),
                },
            )
            .await
            .unwrap()
    );
    let second = store
        .claim_codex_quota_overdraft()
        .await
        .unwrap()
        .expect("retry claim");
    assert_eq!(second.attempts, 2);
    assert!(
        store
            .finish_codex_quota_overdraft(
                &second,
                CodexQuotaOverdraftOutcome::Passed {
                    tested_at: Utc::now(),
                },
            )
            .await
            .unwrap()
    );
    let states = store
        .load_codex_quota_overdraft(&["acct_overdraft_test".to_owned()])
        .await
        .unwrap();
    assert_eq!(states["acct_overdraft_test"].status, "passed");

    sqlx::query(
        "update provider_accounts set quota_access_state='allowed',quota_evidence=null,quota_reset_at=null,quota_access_observed_at=now(),updated_at=now() where id='acct_overdraft_test'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert!(store.claim_codex_quota_overdraft().await.unwrap().is_none());
    let states = store
        .load_codex_quota_overdraft(&["acct_overdraft_test".to_owned()])
        .await
        .unwrap();
    assert_eq!(states["acct_overdraft_test"].status, "recovered");
    database.close().await;
}
