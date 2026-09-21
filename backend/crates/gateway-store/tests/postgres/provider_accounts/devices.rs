use std::sync::Arc;

use gateway_core::account::{ProviderDeviceCodec, ProviderDeviceCodecError};

use super::*;

struct TestDeviceCodec;

impl ProviderDeviceCodec for TestDeviceCodec {
    fn provider_kind(&self) -> &str {
        "openai"
    }

    fn installation_id(
        &self,
        credential: &PlaintextCredential,
    ) -> Result<String, ProviderDeviceCodecError> {
        credential
            .expose_to_provider()
            .get("device")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or(ProviderDeviceCodecError::Invalid)
    }

    fn with_installation_id(
        &self,
        credential: &PlaintextCredential,
        installation_id: &str,
    ) -> Result<PlaintextCredential, ProviderDeviceCodecError> {
        self.installation_id(credential)?;
        let mut value = credential.expose_to_provider().clone();
        value.insert("device".to_owned(), json!(installation_id));
        Ok(PlaintextCredential::new(value))
    }
}

fn device_account(id: &str, user: &str, device: &str, token: &str) -> NewProviderAccount {
    let mut value = account(id, user);
    value.upstream_account_id = Some("workspace-test".to_owned());
    value.provider_credentials_json = device_material(device, token);
    value
}

fn device_material(device: &str, token: &str) -> JsonObject {
    JsonObject::try_from_value(
        "provider_credentials_json",
        json!({
            "device": device,
            "access_token": token,
            "refresh_token": format!("{token}-refresh"),
            "cookies": [{"name": "session", "value": format!("{token}-cookie")}]
        }),
        256 * 1024,
    )
    .expect("test credential")
}

async fn register(repository: &PgProviderAccountRepository) {
    repository
        .initialize_device_registry(Arc::new(TestDeviceCodec))
        .await
        .expect("initialize device registry");
}

#[tokio::test]
async fn relogin_workspace_switch_is_atomic_preserves_settings_device_and_fences_conflicts() {
    let Some(database) = TestDatabase::create("relogin_workspace").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    let id = "acct_workspace_switch";
    let mut seed = device_account(id, "same-user", "original-device", "old-token");
    seed.enabled = false;
    seed.weight = gateway_core::account::AccountWeight::new(17).unwrap();
    seed.concurrency_limit = gateway_core::account::AccountConcurrencyLimit::new(5);
    seed.plan_type = Some("free".into());
    repository.insert_provider_account(seed).await.unwrap();
    sqlx::query(
        "update provider_accounts set custom_name='Keep name',
        quota_access_state='exhausted', quota_evidence='usage_limit_reached',
        quota_access_observed_at=now(), updated_at=now() where id=$1",
    )
    .bind(id)
    .execute(&database.pool)
    .await
    .unwrap();
    let before = repository.load_provider_account(id).await.unwrap().unwrap();
    let command = || {
        let mut credential = credential_update(id, 1, "new-token");
        credential.provider_credentials_json = device_material("runner-device", "new-token");
        let mut profile = profile(id, &before.summary.name);
        profile.email = before.summary.email.clone();
        profile.plan_type = Some("team".into());
        RotateProviderAccount {
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".into(),
            },
            profile,
            replacement_identity: Some(ProviderAccountIdentity::new(
                "same-user".into(),
                Some("workspace-business".into()),
            )),
            credential,
            relogin_operation_id: Some("workspace-switch-operation".into()),
            audit: audit("workspace-switch-audit", "relogin_workspace_switch", id),
        }
    };
    assert!(repository.rotate_provider_account(command()).await.is_err());
    for (user, audit_id, revision) in [
        ("different-user", "wrong-user", 1),
        ("same-user", "stale", 9),
    ] {
        let mut invalid = command();
        invalid.replacement_identity = Some(ProviderAccountIdentity::new(
            user.into(),
            Some("workspace-business".into()),
        ));
        invalid.credential.expected_revision = Revision::new(revision).unwrap();
        invalid.audit = audit(audit_id, "relogin_workspace_switch", id);
        assert!(repository.switch_relogin_workspace(invalid).await.is_err());
        assert_eq!(
            repository.load_provider_account(id).await.unwrap().unwrap(),
            before
        );
    }
    repository
        .switch_relogin_workspace(command())
        .await
        .unwrap();
    let after = repository.load_provider_account(id).await.unwrap().unwrap();
    assert_device(&repository, id, "original-device", "new-token").await;
    assert_eq!(
        after.summary.upstream_account_id.as_deref(),
        Some("workspace-business")
    );
    assert_eq!(
        after.summary.upstream_user_id,
        before.summary.upstream_user_id
    );
    assert_eq!(after.summary.custom_name, before.summary.custom_name);
    assert_eq!(after.summary.name, before.summary.name);
    assert_eq!(after.summary.enabled, before.summary.enabled);
    assert_eq!(after.summary.weight, before.summary.weight);
    assert_eq!(
        after.summary.concurrency_limit,
        before.summary.concurrency_limit
    );
    assert_eq!(after.summary.outbound_proxy, before.summary.outbound_proxy);
    assert_eq!(after.summary.relogin_count, 1);
    let quota: String =
        sqlx::query_scalar("select quota_access_state from provider_accounts where id=$1")
            .bind(id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(quota, "unknown");
    let bindings: Vec<(String, String)> = sqlx::query_as(
        "select upstream_account_id, installation_id from provider_device_identities order by upstream_account_id",
    ).fetch_all(&database.pool).await.unwrap();
    assert_eq!(
        bindings,
        vec![("workspace-business".into(), "original-device".into())]
    );
    assert!(
        repository
            .switch_relogin_workspace(command())
            .await
            .is_err()
    );
    assert_eq!(
        repository.load_provider_account(id).await.unwrap().unwrap(),
        after
    );
    database.close().await;
}

