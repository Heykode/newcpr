use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::{mpsc, oneshot};
use futures::executor::block_on;
use futures::future::BoxFuture;

use gateway_core::account::{AccountSelectionPolicy, ProviderAccountId, RotationStrategy};
use gateway_core::lifecycle::CancellationToken;
use gateway_core::policy::{ClientApiKeyId, PlaintextClientApiKey, RateLimits};
use gateway_core::routing::snapshot::{
    RuntimeSnapshotCompiler, SnapshotAccountGroupFacts, SnapshotAccountGroupMemberFacts,
    SnapshotClientPolicyFacts, SnapshotFacts, SnapshotProviderAccountFacts, SnapshotSettingsFacts,
    SnapshotStoreError, SnapshotStorePort,
};
use gateway_core::routing::{
    AccountGroupId, ConfigRevision, ProviderCatalogGeneration, ProviderCatalogPort,
    ProviderCatalogUnavailable, ProviderKind, ProviderModelCapabilities, RequestTuning,
    RuntimeSnapshot,
};
use gateway_core::runtime::{
    RequestTuningHandle, RuntimeSnapshotHandle, RuntimeSnapshotPublisher,
    RuntimeSnapshotUnavailable, SnapshotControl, SnapshotRevisionStream, SnapshotSubscriptionError,
    SnapshotSubscriptionPort, runtime_revision_needs_refresh,
};
use gateway_core::task::{
    ScheduledTask, WorkerContribution, WorkerCycleContext, WorkerKind, WorkerRunnable,
};

type ReadGate<T> = Arc<Mutex<Option<oneshot::Receiver<Result<T, SnapshotStoreError>>>>>;

mod account_concurrency;
mod publication;

#[derive(Clone)]
struct TestSnapshotStore {
    facts: Arc<Mutex<Result<SnapshotFacts, SnapshotStoreError>>>,
    current_revision: Arc<Mutex<Result<ConfigRevision, SnapshotStoreError>>>,
    next_load: ReadGate<SnapshotFacts>,
    next_revision: ReadGate<ConfigRevision>,
    loads: Arc<AtomicUsize>,
}

impl TestSnapshotStore {
    fn new(facts: Result<SnapshotFacts, SnapshotStoreError>) -> Self {
        let current_revision = facts
            .as_ref()
            .map(SnapshotFacts::config_revision)
            .map_err(Clone::clone);
        Self {
            facts: Arc::new(Mutex::new(facts)),
            current_revision: Arc::new(Mutex::new(current_revision)),
            next_load: Arc::default(),
            next_revision: Arc::default(),
            loads: Arc::default(),
        }
    }
}

impl SnapshotStorePort for TestSnapshotStore {
    fn load_snapshot_facts(&self) -> BoxFuture<'_, Result<SnapshotFacts, SnapshotStoreError>> {
        Box::pin(async move {
            self.loads.fetch_add(1, Ordering::SeqCst);
            let gate = self.next_load.lock().expect("load gate lock").take();
            if let Some(gate) = gate {
                return gate.await.expect("release load");
            }
            self.facts.lock().expect("facts lock").clone()
        })
    }

    fn current_config_revision(&self) -> BoxFuture<'_, Result<ConfigRevision, SnapshotStoreError>> {
        Box::pin(async move {
            let gate = self
                .next_revision
                .lock()
                .expect("revision gate lock")
                .take();
            if let Some(gate) = gate {
                return gate.await.expect("release revision read");
            }
            self.current_revision.lock().expect("revision lock").clone()
        })
    }
}

#[derive(Default)]
struct TestSnapshotSubscriptions {
    published: Mutex<Vec<ConfigRevision>>,
    stream: Mutex<Option<SnapshotRevisionStream>>,
    next_publish: Mutex<Option<oneshot::Receiver<()>>>,
}

