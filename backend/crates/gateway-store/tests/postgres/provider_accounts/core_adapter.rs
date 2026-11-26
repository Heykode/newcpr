//! Core 账号列表的冻结范围与实时启停过滤

use super::*;

#[tokio::test]
async fn connected_quota_rejects_disabled_account_and_changed_effective_proxy() {
    let Some(database) = TestDatabase::create("connected_quota").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_connection").unwrap();
    let mut fixture = account(id.as_str(), "quota-subject");
    fixture.provider_kind = "antigravity".into();
    fixture.outbound_proxy =
        Some(gateway_core::account::OutboundProxy::parse("http://proxy.example:8080").unwrap());
    repo.insert_provider_account(fixture).await.unwrap();
    let proxied = repo.get_account(&id).await.unwrap().unwrap();
    assert!(proxied.outbound_proxy().is_some());
    let observation = QuotaObservation {
        account_id: id.clone(),
        expected_revision: proxied.revision(),
        quota: OpaqueProviderData::new(Default::default()),
        quota_pause: None,
        plan_type: Some("observed-plan".into()),
        observed_at: SystemTime::now(),
        state: QuotaState::unknown(),
    };
    // 代理记录独立于账号行，只改代理开关不会推进账号凭据版本。
    sqlx::query("update outbound_proxies set enabled=false where id=(select outbound_proxy_id from provider_accounts where id=$1)")
        .bind(id.as_str()).execute(&database.pool).await.unwrap();
    let direct = repo.get_account(&id).await.unwrap().unwrap();
    assert_eq!(direct.revision(), proxied.revision());
    assert!(direct.outbound_proxy().is_none());
    assert_eq!(
        repo.compare_and_swap_connected_quota(&proxied, observation.clone())
            .await
            .unwrap(),
        QuotaWriteOutcome::Conflict
    );
    repo.set_enabled(&id, false).await.unwrap();
    assert_eq!(
        repo.compare_and_swap_connected_quota(&direct, observation.clone())
            .await
            .unwrap(),
        QuotaWriteOutcome::Conflict
    );
    assert!(
        repo.get_quotas(std::slice::from_ref(&id))
            .await
            .unwrap()
            .is_empty()
    );
    repo.set_enabled(&id, true).await.unwrap();
    assert_eq!(
        repo.compare_and_swap_connected_quota(&direct, observation)
            .await
            .unwrap(),
        QuotaWriteOutcome::Updated
    );
    assert_eq!(
        repo.get_account(&id).await.unwrap().unwrap().plan_type(),
        Some("observed-plan")
    );
    database.close().await;
}

#[tokio::test]
async fn quota_activity_uses_successful_generation_completion_after_each_snapshot() {
    use gateway_core::account::QuotaObservationVersion;
    let Some(database) = TestDatabase::create("quota_generation_activity").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(database.pool.clone());
    let start = Utc::now() - TimeDelta::minutes(10);
    let mut snapshots = Vec::new();
    for (name, operation, outcome, offset_ms) in [
        ("acct_active", "generate", "succeeded", 500),
        ("acct_observed", "generate", "succeeded", 1000),
        ("acct_count", "count_tokens", "succeeded", 0),
        ("acct_failed", "generate", "failed", 0),
    ] {
        let mut fixture = account(name, name);
        fixture.provider_kind = "antigravity".to_owned();
        repo.insert_provider_account(fixture).await.unwrap();
        seed_model_request(
            &database.pool,
            ModelRequestSeed {
                request_id: name,
                account_id: name,
                provider_kind: "antigravity",
                model: "fixture-model",
                total_tokens: 5,
                cost_amount: "0",
                started_at: start,
            },
        )
        .await
        .unwrap();
        sqlx::query("update model_requests set operation=$2, outcome=$3 where id=$1")
            .bind(name)
            .bind(operation)
            .bind(outcome)
            .execute(&database.pool)
            .await
            .unwrap();
        snapshots.push(QuotaObservationVersion {
            account_id: ProviderAccountId::new(name).unwrap(),
            credential_revision: CredentialRevision::new(1).unwrap(),
            observed_at: (start + TimeDelta::milliseconds(offset_ms)).into(),
        });
    }
    super::super::drain_analytics(&database.pool).await;
    assert_eq!(
        repo.accounts_used_since_quota(&snapshots).await.unwrap(),
        vec![ProviderAccountId::new("acct_active").unwrap()]
    );
    database.close().await;
}

#[tokio::test]
async fn scoped_account_list_empty_scope_does_not_use_database() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://test@localhost/test")
        .expect("lazy database pool");
    pool.close().await;
    let repository = PgProviderAccountRepository::new(pool);
    let provider = ProviderKind::new("openai").expect("provider kind");

    assert!(
        repository
            .list_for_provider_accounts(&provider, &[])
            .await
            .expect("empty scope without a database connection")
            .is_empty()
    );
}