#[tokio::test]
async fn relogin_workspace_switch_cannot_steal_existing_destination_or_device() {
    for existing_pool in [false, true] {
        let Some(database) = TestDatabase::create("relogin_workspace_conflict").await else {
            return;
        };
        let repository = PgProviderAccountRepository::new(database.pool.clone());
        register(&repository).await;
        let id = "acct_switch_source";
        repository
            .insert_provider_account(device_account(
                id,
                "same-user",
                "source-device",
                "old-token",
            ))
            .await
            .unwrap();
        let mut destination = device_account(
            "acct_destination",
            "same-user",
            "destination-device",
            "destination-token",
        );
        destination.upstream_account_id = Some("workspace-business".into());
        repository
            .insert_provider_account(destination)
            .await
            .unwrap();
        if !existing_pool {
            repository
                .delete_provider_account("acct_destination")
                .await
                .unwrap();
        }
        let before = repository.load_provider_account(id).await.unwrap().unwrap();
        let bindings: Vec<serde_json::Value> = sqlx::query_scalar(
            "select to_jsonb(d) from provider_device_identities d order by upstream_account_id",
        )
        .fetch_all(&database.pool)
        .await
        .unwrap();
        let mut credential = credential_update(id, 1, "new-token");
        credential.provider_credentials_json = device_material("source-device", "new-token");
        let mut profile = profile(id, "Preserved");
        profile.email = before.summary.email.clone();
        assert!(
            repository
                .switch_relogin_workspace(RotateProviderAccount {
                    scope: ProviderAccountAdminScope {
                        provider_kind: "openai".into()
                    },
                    profile,
                    replacement_identity: Some(ProviderAccountIdentity::new(
                        "same-user".into(),
                        Some("workspace-business".into())
                    )),
                    credential,
                    relogin_operation_id: Some("conflicting-workspace-operation".into()),
                    audit: audit(
                        "conflicting-workspace-audit",
                        "relogin_workspace_switch",
                        id
                    ),
                })
                .await
                .is_err()
        );
        assert_eq!(
            repository.load_provider_account(id).await.unwrap().unwrap(),
            before
        );
        let after: Vec<serde_json::Value> = sqlx::query_scalar(
            "select to_jsonb(d) from provider_device_identities d order by upstream_account_id",
        )
        .fetch_all(&database.pool)
        .await
        .unwrap();
        assert_eq!(after, bindings);
        database.close().await;
    }
}