impl SnapshotSubscriptionPort for TestSnapshotSubscriptions {
    fn publish_snapshot_revision(
        &self,
        revision: ConfigRevision,
    ) -> BoxFuture<'_, Result<(), SnapshotSubscriptionError>> {
        Box::pin(async move {
            let gate = self.next_publish.lock().expect("publish gate lock").take();
            if let Some(gate) = gate {
                gate.await.expect("release notification");
            }
            self.published
                .lock()
                .expect("published lock")
                .push(revision);
            Ok(())
        })
    }

    fn subscribe_snapshot_revisions(
        &self,
    ) -> BoxFuture<'_, Result<SnapshotRevisionStream, SnapshotSubscriptionError>> {
        Box::pin(async move {
            Ok(self
                .stream
                .lock()
                .expect("subscription stream lock")
                .take()
                .unwrap_or_else(|| Box::pin(futures::stream::pending())))
        })
    }
}

#[test]
fn runtime_revision_reconciliation_should_refresh_missing_or_stale_snapshot() {
    assert!(!runtime_revision_needs_refresh(Some(7), 7));
    assert!(runtime_revision_needs_refresh(Some(6), 7));
    assert!(runtime_revision_needs_refresh(Some(8), 7));
    assert!(runtime_revision_needs_refresh(None, 7));
}

#[test]
fn handle_should_keep_request_snapshot_frozen_across_publish() {
    let handle = RuntimeSnapshotHandle::new(empty_snapshot(1));
    let frozen = handle.acquire().expect("initial snapshot");

    handle.publish(empty_snapshot(2));

    assert_eq!(frozen.revision().get(), 1);
    assert_eq!(handle.revision().map(ConfigRevision::get), Some(2));
}

#[test]
fn request_tuning_handle_should_publish_new_values_without_changing_old_copies() {
    let initial = RequestTuning::default();
    let handle = RequestTuningHandle::new(initial);
    let old = handle.load();
    let mut updated = initial;
    updated.websocket_max_retries = 11;

    handle.publish(updated);

    assert_eq!(old.websocket_max_retries, 5);
    assert_eq!(handle.load().websocket_max_retries, 11);
}

#[test]
fn publisher_should_refresh_locally_and_notify_committed_revision() {
    let store = Arc::new(TestSnapshotStore::new(Ok(facts(2, 2))));
    let compiler = Arc::new(compiler(store));
    let handle = RuntimeSnapshotHandle::new(empty_snapshot(1));
    let subscriptions = Arc::new(TestSnapshotSubscriptions::default());
    let publisher = RuntimeSnapshotPublisher::new(compiler, handle.clone(), subscriptions.clone());

    block_on(publisher.publish_committed(revision(2)));

    assert_eq!(handle.revision().map(ConfigRevision::get), Some(2));
    assert_eq!(
        subscriptions
            .published
            .lock()
            .expect("published lock")
            .as_slice(),
        &[revision(2)],
    );
}

#[test]
fn publisher_should_publish_request_tuning_with_the_new_snapshot() {
    let mut settings = SnapshotSettingsFacts::new(
        3,
        50,
        "smart",
        BTreeMap::from([("public-model".to_owned(), "upstream-model".to_owned())]),
        None,
        None,
    );
    let tuning = RequestTuning {
        rate_limit_cooldown_seconds: 17,
        ..RequestTuning::default()
    };
    settings = settings.with_request_tuning(tuning);
    let store = Arc::new(TestSnapshotStore::new(Ok(SnapshotFacts::new(
        revision(2),
        revision(2),
        settings,
        vec![SnapshotClientPolicyFacts::new(
            ClientApiKeyId::new("key_one").expect("key ID"),
            PlaintextClientApiKey::new("sk_test").expect("plaintext key"),
            Vec::new(),
            RateLimits::unlimited(),
        )],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ))));
    let request_tuning = RequestTuningHandle::new(RequestTuning::default());
    let publisher = RuntimeSnapshotPublisher::new_with_request_tuning(
        Arc::new(compiler(store)),
        RuntimeSnapshotHandle::new(empty_snapshot(1)),
        request_tuning.clone(),
        Arc::new(TestSnapshotSubscriptions::default()),
    );

    block_on(publisher.refresh()).expect("refresh");

    assert_eq!(request_tuning.load().rate_limit_cooldown_seconds, 17);
}

