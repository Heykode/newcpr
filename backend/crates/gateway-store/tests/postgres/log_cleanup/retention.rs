use super::*;

#[tokio::test]
async fn request_retention_round_trips_and_synchronizes_positive_windows() {
    let Some(db) = TestDatabase::create("cleanup_short_retention").await else {
        return;
    };
    let store = PgLogCleanupStore::new(db.pool.clone(), None, None);
    for days in [0, 1, 3, 4, 30, 31, 45, 3651, u32::MAX, 3, 0] {
        let before: (i64, i64) = sqlx::query_as(
            "select usage_retention_days, ops_event_retention_days from runtime_settings where id=1",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let state = store.state().await.unwrap();
        let mut config = state.config;
        config.requests.retention_days = days;
        config.files.retention_days = days.min(3650);
        config.captures.retention_days = days.min(30);
        config.audit.retention_days = days.min(3650);
        store
            .configure(
                CleanupCommand {
                    revision: state.revision,
                    config: config.clone(),
                },
                &context(),
            )
            .await
            .unwrap();
        // Reopen the store to check persisted decoding, not just the submitted draft.
        let reopened = PgLogCleanupStore::new(db.pool.clone(), None, None);
        let saved = reopened.state().await.unwrap();
        assert_eq!(saved.config, config, "retention {days}");
        assert_eq!(saved.revision, state.revision + 1);
        let after: (i64, i64) = sqlx::query_as(
            "select usage_retention_days, ops_event_retention_days from runtime_settings where id=1",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let expected = if days > 0 {
            (i64::from(days), i64::from(days))
        } else {
            before
        };
        assert_eq!(after, expected, "legacy windows for retention {days}");
    }
    db.close().await;
}

#[tokio::test]
async fn short_retention_manual_and_scheduled_jobs_preserve_recent_and_running_requests() {
    let Some(db) = TestDatabase::create("cleanup_short_execution").await else {
        return;
    };
    let store = PgLogCleanupStore::new(db.pool.clone(), None, None);
    for (days, automatic) in [(3, false), (4, true)] {
        let state = store.state().await.unwrap();
        let mut config = state.config;
        config.enabled = automatic;
        config.requests.retention_days = days;
        config.requests.selected = true;
        config.files.selected = false;
        config.captures.selected = false;
        config.audit.selected = false;
        store
            .configure(
                CleanupCommand {
                    revision: state.revision,
                    config,
                },
                &context(),
            )
            .await
            .unwrap();
        let cutoff = Utc::now();
        for (suffix, completed_at) in [
            ("old", Some(cutoff - Duration::days(i64::from(days) + 1))),
            ("recent", Some(cutoff - Duration::hours(1))),
            ("running", None),
        ] {
            sqlx::query(
                "insert into model_requests (
                id,client_api_key_ref,config_revision,protocol,operation,endpoint,client_transport,
                outcome,client_status_code,started_at,deadline_at,completed_at,
                downstream_committed_at,routing_scope
            ) values ($1,'fixture-key',1,'openai','responses','/v1/responses','http',
                $2,$3,$4,$5,$6,$6,'all')",
            )
            .bind(format!("cleanup-short-{days}-{suffix}"))
            .bind(if completed_at.is_some() {
                "succeeded"
            } else {
                "running"
            })
            .bind(completed_at.map(|_| 200_i32))
            .bind(cutoff - Duration::days(10))
            .bind(cutoff + Duration::hours(1))
            .bind(completed_at)
            .execute(&db.pool)
            .await
            .unwrap();
        }
        if automatic {
            sqlx::query("update log_cleanup_control set next_run_at=now()-interval '1 second'")
                .execute(&db.pool)
                .await
                .unwrap();
        } else {
            let saved = store.state().await.unwrap();
            store
                .start(
                    CleanupPreview {
                        revision: saved.revision,
                        config: saved.config,
                        cutoff_at: cutoff,
                    },
                    &context(),
                )
                .await
                .unwrap();
        }
        let job = finish(&store).await;
        assert_eq!(job.status, CleanupJobStatus::Succeeded);
        assert_eq!(job.automatic, automatic);
        assert_eq!(job.config.requests.retention_days, days);
        assert_eq!(job.removed, 1);
        let remaining: Vec<String> =
            sqlx::query_scalar("select id from model_requests where id like $1 order by id")
                .bind(format!("cleanup-short-{days}-%"))
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(
            remaining,
            [
                format!("cleanup-short-{days}-recent"),
                format!("cleanup-short-{days}-running")
            ]
        );
    }
    db.close().await;
}

#[tokio::test]
async fn large_retention_keeps_existing_history_without_date_overflow() {
    let Some(db) = TestDatabase::create("cleanup_large_retention").await else {
        return;
    };
    let store = PgLogCleanupStore::new(db.pool.clone(), None, None);
    sqlx::query("insert into ops_events(id,level,component,operation,failure_kind,message,created_at) values ('retained-event','error','fixture','fixture','fixture','retained',now()-interval '10 days')")
        .execute(&db.pool).await.unwrap();
    // One value fits Chrono but predates PostgreSQL; one exceeds Chrono too.
    for days in [3651, 10_000_000, u32::MAX] {
        let mut preview = request_only(&store).await;
        preview.config.requests.retention_days = days;
        store
            .configure(
                CleanupCommand {
                    revision: preview.revision,
                    config: preview.config,
                },
                &context(),
            )
            .await
            .unwrap();
        let saved = store.state().await.unwrap();
        store
            .start(
                CleanupPreview {
                    revision: saved.revision,
                    config: saved.config,
                    cutoff_at: Utc::now(),
                },
                &context(),
            )
            .await
            .unwrap();
        assert_eq!(finish(&store).await.status, CleanupJobStatus::Succeeded);
        let count: i64 =
            sqlx::query_scalar("select count(*) from ops_events where id='retained-event'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(count, 1, "days={days}");
    }
    db.close().await;
}
