use super::*;

#[tokio::test]
async fn replacement_clears_old_authentication_failure_but_import_uses_explicit_enable_value() {
    let Some(database) = TestDatabase::create("replacement_manual_intent").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    for enabled in [true, false] {
        let id = if enabled { "acct_auto" } else { "acct_manual" };
        let scope = ProviderAccountAdminScope {
            provider_kind: "openai".into(),
        };
        repository
            .insert_provider_account(account(id, id))
            .await
            .unwrap();
        repository
            .apply_state_change(AccountStateChange {
                account_id: ProviderAccountId::new(id).unwrap(),
                expected_revision: CredentialRevision::new(1).unwrap(),
                credential_state: CredentialState::Expired,
                observed_at: SystemTime::now(),
                error_reason: Some(AccountErrorReason::CredentialExpired),
                message: Some("synthetic old credential failure".into()),
            })
            .await
            .unwrap();
        // The administrator may pause after relogin has captured the credential revision.
        repository
            .set_provider_account_enabled(id, enabled)
            .await
            .unwrap();
        repository
            .rotate_provider_account(RotateProviderAccount {
                relogin_operation_id: None,
                scope: scope.clone(),
                profile: profile(id, "verified replacement"),
                replacement_identity: None,
                credential: credential_update(id, 1, "synthetic-new-access"),
                audit: audit(&format!("audit_rotate_{id}"), "rotate", id),
            })
            .await
            .unwrap();
        let current = repository
            .load_current_credential(&ProviderAccountId::new(id).unwrap())
            .await
            .unwrap()
            .account;
        assert_eq!(current.enabled(), enabled);
        assert_eq!(current.credential_state(), CredentialState::Ready);
        assert_eq!(current.last_error_reason(), None);
        assert_eq!(current.last_error_message(), None);
        assert_eq!(
            current.status_projection(SystemTime::now(), None).status,
            if enabled {
                AccountStatus::Normal
            } else {
                AccountStatus::Disabled
            }
        );
        let stale = repository
            .apply_state_change(AccountStateChange {
                account_id: current.id().clone(),
                expected_revision: CredentialRevision::new(1).unwrap(),
                credential_state: CredentialState::Expired,
                observed_at: SystemTime::now(),
                error_reason: Some(AccountErrorReason::CredentialExpired),
                message: Some("late old request".into()),
            })
            .await;
        assert!(stale.is_err());

        let mut imported = account(id, id);
        imported.enabled = enabled;
        repository
            .import_provider_accounts(ImportProviderAccounts {
                settings: Some(gateway_admin::model::accounts::AccountImportSettings {
                    model_access: Default::default(),
                    custom_name: None,
                    enabled: !enabled,
                    turn_state_injection_enabled: None,
                    responses_upstream: Default::default(),
                    excel_models_follow_global: Default::default(),
                    excel_models: Default::default(),
                    concurrency_limit: None,
                    weight: gateway_core::account::AccountWeight::DEFAULT,
                    group_ids: vec![],
                }),
                outbound_proxy: None,
                scope,
                accounts: vec![imported],
                audit: audit(&format!("audit_import_{id}"), "import", id),
            })
            .await
            .unwrap();
        let imported = repository
            .load_current_credential(current.id())
            .await
            .unwrap()
            .account;
        assert_eq!(imported.enabled(), !enabled);
    }
    database.close().await;
}

#[tokio::test]
async fn rejected_oauth_refresh_candidates_match_core_policy() {
    let Some(database) = TestDatabase::create("rejected_oauth_candidates").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let now = Utc::now();
    for (index, expiry, enabled, refresh, reason, retry) in [
        (
            0,
            Some(now + TimeDelta::days(1)),
            true,
            true,
            AccountErrorReason::AccessTokenExpired,
            None,
        ),
        (
            1,
            None,
            true,
            true,
            AccountErrorReason::AccessTokenExpired,
            None,
        ),
        (
            2,
            Some(now),
            false,
            true,
            AccountErrorReason::AccessTokenExpired,
            None,
        ),
        (
            3,
            Some(now),
            true,
            false,
            AccountErrorReason::AccessTokenExpired,
            None,
        ),
        (
            4,
            Some(now),
            true,
            true,
            AccountErrorReason::CredentialExpired,
            None,
        ),
        (
            5,
            Some(now - TimeDelta::days(1)),
            true,
            true,
            AccountErrorReason::AccessTokenExpired,
            Some(now + TimeDelta::hours(1)),
        ),
    ] {
        let id = format!("acct_auth_{index}");
        let mut seed = account(&id, &format!("user-auth-{index}"));
        seed.has_refresh_token = refresh;
        seed.access_token_expires_at = expiry;
        seed.next_refresh_at = retry;
        repository.insert_provider_account(seed).await.unwrap();
        repository
            .apply_state_change(AccountStateChange {
                account_id: ProviderAccountId::new(&id).unwrap(),
                expected_revision: CredentialRevision::new(1).unwrap(),
                credential_state: CredentialState::Expired,
                observed_at: SystemTime::now(),
                error_reason: Some(reason),
                message: Some("synthetic authentication rejection".into()),
            })
            .await
            .unwrap();
        repository
            .set_provider_account_enabled(&id, enabled)
            .await
            .unwrap();
    }
    let query = ProviderRefreshQuery::new(
        ProviderKind::new("openai").unwrap(),
        (now + TimeDelta::minutes(5)).into(),
        (now - TimeDelta::hours(2)).into(),
        now.into(),
        vec![],
        NonZeroU32::new(100).unwrap(),
    );
    let candidates = repository
        .list_refresh_candidates(query.clone())
        .await
        .unwrap();
    let mut ids = candidates
        .iter()
        .map(|row| row.account.id().as_str())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    assert_eq!(ids, ["acct_auth_0", "acct_auth_1"]);
    for index in 0..6 {
        let id = ProviderAccountId::new(format!("acct_auth_{index}")).unwrap();
        let row = repository.load_current_credential(&id).await.unwrap();
        assert_eq!(query.contains(&row.account), index < 2);
        assert_ne!(
            row.account
                .status_projection(SystemTime::now(), None)
                .status,
            AccountStatus::Normal
        );
    }
    database.close().await;
}