#[test]
fn publisher_should_suspend_but_still_notify_after_committed_refresh_failure() {
    let store = Arc::new(TestSnapshotStore::new(Err(
        SnapshotStoreError::unavailable(),
    )));
    let handle = RuntimeSnapshotHandle::new(empty_snapshot(1));
    let subscriptions = Arc::new(TestSnapshotSubscriptions::default());
    let publisher = RuntimeSnapshotPublisher::new(
        Arc::new(compiler(store)),
        handle.clone(),
        subscriptions.clone(),
    );

    block_on(publisher.publish_committed(revision(2)));

    assert_eq!(
        handle.acquire().expect("stale snapshot grace").revision(),
        revision(1),
    );
    assert_eq!(
        subscriptions
            .published
            .lock()
            .expect("published lock")
            .as_slice(),
        &[revision(2)],
    );
}

#[test]
fn snapshot_ports_and_control_should_remain_object_safe() {
    fn accept_store(_: &dyn SnapshotStorePort) {}
    fn accept_subscriptions(_: &dyn SnapshotSubscriptionPort) {}
    fn accept_control(_: &dyn SnapshotControl) {}

    let store = Arc::new(TestSnapshotStore::new(Ok(facts(1, 1))));
    let subscriptions = Arc::new(TestSnapshotSubscriptions::default());
    let publisher = RuntimeSnapshotPublisher::new(
        Arc::new(compiler(store.clone())),
        RuntimeSnapshotHandle::new(empty_snapshot(1)),
        subscriptions.clone(),
    );

    accept_store(store.as_ref());
    accept_subscriptions(subscriptions.as_ref());
    accept_control(&publisher);
}

#[test]
fn publisher_should_contribute_reconciliation_and_subscription_workers() {
    let store = Arc::new(TestSnapshotStore::new(Ok(facts(1, 1))));
    let publisher = RuntimeSnapshotPublisher::new(
        Arc::new(compiler(store)),
        RuntimeSnapshotHandle::new(empty_snapshot(1)),
        Arc::new(TestSnapshotSubscriptions::default()),
    );

    let contributions = publisher
        .worker_contributions()
        .expect("valid frozen worker definitions");
    let kinds = contributions
        .iter()
        .map(gateway_core::task::WorkerContribution::kind)
        .collect::<Vec<_>>();

    assert_eq!(
        kinds,
        vec![
            WorkerKind::RuntimeSnapshotReconciliation,
            WorkerKind::RuntimeChangeSubscription,
        ],
    );
}

