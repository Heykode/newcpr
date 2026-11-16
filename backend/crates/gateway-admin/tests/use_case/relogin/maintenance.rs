use super::*;
use gateway_admin::model::relogin_maintenance::*;

fn outcome() -> MaintenanceOutcome {
    MaintenanceOutcome {
        view: MaintenanceView {
            state: "succeeded".into(),
            ..Default::default()
        },
        session: None,
        credential: None,
        pending_totp: None,
        new_secret: None,
    }
}
fn command(row: &ReloginEntry, action: MaintenanceAction) -> MaintenanceCommand {
    MaintenanceCommand {
        id: row.id.clone(),
        revision: row.revision,
        action,
        target: None,
        device_id_hash: None,
        confirm: Some("maintain_account".into()),
    }
}
fn candidate() -> PendingTotp {
    PendingTotp {
        secret: "JBSWY3DPEHPK3PXP".into(),
        session_id: "fixture-enrollment".into(),
        created_at: Utc::now(),
    }
}
async fn save_private(h: &Harness, row: ReloginEntry) {
    h.store.rows.lock().unwrap().insert(row.id.clone(), row);
}

#[tokio::test]
async fn reads_and_local_edits_preserve_uncertain_candidate_until_explicit_resolution() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("maintenance@example.invalid").await;
    let mut row = h.row(&id).await;
    row.maintenance.uncertain = true;
    row.maintenance.pending_totp = Some(candidate());
    save_private(&h, row).await;
    *h.provider.maintenance_result.lock().unwrap() = Some(outcome());
    h.services
        .relogin()
        .maintain(
            command(&h.row(&id).await, MaintenanceAction::Devices),
            &context("read"),
        )
        .await
        .unwrap();
    let row = h.row(&id).await;
    assert!(row.maintenance.uncertain);
    assert_eq!(row.maintenance.view.state, "uncertain");
    assert!(
        !h.services
            .relogin()
            .queue(std::slice::from_ref(&id))
            .await
            .unwrap()[0]
            .success
    );
    let material = h
        .services
        .relogin()
        .maintenance_material(&id, &context("material"))
        .await
        .unwrap();
    assert_eq!(material.revision, row.revision);
    assert!(material.needs_resolution);
    h.services
        .relogin()
        .update_material(
            MaterialUpdate {
                id: id.clone(),
                revision: material.revision,
                password: "fixture-updated-password".into(),
                mfa_secret: row.mfa_secret,
                confirm_maintenance_resolution: false,
            },
            &context("edit"),
        )
        .await
        .unwrap();
    assert!(h.row(&id).await.maintenance.uncertain);
    let row = h.row(&id).await;
    h.services
        .relogin()
        .update_material(
            MaterialUpdate {
                id: id.clone(),
                revision: row.revision,
                password: row.password,
                mfa_secret: row.mfa_secret,
                confirm_maintenance_resolution: true,
            },
            &context("resolve"),
        )
        .await
        .unwrap();
    let row = h.row(&id).await;
    assert!(!row.maintenance.uncertain);
    assert!(row.maintenance.pending_totp.is_none());
    assert_eq!(
        row.maintenance
            .material_history
            .last()
            .unwrap()
            .pending_totp
            .as_ref()
            .unwrap()
            .session_id,
        "fixture-enrollment"
    );
}

#[tokio::test]
async fn logout_all_pauses_recovery_until_explicit_authorization() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("maintenance@example.invalid").await;
    *h.provider.maintenance_result.lock().unwrap() = Some(outcome());
    let view = h
        .services
        .relogin()
        .maintain(
            command(&h.row(&id).await, MaintenanceAction::LogoutAll),
            &context("logout"),
        )
        .await
        .unwrap();
    assert!(view.login_paused);
    h.cycle().await;
    assert!(h.row(&id).await.maintenance.login_paused);
    assert!(h.provider.relogin_requests.lock().unwrap().is_empty());
    assert!(
        h.services
            .relogin()
            .queue(std::slice::from_ref(&id))
            .await
            .unwrap()[0]
            .success
    );
    assert!(!h.row(&id).await.maintenance.login_paused);
}

