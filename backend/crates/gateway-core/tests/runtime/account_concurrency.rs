use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;
use futures::executor::block_on;
use futures::future::BoxFuture;

use gateway_core::account::ProviderAccountId;
use gateway_core::routing::snapshot::{
    RuntimeSnapshotCompileError, RuntimeSnapshotCompiler, SnapshotFacts,
    SnapshotProviderAccountFacts, SnapshotSettingsFacts, SnapshotStoreError, SnapshotStorePort,
};
use gateway_core::routing::{
    ConfigRevision, ProviderCatalogGeneration, ProviderCatalogPort, ProviderCatalogUnavailable,
    ProviderKind, ProviderModelCapabilities, RequestTuning,
};
use gateway_core::runtime::{
    AccountConcurrencyHandle, RequestTuningHandle, RuntimeSnapshotHandle, RuntimeSnapshotPublisher,
    RuntimeSnapshotUnavailable,
};

use super::{TestSnapshotStore, TestSnapshotSubscriptions, empty_snapshot, revision};

#[test]
fn account_concurrency_should_distinguish_unpublished_from_suspended() {
    let handle = AccountConcurrencyHandle::default();
    assert_eq!(handle.load(), Ok(None));

    handle.suspend();
    assert_eq!(handle.load(), Err(RuntimeSnapshotUnavailable));
    assert!(handle.publish(revision(1), BTreeMap::new()));
    let published = handle.load().expect("available").expect("published");
    assert_eq!(published.revision(), revision(1));
    assert_eq!(published.limit_for("missing"), None);
}

#[test]
fn account_concurrency_should_be_live_without_thawing_request_tuning() {
    let tuning = RequestTuningHandle::default();
    let cloned_tuning = tuning.clone();
    let capacity = tuning.account_concurrency();
    let frozen_tuning = tuning.load();
    assert!(capacity.publish(revision(1), limits(8)));
    let frozen_capacity = capacity.load().expect("available").expect("published");

    assert!(
        cloned_tuning
            .account_concurrency()
            .publish(revision(2), limits(2))
    );
    tuning.publish(RequestTuning {
        websocket_max_retries: 19,
        ..frozen_tuning
    });

    assert_eq!(frozen_tuning.websocket_max_retries, 5);
    assert_eq!(cloned_tuning.load().websocket_max_retries, 19);
    assert_eq!(frozen_capacity.limit_for("account"), NonZeroU32::new(8));
    assert_eq!(
        capacity
            .load()
            .expect("available")
            .expect("published")
            .limit_for("account"),
        NonZeroU32::new(2),
    );
    // Reusing old request tuning must never republish its old account capacity.
    cloned_tuning.publish(frozen_tuning);
    assert_eq!(
        capacity
            .load()
            .expect("available")
            .expect("published")
            .revision(),
        revision(2),
    );
}

#[test]
fn account_concurrency_should_preserve_revision_high_water_mark_through_suspension() {
    let handle = AccountConcurrencyHandle::new(revision(8), limits(2));
    let current = handle.load().expect("available").expect("published");
    assert!(!handle.publish(revision(7), limits(20)));
    assert!(!handle.publish(revision(8), limits(20)));
    assert!(Arc::ptr_eq(
        &current,
        &handle.load().expect("available").expect("published"),
    ));

    handle.suspend();
    assert!(!handle.publish(revision(7), limits(20)));
    assert!(!handle.publish(revision(8), limits(20)));
    assert_eq!(
        handle
            .load()
            .expect("stale snapshot grace")
            .expect("published"),
        current,
    );
    assert!(handle.publish(revision(8), limits(2)));
    assert_eq!(
        handle
            .load()
            .expect("recovered")
            .expect("published")
            .limit_for("account"),
        NonZeroU32::new(2),
    );

    handle.suspend();
    assert!(handle.publish(revision(9), limits(5)));
    assert_eq!(
        handle
            .load()
            .expect("recovered")
            .expect("published")
            .limit_for("account"),
        NonZeroU32::new(5),
    );
    assert!(handle.publish(revision(10), BTreeMap::new()));
    assert_eq!(
        handle
            .load()
            .expect("available")
            .expect("published")
            .limit_for("account"),
        None,
    );
}