#[test]
fn overlapping_refreshes_should_not_restore_revoked_account_scope() {
    block_on(async {
        let (release, gate) = oneshot::channel();
        let catalog = Arc::new(TestCatalog::default());
        *catalog.next_query.lock().expect("catalog gate lock") = Some(gate);
        let old_tuning = RequestTuning {
            websocket_max_retries: 7,
            ..RequestTuning::default()
        };
        let new_tuning = RequestTuning {
            websocket_max_retries: 11,
            ..RequestTuning::default()
        };
        let store = Arc::new(TestSnapshotStore::new(Ok(scoped_facts_with_tuning(
            10, true, old_tuning,
        ))));
        let handle = RuntimeSnapshotHandle::new(empty_snapshot(9));
        let request_tuning = RequestTuningHandle::default();
        let capacity = request_tuning.account_concurrency();
        let publisher = RuntimeSnapshotPublisher::new_with_request_tuning(
            Arc::new(catalog_compiler(store.clone(), catalog.clone())),
            handle.clone(),
            request_tuning.clone(),
            Arc::new(TestSnapshotSubscriptions::default()),
        );
        let other_publisher = publisher.clone();

        // 旧刷新已读完 revision 10 的完整事实，停在目录查询；不用 sleep 碰调度概率。
        let mut old = Box::pin(publisher.refresh());
        assert!(futures::poll!(old.as_mut()).is_pending());
        assert_eq!(catalog.queries.load(Ordering::SeqCst), 1);
        *store.facts.lock().expect("facts lock") =
            Ok(scoped_facts_with_tuning(11, false, new_tuning));
        *store.current_revision.lock().expect("revision lock") = Ok(revision(11));
        let mut new = Box::pin(other_publisher.refresh());
        assert!(futures::poll!(new.as_mut()).is_pending());
        assert_eq!(store.loads.load(Ordering::SeqCst), 1);
        assert_eq!(handle.revision(), Some(revision(9)));
        assert_eq!(capacity.load(), Ok(None));

        release.send(()).expect("release old catalog query");
        assert_eq!(old.await.expect("old refresh"), revision(10));
        let frozen_old = handle.acquire().expect("old request snapshot");
        assert!(removed_account_allowed(&frozen_old));
        assert_eq!(request_tuning.load(), old_tuning);
        assert!(Arc::ptr_eq(
            &frozen_old.account_concurrency(),
            &capacity.load().expect("available").expect("published"),
        ));
        assert_eq!(new.await.expect("new refresh"), revision(11));
        let current = handle.acquire().expect("new request snapshot");
        assert!(!removed_account_allowed(&current));
        assert_eq!(current.revision(), revision(11));
        assert!(removed_account_allowed(&frozen_old));
        assert_eq!(frozen_old.revision(), revision(10));
        assert_eq!(frozen_old.request_tuning(), old_tuning);
        assert_eq!(current.request_tuning(), new_tuning);
        assert_eq!(request_tuning.load(), new_tuning);
        assert_eq!(frozen_old.account_concurrency().revision(), revision(10));
        let live_capacity = capacity.load().expect("available").expect("published");
        assert_eq!(live_capacity.revision(), revision(11));
        assert!(Arc::ptr_eq(&current.account_concurrency(), &live_capacity,));
    });
}

#[test]
fn failed_committed_refresh_should_suspend_before_next_publication() {
    block_on(async {
        let (release, gate) = oneshot::channel();
        let store = Arc::new(TestSnapshotStore::new(Ok(facts(11, 11))));
        *store.next_load.lock().expect("load gate lock") = Some(gate);
        let handle = RuntimeSnapshotHandle::new(empty_snapshot(9));
        let subscriptions = Arc::new(TestSnapshotSubscriptions::default());
        let tuning = RequestTuningHandle::default();
        let capacity = tuning.account_concurrency();
        let publisher = RuntimeSnapshotPublisher::new_with_request_tuning(
            Arc::new(compiler(store.clone())),
            handle.clone(),
            tuning,
            subscriptions.clone(),
        );
        let other_publisher = publisher.clone();
        let mut old = publisher.publish_committed(revision(10));
        assert!(futures::poll!(old.as_mut()).is_pending());
        let mut new = other_publisher.publish_committed(revision(11));
        assert!(futures::poll!(new.as_mut()).is_pending());
        assert_eq!(store.loads.load(Ordering::SeqCst), 1);

        release
            .send(Err(SnapshotStoreError::unavailable()))
            .expect("release failed facts read");
        old.await;
        assert_eq!(
            handle.acquire().expect("stale snapshot grace").revision(),
            revision(9),
        );
        assert_eq!(capacity.load(), Err(RuntimeSnapshotUnavailable));
        new.await;
        assert_eq!(handle.revision(), Some(revision(11)));
        assert_eq!(
            capacity
                .load()
                .expect("recovered capacity")
                .expect("published capacity")
                .revision(),
            revision(11),
        );
        assert_eq!(
            *subscriptions.published.lock().expect("published lock"),
            vec![revision(10), revision(11)],
        );
    });
}