#[tokio::test]
async fn stale_revision_and_duplicate_candidate_never_call_provider() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("maintenance@example.invalid").await;
    let row = h.row(&id).await;
    let mut stale = command(&row, MaintenanceAction::LogoutAll);
    stale.revision -= 1;
    assert!(
        h.services
            .relogin()
            .maintain(stale, &context("stale"))
            .await
            .is_err()
    );
    let mut pending = row;
    pending.maintenance.pending_totp = Some(candidate());
    save_private(&h, pending).await;
    assert!(
        h.services
            .relogin()
            .maintain(
                command(&h.row(&id).await, MaintenanceAction::PrepareTotp),
                &context("duplicate")
            )
            .await
            .is_err()
    );
    assert!(h.provider.maintenance_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn identity_mismatch_finishes_attempt_without_overwriting_old_material() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("maintenance@example.invalid").await;
    let mut result = outcome();
    result.credential = Some(credential());
    *h.provider.maintenance_result.lock().unwrap() = Some(result);
    let view = h
        .services
        .relogin()
        .maintain(
            command(&h.row(&id).await, MaintenanceAction::Authorize),
            &context("identity"),
        )
        .await
        .unwrap();
    assert_eq!(view.state, "failed");
    let row = h.row(&id).await;
    assert!(row.credential.is_none());
    assert_eq!(
        row.last_attempt.as_ref().unwrap().result,
        ReloginAttemptResult::Failed
    );
}

#[tokio::test]
async fn maintenance_shares_account_lock_and_cancellation_keeps_unknown_result() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("maintenance@example.invalid").await;
    *h.provider.relogin_delay.lock().unwrap() = std::time::Duration::from_secs(60);
    *h.provider.maintenance_result.lock().unwrap() = Some(outcome());
    let services = h.services.clone();
    let command = command(&h.row(&id).await, MaintenanceAction::ActivateTotp);
    let pending = tokio::spawn(async move {
        services
            .relogin()
            .maintain(command, &context("cancel"))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while h.provider.maintenance_calls.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !h.services
            .relogin()
            .queue(std::slice::from_ref(&id))
            .await
            .unwrap()[0]
            .success
    );
    let mut row = h.row(&id).await;
    row.maintenance.import_egress = None;
    save_private(&h, row.clone()).await;
    h.services
        .relogin()
        .cancel(&id, row.revision)
        .await
        .unwrap();
    assert!(pending.await.unwrap().is_err());
    let row = h.row(&id).await;
    assert!(row.maintenance.uncertain);
    assert_eq!(
        row.maintenance.last_activity.unwrap().result,
        ReloginAttemptResult::Cancelled
    );
}

#[tokio::test]
async fn security_unknown_replaces_stale_enabled_state() {
    let h = Harness::new(Vec::new()).await;
    let id = h.import("maintenance@example.invalid").await;
    let mut row = h.row(&id).await;
    row.maintenance.view.totp_enabled = Some(true);
    save_private(&h, row).await;
    *h.provider.maintenance_result.lock().unwrap() = Some(outcome());
    let view = h
        .services
        .relogin()
        .maintain(
            command(&h.row(&id).await, MaintenanceAction::Security),
            &context("security"),
        )
        .await
        .unwrap();
    assert_eq!(view.totp_enabled, None);
}

#[tokio::test]
async fn maintenance_and_list_follow_current_account_proxy_and_explicit_removal() {
    use gateway_core::account::OutboundProxy;
    let mut pooled = account(false);
    let first = OutboundProxy::parse("http://first.example.invalid:8080").unwrap();
    let second = OutboundProxy::parse("http://second.example.invalid:8080").unwrap();
    pooled.outbound_proxy = Some(first.clone());
    pooled.outbound_proxy_enabled = true;
    let h = Harness::new(vec![pooled.clone()]).await;
    let id = h.import("test@example.invalid").await;
    for proxy in [Some(first), Some(second), None] {
        pooled.outbound_proxy = proxy.clone();
        h.accounts.set_accounts(vec![pooled.clone()]);
        let view = h.services.relogin().list().await.unwrap().items.remove(0);
        assert_eq!(
            view.egress.address,
            proxy.as_ref().map(OutboundProxy::endpoint)
        );
        for action in [
            MaintenanceAction::CheckSession,
            MaintenanceAction::Devices,
            MaintenanceAction::Workspaces,
            MaintenanceAction::Security,
        ] {
            *h.provider.maintenance_result.lock().unwrap() = Some(outcome());
            h.services
                .relogin()
                .maintain(
                    command(&h.row(&id).await, action),
                    &context("maintenance-route"),
                )
                .await
                .unwrap();
            assert_eq!(
                h.provider
                    .maintenance_routes
                    .lock()
                    .unwrap()
                    .last()
                    .unwrap()
                    .0,
                proxy
            );
        }
    }
}

