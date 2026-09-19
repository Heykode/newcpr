use std::time::{Duration, SystemTime};

use gateway_core::{
    account::{CredentialRevision, ProviderAccountId},
    provider_ports::{
        OpaqueTurnState, ProviderTurnStateAnomaly, ProviderTurnStateCandidate,
        ProviderTurnStatePort, ProviderTurnStatePromotion, ProviderTurnStateRefreshStatus,
        ProviderTurnStateSlot, ProviderTurnStateValue,
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

#[tokio::test]
async fn business_aliases_and_personal_plans_project_their_own_state_and_success_attempt() {
    use gateway_admin::ports::store::AccountStore;
    let Some(database) = TestDatabase::create("state_plan_progress").await else {
        return;
    };
    PgProviderAccountRepository::new(database.pool.clone())
        .insert_provider_account(account("acct_plan_progress", "plan-progress-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let id = ProviderAccountId::new("acct_plan_progress").unwrap();
    let model = UpstreamModelId::new("model-a").unwrap();
    let states = PgProviderTurnStateRepository::new(database.pool.clone());
    let admin = super::admin_account_store(&database.pool);
    let ids = [id.as_str().to_owned()];
    for (plan, length) in [
        ("business", 332),
        ("team", 332),
        (" Business ", 332),
        ("self_serve_business_prolite", 332),
        ("self_serve_business_usage_based", 332),
        ("plus", 292),
        ("free", 292),
    ] {
        sqlx::query("update provider_accounts set plan_type=$1 where id=$2")
            .bind(plan)
            .bind(id.as_str())
            .execute(&database.pool)
            .await
            .unwrap();
        states
            .mark_refresh_status(
                &id,
                &model,
                revision(),
                length,
                ProviderTurnStateRefreshStatus::Refreshing,
                SystemTime::now(),
            )
            .await
            .unwrap();
        states
            .put_candidate(candidate(
                &id,
                &model,
                &"a".repeat(usize::from(length)),
                SystemTime::now(),
                ProviderTurnStateSlot::Active,
                length,
            ))
            .await
            .unwrap();
        states
            .record_probe_progress(&id, &model, revision(), 11, None, Some(2))
            .await
            .unwrap();
        let projection = admin.load_turn_state_status(&ids).await.unwrap();
        let state = &projection[id.as_str()];
        assert_eq!(state.ready_models.len(), 1, "plan {plan}");
        let row = state
            .models
            .iter()
            .find(|row| row.model == model.as_str())
            .unwrap();
        assert_eq!(row.active.as_ref().unwrap().chars, length);
        assert_eq!(row.probe_attempts, 11);
        assert_eq!(row.successful_probe_attempt, Some(2));
        states
            .mark_refresh_status(
                &id,
                &model,
                revision(),
                length,
                ProviderTurnStateRefreshStatus::Refreshing,
                SystemTime::now(),
            )
            .await
            .unwrap();
        let projection = admin.load_turn_state_status(&ids).await.unwrap();
        let row = projection[id.as_str()]
            .models
            .iter()
            .find(|row| row.model == model.as_str())
            .unwrap();
        assert_eq!(row.probe_attempts, 0);
        assert_eq!(row.successful_probe_attempt, None);
    }
    database.close().await;
}

#[tokio::test]
async fn lifecycle_observations_clocks_cutoff_and_cached_projection_remain_consistent() {
    use gateway_admin::ports::store::AccountStore;
    let Some(database) = TestDatabase::create("state_lifecycle").await else {
        return;
    };
    PgProviderAccountRepository::new(database.pool.clone())
        .insert_provider_account(account("acct_lifecycle", "lifecycle-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let id = ProviderAccountId::new("acct_lifecycle").unwrap();
    let model = UpstreamModelId::new("model-a").unwrap();
    let store = PgProviderTurnStateRepository::new(database.pool.clone());
    let now = SystemTime::now();
    let active = candidate(
        &id,
        &model,
        &"a".repeat(290),
        now - Duration::from_secs(2800),
        ProviderTurnStateSlot::Active,
        292,
    );
    let initial = store.put_candidate(active.clone()).await.unwrap();
    let mut next = candidate(
        &id,
        &model,
        &"b".repeat(290),
        now,
        ProviderTurnStateSlot::Standby,
        292,
    );
    next.expected_active_version = Some(initial.state_version());
    let paired = store.put_candidate(next.clone()).await.unwrap();
    assert_eq!(paired.active(), initial.active());
    let promotion = ProviderTurnStatePromotion {
        account_id: id.clone(),
        expected_revision: revision(),
        expected_active_version: initial.state_version(),
        upstream_model: model.clone(),
        normal_length: 292,
        observed_at: now,
        minimum_remaining: Duration::from_secs(60),
    };
    assert!(
        store
            .promote_standby(promotion.clone())
            .await
            .unwrap()
            .is_none()
    );
    let mut suspect = ProviderTurnStateAnomaly {
        account_id: id.clone(),
        expected_revision: revision(),
        expected_active_version: 1,
        upstream_model: model.clone(),
        normal_length: 292,
        observed_length: Some(312),
        suspect: true,
        promote_standby: false,
        observed_at: now,
    };
    let first = store.record_anomaly(suspect.clone()).await.unwrap();
    assert_eq!(first.active(), initial.active());
    let mut healthy = suspect.clone();
    healthy.suspect = false;
    healthy.observed_length = Some(290);
    store.record_anomaly(healthy).await.unwrap();
    assert_eq!(
        store
            .record_anomaly(suspect.clone())
            .await
            .unwrap()
            .state_version(),
        1
    );
    let replaced = store.record_anomaly(suspect.clone()).await.unwrap();
    assert_eq!(replaced.state_version(), 2);
    assert_eq!(replaced.active(), paired.standby());
    assert!(replaced.standby().is_none());
    assert_eq!(
        store.record_anomaly(suspect.clone()).await.unwrap(),
        replaced
    );
    // A late batch cannot publish against the pre-switch version.
    next.slot = ProviderTurnStateSlot::Active;
    next.value = ProviderTurnStateValue::new(
        OpaqueTurnState::new("c".repeat(290)),
        now,
        now + Duration::from_secs(3600),
    );
    assert_eq!(store.put_candidate(next).await.unwrap(), replaced);
    let admin = super::admin_account_store(&database.pool);
    let ids = [id.as_str().to_owned()];
    store
        .record_probe_progress(&id, &model, revision(), 571, Some("missing_state"), None)
        .await
        .unwrap();
    let projection = admin.load_turn_state_status(&ids).await.unwrap();
    let before = projection[id.as_str()]
        .models
        .iter()
        .find(|model| model.model == "model-a")
        .unwrap();
    assert_eq!(before.probe_attempts, 571);
    assert_eq!(before.last_probe_reason.as_deref(), Some("missing_state"));
    assert_eq!(before.active.as_ref().unwrap().chars, 290);
    let captured = before.active.as_ref().unwrap().captured_at;
    assert!(captured.is_some());
    for switch in [
        "update runtime_settings set turn_state_injection_enabled=false",
        "update provider_accounts set turn_state_injection_enabled=false",
    ] {
        sqlx::query(switch).execute(&database.pool).await.unwrap();
        assert!(store.read(&id, &model, revision()).await.unwrap().is_none());
        let projection = admin.load_turn_state_status(&ids).await.unwrap();
        let status = &projection[id.as_str()];
        assert!(!status.enabled);
        assert!(status.ready_models.is_empty());
        let cached = status
            .models
            .iter()
            .find(|model| model.model == "model-a")
            .unwrap()
            .active
            .as_ref()
            .unwrap();
        assert_eq!(cached.captured_at, captured);
        enable(&database).await;
        assert_eq!(
            store
                .read(&id, &model, revision())
                .await
                .unwrap()
                .unwrap()
                .active(),
            replaced.active()
        );
    }
    // At the final minute, projection blocks new chains even while the cache remains visible.
    sqlx::query("update provider_turn_states set active_expires_at=now()+interval '59 seconds' where provider_account_id=$1")
        .bind(id.as_str()).execute(&database.pool).await.unwrap();
    let projection = admin.load_turn_state_status(&ids).await.unwrap();
    assert!(projection[id.as_str()].ready_models.is_empty());
    assert!(
        projection[id.as_str()]
            .models
            .iter()
            .any(|model| model.active.is_some())
    );
    suspect.expected_active_version = 2;
    suspect.promote_standby = true;
    let invalid = store.record_anomaly(suspect).await.unwrap();
    assert!(invalid.active().is_none());
    assert_eq!(invalid.state_version(), 3);
    database.close().await;
}

#[tokio::test]
async fn invalid_credentials_fence_late_candidates_without_deleting_cached_slots() {
    use gateway_admin::ports::store::AccountStore;
    let Some(database) = TestDatabase::create("turn_state_credential_gate").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .insert_provider_account(account("acct_gate", "gate-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let id = ProviderAccountId::new("acct_gate").unwrap();
    let model = UpstreamModelId::new("model-a").unwrap();
    let states = PgProviderTurnStateRepository::new(database.pool.clone());
    let admin = super::admin_account_store(&database.pool);
    let ids = [id.as_str().to_owned()];
    let value = candidate(
        &id,
        &model,
        &"a".repeat(292),
        SystemTime::now(),
        ProviderTurnStateSlot::Active,
        292,
    );
    states.put_candidate(value.clone()).await.unwrap();
    for (state, quota, expired) in [
        ("expired", "allowed", false),
        ("invalid", "allowed", false),
        ("banned", "allowed", false),
        ("ready", "allowed", true),
        ("ready", "exhausted", false),
    ] {
        sqlx::query(
            "update provider_accounts set credential_state=$1, quota_access_state=$2,
            quota_access_observed_at=now(), updated_at=now(),
            quota_evidence=case when $2='exhausted' then 'payment_required' else null end,
            access_token_expires_at=case when $3 then now()-interval '1 second' else null end
            where id=$4",
        )
        .bind(state)
        .bind(quota)
        .bind(expired)
        .bind(id.as_str())
        .execute(&database.pool)
        .await
        .unwrap();
        assert!(
            states
                .read(&id, &model, revision())
                .await
                .unwrap()
                .is_none()
        );
        assert!(states.put_candidate(value.clone()).await.is_err());
        let status = &admin.load_turn_state_status(&ids).await.unwrap()[id.as_str()];
        assert!(status.ready_models.is_empty());
        assert!(status.models.iter().any(|model| model.active.is_some()));
    }
    sqlx::query(
        "update provider_accounts set credential_state='ready', quota_access_state='allowed',
        quota_evidence=null, access_token_expires_at=null where id=$1",
    )
    .bind(id.as_str())
    .execute(&database.pool)
    .await
    .unwrap();
    assert!(
        states
            .read(&id, &model, revision())
            .await
            .unwrap()
            .unwrap()
            .active()
            .is_some()
    );
    assert_eq!(
        admin.load_turn_state_status(&ids).await.unwrap()[id.as_str()]
            .ready_models
            .len(),
        1
    );
    database.close().await;
}

#[tokio::test]
async fn soft_credential_cas_keeps_both_slots_clocks_readiness_and_hard_rotation_fence() {
    use gateway_admin::ports::store::AccountStore;
    use gateway_core::account::{
        CredentialCasOutcome, CredentialCasUpdate, ProviderAccountStore, ProviderAccountUpdate,
    };
    let Some(database) = TestDatabase::create("state_cookie_binding").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .insert_provider_account(account("acct_cookie", "cookie-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let id = ProviderAccountId::new("acct_cookie").unwrap();
    let states = PgProviderTurnStateRepository::new(database.pool.clone());
    let models = ["model-a", "model-b"].map(|m| UpstreamModelId::new(m).unwrap());
    let mut before = Vec::new();
    for model in &models {
        states
            .put_candidate(candidate(
                &id,
                model,
                &"a".repeat(292),
                SystemTime::now(),
                ProviderTurnStateSlot::Active,
                292,
            ))
            .await
            .unwrap();
        before.push(
            states
                .put_candidate(candidate(
                    &id,
                    model,
                    &"b".repeat(292),
                    SystemTime::now(),
                    ProviderTurnStateSlot::Standby,
                    292,
                ))
                .await
                .unwrap(),
        );
    }
    let loaded = accounts.load_current_credential(&id).await.unwrap();
    let make_update = |revision| {
        CredentialCasUpdate::new(
            id.clone(),
            revision,
            ProviderAccountUpdate {
                account_id: id.clone(),
                name: loaded.account.name().into(),
                email: loaded.account.email().map(str::to_owned),
                plan_type: loaded.account.plan_type().map(str::to_owned),
            },
            loaded.credential.clone(),
            false,
            loaded.account.access_token_expires_at(),
            None,
        )
        .unwrap()
        .preserving_profile()
    };
    for next in 2..=21 {
        assert_eq!(
            accounts
                .compare_and_swap_credential(
                    make_update(CredentialRevision::new(next - 1).unwrap())
                        .preserving_turn_state_binding()
                )
                .await
                .unwrap(),
            CredentialCasOutcome::Updated(CredentialRevision::new(next).unwrap())
        );
        let current = accounts.get_account(&id).await.unwrap().unwrap();
        assert_eq!(current.turn_state_binding_revision(), revision());
        for (model, original) in models.iter().zip(&before) {
            let after = states
                .read(&id, model, current.turn_state_binding_revision())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(after.active(), original.active());
            assert_eq!(after.standby(), original.standby());
            assert_eq!(after.state_version(), original.state_version());
            assert_eq!(
                after.last_observed_length(),
                original.last_observed_length()
            );
        }
    }
    let admin = super::admin_account_store(&database.pool);
    let ids = [id.as_str().to_owned()];
    assert_eq!(
        admin.load_turn_state_status(&ids).await.unwrap()[id.as_str()]
            .ready_models
            .len(),
        2
    );
    // Two writers based on the same revision cannot both win, including a soft save.
    let hard = make_update(CredentialRevision::new(21).unwrap());
    let soft = hard.clone().preserving_turn_state_binding();
    assert!(matches!(
        accounts.compare_and_swap_credential(hard).await.unwrap(),
        CredentialCasOutcome::Updated(_)
    ));
    assert_eq!(
        accounts.compare_and_swap_credential(soft).await.unwrap(),
        CredentialCasOutcome::Conflict
    );
    let current = accounts.get_account(&id).await.unwrap().unwrap();
    assert_eq!(current.turn_state_binding_revision().get(), 22);
    assert!(
        admin.load_turn_state_status(&ids).await.unwrap()[id.as_str()]
            .ready_models
            .is_empty()
    );
    for model in &models {
        assert!(states.read(&id, model, revision()).await.unwrap().is_none());
        assert!(
            states
                .put_candidate(candidate(
                    &id,
                    model,
                    &"c".repeat(292),
                    SystemTime::now(),
                    ProviderTurnStateSlot::Active,
                    292
                ))
                .await
                .is_err()
        );
    }
    database.close().await;
}

#[tokio::test]
async fn binding_migration_preserves_current_rows_without_reviving_stale_rows() {
    let Some(database) = TestDatabase::create("state_binding_upgrade").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    for id in ["acct_current", "acct_stale"] {
        accounts
            .insert_provider_account(account(id, id))
            .await
            .unwrap();
    }
    enable(&database).await;
    let states = PgProviderTurnStateRepository::new(database.pool.clone());
    let model = UpstreamModelId::new("model-a").unwrap();
    for id in ["acct_current", "acct_stale"] {
        states
            .put_candidate(candidate(
                &ProviderAccountId::new(id).unwrap(),
                &model,
                &"a".repeat(292),
                SystemTime::now(),
                ProviderTurnStateSlot::Active,
                292,
            ))
            .await
            .unwrap();
    }
    // Simulate the immediately preceding schema without rewriting frozen migrations.
    let mut tx = database.pool.begin().await.unwrap();
    sqlx::query("alter table provider_accounts drop column turn_state_binding_revision")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("update provider_accounts set credential_revision=2 where id='acct_stale'")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../migrations/0027_turn_state_binding_revision.sql"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(
        states
            .read(
                &ProviderAccountId::new("acct_current").unwrap(),
                &model,
                revision()
            )
            .await
            .unwrap()
            .unwrap()
            .active()
            .is_some()
    );
    let stale = ProviderAccountId::new("acct_stale").unwrap();
    assert!(
        states
            .read(&stale, &model, revision())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        states
            .read(&stale, &model, CredentialRevision::new(2).unwrap())
            .await
            .unwrap()
            .is_none()
    );
    database.close().await;
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
    let model_status = statuses[id.as_str()]
        .models
        .iter()
        .find(|status| status.model == "model-a")
        .unwrap();
    assert_eq!(model_status.refresh_status, "ready");
    assert!(model_status.active.as_ref().unwrap().captured_at.is_some());
    assert_eq!(model_status.active.as_ref().unwrap().chars, 292);
    assert!(model_status.standby.is_none());
    store
        .put_candidate(candidate(
            &id,
            &model,
            &"b".repeat(292),
            SystemTime::now(),
            ProviderTurnStateSlot::Standby,
            292,
        ))
        .await
        .unwrap();
    let statuses = admin.load_turn_state_status(&ids).await.unwrap();
    let model_status = statuses[id.as_str()]
        .models
        .iter()
        .find(|status| status.model == "model-a")
        .unwrap();
    assert_eq!(model_status.refresh_status, "ready");
    assert_eq!(model_status.active.as_ref().unwrap().chars, 292);
    assert_eq!(model_status.standby.as_ref().unwrap().chars, 292);
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
    let disabled = admin.load_turn_state_status(&ids).await.unwrap();
    assert!(!disabled[id.as_str()].enabled);
    assert!(disabled[id.as_str()].ready_models.is_empty());
    assert!(
        disabled[id.as_str()]
            .models
            .iter()
            .any(|model| model.active.is_some())
    );
    let status: String = sqlx::query_scalar("select refresh_status from provider_turn_states")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(status, "missing");
    enable(&database).await;
    sqlx::query(
        "update provider_accounts set credential_revision=2, turn_state_binding_revision=2",
    )
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
async fn proactive_promotion_preserves_clock_and_rejects_late_or_short_lived_standby() {
    let Some(database) = TestDatabase::create("turn_state_proactive").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .insert_provider_account(account("acct_promote", "promote-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let store = PgProviderTurnStateRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_promote").unwrap();
    let model = UpstreamModelId::new("model-a").unwrap();
    let now = SystemTime::now();
    let active = candidate(
        &id,
        &model,
        &"a".repeat(292),
        now - Duration::from_secs(3550),
        ProviderTurnStateSlot::Active,
        292,
    );
    let standby = candidate(
        &id,
        &model,
        &"b".repeat(292),
        now - Duration::from_secs(1800),
        ProviderTurnStateSlot::Standby,
        292,
    );
    store.put_candidate(active).await.unwrap();
    let before = store.put_candidate(standby.clone()).await.unwrap();
    let promotion = ProviderTurnStatePromotion {
        account_id: id.clone(),
        expected_revision: revision(),
        expected_active_version: before.state_version(),
        upstream_model: model.clone(),
        normal_length: 292,
        observed_at: now,
        minimum_remaining: Duration::from_secs(60),
    };
    let mut too_late = promotion.clone();
    too_late.observed_at = now + Duration::from_secs(1750);
    assert!(store.promote_standby(too_late).await.unwrap().is_none());
    let promoted = store
        .promote_standby(promotion.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(promoted.active(), before.standby());
    assert!(promoted.standby().is_none());
    assert_eq!(promoted.state_version(), before.state_version() + 1);
    assert!(
        store
            .promote_standby(promotion.clone())
            .await
            .unwrap()
            .is_none()
    );
    let echo = candidate(
        &id,
        &model,
        &"b".repeat(292),
        now,
        ProviderTurnStateSlot::Active,
        292,
    );
    let echoed = store.put_candidate(echo).await.unwrap();
    assert_eq!(echoed.active(), promoted.active());
    assert_eq!(echoed.state_version(), promoted.state_version());
    sqlx::query(
        "update provider_accounts set credential_revision = 2, turn_state_binding_revision = 2",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert!(store.promote_standby(promotion).await.is_err());
    database.close().await;
}

#[tokio::test]
async fn a_standby_echo_cannot_renew_its_original_capture_lifetime() {
    let Some(database) = TestDatabase::create("turn_state_echo_clock").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    accounts
        .insert_provider_account(account("acct_echo_clock", "echo-clock-owner"))
        .await
        .unwrap();
    enable(&database).await;
    let store = PgProviderTurnStateRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_echo_clock").unwrap();
    let model = UpstreamModelId::new("model-a").unwrap();
    let now = SystemTime::now();
    store
        .put_candidate(candidate(
            &id,
            &model,
            &"a".repeat(292),
            now - Duration::from_secs(3550),
            ProviderTurnStateSlot::Active,
            292,
        ))
        .await
        .unwrap();
    let before = store
        .put_candidate(candidate(
            &id,
            &model,
            &"b".repeat(292),
            now - Duration::from_secs(1800),
            ProviderTurnStateSlot::Standby,
            292,
        ))
        .await
        .unwrap();
    let echoed = store
        .put_candidate(candidate(
            &id,
            &model,
            &"b".repeat(292),
            now,
            ProviderTurnStateSlot::Active,
            292,
        ))
        .await
        .unwrap();
    assert_eq!(echoed.active(), before.standby());
    let mut expired_echo = candidate(
        &id,
        &model,
        &"b".repeat(292),
        now,
        ProviderTurnStateSlot::Active,
        292,
    );
    expired_echo.observed_at = now + Duration::from_secs(2000);
    let old = store.put_candidate(expired_echo).await.unwrap();
    assert_eq!(
        old.active(),
        echoed.active(),
        "expired standby echo cannot be renewed"
    );
    database.close().await;
}

#[tokio::test]
async fn repeated_active_is_idempotent_and_replacement_does_not_recycle_old_active() {
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

    let early = store
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
    assert_eq!(
        early.active(),
        first.active(),
        "a fresh active cannot be replaced early"
    );
    let replaced = store
        .put_candidate(candidate(
            &account_id,
            &model,
            &second_value,
            issued_at + Duration::from_secs(3550),
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
    assert!(replaced.standby().is_none());
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
            suspect: true,
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
        ProviderTurnStateRefreshStatus::Ready
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
async fn empty_active_cannot_acquire_a_standby_and_future_skew_is_bounded() {
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
        now - Duration::from_secs(31),
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
                suspect: true,
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
        now + Duration::from_secs(31),
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
    sqlx::query(
        "update provider_accounts set credential_revision = 2, turn_state_binding_revision = 2",
    )
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
            .put_candidate(candidate(
                &id,
                &model,
                &value.repeat(292),
                now + Duration::from_secs(u64::from(slot == ProviderTurnStateSlot::Standby)),
                slot,
                292,
            ))
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
        suspect: true,
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
            .put_candidate(candidate(
                &id,
                &model,
                &value.repeat(292),
                now + Duration::from_secs(u64::from(slot == ProviderTurnStateSlot::Standby)),
                slot,
                292,
            ))
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
            suspect: true,
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
            suspect: true,
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
