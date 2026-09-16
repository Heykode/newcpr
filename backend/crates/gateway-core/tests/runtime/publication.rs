//! Runtime publication regressions through public handles and the reconciliation worker.

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures::executor::block_on;
use futures::future::BoxFuture;

use gateway_core::account::ProviderAccountId;
use gateway_core::routing::snapshot::{
    RuntimeSnapshotCompileError, RuntimeSnapshotCompiler, SnapshotFacts,
    SnapshotProviderAccountFacts, SnapshotSettingsFacts, SnapshotStoreError,
};
use gateway_core::routing::{
    ProviderCatalogGeneration, ProviderCatalogPort, ProviderCatalogUnavailable, ProviderKind,
    ProviderModelCapabilities, RequestTuning,
};
use gateway_core::runtime::{
    AccountConcurrencyHandle, RUNTIME_SNAPSHOT_STALE_GRACE, RequestTuningHandle,
    RuntimeSnapshotHandle, RuntimeSnapshotPublisher, RuntimeSnapshotUnavailable,
};
use gateway_core::task::{
    WorkerContribution, WorkerCycleContext, WorkerKind, WorkerRunnable, WorkerTaskError,
};

use super::{TestSnapshotStore, TestSnapshotSubscriptions, revision};

#[test]
fn reconciliation_should_clear_both_grace_deadlines_after_same_revision_probe_recovers() {
    let runtime = TestRuntime::new();
    let frozen = runtime.snapshots.acquire().expect("initial snapshot");
    let capacity = runtime
        .capacity
        .load()
        .expect("available")
        .expect("published");
    *runtime
        .store
        .current_revision
        .lock()
        .expect("revision lock") = Err(SnapshotStoreError::unavailable());
    assert!(runtime.reconcile().is_err());
    assert!(runtime.reconcile().is_err());
    assert!(Arc::ptr_eq(
        &frozen,
        &runtime.snapshots.acquire().expect("snapshot grace"),
    ));
    assert!(Arc::ptr_eq(
        &capacity,
        &runtime
            .capacity
            .load()
            .expect("capacity grace")
            .expect("published"),
    ));
    *runtime
        .store
        .current_revision
        .lock()
        .expect("revision lock") = Ok(revision(8));
    runtime.reconcile().expect("same revision recovery");

    let recovered = runtime.snapshots.acquire().expect("recovered snapshot");
    let recovered_capacity = runtime
        .capacity
        .load()
        .expect("recovered")
        .expect("published");
    assert!(!Arc::ptr_eq(&frozen, &recovered));
    assert!(!Arc::ptr_eq(&capacity, &recovered_capacity));
    assert_eq!(recovered.revision(), revision(8));
    assert_eq!(recovered_capacity, capacity);

    // A remaining suspension would force another publication even at the same revision.
    runtime.reconcile().expect("healthy no-op");
    assert!(Arc::ptr_eq(
        &recovered,
        &runtime.snapshots.acquire().expect("still recovered"),
    ));
    assert!(Arc::ptr_eq(
        &recovered_capacity,
        &runtime
            .capacity
            .load()
            .expect("still recovered")
            .expect("published"),
    ));
}

#[test]
fn reconciliation_should_recover_either_independently_suspended_handle() {
    for capacity_only in [false, true] {
        let runtime = TestRuntime::new();
        let frozen = runtime.snapshots.acquire().expect("initial snapshot");
        let capacity = runtime
            .capacity
            .load()
            .expect("available")
            .expect("published");
        if capacity_only {
            runtime.capacity.suspend();
        } else {
            runtime.snapshots.suspend();
        }
        runtime.reconcile().expect("recover one suspended handle");
        let recovered = runtime.snapshots.acquire().expect("recovered snapshot");
        let recovered_capacity = runtime
            .capacity
            .load()
            .expect("recovered")
            .expect("published");
        assert!(!Arc::ptr_eq(&frozen, &recovered));
        assert!(!Arc::ptr_eq(&capacity, &recovered_capacity));
        assert_eq!(recovered.revision(), revision(8));
        assert_eq!(recovered_capacity, capacity);
        runtime.reconcile().expect("healthy no-op");
        assert!(Arc::ptr_eq(
            &recovered,
            &runtime.snapshots.acquire().expect("still recovered"),
        ));
        assert!(Arc::ptr_eq(
            &recovered_capacity,
            &runtime
                .capacity
                .load()
                .expect("still recovered")
                .expect("published"),
        ));
    }
}

