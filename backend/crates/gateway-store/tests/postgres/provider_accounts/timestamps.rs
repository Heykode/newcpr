use super::*;
use chrono::DateTime;
use gateway_store::postgres::RecoverProviderAccount;
use serde_json::Value;

async fn seed(database: &TestDatabase, id: &str, enabled: bool) -> DateTime<Utc> {
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let mut candidate = account(id, id);
    candidate.enabled = enabled;
    repository.insert_provider_account(candidate).await.unwrap();
    // Simulate a backwards wall clock without changing the machine clock or sleeping.
    sqlx::query_scalar(
        "update provider_accounts
         set created_at = now() + interval '1 hour',
             credential_observed_at = now() + interval '1 hour 1 second',
             quota_observed_at = now() + interval '1 hour 2 seconds',
             quota_access_observed_at = now() + interval '1 hour 3 seconds',
             quota_access_state = 'allowed',
             provider_quota_json = '{\"marker\":\"original\"}'::jsonb,
             updated_at = now() + interval '1 hour 4 seconds'
         where id = $1 returning created_at",
    )
    .bind(id)
    .fetch_one(&database.pool)
    .await
    .unwrap()
}

async fn row(database: &TestDatabase, id: &str) -> Value {
    sqlx::query_scalar("select to_jsonb(a) from provider_accounts a where id = $1")
        .bind(id)
        .fetch_one(&database.pool)
        .await
        .unwrap()
}

fn unchanged_except(mut before: Value, mut after: Value, fields: &[&str]) {
    for field in fields {
        before.as_object_mut().unwrap().remove(*field);
        after.as_object_mut().unwrap().remove(*field);
    }
    assert_eq!(before, after);
}

