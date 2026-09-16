use provider_openai::credential::token_client::{
    AuthorizationCodeExchangeError, AuthorizationCodeExchanger, AuthorizationCodeGrant,
    OpenAiTokenClient, RefreshFailure, TokenClientConfig, TokenRefresher,
};
use secrecy::{ExposeSecret, SecretString};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn qx_profile() -> provider_openai::transport::profile::CodexWireProfileState {
    let profile = provider_openai::OpenAiConfig::default().wire_profile_state();
    profile
        .apply_user_agent_override(
            &gateway_core::provider_ports::ProviderUserAgentOverride::QxCompatible {
                user_agent: None,
            },
        )
        .expect("QX-compatible profile");
    profile
}

fn native_client(
    endpoint: String,
    profile: provider_openai::transport::profile::CodexWireProfileState,
) -> OpenAiTokenClient {
    OpenAiTokenClient::new(
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        TokenClientConfig {
            client_id: "test-public-client".to_owned(),
            token_endpoint: endpoint,
        },
        profile,
    )
}

fn pat_service(
    client: OpenAiTokenClient,
) -> provider_openai::credential::CodexCredentialAdminService {
    use std::sync::Arc;

    let client = Arc::new(client);
    provider_openai::credential::CodexCredentialAdminService::new(
        client.clone(),
        Arc::new(crate::support::TestLeaseCoordinator::default()),
        crate::support::runtime_policy(),
    )
    .with_personal_access_token_client(client)
}

fn code_grant() -> AuthorizationCodeGrant {
    AuthorizationCodeGrant {
        code: SecretString::from("test-code"),
        code_verifier: SecretString::from("test-verifier"),
    }
}

