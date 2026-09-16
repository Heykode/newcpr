use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gateway_core::operation::{GenerateRequest, ProtocolPayload};
use gateway_core::provider_ports::{
    ProviderSessionPolicy, ProviderTlsProfile, ProviderUserAgentOverride,
};
use provider_openai::encode_generate_request;
use provider_openai::transport::qx_application::project_response_request;

fn compression_context() -> CodexRequestContext<'static> {
    CodexRequestContext {
        installation_id: Some("85c64ae8-4b44-4f82-9696-909adef2144c"),
        session_id: Some("original-session"),
        thread_id: Some("original-thread"),
        ..request_context("compression-fixture", Some("selected-workspace"))
    }
}

fn expected_http_body(request: &CodexResponsesRequest, qx: bool) -> Vec<u8> {
    let mut body = if qx {
        project_response_request(request, compression_context())
            .body()
            .clone()
    } else {
        request.body().clone()
    };
    body.insert("stream".to_owned(), json!(true));
    body.insert("store".to_owned(), json!(false));
    serde_json::to_vec(&body).unwrap()
}

fn compression_request(body: Map<String, Value>) -> CodexResponsesRequest {
    let payload = ProtocolPayload::json_object("openai", body)
        .unwrap()
        .with_context(Map::from_iter([(
            "opaque_request_headers".to_owned(),
            json!([["content-encoding", STANDARD.encode(b"gzip")]]),
        )]));
    let mut request = encode_generate_request(
        &GenerateRequest::from_protocol_payload(payload),
        "gpt-test",
        &Default::default(),
    )
    .unwrap();
    request.force_http_sse = true;
    request.client_api_key_id = Some("trusted-downstream-key".to_owned());
    request.client_conversation_id = Some("original-conversation".to_owned());
    request
}

#[tokio::test]
async fn http_compression_threshold_preserves_identity_and_bytes_across_profiles() {
    for tls_profile in [ProviderTlsProfile::Cpr, ProviderTlsProfile::QxCompatible] {
        for qx in [false, true] {
            for size in [1023, 1024, 1025, 8192] {
                let mut body =
                    json!({
                        "model": "gpt-test",
                        "instructions": "\u{4f60}\u{597d}",
                        "input": [{"role": "user", "content": "preserve this message"}],
                        "tools": [{"type": "function", "name": "test_tool", "parameters": {"type": "object"}}],
                        "stream": false,
                        "prompt_cache_key": "original-cache",
                        "client_metadata": {"session_id": "original-session", "opaque": {"keep": true}}
                    })
                    .as_object()
                    .unwrap()
                    .clone();
                let request = compression_request(body.clone());
                let padding = size - expected_http_body(&request, qx).len();
                body.insert(
                    "instructions".to_owned(),
                    json!(format!("\u{4f60}\u{597d}{}", "x".repeat(padding))),
                );
                let request = compression_request(body);
                let expected = expected_http_body(&request, qx);
                assert_eq!(expected.len(), size);
                let original = request.body().clone();

                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let server = tokio::spawn(async move {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let raw = read_http_request_with_body(&mut stream).await;
                    write_completed_sse_response(&mut stream).await;
                    raw
                });
                let profile = test_wire_profile();
                profile
                    .apply_user_agent_override(&ProviderUserAgentOverride::Independent {
                        user_agent: None,
                        tls_profile,
                        session_policy: if qx {
                            ProviderSessionPolicy::QxCompatible
                        } else {
                            ProviderSessionPolicy::Native
                        },
                    })
                    .unwrap();
                let user_agent = profile.snapshot().user_agent();
                let client = CodexBackendClient::new(
                    reqwest::Client::builder().no_proxy().build().unwrap(),
                    format!("http://{address}"),
                    profile,
                );
                let response = timeout(
                    Duration::from_secs(5),
                    client.create_response(&request, compression_context()),
                )
                .await
                .expect("bounded threshold response")
                .expect("valid HTTP response");
                assert_eq!(response.transport, CodexBackendTransport::HttpSse);
                assert_eq!(request.body(), &original);

                let raw = server.await.unwrap();
                let separator = raw.windows(4).position(|part| part == b"\r\n\r\n").unwrap();
                let head = std::str::from_utf8(&raw[..separator]).unwrap();
                let sent = &raw[separator + 4..];
                let compressed = size >= 1024;
                assert_eq!(
                    read_header_value(head, "content-encoding"),
                    compressed.then_some("zstd"),
                    "TLS={tls_profile:?}, QX={qx}, size={size}",
                );
                assert_eq!(
                    read_header_value(head, "content-length")
                        .unwrap()
                        .parse::<usize>()
                        .unwrap(),
                    sent.len(),
                );
                let decoded = if compressed {
                    zstd::stream::decode_all(sent).expect("valid ZSTD frame")
                } else {
                    sent.to_vec()
                };
                assert_eq!(decoded, expected, "only the content encoding may change");
                assert_eq!(
                    read_header_value(head, "user-agent"),
                    Some(user_agent.as_str())
                );
                assert_eq!(
                    read_header_value(head, "x-codex-installation-id"),
                    compression_context().installation_id,
                );
                assert_eq!(
                    read_header_value(head, "chatgpt-account-id"),
                    compression_context().account_id,
                );
                let body: Value = serde_json::from_slice(&decoded).unwrap();
                if qx {
                    assert_eq!(
                        body["prompt_cache_key"].as_str(),
                        read_header_value(head, "thread-id"),
                    );
                    assert_ne!(body["prompt_cache_key"], "original-cache");
                } else {
                    assert_eq!(body["prompt_cache_key"], "original-cache");
                }
            }
        }
    }
}

#[test]
fn http_compression_threshold_cost_probe() {
    use std::hint::black_box;

    for size in [128, 256, 512, 1024, 2048, 8192, 65536] {
        for repetitive in [false, true] {
            let mut state = 0x1234_5678_u32;
            let text = (0..size)
                .map(|index| {
                    if repetitive {
                        b"synthetic response history "[index % 27] as char
                    } else {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        (b'!' + (state % 90) as u8) as char
                    }
                })
                .collect::<String>();
            let body = serde_json::to_vec(&json!({"model":"fixture","input":text})).unwrap();
            let compressed = zstd::stream::encode_all(body.as_slice(), 3).unwrap();
            assert_eq!(
                zstd::stream::decode_all(compressed.as_slice()).unwrap(),
                body
            );
            let started = Instant::now();
            for _ in 0..100 {
                black_box(zstd::stream::encode_all(black_box(body.as_slice()), 3).unwrap());
            }
            println!(
                "compression_probe {}",
                json!({
                    "repetitive": repetitive,
                    "input_bytes": body.len(),
                    "compressed_bytes": compressed.len(),
                    "mean_compress_us": started.elapsed().as_micros() as f64 / 100.0,
                })
            );
        }
    }
}
