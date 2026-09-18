use std::collections::BTreeMap;

use gateway_admin::{
    model::{
        MutationActor, MutationContext, PageSize,
        account_groups::{
            AccountGroupColor, AccountGroupListQuery, DeleteAccountGroup, NewAccountGroup,
        },
        client_keys::NewClientKey,
        client_keys::UpdateClientKey,
    },
    ports::store::{AccountGroupStore, AdminStoreErrorKind, ClientKeyStore},
};
use gateway_core::{
    policy::{ClientApiKeyId, RateLimits},
    routing::AccountGroupId,
};
use gateway_store::postgres::{PgAccountGroupRepository, PgAdminClientKeyStore};

use super::TestDatabase;

const MIXED_GROUP: &str = "grp_00000000000000000000000000000001";
const EMPTY_GROUP: &str = "grp_00000000000000000000000000000002";

mod monitor_lifecycles;

#[tokio::test]
async fn monitor_reuses_key_bindings_and_includes_ungrouped_quota_peers() {
    let Some(db) = TestDatabase::create("monitor_key_facts").await else {
        return;
    };
    let groups = PgAccountGroupRepository::new(db.pool.clone());
    let keys = PgAdminClientKeyStore::new(db.pool.clone());
    for id in [MIXED_GROUP, EMPTY_GROUP] {
        groups
            .create_account_group(
                NewAccountGroup {
                    id: group_id(id),
                    name: id.to_owned(),
                    description: None,
                    color: group_color("#2563EBFF"),
                    disable_fast: false,
                },
                &context("monitor-groups"),
            )
            .await
            .unwrap();
    }
    for id in ["acct_bound", "acct_unbound"] {
        seed_account(&db.pool, id, "openai", id).await;
    }
    assign_accounts(&db.pool, MIXED_GROUP, &["acct_bound"]).await;
    keys.create_client_key(
        new_key(
            "key_shared",
            vec![group_id(MIXED_GROUP), group_id(EMPTY_GROUP)],
        ),
        &context("monitor-key"),
    )
    .await
    .unwrap();
    keys.create_client_key(new_key("key_all", vec![]), &context("monitor-all"))
        .await
        .unwrap();
    let facts = groups.load_group_monitor(chrono::Utc::now()).await.unwrap();
    assert_eq!(facts.group_client_keys[MIXED_GROUP], ["key_shared"]);
    assert_eq!(facts.group_client_keys[EMPTY_GROUP], ["key_shared"]);
    assert_eq!(facts.members.len(), 1);
    assert_eq!(facts.quota_peers.len(), 2);
    assert!(
        facts
            .quota_peers
            .iter()
            .any(|peer| peer.id == "acct_unbound")
    );
    db.close().await;
}

#[tokio::test]
async fn disable_fast_group_updates_preserve_omitted_values_and_publish_snapshot_facts() {
    use gateway_admin::model::account_groups::UpdateAccountGroup;
    use gateway_store::postgres::{PgRuntimeSnapshotRepository, RuntimeSnapshotRepository};
    let Some(database) = TestDatabase::create("disable_fast_group").await else {
        return;
    };
    let repository = PgAccountGroupRepository::new(database.pool.clone());
    let id = group_id(MIXED_GROUP);
    repository
        .create_account_group(
            NewAccountGroup {
                id: id.clone(),
                name: "Fast policy".to_owned(),
                description: None,
                color: group_color("#2563EBFF"),
                disable_fast: true,
            },
            &context("create-fast"),
        )
        .await
        .unwrap();
    for (value, expected) in [(None, true), (Some(false), false), (Some(true), true)] {
        let mutation = repository
            .update_account_group(
                UpdateAccountGroup {
                    id: id.clone(),
                    name: "Renamed policy".to_owned(),
                    description: None,
                    color: group_color("#2563EBFF"),
                    disable_fast: value,
                },
                &context("update-fast"),
            )
            .await
            .unwrap();
        assert_eq!(mutation.record.unwrap().disable_fast, expected);
        let snapshot = PgRuntimeSnapshotRepository::new(database.pool.clone())
            .load_runtime_snapshot()
            .await
            .unwrap();
        assert_eq!(
            snapshot.config_revision.get(),
            mutation.config_revision.get()
        );
        assert_eq!(
            snapshot
                .account_groups
                .iter()
                .find(|group| group.id == id)
                .unwrap()
                .disable_fast,
            expected
        );
    }
    database.close().await;
}

