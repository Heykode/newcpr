use super::*;
use gateway_admin::model::accounts::ExcelCredentialImportTarget;
use gateway_core::account::{Excel403Action, ExcelCredentialCasUpdate, ResponsesUpstream};

pub(super) fn prepared(id: &str, user: &str, workspace: &str) -> PreparedCredentialCreate {
    PreparedCredentialCreate {
        model_access: None,
        outbound_proxy: None,
        account_id: ProviderAccountId::new(id).unwrap(),
        provider_kind: ProviderKind::new("openai").unwrap(),
        name: "Excel fixture".into(),
        email: Some("fixture@example.invalid".into()),
        upstream_user_id: Some(user.into()),
        upstream_account_id: Some(workspace.into()),
        plan_type: Some("pro".into()),
        authentication_kind: "excel_oauth".into(),
        provider_material: ProviderDocument::new(OpaqueProviderData::new(
            json!({"excel":"fixture"}).as_object().unwrap().clone(),
        )),
        has_refresh_token: true,
        access_token_expires_at: Some(Utc::now() + TimeDelta::hours(1)),
        next_refresh_at: Some(Utc::now() - TimeDelta::minutes(1)),
        enabled: true,
        credential_state: CredentialState::Ready,
        credential_observed_at: Utc::now(),
    }
}

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: uuid::Uuid::new_v4().to_string(),
    }
}

pub(super) async fn seed_sidecar(db: &TestDatabase, id: &str) {
    sqlx::query("insert into openai_excel_credentials(account_id,upstream_user_id,upstream_account_id,
        credential_json,credential_state,credential_observed_at,access_token_expires_at,updated_at)
        select id,upstream_user_id,upstream_account_id,'{}','ready',now(),now()+interval '1 hour',now()
        from provider_accounts where id=$1 on conflict do nothing")
        .bind(id).execute(&db.pool).await.unwrap();
}

#[tokio::test]
async fn excel_sidecar_preserves_codex_and_refresh_claim_is_once_per_revision() {
    let Some(db) = TestDatabase::create("excel_sidecar").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    let admin = admin_account_store(&db.pool);
    let id = ProviderAccountId::new("acct_dual").unwrap();
    let mut seed = account(id.as_str(), "user-dual");
    seed.upstream_account_id = Some("workspace-dual".into());
    seed.weight = gateway_core::account::AccountWeight::new(7).unwrap();
    repo.insert_provider_account(seed).await.unwrap();
    let before: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a where id=$1")
            .bind(id.as_str())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let result = admin
        .commit_excel_credential_import(
            prepared("acct_incoming", "user-dual", "workspace-dual"),
            None,
            None,
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(result.account.id, id.as_str());
    assert!(result.account.has_codex_credential());
    let metadata = result.account.excel_credential.unwrap();
    assert_eq!(metadata.revision.get(), 1);
    assert_eq!(
        repo.list_due_excel_credentials(10, SystemTime::now())
            .await
            .unwrap(),
        vec![id.clone()]
    );
    assert!(
        repo.claim_excel_refresh(&id, metadata.revision)
            .await
            .unwrap()
    );
    assert!(
        !repo
            .claim_excel_refresh(&id, metadata.revision)
            .await
            .unwrap()
    );
    assert!(
        repo.list_due_excel_credentials(10, SystemTime::now())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repo.load_current_excel_credential(&id)
            .await
            .unwrap()
            .metadata
            .refresh_attempted
    );
    let update = ExcelCredentialCasUpdate {
        account_id: id.clone(),
        expected_revision: metadata.revision,
        identity: ProviderAccountIdentity::new("user-dual".into(), Some("workspace-dual".into())),
        credential: PlaintextCredential::new(
            json!({"excel":"rotated"}).as_object().unwrap().clone(),
        ),
        email: None,
        plan_type: None,
        has_refresh_token: true,
        access_token_expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
        next_refresh_at: Some(SystemTime::now() + Duration::from_secs(3000)),
        observed_at: SystemTime::now(),
    };
    assert_eq!(
        repo.compare_and_swap_excel_credential(update.clone())
            .await
            .unwrap(),
        CredentialCasOutcome::Updated(CredentialRevision::new(2).unwrap())
    );
    assert_eq!(
        repo.compare_and_swap_excel_credential(update)
            .await
            .unwrap(),
        CredentialCasOutcome::Conflict
    );
    assert!(
        !repo
            .load_current_excel_credential(&id)
            .await
            .unwrap()
            .metadata
            .refresh_attempted
    );
    let stale = admin
        .commit_excel_credential_import(
            prepared("acct_incoming", "user-dual", "workspace-dual"),
            Some(ExcelCredentialImportTarget {
                account_id: id.to_string(),
                expected_revision: Some(metadata.revision),
            }),
            None,
            &context(),
        )
        .await;
    assert!(stale.is_err());
    repo.apply_excel_state_change(AccountStateChange {
        account_id: id.clone(),
        expected_revision: CredentialRevision::new(2).unwrap(),
        credential_state: CredentialState::Invalid,
        observed_at: SystemTime::now(),
        error_reason: Some(AccountErrorReason::AccessTokenExpired),
        message: Some("Excel fixture expired".into()),
    })
    .await
    .unwrap();
    let after: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a where id=$1")
            .bind(id.as_str())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        before, after,
        "Excel writes must not mutate any Codex/shared column"
    );
    assert_eq!(
        repo.get_account(&id)
            .await
            .unwrap()
            .unwrap()
            .credential_state(),
        CredentialState::Ready
    );
    repo.set_enabled(&id, false).await.unwrap();
    repo.delete_account(&id).await.unwrap();
    let count: i64 = sqlx::query_scalar("select count(*) from openai_excel_credentials")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    db.close().await;
}

