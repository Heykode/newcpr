use std::time::{Duration, SystemTime};

use gateway_core::{
    account::{CredentialRevision, ProviderAccountId},
    provider_ports::{
        OpaqueTurnState, ProviderTurnStateAnomaly, ProviderTurnStateCandidate,
        ProviderTurnStatePort, ProviderTurnStateRefreshStatus, ProviderTurnStateSlot,
        ProviderTurnStateValue,
    },
    routing::UpstreamModelId,
};
use gateway_store::postgres::{
    PgProviderAccountRepository, PgProviderTurnStateRepository, ProviderAccountRepository,
};

use super::{TestDatabase, provider_accounts::account};

fn revision() -> CredentialRevision {
    CredentialRevision::new(1).unwrap()
}

async fn enable(database: &TestDatabase) {
    sqlx::query(
        "update runtime_settings set turn_state_injection_enabled = true,
        turn_state_models = array['gpt-6-astra', 'gpt-5.6-sol', 'model-a', 'model-b']",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query("update provider_accounts set turn_state_injection_enabled = true")
        .execute(&database.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn admin_readiness_and_cancel_cleanup_are_model_revision_and_policy_fenced() {
    use gateway_admin::ports::store::AccountStore;
    let Some(database) = TestDatabase::create("turn_state_readiness").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .insert_provider_account(account("acct_ready", "ready-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let admin = super::admin_account_store(&database.pool);
    let store = PgProviderTurnStateRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_ready").unwrap();
    let model = UpstreamModelId::new("model-a").unwrap();
    let ids = [id.as_str().to_owned()];
    assert!(
        admin.load_turn_state_status(&ids).await.unwrap()[id.as_str()]
            .ready_models
            .is_empty()
    );
    store
        .put_candidate(candidate(
            &id,
            &model,
            &"a".repeat(292),
            SystemTime::now(),
            ProviderTurnStateSlot::Active,
            292,
        ))
        .await
        .unwrap();
    let statuses = admin.load_turn_state_status(&ids).await.unwrap();
    assert_eq!(statuses[id.as_str()].ready_models.len(), 1);
    assert_eq!(statuses[id.as_str()].ready_models[0].0, "model-a");
    store
        .mark_refresh_status(
            &id,
            &model,
            revision(),
            292,
            ProviderTurnStateRefreshStatus::Refreshing,
            SystemTime::now(),
        )
        .await
        .unwrap();
    sqlx::query("update runtime_settings set turn_state_injection_enabled=false")
        .execute(&database.pool)
        .await
        .unwrap();
    store.cancel_refresh(&id, &model, revision()).await.unwrap();
    assert!(admin.load_turn_state_status(&ids).await.unwrap().is_empty());
    let status: String = sqlx::query_scalar("select refresh_status from provider_turn_states")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(status, "failed");
    enable(&database).await;
    sqlx::query("update provider_accounts set credential_revision=2")
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(
        admin.load_turn_state_status(&ids).await.unwrap()[id.as_str()]
            .ready_models
            .is_empty()
    );
    store
        .mark_refresh_status(
            &id,
            &model,
            CredentialRevision::new(2).unwrap(),
            292,
            ProviderTurnStateRefreshStatus::Refreshing,
            SystemTime::now(),
        )
        .await
        .unwrap();
    store.cancel_refresh(&id, &model, revision()).await.unwrap();
    let status: String = sqlx::query_scalar("select refresh_status from provider_turn_states")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(
        status, "refreshing",
        "old cancellation must not finish a newer credential task"
    );
    database.close().await;
}

fn candidate(
    account_id: &ProviderAccountId,
    model: &UpstreamModelId,
    value: &str,
    issued_at: SystemTime,
    slot: ProviderTurnStateSlot,
    normal_length: u16,
) -> ProviderTurnStateCandidate {
    ProviderTurnStateCandidate {
        account_id: account_id.clone(),
        expected_revision: revision(),
        expected_active_version: None,
        upstream_model: model.clone(),
        normal_length,
        slot,
        value: ProviderTurnStateValue::new(
            OpaqueTurnState::new(value.to_owned()),
            issued_at,
            issued_at + Duration::from_secs(60 * 60),
        ),
        observed_at: issued_at,
    }
}

#[tokio::test]
async fn repeated_active_is_idempotent_and_new_active_preserves_a_valid_standby() {
    let Some(database) = TestDatabase::create("turn_state_active").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .insert_provider_account(account("acct_turn_state", "turn-state-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let store = PgProviderTurnStateRepository::new(database.pool.clone());
    let account_id = ProviderAccountId::new("acct_turn_state").unwrap();
    let model = UpstreamModelId::new("gpt-6-astra").unwrap();
    let issued_at = SystemTime::now();
    let first_value = "a".repeat(292);
    let second_value = "b".repeat(292);

    let first = store
        .put_candidate(candidate(
            &account_id,
            &model,
            &first_value,
            issued_at,
            ProviderTurnStateSlot::Active,
            292,
        ))
        .await
        .unwrap();
    assert_eq!(first.state_version(), 1);

    let repeated = store
        .put_candidate(candidate(
            &account_id,
            &model,
            &first_value,
            issued_at,
            ProviderTurnStateSlot::Active,
            292,
        ))
        .await
        .unwrap();
    assert_eq!(
        repeated.state_version(),
        1,
        "same state must not retire WS pools"
    );

    let replaced = store
        .put_candidate(candidate(
            &account_id,
            &model,
            &second_value,
            issued_at + Duration::from_secs(1),
            ProviderTurnStateSlot::Active,
            292,
        ))
        .await
        .unwrap();
    assert_eq!(replaced.state_version(), 2);
    assert_eq!(
        replaced.active().unwrap().state().expose_to_provider(),
        second_value
    );
    assert_eq!(
        replaced.standby().unwrap().state().expose_to_provider(),
        first_value
    );
    assert!(!format!("{replaced:?}").contains(&second_value));
    database.close().await;
}

#[tokio::test]
async fn anomaly_promotes_valid_standby_and_shape_change_clears_old_tokens() {
    let Some(database) = TestDatabase::create("turn_state_promote").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .insert_provider_account(account("acct_turn_promote", "turn-promote-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let store = PgProviderTurnStateRepository::new(database.pool.clone());
    let account_id = ProviderAccountId::new("acct_turn_promote").unwrap();
    let model = UpstreamModelId::new("gpt-5.6-sol").unwrap();
    let issued_at = SystemTime::now();
    let active = "c".repeat(292);
    let standby = "d".repeat(292);
    store
        .put_candidate(candidate(
            &account_id,
            &model,
            &active,
            issued_at,
            ProviderTurnStateSlot::Active,
            292,
        ))
        .await
        .unwrap();
    store
        .put_candidate(candidate(
            &account_id,
            &model,
            &standby,
            issued_at + Duration::from_secs(1),
            ProviderTurnStateSlot::Standby,
            292,
        ))
        .await
        .unwrap();

    let promoted = store
        .record_anomaly(ProviderTurnStateAnomaly {
            account_id: account_id.clone(),
            expected_revision: revision(),
            expected_active_version: 1,
            upstream_model: model.clone(),
            normal_length: 292,
            observed_length: Some(312),
            promote_standby: true,
            observed_at: issued_at + Duration::from_secs(2),
        })
        .await
        .unwrap();
    assert_eq!(promoted.state_version(), 2);
    assert_eq!(
        promoted.active().unwrap().state().expose_to_provider(),
        standby
    );
    assert!(promoted.standby().is_none());
    assert_eq!(
        promoted.refresh_status(),
        ProviderTurnStateRefreshStatus::Refreshing
    );

    let reset = store
        .mark_refresh_status(
            &account_id,
            &model,
            revision(),
            332,
            ProviderTurnStateRefreshStatus::Refreshing,
            issued_at + Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert_eq!(reset.normal_length(), 332);
    assert!(reset.active().is_none());
    assert!(reset.standby().is_none());
    assert_eq!(reset.state_version(), 3);
    assert_eq!(reset.last_observed_length(), None);
    database.close().await;
}

#[tokio::test]
async fn concurrent_repeated_observations_keep_one_version_and_older_values_cannot_replace_it() {
    let Some(database) = TestDatabase::create("turn_state_concurrent").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .insert_provider_account(account("acct_turn_concurrent", "concurrent-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let store = PgProviderTurnStateRepository::new(database.pool.clone());
    let account_id = ProviderAccountId::new("acct_turn_concurrent").unwrap();
    let model = UpstreamModelId::new("model-a").unwrap();
    let now = SystemTime::now();
    let value = "x".repeat(292);
    let results = futures::future::join_all((0..16).map(|_| {
        store.put_candidate(candidate(
            &account_id,
            &model,
            &value,
            now,
            ProviderTurnStateSlot::Active,
            292,
        ))
    }))
    .await;
    for result in results {
        assert_eq!(result.unwrap().state_version(), 1);
    }
    let mut stale = candidate(
        &account_id,
        &model,
        &"y".repeat(292),
        now - Duration::from_secs(10),
        ProviderTurnStateSlot::Active,
        292,
    );
    stale.observed_at = now;
    let record = store.put_candidate(stale).await.unwrap();
    assert_eq!(record.state_version(), 1);
    assert_eq!(record.active().unwrap().state().expose_to_provider(), value);
    assert!(
        store
            .read(
                &account_id,
                &UpstreamModelId::new("model-b").unwrap(),
                revision()
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .read(
                &ProviderAccountId::new("acct_other").unwrap(),
                &model,
                revision()
            )
            .await
            .unwrap()
            .is_none()
    );
    database.close().await;
}

#[tokio::test]
async fn expired_or_not_yet_issued_standby_is_never_promoted() {
    let Some(database) = TestDatabase::create("turn_state_expired").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .insert_provider_account(account("acct_turn_expired", "expired-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let store = PgProviderTurnStateRepository::new(database.pool.clone());
    let account_id = ProviderAccountId::new("acct_turn_expired").unwrap();
    let model = UpstreamModelId::new("model-a").unwrap();
    let now = SystemTime::now();
    store
        .put_candidate(candidate(
            &account_id,
            &model,
            &"s".repeat(292),
            now,
            ProviderTurnStateSlot::Standby,
            292,
        ))
        .await
        .unwrap();
    for observed_at in [
        now - Duration::from_secs(1),
        now + Duration::from_secs(3601),
    ] {
        let record = store
            .record_anomaly(ProviderTurnStateAnomaly {
                account_id: account_id.clone(),
                expected_revision: revision(),
                expected_active_version: 0,
                upstream_model: model.clone(),
                normal_length: 292,
                observed_length: Some(312),
                promote_standby: true,
                observed_at,
            })
            .await
            .unwrap();
        assert!(record.active().is_none());
        assert_eq!(record.state_version(), 0);
    }
    let mut future = candidate(
        &account_id,
        &model,
        &"f".repeat(292),
        now + Duration::from_secs(1),
        ProviderTurnStateSlot::Active,
        292,
    );
    future.observed_at = now;
    assert!(store.put_candidate(future).await.is_err());
    database.close().await;
}

#[tokio::test]
async fn policy_and_credential_fences_reject_late_writes_and_reads() {
    let Some(database) = TestDatabase::create("turn_state_fences").await else {
        return;
    };
    PgProviderAccountRepository::new(database.pool.clone())
        .insert_provider_account(account("acct_turn_fences", "fences-owner"))
        .await
        .unwrap();
    let store = PgProviderTurnStateRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_turn_fences").unwrap();
    let model = UpstreamModelId::new("model-a").unwrap();
    let value = candidate(
        &id,
        &model,
        &"a".repeat(292),
        SystemTime::now(),
        ProviderTurnStateSlot::Active,
        292,
    );
    assert!(store.put_candidate(value.clone()).await.is_err());
    enable(&database).await;
    store.put_candidate(value.clone()).await.unwrap();
    for disable in [
        "update runtime_settings set turn_state_injection_enabled = false",
        "update provider_accounts set turn_state_injection_enabled = false",
        "update runtime_settings set turn_state_models = array['excluded']",
        "update provider_accounts set enabled = false",
    ] {
        sqlx::query(disable).execute(&database.pool).await.unwrap();
        assert!(store.read(&id, &model, revision()).await.unwrap().is_none());
        assert!(store.put_candidate(value.clone()).await.is_err());
        assert!(
            store
                .mark_refresh_status(
                    &id,
                    &model,
                    revision(),
                    292,
                    ProviderTurnStateRefreshStatus::Refreshing,
                    SystemTime::now()
                )
                .await
                .is_err()
        );
        enable(&database).await;
        sqlx::query("update provider_accounts set enabled = true")
            .execute(&database.pool)
            .await
            .unwrap();
    }
    sqlx::query("update provider_accounts set credential_revision = 2")
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(store.read(&id, &model, revision()).await.unwrap().is_none());
    assert!(
        store
            .read(&id, &model, CredentialRevision::new(2).unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.put_candidate(value.clone()).await.is_err());
    let mut fresh = value;
    fresh.expected_revision = CredentialRevision::new(2).unwrap();
    fresh.value = ProviderTurnStateValue::new(
        OpaqueTurnState::new("b".repeat(292)),
        SystemTime::now(),
        SystemTime::now() + Duration::from_secs(3600),
    );
    fresh.observed_at = SystemTime::now();
    let record = store.put_candidate(fresh).await.unwrap();
    assert_eq!(
        record.active().unwrap().state().expose_to_provider(),
        "b".repeat(292)
    );
    assert!(
        record.standby().is_none(),
        "old credential must not become standby"
    );
    database.close().await;
}

#[tokio::test]
async fn late_anomaly_cannot_rotate_new_active_and_rejection_without_standby_clears_active() {
    let Some(database) = TestDatabase::create("turn_state_anomaly_cas").await else {
        return;
    };
    PgProviderAccountRepository::new(database.pool.clone())
        .insert_provider_account(account("acct_turn_cas", "cas-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let store = PgProviderTurnStateRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_turn_cas").unwrap();
    let model = UpstreamModelId::new("model-a").unwrap();
    let now = SystemTime::now();
    for (value, slot) in [
        ("a", ProviderTurnStateSlot::Active),
        ("b", ProviderTurnStateSlot::Standby),
    ] {
        store
            .put_candidate(candidate(&id, &model, &value.repeat(292), now, slot, 292))
            .await
            .unwrap();
    }
    let mut anomaly = ProviderTurnStateAnomaly {
        account_id: id.clone(),
        upstream_model: model.clone(),
        expected_revision: revision(),
        expected_active_version: 1,
        normal_length: 292,
        observed_length: Some(312),
        promote_standby: true,
        observed_at: now,
    };
    let promoted = store.record_anomaly(anomaly.clone()).await.unwrap();
    assert_eq!(promoted.state_version(), 2);
    assert_eq!(
        promoted.active().unwrap().state().expose_to_provider(),
        "b".repeat(292)
    );
    let late = store.record_anomaly(anomaly.clone()).await.unwrap();
    assert_eq!(
        late, promoted,
        "late response must not invalidate a newer version"
    );
    anomaly.expected_active_version = 2;
    let rejected = store.record_anomaly(anomaly).await.unwrap();
    assert_eq!(rejected.state_version(), 3);
    assert!(rejected.active().is_none());
    database.close().await;
}

#[tokio::test]
async fn late_normal_echo_cannot_resurrect_rejected_active_after_standby_promotion() {
    let Some(database) = TestDatabase::create("turn_state_echo_fence").await else {
        return;
    };
    PgProviderAccountRepository::new(database.pool.clone())
        .insert_provider_account(account("acct_turn_echo", "echo-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let store = PgProviderTurnStateRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_turn_echo").unwrap();
    let model = UpstreamModelId::new("model-a").unwrap();
    let now = SystemTime::now();
    for (value, slot) in [
        ("a", ProviderTurnStateSlot::Active),
        ("b", ProviderTurnStateSlot::Standby),
    ] {
        store
            .put_candidate(candidate(&id, &model, &value.repeat(292), now, slot, 292))
            .await
            .unwrap();
    }
    let promoted = store
        .record_anomaly(ProviderTurnStateAnomaly {
            account_id: id.clone(),
            expected_revision: revision(),
            expected_active_version: 1,
            upstream_model: model.clone(),
            normal_length: 292,
            observed_length: Some(312),
            promote_standby: true,
            observed_at: now,
        })
        .await
        .unwrap();
    let mut late = candidate(
        &id,
        &model,
        &"a".repeat(292),
        now,
        ProviderTurnStateSlot::Active,
        292,
    );
    late.expected_active_version = Some(1);
    late.observed_at = now + Duration::from_secs(1);
    assert_eq!(
        store.put_candidate(late.clone()).await.unwrap(),
        promoted,
        "an old request's normal echo cannot undo rejection and standby promotion"
    );
    let rejected = store
        .record_anomaly(ProviderTurnStateAnomaly {
            account_id: id.clone(),
            expected_revision: revision(),
            expected_active_version: promoted.state_version(),
            upstream_model: model.clone(),
            normal_length: 292,
            observed_length: Some(312),
            promote_standby: true,
            observed_at: now + Duration::from_secs(2),
        })
        .await
        .unwrap();
    assert!(rejected.active().is_none());
    assert_eq!(
        store.put_candidate(late).await.unwrap(),
        rejected,
        "an old normal echo cannot restore a cleared active"
    );
    let fresh = store
        .put_candidate(candidate(
            &id,
            &model,
            &"c".repeat(292),
            now + Duration::from_secs(3),
            ProviderTurnStateSlot::Active,
            292,
        ))
        .await
        .unwrap();
    assert_eq!(fresh.state_version(), rejected.state_version() + 1);
    assert_eq!(
        fresh.active().unwrap().state().expose_to_provider(),
        "c".repeat(292)
    );
    database.close().await;
}
