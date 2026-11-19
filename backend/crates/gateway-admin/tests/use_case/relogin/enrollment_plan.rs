use super::*;

fn intent() -> ReloginEnrollment {
    let mut settings = template_config().settings().unwrap();
    settings.custom_name = Some("Planned account".into());
    ReloginEnrollment::new(settings, None, context("plan-test")).unwrap()
}

#[tokio::test]
async fn imported_drafts_skip_existing_pool_plans_and_stale_versions_and_never_enqueue() {
    let h = Harness::new(vec![account_record("openai")]).await;
    let pooled = h.import("test@example.invalid").await;
    let fresh = h.import("fresh@example.invalid").await;
    let scheduled = h.import("scheduled@example.invalid").await;
    let other = h.import("other@example.invalid").await;
    arrange(
        &h,
        &scheduled,
        EnrollmentPlanMode::Scheduled,
        Some(Utc::now() + Duration::hours(1)),
    )
    .await;
    let revisions = BTreeMap::from([
        (fresh.clone(), h.row(&fresh).await.revision),
        (pooled.clone(), h.row(&pooled).await.revision),
        (scheduled.clone(), h.row(&scheduled).await.revision),
        (other.clone(), 0),
    ]);
    let results = h
        .services
        .relogin()
        .arrange_imports(
            ImportedEnrollmentPlanCommand {
                revisions,
                mode: EnrollmentPlanMode::Draft,
                when: None,
                enrollment: intent(),
                template: None,
            },
            &context("draft-batch"),
        )
        .await
        .unwrap();
    assert_eq!(results.iter().filter(|r| r.success).count(), 1);
    assert_eq!(
        h.row(&fresh).await.enrollment_plan.unwrap().state,
        EnrollmentPlanState::Draft
    );
    assert!(
        h.row(&pooled).await.enrollment_plan.is_none()
            && h.row(&other).await.enrollment_plan.is_none()
    );
    assert_eq!(
        h.row(&scheduled).await.enrollment_plan.unwrap().state,
        EnrollmentPlanState::Scheduled
    );
    h.cycle().await;
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn immediate_enrollment_consumes_the_schedule_only_after_a_successful_pool_write() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    let later = Some(Utc::now() + Duration::hours(3));
    arrange(&h, &id, EnrollmentPlanMode::Scheduled, later).await;
    arrange(&h, &id, EnrollmentPlanMode::Now, None).await;
    assert_eq!(h.row(&id).await.enrollment_plan.unwrap().when, later);
    h.cycle().await;
    let plan = h.row(&id).await.enrollment_plan.unwrap();
    assert_eq!(plan.state, EnrollmentPlanState::CompletedEarly);
    assert!(plan.when.is_none());
    h.accounts.set_accounts(vec![]);
    h.cycle().await;
    assert_eq!(h.accounts.import_settings().len(), 1);
}