#[tokio::test]
async fn native_token_routes_keep_explicit_proxies_and_qx_auth_headers() {
    let origin = MockServer::start().await;
    let profile = qx_profile();
    let client = native_client(format!("{}/oauth/token", origin.uri()), profile.clone());
    for _ in 0..2 {
        let proxy = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "access", "refresh_token": "refresh", "id_token": "header.e30.signature"
            })))
            .expect(2)
            .mount(&proxy)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/accounts/v1/user-auth-credential/whoami"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "chatgpt_user_id": "pat-user",
                "chatgpt_account_id": "pat-workspace",
                "chatgpt_plan_type": "team",
                "chatgpt_account_is_fedramp": false
            })))
            .expect(1)
            .mount(&proxy)
            .await;
        let selected = gateway_core::account::OutboundProxy::parse(&proxy.uri()).unwrap();
        client
            .refresh_with_proxy("synthetic-refresh", Some(&selected))
            .await
            .expect("refresh through selected proxy");
        client
            .exchange_with_proxy(code_grant(), Some(&selected))
            .await
            .expect("raw exchange through selected proxy");
        let imported = pat_service(client.clone())
            .prepare_import_document_with_proxy(
                serde_json::json!({"accessToken": "at-synthetic-pat"}),
                Some(&selected),
            )
            .await
            .expect("PAT through selected proxy");
        assert_eq!(
            imported.accounts()[0].account.upstream_user_id(),
            Some("pat-user")
        );
        let requests = proxy.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            3,
            "PAT must not trigger an extra token refresh"
        );
        for index in [0, 1, 2] {
            assert_eq!(
                requests[index].headers["user-agent"],
                profile.snapshot().user_agent()
            );
            assert_eq!(
                requests[index].headers["originator"],
                profile.snapshot().originator
            );
        }
        assert_eq!(requests[0].headers["content-type"], "application/json");
        assert_eq!(
            requests[2].headers["authorization"],
            "Bearer at-synthetic-pat"
        );
        assert_eq!(
            requests[1].headers["content-type"],
            "application/x-www-form-urlencoded"
        );
        for name in ["version", "authorization", "chatgpt-account-id"] {
            assert!(
                !requests[1].headers.contains_key(name),
                "QX auth exchange: {name}"
            );
        }
    }
    assert!(origin.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn native_selection_does_not_infer_private_reqwest_proxy_and_cpr_keeps_original_client() {
    let origin = MockServer::start().await;
    let legacy_proxy = MockServer::start().await;
    for server in [&origin, &legacy_proxy] {
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(server)
            .await;
    }
    let profile = qx_profile();
    let client = OpenAiTokenClient::new(
        reqwest::Client::builder()
            .no_proxy()
            .proxy(reqwest::Proxy::all(legacy_proxy.uri()).unwrap())
            .build()
            .unwrap(),
        TokenClientConfig {
            client_id: "test-public-client".to_owned(),
            token_endpoint: format!("{}/oauth/token", origin.uri()),
        },
        profile.clone(),
    );
    client.refresh("synthetic-refresh").await.unwrap();
    assert_eq!(origin.received_requests().await.unwrap().len(), 1);
    assert!(legacy_proxy.received_requests().await.unwrap().is_empty());
    profile
        .apply_user_agent_override(
            &gateway_core::provider_ports::ProviderUserAgentOverride::Default,
        )
        .unwrap();
    client.refresh("synthetic-refresh").await.unwrap();
    assert_eq!(legacy_proxy.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn native_configuration_failure_is_not_a_retryable_refresh() {
    let origin = MockServer::start().await;
    let client = native_client(
        format!("{}/oauth/token?{}", origin.uri(), "x".repeat(8192)),
        qx_profile(),
    );
    let error = client.refresh("synthetic-refresh").await.unwrap_err();
    assert!(matches!(
        error,
        RefreshFailure::Transport { upstream: None, .. }
    ));
    assert_eq!(
        client
            .exchange_authorization_code(code_grant())
            .await
            .unwrap_err(),
        AuthorizationCodeExchangeError::Unavailable
    );
    assert!(origin.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn native_connect_failure_is_retryable_but_lost_reply_is_ambiguous() {
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/oauth/token", listener.local_addr().unwrap());
    drop(listener);
    let client = native_client(endpoint, qx_profile());
    assert!(matches!(
        client.refresh("synthetic-refresh").await.unwrap_err(),
        RefreshFailure::RetryableTransport { .. }
    ));
    assert_eq!(
        client
            .exchange_authorization_code(code_grant())
            .await
            .unwrap_err(),
        AuthorizationCodeExchangeError::Unavailable
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = native_client(
        format!("http://{}/oauth/token", listener.local_addr().unwrap()),
        qx_profile(),
    );
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_token_request(&mut stream).await;
            drop(stream);
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "a possibly consumed token must not be replayed"
        );
    });
    assert!(matches!(
        client.refresh("synthetic-refresh").await.unwrap_err(),
        RefreshFailure::Transport { upstream: None, .. }
    ));
    assert_eq!(
        client
            .exchange_authorization_code(code_grant())
            .await
            .unwrap_err(),
        AuthorizationCodeExchangeError::Ambiguous
    );
    server.await.unwrap();
}

async fn read_token_request(stream: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncReadExt;

    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).await.unwrap();
        assert_ne!(count, 0, "connection ended before the token request");
        request.extend_from_slice(&buffer[..count]);
        if let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + length {
                return;
            }
        }
    }
}

#[tokio::test]
async fn native_token_timeout_is_shared_by_headers_and_response_body() {
    use std::time::Duration;
    use tokio::{io::AsyncWriteExt, net::TcpListener, sync::oneshot};

    for operation in ["refresh", "exchange", "pat"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let profile = qx_profile();
        let client = native_client(
            format!("http://{}/oauth/token", listener.local_addr().unwrap()),
            profile.clone(),
        );
        let (accepted, received) = oneshot::channel();
        let (send_head, head_allowed) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_token_request(&mut stream).await;
            accepted.send(()).unwrap();
            head_allowed.await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
                .await
                .unwrap();
            std::future::pending::<()>().await;
        });
        let request = tokio::spawn(async move {
            match operation {
                "refresh" => {
                    let error = client.refresh("synthetic-refresh").await.unwrap_err();
                    assert!(matches!(
                        error,
                        RefreshFailure::Transport { upstream: None, .. }
                    ));
                }
                "exchange" => assert_eq!(
                    client
                        .exchange_authorization_code(code_grant())
                        .await
                        .unwrap_err(),
                    AuthorizationCodeExchangeError::Ambiguous
                ),
                "pat" => {
                    let error = pat_service(client)
                        .prepare_import_document(
                            serde_json::json!({"accessToken": "at-synthetic-pat"}),
                        )
                        .await
                        .unwrap_err();
                    assert!(matches!(
                        error,
                        provider_openai::credential::CodexCredentialAdminError::PersonalAccessToken(
                            provider_openai::credential::token_client::PersonalAccessTokenError::InvalidResponse
                        )
                    ));
                }
                _ => unreachable!(),
            }
        });
        received.await.unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(20)).await;
        profile
            .apply_user_agent_override(
                &gateway_core::provider_ports::ProviderUserAgentOverride::Default,
            )
            .unwrap();
        send_head.send(()).unwrap();
        // Keep the paused executor runnable while the loopback header/body arrives.
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(9)).await;
        assert!(
            !request.is_finished(),
            "{operation}: deadline ended too early"
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::time::timeout(Duration::from_secs(1), request)
            .await
            .expect("body must not get a new 30-second budget")
            .unwrap();
        server.abort();
        let _ = server.await;
        tokio::time::resume();
    }
}

