use chrono::{DateTime, Duration, Utc};
use gateway_admin::{
    model::{
        MutationActor, MutationContext,
        account_groups::{
            AccountGroupColor, DeleteAccountGroup, NewAccountGroup, SetAccountGroupEnabled,
            UpdateAccountGroup,
        },
        group_monitor::{
            GroupMonitorFacts, GroupMonitorReport, MonitorUsage, project_group_monitor,
        },
    },
    ports::store::AccountGroupStore,
};
use gateway_core::routing::AccountGroupId;
use gateway_store::postgres::PgAccountGroupRepository;

use super::TestDatabase;

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "monitor-test".to_owned(),
    }
}

async fn seed(repo: &PgAccountGroupRepository) -> Vec<AccountGroupId> {
    let mut ids = Vec::new();
    for index in 1..=4 {
        let id = AccountGroupId::new(format!("grp_{index:032x}")).expect("group id");
        repo.create_account_group(
            NewAccountGroup {
                id: id.clone(),
                name: format!("Group {index}"),
                description: None,
                color: AccountGroupColor::parse("#16A34AFF").expect("color"),
                disable_fast: false,
            },
            &context(),
        )
        .await
        .expect("create group");
        ids.push(id);
    }
    ids
}

fn report(facts: &GroupMonitorFacts, at: DateTime<Utc>) -> GroupMonitorReport {
    GroupMonitorReport {
        generated_at: at,
        refreshing: false,
        pending_group_ids: Vec::new(),
        items: facts
            .groups
            .iter()
            .cloned()
            .map(|group| {
                let mut item = project_group_monitor(group, &[], &MonitorUsage::default(), Some(0));
                item.total_accounts = 4;
                item.eligible_accounts = 3;
                item.estimated_accounts = 2;
                item.used_slots = None;
                item.total_slots = 9;
                item.remaining_usd = Some(120.25);
                item.remaining_status = "partial";
                item.expected_expiry_usd = None;
                item.expiry_status = "learning";
                item.consume_usd_per_minute = Some(2.5);
                item.quota_consume_usd_per_minute = None;
                item.eta_minutes = None;
                item.eta_status = "unknown";
                item.low_sample = true;
                item.earliest_reset_at = Some(at + Duration::hours(1));
                item
            })
            .collect(),
    }
}

#[tokio::test]
async fn monitor_snapshot_round_trip_reopen_atomic_rollback_and_late_write_protection() {
    let Some(db) = TestDatabase::create("monitor_snapshots").await else {
        return;
    };
    let repo = PgAccountGroupRepository::new(db.pool.clone());
    let ids = seed(&repo).await;
    let at = DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("sample time");
    let facts = repo.load_group_monitor(at).await.expect("global facts");
    assert_eq!(facts.groups.len(), 4);
    let pending = repo.read_group_monitor(&ids[..3]).await.unwrap().unwrap();
    assert!(pending.refreshing);
    assert!(pending.items.is_empty());
    assert_eq!(pending.pending_group_ids, ids[..3]);
    assert_eq!(pending.generated_at, DateTime::<Utc>::UNIX_EPOCH);
    assert!(
        repo.save_group_monitor(&pending, facts.config_revision)
            .await
            .is_err()
    );
    let expected = report(&facts, at);
    let revision: i64 =
        sqlx::query_scalar("select config_revision from runtime_settings where id = 1")
            .fetch_one(&db.pool)
            .await
            .expect("revision");
    let audits: i64 = sqlx::query_scalar("select count(*) from admin_audit_events")
        .fetch_one(&db.pool)
        .await
        .expect("audits");
    repo.save_group_monitor(&expected, facts.config_revision)
        .await
        .expect("save");
    let reopened = PgAccountGroupRepository::new(db.pool.clone());
    let mut first_page = expected.clone();
    first_page
        .items
        .retain(|item| ids[..3].contains(&item.group.id));
    assert_eq!(
        reopened
            .read_group_monitor(&ids[..3])
            .await
            .expect("reopen"),
        Some(first_page.clone())
    );
    assert_eq!(
        reopened
            .read_group_monitor(&ids[3..])
            .await
            .expect("unvisited fourth")
            .expect("snapshot")
            .generated_at,
        at
    );

    let mut broken = expected.clone();
    broken.generated_at = at + Duration::seconds(10);
    broken.items[0].remaining_usd = Some(999.0);
    broken.items[1].total_slots = u64::MAX;
    assert!(
        repo.save_group_monitor(&broken, facts.config_revision)
            .await
            .is_err()
    );
    assert_eq!(
        repo.read_group_monitor(&ids[..3])
            .await
            .expect("after rollback"),
        Some(first_page.clone())
    );

    let mut late = expected.clone();
    late.generated_at = at - Duration::seconds(10);
    late.items[0].remaining_usd = Some(888.0);
    repo.save_group_monitor(&late, facts.config_revision)
        .await
        .expect("late write");
    assert_eq!(
        repo.read_group_monitor(&ids[..3])
            .await
            .expect("late ignored"),
        Some(first_page)
    );
    let count: i64 = sqlx::query_scalar("select count(*) from account_group_monitor_snapshots")
        .fetch_one(&db.pool)
        .await
        .expect("one snapshot per group");
    assert_eq!(count, 4);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select config_revision from runtime_settings where id = 1")
            .fetch_one(&db.pool)
            .await
            .expect("unchanged revision"),
        revision
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from admin_audit_events")
            .fetch_one(&db.pool)
            .await
            .expect("unchanged audits"),
        audits
    );
    db.close().await;
}

