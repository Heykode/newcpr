use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use gateway_admin::ports::store::AccountStore;
use gateway_core::account::{
    AccountErrorReason, AccountStateChange, CredentialCasOutcome, CredentialCasUpdate,
    CredentialState, PlaintextCredential, ProviderAccountStore, ProviderAccountUpdate,
    ProviderDeviceCodec, ProviderDeviceCodecError,
};
use gateway_store::{
    JsonObject,
    postgres::{
        ImportProviderAccounts, NewProviderAccount, ProviderAccountAdminRepository,
        ProviderAccountAdminScope, RotateProviderAccount,
    },
};
use serde_json::json;

use crate::postgres::provider_accounts::{audit, credential_update, profile};
use crate::postgres::{
    TestDatabase,
    provider_accounts::account,
    turn_states::{candidate, enable},
};
use gateway_core::{
    account::ProviderAccountId,
    provider_ports::{ProviderTurnStateAnomaly, ProviderTurnStatePort, ProviderTurnStateSlot},
    routing::UpstreamModelId,
};
use gateway_store::postgres::{
    PgProviderAccountRepository, PgProviderTurnStateRepository, ProviderAccountRepository,
};

struct RetentionCodec;

impl ProviderDeviceCodec for RetentionCodec {
    fn provider_kind(&self) -> &str {
        "openai"
    }

    fn installation_id(
        &self,
        credential: &PlaintextCredential,
    ) -> Result<String, ProviderDeviceCodecError> {
        credential.expose_to_provider()["device"]
            .as_str()
            .map(str::to_owned)
            .ok_or(ProviderDeviceCodecError::Invalid)
    }

    fn with_installation_id(
        &self,
        credential: &PlaintextCredential,
        device: &str,
    ) -> Result<PlaintextCredential, ProviderDeviceCodecError> {
        let mut fields = credential.expose_to_provider().clone();
        fields.insert("device".into(), json!(device));
        Ok(PlaintextCredential::new(fields))
    }

    fn can_retain_turn_state(&self, old: &PlaintextCredential, new: &PlaintextCredential) -> bool {
        let old = old.expose_to_provider();
        let new = new.expose_to_provider();
        ["device", "owner"]
            .iter()
            .all(|key| old.get(*key).is_some() && old.get(*key) == new.get(*key))
    }
}

fn material(token: &str) -> JsonObject {
    JsonObject::try_from_value(
        "provider_credentials_json",
        json!({
            "device": "synthetic-device", "owner": "synthetic-owner", "access_token": token
        }),
        256 * 1024,
    )
    .unwrap()
}

fn seed(id: &str, token: &str) -> NewProviderAccount {
    let mut seed = account(id, "synthetic-owner");
    seed.upstream_account_id = Some("synthetic-workspace".into());
    seed.provider_credentials_json = material(token);
    seed
}