async fn arrange(
    h: &Harness,
    id: &str,
    mode: EnrollmentPlanMode,
    when: Option<chrono::DateTime<Utc>>,
) {
    h.services
        .relogin()
        .arrange(
            EnrollmentPlanCommand {
                id: id.to_owned(),
                revision: h.row(id).await.revision,
                mode,
                when,
                enrollment: Some(intent()),
                template: None,
            },
            &context("plan-test"),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn draft_and_future_plan_do_not_login_but_due_plan_enrolls_with_saved_settings() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    arrange(&h, &id, EnrollmentPlanMode::Draft, None).await;
    h.cycle().await;
    assert!(h.provider.relogin_proxies.lock().unwrap().is_empty());
    arrange(
        &h,
        &id,
        EnrollmentPlanMode::Scheduled,
        Some(Utc::now() + Duration::days(2)),
    )
    .await;
    h.cycle().await;
    assert!(h.provider.relogin_proxies.lock().unwrap().is_empty());
    let mut row: ReloginEntry =
        serde_json::from_value(serde_json::to_value(h.row(&id).await).unwrap()).unwrap();
    row.enrollment_plan.as_mut().unwrap().when = Some(Utc::now() - Duration::seconds(1));
    h.store.rows.lock().unwrap().insert(id.clone(), row);
    h.cycle().await;
    let row = h.row(&id).await;
    assert!(row.synced_at.is_some());
    assert_eq!(
        row.enrollment_plan.unwrap().state,
        EnrollmentPlanState::Done
    );
    assert_eq!(
        h.accounts.import_settings(),
        vec![Some(intent().settings().unwrap())]
    );
    h.cycle().await;
    assert_eq!(h.provider.relogin_proxies.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn reschedule_and_cancel_are_revision_fenced_and_cancel_keeps_draft() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    let stale = h.row(&id).await.revision;
    arrange(
        &h,
        &id,
        EnrollmentPlanMode::Scheduled,
        Some(Utc::now() + Duration::hours(1)),
    )
    .await;
    assert!(
        h.services
            .relogin()
            .arrange(
                EnrollmentPlanCommand {
                    id: id.clone(),
                    revision: stale,
                    mode: EnrollmentPlanMode::Cancel,
                    when: None,
                    enrollment: None,
                    template: None,
                },
                &context("stale-plan")
            )
            .await
            .is_err()
    );
    arrange(
        &h,
        &id,
        EnrollmentPlanMode::Scheduled,
        Some(Utc::now() + Duration::days(3)),
    )
    .await;
    assert!(
        h.row(&id).await.enrollment_plan.unwrap().when.unwrap() > Utc::now() + Duration::days(2)
    );
    arrange(&h, &id, EnrollmentPlanMode::Cancel, None).await;
    h.cycle().await;
    let plan = h.row(&id).await.enrollment_plan.unwrap();
    assert_eq!(plan.state, EnrollmentPlanState::Draft);
    assert!(plan.when.is_none());
    assert_eq!(
        plan.enrollment.custom_name.as_deref(),
        Some("Planned account")
    );
    assert!(h.provider.relogin_proxies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn due_plan_respects_pause_and_account_appearing_before_execution() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    arrange(
        &h,
        &id,
        EnrollmentPlanMode::Scheduled,
        Some(Utc::now() + Duration::hours(1)),
    )
    .await;
    h.store
        .rows
        .lock()
        .unwrap()
        .get_mut(&id)
        .unwrap()
        .enrollment_plan
        .as_mut()
        .unwrap()
        .when = Some(Utc::now() - Duration::seconds(1));
    h.store.settings.lock().unwrap().paused = true;
    h.cycle().await;
    assert_eq!(
        h.row(&id).await.enrollment_plan.unwrap().state,
        EnrollmentPlanState::Scheduled
    );
    h.store.settings.lock().unwrap().paused = false;
    h.accounts.set_accounts(vec![account_record("openai")]);
    h.cycle().await;
    assert_eq!(
        h.row(&id).await.enrollment_plan.unwrap().state,
        EnrollmentPlanState::Failed
    );
    assert!(h.provider.relogin_proxies.lock().unwrap().is_empty());
    assert!(h.accounts.import_settings().is_empty());
}

fn proxies(enabled: bool) -> Arc<super::super::proxies::TestProxies> {
    Arc::new(super::super::proxies::TestProxies {
        events: Some(events()),
        records: Some(vec![gateway_admin::model::proxies::ProxyRecord {
            enabled,
            auto_location: false,
            detected_location: None,
            request_location: None,
            id: "maintenance-proxy".into(),
            name: "Maintenance B".into(),
            proxy: gateway_core::account::OutboundProxy::parse("http://maintenance.example:8080")
                .unwrap(),
            revision: revision(1),
            account_count: 0,
            last_test_at: None,
            last_test: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }]),
        ..Default::default()
    })
}

#[tokio::test]
async fn maintenance_proxy_overrides_business_route_for_login_and_device_reads() {
    use gateway_admin::model::relogin_maintenance::*;
    let mut account = account_record("openai");
    account.upstream_account_id = Some("workspace-team".into());
    account.outbound_proxy =
        Some(gateway_core::account::OutboundProxy::parse("http://business.example:8080").unwrap());
    account.credential_proxy_id = Some("maintenance-proxy".into());
    let original = account.clone();
    let h = Harness::with_proxies(vec![account], Some(proxies(true))).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    assert_eq!(
        h.provider.relogin_proxies.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .endpoint(),
        "http://maintenance.example:8080/"
    );
    *h.provider.maintenance_result.lock().unwrap() = Some(MaintenanceOutcome {
        view: MaintenanceView::default(),
        session: None,
        credential: None,
        pending_totp: None,
        new_secret: None,
    });
    h.services
        .relogin()
        .maintain(
            MaintenanceCommand {
                id: id.clone(),
                revision: h.row(&id).await.revision,
                action: MaintenanceAction::Devices,
                target: None,
                device_id_hash: None,
                confirm: None,
            },
            &context("device-read"),
        )
        .await
        .unwrap();
    assert_eq!(
        h.provider.maintenance_routes.lock().unwrap()[0]
            .0
            .as_ref()
            .unwrap()
            .endpoint(),
        "http://maintenance.example:8080/"
    );
    let list = h.services.relogin().list().await.unwrap();
    assert_eq!(list.items[0].credential_egress.label, "Maintenance B");
    assert_eq!(
        list.items[0].egress.address,
        original.outbound_proxy.map(|proxy| proxy.endpoint())
    );
}

#[tokio::test]
async fn disabled_or_missing_maintenance_proxy_never_falls_back_to_business_or_direct() {
    for missing in [false, true] {
        let mut account = account_record("openai");
        account.upstream_account_id = Some("workspace-team".into());
        account.credential_proxy_id = Some(
            if missing {
                "missing-proxy"
            } else {
                "maintenance-proxy"
            }
            .into(),
        );
        let h = Harness::with_proxies(vec![account], Some(proxies(false))).await;
        let id = h.import("test@example.invalid").await;
        h.services
            .relogin()
            .queue(std::slice::from_ref(&id))
            .await
            .unwrap();
        h.cycle().await;
        assert_eq!(h.row(&id).await.status, ReloginStatus::Failed);
        assert!(h.provider.relogin_proxies.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn new_enrollment_uses_maintenance_proxy_and_persists_separate_business_settings() {
    let h = Harness::with_proxies(vec![], Some(proxies(true))).await;
    let id = h.import("test@example.invalid").await;
    let mut enrollment = intent();
    enrollment.config.credential_proxy_id = Some(Some("maintenance-proxy".into()));
    h.services
        .relogin()
        .arrange(
            EnrollmentPlanCommand {
                id: id.clone(),
                revision: h.row(&id).await.revision,
                mode: EnrollmentPlanMode::Now,
                when: None,
                enrollment: Some(enrollment.clone()),
                template: None,
            },
            &context("plan-test"),
        )
        .await
        .unwrap();
    h.cycle().await;
    assert!(h.row(&id).await.synced_at.is_some());
    assert_eq!(
        h.provider.relogin_proxies.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .endpoint(),
        "http://maintenance.example:8080/"
    );
    assert_eq!(
        h.accounts.import_settings(),
        vec![Some(enrollment.settings().unwrap())]
    );
    assert!(enrollment.config.outbound_proxy_id.is_none());
}

#[tokio::test]
async fn cancelling_queued_plan_keeps_settings_without_later_login_or_push() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    arrange(&h, &id, EnrollmentPlanMode::Now, None).await;
    h.services
        .relogin()
        .cancel(&id, h.row(&id).await.revision)
        .await
        .unwrap();
    h.cycle().await;
    let row = h.row(&id).await;
    assert!(row.enrollment.is_none());
    let plan = row.enrollment_plan.unwrap();
    assert_eq!(plan.state, EnrollmentPlanState::Draft);
    assert_eq!(
        plan.enrollment.custom_name.as_deref(),
        Some("Planned account")
    );
    assert!(h.provider.relogin_proxies.lock().unwrap().is_empty());
    assert!(h.accounts.import_settings().is_empty());
}

#[tokio::test]
async fn existing_immediate_enrollment_projects_its_saved_settings_without_a_plan_record() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    let mut row = h.row(&id).await;
    row.status = ReloginStatus::AwaitingWorkspace;
    row.enrollment = Some(intent());
    h.store.rows.lock().unwrap().insert(id, row);
    let view = h.services.relogin().list().await.unwrap();
    let plan = view.items[0].enrollment_plan.as_ref().unwrap();
    assert_eq!(plan.state, "verification");
    assert_eq!(plan.custom_name.as_deref(), Some("Planned account"));
    let json = serde_json::to_value(plan).unwrap();
    assert!(json.get("context").is_none());
    assert!(json.get("password").is_none());
}

#[tokio::test]
async fn imported_plans_execute_through_shared_queue_with_saved_settings_and_concurrency() {
    for mode in [EnrollmentPlanMode::Now, EnrollmentPlanMode::Scheduled] {
        let h = Harness::with_proxies(vec![], Some(proxies(true))).await;
        h.store.settings.lock().unwrap().concurrency = 2;
        *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_millis(20);
        let mut revisions = BTreeMap::new();
        for email in [
            "first@example.invalid",
            "second@example.invalid",
            "third@example.invalid",
        ] {
            let id = h.import(email).await;
            let mut row = h.row(&id).await;
            row.maintenance.login_paused = true;
            revisions.insert(id.clone(), row.revision);
            h.store.rows.lock().unwrap().insert(id, row);
        }
        let mut enrollment = intent();
        enrollment.config.credential_proxy_id = Some(Some("maintenance-proxy".into()));
        let when = (mode == EnrollmentPlanMode::Scheduled).then(|| Utc::now() + Duration::hours(1));
        let results = h
            .services
            .relogin()
            .arrange_imports(
                ImportedEnrollmentPlanCommand {
                    revisions: revisions.clone(),
                    mode,
                    when,
                    enrollment: enrollment.clone(),
                    template: None,
                },
                &context("batch-plan"),
            )
            .await
            .unwrap();
        assert!(results.iter().all(|r| r.success));
        assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
        for id in revisions.keys() {
            let row = h.row(id).await;
            assert!(!row.maintenance.login_paused);
            assert_eq!(row.enrollment_plan.as_ref().unwrap().when, when);
            assert_eq!(
                row.status,
                if mode == EnrollmentPlanMode::Now {
                    ReloginStatus::Queued
                } else {
                    ReloginStatus::Pending
                }
            );
        }
        if mode == EnrollmentPlanMode::Scheduled {
            h.cycle().await;
            assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
            for id in revisions.keys() {
                let mut row: ReloginEntry =
                    serde_json::from_value(serde_json::to_value(h.row(id).await).unwrap()).unwrap();
                row.enrollment_plan.as_mut().unwrap().when =
                    Some(Utc::now() - Duration::seconds(1));
                h.store.rows.lock().unwrap().insert(id.clone(), row);
            }
        }
        h.store.settings.lock().unwrap().paused = true;
        h.cycle().await;
        assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
        h.store.settings.lock().unwrap().paused = false;
        h.cycle().await;
        assert_eq!(h.provider.relogin_peak.load(Ordering::SeqCst), 2);
        assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 3);
        assert!(
            h.provider.relogin_proxies.lock().unwrap().iter().all(|p| p
                .as_ref()
                .unwrap()
                .endpoint()
                == "http://maintenance.example:8080/")
        );
        assert_eq!(
            h.accounts.import_settings(),
            vec![Some(enrollment.settings().unwrap()); 3]
        );
        for id in revisions.keys() {
            let row = h.row(id).await;
            assert!(row.synced_at.is_some());
            assert_eq!(
                row.enrollment_plan.unwrap().state,
                EnrollmentPlanState::Done
            );
        }
        h.cycle().await;
        assert_eq!(h.provider.relogin_requests.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn imported_plan_batch_rejects_invalid_timing_and_pause_before_any_write() {
    let h = Harness::new(vec![]).await;
    let id = h.import("first@example.invalid").await;
    let revision = h.row(&id).await.revision;
    for (mode, when, paused) in [
        (EnrollmentPlanMode::Now, None, true),
        (EnrollmentPlanMode::Scheduled, None, false),
        (
            EnrollmentPlanMode::Scheduled,
            Some(Utc::now() - Duration::hours(1)),
            false,
        ),
        (
            EnrollmentPlanMode::Draft,
            Some(Utc::now() + Duration::hours(1)),
            false,
        ),
        (EnrollmentPlanMode::Cancel, None, false),
    ] {
        h.store.settings.lock().unwrap().paused = paused;
        assert!(
            h.services
                .relogin()
                .arrange_imports(
                    ImportedEnrollmentPlanCommand {
                        revisions: BTreeMap::from([(id.clone(), revision)]),
                        mode,
                        when,
                        enrollment: intent(),
                        template: None,
                    },
                    &context("invalid-batch"),
                )
                .await
                .is_err()
        );
        let row = h.row(&id).await;
        assert_eq!(row.revision, revision);
        assert!(row.enrollment_plan.is_none());
    }
}

#[tokio::test]
async fn imported_execution_skips_existing_stale_and_scheduled_rows_and_reports_partial_save() {
    for mode in [EnrollmentPlanMode::Now, EnrollmentPlanMode::Scheduled] {
        let h = Harness::new(vec![account_record("openai")]).await;
        let mut revisions = BTreeMap::new();
        for email in [
            "test@example.invalid",
            "first@example.invalid",
            "second@example.invalid",
            "planned@example.invalid",
            "stale@example.invalid",
        ] {
            let id = h.import(email).await;
            if email.starts_with("planned") {
                arrange(
                    &h,
                    &id,
                    EnrollmentPlanMode::Scheduled,
                    Some(Utc::now() + Duration::days(1)),
                )
                .await;
            }
            revisions.insert(
                id.clone(),
                if email.starts_with("stale") {
                    0
                } else {
                    h.row(&id).await.revision
                },
            );
        }
        *h.store.fail_after.lock().unwrap() = Some(0);
        let results = h
            .services
            .relogin()
            .arrange_imports(
                ImportedEnrollmentPlanCommand {
                    revisions: revisions.clone(),
                    mode,
                    when: (mode == EnrollmentPlanMode::Scheduled)
                        .then(|| Utc::now() + Duration::hours(1)),
                    enrollment: intent(),
                    template: None,
                },
                &context("partial-batch"),
            )
            .await
            .unwrap();
        assert_eq!(results.iter().filter(|r| r.success).count(), 1);
        assert_eq!(results.iter().filter(|r| !r.success).count(), 4);
        let success = results.iter().find(|r| r.success).unwrap();
        let row = h.row(&success.id).await;
        let repeated = h
            .services
            .relogin()
            .arrange_imports(
                ImportedEnrollmentPlanCommand {
                    revisions: BTreeMap::from([(row.id.clone(), row.revision)]),
                    mode,
                    when: (mode == EnrollmentPlanMode::Scheduled)
                        .then(|| Utc::now() + Duration::hours(1)),
                    enrollment: intent(),
                    template: None,
                },
                &context("repeat-batch"),
            )
            .await
            .unwrap();
        assert!(!repeated[0].success);
        assert_eq!(h.row(&row.id).await.revision, row.revision);
    }
}