#[tokio::test]
async fn normal_credential_updates_still_advance_time() {
    let Some(database) = TestDatabase::create("time_normal_cas").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let id = "acct_time_normal";
    repository
        .insert_provider_account(account(id, id))
        .await
        .unwrap();
    let before: DateTime<Utc> = sqlx::query_scalar(
        "update provider_accounts
         set created_at = now() - interval '3 hours',
             credential_observed_at = now() - interval '2 hours',
             updated_at = now() - interval '1 hour'
         where id = $1 returning updated_at",
    )
    .bind(id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    repository
        .compare_and_swap_credentials(credential_update(id, 1, "synthetic-normal"))
        .await
        .unwrap();
    let after: DateTime<Utc> =
        sqlx::query_scalar("select updated_at from provider_accounts where id = $1")
            .bind(id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(after > before);
    database.close().await;
}

#[tokio::test]
async fn repository_credential_cas_keeps_time_and_stale_revision_fencing() {
    let Some(database) = TestDatabase::create("time_credential_cas").await else {
        return;
    };
    let id = "acct_time_cas";
    seed(&database, id, true).await;
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let before = row(&database, id).await;
    let command = credential_update(id, 1, "synthetic-new");
    assert_eq!(
        repository
            .compare_and_swap_credentials(command.clone())
            .await
            .unwrap()
            .get(),
        2
    );
    let after = row(&database, id).await;
    assert_eq!(after["turn_state_binding_revision"], 2);
    unchanged_except(
        before,
        after.clone(),
        &[
            "provider_credentials_json",
            "credential_revision",
            "turn_state_binding_revision",
            "access_token_expires_at",
        ],
    );
    assert!(matches!(
        repository.compare_and_swap_credentials(command).await,
        Err(StoreError::Conflict {
            kind: ConflictKind::StaleRevision,
            ..
        })
    ));
    assert_eq!(row(&database, id).await, after);

    let (left, right) = tokio::join!(
        repository.compare_and_swap_credentials(credential_update(id, 2, "synthetic-left")),
        repository.compare_and_swap_credentials(credential_update(id, 2, "synthetic-right")),
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let rejected = if left.is_err() { left } else { right };
    assert!(matches!(
        rejected,
        Err(StoreError::Conflict {
            kind: ConflictKind::StaleRevision,
            ..
        })
    ));
    let final_row = row(&database, id).await;
    assert_eq!(final_row["credential_revision"], 3);
    assert_eq!(final_row["turn_state_binding_revision"], 3);
    assert_eq!(final_row["updated_at"], after["updated_at"]);
    database.close().await;
}

#[tokio::test]
async fn core_credential_cas_preserves_other_observation_clocks_and_disabled_state() {
    let Some(database) = TestDatabase::create("time_core_cas").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    for (enabled, with_state) in [(true, false), (true, true), (false, false), (false, true)] {
        let id = ProviderAccountId::new(format!("acct_core_time_{enabled}_{with_state}")).unwrap();
        let base = seed(&database, id.as_str(), enabled).await;
        let before = row(&database, id.as_str()).await;
        let current = repository.get_account(&id).await.unwrap().unwrap();
        let mut update = CredentialCasUpdate::new(
            id.clone(),
            current.revision(),
            ProviderAccountUpdate {
                account_id: id.clone(),
                name: "Must not replace profile".into(),
                email: None,
                plan_type: None,
            },
            plaintext_credential("synthetic-core-refresh"),
            false,
            current.access_token_expires_at(),
            None,
        )
        .unwrap()
        .preserving_profile();
        if with_state {
            update = update.with_account_state(
                CredentialState::Ready,
                (base + TimeDelta::milliseconds(1500)).into(),
                None,
                None,
            );
        }
        assert!(matches!(
            repository
                .compare_and_swap_credential(update.clone())
                .await
                .unwrap(),
            CredentialCasOutcome::Updated(_)
        ));
        let after = row(&database, id.as_str()).await;
        assert_eq!(after["turn_state_binding_revision"], 2);
        let mut changed = vec![
            "provider_credentials_json",
            "credential_revision",
            "turn_state_binding_revision",
        ];
        if enabled && with_state {
            changed.push("credential_observed_at");
        }
        unchanged_except(before, after.clone(), &changed);
        assert!(matches!(
            repository
                .compare_and_swap_credential(update)
                .await
                .unwrap(),
            CredentialCasOutcome::Conflict
        ));
        assert_eq!(row(&database, id.as_str()).await, after);
    }
    database.close().await;
}

#[tokio::test]
async fn admin_rotation_keeps_retained_quota_times_and_disabled_identity() {
    let Some(database) = TestDatabase::create("time_rotation").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    for enabled in [true, false] {
        let id = format!("acct_rotate_time_{enabled}");
        seed(&database, &id, enabled).await;
        let before = row(&database, &id).await;
        let command = RotateProviderAccount {
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".into(),
            },
            profile: profile(&id, "Updated profile"),
            replacement_identity: None,
            credential: credential_update(&id, 1, "synthetic-rotated"),
            relogin_operation_id: None,
            audit: audit(&format!("audit_time_{enabled}"), "rotate", &id),
        };
        repository
            .rotate_provider_account(command.clone())
            .await
            .unwrap();
        let after = row(&database, &id).await;
        let changed = [
            "name",
            "email",
            "plan_type",
            "provider_credentials_json",
            "credential_revision",
            "turn_state_binding_revision",
            "access_token_expires_at",
            "credential_observed_at",
        ];
        assert_eq!(after["enabled"], enabled);
        assert_eq!(after["turn_state_binding_revision"], 2);
        assert_eq!(after["updated_at"], before["updated_at"]);
        assert_eq!(after["credential_state"], "ready");
        let observed: DateTime<Utc> =
            serde_json::from_value(after["credential_observed_at"].clone()).unwrap();
        let previous: DateTime<Utc> =
            serde_json::from_value(before["credential_observed_at"].clone()).unwrap();
        // Replacement credentials start a new observation clock even while manually paused.
        assert!(observed < previous);
        unchanged_except(before, after.clone(), &changed);
        assert!(matches!(
            repository.rotate_provider_account(command).await,
            Err(StoreError::Conflict {
                kind: ConflictKind::StaleRevision,
                ..
            })
        ));
        assert_eq!(row(&database, &id).await, after);
    }
    database.close().await;
}

#[tokio::test]
async fn profile_enable_and_recovery_writes_do_not_move_update_time_backwards() {
    let Some(database) = TestDatabase::create("time_profile").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let id = "acct_profile_time";
    seed(&database, id, true).await;
    let before = row(&database, id).await;
    assert!(
        repository
            .update_provider_account(profile(id, "Updated"))
            .await
            .unwrap()
    );
    let renamed = row(&database, id).await;
    unchanged_except(before, renamed.clone(), &["name", "email", "plan_type"]);
    assert!(
        repository
            .set_provider_account_enabled(id, false)
            .await
            .unwrap()
    );
    let disabled = row(&database, id).await;
    unchanged_except(renamed, disabled.clone(), &["enabled"]);
    repository
        .recover_provider_account_admin(RecoverProviderAccount {
            account_id: id.into(),
            audit: audit("audit_time_recovery", "recover", id),
        })
        .await
        .unwrap();
    let recovered = row(&database, id).await;
    assert_eq!(recovered["enabled"], true);
    assert_eq!(recovered["credential_state"], "ready");
    assert_eq!(recovered["provider_quota_json"], Value::Null);
    unchanged_except(
        disabled,
        recovered,
        &[
            "enabled",
            "credential_observed_at",
            "provider_quota_json",
            "quota_observed_at",
            "quota_access_observed_at",
        ],
    );
    database.close().await;
}

#[tokio::test]
async fn independent_observation_updates_keep_time_and_reject_stale_observations() {
    let Some(database) = TestDatabase::create("time_observations").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    for diagnostic in [false, true] {
        let id = ProviderAccountId::new(format!("acct_state_time_{diagnostic}")).unwrap();
        let base = seed(&database, id.as_str(), !diagnostic).await;
        let before = row(&database, id.as_str()).await;
        let change = AccountStateChange {
            account_id: id.clone(),
            expected_revision: CredentialRevision::new(1).unwrap(),
            credential_state: CredentialState::Invalid,
            observed_at: (base + TimeDelta::milliseconds(1500)).into(),
            error_reason: Some(AccountErrorReason::CredentialInvalid),
            message: Some("Synthetic rejection".into()),
        };
        let apply = |change| async {
            if diagnostic {
                repository.apply_diagnostic_state_change(change).await
            } else {
                repository.apply_state_change(change).await
            }
        };
        apply(change.clone()).await.unwrap();
        let after = row(&database, id.as_str()).await;
        unchanged_except(
            before,
            after.clone(),
            &[
                "credential_state",
                "credential_observed_at",
                "last_error_reason",
                "last_error_message",
            ],
        );
        let stale = AccountStateChange {
            observed_at: base.into(),
            ..change
        };
        assert!(apply(stale).await.is_err());
        assert_eq!(row(&database, id.as_str()).await, after);
    }

    let id = "acct_quota_time";
    let base = seed(&database, id, true).await;
    let revision = Revision::new(1).unwrap();
    let before = row(&database, id).await;
    let observed = base + TimeDelta::milliseconds(2500);
    assert!(
        repository
            .compare_and_swap_provider_quota(
                id,
                revision,
                credential_json("synthetic-quota"),
                observed,
                QuotaState::unknown(),
                None,
            )
            .await
            .unwrap()
    );
    let after = row(&database, id).await;
    unchanged_except(
        before,
        after.clone(),
        &["provider_quota_json", "quota_observed_at"],
    );
    assert!(
        !repository
            .compare_and_swap_provider_quota(
                id,
                revision,
                credential_json("stale-quota"),
                base,
                QuotaState::unknown(),
                None,
            )
            .await
            .unwrap()
    );
    assert_eq!(row(&database, id).await, after);
    assert!(repository.touch_provider_quota_observation(
        id, revision, observed + TimeDelta::milliseconds(100),
    ).await.unwrap());
    let touched = row(&database, id).await;
    unchanged_except(after, touched.clone(), &["quota_observed_at"]);
    let access = base + TimeDelta::milliseconds(3500);
    assert!(
        repository
            .apply_provider_quota_access(id, revision, QuotaState::allowed(access.into()),)
            .await
            .unwrap()
    );
    let accessed = row(&database, id).await;
    unchanged_except(touched, accessed.clone(), &["quota_access_observed_at"]);
    assert!(
        !repository
            .apply_provider_quota_access(id, revision, QuotaState::allowed(base.into()),)
            .await
            .unwrap()
    );
    assert_eq!(row(&database, id).await, accessed);

    // New access evidence can be newer than the quota document observation itself.
    let newest_access = base + TimeDelta::seconds(5);
    assert!(
        repository
            .compare_and_swap_provider_quota(
                id,
                revision,
                credential_json("newer-quota"),
                base + TimeDelta::seconds(3),
                QuotaState::allowed(newest_access.into()),
                None,
            )
            .await
            .unwrap()
    );
    let (updated, access_observed): (DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "select updated_at, quota_access_observed_at from provider_accounts where id = $1",
    )
    .bind(id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!((updated, access_observed), (newest_access, newest_access));

    let before_stale_access = row(&database, id).await;
    let reset_at = base + TimeDelta::hours(1);
    assert!(
        repository
            .compare_and_swap_provider_quota(
                id,
                revision,
                credential_json("fresh-document-stale-access"),
                base + TimeDelta::seconds(4),
                QuotaState::exhausted(
                    QuotaEvidence::UsageLimitReached,
                    access.into(),
                    Some(reset_at.into()),
                ),
                None,
            )
            .await
            .unwrap()
    );
    let after_stale_access = row(&database, id).await;
    unchanged_except(
        before_stale_access,
        after_stale_access.clone(),
        &["provider_quota_json", "quota_observed_at"],
    );
    assert_eq!(after_stale_access["quota_access_state"], "allowed");

    let next_access = base + TimeDelta::seconds(6);
    let next_document = base + TimeDelta::milliseconds(4500);
    let (access_result, touch_result) = tokio::join!(
        repository.apply_provider_quota_access(
            id,
            revision,
            QuotaState::exhausted(
                QuotaEvidence::UsageLimitReached,
                next_access.into(),
                Some(reset_at.into()),
            ),
        ),
        repository.touch_provider_quota_observation(id, revision, next_document),
    );
    assert!(access_result.unwrap());
    assert!(touch_result.unwrap());
    let stored: (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "select updated_at, quota_access_observed_at, quota_observed_at, quota_reset_at
             from provider_accounts where id = $1",
    )
    .bind(id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(stored, (next_access, next_access, next_document, reset_at));
    database.close().await;
}