#[tokio::test]
async fn groups_aggregate_cross_provider_members_and_key_bindings_without_multiplication() {
    let Some(database) = TestDatabase::create("account_group_aggregate").await else {
        return;
    };
    seed_account(
        &database.pool,
        "acct_group_openai",
        "openai",
        "OpenAI Account",
    )
    .await;
    seed_account(&database.pool, "acct_group_xai", "xai", "xAI Account").await;
    sqlx::query(
        "update provider_accounts set concurrency_limit = 4 where id = 'acct_group_openai'",
    )
    .execute(&database.pool)
    .await
    .expect("set account concurrency override");

    let groups = PgAccountGroupRepository::new(database.pool.clone());
    let keys = PgAdminClientKeyStore::new(database.pool.clone());
    let mixed_group = group_id(MIXED_GROUP);
    let empty_group = group_id(EMPTY_GROUP);
    groups
        .create_account_group(
            NewAccountGroup {
                id: mixed_group.clone(),
                name: "Mixed Production".to_owned(),
                description: Some("cross-provider".to_owned()),
                color: group_color("#2563EBFF"),
                disable_fast: false,
            },
            &context("create-mixed"),
        )
        .await
        .expect("create mixed account group");
    groups
        .create_account_group(
            NewAccountGroup {
                id: empty_group.clone(),
                name: "Empty Pool".to_owned(),
                description: None,
                color: group_color("#06B6D4CC"),
                disable_fast: false,
            },
            &context("create-empty"),
        )
        .await
        .expect("create empty account group");
    assign_accounts(
        &database.pool,
        MIXED_GROUP,
        &["acct_group_openai", "acct_group_xai"],
    )
    .await;

    for (id, group_ids) in [
        ("key_group_one", vec![mixed_group.clone()]),
        ("key_group_two", vec![mixed_group.clone()]),
        ("key_empty_pool", vec![empty_group.clone()]),
        ("key_all_accounts", Vec::new()),
    ] {
        keys.create_client_key(new_key(id, group_ids), &context(id))
            .await
            .expect("create scoped client key");
    }
    seed_group_cost_snapshot(
        &database.pool,
        "req_historical_empty_group",
        "acct_group_openai",
        EMPTY_GROUP,
        "1.5",
    )
    .await;

    let page = groups
        .list_account_groups(AccountGroupListQuery {
            page: 1,
            page_size: PageSize::new(20).expect("page size"),
            search: None,
            enabled: None,
        })
        .await
        .expect("list account groups");
    let members = groups
        .load_account_group_members(std::slice::from_ref(&mixed_group))
        .await
        .expect("load current-page group members");
    assert_eq!(members.len(), 2);
    assert_eq!(
        members.iter().map(|member| member.total_slots).sum::<u64>(),
        7
    );
    assert_eq!(page.total, 2);
    let by_id = page
        .items
        .into_iter()
        .map(|group| (group.id.to_string(), group))
        .collect::<BTreeMap<_, _>>();
    let mixed = by_id.get(MIXED_GROUP).expect("mixed group");
    assert_eq!(mixed.member_count, 2);
    assert_eq!(
        mixed.provider_counts,
        BTreeMap::from([("openai".to_owned(), 1), ("xai".to_owned(), 1)])
    );
    assert_eq!(mixed.client_key_count, 2);
    // PostgreSQL 返回持久页与 member facts；实时状态/容量由 Admin query service 投影。
    assert_eq!(mixed.account_summary.available, 0);
    assert_eq!(mixed.account_summary.limited, 0);
    assert_eq!(mixed.account_summary.total, 0);
    assert_eq!(mixed.capacity.used_slots, None);
    assert_eq!(mixed.capacity.total_slots, 0);
    assert_eq!(mixed.usage.today_usd.as_str(), "0");
    assert_eq!(mixed.usage.retained_total_usd.as_str(), "0");
    let empty = by_id.get(EMPTY_GROUP).expect("empty group");
    assert_eq!(empty.member_count, 0);
    assert!(empty.provider_counts.is_empty());
    assert_eq!(empty.client_key_count, 1);
    assert_eq!(empty.usage.today_usd.as_str(), "1.5");
    assert_eq!(empty.usage.retained_total_usd.as_str(), "1.5");

    let all_key = keys
        .reveal_client_key(&client_key_id("key_all_accounts"))
        .await
        .expect("reveal all-accounts key")
        .expect("all-accounts key exists");
    assert!(all_key.record.groups.is_empty());
    assert_eq!(
        all_key
            .record
            .provider_kinds
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>(),
        ["openai", "xai"]
    );
    let empty_pool_key = keys
        .reveal_client_key(&client_key_id("key_empty_pool"))
        .await
        .expect("reveal empty-pool key")
        .expect("empty-pool key exists");
    assert_eq!(empty_pool_key.record.groups.len(), 1);
    assert!(empty_pool_key.record.provider_kinds.is_empty());

    let (scope_revision, widened) = keys
        .update_client_key(
            UpdateClientKey {
                daily_limit_usd: None,
                weekly_limit_usd: None,
                id: client_key_id("key_group_one"),
                name: "key_group_one".to_owned(),
                label: None,
                group_ids: Vec::new(),
                limits: RateLimits::unlimited(),
            },
            &context("widen-group-key"),
        )
        .await
        .expect("widen restricted key to all accounts");
    assert!(widened.groups.is_empty());
    let scope_audit: Vec<String> = sqlx::query_scalar(
        "select changed_fields from admin_audit_events
         where admin_request_id = 'widen-group-key'",
    )
    .fetch_one(&database.pool)
    .await
    .expect("load scope widening audit");
    assert!(scope_audit.contains(&"routing_scope:groups->all".to_owned()));
    assert_eq!(current_revision(&database.pool).await, scope_revision.get());
    let (restricted_revision, restricted) = keys
        .update_client_key(
            UpdateClientKey {
                daily_limit_usd: None,
                weekly_limit_usd: None,
                id: client_key_id("key_group_one"),
                name: "key_group_one".to_owned(),
                label: None,
                group_ids: vec![empty_group],
                limits: RateLimits::unlimited(),
            },
            &context("restrict-all-key"),
        )
        .await
        .expect("restrict all-accounts key to groups");
    assert_eq!(restricted.groups.len(), 1);
    let restricted_audit: Vec<String> = sqlx::query_scalar(
        "select changed_fields from admin_audit_events
         where admin_request_id = 'restrict-all-key'",
    )
    .fetch_one(&database.pool)
    .await
    .expect("load scope restriction audit");
    assert!(restricted_audit.contains(&"routing_scope:all->groups".to_owned()));
    assert_eq!(
        current_revision(&database.pool).await,
        restricted_revision.get()
    );

    let revision_before_delete = current_revision(&database.pool).await;
    let audit_before_delete = audit_count(&database.pool).await;
    let error = groups
        .delete_account_group(
            DeleteAccountGroup { id: mixed_group },
            &context("delete-referenced"),
        )
        .await
        .expect_err("referenced group must not be deleted");
    assert_eq!(error.kind(), AdminStoreErrorKind::Conflict);
    assert_eq!(
        current_revision(&database.pool).await,
        revision_before_delete
    );
    assert_eq!(audit_count(&database.pool).await, audit_before_delete);

    database.close().await;
}

