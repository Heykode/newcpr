use super::{
    TestDatabase, admin_account_store,
    provider_accounts::{account, audit, credential_update, profile},
};
use gateway_admin::{
    model::{
        PageSize,
        accounts::{AccountListQuery, AccountSort, AccountSortField, SortDirection},
        relogin::{ReloginEntry, ReloginSettings, ReloginStatus, ReloginTarget},
    },
    ports::{relogin::ReloginStore, store::AccountStore},
};
use gateway_store::postgres::{
    ImportProviderAccounts, PgProviderAccountRepository, PgReloginStore,
    ProviderAccountAdminRepository, ProviderAccountAdminScope, ProviderAccountRepository,
    RotateProviderAccount,
};

fn entry(id: &str, email: &str) -> ReloginEntry {
    ReloginEntry {
        imported_at: Some(chrono::Utc::now()),
        id: id.into(),
        email: email.into(),
        password: "test-only-password".into(),
        mfa_secret: "JBSWY3DPEHPK3PXP".into(),
        revision: 1,
        automatic: true,
        preferred_workspace_id: None,
        status: ReloginStatus::Pending,
        message: String::new(),
        credential: None,
        target: None,
        automatic_job: false,
        manual_push_context: None,
        automatic_attempts: 0,
        automatic_started_at: Vec::new(),
        attempted_target: None,
        next_attempt_at: None,
        synced_at: None,
        updated_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn relogin_target_projection_survives_cookie_cas_but_fences_real_replacement() {
    use gateway_core::account::{
        CredentialCasOutcome, CredentialCasUpdate, PlaintextCredential, ProviderAccountId,
        ProviderAccountStore, ProviderAccountUpdate,
    };
    let Some(database) = TestDatabase::create("relogin_cookie_binding").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let store = admin_account_store(&database.pool);
    let id = ProviderAccountId::new("acct_cookie_binding").unwrap();
    let provider = gateway_core::routing::ProviderKind::new("openai").unwrap();
    let mut seed = account(id.as_str(), "cookie-user");
    seed.upstream_account_id = Some("cookie-workspace".into());
    repository.insert_provider_account(seed).await.unwrap();
    let original = store
        .credential_details(&provider, &id)
        .await
        .unwrap()
        .unwrap()
        .credential;
    let target = ReloginTarget::from_account(&original).unwrap();
    let stored_target: ReloginTarget =
        serde_json::from_value(serde_json::to_value(&target).unwrap()).unwrap();
    for sequence in 0..3 {
        let loaded = repository.load_current_credential(&id).await.unwrap();
        let mut material = loaded.credential.expose_to_provider().clone();
        material.insert(
            "cookies".into(),
            serde_json::json!([
                {"name": "__cf_bm", "value": format!("synthetic-{sequence}")}
            ]),
        );
        let update = CredentialCasUpdate::new(
            id.clone(),
            loaded.account.revision(),
            ProviderAccountUpdate {
                account_id: id.clone(),
                name: loaded.account.name().into(),
                email: loaded.account.email().map(str::to_owned),
                plan_type: loaded.account.plan_type().map(str::to_owned),
            },
            PlaintextCredential::new(material),
            false,
            loaded.account.access_token_expires_at(),
            None,
        )
        .unwrap()
        .preserving_profile()
        .preserving_turn_state_binding();
        assert!(matches!(
            repository
                .compare_and_swap_credential(update)
                .await
                .unwrap(),
            CredentialCasOutcome::Updated(_)
        ));
        let current = store
            .credential_details(&provider, &id)
            .await
            .unwrap()
            .unwrap()
            .credential;
        assert!(current.credential_revision > original.credential_revision);
        assert_eq!(
            current.turn_state_binding_revision,
            original.turn_state_binding_revision
        );
        assert!(stored_target.matches_account(&current));
    }
    assert!(
        repository
            .rotate_provider_account(rotation(
                id.as_str(),
                1,
                Some("stale-cookie-cas"),
                "stale-cookie-audit"
            ))
            .await
            .is_err(),
        "the final CAS must still reject a stale prepared revision"
    );
    repository
        .compare_and_swap_credentials(credential_update(id.as_str(), 4, "replacement"))
        .await
        .unwrap();
    let replaced = store
        .credential_details(&provider, &id)
        .await
        .unwrap()
        .unwrap()
        .credential;
    assert_eq!(
        replaced.turn_state_binding_revision,
        replaced.credential_revision
    );
    assert!(!stored_target.matches_account(&replaced));
    assert!(
        ReloginTarget::from_account(&replaced)
            .unwrap()
            .matches_account(&replaced)
    );
    database.close().await;
}

#[tokio::test]
async fn relogin_storage_can_read_and_update_legacy_mailbox_rows_without_using_tokens() {
    let Some(database) = TestDatabase::create("relogin_legacy").await else {
        return;
    };
    let store = PgReloginStore::new(database.pool.clone());
    let legacy = entry("relogin_legacy", "legacy@example.invalid");
    let normal = entry("relogin_totp", "totp@example.invalid");
    store.save(&legacy, None).await.unwrap();
    store.save(&normal, None).await.unwrap();
    let mut material = serde_json::to_value(&legacy).unwrap();
    material
        .as_object_mut()
        .unwrap()
        .remove("manual_push_context");
    material
        .as_object_mut()
        .unwrap()
        .remove("automatic_started_at");
    material["mfa_secret"] = "".into();
    material["mailbox"] = serde_json::json!({
        "client_id": "123e4567-e89b-12d3-a456-426614174000",
        "refresh_token": "synthetic-mailbox-token",
    });
    sqlx::query("update account_relogin_entries set material=$1 where id=$2")
        .bind(material)
        .bind(&legacy.id)
        .execute(&database.pool)
        .await
        .unwrap();

    let rows = store.entries().await.unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .find(|row| row.id == normal.id)
            .unwrap()
            .validate_totp()
            .is_ok()
    );
    let mut loaded = rows.into_iter().find(|row| row.id == legacy.id).unwrap();
    assert!(loaded.manual_push_context.is_none());
    assert!(loaded.automatic_started_at.is_empty());
    assert!(loaded.validate_totp().is_err());
    loaded.automatic = false;
    loaded.automatic_started_at = vec![chrono::Utc::now()];
    loaded.manual_push_context = Some(gateway_admin::model::MutationContext {
        actor: gateway_admin::model::MutationActor::AdminSession {
            admin_user_id: "admin-test".into(),
        },
        request_id: "account-menu-test".into(),
    });
    loaded.revision += 1;
    store.save(&loaded, Some(legacy.revision)).await.unwrap();
    let saved: serde_json::Value =
        sqlx::query_scalar("select material from account_relogin_entries where id=$1")
            .bind(&legacy.id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(saved.get("mailbox").is_none());
    assert!(!saved.to_string().contains("synthetic-mailbox-token"));
    let roundtrip: ReloginEntry = serde_json::from_value(saved).unwrap();
    assert_eq!(roundtrip.manual_push_context, loaded.manual_push_context);
    assert_eq!(roundtrip.automatic_started_at, loaded.automatic_started_at);
    store.delete(&legacy.id, loaded.revision).await.unwrap();
    assert_eq!(store.entries().await.unwrap().len(), 1);
    database.close().await;
}

#[tokio::test]
async fn relogin_templates_persist_fence_versions_and_revalidate_references() {
    use gateway_admin::model::relogin_templates::{ReloginTemplate, ReloginTemplateConfig};
    let Some(database) = TestDatabase::create("relogin_templates").await else {
        return;
    };
    let store = PgReloginStore::new(database.pool.clone());
    let group = "grp_00000000000000000000000000000091";
    sqlx::query("insert into account_groups (id,name,color,created_at,updated_at) values ($1,'Template group','#34A853FF',now(),now())")
        .bind(group).execute(&database.pool).await.unwrap();
    sqlx::query("insert into outbound_proxies (id,name,proxy_url,last_test_success) values ('proxy-template','Template proxy','http://127.0.0.1:8080',true)")
        .execute(&database.pool).await.unwrap();
    let mut template = ReloginTemplate {
        id: "template-one".into(),
        revision: 1,
        config: ReloginTemplateConfig {
            name: "Team settings".into(),
            enabled: false,
            concurrency_limit: Some(7),
            weight: 17,
            group_ids: vec![group.into()],
            outbound_proxy_id: Some("proxy-template".into()),
        },
    };
    store.save_template(&template, None).await.unwrap();
    let reopened = PgReloginStore::new(database.pool.clone());
    assert_eq!(reopened.templates().await.unwrap(), vec![template.clone()]);
    let mut duplicate = template.clone();
    duplicate.id = "template-duplicate".into();
    duplicate.config.name = "TEAM SETTINGS".into();
    assert!(store.save_template(&duplicate, None).await.is_err());
    template.revision = 2;
    template.config.weight = 23;
    store.save_template(&template, Some(1)).await.unwrap();
    assert!(store.save_template(&template, Some(1)).await.is_err());
    assert!(store.delete_template(&template.id, 1).await.is_err());
    store
        .validate_template_references(&template.config)
        .await
        .unwrap();
    sqlx::query("update outbound_proxies set last_test_success=false where id='proxy-template'")
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(
        store
            .validate_template_references(&template.config)
            .await
            .is_err()
    );
    sqlx::query("update outbound_proxies set last_test_success=true where id='proxy-template'")
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("delete from account_groups where id=$1")
        .bind(group)
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(
        store
            .validate_template_references(&template.config)
            .await
            .is_err()
    );
    // Stale references remain visible and deletable; they never silently become defaults.
    assert_eq!(reopened.templates().await.unwrap(), vec![template.clone()]);
    store.delete_template(&template.id, 2).await.unwrap();
    assert!(reopened.templates().await.unwrap().is_empty());
    database.close().await;
}

#[tokio::test]
async fn relogin_storage_fences_rows_and_rolls_back_entire_import() {
    let Some(database) = TestDatabase::create("relogin_cas").await else {
        return;
    };
    let store = PgReloginStore::new(database.pool.clone());
    let initial = entry("relogin_a", "a@example.invalid");
    store.save(&initial, None).await.unwrap();
    let duplicate = entry("relogin_duplicate", "A@example.invalid");
    assert!(store.save(&duplicate, None).await.is_err());
    let mut changed = initial.clone();
    changed.revision = 2;
    changed.automatic = false;
    store.save(&changed, Some(1)).await.unwrap();
    assert!(store.save(&initial, Some(1)).await.is_err());
    assert!(store.delete(&initial.id, 1).await.is_err());
    let new = entry("relogin_new", "new@example.invalid");
    assert!(
        store
            .save_batch(&[(new, None), (initial.clone(), Some(1))])
            .await
            .is_err()
    );
    let rows = store.entries().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].revision, 2);
    assert_eq!(rows[0].password, "test-only-password");
    assert!(!rows[0].automatic);
    assert_eq!(store.settings().await.unwrap().concurrency, 1);
    store
        .save_settings(&ReloginSettings {
            concurrency: 3,
            paused: true,
        })
        .await
        .unwrap();
    assert!(store.settings().await.unwrap().paused);
    assert!(
        store
            .save_settings(&ReloginSettings {
                concurrency: 0,
                paused: false
            })
            .await
            .is_err()
    );
    store.delete(&initial.id, 2).await.unwrap();
    assert!(store.entries().await.unwrap().is_empty());
    database.close().await;
}

