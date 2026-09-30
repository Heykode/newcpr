use super::*;
use gateway_admin::model::excel_recovery::{ExcelRecoveryConfig, ExcelRecoveryOutcome};
use gateway_core::account::ResponsesUpstream;

fn patch(id: &str) -> BatchUpdateAccounts {
    BatchUpdateAccounts {
        account_ids: vec![id.into()],
        excel_recovery: None,
        purchase_cost: None,
        egress_mode: None,
        custom_name: None,
        enabled: None,
        turn_state_injection_enabled: None,
        responses_upstream: None,
        excel_models: None,
        excel_models_follow_global: None,
        excel_cache_creation_as_input: None,
        excel_ignore_encrypted_content: None,
        request_proxy_source: None,
        excel_auto_disable_on_403: None,
        excel_403_action: None,
        concurrency_limit: None,
        weight: None,
        model_access: None,
        group_ids: None,
        outbound_proxy: None,
    }
}

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: uuid::Uuid::new_v4().to_string(),
    }
}

async fn seed(db: &TestDatabase, id: &str) {
    PgProviderAccountRepository::new(db.pool.clone())
        .insert_provider_account(account(id, id))
        .await
        .unwrap();
    admin_account_store(&db.pool)
        .batch_update_accounts(
            BatchUpdateAccounts {
                enabled: Some(false),
                responses_upstream: Some(ResponsesUpstream::Excel),
                excel_recovery: Some(ExcelRecoveryConfig {
                    enabled: true,
                    interval_minutes: 60,
                }),
                ..patch(id)
            },
            &context(),
        )
        .await
        .unwrap();
}

async fn due(db: &TestDatabase) {
    sqlx::query("update account_excel_recovery set next_probe_at=now()-interval '1 second'")
        .execute(&db.pool)
        .await
        .unwrap();
}

async fn enabled(db: &TestDatabase, id: &str) -> bool {
    sqlx::query_scalar("select enabled from provider_accounts where id=$1")
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn recovery_defaults_omission_validation_and_success_preserve_history() {
    let Some(db) = TestDatabase::create("excel_recovery_success").await else {
        return;
    };
    let store = admin_account_store(&db.pool);
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    repo.insert_provider_account(account("acct_default", "default"))
        .await
        .unwrap();
    assert!(
        store
            .load_excel_recovery(&["acct_default".into()])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.claim_excel_recovery().await.unwrap().is_none());
    seed(&db, "acct_recovery").await;
    assert!(
        store.claim_excel_recovery().await.unwrap().is_none(),
        "schedule starts after interval"
    );
    assert!(
        store
            .batch_update_accounts(
                BatchUpdateAccounts {
                    excel_recovery: Some(ExcelRecoveryConfig {
                        enabled: true,
                        interval_minutes: 0
                    }),
                    ..patch("acct_recovery")
                },
                &context()
            )
            .await
            .is_err()
    );
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                weight: Some(gateway_core::account::AccountWeight::DEFAULT),
                ..patch("acct_recovery")
            },
            &context(),
        )
        .await
        .unwrap();
    assert!(
        store
            .load_excel_recovery(&["acct_recovery".into()])
            .await
            .unwrap()["acct_recovery"]
            .config
            .enabled
    );
    sqlx::query("update provider_accounts set excel_auto_disabled_at=now(),excel_403_warning_at=now() where id='acct_recovery'").execute(&db.pool).await.unwrap();
    let before: serde_json::Value=sqlx::query_scalar("select to_jsonb(a)-array['enabled','excel_auto_disabled_at','updated_at'] from provider_accounts a where id='acct_recovery'").fetch_one(&db.pool).await.unwrap();
    due(&db).await;
    let claim = store.claim_excel_recovery().await.unwrap().unwrap();
    assert!(store.excel_recovery_current(&claim).await.unwrap());
    assert!(
        store
            .finish_excel_recovery(&claim, ExcelRecoveryOutcome::Recovered)
            .await
            .unwrap()
    );
    assert!(enabled(&db, "acct_recovery").await);
    let after: serde_json::Value=sqlx::query_scalar("select to_jsonb(a)-array['enabled','excel_auto_disabled_at','updated_at'] from provider_accounts a where id='acct_recovery'").fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        before, after,
        "preserve credentials, identity, quota, route and 403 history"
    );
    let state = &store
        .load_excel_recovery(&["acct_recovery".into()])
        .await
        .unwrap()["acct_recovery"];
    assert_eq!(state.last_result.as_deref(), Some("recovered"));
    assert!(state.recovered_at.is_some());
    assert!(
        !store
            .finish_excel_recovery(&claim, ExcelRecoveryOutcome::Recovered)
            .await
            .unwrap(),
        "completion is idempotent"
    );
    due(&db).await;
    assert!(
        store.claim_excel_recovery().await.unwrap().is_none(),
        "enabled accounts are not probed"
    );
    db.close().await;
}