#[test]
fn publisher_should_publish_live_defaults_overrides_and_deletions_from_the_same_snapshot() {
    let store = Arc::new(TestSnapshotStore::new(Ok(account_facts(
        1,
        5,
        &[
            ("acct_inherited", None),
            ("acct_override", Some(2)),
            ("acct_deleted", None),
        ],
    ))));
    let tuning = RequestTuningHandle::default();
    let capacity = tuning.account_concurrency();
    let snapshots = RuntimeSnapshotHandle::default();
    let publisher = publisher(store.clone(), snapshots.clone(), tuning.clone());
    block_on(publisher.refresh()).expect("initial publication");
    let frozen = snapshots.acquire().expect("frozen request snapshot");
    let original_capacity = capacity.load().expect("available").expect("published");
    assert!(Arc::ptr_eq(
        &original_capacity,
        &frozen.account_concurrency()
    ));
    assert_eq!(
        original_capacity.limit_for("acct_inherited"),
        NonZeroU32::new(5)
    );
    assert_eq!(
        original_capacity.limit_for("acct_override"),
        NonZeroU32::new(2)
    );

    *store.facts.lock().expect("facts lock") = Ok(account_facts(
        2,
        9,
        &[("acct_inherited", None), ("acct_override", Some(2))],
    ));
    block_on(publisher.refresh()).expect("increased default and deleted account");
    let increased = capacity.load().expect("available").expect("published");
    assert_eq!(increased.revision(), revision(2));
    assert_eq!(increased.limit_for("acct_inherited"), NonZeroU32::new(9));
    assert_eq!(increased.limit_for("acct_override"), NonZeroU32::new(2));
    assert_eq!(increased.limit_for("acct_deleted"), None);

    *store.facts.lock().expect("facts lock") = Ok(account_facts(
        3,
        1,
        &[("acct_inherited", None), ("acct_override", None)],
    ));
    block_on(publisher.refresh()).expect("decreased default and cleared override");
    let decreased = capacity.load().expect("available").expect("published");
    assert_eq!(decreased.revision(), revision(3));
    assert_eq!(decreased.limit_for("acct_inherited"), NonZeroU32::new(1));
    assert_eq!(decreased.limit_for("acct_override"), NonZeroU32::new(1));
    assert!(Arc::ptr_eq(
        &decreased,
        &snapshots.acquire().expect("current").account_concurrency(),
    ));
    assert_eq!(frozen.revision(), revision(1));
    assert_eq!(
        frozen.account_concurrency().limit_for("acct_inherited"),
        NonZeroU32::new(5)
    );
    assert_eq!(frozen.request_tuning().rate_limit_cooldown_seconds, 1);
    assert_eq!(tuning.load().rate_limit_cooldown_seconds, 3);

    *store.facts.lock().expect("facts lock") = Ok(account_facts(
        4,
        1,
        &[("acct_inherited", None), ("acct_override", Some(7))],
    ));
    block_on(publisher.refresh()).expect("increase one account override");
    let overridden = capacity.load().expect("available").expect("published");
    assert_eq!(overridden.limit_for("acct_inherited"), NonZeroU32::new(1));
    assert_eq!(overridden.limit_for("acct_override"), NonZeroU32::new(7));
}

#[test]
fn publisher_should_reject_stale_capacity_without_replacing_newer_tuning_or_snapshot() {
    let store = Arc::new(TestSnapshotStore::new(Ok(account_facts(
        8,
        2,
        &[("acct_account", None)],
    ))));
    let tuning = RequestTuningHandle::default();
    let capacity = tuning.account_concurrency();
    let snapshots = RuntimeSnapshotHandle::default();
    let publisher = publisher(store.clone(), snapshots.clone(), tuning.clone());
    block_on(publisher.refresh()).expect("publish newer revision");

    *store.facts.lock().expect("facts lock") = Ok(account_facts(7, 20, &[("acct_account", None)]));
    assert_eq!(
        block_on(publisher.refresh()),
        Err(RuntimeSnapshotCompileError::RevisionChanged)
    );
    assert_eq!(snapshots.revision(), Some(revision(8)));
    assert_eq!(tuning.load().rate_limit_cooldown_seconds, 8);
    assert_eq!(
        capacity
            .load()
            .expect("available")
            .expect("published")
            .limit_for("acct_account"),
        NonZeroU32::new(2),
    );

    publisher.suspend();
    assert_eq!(
        block_on(publisher.refresh()),
        Err(RuntimeSnapshotCompileError::RevisionChanged)
    );
    assert_eq!(
        snapshots
            .acquire()
            .expect("stale snapshot grace")
            .revision(),
        revision(8),
    );
    assert_eq!(
        capacity
            .load()
            .expect("stale snapshot grace")
            .expect("published")
            .limit_for("acct_account"),
        NonZeroU32::new(2),
    );

    *store.facts.lock().expect("facts lock") = Ok(account_facts(8, 2, &[("acct_account", None)]));
    block_on(publisher.refresh()).expect("recover at high water mark");
    assert_eq!(snapshots.revision(), Some(revision(8)));
    assert_eq!(
        capacity
            .load()
            .expect("recovered")
            .expect("published")
            .limit_for("acct_account"),
        NonZeroU32::new(2),
    );
}

