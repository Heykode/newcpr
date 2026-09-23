use super::*;

#[derive(Default)]
struct PausingDeviceCodec {
    pause: std::sync::Mutex<Option<DeviceSnapshotPause>>,
}

struct DeviceSnapshotPause {
    entered: tokio::sync::oneshot::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

impl ProviderDeviceCodec for PausingDeviceCodec {
    fn provider_kind(&self) -> &str {
        TestDeviceCodec.provider_kind()
    }

    fn installation_id(
        &self,
        credential: &PlaintextCredential,
    ) -> Result<String, ProviderDeviceCodecError> {
        let device = TestDeviceCodec.installation_id(credential)?;
        let pause = self.pause.lock().unwrap().take();
        if let Some(DeviceSnapshotPause { entered, resume }) = pause {
            let _ = entered.send(());
            // Release the runtime worker so the test's database I/O keeps running.
            tokio::task::block_in_place(|| resume.recv_timeout(Duration::from_secs(5)))
                .map_err(|_| ProviderDeviceCodecError::Unavailable)?;
        }
        Ok(device)
    }

    fn with_installation_id(
        &self,
        credential: &PlaintextCredential,
        installation_id: &str,
    ) -> Result<PlaintextCredential, ProviderDeviceCodecError> {
        TestDeviceCodec.with_installation_id(credential, installation_id)
    }
}

async fn renewal(
    repository: &PgProviderAccountRepository,
    id: &str,
    device: &str,
    token: &str,
    preserve_binding: bool,
) -> CredentialCasUpdate {
    let id = ProviderAccountId::new(id).unwrap();
    let current = repository.load_current_credential(&id).await.unwrap();
    let update = CredentialCasUpdate::new(
        id.clone(),
        current.account.revision(),
        ProviderAccountUpdate {
            account_id: id,
            name: current.account.name().to_owned(),
            email: current.account.email().map(str::to_owned),
            plan_type: current.account.plan_type().map(str::to_owned),
        },
        PlaintextCredential::new(device_material(device, token).fields().clone()),
        true,
        current.account.access_token_expires_at(),
        None,
    )
    .unwrap()
    .preserving_profile();
    if preserve_binding {
        update.preserving_turn_state_binding()
    } else {
        update
    }
}

async fn lock_registry(transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>) {
    sqlx::query(
        "select pg_advisory_xact_lock(hashtextextended(current_schema() || ':cpr.provider-devices', 0))",
    )
    .execute(&mut **transaction)
    .await
    .unwrap();
}

async fn wait_for_blocked_writer(database: &TestDatabase, blocker: i32) {
    // The observer uses the admin connection, not the two-connection worker pool.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "select exists (select 1 from pg_stat_activity
                 where $1 = any(pg_blocking_pids(pid)))",
            )
            .bind(blocker)
            .fetch_one(&database.admin)
            .await
            .unwrap();
            if blocked {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("writer must reach a real database lock, not wait for a pool connection");
}

#[tokio::test]
async fn bound_credential_updates_do_not_wait_for_unrelated_device_mutations() {
    let Some(database) = TestDatabase::create("device_cas_independent").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    for id in ["acct_busy", "acct_refresh"] {
        repository
            .insert_provider_account(device_account(id, id, id, "original"))
            .await
            .unwrap();
    }
    for preserve_binding in [false, true] {
        let update = renewal(
            &repository,
            "acct_refresh",
            "incoming-device",
            "refreshed",
            preserve_binding,
        )
        .await;
        let mut busy = database.pool.begin().await.unwrap();
        lock_registry(&mut busy).await;
        sqlx::query("select id from provider_accounts where id = 'acct_busy' for update")
            .execute(&mut *busy)
            .await
            .unwrap();
        sqlx::query(
            "select installation_id from provider_device_identities
             where upstream_user_id = 'acct_refresh' for update",
        )
        .execute(&mut *busy)
        .await
        .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            repository.compare_and_swap_credential(update),
        )
        .await;
        busy.rollback().await.unwrap();
        assert!(
            matches!(result, Ok(Ok(CredentialCasOutcome::Updated(_)))),
            "an unrelated registry/account lock must not block a bound credential update"
        );
        assert_device(&repository, "acct_refresh", "acct_refresh", "refreshed").await;
    }
    let archived: i64 = sqlx::query_scalar("select count(*) from provider_device_identities")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(archived, 2);
    database.close().await;
}