#[tokio::test]
async fn recovery_failures_and_stale_results_never_enable_accounts() {
    let Some(db) = TestDatabase::create("excel_recovery_fences").await else {
        return;
    };
    let store = admin_account_store(&db.pool);
    seed(&db, "acct_fence").await;
    for outcome in [
        ExcelRecoveryOutcome::RequestFailed,
        ExcelRecoveryOutcome::ResponseMismatch,
        ExcelRecoveryOutcome::Cancelled,
    ] {
        due(&db).await;
        let claim = store.claim_excel_recovery().await.unwrap().unwrap();
        assert!(!store.finish_excel_recovery(&claim, outcome).await.unwrap());
        assert!(!enabled(&db, "acct_fence").await);
    }
    for sql in [
        "update provider_accounts set enabled=false where id='acct_fence'",
        "update provider_accounts set credential_revision=credential_revision+1 where id='acct_fence'",
        "update account_excel_recovery set enabled=false,generation=generation+1 where account_id='acct_fence'",
        "update provider_accounts set responses_upstream='codex' where id='acct_fence'",
        "update runtime_settings set disable_fast=not disable_fast where id=1",
        "update provider_accounts set credential_state='invalid' where id='acct_fence'",
    ] {
        sqlx::query("update provider_accounts set responses_upstream='excel',credential_state='ready' where id='acct_fence'").execute(&db.pool).await.unwrap();
        sqlx::query("update account_excel_recovery set enabled=true where account_id='acct_fence'")
            .execute(&db.pool)
            .await
            .unwrap();
        due(&db).await;
        let claim = store.claim_excel_recovery().await.unwrap().unwrap();
        sqlx::query(sql).execute(&db.pool).await.unwrap();
        assert!(
            !store.excel_recovery_current(&claim).await.unwrap(),
            "{sql}"
        );
        assert!(
            !store
                .finish_excel_recovery(&claim, ExcelRecoveryOutcome::Recovered)
                .await
                .unwrap(),
            "{sql}"
        );
        assert!(!enabled(&db, "acct_fence").await);
    }
    db.close().await;
}