#[test]
fn publisher_should_fence_capacity_on_reload_failure_and_recover_at_the_same_revision() {
    let store = Arc::new(TestSnapshotStore::new(Ok(account_facts(
        1,
        5,
        &[("acct_account", None)],
    ))));
    let tuning = RequestTuningHandle::default();
    let capacity = tuning.account_concurrency();
    let snapshots = RuntimeSnapshotHandle::default();
    let publisher = publisher(store.clone(), snapshots.clone(), tuning);
    block_on(publisher.refresh()).expect("initial publication");

    *store.facts.lock().expect("facts lock") = Err(SnapshotStoreError::unavailable());
    assert_eq!(
        block_on(publisher.refresh()),
        Err(RuntimeSnapshotCompileError::StoreUnavailable)
    );
    assert_eq!(
        capacity
            .load()
            .expect("stale snapshot grace")
            .expect("published")
            .limit_for("acct_account"),
        NonZeroU32::new(5),
    );
    assert_eq!(
        snapshots
            .acquire()
            .expect("stale snapshot grace")
            .revision(),
        revision(1),
    );

    *store.facts.lock().expect("facts lock") = Ok(account_facts(1, 5, &[("acct_account", None)]));
    block_on(publisher.refresh()).expect("recover after transient store failure");
    assert_eq!(snapshots.revision(), Some(revision(1)));
    assert_eq!(
        capacity
            .load()
            .expect("recovered")
            .expect("published")
            .limit_for("acct_account"),
        NonZeroU32::new(5),
    );
}

#[test]
fn publisher_should_not_publish_a_read_started_before_explicit_suspension() {
    block_on(async {
        let (release, delayed) = oneshot::channel();
        let store = Arc::new(ScriptedSnapshotStore::new(vec![
            Box::pin(async move { delayed.await.expect("release delayed snapshot") }),
            Box::pin(async { Ok(account_facts(2, 2, &[("acct_account", None)])) }),
        ]));
        let tuning = RequestTuningHandle::default();
        let capacity = tuning.account_concurrency();
        let snapshots = RuntimeSnapshotHandle::new(empty_snapshot(1));
        let publisher = publisher(store.clone(), snapshots.clone(), tuning);
        let mut refreshing = Box::pin(publisher.refresh());
        assert!(futures::poll!(refreshing.as_mut()).is_pending());

        publisher.suspend();
        release
            .send(Ok(account_facts(1, 20, &[("acct_account", None)])))
            .expect("release");
        assert_eq!(
            refreshing.await,
            Err(RuntimeSnapshotCompileError::RevisionChanged)
        );
        assert_eq!(capacity.load(), Err(RuntimeSnapshotUnavailable));
        assert_eq!(
            snapshots
                .acquire()
                .expect("stale snapshot grace")
                .revision(),
            revision(1),
        );

        publisher.refresh().await.expect("fresh read recovers");
        assert_eq!(
            capacity
                .load()
                .expect("available")
                .expect("published")
                .revision(),
            revision(2)
        );
        for _ in 0..10 {
            assert!(capacity.load().expect("memory-only read").is_some());
        }
        assert_eq!(store.reads.load(Ordering::SeqCst), 2);
    });
}