#[tokio::test]
async fn group_costs_should_include_statusless_websocket_but_reject_statusless_http() {
    let Some(database) = TestDatabase::create("account_group_statusless_websocket_cost").await
    else {
        return;
    };
    seed_account(
        &database.pool,
        "acct_group_statusless",
        "openai",
        "Statusless Account",
    )
    .await;
    let groups = PgAccountGroupRepository::new(database.pool.clone());
    groups
        .create_account_group(
            NewAccountGroup {
                id: group_id(EMPTY_GROUP),
                name: "Statusless Costs".to_owned(),
                description: None,
                color: group_color("#06B6D4CC"),
                disable_fast: false,
            },
            &context("create-statusless-cost-group"),
        )
        .await
        .expect("create statusless cost group");
    for (request_id, cost_amount) in [
        ("req_group_http_success", "1.5"),
        ("req_group_statusless_websocket", "2"),
        ("req_group_statusless_http", "4"),
    ] {
        seed_group_cost_snapshot(
            &database.pool,
            request_id,
            "acct_group_statusless",
            EMPTY_GROUP,
            cost_amount,
        )
        .await;
    }
    sqlx::query(
        "update model_requests
         set client_transport = case id
               when 'req_group_statusless_websocket' then 'websocket'
               else 'http_sse'
             end,
             client_status_code = null
         where id in ('req_group_statusless_websocket', 'req_group_statusless_http')",
    )
    .execute(&database.pool)
    .await
    .expect("make account group requests statusless");

    let page = groups
        .list_account_groups(AccountGroupListQuery {
            page: 1,
            page_size: PageSize::new(20).expect("page size"),
            search: None,
            enabled: None,
        })
        .await
        .expect("list statusless cost group");

    assert_eq!(
        (
            page.items[0].usage.today_usd.as_str(),
            page.items[0].usage.retained_total_usd.as_str(),
        ),
        ("3.5", "3.5"),
    );

    database.close().await;
}