#[tokio::test]
async fn relogin_create_only_import_uses_device_registry_and_preserves_existing_tokens() {
    let Some(database) = TestDatabase::create("relogin_devices").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    repository
        .import_provider_accounts_with_mode(
            import(
                vec![device_account(
                    "acct_relogin",
                    "user-relogin",
                    "device-first",
                    "token-first",
                )],
                "relogin-first",
            ),
            true,
        )
        .await
        .unwrap();
    assert_device(&repository, "acct_relogin", "device-first", "token-first").await;
    assert!(
        repository
            .import_provider_accounts_with_mode(
                import(
                    vec![device_account(
                        "acct_race",
                        "user-relogin",
                        "device-second",
                        "token-stale"
                    )],
                    "relogin-race"
                ),
                true,
            )
            .await
            .is_err()
    );
    assert_device(&repository, "acct_relogin", "device-first", "token-first").await;
    let count: i64 = sqlx::query_scalar("select count(*) from provider_device_identities")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    database.close().await;
}

fn import(accounts: Vec<NewProviderAccount>, audit_id: &str) -> ImportProviderAccounts {
    ImportProviderAccounts {
        scope: ProviderAccountAdminScope {
            provider_kind: "openai".to_owned(),
        },
        settings: None,
        outbound_proxy: None,
        accounts,
        audit: audit(audit_id, "import", "devices"),
    }
}

#[tokio::test]
async fn relogin_success_count_preserves_device_and_scheduling_facts() {
    let Some(database) = TestDatabase::create("relogin_count_device").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    let id = "acct_relogin_count_device";
    let mut seed = device_account(id, "user-count", "original-device", "old-token");
    seed.enabled = false;
    seed.weight = gateway_core::account::AccountWeight::new(17).unwrap();
    seed.concurrency_limit = gateway_core::account::AccountConcurrencyLimit::new(5);
    repository.insert_provider_account(seed).await.unwrap();
    let before = repository.load_provider_account(id).await.unwrap().unwrap();
    let mut credential = credential_update(id, 1, "new-token");
    credential.provider_credentials_json = device_material("runner-device", "new-token");
    repository
        .rotate_provider_account(RotateProviderAccount {
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".into(),
            },
            profile: profile(id, "recovered"),
            replacement_identity: None,
            credential,
            relogin_operation_id: Some("relogin-device-operation".into()),
            audit: audit("relogin-device-audit", "relogin", id),
        })
        .await
        .unwrap();
    assert_device(&repository, id, "original-device", "new-token").await;
    let after = repository.load_provider_account(id).await.unwrap().unwrap();
    assert_eq!(after.summary.relogin_count, 1);
    assert_eq!(after.summary.enabled, before.summary.enabled);
    assert_eq!(after.summary.weight, before.summary.weight);
    assert_eq!(
        after.summary.concurrency_limit,
        before.summary.concurrency_limit
    );
    assert_eq!(after.summary.outbound_proxy, before.summary.outbound_proxy);
    assert_eq!(
        after.summary.upstream_user_id,
        before.summary.upstream_user_id
    );
    assert_eq!(
        after.summary.upstream_account_id,
        before.summary.upstream_account_id
    );
    database.close().await;
}

async fn assert_device(
    repository: &PgProviderAccountRepository,
    id: &str,
    device: &str,
    token: &str,
) {
    let stored = repository
        .load_provider_account(id)
        .await
        .expect("load account")
        .expect("account exists");
    assert_eq!(
        stored.provider_credentials_json.as_value()["device"],
        device
    );
    assert_eq!(
        stored.provider_credentials_json.as_value()["access_token"],
        token
    );
    assert_eq!(
        stored.provider_credentials_json.as_value()["refresh_token"],
        format!("{token}-refresh")
    );
    assert_eq!(
        stored.provider_credentials_json.as_value()["cookies"][0]["value"],
        format!("{token}-cookie")
    );
}