#[tokio::test]
async fn bound_credential_updates_still_wait_for_same_account_and_reject_stale_writes() {
    let Some(database) = TestDatabase::create("device_cas_same_account").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    for preserve_binding in [false, true] {
        let id = format!("acct_same_{preserve_binding}");
        repository
            .insert_provider_account(device_account(&id, &id, &id, "original"))
            .await
            .unwrap();
        let update = renewal(
            &repository,
            &id,
            "incoming-device",
            "stale",
            preserve_binding,
        )
        .await;
        let mut busy = database.pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
            .fetch_one(&mut *busy)
            .await
            .unwrap();
        sqlx::query(
            "update provider_accounts set credential_revision = credential_revision + 1,
             provider_credentials_json = $2 where id = $1",
        )
        .bind(&id)
        .bind(device_material(&id, "winner").as_value())
        .execute(&mut *busy)
        .await
        .unwrap();
        let writer = repository.clone();
        let pending = tokio::spawn(async move { writer.compare_and_swap_credential(update).await });
        wait_for_blocked_writer(&database, pid).await;
        busy.commit().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(result, CredentialCasOutcome::Conflict));
        assert_device(&repository, &id, &id, "winner").await;
    }
    database.close().await;
}

#[tokio::test]
async fn missing_device_archive_is_not_committed_when_credential_projection_fails() {
    let Some(database) = TestDatabase::create("device_cas_rollback").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    repository
        .insert_provider_account(device_account("acct_rollback", "user", "retained", "old"))
        .await
        .unwrap();
    sqlx::query("delete from provider_device_identities")
        .execute(&database.pool)
        .await
        .unwrap();
    let before = repository
        .load_provider_account("acct_rollback")
        .await
        .unwrap();
    let invalid = renewal(&repository, "acct_rollback", "", "invalid", false).await;
    assert!(
        repository
            .compare_and_swap_credential(invalid)
            .await
            .is_err()
    );
    let archived: i64 = sqlx::query_scalar("select count(*) from provider_device_identities")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(archived, 0);
    assert_eq!(
        repository
            .load_provider_account("acct_rollback")
            .await
            .unwrap(),
        before
    );
    let valid = renewal(&repository, "acct_rollback", "incoming", "new", false).await;
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(5),
            repository.compare_and_swap_credential(valid),
        )
        .await
        .unwrap()
        .unwrap(),
        CredentialCasOutcome::Updated(_)
    ));
    assert_device(&repository, "acct_rollback", "retained", "new").await;
    database.close().await;
}

#[tokio::test]
async fn missing_device_archive_keeps_serialized_recovery_and_preserves_new_tokens() {
    let Some(database) = TestDatabase::create("device_cas_backfill").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    repository
        .insert_provider_account(device_account("acct_legacy", "legacy", "device-old", "old"))
        .await
        .unwrap();
    sqlx::query("delete from provider_device_identities")
        .execute(&database.pool)
        .await
        .unwrap();
    let update = renewal(&repository, "acct_legacy", "device-new", "new", false).await;
    let mut busy = database.pool.begin().await.unwrap();
    lock_registry(&mut busy).await;
    let pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *busy)
        .await
        .unwrap();
    let writer = repository.clone();
    let pending = tokio::spawn(async move { writer.compare_and_swap_credential(update).await });
    wait_for_blocked_writer(&database, pid).await;
    // Fallback must not lock the account before waiting for the registry lock.
    sqlx::query("select id from provider_accounts where id = 'acct_legacy' for update nowait")
        .execute(&mut *busy)
        .await
        .unwrap();
    busy.rollback().await.unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        CredentialCasOutcome::Updated(_)
    ));
    assert_device(&repository, "acct_legacy", "device-old", "new").await;
    let archived: String =
        sqlx::query_scalar("select installation_id from provider_device_identities")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(archived, "device-old");
    database.close().await;
}

