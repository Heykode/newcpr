use gateway_admin::{
    model::{
        MutationActor, MutationContext, Revision,
        egress::{ReplaceProviderEgress, SetProviderAccountEgress},
    },
    ports::store::ProviderEgressStore,
};
use gateway_core::{
    account::{OutboundProxy, ProviderAccountId},
    provider_ports::egress::{
        EgressMode, ProviderEgressAddress, ProviderEgressConfig, ProviderEgressStorePort,
    },
};
use gateway_store::postgres::{
    PgProviderAccountRepository, PgProviderEgressRepository, ProviderAccountRepository,
};

use super::{TestDatabase, provider_accounts::account};

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "ipv6-test".to_owned(),
    }
}

fn pool(enabled: bool) -> Vec<ProviderEgressAddress> {
    ["2001:db8::1", "2001:db8::2"]
        .into_iter()
        .enumerate()
        .map(|(index, address)| ProviderEgressAddress {
            id: format!("ipv6_{index}"),
            address: address.parse().unwrap(),
            enabled,
        })
        .collect()
}

async fn load(store: &PgProviderEgressRepository) -> ProviderEgressConfig {
    ProviderEgressStore::load(store).await.unwrap()
}

fn replace(
    config: &ProviderEgressConfig,
    mode: EgressMode,
    addresses: Vec<ProviderEgressAddress>,
) -> ReplaceProviderEgress {
    ReplaceProviderEgress {
        expected_revision: Revision::new(config.revision).unwrap(),
        default_mode: mode,
        addresses,
    }
}

#[tokio::test]
async fn fixed_affinity_prefers_vacant_then_rotates_and_survives_deletion_and_disabled_history() {
    let Some(database) = TestDatabase::create("ipv6_affinity").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    let store = PgProviderEgressRepository::new(database.pool.clone());
    for (id, user) in [
        ("acct_a", "user-a"),
        ("acct_b", "user-b"),
        ("acct_c", "user-c"),
    ] {
        let mut seed = account(id, user);
        seed.upstream_account_id = Some("workspace-shared".to_owned());
        accounts.insert_provider_account(seed).await.unwrap();
    }
    let initial = load(&store).await;
    assert_eq!(initial.default_mode, EgressMode::Unchanged);
    assert!(initial.addresses.is_empty());
    assert!(initial.fixed_bindings.is_empty());
    assert_eq!(initial.account_overrides.len(), 3);
    assert!(initial.account_overrides.values().all(Option::is_none));
    let before: Vec<serde_json::Value> =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a order by id")
            .fetch_all(&database.pool)
            .await
            .unwrap();
    let configured = store
        .replace(
            replace(&initial, EgressMode::FixedIpv6Reuse, pool(true)),
            &context(),
        )
        .await
        .unwrap()
        .config;
    let address = |config: &ProviderEgressConfig, id: &str| {
        config
            .fixed_bindings
            .get(&ProviderAccountId::new(id).unwrap())
            .copied()
            .unwrap()
    };
    assert_eq!(address(&configured, "acct_a"), pool(true)[0].address);
    assert_eq!(address(&configured, "acct_b"), pool(true)[1].address);
    assert_eq!(address(&configured, "acct_c"), pool(true)[0].address);
    let after: Vec<serde_json::Value> =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a order by id")
            .fetch_all(&database.pool)
            .await
            .unwrap();
    assert_eq!(
        before, after,
        "egress changes must not rotate credentials or profile"
    );
    assert!(
        store
            .replace(replace(&initial, EgressMode::Unchanged, vec![]), &context())
            .await
            .is_err()
    );
    assert_eq!(
        load(&store).await,
        configured,
        "stale writes must be atomic"
    );
    sqlx::query("delete from provider_accounts where id = 'acct_b'")
        .execute(&database.pool)
        .await
        .unwrap();
    let mut restored = account("acct_b_restored", "user-b");
    restored.upstream_account_id = Some("workspace-shared".to_owned());
    accounts.insert_provider_account(restored).await.unwrap();
    let binding = ProviderEgressStorePort::ensure_fixed_affinity(
        &store,
        &ProviderAccountId::new("acct_b_restored").unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(binding, Some(pool(true)[1].address));
    let restored_config = load(&store).await;
    assert!(
        !restored_config
            .account_overrides
            .contains_key(&ProviderAccountId::new("acct_b").unwrap())
    );
    let disabled = store
        .replace(
            replace(&restored_config, EgressMode::FixedIpv6Reuse, pool(false)),
            &context(),
        )
        .await
        .unwrap()
        .config;
    assert_eq!(address(&disabled, "acct_b_restored"), pool(true)[1].address);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from provider_egress_fixed_affinity")
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        3
    );
    database.close().await;
}

