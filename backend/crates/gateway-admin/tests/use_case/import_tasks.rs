use std::{sync::Arc, time::Duration};

use gateway_admin::{
    AdminBundle, AdminServices,
    model::{
        AdminErrorKind, MutationActor, MutationContext,
        import_tasks::{ImportTaskInput, SubmitImportTask},
        provider_credentials::ImportCredentials,
    },
    ports::provider::ProviderAdminErrorKind,
};
use gateway_core::{
    lifecycle::CancellationToken,
    routing::ProviderKind,
    task::{WorkerContribution, WorkerKind, WorkerRunnable},
};
use uuid::Uuid;

use super::{
    AdminHarness,
    accounts::{
        EventLog, FakeAccountStore, FakeProviderAdmin, context, document, events, import_settings,
        recorded,
    },
};

fn command(id: Uuid, count: usize) -> SubmitImportTask {
    let context = context("task-submit");
    SubmitImportTask {
        submission_id: id,
        fingerprint: [1; 32],
        context: context.clone(),
        items: (0..count)
            .map(|_| ImportTaskInput {
                provider: ProviderKind::new("openai").unwrap(),
                command: ImportCredentials {
                    outbound_proxy_id: None,
                    settings: None,
                    context: context.clone(),
                    document: document(),
                },
            })
            .collect(),
    }
}

async fn harness() -> (AdminBundle, Arc<FakeProviderAdmin>, EventLog) {
    let log = events();
    let provider = FakeProviderAdmin::new("openai", log.clone());
    let bundle = AdminHarness::new()
        .provider(provider.clone())
        .accounts(FakeAccountStore::new("openai", log.clone()))
        .build_bundle()
        .await;
    (bundle, provider, log)
}

fn start(bundle: &mut AdminBundle) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let registrations: Vec<_> = bundle
        .take_worker_contributions()
        .into_iter()
        .filter_map(|contribution| match contribution {
            WorkerContribution::Registration(registration)
                if registration.id.kind() == WorkerKind::AccountImport =>
            {
                Some(registration)
            }
            _ => None,
        })
        .collect();
    assert_eq!(registrations.len(), 1);
    let registration = registrations.into_iter().next().unwrap();
    assert_eq!(registration.id.owner(), "admin");
    assert!(bundle.take_worker_contributions().is_empty());
    let WorkerRunnable::Daemon { task, .. } = registration.runnable else {
        panic!("import worker must be a Host daemon")
    };
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let handle = tokio::spawn(async move {
        task.run(token).await.expect("import daemon exits cleanly");
    });
    (cancellation, handle)
}

async fn wait_started(log: &EventLog, count: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while recorded(log)
            .iter()
            .filter(|event| **event == "provider.prepare_import")
            .count()
            < count
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("imports started");
}

async fn wait_finished(services: &AdminServices, id: Uuid) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while services
            .import_tasks()
            .detail(&context("poll"), id)
            .unwrap()
            .summary
            .finished_at
            .is_none()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("imports finished");
}

#[tokio::test]
async fn retry_returns_same_task_but_rejects_changed_content_even_after_stop() {
    let (bundle, _, _) = harness().await;
    let services = bundle.services();
    let id = Uuid::now_v7();
    let first = services.import_tasks().submit(command(id, 2)).unwrap();
    let retry = services.import_tasks().submit(command(id, 2)).unwrap();
    assert_eq!(first.task_id, retry.task_id);
    assert_eq!(
        services
            .import_tasks()
            .list(&context("reopened-page"))
            .len(),
        1
    );
    services
        .import_tasks()
        .stop(&context("stop"), first.task_id)
        .unwrap();
    let retry = services.import_tasks().submit(command(id, 2)).unwrap();
    assert_eq!(first.task_id, retry.task_id);
    assert_eq!(retry.counts.skipped, 2);
    let mut changed = command(id, 2);
    changed.fingerprint = [2; 32];
    assert_eq!(
        services.import_tasks().submit(changed).unwrap_err().kind(),
        AdminErrorKind::Conflict
    );
}

#[tokio::test]
async fn reads_stop_and_submission_ids_are_isolated_by_actor_not_request_id() {
    let (bundle, _, _) = harness().await;
    let services = bundle.services();
    let id = Uuid::now_v7();
    let task = services.import_tasks().submit(command(id, 1)).unwrap();
    let other = MutationContext {
        actor: MutationActor::AdminSession {
            admin_user_id: "another-admin".to_owned(),
        },
        request_id: "other-session".to_owned(),
    };
    assert!(services.import_tasks().list(&other).is_empty());
    assert_eq!(
        services
            .import_tasks()
            .detail(&other, task.task_id)
            .unwrap_err()
            .kind(),
        AdminErrorKind::NotFound
    );
    assert_eq!(
        services
            .import_tasks()
            .stop(&other, task.task_id)
            .unwrap_err()
            .kind(),
        AdminErrorKind::NotFound
    );
    let mut another = command(id, 1);
    another.context = other.clone();
    another.items[0].command.context = other.clone();
    let second = services.import_tasks().submit(another).unwrap();
    assert_ne!(task.task_id, second.task_id);
    assert_eq!(services.import_tasks().list(&other).len(), 1);
    let mut forged = command(Uuid::now_v7(), 1);
    forged.items[0].command.context = other;
    assert_eq!(
        services.import_tasks().submit(forged).unwrap_err().kind(),
        AdminErrorKind::Invalid
    );
}

