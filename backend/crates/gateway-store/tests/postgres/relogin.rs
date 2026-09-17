use super::{
    TestDatabase, admin_account_store,
    provider_accounts::{account, audit, credential_update, profile},
};
use gateway_admin::{
    model::{
        PageSize,
        accounts::{AccountListQuery, AccountSort, AccountSortField, SortDirection},
        relogin::{ReloginEntry, ReloginSettings, ReloginStatus},
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
        imported_at: Some(Utc::now()),
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
        automatic_attempts: 0,
        attempted_target: None,
        next_attempt_at: None,
        synced_at: None,
        updated_at: chrono::Utc::now(),
    }
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
    assert!(loaded.validate_totp().is_err());
    loaded.automatic = false;
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
    store.delete(&legacy.id, loaded.revision).await.unwrap();
    assert_eq!(store.entries().await.unwrap().len(), 1);
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
