use super::*;
use std::collections::BTreeMap;

use futures::{TryStreamExt, future::BoxFuture};
use gateway_core::{
    account::CredentialRevision,
    provider_ports::{ProviderStoreError, egress::ProviderEgressAddress},
    routing::ProviderKind,
};

struct Store(Mutex<ProviderEgressConfig>);

impl ProviderEgressStorePort for Store {
    fn load(&self) -> BoxFuture<'_, Result<Arc<ProviderEgressConfig>, ProviderStoreError>> {
        Box::pin(async { Ok(Arc::new(self.0.lock().unwrap().clone())) })
    }

    fn ensure_fixed_affinity(
        &self,
        _: &ProviderAccountId,
    ) -> BoxFuture<'_, Result<Option<Ipv6Addr>, ProviderStoreError>> {
        Box::pin(async { panic!("routing must not allocate fixed affinity") })
    }
}

fn account(id: usize) -> ProviderAccount {
    ProviderAccount::new(
        ProviderAccountId::new(format!("acct_round_robin_{id}")).unwrap(),
        ProviderKind::new("openai").unwrap(),
        format!("round-robin-{id}"),
        Some(format!("round-robin-{id}")),
        "oauth".to_owned(),
        CredentialRevision::new(1).unwrap(),
        None,
    )
}

fn addresses() -> [Ipv6Addr; 4] {
    [1, 2, 3, 4].map(|suffix| Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, suffix))
}

async fn fixture(
    sources: &[Ipv6Addr],
    mode: EgressMode,
) -> (Arc<Store>, Arc<CodexEgressRuntime>, [ProviderAccount; 3]) {
    let accounts = [account(0), account(1), account(2)];
    let store = Arc::new(Store(Mutex::new(ProviderEgressConfig {
        revision: 1,
        default_mode: mode,
        addresses: sources
            .iter()
            .enumerate()
            .map(|(index, address)| ProviderEgressAddress {
                id: format!("source-{index}"),
                address: *address,
                enabled: true,
            })
            .collect(),
        account_overrides: accounts.iter().map(|a| (a.id().clone(), None)).collect(),
        fixed_bindings: accounts
            .iter()
            .filter_map(|a| sources.first().map(|source| (a.id().clone(), *source)))
            .collect(),
    })));
    let runtime = CodexEgressRuntime::load(store.clone()).await.unwrap();
    (store, runtime, accounts)
}

fn selected(runtime: &CodexEgressRuntime, account: &ProviderAccount) -> Ipv6Addr {
    runtime.select(account).unwrap().unwrap().source
}

#[tokio::test]
async fn rotating_modes_and_accounts_share_one_cursor_across_reload() {
    let sources = addresses();
    let (store, runtime, accounts) = fixture(&sources, EgressMode::RandomIpv6Reuse).await;
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
    {
        let mut state = store.0.lock().unwrap();
        state.revision += 1;
        state
            .account_overrides
            .insert(accounts[1].id().clone(), Some(EgressMode::RandomIpv6Fresh));
    }
    runtime.reload().await.unwrap();
    for (account, source) in [
        (&accounts[1], sources[1]),
        (&accounts[2], sources[2]),
        (&accounts[0], sources[3]),
        (&accounts[2], sources[0]),
        (&accounts[1], sources[1]),
    ] {
        assert_eq!(selected(&runtime, account), source);
    }
}