#[tokio::test]
async fn recovery_slots_are_global_exclusive_and_survive_deletion_until_drained() {
    let Some(db) = TestDatabase::create("excel_recovery_slots").await else {
        return;
    };
    for id in ["acct_a", "acct_b", "acct_c", "acct_d"] {
        seed(&db, id).await;
    }
    due(&db).await;
    let store = admin_account_store(&db.pool);
    let other = admin_account_store(&db.pool);
    let (a, b, c) = tokio::join!(
        store.claim_excel_recovery(),
        other.claim_excel_recovery(),
        store.claim_excel_recovery()
    );
    let claims = [
        a.unwrap().unwrap(),
        b.unwrap().unwrap(),
        c.unwrap().unwrap(),
    ];
    assert_ne!(claims[0].account_id, claims[1].account_id);
    assert_ne!(claims[0].account_id, claims[2].account_id);
    assert!(other.claim_excel_recovery().await.unwrap().is_none());
    sqlx::query("delete from provider_accounts where id=$1")
        .bind(&claims[0].account_id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(!store.excel_recovery_current(&claims[0]).await.unwrap());
    assert!(
        store.claim_excel_recovery().await.unwrap().is_none(),
        "deleted job still owns its slot while cancelling"
    );
    assert!(
        !store
            .finish_excel_recovery(&claims[0], ExcelRecoveryOutcome::Recovered)
            .await
            .unwrap()
    );
    let next = other.claim_excel_recovery().await.unwrap().unwrap();
    assert!(!claims.iter().any(|c| c.account_id == next.account_id));
    sqlx::query("update excel_recovery_slots set lease_until=now()-interval '1 second';")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("update account_excel_recovery set lease_until=now()-interval '1 second',next_probe_at=now()-interval '1 second'").execute(&db.pool).await.unwrap();
    assert_eq!(
        store
            .load_excel_recovery(std::slice::from_ref(&next.account_id))
            .await
            .unwrap()[&next.account_id]
            .last_result
            .as_deref(),
        Some("interrupted"),
        "expired leases must not remain visibly probing"
    );
    assert!(
        store.claim_excel_recovery().await.unwrap().is_some(),
        "abandoned leases recover after expiry"
    );
    db.close().await;
}

#[tokio::test]
async fn recovery_keeps_failed_model_and_skips_unavailable_credentials() {
    let Some(db) = TestDatabase::create("excel_recovery_models").await else {
        return;
    };
    seed(&db, "acct_model").await;
    let store = admin_account_store(&db.pool);
    sqlx::query("update account_excel_recovery set failed_model='missing_model' where account_id='acct_model'").execute(&db.pool).await.unwrap();
    due(&db).await;
    assert!(
        store.claim_excel_recovery().await.unwrap().is_none(),
        "do not use another model to manufacture recovery"
    );
    assert_eq!(
        store
            .load_excel_recovery(&["acct_model".into()])
            .await
            .unwrap()["acct_model"]
            .last_result
            .as_deref(),
        Some("model_unavailable")
    );
    sqlx::query(
        "update account_excel_recovery set failed_model=null where account_id='acct_model'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    for sql in [
        "update provider_accounts set credential_state='invalid' where id='acct_model'",
        "update provider_accounts set credential_state='ready',access_token_expires_at=now()-interval '1 second' where id='acct_model'",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
        due(&db).await;
        assert!(store.claim_excel_recovery().await.unwrap().is_none());
    }
    db.close().await;
}

#[tokio::test]
async fn recovery_success_does_not_invalidate_other_accounts_and_new_pause_clears_verified_status()
{
    let Some(db) = TestDatabase::create("excel_recovery_parallel").await else {
        return;
    };
    for id in ["acct_one", "acct_two", "acct_three"] {
        seed(&db, id).await;
    }
    due(&db).await;
    let store = admin_account_store(&db.pool);
    let mut claims = Vec::new();
    for _ in 0..3 {
        claims.push(store.claim_excel_recovery().await.unwrap().unwrap());
    }
    for claim in &claims {
        assert!(
            store
                .finish_excel_recovery(claim, ExcelRecoveryOutcome::Recovered)
                .await
                .unwrap(),
            "other account publications are not a configuration change for this account"
        );
    }
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                enabled: Some(false),
                ..patch("acct_one")
            },
            &context(),
        )
        .await
        .unwrap();
    assert!(
        store
            .load_excel_recovery(&["acct_one".into()])
            .await
            .unwrap()["acct_one"]
            .recovered_at
            .is_none()
    );
    store
        .batch_update_accounts(
            BatchUpdateAccounts {
                enabled: Some(true),
                ..patch("acct_one")
            },
            &context(),
        )
        .await
        .unwrap();
    assert!(
        store
            .load_excel_recovery(&["acct_one".into()])
            .await
            .unwrap()["acct_one"]
            .recovered_at
            .is_none(),
        "manual enable cannot reuse an earlier verified result"
    );
    db.close().await;
}
