use super::TestDatabase;
use chrono::Utc;
use gateway_admin::{
    model::{MutationActor, MutationContext, token_guard::*},
    ports::token_guard::TokenGuardStore,
};
use gateway_store::postgres::PgTokenGuardStore;

#[tokio::test]
async fn token_guard_configuration_is_opt_in_audited_and_rejects_missing_groups() {
    let Some(database) = TestDatabase::create("token_guard_config").await else {
        return;
    };
    let store = PgTokenGuardStore::new(database.pool.clone());
    let mut config = store.config().await.unwrap();
    assert_eq!(config, TokenGuardConfig::default());
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "guard_config_fixture".into(),
    };
    config.enabled = true;
    config.concurrency = 2;
    store.configure(&config, &context).await.unwrap();
    assert_eq!(store.config().await.unwrap(), config);
    let audits: i64 = sqlx::query_scalar(
        "select count(*) from admin_audit_events where action='token_guard.configure'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    let mut invalid = config.clone();
    invalid.group_ids.push("missing_fixture_group".into());
    assert!(store.configure(&invalid, &context).await.is_err());
    assert_eq!(store.config().await.unwrap(), config);
    assert!(store.events().await.unwrap().is_empty());
    let missing = TokenGuardEvent {
        account_id: "deleted_fixture_account".into(),
        observed_revision: 1,
        outcome: TokenGuardOutcome::AuthRequired,
        reason: TokenGuardReason::CredentialExpired,
        latency_ms: 0,
        observed_at: Utc::now(),
    };
    store.record(&missing).await.unwrap();
    assert!(store.latest().await.unwrap().is_empty());
    sqlx::query("insert into provider_accounts (
        id,provider_kind,name,upstream_user_id,authentication_kind,provider_credentials_json,
        credential_revision,turn_state_binding_revision,has_refresh_token,credential_observed_at,created_at,updated_at
    ) values ('guard-owner','openai','Guard','fixture-user','oauth','{}',10,10,false,now(),now(),now())")
        .execute(&database.pool).await.unwrap();
    let observation = TokenGuardEvent {
        account_id: "guard-owner".into(),
        observed_revision: 10,
        outcome: TokenGuardOutcome::Healthy,
        reason: TokenGuardReason::Completed,
        ..missing
    };
    store.record(&observation).await.unwrap();
    sqlx::query("update provider_accounts set credential_revision=11 where id='guard-owner'")
        .execute(&database.pool)
        .await
        .unwrap();
    store.record(&observation).await.unwrap();
    assert_eq!(
        store.events().await.unwrap().len(),
        2,
        "Cookie rotation retains probe eligibility"
    );
    sqlx::query("update provider_accounts set credential_revision=12,turn_state_binding_revision=12 where id='guard-owner'")
        .execute(&database.pool).await.unwrap();
    store.record(&observation).await.unwrap();
    assert_eq!(
        store.events().await.unwrap().len(),
        2,
        "old generation cannot write after reauthorization"
    );
    sqlx::query("delete from provider_accounts where id='guard-owner'")
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(store.events().await.unwrap().is_empty());
    database.close().await;
}