#[tokio::test]
async fn parallel_accounts_consume_each_source_once_per_cycle() {
    let sources = addresses();
    let (_, runtime, accounts) = fixture(&sources, EgressMode::RandomIpv6Reuse).await;
    let barrier = Arc::new(std::sync::Barrier::new(32));
    let workers: Vec<_> = (0..32)
        .map(|index| {
            let runtime = runtime.clone();
            let account = accounts[index % accounts.len()].clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                (0..8)
                    .map(|_| selected(&runtime, &account))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut counts = BTreeMap::new();
    for source in workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
    {
        *counts.entry(source).or_insert(0) += 1;
    }
    assert_eq!(
        counts,
        sources.into_iter().map(|source| (source, 64)).collect()
    );
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
}

#[tokio::test]
async fn disabled_and_cooling_sources_are_skipped_without_reindexing_the_cursor() {
    let sources = addresses();
    let (store, runtime, accounts) = fixture(&sources, EgressMode::RandomIpv6Reuse).await;
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
    store.0.lock().unwrap().addresses[0].enabled = false;
    runtime.reload().await.unwrap();
    runtime.source_health.lock().unwrap().insert(
        sources[2],
        SourceHealth {
            available_until: None,
            blocked_until: Some(Instant::now() + Duration::from_secs(60)),
        },
    );
    assert_eq!(selected(&runtime, &accounts[1]), sources[1]);
    assert_eq!(selected(&runtime, &accounts[2]), sources[3]);
    assert_eq!(selected(&runtime, &accounts[0]), sources[1]);
    runtime
        .source_health
        .lock()
        .unwrap()
        .get_mut(&sources[2])
        .unwrap()
        .blocked_until = None;
    assert_eq!(selected(&runtime, &accounts[1]), sources[2]);
}

#[tokio::test]
async fn fixed_default_and_invalid_accounts_do_not_advance_rotation() {
    let sources = addresses();
    let (store, runtime, accounts) = fixture(&sources, EgressMode::RandomIpv6Reuse).await;
    {
        let mut state = store.0.lock().unwrap();
        state
            .account_overrides
            .insert(accounts[1].id().clone(), Some(EgressMode::FixedIpv6Reuse));
        state
            .account_overrides
            .insert(accounts[2].id().clone(), Some(EgressMode::Unchanged));
    }
    runtime.reload().await.unwrap();
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
    assert_eq!(selected(&runtime, &accounts[1]), sources[0]);
    assert!(runtime.select(&accounts[2]).unwrap().is_none());
    assert_eq!(
        runtime.select(&account(99)),
        Err(CodexEgressError::ConfigurationChanged)
    );
    let proxied = accounts[0].clone().with_outbound_proxy(Some(
        gateway_core::account::OutboundProxy::parse("http://127.0.0.1:9").unwrap(),
    ));
    assert_eq!(
        runtime.select(&proxied),
        Err(CodexEgressError::ProxyConflict)
    );
    assert_eq!(selected(&runtime, &accounts[0]), sources[1]);
}

#[tokio::test]
async fn empty_single_source_and_all_cooling_pools_keep_existing_error_semantics() {
    let sources = addresses();
    let (store, runtime, accounts) = fixture(&[], EgressMode::RandomIpv6Fresh).await;
    assert_eq!(
        runtime.select(&accounts[0]),
        Err(CodexEgressError::EmptyPool)
    );
    store
        .0
        .lock()
        .unwrap()
        .addresses
        .push(ProviderEgressAddress {
            id: "only".into(),
            address: sources[0],
            enabled: true,
        });
    runtime.reload().await.unwrap();
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
    assert_eq!(selected(&runtime, &accounts[1]), sources[0]);
    runtime.source_health.lock().unwrap().insert(
        sources[0],
        SourceHealth {
            available_until: None,
            blocked_until: Some(Instant::now() + Duration::from_secs(60)),
        },
    );
    assert_eq!(selected(&runtime, &accounts[2]), sources[0]);
    assert_eq!(
        runtime.ensure_source_ready(sources[0]),
        Err(CodexEgressError::SourceUnavailable)
    );
    store.0.lock().unwrap().addresses[0].enabled = false;
    runtime.reload().await.unwrap();
    assert_eq!(
        runtime.select(&accounts[0]),
        Err(CodexEgressError::EmptyPool)
    );
}

#[tokio::test]
async fn reordered_pool_follows_last_address_and_removed_cursor_starts_at_front() {
    let sources = addresses();
    let (store, runtime, accounts) = fixture(&sources, EgressMode::RandomIpv6Reuse).await;
    assert_eq!(selected(&runtime, &accounts[0]), sources[0]);
    store.0.lock().unwrap().addresses.reverse();
    runtime.reload().await.unwrap();
    assert_eq!(selected(&runtime, &accounts[1]), sources[3]);
    store.0.lock().unwrap().addresses.remove(0);
    runtime.reload().await.unwrap();
    assert_eq!(selected(&runtime, &accounts[2]), sources[2]);
}

#[tokio::test]
async fn excel_ipv6_http_sse_reuses_connections_but_isolates_accounts() {
    use crate::{
        config::OpenAiConfig,
        transport::{
            CodexBackendClient, CodexBackendTransport, CodexRequestContext,
            excel::{ClientTools, ExcelPreparedRequest, RESPONSES_PATH, prepare_request},
            protocol::responses::CodexResponsesRequest,
        },
    };
    use serde_json::json;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::timeout,
    };

    async fn reply(stream: &mut TcpStream) {
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = stream.read(&mut buffer).await.unwrap();
            assert_ne!(
                count, 0,
                "expected another HTTP request on the selected connection"
            );
            bytes.extend_from_slice(&buffer[..count]);
            assert!(bytes.len() < 64 * 1024);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let head = std::str::from_utf8(&bytes[..end]).unwrap();
                assert!(head.starts_with("POST /basispoints/api/responses HTTP/1.1"));
                assert!(!head.to_ascii_lowercase().contains("upgrade: websocket"));
                let length: usize = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let body = "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_ipv6_fixture\",\"status\":\"completed\",\"output\":[]}}\n\n";
        stream.write_all(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}", body.len(),
        ).as_bytes()).await.unwrap();
    }

    for mode in [
        EgressMode::FixedIpv6Reuse,
        EgressMode::RandomIpv6Reuse,
        EgressMode::FixedIpv6Fresh,
        EgressMode::RandomIpv6Fresh,
    ] {
        let listener = TcpListener::bind("[::1]:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (_, runtime, accounts) = fixture(&[Ipv6Addr::LOCALHOST], mode).await;
        let server = tokio::spawn(async move {
            let (mut first, peer) = listener.accept().await.unwrap();
            assert_eq!(peer.ip(), IpAddr::V6(Ipv6Addr::LOCALHOST));
            reply(&mut first).await;
            if mode.is_fresh() {
                let (mut second, _) = listener.accept().await.unwrap();
                reply(&mut second).await;
            } else {
                reply(&mut first).await;
            }
            // Account B must open its own pool even though its source is also ::1.
            let (mut other, peer) = listener.accept().await.unwrap();
            assert_eq!(peer.ip(), IpAddr::V6(Ipv6Addr::LOCALHOST));
            reply(&mut other).await;
        });
        let client = CodexBackendClient::new(
            Client::builder().no_proxy().build().unwrap(),
            &base,
            OpenAiConfig::default().wire_profile_state(),
        )
        .with_egress_runtime(runtime);
        let mut request = CodexResponsesRequest::from_body(
            json!({"model":"gpt-5.6-sol","input":"fixture","stream":false})
                .as_object()
                .unwrap()
                .clone(),
        );
        // Excel must still choose HTTP even when downstream originally asked for WS.
        request.use_websocket = true;
        let tools = ClientTools::default();
        request.excel = Some(ExcelPreparedRequest {
            body: prepare_request(request.body(), &tools, &BTreeMap::new(), None).unwrap(),
            tools,
            structured: None,
            _image_lease: None,
            image_limits: Default::default(),
            completed: Default::default(),
            usage: Default::default(),
            replay: None,
            endpoint: format!("{base}{RESPONSES_PATH}"),
        });
        for account in [&accounts[0], &accounts[0], &accounts[1]] {
            let response = timeout(
                Duration::from_secs(5),
                client
                    .for_account(account)
                    .unwrap()
                    .create_response_stream_with_pool_account(
                        &request,
                        CodexRequestContext::auxiliary(
                            "Bearer fixture",
                            Some("workspace"),
                            "ipv6-reuse",
                            None,
                        ),
                        Some(account.id().as_str()),
                    ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response.transport, CodexBackendTransport::HttpSse);
            let body = timeout(
                Duration::from_secs(5),
                response.body.try_collect::<Vec<_>>(),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(
                String::from_utf8(body.concat())
                    .unwrap()
                    .contains("resp_ipv6_fixture")
            );
        }
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}