#[test]
fn publisher_should_serialize_failure_fencing_before_a_waiting_successful_reload() {
    block_on(async {
        let (release, delayed) = oneshot::channel();
        let store = Arc::new(ScriptedSnapshotStore::new(vec![
            Box::pin(async move { delayed.await.expect("release failure") }),
            Box::pin(async { Ok(account_facts(2, 2, &[("acct_account", None)])) }),
        ]));
        let tuning = RequestTuningHandle::default();
        let capacity = tuning.account_concurrency();
        let snapshots = RuntimeSnapshotHandle::new(empty_snapshot(1));
        let publisher = publisher(store.clone(), snapshots.clone(), tuning);
        let cloned_publisher = publisher.clone();
        let mut failing = Box::pin(publisher.refresh());
        let mut recovering = Box::pin(cloned_publisher.refresh());
        assert!(futures::poll!(failing.as_mut()).is_pending());
        assert!(futures::poll!(recovering.as_mut()).is_pending());
        assert_eq!(store.reads.load(Ordering::SeqCst), 1);

        release
            .send(Err(SnapshotStoreError::unavailable()))
            .expect("release");
        assert_eq!(
            failing.await,
            Err(RuntimeSnapshotCompileError::StoreUnavailable)
        );
        assert_eq!(capacity.load(), Err(RuntimeSnapshotUnavailable));
        assert_eq!(
            snapshots
                .acquire()
                .expect("stale snapshot grace")
                .revision(),
            revision(1),
        );

        assert_eq!(recovering.await, Ok(revision(2)));
        assert_eq!(store.reads.load(Ordering::SeqCst), 2);
        assert_eq!(snapshots.revision(), Some(revision(2)));
        assert_eq!(
            capacity
                .load()
                .expect("recovered")
                .expect("published")
                .revision(),
            revision(2)
        );
    });
}

type SnapshotLoad = BoxFuture<'static, Result<SnapshotFacts, SnapshotStoreError>>;

struct ScriptedSnapshotStore {
    loads: Mutex<VecDeque<SnapshotLoad>>,
    reads: AtomicUsize,
}

impl ScriptedSnapshotStore {
    fn new(loads: Vec<SnapshotLoad>) -> Self {
        Self {
            loads: Mutex::new(loads.into()),
            reads: AtomicUsize::new(0),
        }
    }
}

impl SnapshotStorePort for ScriptedSnapshotStore {
    fn load_snapshot_facts(&self) -> BoxFuture<'_, Result<SnapshotFacts, SnapshotStoreError>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.loads
            .lock()
            .expect("loads lock")
            .pop_front()
            .expect("scripted load")
    }

    fn current_config_revision(&self) -> BoxFuture<'_, Result<ConfigRevision, SnapshotStoreError>> {
        Box::pin(async { Ok(revision(1)) })
    }
}

struct TestCatalog;

impl ProviderCatalogPort for TestCatalog {
    fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        BTreeMap::from([(
            ProviderKind::new("alpha").expect("provider"),
            ProviderCatalogGeneration::new(0),
        )])
    }

    fn query_model_capabilities(
        &self,
        _: &ProviderKind,
    ) -> BoxFuture<'_, Result<Vec<ProviderModelCapabilities>, ProviderCatalogUnavailable>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

fn publisher(
    store: Arc<dyn SnapshotStorePort>,
    snapshots: RuntimeSnapshotHandle,
    tuning: RequestTuningHandle,
) -> RuntimeSnapshotPublisher {
    RuntimeSnapshotPublisher::new_with_request_tuning(
        Arc::new(RuntimeSnapshotCompiler::new(store, Arc::new(TestCatalog))),
        snapshots,
        tuning,
        Arc::new(TestSnapshotSubscriptions::default()),
    )
}

fn account_facts(
    config_revision: u64,
    default_limit: u32,
    accounts: &[(&str, Option<u32>)],
) -> SnapshotFacts {
    SnapshotFacts::new(
        revision(config_revision),
        revision(config_revision),
        SnapshotSettingsFacts::new(default_limit, 0, "smart", BTreeMap::new(), None, None)
            .with_request_tuning(RequestTuning {
                rate_limit_cooldown_seconds: config_revision,
                ..RequestTuning::default()
            }),
        Vec::new(),
        Vec::new(),
        accounts
            .iter()
            .map(|(id, limit)| {
                SnapshotProviderAccountFacts::new(
                    ProviderAccountId::new(*id).expect("account ID"),
                    "alpha",
                )
                .with_concurrency_limit(*limit)
            })
            .collect(),
        Vec::new(),
    )
}

fn limits(limit: u32) -> BTreeMap<String, NonZeroU32> {
    BTreeMap::from([(
        "account".to_owned(),
        NonZeroU32::new(limit).expect("positive limit"),
    )])
}