#[tokio::test]
async fn imported_proxy_survives_manual_queue_and_is_used_by_maintenance() {
    let proxies = Arc::new(super::super::proxies::TestProxies {
        events: Some(events()),
        ..Default::default()
    });
    let h = Harness::with_proxies(vec![], Some(proxies)).await;
    let mut config = template_config();
    config.outbound_proxy_id = Some("fixture-proxy".into());
    let enrollment = ReloginEnrollment::new(
        config.settings().unwrap(),
        Some("fixture-proxy".into()),
        context("import-route"),
    )
    .unwrap();
    let id = h
        .services
        .relogin()
        .enroll(
            "test@example.invalid----test-only-password----JBSWY3DPEHPK3PXP",
            false,
            enrollment,
        )
        .await
        .unwrap()
        .remove(0);
    let row = h.row(&id).await;
    h.services
        .relogin()
        .cancel(&id, row.revision)
        .await
        .unwrap();
    h.ready(&id).await;
    assert!(
        h.provider
            .relogin_proxies
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .is_some()
    );
    *h.provider.maintenance_result.lock().unwrap() = Some(outcome());
    h.services
        .relogin()
        .maintain(
            command(&h.row(&id).await, MaintenanceAction::Devices),
            &context("import-maintenance"),
        )
        .await
        .unwrap();
    assert_eq!(
        h.provider
            .maintenance_routes
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .0,
        *h.provider.relogin_proxies.lock().unwrap().last().unwrap()
    );
}