#[tokio::test]
async fn refresh_and_code_exchange_use_the_selected_account_proxy() {
    let direct = MockServer::start().await;
    let proxy_a = MockServer::start().await;
    let proxy_b = MockServer::start().await;
    for (proxy, access) in [(&proxy_a, "exit-a-access"), (&proxy_b, "exit-b-access")] {
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": access,
                "refresh_token": "rotated-refresh",
                "id_token": "header.e30.signature"
            })))
            .expect(2)
            .mount(proxy)
            .await;
    }
    let client = client(&direct);
    for (proxy, expected) in [(&proxy_a, "exit-a-access"), (&proxy_b, "exit-b-access")] {
        let proxy = gateway_core::account::OutboundProxy::parse(&proxy.uri()).unwrap();
        let refreshed = client
            .refresh_with_proxy("initial-refresh", Some(&proxy))
            .await
            .unwrap();
        assert_eq!(refreshed.access_token.as_deref(), Some(expected));
        let exchanged = client
            .exchange_with_proxy(
                AuthorizationCodeGrant {
                    code: SecretString::from("test-code"),
                    code_verifier: SecretString::from("test-verifier"),
                },
                Some(&proxy),
            )
            .await
            .unwrap();
        assert_eq!(exchanged.secret.access_token.expose_secret(), expected);
    }
    assert!(direct.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn refresh_tracks_the_online_profile_without_contaminating_code_exchange() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "audit-access", "refresh_token": "audit-refresh", "id_token": "header.e30.signature"
        })))
        .expect(4)
        .mount(&server)
        .await;
    let profile = provider_openai::OpenAiConfig::default().wire_profile_state();
    let client = provider_openai::credential::token_client::openai_token_client(
        TokenClientConfig {
            client_id: "audit-client".to_owned(),
            token_endpoint: format!("{}/oauth/token", server.uri()),
        },
        profile.clone(),
    )
    .expect("production auth transport");
    for (core, desktop, build) in [
        ("1.2.3", "26.800.10000", "8000"),
        ("1.2.4", "26.901.10000", "8109"),
    ] {
        profile.update_bundled_release(
            &provider_openai::transport::profile::CodexBundledReleaseProfile {
                codex_version: core.to_owned(),
                desktop_version: desktop.to_owned(),
                desktop_build: build.to_owned(),
                verified_at: chrono::Utc::now(),
            },
        );
        client.refresh("audit-refresh").await.expect("refresh");
        client
            .exchange_authorization_code(AuthorizationCodeGrant {
                code: SecretString::from("audit-code"),
                code_verifier: SecretString::from("audit-verifier"),
            })
            .await
            .expect("code exchange");
        let requests = server.received_requests().await.expect("requests");
        let refresh = &requests[requests.len() - 2];
        let exchange = &requests[requests.len() - 1];
        assert_eq!(
            refresh.headers["user-agent"],
            profile.snapshot().user_agent()
        );
        assert_eq!(refresh.headers["originator"], "Codex Desktop");
        assert_eq!(refresh.headers["content-type"], "application/json");
        assert_eq!(
            exchange.headers["content-type"],
            "application/x-www-form-urlencoded"
        );
        for name in [
            "originator",
            "user-agent",
            "version",
            "x-openai-internal-codex-residency",
        ] {
            assert!(!exchange.headers.contains_key(name), "raw exchange: {name}");
        }
        for name in ["authorization", "chatgpt-account-id", "version"] {
            assert!(!refresh.headers.contains_key(name), "refresh: {name}");
        }
    }
}