fn new_key(id: &str, group_ids: Vec<AccountGroupId>) -> NewClientKey {
    let marker = char::from(id.as_bytes().last().copied().unwrap_or(b'k'));
    NewClientKey {
        budget: Default::default(),
        id: client_key_id(id),
        name: id.to_owned(),
        label: None,
        group_ids,
        limits: RateLimits::unlimited(),
        plaintext: format!("sk_{}", marker.to_string().repeat(43)),
    }
}

#[tokio::test]
async fn monitor_snapshot_uses_completion_window_shared_costs_and_durable_identity_without_writes()
{
    use chrono::{Duration, Utc};

    let Some(database) = TestDatabase::create("group_monitor").await else {
        return;
    };
    let now = Utc::now();
    let groups = PgAccountGroupRepository::new(database.pool.clone());
    const THIRD_GROUP: &str = "grp_00000000000000000000000000000003";
    for id in [MIXED_GROUP, EMPTY_GROUP, THIRD_GROUP] {
        groups
            .create_account_group(
                NewAccountGroup {
                    id: group_id(id),
                    name: id.to_owned(),
                    description: None,
                    color: group_color("#2563EBFF"),
                    disable_fast: false,
                },
                &context(id),
            )
            .await
            .expect("group");
    }
    for id in ["acct_shared", "acct_unknown", "acct_banned", "acct_revoked"] {
        seed_account(&database.pool, id, "openai", id).await;
    }
    assign_accounts(
        &database.pool,
        MIXED_GROUP,
        &["acct_shared", "acct_unknown"],
    )
    .await;
    assign_accounts(&database.pool, EMPTY_GROUP, &["acct_shared"]).await;
    sqlx::query("update provider_accounts set plan_type = 'plus', created_at = $1")
        .bind(now - Duration::minutes(10))
        .execute(&database.pool)
        .await
        .expect("account ages");
    sqlx::query(
        "insert into provider_device_identities
           (provider_kind, upstream_user_id, upstream_account_id, installation_id, created_at, updated_at)
         values ('openai', 'acct_shared-user', '', 'monitor-device-shared', $1, $1),
                ('openai', 'acct_banned-user', '', 'monitor-device-banned', $1, $1)",
    ).bind(now - Duration::minutes(100)).execute(&database.pool).await.expect("durable devices");
    sqlx::query(
        "update provider_accounts set credential_state = 'banned', last_error_reason = 'account_banned',
            credential_observed_at = $1 where id = 'acct_banned'",
    ).bind(now - Duration::minutes(5)).execute(&database.pool).await.expect("known ban");
    sqlx::query(
        "update provider_accounts set credential_state = 'expired', last_error_reason = 'credential_expired', last_error_message = 'token_revoked',
            credential_observed_at = $1 where id = 'acct_revoked'",
    ).bind(now - Duration::minutes(1)).execute(&database.pool).await.expect("token failure");
    for (id, account, group, cost) in [
        ("req_monitor_shared", "acct_shared", MIXED_GROUP, "2"),
        ("req_monitor_other_group", "acct_shared", EMPTY_GROUP, "3"),
        ("req_monitor_unknown", "acct_unknown", MIXED_GROUP, "1"),
        ("req_monitor_old", "acct_shared", MIXED_GROUP, "90"),
        ("req_monitor_future", "acct_shared", MIXED_GROUP, "90"),
        ("req_monitor_prewarm", "acct_shared", MIXED_GROUP, "90"),
        ("req_monitor_failed", "acct_shared", MIXED_GROUP, "90"),
    ] {
        seed_group_cost_snapshot(&database.pool, id, account, group, cost).await;
    }
    sqlx::query("update model_requests set started_at = $1, completed_at = $2")
        .bind(now - Duration::hours(1))
        .bind(now)
        .execute(&database.pool)
        .await
        .expect("long requests");
    sqlx::query("update model_requests set routing_group_refs = array[$1, $2], routing_group_names_snapshot = jsonb_build_array($1::text, $2::text) where id = 'req_monitor_shared'")
        .bind(MIXED_GROUP).bind(EMPTY_GROUP).execute(&database.pool).await.expect("overlapping scope");
    sqlx::query("update model_requests set cost_amount = null, cost_currency = null, cost_source = 'unavailable' where id = 'req_monitor_unknown'")
        .execute(&database.pool).await.expect("unknown cost");
    sqlx::query("update model_requests set completed_at = $1 where id = 'req_monitor_old'")
        .bind(now - Duration::seconds(60))
        .execute(&database.pool)
        .await
        .expect("excluded lower boundary");
    sqlx::query("update model_requests set completed_at = $1 where id = 'req_monitor_future'")
        .bind(now + Duration::seconds(1))
        .execute(&database.pool)
        .await
        .expect("excluded upper boundary");
    sqlx::query(
        "update model_requests set request_kind = 'prewarm' where id = 'req_monitor_prewarm'",
    )
    .execute(&database.pool)
    .await
    .expect("prewarm");
    sqlx::query("update model_requests set outcome = 'failed', client_status_code = 500 where id = 'req_monitor_failed'")
        .execute(&database.pool).await.expect("failed request");

    let revision = current_revision(&database.pool).await;
    let audits = audit_count(&database.pool).await;
    let accounts_before: Vec<serde_json::Value> =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a order by id")
            .fetch_all(&database.pool)
            .await
            .expect("before");
    let facts = groups
        .load_group_monitor(now)
        .await
        .expect("monitor snapshot");
    assert_eq!(facts.groups.len(), 3);
    assert_eq!(facts.members.len(), 3);
    assert_eq!(facts.group_usage[MIXED_GROUP].usd, 2.0);
    assert_eq!(facts.group_usage[MIXED_GROUP].missing_costs, 1);
    assert_eq!(facts.group_usage[EMPTY_GROUP].usd, 5.0);
    assert_eq!(facts.account_usage["acct_shared"].usd, 5.0);
    assert_eq!(facts.account_usage["acct_unknown"].missing_costs, 1);
    let first = facts
        .members
        .iter()
        .find(|m| m.member.account_id == "acct_shared")
        .expect("shared");
    assert!(
        (first.first_seen_at - (now - Duration::minutes(10)))
            .num_milliseconds()
            .abs()
            < 1
    );
    assert_eq!(facts.lifespans.len(), 1);
    assert_eq!(facts.lifespans[0].samples, 1);
    assert!((facts.lifespans[0].minutes - 5.0).abs() < 0.001);
    assert_eq!(current_revision(&database.pool).await, revision);
    assert_eq!(audit_count(&database.pool).await, audits);
    let accounts_after: Vec<serde_json::Value> =
        sqlx::query_scalar("select to_jsonb(a) from provider_accounts a order by id")
            .fetch_all(&database.pool)
            .await
            .expect("after");
    assert_eq!(accounts_before, accounts_after);
    database.close().await;
}