#[tokio::test]
async fn maintenance_uses_the_existing_fixed_ipv6_and_never_falls_back_when_disabled() {
    use gateway_core::{
        account::ProviderAccountId,
        provider_ports::egress::{EgressMode, ProviderEgressAddress, ProviderEgressConfig},
    };
    let pooled = account(false);
    let account_id = ProviderAccountId::new(pooled.id.clone()).unwrap();
    let h = Harness::new(vec![pooled]).await;
    let id = h.import("test@example.invalid").await;
    let source = "2001:db8::21".parse().unwrap();
    let mut state = ProviderEgressConfig {
        default_mode: EgressMode::FixedIpv6Reuse,
        addresses: vec![ProviderEgressAddress {
            id: "fixture-ipv6".into(),
            address: source,
            enabled: true,
        }],
        account_overrides: BTreeMap::from([(account_id.clone(), None)]),
        fixed_bindings: BTreeMap::from([(account_id, source)]),
        ..Default::default()
    };
    *h.provider.relogin_egress.lock().unwrap() = Some(Arc::new(state.clone()));
    *h.provider.maintenance_result.lock().unwrap() = Some(outcome());
    h.services
        .relogin()
        .maintain(
            command(&h.row(&id).await, MaintenanceAction::Devices),
            &context("fixed-source"),
        )
        .await
        .unwrap();
    assert_eq!(
        h.provider.maintenance_routes.lock().unwrap()[0].1,
        Some(source)
    );
    state.addresses[0].enabled = false;
    *h.provider.relogin_egress.lock().unwrap() = Some(Arc::new(state));
    assert!(
        h.services
            .relogin()
            .maintain(
                command(&h.row(&id).await, MaintenanceAction::Devices),
                &context("disabled-source")
            )
            .await
            .is_err()
    );
    assert_eq!(h.provider.maintenance_routes.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn material_history_does_not_replace_the_login_attempt_waiting_for_push() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    h.ready(&id).await;
    let row = h.row(&id).await;
    let attempt_id = row.last_attempt.as_ref().unwrap().id.clone();
    h.services
        .relogin()
        .update_material(
            MaterialUpdate {
                id: id.clone(),
                revision: row.revision,
                password: "updated-test-password".into(),
                mfa_secret: row.mfa_secret,
                confirm_maintenance_resolution: false,
            },
            &context("edit-material"),
        )
        .await
        .unwrap();
    let row = h.row(&id).await;
    assert_eq!(row.last_attempt.as_ref().unwrap().id, attempt_id);
    assert_eq!(
        row.maintenance.last_activity.as_ref().unwrap().operation,
        Some(MaintenanceAction::EditMaterial)
    );
    let history = h.services.relogin().history(&id, None).await.unwrap();
    assert_eq!(history.items.len(), 2);
    let serialized = serde_json::to_string(&history).unwrap();
    assert!(!serialized.contains(&row.password));
    assert!(!serialized.contains(&row.mfa_secret));
}

#[tokio::test]
async fn maintenance_reads_preserve_authorization_push_history() {
    let h = Harness::new(vec![]).await;
    let id = h.import("test@example.invalid").await;
    let mut authorized = outcome();
    authorized.credential = Some(credential());
    *h.provider.maintenance_result.lock().unwrap() = Some(authorized);
    h.services
        .relogin()
        .maintain(
            command(&h.row(&id).await, MaintenanceAction::Authorize),
            &context("authorize"),
        )
        .await
        .unwrap();
    let original = h.row(&id).await.last_attempt.unwrap();
    assert_eq!(original.result, ReloginAttemptResult::Succeeded);
    *h.provider.maintenance_result.lock().unwrap() = Some(outcome());
    h.services
        .relogin()
        .maintain(
            command(&h.row(&id).await, MaintenanceAction::Devices),
            &context("devices"),
        )
        .await
        .unwrap();
    let row = h.row(&id).await;
    assert_eq!(row.last_attempt.unwrap().id, original.id);
    let results = h
        .services
        .relogin()
        .push_with_template(
            &[id.clone()],
            &BTreeMap::from([(id.clone(), row.revision)]),
            None,
            None,
            &context("push"),
        )
        .await
        .unwrap();
    assert!(results[0].success, "{:?}", results);
    let history = h.services.relogin().history(&id, None).await.unwrap();
    let login = history
        .items
        .iter()
        .find(|item| item.id == original.id)
        .unwrap();
    assert_eq!(login.push_result, ReloginPushResult::Succeeded);
    let devices = history
        .items
        .iter()
        .find(|item| item.operation == Some(MaintenanceAction::Devices))
        .unwrap();
    assert_eq!(devices.push_result, ReloginPushResult::NotRequested);
}

#[tokio::test]
async fn new_account_push_template_route_overrides_or_preserves_import() {
    use gateway_core::provider_ports::egress::EgressMode;
    for (preserve, template_proxy, mode, expected_proxy) in [
        (true, None, None, Some("proxy-import")),
        (false, Some("proxy-template"), None, Some("proxy-template")),
        (false, None, None, None),
        (true, None, Some(Some(EgressMode::FixedIpv6Reuse)), None),
    ] {
        let proxies = Arc::new(super::super::proxies::TestProxies {
            events: Some(events()),
            ..Default::default()
        });
        let h = Harness::with_proxies(vec![], Some(proxies)).await;
        let id = h.import("test@example.invalid").await;
        let mut row = h.row(&id).await;
        row.maintenance.import_egress = Some(ReloginImportEgress {
            proxy_id: Some("proxy-import".into()),
            mode: None,
        });
        save_private(&h, row).await;
        h.ready(&id).await;
        let mut config = template_config();
        config.preserve_outbound_proxy = preserve;
        config.outbound_proxy_id = template_proxy.map(str::to_owned);
        config.egress_mode = mode;
        let template = h
            .services
            .account_templates()
            .save_template(None, config)
            .await
            .unwrap();
        let revision = h.row(&id).await.revision;
        let result = h
            .services
            .relogin()
            .push_with_template(
                &[id.clone()],
                &BTreeMap::from([(id.clone(), revision)]),
                Some(template_selection(&template)),
                None,
                &context("template-route"),
            )
            .await
            .unwrap();
        assert!(result[0].success, "{:?}", result);
        let pool = h.accounts.accounts.lock().unwrap().clone();
        assert_eq!(pool[0].outbound_proxy_id.as_deref(), expected_proxy);
        assert_eq!(
            h.accounts.import_settings()[0]
                .as_ref()
                .unwrap()
                .egress_mode,
            mode
        );
    }
}

#[tokio::test]
async fn explicit_workspace_maintenance_keeps_its_proxy_when_other_workspace_differs() {
    use gateway_core::account::OutboundProxy;
    let mut first = account(false);
    first.outbound_proxy = Some(OutboundProxy::parse("http://first.example:8080").unwrap());
    let mut second = first.clone();
    second.id = "acct_second".into();
    second.upstream_account_id = Some("workspace-second".into());
    second.outbound_proxy = Some(OutboundProxy::parse("http://second.example:8080").unwrap());
    let h = Harness::new(vec![first.clone(), second]).await;
    let id = h.import("test@example.invalid").await;
    assert_eq!(
        h.services.relogin().list().await.unwrap().items[0]
            .egress
            .kind,
        "multiple"
    );
    let mut row = h.row(&id).await;
    row.preferred_workspace_id = first.upstream_account_id.clone();
    row.maintenance.view.workspaces = vec![MaintenanceWorkspace {
        id: "workspace-second".into(),
        name: "Second".into(),
        plan_type: None,
        available: true,
    }];
    save_private(&h, row).await;
    let mut request = command(&h.row(&id).await, MaintenanceAction::SwitchWorkspace);
    request.target = Some("workspace-second".into());
    *h.provider.maintenance_result.lock().unwrap() = Some(outcome());
    h.services
        .relogin()
        .maintain(request, &context("workspace-route"))
        .await
        .unwrap();
    assert_eq!(
        h.provider.maintenance_routes.lock().unwrap()[0].0,
        first.outbound_proxy
    );
}