fn client(server: &MockServer) -> OpenAiTokenClient {
    OpenAiTokenClient::new(
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("test HTTP client"),
        TokenClientConfig {
            client_id: "test-public-client".to_owned(),
            token_endpoint: format!("{}/oauth/token", server.uri()),
        },
        provider_openai::OpenAiConfig::default().wire_profile_state(),
    )
}

#[tokio::test]
async fn oversized_chunked_oauth_response_should_fail_closed_and_redact_body() {
    let server = MockServer::start().await;
    let marker = "oauth-secret-response-marker";
    let body = format!(
        "{{\"error\":\"invalid_grant\",\"marker\":\"{marker}\",\"padding\":\"{}\"}}",
        "x".repeat(70 * 1024)
    );
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(
            ResponseTemplate::new(400)
                .insert_header("transfer-encoding", "chunked")
                .set_body_string(body),
        )
        .expect(1)
        .mount(&server)
        .await;

    let failure = client(&server)
        .refresh("refresh-secret-request-marker")
        .await
        .expect_err("oversized response must fail closed before body classification");
    let diagnostic = format!("{failure:?} {failure}");

    assert!(matches!(
        failure,
        RefreshFailure::Transport { upstream: None, .. }
    ));
    assert!(!diagnostic.contains(marker));
    assert!(!diagnostic.contains("refresh-secret-request-marker"));
}

#[tokio::test]
async fn bounded_oauth_response_should_parse_the_official_rotated_token_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-rotated",
            "refresh_token": "refresh-rotated",
            "id_token": "header.e30.signature",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = client(&server)
        .refresh("refresh-initial")
        .await
        .expect("bounded response");

    assert_eq!(tokens.access_token.as_deref(), Some("access-rotated"));
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-rotated"));
    assert_eq!(tokens.id_token.as_deref(), Some("header.e30.signature"));
}

#[tokio::test]
async fn refresh_response_should_reject_a_malformed_rotated_id_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-rotated",
            "id_token": "not-a-jwt"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let failure = client(&server)
        .refresh("refresh-initial")
        .await
        .expect_err("malformed ID token must not replace the stored token set");

    assert!(matches!(
        failure,
        RefreshFailure::Transport { upstream: None, .. }
    ));
}

#[tokio::test]
async fn refresh_response_should_allow_all_rotated_tokens_to_be_omitted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = client(&server)
        .refresh("refresh-initial")
        .await
        .expect("official refresh response fields are optional");

    assert!(tokens.access_token.is_none());
    assert!(tokens.refresh_token.is_none());
    assert!(tokens.id_token.is_none());
}

#[tokio::test]
async fn refresh_response_keeps_the_upstream_rotated_token_verbatim() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-rotated",
            "refresh_token": " ",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = client(&server)
        .refresh("refresh-initial")
        .await
        .expect("upstream response is accepted");

    assert_eq!(tokens.refresh_token.as_deref(), Some(" "));
}

#[tokio::test]
async fn refresh_should_exchange_the_official_json_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "new-access",
            "refresh_token": "new-refresh",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    client(&server)
        .refresh("refresh secret")
        .await
        .expect("refresh succeeds");
    let requests = server.received_requests().await.expect("received request");
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("JSON request body");

    assert_eq!(
        body,
        serde_json::json!({
            "client_id": "test-public-client",
            "grant_type": "refresh_token",
            "refresh_token": "refresh secret"
        })
    );
    assert_eq!(
        requests[0]
            .headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
}