fn group_id(value: &str) -> AccountGroupId {
    AccountGroupId::new(value).expect("valid account group ID")
}

fn group_color(value: &str) -> AccountGroupColor {
    AccountGroupColor::parse(value).expect("valid account group color")
}

fn client_key_id(value: &str) -> ClientApiKeyId {
    ClientApiKeyId::new(value).expect("valid client key ID")
}

fn context(request_id: &str) -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: request_id.to_owned(),
    }
}

async fn seed_account(pool: &sqlx::PgPool, id: &str, provider: &str, name: &str) {
    sqlx::query(
        "insert into provider_accounts (
           id, provider_kind, name, email, upstream_user_id, upstream_account_id,
           plan_type, authentication_kind, provider_credentials_json, credential_revision,
           has_refresh_token, access_token_expires_at, next_refresh_at, enabled,
           credential_state, credential_observed_at, created_at, updated_at
         ) values (
           $1, $2, $3, null, $1 || '-user', null, null, 'oauth', '{}'::jsonb, 1,
           false, null, null, true, 'ready', now(), now(), now()
         )",
    )
    .bind(id)
    .bind(provider)
    .bind(name)
    .execute(pool)
    .await
    .expect("seed provider account");
}

async fn assign_accounts(pool: &sqlx::PgPool, group_id: &str, account_ids: &[&str]) {
    for account_id in account_ids {
        sqlx::query(
            "insert into account_group_accounts (
               account_group_id, provider_account_id, created_at
             )
             values ($1, $2, now())",
        )
        .bind(group_id)
        .bind(account_id)
        .execute(pool)
        .await
        .expect("seed account group membership");
    }
}

