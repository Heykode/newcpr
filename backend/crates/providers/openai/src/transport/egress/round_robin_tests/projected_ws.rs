use super::*;
use crate::transport::{
    CodexClientError, protocol::responses::PreviousResponseScope,
    websocket::PreviousResponseUnavailableReason,
};
use gateway_core::lifecycle::CancellationToken;
use serde_json::Value;
use tungstenite::{
    Message,
    handshake::server::{Callback, ErrorResponse, Request, Response},
    protocol::WebSocketConfig,
};

const EXCLUDE: &str = "x-codex-exclude-stream-fields";

struct Opening(Option<&'static str>);
impl Callback for Opening {
    fn on_request(
        self,
        request: &Request,
        mut response: Response,
    ) -> Result<Response, ErrorResponse> {
        assert_eq!(
            request.headers().get(EXCLUDE).map(|v| v.to_str().unwrap()),
            self.0
        );
        response
            .headers_mut()
            .insert("x-models-etag", "opening".parse().unwrap());
        Ok(response)
    }
}

async fn server(listener: TcpListener, opening: Opening, expected_generate: &[bool]) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut config = WebSocketConfig::default();
    config.extensions.permessage_deflate = Some(Default::default());
    let mut socket = tokio_tungstenite::accept_hdr_async_with_config(stream, opening, Some(config))
        .await
        .unwrap();
    for (round, &generate) in expected_generate.iter().enumerate() {
        let text = socket.next().await.unwrap().unwrap().into_text().unwrap();
        let frame: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            frame
                .get("generate")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            generate
        );
        socket
            .send(Message::Text(
                completed(&format!("resp_projection_{round}")).into(),
            ))
            .await
            .unwrap();
    }
    while let Some(Ok(message)) = socket.next().await {
        if message.is_close() {
            break;
        }
    }
}

fn policy(
    client: &CodexBackendClient,
    account: &ProviderAccount,
    scope: &str,
    selector: &str,
    value: Value,
) -> CodexBackendClient {
    let step = json!({
        "scope": scope,
        "selector": selector,
        "action": "set_value",
        "value": value,
        "replacement": "",
        "pattern": null,
        "replaceAll": true,
        "caseInsensitive": false,
        "multiLine": false,
    });
    let config = serde_json::from_value(json!({
        "enabled": true,
        "onError": "reject_request",
        "rules": [{
            "id": "projection",
            "name": "projection",
            "enabled": true,
            "steps": [step],
        }],
    }))
    .unwrap();
    client.clone().with_account_privacy(
        Some(crate::transport::privacy::compile(&config).unwrap()),
        CancellationToken::new(),
        account,
        None,
    )
}

#[tokio::test]
async fn projected_generate_controls_handshake_etag() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pool = Arc::new(CodexWebSocketPool::new(Duration::from_secs(60)));
    let client = CodexBackendClient::new(
        Client::builder().no_proxy().build().unwrap(),
        format!("http://{}", listener.local_addr().unwrap()),
        OpenAiConfig::default().wire_profile_state(),
    )
    .with_websocket_pool(pool.clone());
    let task = tokio::spawn(server(listener, Opening(None), &[false, true, true]));
    let account = account(0);
    for (input, wire, expected_etag) in [
        (true, false, None),
        (false, true, Some("opening")),
        (false, true, None),
    ] {
        let mut request = managed_proxy_request(false);
        request.body_mut().insert("generate".into(), json!(input));
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            policy(&client, &account, "request_body", "$.generate", json!(wire))
                .create_response_stream_with_pool_account(
                    &request,
                    CodexRequestContext::auxiliary("Bearer synthetic", None, "etag", None),
                    Some(account.id().as_str()),
                ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            response.response_metadata.models_etag.as_deref(),
            expected_etag
        );
        response.body.try_collect::<Vec<_>>().await.unwrap();
    }
    pool.shutdown().await;
    task.await.unwrap();
}

#[tokio::test]
async fn projected_exclusions_preserve_exact_owner_on_fresh_egress() {
    let listener = TcpListener::bind("[::1]:0").await.unwrap();
    let (_, runtime, accounts) = fixture(&[Ipv6Addr::LOCALHOST], EgressMode::RandomIpv6Fresh).await;
    let account = &accounts[0];
    let pool = Arc::new(CodexWebSocketPool::new(Duration::from_secs(60)));
    let client = CodexBackendClient::new(
        Client::builder().no_proxy().build().unwrap(),
        format!("http://{}", listener.local_addr().unwrap()),
        OpenAiConfig::default().wire_profile_state(),
    )
    .with_egress_runtime(runtime)
    .with_websocket_pool(pool.clone())
    .for_account(account)
    .unwrap();
    let task = tokio::spawn(server(listener, Opening(Some("output")), &[true, true]));
    let mut request = managed_proxy_request(false);
    request
        .passthrough_headers
        .insert(EXCLUDE, "original".parse().unwrap());
    let context = CodexRequestContext::auxiliary("Bearer synthetic", None, "exclusion", None);
    for (value, expected) in [
        ("output", Some("new")),
        ("other", None),
        ("output", Some("reuse")),
    ] {
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            policy(&client, account, "request_header", EXCLUDE, json!(value))
                .create_response_stream_with_pool_account(
                    &request,
                    context,
                    Some(account.id().as_str()),
                ),
        )
        .await
        .unwrap();
        if let Some(expected) = expected {
            let response = result.unwrap();
            assert_eq!(response.websocket_pool_decision.unwrap().kind(), expected);
            response.body.try_collect::<Vec<_>>().await.unwrap();
        } else {
            assert!(matches!(
                result,
                Err(CodexClientError::WebSocket(
                    CodexWebSocketExchangeError::ContinuationUnavailable {
                        reason: PreviousResponseUnavailableReason::FreshConnectionRequired,
                        ..
                    }
                ))
            ));
        }
        request.set_previous_response_id(Some("resp_projection_0".into()));
        request.previous_response_scope = Some(PreviousResponseScope::ConnectionLocal);
    }
    pool.shutdown().await;
    task.await.unwrap();
}