#[tokio::test]
async fn durable_devices_survive_delete_reimport_and_repository_reload() {
    let Some(database) = TestDatabase::create("devices_reimport").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    // Existing accounts must be adopted without changing a single credential.
    repository
        .insert_provider_account(device_account(
            "acct_original",
            "user-stable",
            "old-device",
            "old",
        ))
        .await
        .expect("seed old account");
    let revision_before_backfill = current_revision(&database.pool).await;
    register(&repository).await;
    let initial = repository
        .load_provider_account("acct_original")
        .await
        .expect("read existing account")
        .expect("existing account");
    assert_eq!(initial.summary.credential_revision.get(), 1);
    assert_eq!(
        current_revision(&database.pool).await,
        revision_before_backfill
    );
    repository
        .import_provider_accounts(import(
            vec![device_account(
                "acct_candidate",
                "user-stable",
                "new-device",
                "new",
            )],
            "audit_device_update",
        ))
        .await
        .expect("update existing principal");
    assert_device(&repository, "acct_original", "old-device", "new").await;
    repository
        .delete_provider_accounts_admin(DeleteProviderAccounts {
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".to_owned(),
            },
            account_ids: vec!["acct_original".to_owned()],
            audit: audit("audit_device_delete", "delete", "acct_original"),
        })
        .await
        .expect("delete without deleting device archive");

    let restarted = PgProviderAccountRepository::new(database.pool.clone());
    register(&restarted).await;
    restarted
        .import_provider_accounts(import(
            vec![device_account(
                "acct_returned",
                "user-stable",
                "third-device",
                "returned",
            )],
            "audit_device_return",
        ))
        .await
        .expect("reimport deleted principal");
    assert_device(&restarted, "acct_returned", "old-device", "returned").await;
    let columns: Vec<String> = sqlx::query_scalar(
        "select column_name from information_schema.columns
         where table_schema = current_schema() and table_name = 'provider_device_identities'",
    )
    .fetch_all(&database.pool)
    .await
    .expect("registry schema");
    assert!(
        !columns
            .iter()
            .any(|name| name.contains("token") || name.contains("cookie"))
    );
    database.close().await;
}

#[tokio::test]
async fn concurrent_device_imports_converge_without_losing_new_credentials() {
    let Some(database) = TestDatabase::create("devices_concurrent").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    let first = repository.clone();
    let second = repository.clone();
    let (first, second) = tokio::join!(
        first.import_provider_accounts(import(
            vec![device_account(
                "acct_race_a",
                "same-user",
                "device-a",
                "token-a"
            )],
            "audit_device_race_a",
        )),
        second.import_provider_accounts(import(
            vec![device_account(
                "acct_race_b",
                "same-user",
                "device-b",
                "token-b"
            )],
            "audit_device_race_b",
        )),
    );
    let first = first.expect("first import");
    let second = second.expect("second import");
    assert_eq!(first.account_ids, second.account_ids);
    let stored = repository
        .load_provider_account(&first.account_ids[0])
        .await
        .expect("load winner")
        .expect("winner");
    let archived: (i64, String) = sqlx::query_as(
        "select count(*)::bigint, min(installation_id) from provider_device_identities",
    )
    .fetch_one(&database.pool)
    .await
    .expect("one archive");
    assert_eq!(archived.0, 1);
    assert_eq!(
        stored.provider_credentials_json.as_value()["device"],
        archived.1
    );
    assert!(matches!(
        stored.provider_credentials_json.as_value()["access_token"].as_str(),
        Some("token-a" | "token-b")
    ));
    database.close().await;
}