#[tokio::test]
async fn authorization_code_exchange_should_require_bounded_oidc_token_set_and_pkce_form() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "header.access.signature",
            "refresh_token": "refresh-token",
            "id_token": "header.id.signature",
            "token_type": "Bearer",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = client(&server)
        .exchange_authorization_code(AuthorizationCodeGrant {
            code: SecretString::from("authorization code"),
            code_verifier: SecretString::from("pkce-verifier-secret"),
        })
        .await
        .expect("exchange bounded OIDC token set");
    let requests = server.received_requests().await.expect("received request");
    let body = String::from_utf8(requests[0].body.clone()).expect("form body");

    assert_eq!(
        tokens.secret.access_token.expose_secret(),
        "header.access.signature"
    );
    assert_eq!(tokens.id_token.expose_secret(), "header.id.signature");
    assert!(body.contains("grant_type=authorization_code"));
    assert!(body.contains("client_id=test-public-client"));
    assert!(body.contains("code=authorization+code"));
    assert!(body.contains("code_verifier=pkce-verifier-secret"));
    assert!(body.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
}

#[tokio::test]
async fn authorization_code_exchange_keeps_upstream_access_and_refresh_tokens_verbatim() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": " ",
            "refresh_token": "",
            "id_token": "header.payload.signature"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = client(&server)
        .exchange_authorization_code(AuthorizationCodeGrant {
            code: SecretString::from("authorization-code"),
            code_verifier: SecretString::from("pkce-verifier"),
        })
        .await
        .expect("token endpoint response is trusted structurally");

    assert_eq!(tokens.secret.access_token.expose_secret(), " ");
    assert_eq!(
        tokens
            .secret
            .refresh_token
            .as_ref()
            .expect("required response field")
            .expose_secret(),
        ""
    );
}

#[tokio::test]
async fn authorization_code_exchange_requires_id_token_and_json_response() {
    let missing_id = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "header.access.signature",
            "refresh_token": "refresh-token",
            "token_type": "Bearer",
            "expires_in": 3600
        })))
        .mount(&missing_id)
        .await;
    let non_json = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
        .mount(&non_json)
        .await;

    for server in [&missing_id, &non_json] {
        let error = client(server)
            .exchange_authorization_code(AuthorizationCodeGrant {
                code: SecretString::from("code-secret"),
                code_verifier: SecretString::from("verifier-secret"),
            })
            .await
            .expect_err("invalid token response fails closed");
        assert_eq!(error, AuthorizationCodeExchangeError::Rejected);
    }
}