#[tokio::test]
async fn empty_pool_and_incomplete_identity_do_not_block_import_or_create_history() {
    let Some(database) = TestDatabase::create("ipv6_incomplete").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    let store = PgProviderEgressRepository::new(database.pool.clone());
    store
        .replace(
            replace(&load(&store).await, EgressMode::FixedIpv6Fresh, vec![]),
            &context(),
        )
        .await
        .unwrap();
    let mut complete = account("acct_empty_pool", "complete-user");
    complete.upstream_account_id = Some("workspace".to_owned());
    accounts.insert_provider_account(complete).await.unwrap();
    assert!(
        ProviderEgressStorePort::ensure_fixed_affinity(
            &store,
            &ProviderAccountId::new("acct_empty_pool").unwrap()
        )
        .await
        .unwrap()
        .is_none()
    );
    let mut incomplete = account("acct_incomplete", "user-only");
    incomplete.upstream_account_id = None;
    accounts.insert_provider_account(incomplete).await.unwrap();
    let configured = store
        .replace(
            replace(&load(&store).await, EgressMode::FixedIpv6Fresh, pool(true)),
            &context(),
        )
        .await
        .unwrap()
        .config;
    assert!(
        configured
            .fixed_bindings
            .contains_key(&ProviderAccountId::new("acct_empty_pool").unwrap())
    );
    assert!(
        !configured
            .fixed_bindings
            .contains_key(&ProviderAccountId::new("acct_incomplete").unwrap())
    );
    database.close().await;
}

#[tokio::test]
async fn account_inheritance_is_distinct_from_unchanged_and_proxy_conflicts_roll_back() {
    let Some(database) = TestDatabase::create("ipv6_proxy_conflict").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    let store = PgProviderEgressRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_proxy").unwrap();
    let mut seed = account(id.as_str(), "proxy-user");
    seed.upstream_account_id = Some("workspace".to_owned());
    seed.outbound_proxy = Some(OutboundProxy::parse("http://user:secret@127.0.0.1:1080").unwrap());
    accounts.insert_provider_account(seed).await.unwrap();
    let initial = load(&store).await;
    assert!(
        store
            .replace(
                replace(&initial, EgressMode::RandomIpv6Reuse, pool(true)),
                &context()
            )
            .await
            .is_err()
    );
    assert_eq!(load(&store).await, initial);
    let direct = store
        .set_account(
            SetProviderAccountEgress {
                account_id: id.clone(),
                expected_revision: Revision::new(initial.revision).unwrap(),
                mode: Some(EgressMode::Unchanged),
            },
            &context(),
        )
        .await
        .unwrap()
        .config;
    let active = store
        .replace(
            replace(&direct, EgressMode::RandomIpv6Reuse, pool(true)),
            &context(),
        )
        .await
        .unwrap()
        .config;
    assert_eq!(
        active.account_overrides.get(&id),
        Some(&Some(EgressMode::Unchanged))
    );
    assert!(
        store
            .set_account(
                SetProviderAccountEgress {
                    account_id: id.clone(),
                    expected_revision: Revision::new(active.revision).unwrap(),
                    mode: None,
                },
                &context()
            )
            .await
            .is_err()
    );
    assert_eq!(load(&store).await, active);
    let disabled = store
        .replace(
            replace(&active, EgressMode::Unchanged, pool(true)),
            &context(),
        )
        .await
        .unwrap()
        .config;
    let inherited = store
        .set_account(
            SetProviderAccountEgress {
                account_id: id.clone(),
                expected_revision: Revision::new(disabled.revision).unwrap(),
                mode: None,
            },
            &context(),
        )
        .await
        .unwrap()
        .config;
    assert_eq!(inherited.account_overrides.get(&id), Some(&None));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "select outbound_proxy_url from provider_accounts where id = 'acct_proxy'"
        )
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        "http://user:secret@127.0.0.1:1080/"
    );
    database.close().await;
}