#[tokio::test]
async fn reimport_keeps_timestamps_monotonic_when_the_transaction_clock_is_older() {
    let Some(database) = TestDatabase::create("devices_older_transaction").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    repository
        .import_provider_accounts(import(
            vec![device_account(
                "acct_clock_original",
                "clock-user",
                "retained-device",
                "original",
            )],
            "audit_clock_seed",
        ))
        .await
        .expect("seed original account");

    // A newer transaction may commit first while an older importer waits for a lock.
    let created_at = Utc::now() + chrono::Duration::hours(1);
    let updated_at = created_at + chrono::Duration::seconds(1);
    let (created_at, updated_at): (chrono::DateTime<Utc>, chrono::DateTime<Utc>) = sqlx::query_as(
        "update provider_accounts set created_at = $2, updated_at = $3
             where id = $1 returning created_at, updated_at",
    )
    .bind("acct_clock_original")
    .bind(created_at)
    .bind(updated_at)
    .fetch_one(&database.pool)
    .await
    .expect("represent the newer committed row");

    let revision_before = current_revision(&database.pool).await;
    let mut replacement = device_account(
        "acct_clock_candidate",
        "clock-user",
        "discarded-device",
        "replacement",
    );
    replacement.credential_observed_at = Utc::now() - chrono::Duration::minutes(1);
    let observed_at = replacement.credential_observed_at.timestamp_micros();
    let result = repository
        .import_provider_accounts(import(vec![replacement], "audit_clock_reimport"))
        .await
        .expect("reimport with an older transaction clock");
    assert_eq!(result.account_ids, ["acct_clock_original"]);
    assert_device(
        &repository,
        "acct_clock_original",
        "retained-device",
        "replacement",
    )
    .await;
    let stored: (chrono::DateTime<Utc>, chrono::DateTime<Utc>, i64) = sqlx::query_as(
        "select created_at, updated_at, credential_revision from provider_accounts where id = $1",
    )
    .bind("acct_clock_original")
    .fetch_one(&database.pool)
    .await
    .expect("load imported timestamps");
    assert_eq!(stored, (created_at, updated_at, 2));
    let stored_observation: chrono::DateTime<Utc> = sqlx::query_scalar(
        "select credential_observed_at from provider_accounts where id = 'acct_clock_original'",
    )
    .fetch_one(&database.pool)
    .await
    .expect("load incoming observation");
    assert_eq!(stored_observation.timestamp_micros(), observed_at);
    assert_eq!(current_revision(&database.pool).await, revision_before + 1);
    let counts: (i64, i64) = sqlx::query_as(
        "select (select count(*) from provider_accounts),
                (select count(*) from provider_device_identities)",
    )
    .fetch_one(&database.pool)
    .await
    .expect("one account and one device archive");
    assert_eq!(counts, (1, 1));
    database.close().await;
}

#[tokio::test]
async fn failed_import_rolls_back_device_archive_and_does_not_reserve_identity() {
    let Some(database) = TestDatabase::create("devices_rollback").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    let mut command = import(
        vec![
            device_account("acct_conflict", "first-user", "first-device", "first"),
            device_account("acct_conflict", "second-user", "second-device", "second"),
        ],
        "audit_device_rollback",
    );
    // A later account ID conflict occurs after the first account and device
    // would otherwise have been persisted.
    repository
        .insert_provider_account(device_account(
            "acct_existing",
            "existing-user",
            "existing-device",
            "existing",
        ))
        .await
        .expect("seed conflicting row");
    command.accounts[1].id = "acct_existing".to_owned();
    assert!(repository.import_provider_accounts(command).await.is_err());
    let records: i64 = sqlx::query_scalar(
        "select count(*) from provider_device_identities where upstream_user_id in ('first-user','second-user')",
    )
    .fetch_one(&database.pool)
    .await
    .expect("count rolled back device facts");
    assert_eq!(records, 0);
    assert!(
        repository
            .load_provider_account("acct_conflict")
            .await
            .expect("read")
            .is_none()
    );
    repository
        .insert_provider_account(device_account(
            "acct_after_rollback",
            "first-user",
            "replacement-device",
            "retry",
        ))
        .await
        .expect("rollback released device reservation");
    assert_device(
        &repository,
        "acct_after_rollback",
        "replacement-device",
        "retry",
    )
    .await;
    database.close().await;
}

