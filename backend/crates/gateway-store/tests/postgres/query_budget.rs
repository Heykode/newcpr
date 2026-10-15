use std::time::Duration;

use futures::TryStreamExt;
use gateway_store::{StoreError, postgres::ObservabilityQueryBudget};
use sqlx::Connection as _;
use tokio::sync::{mpsc, oneshot};

use super::TestDatabase;

#[test]
fn query_budget_rejects_an_empty_budget() {
    assert!(ObservabilityQueryBudget::try_new(0, Duration::from_secs(1)).is_err());
    assert!(ObservabilityQueryBudget::try_new(1, Duration::ZERO).is_err());
    for timeout in [Duration::ZERO, Duration::from_millis(i32::MAX as u64 + 1)] {
        assert!(
            ObservabilityQueryBudget::try_new_with_timeout(1, Duration::from_secs(1), timeout)
                .is_err()
        );
    }
}

#[tokio::test]
async fn observation_timeout_is_scoped_and_restored_on_reused_connections() {
    let Some(database) = TestDatabase::create("budget_restore").await else {
        return;
    };
    // Occupy one of the two connections so all checks reuse the same backend.
    let held = database.pool.acquire().await.unwrap();
    sqlx::query("set statement_timeout = '50ms'")
        .execute(&database.pool)
        .await
        .unwrap();
    let budget = ObservabilityQueryBudget::try_new_with_timeout(
        1,
        Duration::from_secs(1),
        Duration::from_secs(2),
    )
    .unwrap();
    for fail in [false, true] {
        let result = budget
            .run(
                &database.pool,
                "scoped timeout",
                |mut connection| async move {
                    let timeout: String = sqlx::query_scalar("show statement_timeout")
                        .fetch_one(&mut *connection)
                        .await
                        .unwrap();
                    assert_eq!(timeout, "2s");
                    sqlx::query("select pg_sleep(0.1)")
                        .execute(&mut *connection)
                        .await
                        .unwrap();
                    if fail {
                        let mut transaction = connection.begin().await.unwrap();
                        assert!(
                            sqlx::query("select 1 / 0")
                                .execute(&mut *transaction)
                                .await
                                .is_err()
                        );
                        return Err(StoreError::InvalidData {
                            entity: "test",
                            message: "failed query".to_owned(),
                        });
                    }
                    Ok(())
                },
            )
            .await;
        assert_eq!(result.is_err(), fail);
        let timeout: String = sqlx::query_scalar("show statement_timeout")
            .fetch_one(&database.pool)
            .await
            .unwrap();
        assert_eq!(
            timeout, "50ms",
            "pool reuse must restore the original timeout"
        );
    }
    drop(held);
    database.close().await;
}

#[tokio::test]
async fn query_stream_holds_connection_until_completion_error_or_drop() {
    let Some(database) = TestDatabase::create("budget_stream").await else {
        return;
    };
    let budget = ObservabilityQueryBudget::try_new(1, Duration::from_millis(100)).unwrap();
    for terminal in [0, 1, 2] {
        let mut facts = budget.run_stream(&database.pool, "stream test", move |mut connection| {
            async_stream::stream! {
                let first = sqlx::query_scalar::<_, i32>("select 1")
                    .fetch_one(&mut *connection).await.unwrap();
                yield Ok(first);
                match terminal {
                    0 => std::future::pending::<()>().await,
                    1 => yield Ok(2),
                    _ => yield Err(StoreError::InvalidData {
                        entity: "test stream", message: "decode failed".to_owned(),
                    }),
                }
            }
        });
        assert_eq!(facts.try_next().await.unwrap(), Some(1));
        assert!(
            budget
                .run(&database.pool, "blocked query", |_| async { Ok(()) })
                .await
                .is_err()
        );
        if terminal == 0 {
            drop(facts);
        } else {
            if terminal == 1 {
                assert_eq!(facts.try_next().await.unwrap(), Some(2));
                assert!(facts.try_next().await.unwrap().is_none());
            } else {
                assert!(facts.try_next().await.is_err());
            }
            budget
                .run(&database.pool, "after terminal", |_| async { Ok(()) })
                .await
                .expect("finished stream releases slot even if retained");
        }
        budget
            .run(&database.pool, "after stream", |_| async { Ok(()) })
            .await
            .unwrap();
    }
    database.close().await;
}