#[tokio::test]
async fn bound_credential_updates_fail_closed_on_archive_conflicts_and_invalid_material() {
    let Some(database) = TestDatabase::create("device_cas_conflict").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    repository
        .insert_provider_account(device_account("acct_conflict", "user", "device", "old"))
        .await
        .unwrap();
    let before = repository
        .load_provider_account("acct_conflict")
        .await
        .unwrap();
    let invalid = renewal(&repository, "acct_conflict", "", "invalid", false).await;
    assert!(
        repository
            .compare_and_swap_credential(invalid)
            .await
            .is_err()
    );
    sqlx::query("update provider_device_identities set installation_id = 'conflicting-device'")
        .execute(&database.pool)
        .await
        .unwrap();
    let update = renewal(&repository, "acct_conflict", "device", "new", false).await;
    assert!(
        repository
            .compare_and_swap_credential(update)
            .await
            .is_err()
    );
    assert_eq!(
        repository
            .load_provider_account("acct_conflict")
            .await
            .unwrap(),
        before
    );
    let archived: String =
        sqlx::query_scalar("select installation_id from provider_device_identities")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(archived, "conflicting-device");
    database.close().await;
}

#[tokio::test]
async fn credential_refresh_racing_reimport_keeps_the_device_and_latest_import() {
    let Some(database) = TestDatabase::create("device_cas_import").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    repository
        .insert_provider_account(device_account("acct_imported", "user", "retained", "old"))
        .await
        .unwrap();
    for round in 0..8 {
        let update = renewal(
            &repository,
            "acct_imported",
            "refresh-device",
            "refreshed",
            round % 2 == 0,
        )
        .await;
        let command = import(
            vec![device_account(
                "acct_candidate",
                "user",
                "import-device",
                "imported",
            )],
            &format!("audit_racing_import_{round}"),
        );
        let (refreshed, imported) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                repository.compare_and_swap_credential(update),
                repository.import_provider_accounts(command),
            )
        })
        .await
        .expect("refresh and reimport must not deadlock");
        assert!(matches!(
            refreshed.unwrap(),
            CredentialCasOutcome::Updated(_) | CredentialCasOutcome::Conflict
        ));
        assert_eq!(imported.unwrap().account_ids, ["acct_imported"]);
        assert_device(&repository, "acct_imported", "retained", "imported").await;
    }
    database.close().await;
}

#[tokio::test]
async fn credential_refresh_racing_deletion_cannot_resurrect_or_replace_the_device() {
    let Some(database) = TestDatabase::create("device_cas_delete").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    for round in 0..8 {
        let id = format!("acct_deleted_{round}");
        repository
            .insert_provider_account(device_account(&id, &id, &id, "old"))
            .await
            .unwrap();
        let update = renewal(
            &repository,
            &id,
            "refresh-device",
            "refreshed",
            round % 2 == 0,
        )
        .await;
        let late_update = update.clone();
        let delete = DeleteProviderAccounts {
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".into(),
            },
            account_ids: vec![id.clone()],
            audit: audit(&format!("audit_racing_delete_{round}"), "delete", &id),
        };
        let (refreshed, deleted) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                repository.compare_and_swap_credential(update),
                repository.delete_provider_accounts_admin(delete),
            )
        })
        .await
        .expect("refresh and deletion must not deadlock");
        refreshed.unwrap();
        deleted.unwrap();
        assert!(
            repository
                .load_provider_account(&id)
                .await
                .unwrap()
                .is_none()
        );
        let restored_id = format!("acct_restored_{round}");
        repository
            .import_provider_accounts(import(
                vec![device_account(&restored_id, &id, "new-device", "restored")],
                &format!("audit_restored_{round}"),
            ))
            .await
            .unwrap();
        assert!(matches!(
            repository
                .compare_and_swap_credential(late_update)
                .await
                .unwrap(),
            CredentialCasOutcome::Conflict
        ));
        assert_device(&repository, &restored_id, &id, "restored").await;
    }
    database.close().await;
}