#[test]
fn reconciliation_revision_failure_should_not_suspend_later_committed_refresh() {
    block_on(async {
        let (release, gate) = oneshot::channel();
        let store = Arc::new(TestSnapshotStore::new(Ok(facts(11, 11))));
        *store.next_revision.lock().expect("revision gate lock") = Some(gate);
        let handle = RuntimeSnapshotHandle::new(empty_snapshot(9));
        let tuning = RequestTuningHandle::default();
        let capacity = tuning.account_concurrency();
        let publisher = RuntimeSnapshotPublisher::new_with_request_tuning(
            Arc::new(compiler(store.clone())),
            handle.clone(),
            tuning,
            Arc::new(TestSnapshotSubscriptions::default()),
        );
        let (task, context) = reconciliation_task(&publisher);
        let mut reconcile = task.run_cycle(context);
        assert!(futures::poll!(reconcile.as_mut()).is_pending());
        let mut committed = publisher.publish_committed(revision(11));
        assert!(futures::poll!(committed.as_mut()).is_pending());
        assert_eq!(store.loads.load(Ordering::SeqCst), 0);

        release
            .send(Err(SnapshotStoreError::unavailable()))
            .expect("release failed revision read");
        assert!(reconcile.await.is_err());
        assert_eq!(
            handle.acquire().expect("stale snapshot grace").revision(),
            revision(9),
        );
        assert_eq!(capacity.load(), Err(RuntimeSnapshotUnavailable));
        committed.await;
        assert_eq!(handle.revision(), Some(revision(11)));
        assert_eq!(
            capacity
                .load()
                .expect("recovered capacity")
                .expect("published capacity")
                .revision(),
            revision(11),
        );
    });
}

#[test]
fn subscription_refresh_should_wait_for_inflight_compile_and_reload_authoritative_facts() {
    block_on(async {
        let (release, gate) = oneshot::channel();
        let catalog = Arc::new(TestCatalog::default());
        *catalog.next_query.lock().expect("catalog gate lock") = Some(gate);
        let store = Arc::new(TestSnapshotStore::new(Ok(scoped_facts(10, true))));
        let handle = RuntimeSnapshotHandle::new(empty_snapshot(9));
        let (notifications, stream) = mpsc::unbounded();
        let subscriptions = Arc::new(TestSnapshotSubscriptions {
            stream: Mutex::new(Some(Box::pin(stream))),
            ..TestSnapshotSubscriptions::default()
        });
        let publisher = RuntimeSnapshotPublisher::new(
            Arc::new(catalog_compiler(store.clone(), catalog)),
            handle.clone(),
            subscriptions,
        );
        let task = publisher
            .worker_contributions()
            .expect("workers")
            .into_iter()
            .find_map(|contribution| match contribution {
                WorkerContribution::Registration(registration) => match registration.runnable {
                    WorkerRunnable::Daemon { task, .. } => Some(task),
                    WorkerRunnable::Scheduled { .. } => None,
                },
                WorkerContribution::Disabled { .. } => None,
            })
            .expect("subscription task");
        let cancellation = CancellationToken::new();
        let mut daemon = task.run(cancellation.clone());
        let mut old = Box::pin(publisher.refresh());
        assert!(futures::poll!(old.as_mut()).is_pending());
        *store.facts.lock().expect("facts lock") = Ok(scoped_facts(11, false));
        // 通知只是提示，即使版本过期也要在取得发布权后重新读取 Store。
        notifications
            .unbounded_send(Ok(revision(3)))
            .expect("notify");
        assert!(futures::poll!(daemon.as_mut()).is_pending());
        assert_eq!(store.loads.load(Ordering::SeqCst), 1);
        assert_eq!(handle.revision(), Some(revision(9)));

        release.send(()).expect("release old catalog query");
        old.await.expect("old refresh");
        assert!(futures::poll!(daemon.as_mut()).is_pending());
        let current = handle.acquire().expect("subscription snapshot");
        assert_eq!(current.revision(), revision(11));
        assert!(!removed_account_allowed(&current));
        cancellation.cancel();
        daemon.await.expect("stop subscription");
    });
}