#[test]
fn rejected_publication_should_expire_without_downgrading_or_extending_grace() {
    let cases = [(7, 20), (8, 20)].map(|(rejected_revision, rejected_limit)| {
        let runtime = TestRuntime::new();
        let frozen = runtime.snapshots.acquire().expect("initial snapshot");
        let capacity = runtime
            .capacity
            .load()
            .expect("available")
            .expect("published");
        *runtime.store.facts.lock().expect("facts lock") =
            Ok(facts(rejected_revision, rejected_limit));

        assert_eq!(
            block_on(runtime.publisher.refresh()),
            Err(RuntimeSnapshotCompileError::RevisionChanged),
        );
        assert!(Arc::ptr_eq(
            &frozen,
            &runtime.snapshots.acquire().expect("snapshot grace"),
        ));
        assert!(Arc::ptr_eq(
            &capacity,
            &runtime
                .capacity
                .load()
                .expect("capacity grace")
                .expect("published"),
        ));
        assert_eq!(runtime.tuning.load().rate_limit_cooldown_seconds, 8);
        (runtime, frozen, capacity)
    });
    let catalog_cases = [Err(SnapshotStoreError::unavailable()), Ok(facts(8, 20))].map(|next| {
        let runtime = TestRuntime::new();
        let frozen = runtime.snapshots.acquire().expect("initial snapshot");
        let capacity = runtime
            .capacity
            .load()
            .expect("available")
            .expect("published");
        runtime.fail_catalog_refresh(next);
        (runtime, frozen, capacity)
    });

    // Rejections and catalog-only controls share one grace window. Retry halfway
    // through so a reset deadline would still be live when the original grace expires.
    let deadline = Instant::now() + RUNTIME_SNAPSHOT_STALE_GRACE + Duration::from_millis(50);
    std::thread::sleep(RUNTIME_SNAPSHOT_STALE_GRACE / 2);
    for (runtime, _, _) in &cases {
        assert_eq!(
            block_on(runtime.publisher.refresh()),
            Err(RuntimeSnapshotCompileError::RevisionChanged),
        );
    }
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));

    for (runtime, frozen, capacity) in catalog_cases {
        assert!(Arc::ptr_eq(
            &frozen,
            &runtime
                .snapshots
                .acquire()
                .expect("catalog-only failure must not expire requests"),
        ));
        assert!(Arc::ptr_eq(
            &capacity,
            &runtime
                .capacity
                .load()
                .expect("catalog-only failure must not expire capacity")
                .expect("published"),
        ));
    }

    for (runtime, frozen, capacity) in cases {
        assert!(runtime.snapshots.acquire().is_err());
        assert_eq!(runtime.snapshots.revision(), None);
        assert_eq!(runtime.capacity.load(), Err(RuntimeSnapshotUnavailable));
        assert_eq!(
            block_on(runtime.publisher.refresh()),
            Err(RuntimeSnapshotCompileError::RevisionChanged),
        );
        assert!(runtime.snapshots.acquire().is_err());
        assert_eq!(runtime.capacity.load(), Err(RuntimeSnapshotUnavailable));
        let conflicting_limits = BTreeMap::from([(
            "acct_account".to_owned(),
            NonZeroU32::new(20).expect("positive limit"),
        )]);
        assert!(
            !runtime
                .capacity
                .publish(revision(7), conflicting_limits.clone())
        );
        assert!(!runtime.capacity.publish(revision(8), conflicting_limits));
        assert_eq!(runtime.capacity.load(), Err(RuntimeSnapshotUnavailable));
        assert_eq!(runtime.tuning.load().rate_limit_cooldown_seconds, 8);

        *runtime.store.facts.lock().expect("facts lock") = Ok(facts(8, 2));
        runtime.reconcile().expect("recover at high water mark");
        let recovered = runtime.snapshots.acquire().expect("recovered snapshot");
        let recovered_capacity = runtime
            .capacity
            .load()
            .expect("recovered")
            .expect("published");
        assert_eq!(recovered.revision(), revision(8));
        assert!(!Arc::ptr_eq(&frozen, &recovered));
        assert!(!Arc::ptr_eq(&capacity, &recovered_capacity));
        assert_eq!(recovered_capacity, capacity);
        assert_eq!(
            recovered_capacity.limit_for("acct_account"),
            NonZeroU32::new(2)
        );
        runtime.reconcile().expect("healthy no-op");
        assert!(Arc::ptr_eq(
            &recovered,
            &runtime.snapshots.acquire().expect("still recovered"),
        ));
        assert!(Arc::ptr_eq(
            &recovered_capacity,
            &runtime
                .capacity
                .load()
                .expect("still recovered")
                .expect("published"),
        ));
    }
}