#[tokio::test]
async fn distinct_principals_cannot_share_devices_even_when_email_matches() {
    let Some(database) = TestDatabase::create("devices_conflicts").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    let mut first = device_account("acct_identity_a", "shared-user", "device-a", "a");
    first.upstream_account_id = Some("workspace-a".to_owned());
    let mut second = device_account("acct_identity_b", "shared-user", "device-a", "b");
    second.upstream_account_id = Some("workspace-b".to_owned());
    second.email = first.email.clone();
    repository
        .insert_provider_account(first)
        .await
        .expect("first identity");
    assert!(matches!(
        repository.insert_provider_account(second.clone()).await,
        Err(StoreError::Conflict { .. })
    ));
    second.provider_credentials_json = device_material("device-b", "b");
    repository
        .insert_provider_account(second)
        .await
        .expect("distinct workspace device");
    assert_device(&repository, "acct_identity_a", "device-a", "a").await;
    assert_device(&repository, "acct_identity_b", "device-b", "b").await;
    database.close().await;
}

#[tokio::test]
async fn device_refresh_keeps_archive_and_new_tokens_and_rejects_identity_move() {
    let Some(database) = TestDatabase::create("devices_refresh").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    repository
        .insert_provider_account(device_account(
            "acct_refresh",
            "original-user",
            "device-original",
            "original",
        ))
        .await
        .expect("seed");
    let mut update = credential_update("acct_refresh", 1, "ignored");
    update.provider_credentials_json = device_material("incorrect-new-device", "refreshed");
    repository
        .compare_and_swap_credentials(update)
        .await
        .expect("refresh");
    assert_device(&repository, "acct_refresh", "device-original", "refreshed").await;

    let mut update = credential_update("acct_refresh", 2, "ignored");
    update.provider_credentials_json = device_material("device-original", "reauthorized");
    repository
        .rotate_provider_account(RotateProviderAccount {
            relogin_operation_id: None,
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".to_owned(),
            },
            profile: profile("acct_refresh", "new name"),
            replacement_identity: None,
            credential: update.clone(),
            audit: audit("audit_device_reauthorize", "rotate", "acct_refresh"),
        })
        .await
        .expect("reauthorization");
    assert_device(
        &repository,
        "acct_refresh",
        "device-original",
        "reauthorized",
    )
    .await;
    update.expected_revision = Revision::new(3).expect("revision");
    let identity = ProviderAccountIdentity::new("other-user".to_owned(), None);
    assert!(
        repository
            .rotate_provider_account(RotateProviderAccount {
                relogin_operation_id: None,
                scope: ProviderAccountAdminScope {
                    provider_kind: "openai".to_owned()
                },
                profile: profile("acct_refresh", "different principal"),
                replacement_identity: Some(identity),
                credential: update,
                audit: audit("audit_device_move", "rotate", "acct_refresh"),
            })
            .await
            .is_err()
    );
    assert_device(
        &repository,
        "acct_refresh",
        "device-original",
        "reauthorized",
    )
    .await;
    database.close().await;
}

#[tokio::test]
async fn startup_detects_conflicting_history_without_rewriting_active_device() {
    let Some(database) = TestDatabase::create("devices_backfill_conflict").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    repository
        .insert_provider_account(device_account(
            "acct_legacy",
            "legacy-user",
            "existing-device",
            "active",
        ))
        .await
        .expect("legacy account");
    sqlx::query(
        "insert into provider_device_identities
         (provider_kind, upstream_user_id, upstream_account_id, installation_id)
         values ('openai', 'legacy-user', 'workspace-test', 'different-history')",
    )
    .execute(&database.pool)
    .await
    .expect("conflicting legacy history");
    assert!(
        repository
            .initialize_device_registry(Arc::new(TestDeviceCodec))
            .await
            .is_err()
    );
    assert_device(&repository, "acct_legacy", "existing-device", "active").await;
    database.close().await;
}