#[tokio::test]
async fn refresh_token_reuse_should_be_classified_as_invalid_grant() {
    let upstream_message = "Your refresh token has already been used to generate a new access token. Please try signing in again.";
    let failure = refresh_failure(
        400,
        r#"{"error":{"message":"Your refresh token has already been used to generate a new access token. Please try signing in again.","type":"invalid_request_error","param":null,"code":"refresh_token_reused"}}"#,
    )
    .await;

    let RefreshFailure::InvalidGrant { message, upstream } = failure else {
        panic!("refresh-token reuse must be terminal");
    };
    assert_eq!(message.as_deref(), Some(upstream_message));
    let upstream = upstream.expect("complete upstream failure");
    assert_eq!(upstream.status(), 400);
    assert_eq!(upstream.code(), Some("refresh_token_reused"));
    assert_eq!(upstream.error_type(), Some("invalid_request_error"));
    assert!(upstream.body().contains(r#""code":"refresh_token_reused""#));
}

#[tokio::test]
async fn generic_invalid_grant_should_remain_transient_like_official_codex() {
    let body = r#"{"error":"invalid_grant","error_description":"not the official refresh error message field"}"#;
    let failure = refresh_failure(400, body).await;
    let diagnostic = format!("{failure:?} {failure}");

    let RefreshFailure::Transport { message, upstream } = failure else {
        panic!("unknown official refresh code must remain transient");
    };
    assert_eq!(message, None);
    let upstream = upstream.expect("complete upstream failure");
    assert_eq!(upstream.code(), Some("invalid_grant"));
    assert_eq!(upstream.body(), body);
    assert!(!diagnostic.contains("error_description"));
}

#[tokio::test]
async fn unauthorized_should_preserve_upstream_details_and_remain_retryable() {
    let body = r#"{
        "error": {
            "message": "Invalid refresh token.",
            "type": "invalid_request_error",
            "param": null,
            "code": "invalid_refresh_token"
        }
    }"#;
    let failure = refresh_failure(401, body).await;

    let RefreshFailure::Transport { message, upstream } = failure else {
        panic!("production policy gives every 401 a bounded recovery window");
    };
    assert_eq!(message.as_deref(), Some("Invalid refresh token."));
    let upstream = upstream.expect("complete upstream failure");
    assert_eq!(upstream.status(), 401);
    assert_eq!(upstream.code(), Some("invalid_refresh_token"));
    assert_eq!(upstream.error_type(), Some("invalid_request_error"));
    assert_eq!(upstream.body(), body);
}

#[tokio::test]
async fn unauthorized_should_back_off_even_with_a_recognized_refresh_code() {
    let failure = refresh_failure(
        401,
        r#"{"error":{"code":"refresh_token_expired","message":"Refresh token expired."}}"#,
    )
    .await;

    assert_transport_failure(
        &failure,
        401,
        Some("Refresh token expired."),
        r#"{"error":{"code":"refresh_token_expired","message":"Refresh token expired."}}"#,
    );
}

#[tokio::test]
async fn deactivated_account_should_be_classified_as_banned() {
    let failure = refresh_failure(
        403,
        r#"{"error":{"code":"access_denied","message":"account has been deactivated"}}"#,
    )
    .await;

    let RefreshFailure::Banned { message, upstream } = failure else {
        panic!("deactivated account must be banned");
    };
    assert_eq!(message.as_deref(), Some("account has been deactivated"));
    assert_eq!(upstream.expect("complete upstream failure").status(), 403);
}

#[tokio::test]
async fn generic_banned_text_should_not_impersonate_the_deactivation_contract() {
    let failure = refresh_failure(403, "account is banned").await;

    assert_transport_failure(&failure, 403, None, "account is banned");
}

#[tokio::test]
async fn unregistered_disabled_account_text_should_remain_a_transport_failure() {
    let failure = refresh_failure(400, "account disabled").await;

    assert_transport_failure(&failure, 400, None, "account disabled");
}

#[tokio::test]
async fn quota_text_should_not_disable_the_oauth_credential() {
    let failure = refresh_failure(400, "quota exceeded").await;

    assert_transport_failure(&failure, 400, None, "quota exceeded");
}

#[tokio::test]
async fn token_revoked_text_without_invalid_grant_should_remain_temporary() {
    let failure = refresh_failure(400, "token_revoked").await;

    assert_transport_failure(&failure, 400, None, "token_revoked");
}

#[tokio::test]
async fn recognized_refresh_code_should_be_permanent_on_non_unauthorized_status() {
    for status in [500, 502, 503, 429] {
        let failure = refresh_failure(
            status,
            r#"{"error":{"code":"refresh_token_expired","message":"Refresh token expired."}}"#,
        )
        .await;
        assert!(
            matches!(failure, RefreshFailure::InvalidGrant { .. }),
            "official refresh code must take precedence for status {status}"
        );
    }
}

#[tokio::test]
async fn unknown_server_error_or_rate_limit_should_remain_transient() {
    let body = r#"{"error":{"code":"temporarily_unavailable","message":"Try again."}}"#;
    for status in [500, 502, 503, 429] {
        let failure = refresh_failure(status, body).await;
        assert_transport_failure(&failure, status, Some("Try again."), body);
    }
}

fn assert_transport_failure(
    failure: &RefreshFailure,
    status: u16,
    message: Option<&str>,
    body: &str,
) {
    let RefreshFailure::Transport {
        message: actual_message,
        upstream,
    } = failure
    else {
        panic!("status {status} must classify as transient");
    };
    assert_eq!(actual_message.as_deref(), message);
    let upstream = upstream.as_deref().expect("complete upstream failure");
    assert_eq!(upstream.status(), status);
    assert_eq!(upstream.body(), body);
}

async fn refresh_failure(status: u16, body: &str) -> RefreshFailure {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .expect(1)
        .mount(&server)
        .await;

    client(&server)
        .refresh("refresh-secret")
        .await
        .expect_err("refresh must fail")
}