#[test]
fn catalog_only_compile_or_publication_failure_should_not_suspend_healthy_state() {
    for next in [Err(SnapshotStoreError::unavailable()), Ok(facts(8, 20))] {
        let runtime = TestRuntime::new();
        let frozen = runtime.snapshots.acquire().expect("initial snapshot");
        let capacity = runtime
            .capacity
            .load()
            .expect("available")
            .expect("published");
        runtime.fail_catalog_refresh(next);
        assert!(Arc::ptr_eq(
            &frozen,
            &runtime.snapshots.acquire().expect("retained snapshot"),
        ));
        assert!(Arc::ptr_eq(
            &capacity,
            &runtime
                .capacity
                .load()
                .expect("available")
                .expect("published"),
        ));

        *runtime.store.facts.lock().expect("facts lock") = Ok(facts(8, 2));
        runtime.reconcile().expect("retry catalog-only refresh");
        assert!(!Arc::ptr_eq(
            &frozen,
            &runtime.snapshots.acquire().expect("updated catalog"),
        ));
        let updated_capacity = runtime
            .capacity
            .load()
            .expect("available")
            .expect("published");
        assert!(!Arc::ptr_eq(&capacity, &updated_capacity));
        assert_eq!(updated_capacity, capacity);
    }
}

struct TestRuntime {
    store: Arc<TestSnapshotStore>,
    catalog: Arc<TestCatalog>,
    tuning: RequestTuningHandle,
    capacity: AccountConcurrencyHandle,
    snapshots: RuntimeSnapshotHandle,
    publisher: RuntimeSnapshotPublisher,
}

impl TestRuntime {
    fn new() -> Self {
        let store = Arc::new(TestSnapshotStore::new(Ok(facts(8, 2))));
        let catalog = Arc::new(TestCatalog::default());
        let tuning = RequestTuningHandle::default();
        let capacity = tuning.account_concurrency();
        let snapshots = RuntimeSnapshotHandle::default();
        let publisher = RuntimeSnapshotPublisher::new_with_request_tuning(
            Arc::new(RuntimeSnapshotCompiler::new(store.clone(), catalog.clone())),
            snapshots.clone(),
            tuning.clone(),
            Arc::new(TestSnapshotSubscriptions::default()),
        );
        block_on(publisher.refresh()).expect("initial publication");
        Self {
            store,
            catalog,
            tuning,
            capacity,
            snapshots,
            publisher,
        }
    }

    fn fail_catalog_refresh(&self, next: Result<SnapshotFacts, SnapshotStoreError>) {
        self.catalog.generation.store(1, Ordering::SeqCst);
        *self.store.facts.lock().expect("facts lock") = next;
        assert!(self.reconcile().is_err());
    }

    fn reconcile(&self) -> Result<(), WorkerTaskError> {
        let registration = self
            .publisher
            .worker_contributions()
            .expect("worker definitions")
            .into_iter()
            .find_map(|worker| match worker {
                WorkerContribution::Registration(registration)
                    if registration.id.kind() == WorkerKind::RuntimeSnapshotReconciliation =>
                {
                    Some(registration)
                }
                _ => None,
            })
            .expect("reconciliation worker");
        let WorkerRunnable::Scheduled { task, .. } = registration.runnable else {
            panic!("reconciliation must be scheduled");
        };
        block_on(task.run_cycle(WorkerCycleContext::new(
            registration.id,
            None,
            gateway_core::lifecycle::CancellationToken::new(),
        )))
    }
}

#[derive(Default)]
struct TestCatalog {
    generation: AtomicU64,
}

impl ProviderCatalogPort for TestCatalog {
    fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        BTreeMap::from([(
            ProviderKind::new("alpha").expect("provider"),
            ProviderCatalogGeneration::new(self.generation.load(Ordering::SeqCst)),
        )])
    }

    fn query_model_capabilities(
        &self,
        _: &ProviderKind,
    ) -> BoxFuture<'_, Result<Vec<ProviderModelCapabilities>, ProviderCatalogUnavailable>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

fn facts(config_revision: u64, limit: u32) -> SnapshotFacts {
    SnapshotFacts::new(
        revision(config_revision),
        revision(config_revision),
        SnapshotSettingsFacts::new(5, 0, "smart", BTreeMap::new(), None, None).with_request_tuning(
            RequestTuning {
                rate_limit_cooldown_seconds: config_revision,
                ..RequestTuning::default()
            },
        ),
        Vec::new(),
        Vec::new(),
        vec![
            SnapshotProviderAccountFacts::new(
                ProviderAccountId::new("acct_account").expect("account ID"),
                "alpha",
            )
            .with_concurrency_limit(Some(limit)),
        ],
        Vec::new(),
    )
}