async fn renew(accounts: &PgProviderAccountRepository, id: &ProviderAccountId, path: usize) {
    let current = accounts.load_current_credential(id).await.unwrap();
    let revision = current.account.revision().get();
    let token = format!("synthetic-renewed-{revision}");
    let mut update = credential_update(id.as_str(), revision, &token);
    update.provider_credentials_json = material(&token);
    match path {
        0 => {
            let update = CredentialCasUpdate::new(
                id.clone(),
                current.account.revision(),
                ProviderAccountUpdate {
                    account_id: id.clone(),
                    name: "test".into(),
                    email: None,
                    plan_type: current.account.plan_type().map(str::to_owned),
                },
                PlaintextCredential::new(material(&token).fields().clone()),
                false,
                current.account.access_token_expires_at(),
                None,
            )
            .unwrap()
            .preserving_profile();
            assert!(matches!(
                accounts.compare_and_swap_credential(update).await.unwrap(),
                CredentialCasOutcome::Updated(_)
            ));
        }
        1 => {
            accounts.compare_and_swap_credentials(update).await.unwrap();
        }
        2 => {
            // A relogin/OAuth rotation carries a new credential and the same profile.
            let mut profile = profile(id.as_str(), "renewed");
            profile.plan_type = current.account.plan_type().map(str::to_owned);
            accounts
                .rotate_provider_account(RotateProviderAccount {
                    scope: ProviderAccountAdminScope {
                        provider_kind: "openai".into(),
                    },
                    profile,
                    replacement_identity: None,
                    credential: update,
                    relogin_operation_id: Some(format!("renewal-{revision}")),
                    audit: audit(&format!("rotation-{revision}"), "rotate", id.as_str()),
                })
                .await
                .unwrap();
        }
        3 | 4 => {
            let mut imported = seed("synthetic-import-id", &token);
            if path == 4 {
                imported.provider_credentials_json = JsonObject::try_from_value(
                    "provider_credentials_json",
                    serde_json::Value::Object(current.credential.into_inner()),
                    256 * 1024,
                )
                .unwrap();
            }
            let result = accounts
                .import_provider_accounts(ImportProviderAccounts {
                    scope: ProviderAccountAdminScope {
                        provider_kind: "openai".into(),
                    },
                    settings: None,
                    outbound_proxy: None,
                    accounts: vec![imported],
                    audit: audit(&format!("import-{revision}"), "import", id.as_str()),
                })
                .await
                .unwrap();
            assert_eq!(result.account_ids, vec![id.as_str()]);
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn all_credential_renewal_paths_retain_slots_and_clocks_but_fence_old_writers() {
    let Some(database) = TestDatabase::create("state_renewal_paths").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .initialize_device_registry(Arc::new(RetentionCodec))
        .await
        .unwrap();
    let id = ProviderAccountId::new("acct_renewal").unwrap();
    accounts
        .insert_provider_account(seed(id.as_str(), "synthetic-initial"))
        .await
        .unwrap();
    enable(&database).await;
    let states = PgProviderTurnStateRepository::new(database.pool.clone());
    let model = UpstreamModelId::new("model-a").unwrap();
    let other = UpstreamModelId::new("model-b").unwrap();
    let now = SystemTime::now();
    states
        .put_candidate(candidate(
            &id,
            &model,
            &"a".repeat(292),
            now - Duration::from_secs(600),
            ProviderTurnStateSlot::Active,
            292,
        ))
        .await
        .unwrap();
    let original = states
        .put_candidate(candidate(
            &id,
            &model,
            &"b".repeat(292),
            now,
            ProviderTurnStateSlot::Standby,
            292,
        ))
        .await
        .unwrap();
    assert!(original.active().is_some());
    assert!(original.standby().is_some());
    let ids = [id.as_str().to_owned()];
    let admin = crate::postgres::admin_account_store(&database.pool);
    let initial_projection = admin.load_turn_state_status(&ids).await.unwrap();
    let initial_model = initial_projection[id.as_str()]
        .models
        .iter()
        .find(|m| m.model == "model-a")
        .unwrap();
    let captured = initial_model.active.as_ref().unwrap().captured_at;
    let standby_captured = initial_model.standby.as_ref().unwrap().captured_at;
    let mut version = original.state_version();
    for path in 0..5 {
        let old = accounts.get_account(&id).await.unwrap().unwrap();
        renew(&accounts, &id, path).await;
        let current = accounts.get_account(&id).await.unwrap().unwrap();
        assert!(current.turn_state_binding_revision() > old.turn_state_binding_revision());
        assert!(
            accounts
                .apply_state_change(AccountStateChange {
                    account_id: id.clone(),
                    expected_revision: old.revision(),
                    credential_state: CredentialState::Invalid,
                    observed_at: SystemTime::now(),
                    error_reason: Some(AccountErrorReason::CredentialInvalid),
                    message: None,
                })
                .await
                .is_err(),
            "a late authentication failure must not disable the renewed account"
        );
        let retained = states
            .read(&id, &model, current.turn_state_binding_revision())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.active(), original.active());
        assert_eq!(retained.standby(), original.standby());
        assert_eq!(
            retained.last_observed_length(),
            original.last_observed_length()
        );
        assert_eq!(retained.state_version(), version + 1);
        version = retained.state_version();
        assert!(
            states
                .read(&id, &model, old.turn_state_binding_revision())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            states
                .read(&id, &other, current.turn_state_binding_revision())
                .await
                .unwrap()
                .is_none()
        );

        let mut late = candidate(
            &id,
            &model,
            &"c".repeat(292),
            now,
            ProviderTurnStateSlot::Active,
            292,
        );
        late.expected_revision = old.turn_state_binding_revision();
        assert!(states.put_candidate(late).await.is_err());
        assert!(
            states
                .record_anomaly(ProviderTurnStateAnomaly {
                    account_id: id.clone(),
                    upstream_model: model.clone(),
                    expected_revision: old.turn_state_binding_revision(),
                    expected_active_version: version - 1,
                    normal_length: 292,
                    observed_length: Some(312),
                    suspect: true,
                    promote_standby: true,
                    observed_at: SystemTime::now(),
                })
                .await
                .is_err()
        );
        states
            .cancel_refresh(&id, &model, old.turn_state_binding_revision())
            .await
            .unwrap();
        let projection = admin.load_turn_state_status(&ids).await.unwrap();
        assert_eq!(projection[id.as_str()].ready_models.len(), 1);
        let projected = projection[id.as_str()]
            .models
            .iter()
            .find(|m| m.model == "model-a")
            .unwrap();
        assert_eq!(projected.active.as_ref().unwrap().captured_at, captured);
        assert_eq!(
            projected.standby.as_ref().unwrap().captured_at,
            standby_captured
        );
        assert_eq!(projected.refresh_status, "ready");
    }
    database.close().await;
}

#[tokio::test]
async fn renewal_retains_disabled_cache_without_resurrecting_expired_or_rejected_slots() {
    let Some(database) = TestDatabase::create("state_renewal_disabled").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .initialize_device_registry(Arc::new(RetentionCodec))
        .await
        .unwrap();
    let id = ProviderAccountId::new("acct_renewal_cache").unwrap();
    accounts
        .insert_provider_account(seed(id.as_str(), "synthetic-initial"))
        .await
        .unwrap();
    enable(&database).await;
    let states = PgProviderTurnStateRepository::new(database.pool.clone());
    let model = UpstreamModelId::new("model-a").unwrap();
    states
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
    sqlx::query("update provider_accounts set turn_state_injection_enabled=false, credential_state='expired'")
        .execute(&database.pool).await.unwrap();
    renew(&accounts, &id, 0).await;
    let current = accounts.get_account(&id).await.unwrap().unwrap();
    assert!(
        states
            .read(&id, &model, current.turn_state_binding_revision())
            .await
            .unwrap()
            .is_none()
    );
    let count: i64 = sqlx::query_scalar("select count(*) from provider_turn_states where active_state is not null and credential_revision=2")
        .fetch_one(&database.pool).await.unwrap();
    assert_eq!(count, 1);
    sqlx::query(
        "update provider_accounts set turn_state_injection_enabled=true, credential_state='ready'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert!(
        states
            .read(&id, &model, current.turn_state_binding_revision())
            .await
            .unwrap()
            .unwrap()
            .active()
            .is_some()
    );
    sqlx::query("update provider_turn_states set active_issued_at=now()-interval '2 hours', active_expires_at=now()-interval '1 hour'")
        .execute(&database.pool).await.unwrap();
    renew(&accounts, &id, 0).await;
    let current = accounts.get_account(&id).await.unwrap().unwrap();
    assert!(
        states
            .read(&id, &model, current.turn_state_binding_revision())
            .await
            .unwrap()
            .unwrap()
            .active()
            .is_none()
    );
    // An explicitly rejected (empty) slot also stays empty across the next login.
    renew(&accounts, &id, 2).await;
    let current = accounts.get_account(&id).await.unwrap().unwrap();
    assert!(
        states
            .read(&id, &model, current.turn_state_binding_revision())
            .await
            .unwrap()
            .unwrap()
            .active()
            .is_none()
    );
    database.close().await;
}

#[tokio::test]
async fn renewal_does_not_rebind_a_stale_generation_or_changed_plan_or_principal() {
    let Some(database) = TestDatabase::create("state_renewal_mismatch").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .initialize_device_registry(Arc::new(RetentionCodec))
        .await
        .unwrap();
    for change in [
        "stale",
        "plan",
        "principal",
        "missing_user",
        "missing_workspace",
    ] {
        sqlx::query("delete from provider_accounts")
            .execute(&database.pool)
            .await
            .unwrap();
        let id = ProviderAccountId::new("acct_renewal_mismatch").unwrap();
        accounts
            .insert_provider_account(seed(id.as_str(), "synthetic-initial"))
            .await
            .unwrap();
        enable(&database).await;
        let model = UpstreamModelId::new("model-a").unwrap();
        let states = PgProviderTurnStateRepository::new(database.pool.clone());
        states
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
        match change {
            "stale" => {
                sqlx::query("update provider_turn_states set credential_revision=0")
                    .execute(&database.pool)
                    .await
                    .unwrap();
            }
            "missing_user" => {
                sqlx::query("update provider_accounts set upstream_user_id=null, credential_state='unknown'").execute(&database.pool).await.unwrap();
            }
            "missing_workspace" => {
                sqlx::query("update provider_accounts set upstream_account_id=null")
                    .execute(&database.pool)
                    .await
                    .unwrap();
            }
            _ => {}
        }
        let loaded = accounts.load_current_credential(&id).await.unwrap();
        let mut data = material("synthetic-new").fields().clone();
        if change == "principal" {
            data.insert("owner".into(), json!("different-owner"));
        }
        let update = CredentialCasUpdate::new(
            id.clone(),
            loaded.account.revision(),
            ProviderAccountUpdate {
                account_id: id.clone(),
                name: "test".into(),
                email: None,
                plan_type: Some(if change == "plan" { "business" } else { "pro" }.into()),
            },
            PlaintextCredential::new(data),
            false,
            loaded.account.access_token_expires_at(),
            None,
        )
        .unwrap();
        assert!(matches!(
            accounts.compare_and_swap_credential(update).await.unwrap(),
            CredentialCasOutcome::Updated(_)
        ));
        let current = accounts.get_account(&id).await.unwrap().unwrap();
        assert!(
            states
                .read(&id, &model, current.turn_state_binding_revision())
                .await
                .unwrap()
                .is_none(),
            "{change}"
        );
    }
    database.close().await;
}