#[test]
fn reconciliation_should_fail_closed_on_persisted_revision_rollback_and_recover() {
    block_on(async {
        let store = Arc::new(TestSnapshotStore::new(Ok(facts(8, 8))));
        let handle = RuntimeSnapshotHandle::default();
        let tuning = RequestTuningHandle::default();
        let capacity = tuning.account_concurrency();
        let publisher = RuntimeSnapshotPublisher::new_with_request_tuning(
            Arc::new(compiler(store.clone())),
            handle.clone(),
            tuning,
            Arc::new(TestSnapshotSubscriptions::default()),
        );
        publisher.refresh().await.expect("initial publication");
        let frozen = handle.acquire().expect("initial snapshot");
        let frozen_capacity = capacity.load().expect("available").expect("published");
        let (task, context) = reconciliation_task(&publisher);

        *store.current_revision.lock().expect("revision lock") = Ok(revision(7));
        *store.facts.lock().expect("facts lock") = Err(SnapshotStoreError::unavailable());
        assert!(task.run_cycle(context.clone()).await.is_err());
        assert!(Arc::ptr_eq(
            &frozen,
            &handle.acquire().expect("stale snapshot grace"),
        ));
        // Reading a consistent older revision cannot lower already published pool capacity.
        for value in [7, 6] {
            *store.facts.lock().expect("facts lock") = Ok(facts(value, value));
            *store.current_revision.lock().expect("revision lock") = Ok(revision(value));
            assert!(task.run_cycle(context.clone()).await.is_err());
            assert!(Arc::ptr_eq(
                &frozen,
                &handle.acquire().expect("stale snapshot grace"),
            ));
            assert!(Arc::ptr_eq(
                &frozen_capacity,
                &capacity.load().expect("capacity grace").expect("published"),
            ));
        }

        *store.facts.lock().expect("facts lock") = Ok(facts(8, 8));
        *store.current_revision.lock().expect("revision lock") = Ok(revision(8));
        task.run_cycle(context.clone())
            .await
            .expect("recover at high water mark");
        let recovered = handle.acquire().expect("recovered snapshot");
        let recovered_capacity = capacity.load().expect("recovered").expect("published");
        assert_eq!(recovered.revision(), revision(8));
        assert!(!Arc::ptr_eq(&frozen, &recovered));
        assert!(!Arc::ptr_eq(&frozen_capacity, &recovered_capacity));
        assert_eq!(recovered_capacity, frozen_capacity);

        task.run_cycle(context).await.expect("healthy no-op");
        assert!(Arc::ptr_eq(
            &recovered,
            &handle.acquire().expect("still recovered"),
        ));
        assert!(Arc::ptr_eq(
            &recovered_capacity,
            &capacity
                .load()
                .expect("still recovered")
                .expect("published"),
        ));
    });
}

