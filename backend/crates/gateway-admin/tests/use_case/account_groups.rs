use std::{
    collections::BTreeMap,
    str::FromStr as _,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use gateway_admin::{
    model::{
        MutationContext, PageSize, Revision,
        account_groups::{
            AccountGroupAccountSummary, AccountGroupCapacity, AccountGroupColor,
            AccountGroupListQuery, AccountGroupMemberFact, AccountGroupMutation, AccountGroupPage,
            AccountGroupRecord, AccountGroupUsage, DeleteAccountGroup, NewAccountGroup,
            SetAccountGroupEnabled, UpdateAccountGroup,
        },
        accounts::AccountRuntimeSnapshot,
        group_monitor::{GroupMonitorFacts, GroupMonitorReport},
        observability::DecimalAmount,
    },
    ports::store::{
        AccountGroupStore, AccountRuntimeStore, AdminStoreError, AdminStoreErrorKind,
        AdminStoreResult,
    },
};
use gateway_core::{
    account::{AccountStatusFacts, CredentialState, QuotaState},
    routing::AccountGroupId,
};

use super::AdminHarness;

const GROUP_ID: &str = "grp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[tokio::test]
async fn group_query_service_enriches_only_current_page_members_with_runtime_facts() {
    let groups = Arc::new(FakeGroupStore::default());
    let runtime = Arc::new(FakeRuntimeStore::default());
    let service = AdminHarness::new()
        .account_groups(groups.clone())
        .account_runtime(runtime.clone())
        .build()
        .await;

    let page = service
        .account_groups()
        .list(AccountGroupListQuery {
            page: 1,
            page_size: PageSize::new(20).expect("page size"),
            search: None,
            enabled: None,
        })
        .await
        .expect("list projected account groups");

    assert_eq!(
        groups
            .requested_groups
            .lock()
            .expect("requested groups")
            .as_slice(),
        [GROUP_ID]
    );
    assert_eq!(
        runtime
            .requested_accounts
            .lock()
            .expect("requested accounts")
            .as_slice(),
        ["acct_available", "acct_limited"]
    );
    let group = &page.items[0];
    assert_eq!(
        group.account_summary,
        AccountGroupAccountSummary {
            available: 1,
            limited: 1,
            total: 2,
        }
    );
    assert_eq!(
        group.capacity,
        AccountGroupCapacity {
            used_slots: Some(2),
            total_slots: 4,
        }
    );
}

#[derive(Default)]
struct FakeGroupStore {
    requested_groups: Mutex<Vec<String>>,
    snapshot: Mutex<Option<GroupMonitorReport>>,
    samples: AtomicUsize,
    failed: AtomicBool,
    delay: std::time::Duration,
    extra_groups: usize,
    client_keys: BTreeMap<String, Vec<String>>,
    extra_quota_peers: Vec<gateway_admin::model::group_monitor::MonitorQuotaPeer>,
}

#[async_trait]
impl AccountGroupStore for FakeGroupStore {
    async fn load_group_monitor(
        &self,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> gateway_admin::ports::store::AdminStoreResult<
        gateway_admin::model::group_monitor::GroupMonitorFacts,
    > {
        self.samples.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        if self.failed.load(Ordering::SeqCst) {
            return Err(unused());
        }
        let groups = self.monitor_groups();
        let members = groups
            .iter()
            .map(|group| {
                let mut account = member("acct_test", 4);
                account.group_id = group.id.clone();
                gateway_admin::model::group_monitor::MonitorMember {
                    member: account,
                    provider: "openai".to_owned(),
                    plan: Some("plus".to_owned()),
                    first_seen_at: Utc::now() - Duration::hours(1),
                }
            })
            .collect();
        Ok(GroupMonitorFacts {
            config_revision: 1,
            groups,
            members,
            group_client_keys: self.client_keys.clone(),
            quota_peers: std::iter::once(gateway_admin::model::group_monitor::MonitorQuotaPeer {
                id: "acct_test".to_owned(),
                provider: "openai".to_owned(),
                plan: Some("plus".to_owned()),
                created_at: Utc::now() - Duration::hours(1),
            })
            .chain(self.extra_quota_peers.clone())
            .collect(),
            ..Default::default()
        })
    }

    async fn save_group_monitor(
        &self,
        report: &GroupMonitorReport,
        _: u64,
    ) -> AdminStoreResult<()> {
        *self.snapshot.lock().expect("monitor snapshot") = Some(report.clone());
        Ok(())
    }

    async fn read_group_monitor(
        &self,
        ids: &[AccountGroupId],
    ) -> AdminStoreResult<Option<GroupMonitorReport>> {
        let groups = self.monitor_groups();
        if ids
            .iter()
            .any(|id| !groups.iter().any(|group| &group.id == id))
        {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                "monitor",
                "unknown group",
            ));
        }
        Ok(self
            .snapshot
            .lock()
            .expect("monitor snapshot")
            .clone()
            .map(|mut report| {
                report.items.retain(|item| ids.contains(&item.group.id));
                report
            }))
    }

    async fn list_account_groups(
        &self,
        query: AccountGroupListQuery,
    ) -> AdminStoreResult<AccountGroupPage> {
        Ok(AccountGroupPage {
            config_revision: Revision::new(1).expect("revision"),
            items: vec![group_record()],
            total: 1,
            page: query.page,
            page_size: query.page_size.get(),
        })
    }

    async fn load_account_group_members(
        &self,
        group_ids: &[AccountGroupId],
    ) -> AdminStoreResult<Vec<AccountGroupMemberFact>> {
        *self.requested_groups.lock().expect("requested groups") = group_ids
            .iter()
            .map(|group_id| group_id.as_str().to_owned())
            .collect();
        Ok(vec![member("acct_available", 4), member("acct_limited", 3)])
    }

    async fn create_account_group(
        &self,
        _: NewAccountGroup,
        _: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation> {
        Err(unused())
    }

    async fn update_account_group(
        &self,
        _: UpdateAccountGroup,
        _: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation> {
        Err(unused())
    }

    async fn set_account_group_enabled(
        &self,
        _: SetAccountGroupEnabled,
        _: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation> {
        Err(unused())
    }

    async fn delete_account_group(
        &self,
        _: DeleteAccountGroup,
        _: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation> {
        Err(unused())
    }
}

#[tokio::test]
async fn monitor_reuses_ungrouped_peer_current_usage_then_switches_to_own_estimate() {
    use super::accounts::{FakeAccountStore, FakeProviderAdmin, account_record, quota_local_usage};
    use gateway_admin::model::{
        accounts::{AccountCost, AccountUsageWindowResult},
        group_monitor::MonitorQuotaPeer,
        provider_credentials::{ProviderQuota, ProviderQuotaWindow, QuotaLocalUsageAttribution},
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let store = FakeAccountStore::new("openai", events.clone());
    let mut donor = account_record("openai");
    donor.id = "acct_peer".to_owned();
    store.set_accounts(vec![account_record("openai"), donor]);
    let provider = FakeProviderAdmin::new("openai", events.clone());
    provider.set_quota(ProviderQuota {
        plan_type: Some("plus".to_owned()),
        observed_at: Some(Utc::now()),
        refresh_token_expires_at: None,
        limit_reached: false,
        provider_data: None,
        windows: vec![ProviderQuotaWindow {
            key: "week".to_owned(),
            group: "weekly".to_owned(),
            label: "7d".to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
            window_seconds: Some(604_800),
            used_percent: Some(10.0),
            reset_at: Some(Utc::now() + Duration::days(1)),
            limit_reached: false,
            local_usage: None,
            provider_data: None,
        }],
    });
    let services = AdminHarness::new()
        .accounts(store.clone())
        .provider(provider)
        .account_runtime(Arc::new(FakeRuntimeStore::default()))
        .account_groups(Arc::new(FakeGroupStore {
            extra_quota_peers: vec![MonitorQuotaPeer {
                id: "acct_peer".to_owned(),
                provider: "openai".to_owned(),
                plan: Some("plus".to_owned()),
                created_at: Utc::now() - Duration::days(1),
            }],
            ..Default::default()
        }))
        .build()
        .await;
    for (own_cost, donor_cost, remaining) in
        [("0", "10", 90.0), ("0", "20", 180.0), ("1", "20", 9.0)]
    {
        store.set_quota_window_usage(
            [("acct_test", own_cost), ("acct_peer", donor_cost)]
                .into_iter()
                .map(|(id, cost)| {
                    let mut usage = quota_local_usage(id, 0);
                    usage.costs = vec![AccountCost {
                        currency: "USD".to_owned(),
                        amount: cost.parse().unwrap(),
                    }];
                    usage.cost_coverage = Default::default();
                    AccountUsageWindowResult {
                        account_id: id.to_owned(),
                        key: "week".to_owned(),
                        usage,
                    }
                })
                .collect(),
        );
        services.group_monitor().sample().await.unwrap();
        let report = services
            .group_monitor()
            .read(vec![group_id()], false)
            .await
            .unwrap();
        assert_eq!(report.items[0].remaining_usd, Some(remaining));
        assert_eq!(report.items[0].estimated_accounts, 1);
    }
    store.set_accounts(vec![account_record("openai")]);
    services
        .group_monitor()
        .sample()
        .await
        .expect("unreadable optional peer does not block own estimate");
    assert_eq!(
        services
            .group_monitor()
            .read(vec![group_id()], false)
            .await
            .unwrap()
            .items[0]
            .remaining_usd,
        Some(9.0)
    );
    store.set_accounts(vec![]);
    assert!(
        services.group_monitor().sample().await.is_err(),
        "required account read failure must not be hidden"
    );
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.contains("commit")
                || event.contains("refresh")
                || *event == "provider.quota")
    );
}

impl FakeGroupStore {
    fn monitor_groups(&self) -> Vec<gateway_admin::model::account_groups::AccountGroupRef> {
        (0..=self.extra_groups)
            .map(|index| {
                let group = group_record();
                gateway_admin::model::account_groups::AccountGroupRef {
                    id: if index == 0 {
                        group.id
                    } else {
                        AccountGroupId::new(format!("grp_{index:032x}")).expect("extra group")
                    },
                    name: group.name,
                    color: group.color,
                    enabled: group.enabled,
                }
            })
            .collect()
    }
}

#[derive(Default)]
struct FakeRuntimeStore {
    requested_accounts: Mutex<Vec<String>>,
    unavailable: bool,
    client_counts: Option<BTreeMap<String, u64>>,
}

#[async_trait]
impl AccountRuntimeStore for FakeRuntimeStore {
    async fn client_in_flight(
        &self,
        _: &[String],
    ) -> AdminStoreResult<Option<BTreeMap<String, u64>>> {
        Ok(self.client_counts.clone())
    }
    async fn active_rate_limits(&self) -> AdminStoreResult<AccountRuntimeSnapshot> {
        Ok(AccountRuntimeSnapshot::default())
    }

    async fn account_runtime(
        &self,
        account_ids: &[String],
    ) -> AdminStoreResult<AccountRuntimeSnapshot> {
        *self.requested_accounts.lock().expect("requested accounts") = account_ids.to_vec();
        if self.unavailable {
            return Err(unused());
        }
        Ok(AccountRuntimeSnapshot {
            rate_limited_until: BTreeMap::from([(
                "acct_limited".to_owned(),
                Utc::now() + Duration::minutes(5),
            )]),
            in_flight: Some(BTreeMap::from([("acct_available".to_owned(), 2)])),
        })
    }
}

#[tokio::test]
async fn monitor_reads_key_occupancy_and_does_not_report_failed_reads_as_idle() {
    use super::accounts::{FakeAccountStore, FakeProviderAdmin};
    for counts in [None, Some(BTreeMap::from([("key-a".to_owned(), 3)]))] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let services = AdminHarness::new()
            .account_groups(Arc::new(FakeGroupStore {
                client_keys: BTreeMap::from([(
                    GROUP_ID.to_owned(),
                    vec!["key-a".to_owned(), "key-a".to_owned()],
                )]),
                ..Default::default()
            }))
            .account_runtime(Arc::new(FakeRuntimeStore {
                client_counts: counts.clone(),
                ..Default::default()
            }))
            .accounts(FakeAccountStore::new("openai", events.clone()))
            .provider(FakeProviderAdmin::new("openai", events))
            .build()
            .await;
        services.group_monitor().sample().await.unwrap();
        let report = services
            .group_monitor()
            .read(vec![group_id()], false)
            .await
            .unwrap();
        let item = &report.items[0];
        assert_eq!(item.used_slots, counts.as_ref().map(|_| 3));
        assert_eq!(item.total_slots, if counts.is_some() { 7 } else { 0 });
    }
}

#[tokio::test]
async fn monitor_reads_shared_snapshots_without_sampling_and_manual_refresh_replaces_them() {
    use super::accounts::{FakeAccountStore, FakeProviderAdmin};

    let events = Arc::new(Mutex::new(Vec::new()));
    let services = AdminHarness::new()
        .account_groups(Arc::new(FakeGroupStore::default()))
        .account_runtime(Arc::new(FakeRuntimeStore::default()))
        .accounts(FakeAccountStore::new("openai", events.clone()))
        .provider(FakeProviderAdmin::new("openai", events.clone()))
        .build()
        .await;
    events.lock().expect("events").clear();
    assert!(
        services
            .group_monitor()
            .read(vec![group_id()], false)
            .await
            .is_err()
    );
    assert!(events.lock().expect("events").is_empty());
    services
        .group_monitor()
        .sample()
        .await
        .expect("background sample");
    for _ in 0..2 {
        let report = services
            .group_monitor()
            .read(vec![group_id()], false)
            .await
            .expect("monitor");
        assert_eq!(report.items[0].eligible_accounts, 1);
        assert_eq!(report.items[0].remaining_status, "learning");
        assert_eq!(report.items[0].used_slots, Some(0));
    }
    let recorded = events.lock().expect("events").clone();
    assert_eq!(
        recorded
            .iter()
            .filter(|event| **event == "store.load_account")
            .count(),
        1
    );
    assert!(!recorded.iter().any(|event| event.contains("commit")
        || event.contains("refresh")
        || *event == "provider.quota"));
    assert!(
        services
            .group_monitor()
            .read(Vec::new(), false)
            .await
            .is_err()
    );
    assert!(
        services
            .group_monitor()
            .read(vec![group_id(), group_id()], true)
            .await
            .is_err()
    );
    assert_eq!(*events.lock().expect("events"), recorded);
    for _ in 0..2 {
        services
            .group_monitor()
            .read(vec![group_id()], true)
            .await
            .expect("each monitor refresh reloads forecasts");
    }
    services
        .group_monitor()
        .read(vec![group_id()], false)
        .await
        .expect("reuse explicitly updated cache");
    let recorded = events.lock().expect("events");
    assert_eq!(
        recorded
            .iter()
            .filter(|event| **event == "store.load_account")
            .count(),
        3
    );
    assert!(!recorded.iter().any(|event| event.contains("commit")
        || event.contains("refresh")
        || *event == "provider.quota"));
}

#[tokio::test]
async fn monitor_runtime_failure_does_not_publish_zero_or_infinite_eta() {
    let services = AdminHarness::new()
        .account_groups(Arc::new(FakeGroupStore::default()))
        .account_runtime(Arc::new(FakeRuntimeStore {
            unavailable: true,
            ..Default::default()
        }))
        .build()
        .await;
    assert!(services.group_monitor().sample().await.is_err());
    assert!(
        services
            .group_monitor()
            .read(vec![group_id()], false)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn monitor_recalculates_each_account_from_current_weekly_usage_without_plan_learning() {
    use super::accounts::{FakeAccountStore, FakeProviderAdmin};
    use gateway_admin::model::{
        accounts::{AccountCost, AccountUsage},
        provider_credentials::{ProviderQuota, ProviderQuotaWindow, QuotaLocalUsageAttribution},
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let provider = FakeProviderAdmin::new("openai", events.clone());
    let mut quota = ProviderQuota {
        plan_type: Some("plus".to_owned()),
        observed_at: Some(Utc::now()),
        refresh_token_expires_at: None,
        windows: vec![ProviderQuotaWindow {
            key: "week".to_owned(),
            group: "weekly".to_owned(),
            label: "7d".to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
            window_seconds: Some(604_800),
            used_percent: Some(1.0),
            reset_at: Some(Utc::now() + Duration::days(1)),
            limit_reached: false,
            local_usage: Some(AccountUsage {
                account_id: "acct_test".to_owned(),
                request_count: 1,
                success_count: 1,
                input_tokens: None,
                output_tokens: None,
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
                image_input_tokens: None,
                image_output_tokens: None,
                image_request_count: 0,
                image_request_failed_count: 0,
                total_tokens: None,
                cost_coverage: Default::default(),
                costs: vec![AccountCost {
                    currency: "USD".to_owned(),
                    amount: "2".parse().unwrap(),
                }],
                last_used_at: None,
                request_buckets: vec![],
                models: vec![],
            }),
            provider_data: None,
        }],
        limit_reached: false,
        provider_data: None,
    };
    provider.set_quota(quota.clone());
    let services = AdminHarness::new()
        .account_groups(Arc::new(FakeGroupStore::default()))
        .account_runtime(Arc::new(FakeRuntimeStore::default()))
        .accounts(FakeAccountStore::new("openai", events))
        .provider(provider.clone())
        .build()
        .await;
    services.group_monitor().sample().await.unwrap();
    let first = services
        .group_monitor()
        .read(vec![group_id()], false)
        .await
        .unwrap();
    assert_eq!(first.items[0].remaining_usd, Some(198.0));
    quota.windows[0].local_usage.as_mut().unwrap().costs[0].amount = "3".parse().unwrap();
    provider.set_quota(quota.clone());
    services.group_monitor().sample().await.unwrap();
    assert_eq!(
        services
            .group_monitor()
            .read(vec![group_id()], false)
            .await
            .unwrap()
            .items[0]
            .remaining_usd,
        Some(297.0)
    );
    quota.windows[0].used_percent = Some(2.0);
    provider.set_quota(quota);
    services.group_monitor().sample().await.unwrap();
    assert_eq!(
        services
            .group_monitor()
            .read(vec![group_id()], false)
            .await
            .unwrap()
            .items[0]
            .remaining_usd,
        Some(147.0)
    );
}

#[tokio::test]
async fn monitor_forecast_failure_does_not_publish_a_learning_snapshot() {
    let services = AdminHarness::new()
        .account_groups(Arc::new(FakeGroupStore::default()))
        .account_runtime(Arc::new(FakeRuntimeStore::default()))
        .build()
        .await;
    assert!(services.group_monitor().sample().await.is_err());
    assert!(
        services
            .group_monitor()
            .read(vec![group_id()], false)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn monitor_samples_all_groups_deduplicates_accounts_and_coalesces_overlapping_requests() {
    use super::accounts::{FakeAccountStore, FakeProviderAdmin};
    let events = Arc::new(Mutex::new(Vec::new()));
    let groups = Arc::new(FakeGroupStore {
        extra_groups: 3,
        delay: std::time::Duration::from_millis(30),
        ..Default::default()
    });
    let services = AdminHarness::new()
        .account_groups(groups.clone())
        .account_runtime(Arc::new(FakeRuntimeStore::default()))
        .accounts(FakeAccountStore::new("openai", events.clone()))
        .provider(FakeProviderAdmin::new("openai", events.clone()))
        .build()
        .await;
    events.lock().expect("events").clear();
    let (background, manual) = tokio::join!(
        services.group_monitor().sample(),
        services.group_monitor().read(vec![group_id()], true),
    );
    background.expect("background");
    assert_eq!(manual.expect("manual").items.len(), 1);
    assert_eq!(groups.samples.load(Ordering::SeqCst), 1);
    assert_eq!(
        groups
            .snapshot
            .lock()
            .expect("snapshot")
            .as_ref()
            .expect("sample")
            .items
            .len(),
        4
    );
    let recorded = events.lock().expect("events").clone();
    assert_eq!(
        recorded
            .iter()
            .filter(|event| **event == "store.load_account")
            .count(),
        1
    );
    assert!(
        !recorded
            .iter()
            .any(|event| event.contains("refresh") || *event == "provider.quota")
    );
    let fourth = groups.monitor_groups()[3].id.clone();
    services
        .group_monitor()
        .read(vec![fourth], false)
        .await
        .expect("unvisited fourth group");
    assert_eq!(groups.samples.load(Ordering::SeqCst), 1);
    let previous = groups
        .snapshot
        .lock()
        .expect("snapshot")
        .clone()
        .expect("sample");
    let (first_manual, second_manual, page_read) = tokio::join!(
        services.group_monitor().read(vec![group_id()], true),
        services.group_monitor().read(vec![group_id()], true),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            services.group_monitor().read(vec![group_id()], false).await
        },
    );
    first_manual.expect("first manual");
    second_manual.expect("coalesced manual");
    assert_eq!(
        page_read
            .expect("page read is not blocked by sampling")
            .generated_at,
        previous.generated_at
    );
    assert_eq!(groups.samples.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn monitor_failed_sample_retains_original_snapshot_and_timestamp() {
    use super::accounts::{FakeAccountStore, FakeProviderAdmin};
    let events = Arc::new(Mutex::new(Vec::new()));
    let groups = Arc::new(FakeGroupStore::default());
    let services = AdminHarness::new()
        .account_groups(groups.clone())
        .account_runtime(Arc::new(FakeRuntimeStore::default()))
        .accounts(FakeAccountStore::new("openai", events.clone()))
        .provider(FakeProviderAdmin::new("openai", events))
        .build()
        .await;
    services
        .group_monitor()
        .sample()
        .await
        .expect("first sample");
    let first = services
        .group_monitor()
        .read(vec![group_id()], false)
        .await
        .expect("first");
    groups.failed.store(true, Ordering::SeqCst);
    let fallback = services
        .group_monitor()
        .read(vec![group_id()], true)
        .await
        .expect("display previous sample when manual refresh fails");
    assert!(fallback.refreshing);
    assert_eq!(fallback.generated_at, first.generated_at);
    assert_eq!(fallback.items, first.items);
    let retained = services
        .group_monitor()
        .read(vec![group_id()], false)
        .await
        .expect("retained");
    assert_eq!(retained.generated_at, first.generated_at);
    assert_eq!(retained.items[0].used_slots, first.items[0].used_slots);
    groups.failed.store(false, Ordering::SeqCst);
    services.group_monitor().sample().await.expect("recovered");
    assert!(
        services
            .group_monitor()
            .read(vec![group_id()], false)
            .await
            .expect("new")
            .generated_at
            > first.generated_at
    );
}

#[tokio::test]
async fn monitor_worker_retries_without_page_reads_on_the_next_ten_second_tick_and_stops_on_shutdown()
 {
    use super::accounts::{FakeAccountStore, FakeProviderAdmin};
    use gateway_core::{
        lifecycle::CancellationToken,
        task::{WorkerContribution, WorkerRunnable},
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let groups = Arc::new(FakeGroupStore::default());
    groups.failed.store(true, Ordering::SeqCst);
    let mut bundle = AdminHarness::new()
        .account_groups(groups.clone())
        .account_runtime(Arc::new(FakeRuntimeStore::default()))
        .accounts(FakeAccountStore::new("openai", events.clone()))
        .provider(FakeProviderAdmin::new("openai", events))
        .build_bundle()
        .await;
    let task = bundle
        .take_worker_contributions()
        .into_iter()
        .find_map(|contribution| match contribution {
            WorkerContribution::Registration(registration)
                if registration.id.owner() == "admin_group_monitor" =>
            {
                match registration.runnable {
                    WorkerRunnable::Daemon { task, restart } => {
                        assert_eq!(
                            restart.initial_backoff(),
                            std::time::Duration::from_secs(10)
                        );
                        Some(task)
                    }
                    _ => panic!("monitor must be a cancellable daemon"),
                }
            }
            _ => None,
        })
        .expect("monitor contribution");
    let cancellation = CancellationToken::new();
    let stop = cancellation.clone();
    let handle = tokio::spawn(async move { task.run(stop).await });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while groups.samples.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    })
    .await
    .expect("first failed tick");
    assert!(
        !handle.is_finished(),
        "a sampling failure must not exit the worker"
    );
    assert!(groups.snapshot.lock().expect("snapshot").is_none());
    groups.failed.store(false, Ordering::SeqCst);
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        while groups.snapshot.lock().expect("snapshot").is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("two unattended ticks");
    cancellation.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(1), handle)
        .await
        .expect("prompt shutdown")
        .expect("worker join")
        .expect("cancelled cleanly");
    assert_eq!(groups.samples.load(Ordering::SeqCst), 2);
    assert!(groups.snapshot.lock().expect("snapshot").is_some());
}

fn group_record() -> AccountGroupRecord {
    let now = Utc::now();
    AccountGroupRecord {
        id: group_id(),
        name: "Primary".to_owned(),
        description: None,
        color: AccountGroupColor::parse("#2563EBFF").expect("color"),
        enabled: true,
        disable_fast: false,
        member_count: 2,
        provider_counts: BTreeMap::from([("openai".to_owned(), 2)]),
        client_key_count: 1,
        account_summary: AccountGroupAccountSummary {
            available: 0,
            limited: 0,
            total: 0,
        },
        capacity: AccountGroupCapacity {
            used_slots: None,
            total_slots: 0,
        },
        usage: AccountGroupUsage {
            today_usd: DecimalAmount::from_str("1").expect("today usage"),
            retained_total_usd: DecimalAmount::from_str("3").expect("retained usage"),
        },
        created_at: now,
        updated_at: now,
    }
}

fn member(account_id: &str, total_slots: u64) -> AccountGroupMemberFact {
    AccountGroupMemberFact {
        group_id: group_id(),
        account_id: account_id.to_owned(),
        status: AccountStatusFacts {
            enabled: true,
            credential_state: CredentialState::Ready,
            access_token_expires_at: None,
            quota: QuotaState::default(),
            rate_limited_until: None,
            last_error_reason: None,
            last_error_message: None,
        },
        total_slots,
    }
}

fn group_id() -> AccountGroupId {
    AccountGroupId::new(GROUP_ID).expect("group ID")
}

fn unused() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "account group",
        "unused test operation",
    )
}
