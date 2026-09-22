use super::*;
use crate::provider::turn_state::{ProbeStates, fresh_state};
use gateway_core::{
    provider_ports::{
        OpaqueTurnState, ProviderTurnStateCandidate, ProviderTurnStatePort, ProviderTurnStateSlot,
        ProviderTurnStateValue,
    },
    routing::OpenAiTurnStatePolicy,
    runtime::RequestTuningHandle,
    task::{WorkerContribution, WorkerRunnable},
};
use provider_openai::credential::CodexCredentialAdmin;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn business_turn_state_learning_requires_completed_success_and_keeps_model_scope() {
    for scenario in [
        "same",
        "new",
        "missing",
        "failed",
        "truncated",
        "incomplete",
        "late_error",
    ] {
        let accounts = Arc::new(MemoryAccountStore::default());
        let mut imported = CodexCredentialAdmin
            .prepare_import(ImportCodexOAuthCredential {
                account_id: "acct_provider_contract".to_owned(),
                name: "Managed business test".to_owned(),
                secret: secret("managed-business-test"),
                verified_account: profile("managed-business-owner"),
                next_refresh_at: None,
                enabled: true,
            })
            .unwrap();
        imported.account = imported.account.with_turn_state_injection_enabled(true);
        let account = imported.account.clone();
        accounts.create_account(imported).await.unwrap();
        let states = Arc::new(ProbeStates::default());
        let ports =
            crate::admin::provider_ports_with_accounts(accounts).with_turn_states(states.clone());
        let server = MockServer::start().await;
        let mut config = crate::admin::valid_config();
        config.config.api.base_url = server.uri();
        let tuning = RequestTuningHandle::default();
        let model = UpstreamModelId::new("gpt-5.4").unwrap();
        tuning.publish_openai_turn_state_policy(OpenAiTurnStatePolicy::new(
            true,
            [model.clone()].into(),
        ));
        let mut bundle =
            provider_openai::initialize_with_request_tuning(config.config, ports, tuning)
                .await
                .unwrap();
        let now = SystemTime::now();
        let initial_value = fresh_state(1);
        let initial = states
            .put_candidate(ProviderTurnStateCandidate {
                account_id: account.id().clone(),
                expected_revision: account.turn_state_binding_revision(),
                expected_active_version: None,
                upstream_model: model.clone(),
                normal_length: 292,
                slot: ProviderTurnStateSlot::Active,
                value: ProviderTurnStateValue::new(
                    OpaqueTurnState::new(initial_value.clone()),
                    now,
                    now + Duration::from_secs(190),
                ),
                observed_at: now,
            })
            .await
            .unwrap();
        let cancelled = CancellationToken::new();
        let observer = bundle
            .take_worker_contributions()
            .into_iter()
            .find_map(|entry| match entry {
                WorkerContribution::Registration(registration)
                    if registration.id.owner() == "openai-turn-state-observations" =>
                {
                    match registration.runnable {
                        WorkerRunnable::Daemon { task, .. } => Some(task),
                        _ => None,
                    }
                }
                _ => None,
            })
            .unwrap();
        let cancellation = cancelled.clone();
        let handle = tokio::spawn(async move { observer.run(cancellation).await });
        let error = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"server_error\",\"code\":\"server_error\",\"message\":\"synthetic failure\"}}\n\n";
        let created = "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_state_business\",\"model\":\"different-model\",\"status\":\"in_progress\"}}\n\n";
        let completed = "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_state_business\",\"model\":\"different-model\",\"status\":\"completed\",\"output\":[]}}\n\n";
        let body = match scenario {
            "failed" => error.to_owned(),
            "truncated" => "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_state_business\",\"status\":\"in_progress\",\"output\":[]}}\n\n".to_owned(),
            "incomplete" => "event: response.incomplete\ndata: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_state_business\",\"status\":\"incomplete\",\"output\":[]}}\n\n".to_owned(),
            "late_error" => format!("{completed}{error}"),
            _ => completed.to_owned(),
        };
        let mut response = ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(format!("{created}{body}"));
        if scenario != "missing" {
            response = response.insert_header(
                "x-codex-turn-state",
                if scenario == "same" {
                    initial_value.clone()
                } else {
                    fresh_state(2)
                },
            );
        }
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(response)
            .mount(&server)
            .await;
        let result = bundle
            .core_provider()
            .execute(
                planned_request("openai", http_generate_operation()),
                context("req_state_business", CancellationToken::new()),
            )
            .await;
        let learns = matches!(scenario, "same" | "new");
        let mut stream = result.expect("managed account must pass new-chain selection");
        let mut completed = false;
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => {
                    completed |= event
                        .canonical_facts()
                        .iter()
                        .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
                }
                Err(error) => assert!(!learns, "{scenario}: {error:?}"),
            }
        }
        assert!(completed || !learns, "{scenario}: no successful completion");
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].headers.get("x-codex-turn-state").unwrap(),
            &initial_value
        );
        if learns {
            timeout(Duration::from_secs(3), async {
                while states.writes.load(Ordering::SeqCst) < 2 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("{scenario}: qualified business response was not learned"));
        } else {
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert_eq!(states.writes.load(Ordering::SeqCst), 1, "{scenario}");
            assert!(states.observations.lock().unwrap().is_empty(), "{scenario}");
        }
        cancelled.cancel();
        handle.await.unwrap().unwrap();
        let record = states
            .read(account.id(), &model, account.turn_state_binding_revision())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            record.state_version(),
            initial.state_version(),
            "{scenario}"
        );
        if scenario == "same" {
            assert!(record.active().unwrap().expires_at() > initial.active().unwrap().expires_at());
            assert!(record.standby().is_none());
        } else if scenario == "new" {
            assert_eq!(record.active(), initial.active());
            assert!(record.standby().is_some());
        } else {
            assert_eq!(record, initial);
        }
        assert_eq!(
            states.records.lock().unwrap().len(),
            1,
            "reported model must not create another partition"
        );
    }
}