#[test]
fn catalog_only_reconciliation_should_keep_snapshot_on_failure_and_retry_generation() {
    block_on(async {
        let catalog = Arc::new(TestCatalog::default());
        let store = Arc::new(TestSnapshotStore::new(Ok(scoped_facts(10, true))));
        let handle = RuntimeSnapshotHandle::default();
        let publisher = RuntimeSnapshotPublisher::new(
            Arc::new(catalog_compiler(store.clone(), catalog.clone())),
            handle.clone(),
            Arc::new(TestSnapshotSubscriptions::default()),
        );
        publisher.refresh().await.expect("initial snapshot");
        let frozen = handle.acquire().expect("frozen snapshot");
        let (task, context) = reconciliation_task(&publisher);
        task.run_cycle(context.clone()).await.expect("unchanged");
        assert_eq!(store.loads.load(Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(
            &frozen,
            &handle.acquire().expect("unchanged snapshot")
        ));

        catalog.generation.store(1, Ordering::SeqCst);
        *store.facts.lock().expect("facts lock") = Err(SnapshotStoreError::unavailable());
        assert!(task.run_cycle(context.clone()).await.is_err());
        assert!(Arc::ptr_eq(
            &frozen,
            &handle.acquire().expect("retained snapshot")
        ));
        *store.facts.lock().expect("facts lock") = Ok(scoped_facts(10, true));
        task.run_cycle(context)
            .await
            .expect("retry catalog refresh");
        let current = handle.acquire().expect("refreshed catalog");
        assert_eq!(current.revision(), revision(10));
        assert_eq!(
            current
                .provider_catalog_generations()
                .values()
                .copied()
                .collect::<Vec<_>>(),
            vec![ProviderCatalogGeneration::new(1)],
        );
        assert_eq!(
            frozen
                .provider_catalog_generations()
                .values()
                .copied()
                .collect::<Vec<_>>(),
            vec![ProviderCatalogGeneration::new(0)],
        );
    });
}

#[test]
fn dropping_inflight_refresh_should_release_publication_for_next_refresh() {
    block_on(async {
        let (release, gate) = oneshot::channel();
        let store = Arc::new(TestSnapshotStore::new(Ok(facts(11, 11))));
        *store.next_load.lock().expect("load gate lock") = Some(gate);
        let handle = RuntimeSnapshotHandle::new(empty_snapshot(9));
        let publisher = RuntimeSnapshotPublisher::new(
            Arc::new(compiler(store)),
            handle.clone(),
            Arc::new(TestSnapshotSubscriptions::default()),
        );
        let mut cancelled = Box::pin(publisher.refresh());
        assert!(futures::poll!(cancelled.as_mut()).is_pending());
        let mut next = Box::pin(publisher.refresh());
        assert!(futures::poll!(next.as_mut()).is_pending());
        drop(cancelled);
        assert!(release.send(Ok(facts(10, 10))).is_err());
        assert_eq!(next.await.expect("next refresh"), revision(11));
        assert_eq!(handle.revision(), Some(revision(11)));
    });
}

#[test]
fn pending_notification_should_not_block_next_local_refresh() {
    block_on(async {
        let (release, gate) = oneshot::channel();
        let store = Arc::new(TestSnapshotStore::new(Ok(facts(10, 10))));
        let handle = RuntimeSnapshotHandle::new(empty_snapshot(9));
        let subscriptions = Arc::new(TestSnapshotSubscriptions {
            next_publish: Mutex::new(Some(gate)),
            ..TestSnapshotSubscriptions::default()
        });
        let publisher = RuntimeSnapshotPublisher::new(
            Arc::new(compiler(store.clone())),
            handle.clone(),
            subscriptions,
        );
        let mut committed = publisher.publish_committed(revision(10));
        assert!(futures::poll!(committed.as_mut()).is_pending());
        assert_eq!(handle.revision(), Some(revision(10)));
        *store.facts.lock().expect("facts lock") = Ok(facts(11, 11));
        let mut next = Box::pin(publisher.refresh());
        assert_eq!(
            futures::poll!(next.as_mut()),
            std::task::Poll::Ready(Ok(revision(11))),
        );
        release.send(()).expect("release notification");
        committed.await;
        assert_eq!(handle.revision(), Some(revision(11)));
    });
}

fn reconciliation_task(
    publisher: &RuntimeSnapshotPublisher,
) -> (Box<dyn ScheduledTask>, WorkerCycleContext) {
    publisher
        .worker_contributions()
        .expect("workers")
        .into_iter()
        .find_map(|contribution| match contribution {
            WorkerContribution::Registration(registration) => match registration.runnable {
                WorkerRunnable::Scheduled { task, .. } => Some((
                    task,
                    WorkerCycleContext::new(registration.id, None, CancellationToken::new()),
                )),
                WorkerRunnable::Daemon { .. } => None,
            },
            WorkerContribution::Disabled { .. } => None,
        })
        .expect("reconciliation task")
}

#[derive(Default)]
struct TestCatalog {
    generation: AtomicU64,
    queries: AtomicUsize,
    next_query: Mutex<Option<oneshot::Receiver<()>>>,
}

impl ProviderCatalogPort for TestCatalog {
    fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        BTreeMap::from([(
            ProviderKind::new("audit").expect("provider"),
            ProviderCatalogGeneration::new(self.generation.load(Ordering::SeqCst)),
        )])
    }

    fn query_model_capabilities(
        &self,
        _: &ProviderKind,
    ) -> BoxFuture<'_, Result<Vec<ProviderModelCapabilities>, ProviderCatalogUnavailable>> {
        Box::pin(async move {
            self.queries.fetch_add(1, Ordering::SeqCst);
            let gate = self.next_query.lock().expect("catalog gate lock").take();
            if let Some(gate) = gate {
                gate.await.expect("release catalog query");
            }
            Ok(Vec::new())
        })
    }
}