#[tokio::test]
async fn relogin_deleting_material_does_not_delete_pool_or_fingerprint() {
    let Some(database) = TestDatabase::create("relogin_delete").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    let mut seed = account("acct_original", "user-original");
    seed.upstream_account_id = Some("workspace-team".into());
    accounts.insert_provider_account(seed).await.unwrap();
    let before: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a where id='acct_original'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    let devices: Vec<serde_json::Value> =
        sqlx::query_scalar("select to_jsonb(d) from provider_device_identities d")
            .fetch_all(&database.pool)
            .await
            .unwrap();
    let store = PgReloginStore::new(database.pool.clone());
    let material = entry("relogin_a", "a@example.invalid");
    store.save(&material, None).await.unwrap();
    store.delete(&material.id, 1).await.unwrap();
    let after: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a where id='acct_original'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    let after_devices: Vec<serde_json::Value> =
        sqlx::query_scalar("select to_jsonb(d) from provider_device_identities d")
            .fetch_all(&database.pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    assert_eq!(devices, after_devices);
    database.close().await;
}

#[tokio::test]
async fn relogin_create_only_import_cannot_overwrite_racing_import() {
    let Some(database) = TestDatabase::create("relogin_create").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let mut seed = account("acct_original", "user-original");
    seed.upstream_account_id = Some("workspace-team".into());
    let command = |id: &str| {
        let mut candidate = seed.clone();
        candidate.id = id.into();
        ImportProviderAccounts {
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".into(),
            },
            accounts: vec![candidate],
            settings: None,
            outbound_proxy: None,
            audit: audit(&format!("audit-{id}"), "relogin_import", id),
        }
    };
    repository
        .import_provider_accounts_with_mode(command("acct_original"), true)
        .await
        .unwrap();
    let before: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a where id='acct_original'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    let before_count: i64 = sqlx::query_scalar("select count(*) from provider_device_identities")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert!(
        repository
            .import_provider_accounts_with_mode(command("acct_racing"), true)
            .await
            .is_err()
    );
    let after: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a where id='acct_original'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    let count: i64 = sqlx::query_scalar("select count(*) from provider_device_identities")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(count, before_count);
    database.close().await;
}

