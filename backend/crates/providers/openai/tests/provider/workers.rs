use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

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
use gateway_core::task::{WorkerContribution, WorkerRunnable};
use provider_openai::credential::ImportCodexOAuthCredential;
use serde_json::{Map, json};
use tokio::time::timeout;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::admin::{provider_ports_with_accounts, valid_config};
use crate::support::{MemoryAccountStore, account_policy, profile, secret};

const COMPLETED_SSE: &str = concat!(
    "event: response.completed\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_etag_worker\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
);

struct ResponseEtagResponder {
    calls: Arc<AtomicUsize>,
}

impl Respond for ResponseEtagResponder {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let version = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .insert_header("x-models-etag", format!("models-v{version}"))
            .set_body_string(COMPLETED_SSE)
    }
}

struct CatalogBackoffResponder {
    calls: Arc<AtomicUsize>,
    attempts: Arc<Mutex<Vec<Instant>>>,
}

impl Respond for CatalogBackoffResponder {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let attempt = self.calls.fetch_add(1, Ordering::SeqCst);
        self.attempts
            .lock()
            .expect("catalog attempt lock")
            .push(Instant::now());
        if matches!(attempt, 2 | 4) {
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "models": [{"slug": "gpt-5.4", "display_name": "GPT-5.4"}]
                }))
        } else {
            ResponseTemplate::new(401)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({"detail": "Unauthorized"}))
        }
    }
}

async fn wait_for_calls(calls: &AtomicUsize, expected: usize) {
    timeout(Duration::from_secs(8), async {
        while calls.load(Ordering::SeqCst) < expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("catalog attempt {expected} did not start"));
}

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

async fn execute_response(
    provider: &Arc<dyn gateway_core::engine::provider::Provider>,
    account_id: &str,
    request_id: &str,
) {
    let mut stream = provider
        .execute(request(account_id), attempt(request_id, account_id))
        .await
        .expect("prepare provider stream");
    while let Some(event) = stream.next().await {
        event.expect("complete provider stream");
    }
}

#[tokio::test]
async fn model_etag_daemon_backs_off_and_resets_after_success() {
    let account_id = "acct_etag_backoff";
    let store = Arc::new(MemoryAccountStore::default());
    store
        .seed_oauth_credential(ImportCodexOAuthCredential {
            account_id: account_id.to_owned(),
            name: account_id.to_owned(),
            secret: secret("at-etag-backoff"),
            verified_account: profile("chatgpt-etag-backoff"),
            next_refresh_at: None,
            enabled: true,
        })
        .await;

    let response_calls = Arc::new(AtomicUsize::new(0));
    let catalog_calls = Arc::new(AtomicUsize::new(0));
    let catalog_attempts = Arc::new(Mutex::new(Vec::new()));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseEtagResponder {
            calls: Arc::clone(&response_calls),
        })
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(CatalogBackoffResponder {
            calls: Arc::clone(&catalog_calls),
            attempts: Arc::clone(&catalog_attempts),
        })
        .expect(5)
        .mount(&server)
        .await;

    let mut config = valid_config();
    config.config.api.base_url = server.uri();
    let mut bundle =
        provider_openai::initialize(config.config.clone(), provider_ports_with_accounts(store))
            .await
            .expect("initialize OpenAI provider");
    let provider = bundle.core_provider();
    let daemon = bundle
        .take_worker_contributions()
        .into_iter()
        .find_map(|contribution| match contribution {
            WorkerContribution::Registration(registration)
                if registration.id.owner() == "openai-model-etag" =>
            {
                match registration.runnable {
                    WorkerRunnable::Daemon { task, .. } => Some(task),
                    WorkerRunnable::Scheduled { .. } => panic!("ETag worker must be a daemon"),
                }
            }
            WorkerContribution::Registration(_) | WorkerContribution::Disabled { .. } => None,
        })
        .expect("ETag daemon contribution");
    let cancellation = CancellationToken::new();
    let run_cancellation = cancellation.clone();
    let daemon_handle = tokio::spawn(async move { daemon.run(run_cancellation).await });

    execute_response(&provider, account_id, "req_etag_backoff_first").await;
    wait_for_calls(&catalog_calls, 1).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(catalog_calls.load(Ordering::SeqCst), 1);
    wait_for_calls(&catalog_calls, 2).await;
    wait_for_calls(&catalog_calls, 3).await;

    execute_response(&provider, account_id, "req_etag_backoff_second").await;
    wait_for_calls(&catalog_calls, 4).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(catalog_calls.load(Ordering::SeqCst), 4);
    wait_for_calls(&catalog_calls, 5).await;

    let attempts = catalog_attempts
        .lock()
        .expect("catalog attempt lock")
        .clone();
    assert!(attempts[1].duration_since(attempts[0]) >= Duration::from_millis(900));
    assert!(attempts[2].duration_since(attempts[1]) >= Duration::from_millis(1_900));
    let reset_delay = attempts[4].duration_since(attempts[3]);
    assert!(reset_delay >= Duration::from_millis(900));
    assert!(reset_delay < Duration::from_secs(3));

    cancellation.cancel();
    assert!(
        timeout(Duration::from_secs(1), daemon_handle)
            .await
            .expect("ETag daemon cancels promptly")
            .expect("join ETag daemon")
            .is_ok()
    );
    server.verify().await;
}
