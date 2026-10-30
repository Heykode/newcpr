use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::Utc;
use futures::StreamExt;
use gateway_core::account::ProviderAccountId;
use gateway_core::engine::provider::ProviderRequest;
use gateway_core::engine::{
    AccountAttemptContext, AttemptContext, ModelRequestId, RequestAttemptContext,
};
use gateway_core::lifecycle::CancellationToken;
use gateway_core::operation::{GenerateRequest, Operation, ProtocolPayload};
use gateway_core::policy::ClientApiKeyId;
use gateway_core::routing::{
    ClientRoutingScope, ConfigRevision, FrozenAccountScope, ModelCapabilities, ProviderKind,
    ProviderModel, PublicModelId, RoutingContext, RuntimeAccount, RuntimeAccountDirectory,
    RuntimeSnapshot, UpstreamModelId,
};
use gateway_core::task::{WorkerContribution, WorkerDaemonContext, WorkerRunnable};
use provider_openai::credential::ImportCodexOAuthCredential;
use serde_json::{Map, json};
use tokio::time::timeout;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::admin::{provider_ports_with_accounts, valid_config};
use crate::support::{MemoryAccountStore, account_policy, profile, secret};

fn request(account_id: &str) -> ProviderRequest {
    let provider = ProviderKind::new("openai").expect("provider");
    let upstream_model = UpstreamModelId::new("gpt-5.4").expect("upstream model");
    let public_model = PublicModelId::new(upstream_model.as_str()).expect("public model");
    let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object(
            "openai",
            Map::from_iter([
                ("model".to_owned(), json!("gpt-5.4")),
                ("input".to_owned(), json!("hello")),
            ]),
        )
        .expect("OpenAI payload")
        .with_context(Map::from_iter([("use_websocket".to_owned(), json!(false))])),
    ));
    let scope = account_scope(account_id);
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("revision"),
        account_policy(),
        vec![provider.clone()],
        vec![ProviderModel::new(
            provider,
            upstream_model,
            ModelCapabilities::new(BTreeSet::from([operation.kind()]), Some(32_000))
                .with_upstream_feature_validation(),
        )],
        Vec::new(),
    )
    .expect("runtime snapshot");
    let plan = snapshot
        .plan(&public_model, &operation, scope, &RoutingContext::default())
        .expect("routing plan");
    ProviderRequest::new(operation, plan.candidates()[0].clone())
}

fn attempt(request_id: &str, account_id: &str) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new(request_id).expect("request ID"),
            ClientApiKeyId::new("key_etag_backoff").expect("client key ID"),
        ),
        NonZeroU32::new(1).expect("attempt"),
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(account_scope(account_id)),
        None,
        CancellationToken::new(),
    )
}

fn account_scope(account_id: &str) -> Arc<FrozenAccountScope> {
    let provider = ProviderKind::new("openai").expect("provider");
    Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([(
            ProviderAccountId::new(account_id).expect("account ID"),
            RuntimeAccount::new(provider, BTreeSet::new()),
        )]))),
        ClientRoutingScope::all_accounts(),
    ))
}

#[tokio::test]
async fn catalog_etag_failure_returns_to_supervisor_and_recovers_pending_refresh() {
    let account_id = "acct_catalog_etag_worker";
    let store = Arc::new(MemoryAccountStore::default());
    store
        .seed_oauth_credential(ImportCodexOAuthCredential {
            account_id: account_id.to_owned(),
            name: account_id.to_owned(),
            secret: secret("at-catalog-etag-worker"),
            verified_account: profile("chatgpt-catalog-etag-worker"),
            next_refresh_at: Some(Utc::now() + chrono::Duration::minutes(30)),
            enabled: true,
        })
        .await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("x-models-etag", "models-worker-v2")
                .set_body_string(concat!(
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_etag\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[]}}\n\n"
                )),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(ResponseTemplate::new(503))
        .with_priority(1)
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            include_bytes!("../transport/fixtures/official_models_snapshot.json"),
            "application/json",
        ))
        .with_priority(2)
        .expect(1)
        .mount(&server)
        .await;
    let mut config = valid_config();
    config.config.api.base_url = server.uri();
    let mut bundle =
        provider_openai::initialize(config.config, provider_ports_with_accounts(store))
            .await
            .unwrap();
    let provider = bundle.core_provider();
    let mut stream = provider
        .execute(
            request(account_id),
            attempt("req_catalog_etag_worker", account_id),
        )
        .await
        .expect("provider stream");
    while let Some(event) = stream.next().await {
        event.expect("response carrying catalog ETag");
    }
    let worker = bundle
        .take_worker_contributions()
        .into_iter()
        .find_map(|contribution| match contribution {
            WorkerContribution::Registration(registration)
                if registration.id.owner() == "openai-model-etag" =>
            {
                Some(registration)
            }
            _ => None,
        })
        .expect("catalog ETag worker");
    let WorkerRunnable::Daemon { task, .. } = worker.runnable else {
        panic!("catalog ETag worker must be a daemon");
    };

    for expected_requests in 1..=2 {
        let context = WorkerDaemonContext::new(CancellationToken::new());
        let result = timeout(Duration::from_secs(2), task.run(context.clone())).await;
        let requests = server.received_requests().await.unwrap();
        let paths: Vec<_> = requests.iter().map(|request| request.url.path()).collect();
        assert!(
            result.is_ok(),
            "failed refresh must return control to the supervisor: {paths:?}, completed={}",
            context.completed_cycles()
        );
        let error = result.unwrap().expect_err("catalog unavailable");
        assert!(
            error.error_details().is_some(),
            "preserve the failure cause"
        );
        assert_eq!(context.completed_cycles(), 0);
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.url.path() == "/codex/models")
                .count(),
            expected_requests,
            "each run must perform only one failed refresh"
        );
    }

    let context = WorkerDaemonContext::new(CancellationToken::new());
    let run = task.run(context.clone());
    tokio::pin!(run);
    tokio::select! {
        result = &mut run => panic!("daemon exited before recovery: {result:?}"),
        result = timeout(Duration::from_secs(2), context.progress_since(0)) => {
            result.expect("pending ETag must survive failed runs");
        }
    }
    assert_eq!(context.completed_cycles(), 1);
    context.cancellation().cancel();
    timeout(Duration::from_secs(2), run)
        .await
        .expect("idle worker must stop on cancellation")
        .expect("cancelled worker");
}