#[tokio::test]
async fn excel_only_shell_has_no_codex_material_and_rejects_workspace_rebind() {
    let Some(db) = TestDatabase::create("excel_shell").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    let admin = admin_account_store(&db.pool);
    let result = admin
        .commit_excel_credential_import(
            prepared("acct_excel_only", "user-only", "workspace-only"),
            None,
            None,
            &context(),
        )
        .await
        .unwrap();
    let id = ProviderAccountId::new(result.account.id.clone()).unwrap();
    assert!(!result.account.has_codex_credential());
    assert_eq!(result.account.responses_upstream, ResponsesUpstream::Codex);
    let codex = admin
        .load_account(id.as_str(), Default::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        codex.projection.status,
        AccountStatus::Error,
        "Excel-only accounts cannot display Codex ready"
    );
    sqlx::query("update provider_accounts set responses_upstream='excel' where id=$1")
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .unwrap();
    let active = admin
        .load_account(id.as_str(), Default::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.projection.status, AccountStatus::Normal);
    let loaded = repo
        .load_provider_account(id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(loaded.provider_credentials_json.fields().is_empty());
    assert!(!loaded.summary.has_refresh_token);
    let target = ExcelCredentialImportTarget {
        account_id: id.to_string(),
        expected_revision: Some(CredentialRevision::new(1).unwrap()),
    };
    assert!(
        admin
            .commit_excel_credential_import(
                prepared("acct_wrong", "user-only", "other-workspace"),
                Some(target),
                None,
                &context()
            )
            .await
            .is_err()
    );
    let revision = repo
        .rotate_provider_account(RotateProviderAccount {
            relogin_operation_id: None,
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".into(),
            },
            profile: profile(id.as_str(), "Codex added"),
            replacement_identity: Some(ProviderAccountIdentity::new(
                "user-only".into(),
                Some("workspace-only".into()),
            )),
            credential: credential_update(id.as_str(), 1, "codex-fixture"),
            audit: audit("promote-shell", "rotate", id.as_str()),
        })
        .await
        .unwrap();
    assert_eq!(revision.credential_revision.get(), 2);
    let dual = repo.get_account(&id).await.unwrap().unwrap();
    assert!(dual.has_codex_credential());
    assert_eq!(dual.excel_credential().unwrap().revision.get(), 1);
    assert_eq!(dual.responses_upstream(), ResponsesUpstream::Excel);
    db.close().await;
}

