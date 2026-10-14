use super::*;
use gateway_core::engine::connection::ConnectionBudget;
use std::{cell::Cell, time::Instant};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

thread_local! { static NOW: Cell<Instant> = Cell::new(Instant::now()); }
fn now() -> Instant {
    NOW.with(Cell::get)
}

fn test_client(base: String, budget: ConnectionBudget) -> CodexBackendClient {
    let profile = super::super::profile::CodexWireProfile::parse_user_agent(
        "Codex Desktop/0.153.4 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.901.51231)",
    )
    .unwrap();
    CodexBackendClient::new(
        build_reqwest_client().unwrap(),
        base,
        CodexWireProfileState::new(profile),
    )
    .with_default_http_client()
    .with_connection_budget(budget)
}

#[tokio::test]
async fn shortened_connection_budget_preserves_injected_client_configuration() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let budget = ConnectionBudget::with_clock(now);
    budget.begin().unwrap();
    NOW.with(|clock| clock.set(clock.get() + Duration::from_secs(29)));
    let profile = test_client(base.clone(), budget.clone()).profile;
    let client = CodexBackendClient::new(
        Client::builder()
            .no_proxy()
            .user_agent("synthetic-injected-client")
            .build()
            .unwrap(),
        base.clone(),
        profile,
    )
    .with_connection_budget(budget);
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            request.push(socket.read_u8().await.unwrap());
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        String::from_utf8(request).unwrap()
    });
    tokio::time::timeout(
        Duration::from_secs(5),
        client.send_profiled(&client.profile.snapshot(), false, |http| http.get(base)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        server
            .await
            .unwrap()
            .contains("user-agent: synthetic-injected-client\r\n")
    );
}

#[tokio::test]
async fn subsecond_connection_budget_does_not_limit_response_duration() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let budget = ConnectionBudget::with_clock(now);
    budget.begin().unwrap();
    NOW.with(|clock| clock.set(clock.get() + Duration::from_millis(29_500)));
    let client = test_client(base.clone(), budget.clone());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let byte = socket.read_u8().await.unwrap();
            request.push(byte);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        // Both response headers and body may arrive after the opening deadline.
        tokio::time::sleep(Duration::from_millis(650)).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(650)).await;
        socket.write_all(b"ok").await.unwrap();
    });
    let response = tokio::time::timeout(Duration::from_secs(5), async {
        client
            .send_profiled(&client.profile.snapshot(), false, |http| http.get(base))
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(response, "ok");
    assert!(budget.retry_delay(0, "req_connection").is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn exhausted_connection_budget_never_opens_http_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let budget = ConnectionBudget::default();
    for _ in 0..4 {
        budget.begin().unwrap();
    }
    let client = test_client(base, budget);
    let result = client
        .send_profiled(&client.profile.snapshot(), false, |_| {
            panic!("exhausted budget must not build or send a request")
        })
        .await;
    assert!(matches!(
        result,
        Err(CodexClientError::ConnectionBudgetExhausted)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn shortened_connection_budget_preserves_account_proxy_and_headers() {
    use gateway_core::{
        account::{CredentialRevision, OutboundProxy, ProviderAccount, ProviderAccountId},
        routing::ProviderKind,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let account = ProviderAccount::new(
        ProviderAccountId::new("acct_connection_proxy").unwrap(),
        ProviderKind::new("openai").unwrap(),
        "synthetic".to_owned(),
        None,
        "oauth".to_owned(),
        CredentialRevision::new(1).unwrap(),
        None,
    )
    .with_outbound_proxy(Some(OutboundProxy::parse(&proxy).unwrap()));
    let budget = ConnectionBudget::with_clock(now);
    budget.begin().unwrap();
    NOW.with(|clock| clock.set(clock.get() + Duration::from_secs(29)));
    let client = test_client("http://upstream.invalid".to_owned(), budget)
        .for_account(&account)
        .unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            request.push(socket.read_u8().await.unwrap());
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
        String::from_utf8(request).unwrap()
    });
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        client.send_profiled(&client.profile.snapshot(), false, |http| {
            http.get("http://upstream.invalid/responses")
                .header("chatgpt-account-id", "synthetic-account")
                .header("user-agent", client.profile.snapshot().user_agent())
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let request = server.await.unwrap();
    assert!(request.starts_with("GET http://upstream.invalid/responses HTTP/1.1\r\n"));
    assert!(request.contains("chatgpt-account-id: synthetic-account\r\n"));
    assert!(request.contains(&format!(
        "user-agent: {}\r\n",
        client.profile.snapshot().user_agent()
    )));
}