fn rotation(
    id: &str,
    revision: u64,
    operation: Option<&str>,
    audit_id: &str,
) -> RotateProviderAccount {
    RotateProviderAccount {
        scope: ProviderAccountAdminScope {
            provider_kind: "openai".into(),
        },
        profile: profile(id, "test-only relogin"),
        replacement_identity: None,
        credential: credential_update(id, revision, audit_id),
        relogin_operation_id: operation.map(str::to_owned),
        audit: audit(audit_id, "rotate", id),
    }
}

async fn stored_account(pool: &sqlx::PgPool, id: &str) -> serde_json::Value {
    sqlx::query_scalar("select to_jsonb(a) from provider_accounts a where id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn relogin_count_is_atomic_idempotent_and_independent_of_library_and_token_refresh() {
    let Some(database) = TestDatabase::create("relogin_count").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let id = "acct_count";
    repository
        .insert_provider_account(account(id, "user-count"))
        .await
        .unwrap();
    let initial = repository.load_provider_account(id).await.unwrap().unwrap();
    assert_eq!(initial.summary.relogin_count, 0);
    assert!(initial.summary.last_relogin_at.is_none());

    repository
        .compare_and_swap_credentials(credential_update(id, 1, "refresh"))
        .await
        .unwrap();
    repository
        .rotate_provider_account(rotation(id, 2, None, "manual-json"))
        .await
        .unwrap();
    assert_eq!(
        repository
            .load_provider_account(id)
            .await
            .unwrap()
            .unwrap()
            .summary
            .relogin_count,
        0
    );
    repository
        .rotate_provider_account(rotation(id, 3, Some("login-1"), "audit-login-1"))
        .await
        .unwrap();
    let committed = stored_account(&database.pool, id).await;
    assert_eq!(committed["relogin_count"], 1);
    assert!(committed["last_relogin_at"].is_string());

    for command in [
        rotation(id, 3, Some("login-1"), "repeated-cas"),
        rotation(id, 4, Some("login-1"), "repeated-operation"),
        rotation(id, 4, Some("login-2"), "audit-login-1"),
    ] {
        assert!(repository.rotate_provider_account(command).await.is_err());
        assert_eq!(stored_account(&database.pool, id).await, committed);
        let count: i64 = sqlx::query_scalar("select count(*) from account_relogin_successes")
            .fetch_one(&database.pool)
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "events roll back together with credentials and the counter"
        );
    }

    repository
        .rotate_provider_account(rotation(id, 4, Some("login-2"), "audit-login-2"))
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        repository.rotate_provider_account(rotation(id, 5, Some("race-a"), "audit-race-a")),
        repository.rotate_provider_account(rotation(id, 5, Some("race-b"), "audit-race-b")),
    );
    assert_ne!(a.is_ok(), b.is_ok(), "only one competing CAS can count");
    let after = stored_account(&database.pool, id).await;
    assert_eq!(after["relogin_count"], 3);
    assert_eq!(after["credential_revision"], 6);

    let store = PgReloginStore::new(database.pool.clone());
    let material = entry("count-material", "acct_count@rotated.example.invalid");
    store.save(&material, None).await.unwrap();
    store.delete(&material.id, material.revision).await.unwrap();
    store.save(&material, None).await.unwrap();
    assert_eq!(stored_account(&database.pool, id).await, after);
    let reopened = PgProviderAccountRepository::new(database.pool.clone());
    assert_eq!(
        reopened
            .load_provider_account(id)
            .await
            .unwrap()
            .unwrap()
            .summary
            .relogin_count,
        3
    );

    let mut reimport = account(id, "user-count");
    reimport.name = "reimported".into();
    repository
        .import_provider_accounts(ImportProviderAccounts {
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".into(),
            },
            accounts: vec![reimport],
            settings: None,
            outbound_proxy: None,
            audit: audit("audit-reimport", "import", id),
        })
        .await
        .unwrap();
    let latest = reopened.load_provider_account(id).await.unwrap().unwrap();
    assert_eq!(latest.summary.relogin_count, 3);
    assert_eq!(
        latest.summary.last_relogin_at,
        serde_json::from_value(after["last_relogin_at"].clone()).unwrap()
    );
    database.close().await;
}