#[tokio::test]
async fn scoped_account_list_filters_ids_provider_and_live_enabled_state() {
    let Some(database) = TestDatabase::create("scoped_account_list").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let first = account("acct_scope_a", "scope-a");
    let last = account("acct_scope_z", "scope-z");
    let outside = account("acct_scope_outside", "scope-outside");
    let mut disabled = account("acct_scope_disabled", "scope-disabled");
    disabled.enabled = false;
    let mut other_provider = account("acct_scope_other_provider", "scope-other-provider");
    other_provider.provider_kind = "xai".to_owned();
    for fixture in [first, last, outside, disabled, other_provider] {
        repository
            .insert_provider_account(fixture)
            .await
            .expect("seed scoped account list");
    }
    let provider = ProviderKind::new("openai").expect("provider kind");
    let account_ids = [
        "acct_scope_z",
        "acct_scope_disabled",
        "acct_scope_other_provider",
        "acct_scope_z",
        "acct_scope_missing",
        "acct_scope_a",
    ]
    .map(|id| ProviderAccountId::new(id).expect("account id"));

    let scoped = repository
        .list_for_provider_accounts(&provider, &account_ids)
        .await
        .expect("load scoped account list");
    assert_eq!(
        scoped
            .iter()
            .map(|account| account.id().as_str())
            .collect::<Vec<_>>(),
        ["acct_scope_a", "acct_scope_z"]
    );
    let full_pool = repository
        .list_for_provider(&provider, false)
        .await
        .expect("load full provider pool");
    assert_eq!(
        full_pool
            .iter()
            .map(|account| account.id().as_str())
            .collect::<Vec<_>>(),
        ["acct_scope_a", "acct_scope_outside", "acct_scope_z"]
    );
    let maintenance = repository
        .list_for_provider(&provider, true)
        .await
        .expect("load provider accounts for maintenance");
    assert_eq!(
        maintenance
            .iter()
            .map(|account| account.id().as_str())
            .collect::<Vec<_>>(),
        [
            "acct_scope_a",
            "acct_scope_disabled",
            "acct_scope_outside",
            "acct_scope_z"
        ]
    );
    let all = repository
        .list_provider_accounts(None, true)
        .await
        .expect("load administrative account list");
    assert_eq!(all.len(), 5);
    assert!(all.iter().any(|account| !account.enabled));
    assert!(all.iter().any(|account| account.provider_kind == "xai"));

    let disabled_id = ProviderAccountId::new("acct_scope_disabled").expect("disabled account id");
    let enabled_id = ProviderAccountId::new("acct_scope_a").expect("enabled account id");
    repository
        .set_enabled(&enabled_id, false)
        .await
        .expect("disable an account between scoped reads");
    repository
        .set_enabled(&disabled_id, true)
        .await
        .expect("enable an account between scoped reads");
    let changed = repository
        .list_for_provider_accounts(&provider, &account_ids)
        .await
        .expect("reload scoped account list after enabled state changes");
    assert_eq!(
        changed
            .iter()
            .map(|account| account.id().as_str())
            .collect::<Vec<_>>(),
        ["acct_scope_disabled", "acct_scope_z"]
    );
    database.close().await;
}