#[tokio::test]
async fn credential_refresh_racing_workspace_rebinding_cannot_restore_the_old_owner() {
    let Some(database) = TestDatabase::create("device_cas_rebinding").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    register(&repository).await;
    for round in 0..8 {
        let id = format!("acct_rebound_{round}");
        repository
            .insert_provider_account(device_account(&id, &id, &id, "old"))
            .await
            .unwrap();
        let before = repository
            .load_provider_account(&id)
            .await
            .unwrap()
            .unwrap();
        let update = renewal(
            &repository,
            &id,
            "refresh-device",
            "refreshed",
            round % 2 == 0,
        )
        .await;
        let mut credential = credential_update(&id, 1, "rebound");
        credential.provider_credentials_json = device_material("relogin-device", "rebound");
        let mut replacement_profile = profile(&id, "rebound");
        replacement_profile.email = before.summary.email.clone();
        let command = RotateProviderAccount {
            scope: ProviderAccountAdminScope {
                provider_kind: "openai".into(),
            },
            profile: replacement_profile,
            replacement_identity: Some(ProviderAccountIdentity::new(
                id.clone(),
                Some("new-workspace".into()),
            )),
            credential,
            relogin_operation_id: Some(format!("workspace-race-{round}")),
            audit: audit(&format!("audit_workspace_race_{round}"), "switch", &id),
        };
        let (refreshed, rebound) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                repository.compare_and_swap_credential(update),
                repository.switch_relogin_workspace(command),
            )
        })
        .await
        .expect("refresh and workspace switch must not deadlock");
        let workspace = match (refreshed.unwrap(), rebound) {
            (CredentialCasOutcome::Conflict, Ok(_)) => {
                assert_device(&repository, &id, &id, "rebound").await;
                "new-workspace"
            }
            (CredentialCasOutcome::Updated(_), Err(StoreError::Conflict { .. })) => {
                assert_device(&repository, &id, &id, "refreshed").await;
                "workspace-test"
            }
            _ => panic!("exactly one credential generation may win"),
        };
        let archived: (String, String) = sqlx::query_as(
            "select upstream_account_id, installation_id from provider_device_identities
             where upstream_user_id = $1",
        )
        .bind(&id)
        .fetch_one(&database.pool)
        .await
        .unwrap();
        assert_eq!(archived, (workspace.to_owned(), id.clone()));
        let after = repository
            .load_provider_account(&id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            after.summary.upstream_account_id.as_deref(),
            Some(workspace)
        );
        assert_eq!(after.summary.credential_revision.get(), 2);
    }
    database.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recreated_local_id_cannot_accept_a_snapshot_of_the_deleted_credential() {
    let Some(database) = TestDatabase::create("device_cas_reused_id").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let codec = Arc::new(PausingDeviceCodec::default());
    repository
        .initialize_device_registry(codec.clone())
        .await
        .unwrap();
    for preserve_binding in [false, true] {
        for same_principal in [false, true] {
            let id = format!("acct_reused_{preserve_binding}_{same_principal}");
            repository
                .insert_provider_account(device_account(&id, &id, &id, "old"))
                .await
                .unwrap();
            let update = renewal(&repository, &id, "incoming", "stale", preserve_binding).await;
            let (entered, paused) = tokio::sync::oneshot::channel();
            let (resume, wait) = std::sync::mpsc::channel();
            *codec.pause.lock().unwrap() = Some(DeviceSnapshotPause {
                entered,
                resume: wait,
            });
            let writer = repository.clone();
            let pending =
                tokio::spawn(async move { writer.compare_and_swap_credential(update).await });
            tokio::time::timeout(Duration::from_secs(5), paused)
                .await
                .unwrap()
                .unwrap();

            // Replace the row after the fast-path snapshot, before its write.
            repository
                .delete_provider_accounts_admin(DeleteProviderAccounts {
                    scope: ProviderAccountAdminScope {
                        provider_kind: "openai".into(),
                    },
                    account_ids: vec![id.clone()],
                    audit: audit(&format!("audit_delete_{id}"), "delete", &id),
                })
                .await
                .unwrap();
            let principal = if same_principal {
                id.clone()
            } else {
                format!("{id}-new-user")
            };
            let replacement_device = format!("{id}-new-device");
            repository
                .import_provider_accounts(import(
                    vec![device_account(&id, &principal, &replacement_device, "new")],
                    &format!("audit_recreate_{id}"),
                ))
                .await
                .unwrap();
            resume.send(()).unwrap();
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(5), pending)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap(),
                CredentialCasOutcome::Conflict
            ));
            let expected_device = if same_principal {
                &id
            } else {
                &replacement_device
            };
            assert_device(&repository, &id, expected_device, "new").await;
            let after = repository
                .load_provider_account(&id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(after.summary.credential_revision.get(), 1);
            assert_eq!(
                after.summary.upstream_user_id.as_deref(),
                Some(principal.as_str())
            );
        }
    }
    database.close().await;
}
