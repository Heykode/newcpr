use super::accounts::{FakeAccountStore, FakeProviderAdmin};
use super::*;
use gateway_admin::model::excel_recovery::{
    ExcelRecoveryClaim, ExcelRecoveryConfig, ExcelRecoveryOutcome,
};
use gateway_core::{
    lifecycle::CancellationToken,
    operation::Operation,
    task::{WorkerContribution, WorkerRunnable},
};
use std::sync::atomic::{AtomicBool, Ordering};

struct RecoveryProbe {
    mode: &'static str,
    started: AtomicBool,
    drained: AtomicBool,
}

impl AccountProbe for RecoveryProbe {
    fn probe(
        &self,
        _: AccountProbeRequest,
    ) -> BoxFuture<'_, Result<AccountProbeResult, AccountProbeError>> {
        Box::pin(async { panic!("recovery must never use ordinary diagnostic probe") })
    }
    fn excel_recovery(
        &self,
        request: AccountProbeRequest,
        revision: u64,
        config: u64,
        expected_nonce: String,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<AccountProbeResult, AccountProbeError>> {
        Box::pin(async move {
            assert_eq!(request.account_id.as_str(), "acct_recovery");
            assert_eq!(request.upstream_model.as_str(), "fixture-model");
            assert_eq!((revision, config), (7, 11));
            self.started.store(true, Ordering::SeqCst);
            if self.mode == "pending" {
                cancellation.cancelled().await;
                tokio::task::yield_now().await;
                self.drained.store(true, Ordering::SeqCst);
                return Err(
                    GatewayError::new(GatewayErrorKind::Cancelled, "fixture cancellation").into(),
                );
            }
            if self.mode == "failure" {
                return Err(GatewayError::new(
                    GatewayErrorKind::UpstreamUnavailable,
                    "fixture upstream failure",
                )
                .into());
            }
            let Operation::Generate(generate) = request.operation else {
                panic!("generate");
            };
            let prompt = generate
                .protocol_payload()
                .body()
                .get("input")
                .and_then(serde_json::Value::as_str)
                .unwrap();
            let nonce = prompt.rsplit_once(' ').unwrap().1;
            assert_eq!(nonce, expected_nonce);
            assert!(uuid::Uuid::parse_str(nonce).is_ok());
            let text = match self.mode {
                "match" => vec![nonce[..10].into(), nonce[10..].into()],
                "empty" => vec![],
                _ => vec!["not the expected nonce".into()],
            };
            Ok(AccountProbeResult {
                text,
                upstream_response_model: Some("fixture-model".into()),
            })
        })
    }
}

fn claim() -> ExcelRecoveryClaim {
    ExcelRecoveryClaim {
        account_id: "acct_recovery".into(),
        generation: 1,
        lease_id: "fixture-lease".into(),
        credential_revision: 7,
        config_revision: 11,
        model: "fixture-model".into(),
        scope: serde_json::json!([]),
    }
}

#[test]
fn recovery_config_is_opt_in_and_rejects_invalid_intervals() {
    assert!(!ExcelRecoveryConfig::default().enabled);
    assert_eq!(ExcelRecoveryConfig::default().interval_minutes, 60);
    for value in [
        serde_json::json!({"enabled":true,"intervalMinutes":0}),
        serde_json::json!({"enabled":true,"intervalMinutes":10081}),
        serde_json::json!({"enabled":true,"intervalMinutes":1.5}),
        serde_json::json!({"enabled":true,"intervalMinutes":5,"unknown":true}),
    ] {
        assert!(serde_json::from_value::<ExcelRecoveryConfig>(value).is_err());
    }
    assert!(
        serde_json::from_value::<ExcelRecoveryConfig>(
            serde_json::json!({"enabled":true,"intervalMinutes":10080})
        )
        .is_ok()
    );
}

#[test]
fn recovery_template_options_preserve_omission_and_apply_explicit_false() {
    use gateway_admin::model::relogin_templates::{ExcelImportSettings, ReloginTemplateConfig};
    let template: ReloginTemplateConfig=serde_json::from_value(serde_json::json!({
        "name":"Recovery template","enabled":true,"weight":1,"groupIds":[],"preserveOutboundProxy":true,
        "excelRecovery":{"enabled":true,"intervalMinutes":15}
    })).unwrap();
    let mut settings = template.settings().unwrap();
    let mut options: ExcelImportSettings = serde_json::from_value(
        serde_json::json!({"responsesUpstream":"excel","excelModelsFollowGlobal":true}),
    )
    .unwrap();
    options.apply(&mut settings);
    assert_eq!(
        settings.excel_recovery,
        Some(ExcelRecoveryConfig {
            enabled: true,
            interval_minutes: 15
        })
    );
    options.excel_recovery = Some(ExcelRecoveryConfig::default());
    options.apply(&mut settings);
    assert_eq!(
        settings.excel_recovery,
        Some(ExcelRecoveryConfig::default())
    );
}

#[tokio::test(start_paused = true)]
async fn recovery_worker_requires_matching_nonce_and_drains_timeout_or_superseded_work() {
    for (mode, expected, supersede) in [
        ("match", ExcelRecoveryOutcome::Recovered, false),
        ("empty", ExcelRecoveryOutcome::ResponseMismatch, false),
        ("mismatch", ExcelRecoveryOutcome::ResponseMismatch, false),
        ("failure", ExcelRecoveryOutcome::RequestFailed, false),
        ("pending", ExcelRecoveryOutcome::RequestFailed, false),
        ("pending", ExcelRecoveryOutcome::Cancelled, true),
    ] {
        let log = Arc::new(Mutex::new(vec![]));
        let store = FakeAccountStore::new("openai", log.clone());
        *store.recovery_claim.lock().unwrap() = Some(claim());
        let probe = Arc::new(RecoveryProbe {
            mode,
            started: AtomicBool::new(false),
            drained: AtomicBool::new(false),
        });
        let mut bundle = AdminHarness::new()
            .accounts(store.clone())
            .provider(FakeProviderAdmin::new("openai", log))
            .probe(probe.clone())
            .build_bundle()
            .await;
        let registration = bundle
            .take_worker_contributions()
            .into_iter()
            .find_map(|worker| match worker {
                WorkerContribution::Registration(registration)
                    if registration.id.owner() == "admin_excel_recovery" =>
                {
                    Some(registration)
                }
                _ => None,
            })
            .expect("registered recovery worker");
        let WorkerRunnable::Daemon { task, .. } = registration.runnable else {
            panic!("daemon");
        };
        let cancel = CancellationToken::new();
        let child = cancel.clone();
        let worker = tokio::spawn(async move {
            task.run(child).await.unwrap();
        });
        for _ in 0..100 {
            if probe.started.load(Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(probe.started.load(Ordering::SeqCst));
        if supersede {
            store.recovery_current.store(false, Ordering::SeqCst);
        }
        if mode == "pending" {
            tokio::time::advance(std::time::Duration::from_secs(if supersede {
                2
            } else {
                46
            }))
            .await;
        }
        for _ in 0..100 {
            if !store.recovery_results.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            *store.recovery_results.lock().unwrap(),
            vec![expected],
            "{mode},supersede={supersede}"
        );
        if mode == "pending" {
            assert!(
                probe.drained.load(Ordering::SeqCst),
                "release only after cancellation drains"
            );
        }
        cancel.cancel();
        worker.await.unwrap();
    }
}