#[tokio::test]
async fn query_budget_queues_and_times_out_at_the_connection_limit() {
    let Some(database) = TestDatabase::create("budget_queue").await else {
        return;
    };
    let budget = ObservabilityQueryBudget::try_new(2, Duration::from_millis(100)).unwrap();
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let mut releases = Vec::new();
    let mut occupied = Vec::new();
    for _ in 0..2 {
        let occupied_budget = budget.clone();
        let pool = database.pool.clone();
        let started_tx = started_tx.clone();
        let (release_tx, release_rx) = oneshot::channel();
        releases.push(release_tx);
        occupied.push(tokio::spawn(async move {
            occupied_budget
                .run(&pool, "occupied query", |mut connection| async move {
                    sqlx::query("select 1")
                        .execute(&mut *connection)
                        .await
                        .unwrap();
                    started_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                    Ok(())
                })
                .await
        }));
    }
    for _ in 0..2 {
        started_rx.recv().await.unwrap();
    }
    let error = budget
        .run(&database.pool, "blocked query", |_| async { Ok(()) })
        .await
        .expect_err("budget exhausted");
    assert!(matches!(error, StoreError::Unavailable { message, .. }
        if message == "observability PostgreSQL connection budget is exhausted"));
    let queued_budget = budget.clone();
    let pool = database.pool.clone();
    let queued = tokio::spawn(async move {
        queued_budget
            .run(&pool, "queued query", |_| async { Ok(()) })
            .await
    });
    releases.pop().unwrap().send(()).unwrap();
    queued
        .await
        .unwrap()
        .expect("released slot admits queued query");
    releases.pop().unwrap().send(()).unwrap();
    for task in occupied {
        task.await.unwrap().unwrap();
    }
    database.close().await;
}

#[tokio::test]
async fn cancelled_query_keeps_slot_through_protocol_cleanup_and_rolls_back() {
    let Some(database) = TestDatabase::create("budget_cancel").await else {
        return;
    };
    sqlx::query("create table budget_cancel_test (id integer)")
        .execute(&database.pool)
        .await
        .unwrap();
    let budget = ObservabilityQueryBudget::try_new(1, Duration::from_millis(30)).unwrap();
    let occupied_budget = budget.clone();
    let pool = database.pool.clone();
    let (started_tx, started_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        occupied_budget
            .run(&pool, "cancelled query", |mut connection| async move {
                let mut transaction = connection.begin().await.unwrap();
                sqlx::query("insert into budget_cancel_test values (1)")
                    .execute(&mut *transaction)
                    .await
                    .unwrap();
                let pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
                    .fetch_one(&mut *transaction)
                    .await
                    .unwrap();
                started_tx.send(pid).unwrap();
                sqlx::query("select pg_sleep(0.4)")
                    .execute(&mut *transaction)
                    .await
                    .unwrap();
                transaction.commit().await.unwrap();
                Ok(())
            })
            .await
    });
    let pid = started_rx.await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let active: bool = sqlx::query_scalar(
                "select state = 'active' and query = 'select pg_sleep(0.4)' from pg_stat_activity where pid = $1",
            ).bind(pid).fetch_one(&database.pool).await.unwrap();
            if active { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        budget
            .run(&database.pool, "during cleanup", |_| async { Ok(()) })
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if budget
                .run(
                    &database.pool,
                    "after cleanup",
                    |mut connection| async move {
                        let count: i64 =
                            sqlx::query_scalar("select count(*) from budget_cancel_test")
                                .fetch_one(&mut *connection)
                                .await
                                .unwrap();
                        assert_eq!(count, 0, "cancelled transaction must be rolled back");
                        Ok(())
                    },
                )
                .await
                .is_ok()
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    database.close().await;
}

#[tokio::test]
async fn observation_execution_deadline_releases_the_connection() {
    let Some(database) = TestDatabase::create("budget_deadline").await else {
        return;
    };
    let budget = ObservabilityQueryBudget::try_new_with_timeout(
        1,
        Duration::from_secs(1),
        Duration::from_millis(100),
    )
    .unwrap();
    let error = budget
        .run(
            &database.pool,
            "never finishes",
            |mut connection| async move {
                sqlx::query("select 1")
                    .execute(&mut *connection)
                    .await
                    .unwrap();
                std::future::pending::<Result<(), StoreError>>().await
            },
        )
        .await
        .expect_err("execution deadline");
    assert!(matches!(error, StoreError::Unavailable { message, .. }
        if message.contains("execution budget is exhausted")));
    budget
        .run(&database.pool, "after deadline", |_| async { Ok(()) })
        .await
        .unwrap();
    database.close().await;
}