fn catalog_compiler(
    store: Arc<dyn SnapshotStorePort>,
    catalog: Arc<TestCatalog>,
) -> RuntimeSnapshotCompiler {
    RuntimeSnapshotCompiler::new(store, catalog)
}

fn scoped_facts(value: u64, allow_removed: bool) -> SnapshotFacts {
    scoped_facts_with_tuning(value, allow_removed, RequestTuning::default())
}

fn scoped_facts_with_tuning(
    value: u64,
    allow_removed: bool,
    tuning: RequestTuning,
) -> SnapshotFacts {
    let group = AccountGroupId::new(format!("grp_{:032x}", 1)).expect("group");
    let keep = ProviderAccountId::new("acct_audit_keep").expect("kept account");
    let removed = ProviderAccountId::new("acct_audit_removed").expect("removed account");
    let mut memberships = vec![SnapshotAccountGroupMemberFacts::new(
        group.clone(),
        keep.clone(),
    )];
    if allow_removed {
        memberships.push(SnapshotAccountGroupMemberFacts::new(
            group.clone(),
            removed.clone(),
        ));
    }
    SnapshotFacts::new(
        revision(value),
        revision(value),
        SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None)
            .with_request_tuning(tuning),
        vec![SnapshotClientPolicyFacts::new(
            ClientApiKeyId::new("key_audit_synthetic").expect("key ID"),
            PlaintextClientApiKey::new("sk_audit_synthetic_not_a_real_key").expect("synthetic key"),
            vec![group.clone()],
            RateLimits::unlimited(),
        )],
        vec![SnapshotAccountGroupFacts::new(
            group,
            "Synthetic group".to_owned(),
            true,
        )],
        vec![
            SnapshotProviderAccountFacts::new(keep, "audit"),
            SnapshotProviderAccountFacts::new(removed, "audit"),
        ],
        memberships,
    )
}

fn removed_account_allowed(snapshot: &RuntimeSnapshot) -> bool {
    snapshot
        .client_policies()
        .next()
        .expect("client policy")
        .account_scope()
        .allows(&ProviderAccountId::new("acct_audit_removed").expect("removed account"))
}

fn facts(config_revision: u64, observed_current_revision: u64) -> SnapshotFacts {
    SnapshotFacts::new(
        revision(config_revision),
        revision(observed_current_revision),
        SnapshotSettingsFacts::new(
            3,
            50,
            "smart",
            BTreeMap::from([("public-model".to_owned(), "upstream-model".to_owned())]),
            None,
            None,
        ),
        vec![SnapshotClientPolicyFacts::new(
            ClientApiKeyId::new("key_one").expect("key ID"),
            PlaintextClientApiKey::new("sk_test").expect("plaintext key"),
            Vec::new(),
            RateLimits::unlimited(),
        )],
        Vec::<SnapshotAccountGroupFacts>::new(),
        Vec::<SnapshotProviderAccountFacts>::new(),
        Vec::<SnapshotAccountGroupMemberFacts>::new(),
    )
}

fn compiler(store: Arc<dyn SnapshotStorePort>) -> RuntimeSnapshotCompiler {
    catalog_compiler(store, Arc::new(TestCatalog::default()))
}

fn empty_snapshot(value: u64) -> RuntimeSnapshot {
    RuntimeSnapshot::new(
        revision(value),
        AccountSelectionPolicy::new(
            RotationStrategy::Smart,
            NonZeroU32::new(1).expect("positive concurrency"),
            Duration::ZERO,
        ),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .expect("empty snapshot")
}

fn revision(value: u64) -> ConfigRevision {
    ConfigRevision::new(value).expect("positive revision")
}