async fn seed_group_cost_snapshot(
    pool: &sqlx::PgPool,
    request_id: &str,
    account_id: &str,
    historical_group_id: &str,
    cost_amount: &str,
) {
    sqlx::query(
        "insert into model_requests (
           id, client_api_key_ref, config_revision, protocol, operation, endpoint,
           client_transport, requested_model_id, provider_kind, provider_account_id,
           provider_account_ref, upstream_model_id, upstream_transport, attempt_count,
           upstream_send_state, downstream_committed_at, outcome, client_status_code,
           upstream_status_code, total_tokens, cost_source, cost_amount, cost_currency,
           started_at, deadline_at, completed_at,
           routing_scope, routing_group_refs, routing_group_names_snapshot
         ) values (
           $1, 'key-group-history', 1, 'openai', 'responses', '/v1/responses',
           'http_sse', 'gpt-group', 'openai', $2, $2, 'gpt-group', 'http_sse', 1,
           'sent', now(), 'succeeded', 200, 200, 10,
           'provider_reported', $4::numeric, 'USD', now() - interval '1 minute',
           now() + interval '5 minutes', now(),
           'groups', array[$3]::text[], jsonb_build_array($3::text)
         )",
    )
    .bind(request_id)
    .bind(account_id)
    .bind(historical_group_id)
    .bind(cost_amount)
    .execute(pool)
    .await
    .expect("seed historical group cost snapshot");
}

async fn current_revision(pool: &sqlx::PgPool) -> u64 {
    let value =
        sqlx::query_scalar::<_, i64>("select config_revision from runtime_settings where id = 1")
            .fetch_one(pool)
            .await
            .expect("load config revision");
    u64::try_from(value).expect("positive config revision")
}

async fn audit_count(pool: &sqlx::PgPool) -> u64 {
    let value = sqlx::query_scalar::<_, i64>("select count(*) from admin_audit_events")
        .fetch_one(pool)
        .await
        .expect("count audit rows");
    u64::try_from(value).expect("non-negative audit count")
}