#[tokio::test]
async fn bulk_catalog_restore_keeps_ten_thousand_owners_and_fences_late_writes() {
    use gateway_core::account::{AccountCatalogObservation, CredentialRevision};
    let Some(database) = TestDatabase::create("account_catalog_bulk").await else {
        return;
    };
    let repo = PgProviderAccountRepository::new(database.pool.clone());
    sqlx::query("insert into provider_accounts (id,provider_kind,name,authentication_kind,provider_credentials_json,has_refresh_token,credential_state,credential_observed_at,created_at,updated_at,plan_type)
        select 'acct_'||i,'openai','source '||i,'oauth','{}',false,'ready',now(),now(),now(),'pro' from generate_series(1,10000) i").execute(&database.pool).await.unwrap();
    let observation = AccountCatalogObservation {
        account_id: ProviderAccountId::new("acct_1").unwrap(),
        revision: CredentialRevision::new(1).unwrap(),
        observed_at: std::time::SystemTime::now(),
        etag: Some("v1".into()),
        plan: Some("pro".into()),
    };
    let document = vec![b'x'; 64 * 1024];
    assert!(
        repo.save_account_catalog(&observation, &document)
            .await
            .unwrap()
    );
    sqlx::query("insert into provider_account_model_catalogs (account_id,credential_revision,plan_type,observed_at,document)
        select a.id,1,'pro',now(),c.document from provider_accounts a cross join provider_account_model_catalogs c where a.id<>'acct_1'").execute(&database.pool).await.unwrap();
    let provider = ProviderKind::new("openai").unwrap();
    let start = std::time::Instant::now();
    let group = repo.load_account_catalogs(&provider).await.unwrap();
    eprintln!(
        "catalog bulk restore: 10000 accounts, {} documents, {} ms",
        group.len(),
        start.elapsed().as_millis()
    );
    assert_eq!(group.len(), 1);
    assert_eq!(group[0].accounts.len(), 10000);
    assert_eq!(&group[0].document[..], &document);
    let older = AccountCatalogObservation {
        observed_at: observation.observed_at - std::time::Duration::from_secs(1),
        ..observation.clone()
    };
    assert!(!repo.save_account_catalog(&older, b"late").await.unwrap());
    sqlx::query("update provider_accounts set credential_revision=2 where id='acct_1'")
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(
        !repo
            .save_account_catalog(&observation, b"old revision")
            .await
            .unwrap()
    );
    sqlx::query("update provider_accounts set enabled=false where id='acct_2'")
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("delete from provider_accounts where id='acct_3'")
        .execute(&database.pool)
        .await
        .unwrap();
    let group = repo.load_account_catalogs(&provider).await.unwrap();
    assert_eq!(group[0].accounts.len(), 9997);
    assert!(
        group[0]
            .accounts
            .iter()
            .all(|a| !matches!(a.account_id.as_str(), "acct_1" | "acct_2" | "acct_3"))
    );
    // 刷新到期不改变目录排除结果；凭据变化与显式例外仍进入选号
    let hash: Vec<u8> = sqlx::query_scalar("select sha256($1::bytea)")
        .bind(&document)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let now = std::time::SystemTime::now();
    sqlx::query("update provider_account_model_catalogs set observed_at=now()-interval '1 hour' where account_id='acct_4'").execute(&database.pool).await.unwrap();
    let filter = gateway_core::account::AccountCatalogOmissions {
        documents: vec![hash.try_into().unwrap()],
        exceptions: vec![ProviderAccountId::new("acct_5").unwrap()],
        as_of: now,
    };
    let ids = (1..=10000)
        .map(|n| ProviderAccountId::new(format!("acct_{n}")).unwrap())
        .collect::<Vec<_>>();
    let candidates = repo
        .list_catalog_candidates(&provider, &ids, &filter)
        .await
        .unwrap();
    let ids = candidates
        .iter()
        .map(|a| a.id().as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids, std::collections::BTreeSet::from(["acct_1", "acct_5"]));
    database.close().await;
}

#[tokio::test]
async fn quota_merge_can_only_replace_the_snapshot_it_read() {
    use gateway_core::account::{QuotaMergeBase, QuotaPauseDecision};
    let Some(database) = TestDatabase::create("quota_merge_base").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_merge").unwrap();
    repository
        .insert_provider_account(account(id.as_str(), "fixture"))
        .await
        .unwrap();
    let limits = json!({"codex:18000s": {"limitPercent": 95, "warmup": null}});
    sqlx::query("update provider_accounts set quota_settings = $2 where id = $1")
        .bind(id.as_str())
        .bind(&limits)
        .execute(&database.pool)
        .await
        .unwrap();
    let limits: gateway_core::account::AccountQuotaSettings =
        serde_json::from_value(limits).unwrap();
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let later = start + Duration::from_secs(1);
    let observed = |base, observed_at, paused| QuotaObservation {
        quota_pause: Some(QuotaPauseDecision {
            limits: limits.clone(),
            paused,
            merge_base: Some(QuotaMergeBase { observed_at: base }),
        }),
        account_id: id.clone(),
        expected_revision: CredentialRevision::new(1).unwrap(),
        quota: OpaqueProviderData::new(Default::default()),
        observed_at,
        plan_type: None,
        state: QuotaState::unknown(),
    };
    let write = |base, at, paused| repository.compare_and_swap_quota(observed(base, at, paused));
    assert_eq!(
        write(None, start, true).await.unwrap(),
        QuotaWriteOutcome::Updated
    );
    for (base, at) in [
        (None, later),
        (Some(start - Duration::from_secs(1)), later),
        // PostgreSQL 时间精度为微秒，同一版本不能代表两份不同的快照
        (Some(start), start + Duration::from_nanos(1)),
    ] {
        assert_eq!(
            write(base, at, false).await.unwrap(),
            QuotaWriteOutcome::Conflict
        );
    }
    assert!(
        repository
            .get_account(&id)
            .await
            .unwrap()
            .unwrap()
            .quota_paused()
    );
    assert_eq!(
        write(Some(start), later, false).await.unwrap(),
        QuotaWriteOutcome::Updated
    );
    assert!(
        !repository
            .get_account(&id)
            .await
            .unwrap()
            .unwrap()
            .quota_paused()
    );
    database.close().await;
}
