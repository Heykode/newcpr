use super::*;
use gateway_admin::ports::store::{AdminStoreError, AdminStoreErrorKind};
use std::{cell::Cell, future::ready};
use tokio::time::Instant;

#[tokio::test(start_paused = true)]
async fn progressing_batches_do_not_wait_between_batches() {
    let wakeup = Notify::new();
    let cancellation = CancellationToken::new();
    let started = Instant::now();
    let mut count = 0;
    run_loop(&wakeup, cancellation.clone(), || {
        count += 1;
        if count == 10_000 {
            cancellation.cancel();
        }
        ready(Ok(BatchOutcome::Continue))
    })
    .await
    .unwrap();
    assert_eq!(count, 10_000);
    assert_eq!(Instant::now(), started);
}

#[tokio::test(start_paused = true)]
async fn idle_worker_wakes_without_waiting_for_poll() {
    let wakeup = Notify::new();
    let cancellation = CancellationToken::new();
    let started = Instant::now();
    let mut count = 0;
    let run = run_loop(&wakeup, cancellation.clone(), || {
        count += 1;
        if count == 2 {
            cancellation.cancel();
        }
        ready(Ok(BatchOutcome::Idle))
    });
    let signal = async {
        tokio::task::yield_now().await;
        wakeup.notify_one();
    };
    let (result, ()) = tokio::join!(run, signal);
    result.unwrap();
    assert_eq!(count, 2);
    assert_eq!(Instant::now(), started);
}

#[tokio::test(start_paused = true)]
async fn idle_poll_discovers_work_without_local_notification() {
    let wakeup = Notify::new();
    let cancellation = CancellationToken::new();
    let started = Instant::now();
    let mut count = 0;
    run_loop(&wakeup, cancellation.clone(), || {
        count += 1;
        if count == 2 {
            cancellation.cancel();
        }
        ready(Ok(BatchOutcome::Idle))
    })
    .await
    .unwrap();
    assert_eq!(count, 2);
    assert_eq!(Instant::now() - started, IDLE_POLL_INTERVAL);
}

#[tokio::test(start_paused = true)]
async fn contention_backs_off_even_with_a_pending_notification() {
    let wakeup = Notify::new();
    wakeup.notify_one();
    let cancellation = CancellationToken::new();
    let started = Instant::now();
    let mut count = 0;
    run_loop(&wakeup, cancellation.clone(), || {
        count += 1;
        if count == 2 {
            cancellation.cancel();
        }
        ready(Ok(BatchOutcome::Retry))
    })
    .await
    .unwrap();
    assert_eq!(count, 2);
    assert_eq!(Instant::now() - started, RETRY_INTERVAL);
}

#[tokio::test(start_paused = true)]
async fn shutdown_finishes_current_batch_before_stopping() {
    let wakeup = Notify::new();
    let entered = Notify::new();
    let release = Notify::new();
    let cancellation = CancellationToken::new();
    let committed = Cell::new(false);
    let run = run_loop(&wakeup, cancellation.clone(), || async {
        entered.notify_one();
        release.notified().await;
        committed.set(true);
        Ok(BatchOutcome::Continue)
    });
    let shutdown = async {
        entered.notified().await;
        cancellation.cancel();
        tokio::task::yield_now().await;
        assert!(!committed.get());
        release.notify_one();
    };
    let (result, ()) = tokio::join!(run, shutdown);
    result.unwrap();
    assert!(committed.get());
}

#[tokio::test(start_paused = true)]
async fn cancelled_worker_does_not_start_another_batch() {
    let wakeup = Notify::new();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let mut called = false;
    run_loop(&wakeup, cancellation, || {
        called = true;
        ready(Ok(BatchOutcome::Continue))
    })
    .await
    .unwrap();
    assert!(!called);
}

#[tokio::test(start_paused = true)]
async fn progressing_job_has_no_total_time_limit() {
    let wakeup = Notify::new();
    let cancellation = CancellationToken::new();
    let started = Instant::now();
    let mut count = 0;
    run_loop(&wakeup, cancellation.clone(), || {
        count += 1;
        let cancellation = cancellation.clone();
        async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if count == 70 {
                cancellation.cancel();
            }
            Ok(BatchOutcome::Continue)
        }
    })
    .await
    .unwrap();
    assert_eq!(count, 70);
    assert_eq!(Instant::now() - started, Duration::from_secs(70));
}

#[tokio::test(start_paused = true)]
async fn storage_failure_returns_to_daemon_supervision() {
    let wakeup = Notify::new();
    let mut count = 0;
    let error = run_loop(&wakeup, CancellationToken::new(), || {
        count += 1;
        ready(Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "log cleanup",
            "fixture storage failure",
        )))
    })
    .await
    .unwrap_err();
    assert_eq!(count, 1);
    assert_eq!(error.kind(), AdminStoreErrorKind::Unavailable);
}