#[tokio::test]
async fn stop_skips_pending_items_and_allows_three_in_flight_to_commit() {
    let (mut bundle, provider, log) = harness().await;
    let gate = provider.block_imports();
    let services = bundle.services();
    let task = services
        .import_tasks()
        .submit(command(Uuid::now_v7(), 8))
        .unwrap();
    let (cancel, worker) = start(&mut bundle);
    wait_started(&log, 3).await;
    let stopped = services
        .import_tasks()
        .stop(&context("stop"), task.task_id)
        .unwrap();
    assert_eq!(stopped.summary.counts.running, 3);
    assert_eq!(stopped.summary.counts.skipped, 5);
    assert!(stopped.summary.finished_at.is_none());
    let stopped_again = services
        .import_tasks()
        .stop(&context("stop-again"), task.task_id)
        .unwrap();
    assert_eq!(stopped_again.summary.counts.skipped, 5);
    gate.add_permits(3);
    wait_finished(&services, task.task_id).await;
    let finished = services
        .import_tasks()
        .detail(&context("new-page"), task.task_id)
        .unwrap();
    assert_eq!(finished.summary.counts.imported_accounts, 3);
    assert_eq!(
        recorded(&log)
            .iter()
            .filter(|event| **event == "provider.prepare_import")
            .count(),
        3
    );
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn concurrency_limit_is_shared_and_batches_are_rotated() {
    let (mut bundle, provider, log) = harness().await;
    let gate = provider.block_imports();
    let services = bundle.services();
    let first = services
        .import_tasks()
        .submit(command(Uuid::now_v7(), 6))
        .unwrap();
    let second = services
        .import_tasks()
        .submit(command(Uuid::now_v7(), 6))
        .unwrap();
    let (cancel, worker) = start(&mut bundle);
    wait_started(&log, 3).await;
    let summaries = services.import_tasks().list(&context("poll"));
    assert_eq!(
        summaries
            .iter()
            .map(|task| task.counts.running)
            .sum::<usize>(),
        3
    );
    assert!(summaries.iter().all(|task| task.counts.running > 0));
    gate.add_permits(12);
    wait_finished(&services, first.task_id).await;
    wait_finished(&services, second.task_id).await;
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn failed_and_unknown_items_do_not_abort_batch_or_retry_credentials() {
    for (kind, failed, unknown) in [
        (ProviderAdminErrorKind::Invalid, 1, 0),
        (ProviderAdminErrorKind::Ambiguous, 0, 1),
        (ProviderAdminErrorKind::Unavailable, 0, 1),
        (ProviderAdminErrorKind::Internal, 0, 1),
    ] {
        let (mut bundle, provider, log) = harness().await;
        provider.fail_next(kind);
        let services = bundle.services();
        let submission_id = Uuid::now_v7();
        let task = services
            .import_tasks()
            .submit(command(submission_id, 4))
            .unwrap();
        let (cancel, worker) = start(&mut bundle);
        wait_finished(&services, task.task_id).await;
        let result = services
            .import_tasks()
            .detail(&context("poll"), task.task_id)
            .unwrap();
        assert_eq!(
            (
                result.summary.counts.succeeded,
                result.summary.counts.failed,
                result.summary.counts.unknown
            ),
            (3, failed, unknown)
        );
        let retry = services
            .import_tasks()
            .submit(command(submission_id, 4))
            .unwrap();
        assert_eq!(retry.task_id, task.task_id);
        assert!(retry.finished_at.is_some());
        assert_eq!(
            recorded(&log)
                .iter()
                .filter(|event| **event == "provider.prepare_import")
                .count(),
            4
        );
        cancel.cancel();
        worker.await.unwrap();
    }
}

#[tokio::test]
async fn imports_keep_existing_service_commit_settings_and_account_ids() {
    for kind in ["openai", "xai"] {
        let log = events();
        let provider = FakeProviderAdmin::new(kind, log.clone());
        provider.set_import_account_ids(&["acct_first", "acct_second"]);
        let store = FakeAccountStore::new(kind, log.clone());
        let mut bundle = AdminHarness::new()
            .provider(provider)
            .accounts(store.clone())
            .build_bundle()
            .await;
        let services = bundle.services();
        let mut input = command(Uuid::now_v7(), 1);
        input.items[0].provider = ProviderKind::new(kind).unwrap();
        input.items[0].command.settings = Some(import_settings());
        let task = services.import_tasks().submit(input).unwrap();
        let (cancel, worker) = start(&mut bundle);
        wait_finished(&services, task.task_id).await;
        let detail = services
            .import_tasks()
            .detail(&context("poll"), task.task_id)
            .unwrap();
        assert_eq!(detail.summary.counts.succeeded, 1);
        assert_eq!(detail.summary.counts.imported_accounts, 2);
        assert_eq!(detail.items[0].index, 1);
        assert_eq!(
            detail.items[0]
                .account_ids
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>(),
            ["acct_first", "acct_second"]
        );
        assert_eq!(store.import_settings(), vec![Some(import_settings())]);
        let log = recorded(&log);
        assert_eq!(
            log.iter()
                .filter(|event| **event == "store.commit_import")
                .count(),
            1
        );
        assert!(!log.contains(&"store.commit_new_import"));
        cancel.cancel();
        worker.await.unwrap();
    }
}

#[tokio::test]
async fn store_failure_is_unknown_and_not_replayed() {
    let log = events();
    let provider = FakeProviderAdmin::new("openai", log.clone());
    let store = FakeAccountStore::new("openai", log.clone());
    store.fail_next_commit();
    let mut bundle = AdminHarness::new()
        .provider(provider)
        .accounts(store)
        .build_bundle()
        .await;
    let services = bundle.services();
    let id = Uuid::now_v7();
    let task = services.import_tasks().submit(command(id, 1)).unwrap();
    let (cancel, worker) = start(&mut bundle);
    wait_finished(&services, task.task_id).await;
    let retry = services.import_tasks().submit(command(id, 1)).unwrap();
    assert_eq!(retry.task_id, task.task_id);
    assert_eq!(retry.counts.unknown, 1);
    assert_eq!(
        recorded(&log)
            .iter()
            .filter(|event| **event == "provider.prepare_import")
            .count(),
        1
    );
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn task_messages_only_expose_sanitized_admin_errors() {
    for provider_kind in ["openai", "xai"] {
        let log = events();
        let provider = FakeProviderAdmin::new(provider_kind, log.clone());
        let mut bundle = AdminHarness::new()
            .provider(provider.clone())
            .accounts(FakeAccountStore::new(provider_kind, log))
            .build_bundle()
            .await;
        let services = bundle.services();
        let (cancel, worker) = start(&mut bundle);
        for kind in [
            ProviderAdminErrorKind::Invalid,
            ProviderAdminErrorKind::Conflict,
            ProviderAdminErrorKind::Ambiguous,
            ProviderAdminErrorKind::Unavailable,
            ProviderAdminErrorKind::BadGateway,
            ProviderAdminErrorKind::Internal,
        ] {
            provider.fail_next_with_message(kind, "synthetic-sensitive-token");
            let mut input = command(Uuid::now_v7(), 1);
            input.items[0].provider = ProviderKind::new(provider_kind).unwrap();
            let task = services.import_tasks().submit(input).unwrap();
            wait_finished(&services, task.task_id).await;
            let detail = services
                .import_tasks()
                .detail(&context("poll"), task.task_id)
                .unwrap();
            assert!(detail.items[0].message.is_some());
            assert!(!format!("{detail:?}").contains("synthetic-sensitive-token"));
        }
        provider.fail_next_with_public_message(
            ProviderAdminErrorKind::Invalid,
            "Synthetic safe import validation message",
        );
        let mut input = command(Uuid::now_v7(), 1);
        input.items[0].provider = ProviderKind::new(provider_kind).unwrap();
        let task = services.import_tasks().submit(input).unwrap();
        wait_finished(&services, task.task_id).await;
        let detail = services
            .import_tasks()
            .detail(&context("poll"), task.task_id)
            .unwrap();
        assert_eq!(
            detail.items[0].message.as_deref(),
            Some("Synthetic safe import validation message")
        );
        assert!(!format!("{detail:?}").contains("upstream body"));
        cancel.cancel();
        worker.await.unwrap();
    }
}

#[tokio::test]
async fn host_forced_shutdown_marks_in_flight_unknown_without_replaying() {
    let (mut bundle, provider, log) = harness().await;
    let _gate = provider.block_imports();
    let services = bundle.services();
    let id = Uuid::now_v7();
    let task = services.import_tasks().submit(command(id, 5)).unwrap();
    let (cancel, worker) = start(&mut bundle);
    wait_started(&log, 3).await;
    cancel.cancel();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    let retry = services.import_tasks().submit(command(id, 5)).unwrap();
    assert_eq!(retry.task_id, task.task_id);
    assert_eq!((retry.counts.unknown, retry.counts.skipped), (3, 2));
    assert!(retry.finished_at.is_some());
    assert_eq!(
        recorded(&log)
            .iter()
            .filter(|event| **event == "provider.prepare_import")
            .count(),
        3
    );
}

#[tokio::test]
async fn shutdown_drains_running_items_and_rejects_new_work() {
    let (mut bundle, provider, log) = harness().await;
    let gate = provider.block_imports();
    let services = bundle.services();
    let id = Uuid::now_v7();
    let task = services.import_tasks().submit(command(id, 7)).unwrap();
    let (cancel, worker) = start(&mut bundle);
    wait_started(&log, 3).await;
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !services
            .import_tasks()
            .detail(&context("poll"), task.task_id)
            .unwrap()
            .summary
            .stop_requested
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("shutdown observed");
    assert_eq!(
        services
            .import_tasks()
            .submit(command(Uuid::now_v7(), 1))
            .unwrap_err()
            .kind(),
        AdminErrorKind::Unavailable
    );
    assert_eq!(
        services
            .import_tasks()
            .submit(command(id, 7))
            .unwrap()
            .task_id,
        task.task_id
    );
    assert!(!worker.is_finished());
    gate.add_permits(3);
    worker.await.unwrap();
    let result = services
        .import_tasks()
        .detail(&context("poll"), task.task_id)
        .unwrap();
    assert_eq!(
        (
            result.summary.counts.succeeded,
            result.summary.counts.skipped
        ),
        (3, 4)
    );
}

#[tokio::test(start_paused = true)]
async fn completed_records_expire_at_one_hour_and_new_process_starts_empty() {
    let (bundle, _, _) = harness().await;
    let services = bundle.services();
    let task = services
        .import_tasks()
        .submit(command(Uuid::now_v7(), 2))
        .unwrap();
    services
        .import_tasks()
        .stop(&context("stop"), task.task_id)
        .unwrap();
    tokio::time::advance(Duration::from_secs(3599)).await;
    assert_eq!(services.import_tasks().list(&context("poll")).len(), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(services.import_tasks().list(&context("poll")).is_empty());
    assert_eq!(
        services
            .import_tasks()
            .detail(&context("poll"), task.task_id)
            .unwrap_err()
            .kind(),
        AdminErrorKind::NotFound
    );
    let (fresh, _, _) = harness().await;
    assert!(
        fresh
            .services()
            .import_tasks()
            .list(&context("poll"))
            .is_empty()
    );
}

#[tokio::test]
async fn queue_rejects_excess_work_before_starting_imports() {
    let (bundle, _, log) = harness().await;
    let services = bundle.services();
    for count in [0, 201] {
        assert_eq!(
            services
                .import_tasks()
                .submit(command(Uuid::now_v7(), count))
                .unwrap_err()
                .kind(),
            AdminErrorKind::Invalid
        );
    }
    assert_eq!(
        services
            .import_tasks()
            .submit(command(Uuid::nil(), 1))
            .unwrap_err()
            .kind(),
        AdminErrorKind::Invalid
    );
    for _ in 0..8 {
        services
            .import_tasks()
            .submit(command(Uuid::now_v7(), 200))
            .unwrap();
    }
    assert_eq!(
        services
            .import_tasks()
            .submit(command(Uuid::now_v7(), 1))
            .unwrap_err()
            .kind(),
        AdminErrorKind::RateLimited
    );
    assert!(recorded(&log).is_empty());
}

#[tokio::test(start_paused = true)]
async fn retained_results_are_bounded_without_evicting_idempotency_records_early() {
    let (bundle, _, _) = harness().await;
    let services = bundle.services();
    let id = Uuid::now_v7();
    let first = services.import_tasks().submit(command(id, 1)).unwrap();
    services
        .import_tasks()
        .stop(&context("stop"), first.task_id)
        .unwrap();
    for _ in 1..100 {
        let task = services
            .import_tasks()
            .submit(command(Uuid::now_v7(), 1))
            .unwrap();
        services
            .import_tasks()
            .stop(&context("stop"), task.task_id)
            .unwrap();
    }
    assert_eq!(services.import_tasks().list(&context("poll")).len(), 100);
    assert_eq!(
        services
            .import_tasks()
            .submit(command(id, 1))
            .unwrap()
            .task_id,
        first.task_id
    );
    assert_eq!(
        services
            .import_tasks()
            .submit(command(Uuid::now_v7(), 1))
            .unwrap_err()
            .kind(),
        AdminErrorKind::RateLimited
    );
    tokio::time::advance(Duration::from_secs(3600)).await;
    services
        .import_tasks()
        .submit(command(Uuid::now_v7(), 1))
        .unwrap();
}