#[tokio::test]
async fn inactive_modes_preallocate_without_egress_and_deleted_identities_release_vacancy() {
    let Some(database) = TestDatabase::create("ipv6_preallocation").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    let store = PgProviderEgressRepository::new(database.pool.clone());
    for (id, user) in [("acct_first", "user-first"), ("acct_second", "user-second")] {
        let mut seed = account(id, user);
        seed.upstream_account_id = Some("workspace".to_owned());
        accounts.insert_provider_account(seed).await.unwrap();
    }
    let initial = load(&store).await;
    let configured = store
        .replace(
            replace(&initial, EgressMode::Unchanged, pool(true)),
            &context(),
        )
        .await
        .unwrap()
        .config;
    assert_eq!(configured.fixed_bindings.len(), 2);
    assert_eq!(configured.default_mode, EgressMode::Unchanged);
    sqlx::query("delete from provider_accounts where id = 'acct_first'")
        .execute(&database.pool)
        .await
        .unwrap();
    // If old history still occupied the address, the next rotating slot would be address 2.
    sqlx::query("update provider_egress_settings set rotation_cursor = 1 where id = 1")
        .execute(&database.pool)
        .await
        .unwrap();
    let mut seed = account("acct_new", "user-new");
    seed.upstream_account_id = Some("workspace".to_owned());
    accounts.insert_provider_account(seed).await.unwrap();
    let allocated = ProviderEgressStorePort::ensure_fixed_affinity(
        &store,
        &ProviderAccountId::new("acct_new").unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(allocated, Some(pool(true)[0].address));
    let mut restored = account("acct_restored", "user-first");
    restored.upstream_account_id = Some("workspace".to_owned());
    accounts.insert_provider_account(restored).await.unwrap();
    let previous = ProviderEgressStorePort::ensure_fixed_affinity(
        &store,
        &ProviderAccountId::new("acct_restored").unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        previous, allocated,
        "restoring history permits sharing without rotation"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select rotation_cursor from provider_egress_settings where id = 1"
        )
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        1
    );
    database.close().await;
}

#[tokio::test]
async fn fixed_affinity_balances_by_active_occupancy_and_ignores_disabled_accounts() {
    let Some(database) = TestDatabase::create("ipv6_least_occupied").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    let store = PgProviderEgressRepository::new(database.pool.clone());
    let configured = store
        .replace(
            replace(&load(&store).await, EgressMode::FixedIpv6Reuse, pool(true)),
            &context(),
        )
        .await
        .unwrap()
        .config;
    assert_eq!(configured.fixed_bindings.len(), 0);

    for (id, user) in [
        ("acct_one", "user-one"),
        ("acct_two", "user-two"),
        ("acct_three", "user-three"),
        ("acct_four", "user-four"),
    ] {
        let mut seed = account(id, user);
        seed.upstream_account_id = Some("workspace".to_owned());
        accounts.insert_provider_account(seed).await.unwrap();
    }
    let after_four = load(&store).await;
    let address = |config: &ProviderEgressConfig, id: &str| {
        config
            .fixed_bindings
            .get(&ProviderAccountId::new(id).unwrap())
            .copied()
            .unwrap()
    };
    assert_eq!(address(&after_four, "acct_one"), pool(true)[0].address);
    assert_eq!(address(&after_four, "acct_two"), pool(true)[1].address);
    assert_eq!(address(&after_four, "acct_three"), pool(true)[0].address);
    assert_eq!(address(&after_four, "acct_four"), pool(true)[1].address);

    accounts
        .set_provider_account_enabled("acct_one", false)
        .await
        .unwrap();
    let mut fifth = account("acct_five", "user-five");
    fifth.upstream_account_id = Some("workspace".to_owned());
    accounts.insert_provider_account(fifth).await.unwrap();
    assert_eq!(
        address(&load(&store).await, "acct_five"),
        pool(true)[0].address
    );

    database.close().await;
}