#[tokio::test]
async fn incomplete_identity_is_not_matched_until_verified_fields_are_available() {
    let Some(database) = TestDatabase::create("devices_identity_completion").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    let mut incomplete = device_account(
        "acct_incomplete",
        "known-user",
        "existing-incomplete-device",
        "incomplete",
    );
    incomplete.upstream_account_id = None;
    repository
        .insert_provider_account(incomplete)
        .await
        .expect("incomplete account is still importable");
    let archived: i64 = sqlx::query_scalar("select count(*) from provider_device_identities")
        .fetch_one(&database.pool)
        .await
        .expect("registry count");
    assert_eq!(archived, 0);
    let mut update = credential_update("acct_incomplete", 1, "ignored");
    update.provider_credentials_json = device_material("incorrect-new-device", "verified");
    repository
        .rotate_provider_account(RotateProviderAccount {
            relogin_operation_id: None,
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".to_owned(),
            },
            profile: profile("acct_incomplete", "verified account"),
            replacement_identity: Some(ProviderAccountIdentity::new(
                "known-user".to_owned(),
                Some("verified-workspace".to_owned()),
            )),
            credential: update,
            audit: audit("audit_device_complete", "rotate", "acct_incomplete"),
        })
        .await
        .expect("verified missing field is an enrichment, not an identity move");
    assert_device(
        &repository,
        "acct_incomplete",
        "existing-incomplete-device",
        "verified",
    )
    .await;
    let archived: (String, String) = sqlx::query_as(
        "select upstream_account_id, installation_id from provider_device_identities",
    )
    .fetch_one(&database.pool)
    .await
    .expect("complete identity archive");
    assert_eq!(
        archived,
        (
            "verified-workspace".to_owned(),
            "existing-incomplete-device".to_owned()
        )
    );
    database.close().await;
}

#[tokio::test]
async fn shared_admin_store_applies_device_registry_to_create_delete_and_import() {
    use gateway_admin::model::provider_credentials::{
        CredentialImportCommit, PreparedCredentialImport,
    };

    let Some(database) = TestDatabase::create("devices_admin_shared").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    // Create the admin port before registration, like the production bundle.
    let admin = PgAdminAccountStore::with_repository(
        database.pool.clone(),
        repository.clone(),
        None,
        super::super::observability_query_budget(),
    );
    register(&repository).await;
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "device_admin_shared".to_owned(),
    };
    let prepare = |id: &str, device: &str, token: &str| PreparedCredentialCreate {
        outbound_proxy: None,
        account_id: ProviderAccountId::new(id).expect("account ID"),
        provider_kind: ProviderKind::new("openai").expect("provider"),
        name: id.to_owned(),
        email: Some("same-email@example.invalid".to_owned()),
        upstream_user_id: Some("admin-user".to_owned()),
        upstream_account_id: Some("admin-workspace".to_owned()),
        plan_type: Some("pro".to_owned()),
        authentication_kind: "oauth".to_owned(),
        provider_material: ProviderDocument::new(OpaqueProviderData::new(
            device_material(device, token).fields().clone(),
        )),
        has_refresh_token: true,
        access_token_expires_at: Some(Utc::now() + TimeDelta::hours(1)),
        next_refresh_at: None,
        enabled: true,
        credential_state: CredentialState::Ready,
        credential_observed_at: Utc::now(),
    };
    admin
        .commit_credential_import(
            CredentialImportCommit {
                outbound_proxy: None,
                settings: None,
                prepared: PreparedCredentialImport {
                    provider_kind: ProviderKind::new("openai").expect("provider"),
                    credentials: vec![prepare("acct_admin_first", "admin-device", "first")],
                },
            },
            &context,
        )
        .await
        .expect("admin import");
    admin
        .delete_accounts(
            DeleteAccounts {
                account_ids: vec!["acct_admin_first".to_owned()],
            },
            &context,
        )
        .await
        .expect("admin deletion");
    admin
        .commit_credential_import(
            CredentialImportCommit {
                outbound_proxy: None,
                settings: None,
                prepared: PreparedCredentialImport {
                    provider_kind: ProviderKind::new("openai").expect("provider"),
                    credentials: vec![prepare("acct_admin_second", "discard-new-device", "second")],
                },
            },
            &context,
        )
        .await
        .expect("admin reimport");
    assert_device(&repository, "acct_admin_second", "admin-device", "second").await;
    database.close().await;
}