#[tokio::test]
async fn monitor_snapshot_config_changes_retain_previous_values_and_deletion_removes_only_its_snapshot()
 {
    let Some(db) = TestDatabase::create("monitor_snapshot_config").await else {
        return;
    };
    let repo = PgAccountGroupRepository::new(db.pool.clone());
    let ids = seed(&repo).await;
    let at = DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("sample time");
    let facts = repo.load_group_monitor(at).await.expect("facts");
    repo.save_group_monitor(&report(&facts, at), facts.config_revision)
        .await
        .expect("sample");
    repo.update_account_group(
        UpdateAccountGroup {
            id: ids[0].clone(),
            name: "Renamed".to_owned(),
            description: None,
            color: AccountGroupColor::parse("#2563EBFF").expect("color"),
            disable_fast: None,
        },
        &context(),
    )
    .await
    .expect("changed config");
    let retained = repo.read_group_monitor(&ids[..3]).await.unwrap().unwrap();
    assert!(retained.refreshing);
    assert_eq!(retained.generated_at, at);
    assert_eq!(retained.items[0].group.name, "Renamed");
    assert_eq!(retained.items[0].remaining_usd, Some(120.25));
    assert!(
        repo.save_group_monitor(&retained, facts.config_revision)
            .await
            .is_err()
    );
    repo.save_group_monitor(
        &report(&facts, at + Duration::seconds(10)),
        facts.config_revision,
    )
    .await
    .expect("old configuration remains displayable");
    assert!(
        repo.read_group_monitor(&ids[..3])
            .await
            .unwrap()
            .unwrap()
            .refreshing
    );
    let current = repo.load_group_monitor(at).await.expect("new facts");
    repo.save_group_monitor(
        &report(&current, at + Duration::seconds(20)),
        current.config_revision,
    )
    .await
    .expect("new sample");
    assert!(
        !repo
            .read_group_monitor(&ids[..3])
            .await
            .unwrap()
            .unwrap()
            .refreshing
    );
    assert_eq!(
        repo.read_group_monitor(&ids[..1])
            .await
            .expect("current")
            .expect("sample")
            .items[0]
            .group
            .name,
        "Renamed"
    );
    repo.delete_account_group(DeleteAccountGroup { id: ids[3].clone() }, &context())
        .await
        .expect("delete group");
    assert!(repo.read_group_monitor(&ids[3..]).await.is_err());
    let count: i64 = sqlx::query_scalar("select count(*) from account_group_monitor_snapshots")
        .fetch_one(&db.pool)
        .await
        .expect("cascade");
    assert_eq!(count, 3);
    repo.save_group_monitor(
        &report(&current, at + Duration::seconds(30)),
        current.config_revision,
    )
    .await
    .expect("group deleted while sampling is skipped");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from account_group_monitor_snapshots")
            .fetch_one(&db.pool)
            .await
            .expect("no resurrection"),
        3
    );
    db.close().await;
}

#[tokio::test]
async fn new_unsampled_group_does_not_hide_existing_groups_and_disabled_state_is_current() {
    let Some(db) = TestDatabase::create("monitor_snapshot_pending").await else {
        return;
    };
    let repo = PgAccountGroupRepository::new(db.pool.clone());
    let ids = seed(&repo).await;
    let at = DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap();
    let facts = repo.load_group_monitor(at).await.unwrap();
    let mut sample = report(&facts, at);
    sample.items.retain(|item| item.group.id != ids[1]);
    sample.items[0].expiry_status = "all_accounts_outlived_average";
    sample.items[1].expiry_status = "lifespan_learning";
    sample.items[2].expiry_status = "rate_sampling";
    repo.save_group_monitor(&sample, facts.config_revision)
        .await
        .unwrap();
    // Reopening the repository must retain samples, not just a page-local cache.
    let reopened = PgAccountGroupRepository::new(db.pool.clone());
    let read = reopened
        .read_group_monitor(&ids[..3])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.items.len(), 2);
    assert_eq!(read.pending_group_ids, vec![ids[1].clone()]);
    assert!(read.refreshing);
    assert_eq!(read.generated_at, at);
    assert_eq!(read.items[0].expiry_status, "all_accounts_outlived_average");
    assert_eq!(read.items[1].expiry_status, "lifespan_learning");
    assert_eq!(
        reopened
            .read_group_monitor(&ids[3..])
            .await
            .unwrap()
            .unwrap()
            .items[0]
            .expiry_status,
        "rate_sampling"
    );
    repo.set_account_group_enabled(
        SetAccountGroupEnabled {
            id: ids[0].clone(),
            enabled: false,
        },
        &context(),
    )
    .await
    .unwrap();
    let disabled = reopened
        .read_group_monitor(&ids[..1])
        .await
        .unwrap()
        .unwrap();
    assert!(disabled.refreshing);
    assert!(!disabled.items[0].group.enabled);
    assert_eq!(disabled.items[0].remaining_status, "disabled");
    assert_eq!(disabled.items[0].expiry_status, "disabled");
    assert_eq!(disabled.items[0].eta_status, "disabled");
    assert_eq!(disabled.items[0].eligible_accounts, 0);
    assert_eq!(disabled.generated_at, at);
    db.close().await;
}