#[tokio::test]
async fn affinity_fills_least_occupied_address_before_using_position_tiebreaker() {
    for mode in [EgressMode::FixedIpv6Reuse, EgressMode::Unchanged] {
        let Some(database) = TestDatabase::create("ipv6_uneven_occupancy").await else {
            return;
        };
        let accounts = PgProviderAccountRepository::new(database.pool.clone());
        let store = PgProviderEgressRepository::new(database.pool.clone());
        let addresses: Vec<_> = (1..=4)
            .map(|index| ProviderEgressAddress {
                id: format!("ipv6_{index}"),
                address: format!("2001:db8::{index}").parse().unwrap(),
                enabled: true,
            })
            .collect();
        store
            .replace(
                replace(&load(&store).await, mode, addresses.clone()),
                &context(),
            )
            .await
            .unwrap();
        for index in 0..16 {
            let mut seed = account(&format!("acct_{index:02}"), &format!("user-{index}"));
            seed.upstream_account_id = Some("workspace".to_owned());
            accounts.insert_provider_account(seed).await.unwrap();
        }
        let initial = load(&store).await;
        for index in 0..16 {
            assert_eq!(
                initial.fixed_bindings
                    [&ProviderAccountId::new(format!("acct_{index:02}")).unwrap()],
                addresses[index % 4].address,
            );
        }

        // Leave occupancy at 4/4/4/2 without discarding either disabled identity's history.
        for id in ["acct_03", "acct_07"] {
            accounts
                .set_provider_account_enabled(id, false)
                .await
                .unwrap();
        }
        for (index, expected_address) in [(16, 3), (17, 3), (18, 0)] {
            let id = format!("acct_{index:02}");
            let mut seed = account(&id, &format!("user-{index}"));
            seed.upstream_account_id = Some("workspace".to_owned());
            accounts.insert_provider_account(seed).await.unwrap();
            assert_eq!(
                load(&store).await.fixed_bindings[&ProviderAccountId::new(id).unwrap()],
                addresses[expected_address].address,
            );
        }
        accounts
            .set_provider_account_enabled("acct_03", true)
            .await
            .unwrap();
        let restored = load(&store).await;
        for id in ["acct_03", "acct_07"] {
            assert_eq!(
                restored.fixed_bindings[&ProviderAccountId::new(id).unwrap()],
                addresses[3].address,
                "reenabling or remaining disabled must preserve historical affinity",
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("select count(*) from provider_egress_fixed_affinity")
                .fetch_one(&database.pool)
                .await
                .unwrap(),
            19,
        );
        database.close().await;
    }
}

#[tokio::test]
async fn concurrent_imports_balance_affinity_using_committed_occupancy() {
    let Some(database) = TestDatabase::create("ipv6_concurrent_occupancy").await else {
        return;
    };
    let accounts = PgProviderAccountRepository::new(database.pool.clone());
    let store = PgProviderEgressRepository::new(database.pool.clone());
    store
        .replace(
            replace(&load(&store).await, EgressMode::FixedIpv6Fresh, pool(true)),
            &context(),
        )
        .await
        .unwrap();
    let results = futures::future::join_all((0..8).map(|index| {
        let accounts = &accounts;
        async move {
            let mut seed = account(&format!("acct_{index}"), &format!("user-{index}"));
            seed.upstream_account_id = Some("workspace".to_owned());
            accounts.insert_provider_account(seed).await
        }
    }))
    .await;
    for result in results {
        result.expect("concurrent import");
    }
    let configured = load(&store).await;
    assert_eq!(configured.fixed_bindings.len(), 8);
    for address in pool(true) {
        assert_eq!(
            configured
                .fixed_bindings
                .values()
                .filter(|&&bound| bound == address.address)
                .count(),
            4,
        );
    }
    database.close().await;
}