#[tokio::test]
async fn device_and_ipv6_history_share_account_transactions_without_crossing_principals() {
    let Some(database) = TestDatabase::create("devices_ipv6_transaction").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    sqlx::query(
        "insert into provider_egress_addresses (id, address, enabled, position)
         values ('source-a', '2001:db8::1', true, 0), ('source-b', '2001:db8::2', true, 1)",
    )
    .execute(&database.pool)
    .await
    .expect("seed local-only source configuration");
    repository
        .import_provider_accounts(import(
            vec![device_account(
                "acct_linked",
                "linked-user",
                "linked-device",
                "original",
            )],
            "audit_device_linked",
        ))
        .await
        .expect("import archives device and reserves source together");
    let source: String = sqlx::query_scalar(
        "select address from provider_egress_fixed_affinity where upstream_user_key = 'linked-user'",
    )
    .fetch_one(&database.pool)
    .await
    .expect("source was allocated by actual account import");
    assert_eq!(source, "2001:db8::1");
    let before_delete: i64 =
        sqlx::query_scalar("select revision from provider_egress_settings where id = 1")
            .fetch_one(&database.pool)
            .await
            .expect("egress revision");
    repository
        .delete_provider_accounts_admin(DeleteProviderAccounts {
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".to_owned(),
            },
            account_ids: vec!["acct_linked".to_owned()],
            audit: audit("audit_device_linked_delete", "delete", "acct_linked"),
        })
        .await
        .expect("delete account but retain both histories");
    let after_delete: i64 =
        sqlx::query_scalar("select revision from provider_egress_settings where id = 1")
            .fetch_one(&database.pool)
            .await
            .expect("egress revision");
    assert_eq!(after_delete, before_delete + 1);
    repository
        .insert_provider_account(device_account(
            "acct_other",
            "different-user",
            "different-device",
            "other",
        ))
        .await
        .expect("vacant source can be used by another account");
    repository
        .insert_provider_account(device_account(
            "acct_linked_return",
            "linked-user",
            "discard-device",
            "returned",
        ))
        .await
        .expect("returning account restores its own histories");
    assert_device(
        &repository,
        "acct_linked_return",
        "linked-device",
        "returned",
    )
    .await;
    assert_device(&repository, "acct_other", "different-device", "other").await;
    let source: String = sqlx::query_scalar(
        "select address from provider_egress_fixed_affinity where upstream_user_key = 'linked-user'",
    )
    .fetch_one(&database.pool)
    .await
    .expect("source history still exists");
    assert_eq!(source, "2001:db8::1");

    sqlx::query(
        "update provider_egress_settings set default_mode = 'fixed_ipv6_reuse' where id = 1",
    )
    .execute(&database.pool)
    .await
    .expect("activate source policy for validation");
    let mut conflicting = device_account(
        "acct_proxy_conflict",
        "proxy-user",
        "proxy-device",
        "secret",
    );
    conflicting.outbound_proxy = Some(
        gateway_core::account::OutboundProxy::parse("http://127.0.0.1:7999")
            .expect("syntactically valid test proxy"),
    );
    assert!(
        repository
            .import_provider_accounts(import(vec![conflicting], "audit_device_proxy_conflict"))
            .await
            .is_err()
    );
    let registry_count: i64 = sqlx::query_scalar(
        "select count(*) from provider_device_identities where upstream_user_id = 'proxy-user'",
    )
    .fetch_one(&database.pool)
    .await
    .expect("count rolled-back device archive");
    assert_eq!(registry_count, 0);
    assert!(
        repository
            .load_provider_account("acct_proxy_conflict")
            .await
            .expect("load rolled-back account")
            .is_none()
    );
    database.close().await;
}