#[tokio::test]
async fn excel_403_suspends_excel_without_disabling_codex_or_switching_entry() {
    let Some(db) = TestDatabase::create("excel_403_isolation").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    let admin = admin_account_store(&db.pool);
    let mut seed = account("acct_403_dual", "user-403");
    seed.upstream_account_id = Some("workspace-403".into());
    repo.insert_provider_account(seed).await.unwrap();
    admin
        .commit_excel_credential_import(
            prepared("acct_incoming", "user-403", "workspace-403"),
            None,
            None,
            &context(),
        )
        .await
        .unwrap();
    let id = ProviderAccountId::new("acct_403_dual").unwrap();
    for action in [Excel403Action::PauseAccount, Excel403Action::DisableExcel] {
        sqlx::query("update openai_excel_credentials set suspended=false where account_id=$1")
            .bind(id.as_str())
            .execute(&db.pool)
            .await
            .unwrap();
        sqlx::query("update provider_accounts set responses_upstream='excel',excel_403_action=$2 where id=$1").bind(id.as_str()).bind(action.as_str()).execute(&db.pool).await.unwrap();
        let frozen = repo.get_account(&id).await.unwrap().unwrap();
        assert!(repo.apply_excel_403_action(&frozen).await.unwrap());
        assert!(!repo.apply_excel_403_action(&frozen).await.unwrap());
        let after = repo.get_account(&id).await.unwrap().unwrap();
        assert!(after.enabled());
        assert!(after.excel_credential().unwrap().suspended);
        assert_eq!(after.responses_upstream(), ResponsesUpstream::Excel);
        assert_eq!(after.revision(), frozen.revision());
        assert_eq!(
            after.turn_state_binding_revision(),
            frozen.turn_state_binding_revision()
        );
        assert_eq!(after.credential_state(), CredentialState::Ready);
    }
    db.close().await;
}