#[tokio::test]
async fn relogin_count_sorts_before_pagination_and_list_detail_read_the_same_stats() {
    let Some(database) = TestDatabase::create("relogin_count_sort").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    for (id, count) in [("acct_z", 3), ("acct_a", 1), ("acct_b", 1), ("acct_m", 0)] {
        repository
            .insert_provider_account(account(id, &format!("user-{id}")))
            .await
            .unwrap();
        for revision in 1..=count {
            let operation = format!("{id}-{revision}");
            repository
                .rotate_provider_account(rotation(id, revision, Some(&operation), &operation))
                .await
                .unwrap();
        }
    }
    let store = admin_account_store(&database.pool);
    for (direction, expected) in [
        (SortDirection::Asc, ["acct_m", "acct_a", "acct_b", "acct_z"]),
        (
            SortDirection::Desc,
            ["acct_z", "acct_b", "acct_a", "acct_m"],
        ),
    ] {
        let mut actual = Vec::new();
        for page in 1..=2 {
            let result = store
                .list_accounts(
                    AccountListQuery {
                        page,
                        page_size: PageSize::new(2).unwrap(),
                        provider_kind: None,
                        group_filter: None,
                        search: None,
                        status: None,
                        plan_type: None,
                        sort: Some(AccountSort {
                            field: AccountSortField::ReloginCount,
                            direction,
                        }),
                    },
                    Default::default(),
                )
                .await
                .unwrap();
            assert_eq!(result.total, 4);
            for item in result.items {
                let detail = store
                    .credential_details(
                        &gateway_core::routing::ProviderKind::new("openai").unwrap(),
                        &gateway_core::account::ProviderAccountId::new(item.account.id.clone())
                            .unwrap(),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(item.account.relogin_count, detail.credential.relogin_count);
                assert_eq!(
                    item.account.last_relogin_at,
                    detail.credential.last_relogin_at
                );
                actual.push(item.account.id);
            }
        }
        assert_eq!(actual, expected);
    }
    database.close().await;
}
