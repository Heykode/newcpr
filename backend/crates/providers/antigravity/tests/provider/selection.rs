//! 验证并发已满的账号参与有界排队，取消或超时后归还队位

use crate::support::{Fixture, context_with_policy, request};
use gateway_core::{
    account::{AccountSelectionPolicy, RotationStrategy},
    concurrency::ConcurrencyQueuePolicy,
    engine::{
        ModelRequestId, RequestAttemptContext,
        provider::{Provider, ProviderStream},
    },
    error::{ProviderError, ProviderErrorKind},
    lifecycle::CancellationToken,
    policy::ClientApiKeyId,
    upstream::UpstreamSendState,
};
use std::{num::NonZeroU32, sync::Arc, time::Duration};

async fn acquire(
    fixture: &Fixture,
    max_waiting: u32,
    timeout: Duration,
    budget: Option<Arc<gateway_core::engine::AccountWaitBudget>>,
    cancellation: CancellationToken,
) -> Result<ProviderStream, ProviderError> {
    let policy = AccountSelectionPolicy::new(
        RotationStrategy::Smart,
        NonZeroU32::MIN,
        std::time::Duration::ZERO,
    )
    .with_queue(ConcurrencyQueuePolicy {
        max_waiting,
        timeout,
    });
    let attempt = RequestAttemptContext::new(
        ModelRequestId::new("req_fixture").unwrap(),
        ClientApiKeyId::new("key_fixture").unwrap(),
    );
    let context = context_with_policy(attempt, policy, cancellation);
    let context = match budget {
        Some(budget) => context.with_account_wait_budget(budget),
        None => context,
    };
    fixture
        .provider
        .clone()
        .execute(request(false), context)
        .await
}

#[tokio::test]
async fn saturated_account_reports_capacity_and_releases_cancelled_queue_position() {
    let fixture = Fixture::new("http://127.0.0.1:1");
    let attempt = |max_waiting, cancellation| {
        acquire(
            &fixture,
            max_waiting,
            Duration::from_secs(2),
            Default::default(),
            cancellation,
        )
    };
    let holder = attempt(0, CancellationToken::new()).await.unwrap();
    let error = attempt(0, CancellationToken::new()).await.err().unwrap();
    assert_eq!(error.kind(), ProviderErrorKind::AccountCapacityUnavailable);
    assert_eq!(error.send_state(), UpstreamSendState::NotSent);

    let cancelled = CancellationToken::new();
    let waiter = attempt(1, cancelled.clone());
    tokio::pin!(waiter);
    assert!(futures::poll!(&mut waiter).is_pending());
    let full = attempt(1, CancellationToken::new()).await.err().unwrap();
    assert_eq!(full.kind(), ProviderErrorKind::ConcurrencyQueueFull);
    cancelled.cancel();
    assert_eq!(
        waiter.await.err().unwrap().kind(),
        ProviderErrorKind::Cancelled
    );

    let next = attempt(1, CancellationToken::new());
    tokio::pin!(next);
    assert!(futures::poll!(&mut next).is_pending());
    drop(holder);
    let newcomer = attempt(0, CancellationToken::new()).await.err().unwrap();
    assert_eq!(
        newcomer.kind(),
        ProviderErrorKind::AccountCapacityUnavailable
    );
    let stream = next.await.unwrap();
    let observation = stream.metadata().selection_observation().unwrap();
    assert!(observation.account_selection_wait_ms() > 0);
    let capacity = observation.capacity().unwrap();
    assert_eq!((capacity.used_slots(), capacity.total_slots()), (1, 1));
}

#[tokio::test]
async fn repeated_selection_uses_the_original_account_wait_deadline() {
    let fixture = Fixture::new("http://127.0.0.1:1");
    let holder = acquire(
        &fixture,
        0,
        Duration::ZERO,
        Default::default(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let budget = Some(Arc::new(gateway_core::engine::AccountWaitBudget::new(
        std::time::SystemTime::now() + Duration::from_secs(30),
        Default::default(),
    )));
    let queued = |duration| {
        acquire(
            &fixture,
            1,
            duration,
            budget.clone(),
            CancellationToken::new(),
        )
    };
    let first = queued(Duration::from_millis(15)).await.err().unwrap();
    assert_eq!(first.kind(), ProviderErrorKind::ConcurrencyQueueTimeout);
    let second = tokio::time::timeout(Duration::from_secs(1), queued(Duration::from_secs(30)))
        .await
        .expect("an expired wait budget cannot restart")
        .err()
        .unwrap();
    assert_eq!(second.kind(), ProviderErrorKind::ConcurrencyQueueTimeout);
    drop(holder);
}

#[tokio::test]
async fn wakeup_rechecks_model_access_before_issuing_a_lease() {
    let fixture = Fixture::new("http://127.0.0.1:1");
    let attempt = |max_waiting| {
        acquire(
            &fixture,
            max_waiting,
            Duration::from_secs(2),
            Default::default(),
            CancellationToken::new(),
        )
    };
    let holder = attempt(0).await.unwrap();
    let waiter = attempt(1);
    tokio::pin!(waiter);
    assert!(futures::poll!(&mut waiter).is_pending());
    fixture.store.primary().catalog.lock().unwrap().take();
    drop(holder);
    let error = waiter.await.err().unwrap();
    assert_eq!(error.kind(), ProviderErrorKind::NoEligibleAccount);
    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
}