#[tokio::test]
async fn excel_directory_status_filter_and_counts_follow_active_credential() {
    let Some(db) = TestDatabase::create("excel_directory").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    let admin = admin_account_store(&db.pool);
    let scenarios = [
        ("normal", AccountStatus::Normal),
        ("invalid", AccountStatus::Error),
        ("missing", AccountStatus::Error),
        ("suspended", AccountStatus::Disabled),
        ("exhausted", AccountStatus::QuotaExhausted),
    ];
    for (id, _) in scenarios {
        let mut seed = account(id, id);
        seed.credential_state = CredentialState::Invalid;
        repo.insert_provider_account(seed).await.unwrap();
        if id != "missing" {
            seed_sidecar(&db, id).await;
        }
        sqlx::query("update provider_accounts set responses_upstream='excel' where id=$1")
            .bind(id)
            .execute(&db.pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "update openai_excel_credentials set credential_state='invalid' where account_id='invalid'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("update openai_excel_credentials set suspended=true where account_id='suspended'")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("update openai_excel_credentials set quota_access_state='exhausted',quota_evidence='provider_denied',quota_access_observed_at=now(),updated_at=now() where account_id='exhausted'")
        .execute(&db.pool).await.unwrap();
    let runtime = AccountRuntimeSnapshot {
        rate_limited_until: scenarios
            .iter()
            .map(|(id, _)| (id.to_string(), Utc::now() + TimeDelta::minutes(5)))
            .collect(),
        ..Default::default()
    };
    for (id, expected) in scenarios {
        let page = admin
            .list_accounts(
                AccountListQuery {
                    page: 1,
                    page_size: PageSize::new(20).unwrap(),
                    provider_kind: None,
                    group_filter: None,
                    search: None,
                    status: Some(expected),
                    plan_type: None,
                    sort: None,
                },
                runtime.clone(),
            )
            .await
            .unwrap();
        assert!(
            page.items
                .iter()
                .any(|item| item.account.id == id && item.projection.status == expected)
        );
        assert_eq!(
            page.total,
            if expected == AccountStatus::Error {
                2
            } else {
                1
            }
        );
        assert_eq!(page.summary.total, 5);
        assert_eq!(page.summary.normal, 1);
        assert_eq!(page.summary.error, 2);
        assert_eq!(page.summary.disabled, 1);
        assert_eq!(page.summary.quota_exhausted, 1);
        assert_eq!(page.summary.rate_limited, 0);
    }
    db.close().await;
}

#[tokio::test]
async fn excel_entry_requires_current_independent_credential() {
    let Some(db) = TestDatabase::create("excel_enable_gate").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    let admin = admin_account_store(&db.pool);
    repo.insert_provider_account(account("acct_gate", "user-gate"))
        .await
        .unwrap();
    let patch = BatchUpdateAccounts {
        responses_upstream: Some(ResponsesUpstream::Excel),
        ..super::excel_recovery::patch("acct_gate")
    };
    assert!(
        admin
            .batch_update_accounts(patch.clone(), Default::default(), &context())
            .await
            .is_err()
    );
    seed_sidecar(&db, "acct_gate").await;
    sqlx::query("update openai_excel_credentials set access_token_expires_at=now()-interval '1 second' where account_id='acct_gate'")
        .execute(&db.pool).await.unwrap();
    assert!(
        admin
            .batch_update_accounts(patch.clone(), Default::default(), &context())
            .await
            .is_err()
    );
    sqlx::query("update openai_excel_credentials set access_token_expires_at=now()+interval '1 hour' where account_id='acct_gate'")
        .execute(&db.pool).await.unwrap();
    admin
        .batch_update_accounts(patch, Default::default(), &context())
        .await
        .unwrap();
    assert_eq!(
        repo.get_account(&ProviderAccountId::new("acct_gate").unwrap())
            .await
            .unwrap()
            .unwrap()
            .responses_upstream(),
        ResponsesUpstream::Excel
    );
    db.close().await;
}

#[tokio::test]
async fn codex_import_promotes_excel_shell_and_preserves_shared_settings() {
    use gateway_admin::model::provider_credentials::{
        CredentialImportCommit, PreparedCredentialImport,
    };
    let Some(db) = TestDatabase::create("excel_shell_import").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(db.pool.clone());
    let admin = admin_account_store(&db.pool);
    admin
        .commit_excel_credential_import(
            prepared("acct_shell", "user-shell", "workspace-shell"),
            None,
            None,
            &context(),
        )
        .await
        .unwrap();
    sqlx::query("update provider_accounts set name='shared-name',enabled=false,weight=7,model_access_json='{\"mode\":\"allowlist\",\"models\":[\"model-a\"]}' where id='acct_shell'")
        .execute(&db.pool).await.unwrap();
    let before: serde_json::Value = sqlx::query_scalar(
        "select to_jsonb(x) from openai_excel_credentials x where account_id='acct_shell'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let mut codex = prepared("acct_imported", "user-shell", "workspace-shell");
    codex.authentication_kind = "oauth".into();
    codex.name = "incoming-name".into();
    admin
        .commit_credential_import(
            CredentialImportCommit {
                outbound_proxy: None,
                settings: None,
                prepared: PreparedCredentialImport {
                    provider_kind: ProviderKind::new("openai").unwrap(),
                    credentials: vec![codex],
                },
            },
            &context(),
        )
        .await
        .unwrap();
    let id = ProviderAccountId::new("acct_shell").unwrap();
    let dual = repo.get_account(&id).await.unwrap().unwrap();
    assert!(dual.has_codex_credential());
    assert!(!dual.enabled());
    assert_eq!(dual.weight().get(), 7);
    let after: serde_json::Value = sqlx::query_scalar(
        "select to_jsonb(x) from openai_excel_credentials x where account_id='acct_shell'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    let name: String =
        sqlx::query_scalar("select name from provider_accounts where id='acct_shell'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(name, "shared-name");
    let model_access: serde_json::Value =
        sqlx::query_scalar("select model_access_json from provider_accounts where id='acct_shell'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        model_access,
        json!({"mode":"allowlist","models":["model-a"]})
    );
    assert!(
        repo.get_account(&ProviderAccountId::new("acct_imported").unwrap())
            .await
            .unwrap()
            .is_none()
    );
    db.close().await;
}

#[tokio::test]
async fn excel_shell_import_settings_only_enable_explicit_excel_entry() {
    use gateway_admin::model::relogin_templates::ReloginTemplateConfig;
    let Some(db) = TestDatabase::create("excel_shell_settings").await else {
        return;
    };
    let admin = admin_account_store(&db.pool);
    for (index, upstream) in [None, Some("codex"), Some("excel")].into_iter().enumerate() {
        let config: ReloginTemplateConfig = serde_json::from_value(json!({
            "name":"Shell fixture", "enabled":true, "weight":3,
            "concurrencyLimit":2, "groupIds":[], "outboundProxyId":null,
            "responsesUpstream":upstream
        }))
        .unwrap();
        let id = format!("acct_shell_{index}");
        let result = admin
            .commit_excel_credential_import(
                prepared(&id, &format!("user-{index}"), &format!("workspace-{index}")),
                None,
                Some(config.settings().unwrap()),
                &context(),
            )
            .await
            .unwrap();
        assert_eq!(
            result.account.responses_upstream,
            if upstream == Some("excel") {
                ResponsesUpstream::Excel
            } else {
                ResponsesUpstream::Codex
            }
        );
        assert!(!result.account.has_codex_credential());
        assert!(result.account.excel_credential.is_some());
    }
    db.close().await;
}
