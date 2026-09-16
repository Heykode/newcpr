use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::{Decompress, FlushDecompress};
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

use super::*;

#[derive(Debug)]
struct ClientFrame {
    first_byte: u8,
    masked: bool,
    mask_len: usize,
    payload: Vec<u8>,
}

#[tokio::test]
async fn official_websocket_wire_fingerprint_should_remain_aligned() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fingerprint server");
    let address = listener.local_addr().expect("fingerprint server address");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept fingerprint client");
        let opening = read_http_request(&mut stream).await;
        let websocket_key = read_header_value(&opening, "sec-websocket-key")
            .expect("WebSocket key")
            .to_owned();
        let response = format!(
            concat!(
                "HTTP/1.1 101 Switching Protocols\r\n",
                "Connection: Upgrade\r\n",
                "Upgrade: websocket\r\n",
                "Sec-WebSocket-Accept: {}\r\n",
                "Sec-WebSocket-Extensions: permessage-deflate\r\n",
                "\r\n"
            ),
            derive_accept_key(websocket_key.as_bytes())
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("write upgrade response");

        let frame = read_client_frame(&mut stream).await;
        write_server_text_frame(
            &mut stream,
            &completed_websocket_response("resp_wire_fingerprint", 1, 1),
        )
        .await;
        (opening, websocket_key, frame)
    });

    let mut request = CodexResponsesRequest::from_body(
        json!({
            "model": "gpt-5.5",
            "instructions": "be brief",
            "input": [],
            "tools": [],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "reasoning": {"effort": "medium", "summary": "auto"},
            "store": false,
            "stream": true,
            "include": ["reasoning.encrypted_content"],
            "prompt_cache_key": "conversation-fingerprint",
            "generate": false,
            "client_metadata": {"request_kind": "prewarm"}
        })
        .as_object()
        .expect("request object")
        .clone(),
    );
    request.use_websocket = true;
    request.local_conversation_id = Some("conversation-fingerprint".to_owned());
    request.turn_metadata = Some(r#"{"kind":"fingerprint"}"#.to_owned());
    request.beta_features = Some("feature-a".to_owned());
    request.version = Some("26.908.40834".to_owned());
    request.codex_window_id = Some("window-fingerprint".to_owned());
    let client = CodexBackendClient::new(
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("HTTP client"),
        format!("http://{address}/backend-api"),
        test_wire_profile(),
    )
    .with_websocket_pool(Arc::new(CodexWebSocketPool::new(Duration::from_mins(1))));

    let result = client
        .create_response(
            &request,
            CodexRequestContext {
                trace: None,
                turn_metadata: request.turn_metadata.as_deref(),
                beta_features: request.beta_features.as_deref(),
                version: request.version.as_deref(),
                codex_window_id: request.codex_window_id.as_deref(),
                session_id: Some("session-fingerprint"),
                thread_id: Some("thread-fingerprint"),
                client_request_id: Some("request-fingerprint"),
                ..request_context("req_wire_fingerprint", Some("account-fingerprint"))
            },
        )
        .await;

    result.unwrap_or_else(|error| panic!("fingerprint WebSocket response: {error}"));
    let (opening, websocket_key, frame) = server.await.expect("fingerprint server task");
    assert_eq!(
        opening.lines().next(),
        Some("GET /backend-api/codex/responses HTTP/1.1")
    );
    // Native WS ordering after the unified QX session header projection.
    assert_eq!(
        read_header_names(&opening),
        vec![
            "host",
            "connection",
            "upgrade",
            "sec-websocket-version",
            "sec-websocket-key",
            "session_id",
            "chatgpt-account-id",
            "authorization",
            "user-agent",
            "originator",
            "version",
            "x-codex-beta-features",
            "x-client-request-id",
            "session-id",
            "thread-id",
            "x-codex-window-id",
            "x-codex-turn-metadata",
            "x-codex-routing-hint",
            "openai-beta",
            "sec-websocket-extensions",
        ]
    );
    assert_eq!(
        read_header_value(&opening, "sec-websocket-extensions"),
        Some("permessage-deflate; client_max_window_bits")
    );
    assert_eq!(
        (
            read_header_value(&opening, "chatgpt-account-id"),
            read_header_value(&opening, "authorization"),
            read_header_value(&opening, "originator"),
            read_header_value(&opening, "version"),
            read_header_value(&opening, "openai-beta"),
            read_header_value(&opening, "x-client-request-id"),
            read_header_value(&opening, "x-codex-routing-hint"),
        ),
        (
            Some("account-fingerprint"),
            Some("Bearer access-token"),
            Some("codex_cli_rs"),
            Some("1.2.3"),
            Some("responses_websockets=2026-02-06"),
            Some("thread-fingerprint"),
            Some("model=gpt-5.5"),
        )
    );
    assert_eq!(
        STANDARD
            .decode(websocket_key)
            .expect("base64 WebSocket key")
            .len(),
        16
    );

    assert_eq!(frame.first_byte, 0b1100_0001, "FIN + RSV1 + text opcode");
    assert!(frame.masked, "official client frames are masked");
    assert_eq!(frame.mask_len, 4);
    let payload = decompress_permessage_deflate(&frame.payload);
    let payload: Value = serde_json::from_slice(&payload).expect("response.create JSON");
    assert_eq!(payload["type"], "response.create");
    assert_eq!(payload["generate"], false);
    assert_eq!(
        payload
            .as_object()
            .expect("response.create object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![
            "type",
            "model",
            "instructions",
            "input",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
            "reasoning",
            "store",
            "stream",
            "include",
            "prompt_cache_key",
            "generate",
            "client_metadata",
        ]
    );
}

#[tokio::test]
async fn websocket_compression_threshold_keeps_negotiated_dictionary_and_pool_owner() {
    use gateway_core::provider_ports::ProviderUserAgentOverride;

    let sizes = [
        127, 128, 129, 127, 4096, 4096, 127, 4096, 511, 512, 513, 127,
    ];
    for selection in [
        ProviderUserAgentOverride::Default,
        ProviderUserAgentOverride::Custom {
            user_agent: provider_openai::transport::profile::qx::DEFAULT_USER_AGENT.to_owned(),
        },
    ] {
        for (extension, threshold, reset_dictionary) in [
            (None, None, false),
            (Some("permessage-deflate"), Some(128), false),
            (
                Some("permessage-deflate; client_no_context_takeover"),
                Some(512),
                true,
            ),
            (
                Some("permessage-deflate; server_no_context_takeover"),
                Some(128),
                false,
            ),
            (
                Some("permessage-deflate; server_no_context_takeover; client_no_context_takeover"),
                Some(512),
                true,
            ),
            (
                Some("permessage-deflate; client_max_window_bits=12"),
                Some(128),
                false,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let opening = read_http_request(&mut stream).await;
                assert_eq!(
                    read_header_value(&opening, "sec-websocket-extensions"),
                    Some("permessage-deflate; client_max_window_bits"),
                );
                let key = read_header_value(&opening, "sec-websocket-key").unwrap();
                let mut reply = format!(
                    "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n",
                    derive_accept_key(key.as_bytes()),
                );
                if let Some(extension) = extension {
                    reply.push_str(&format!("Sec-WebSocket-Extensions: {extension}\r\n"));
                }
                reply.push_str("\r\n");
                stream.write_all(reply.as_bytes()).await.unwrap();

                let mut dictionary = Decompress::new(false);
                let mut models = Vec::new();
                for size in sizes {
                    let frame = read_client_frame(&mut stream).await;
                    let compressed = threshold.is_some_and(|threshold| size >= threshold);
                    assert_eq!(
                        frame.first_byte,
                        if compressed { 0xc1 } else { 0x81 },
                        "extension={extension:?}, payload bytes={size}",
                    );
                    assert!(frame.masked);
                    assert_eq!(frame.mask_len, 4);
                    let decoded = if compressed {
                        if reset_dictionary {
                            dictionary.reset(false);
                        }
                        let mut wire = frame.payload;
                        wire.extend_from_slice(&[0, 0, 0xff, 0xff]);
                        let before = dictionary.total_out();
                        let mut output = vec![0; 16 * 1024];
                        dictionary
                            .decompress(&wire, &mut output, FlushDecompress::Sync)
                            .expect(
                                "mixed compressed/uncompressed frames must keep dictionary sync",
                            );
                        output.truncate((dictionary.total_out() - before) as usize);
                        output
                    } else {
                        frame.payload
                    };
                    assert_eq!(
                        decoded.len(),
                        size,
                        "threshold uses uncompressed UTF-8 bytes"
                    );
                    let body: Value = serde_json::from_slice(&decoded).unwrap();
                    assert_eq!(body["type"], "response.create");
                    assert_eq!(body["stream"], true);
                    models.push(body["model"].as_str().unwrap().to_owned());
                    write_server_text_frame(
                        &mut stream,
                        &completed_websocket_response("resp_threshold", 1, 1),
                    )
                    .await;
                }
                models
            });

            let profile = test_wire_profile();
            profile.apply_user_agent_override(&selection).unwrap();
            let client = CodexBackendClient::new(
                reqwest::Client::builder().no_proxy().build().unwrap(),
                format!("http://{address}"),
                profile,
            )
            .with_websocket_pool(Arc::new(CodexWebSocketPool::new(Duration::from_secs(60))));
            let timestamp_digits = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .to_string()
                .len();
            let base_len = serde_json::to_vec(&json!({
                    "type": "response.create",
                    "model": "\u{4f60}",
                    "stream": true,
                    "client_metadata": {"x-codex-ws-stream-request-start-ms": "0".repeat(timestamp_digits)},
                }))
                .unwrap()
                .len();
            let mut expected_models = Vec::new();
            for (index, size) in sizes.into_iter().enumerate() {
                let model = format!("\u{4f60}{}", "x".repeat(size - base_len));
                expected_models.push(model.clone());
                // A minimal synthetic body exercises the actual 128-byte boundary
                // after the transport adds its ordinary stream/timestamp metadata.
                let request = websocket_only_request(CodexResponsesRequest::from_body(
                    json!({"model": model, "stream": false})
                        .as_object()
                        .unwrap()
                        .clone(),
                ));
                let original = request.body().clone();
                let result = timeout(
                    Duration::from_secs(5),
                    client.create_response(
                        &request,
                        request_context("compression-chain", Some("compression-account")),
                    ),
                )
                .await
                .expect("bounded compressed WS exchange")
                .expect("compressed WS exchange");
                assert_eq!(result.transport, CodexBackendTransport::WebSocket);
                assert_eq!(
                    result.websocket_pool_decision,
                    Some(if index == 0 {
                        WebSocketPoolDecision::new()
                    } else {
                        WebSocketPoolDecision::reuse()
                    }),
                );
                assert_eq!(request.body(), &original);
            }
            assert_eq!(
                timeout(Duration::from_secs(5), server)
                    .await
                    .unwrap()
                    .unwrap(),
                expected_models,
                "alternating compression must preserve every message on the same socket",
            );
        }
    }
}

async fn read_client_frame(stream: &mut TcpStream) -> ClientFrame {
    let first_byte = stream.read_u8().await.expect("frame first byte");
    let second_byte = stream.read_u8().await.expect("frame second byte");
    let payload_len = match second_byte & 0x7f {
        len @ 0..=125 => u64::from(len),
        126 => u64::from(stream.read_u16().await.expect("16-bit frame length")),
        127 => stream.read_u64().await.expect("64-bit frame length"),
        _ => unreachable!(),
    };
    let masked = second_byte & 0x80 != 0;
    let mut mask = [0_u8; 4];
    if masked {
        stream.read_exact(&mut mask).await.expect("frame mask");
    }
    let mut payload = vec![0_u8; usize::try_from(payload_len).expect("frame length fits usize")];
    stream
        .read_exact(&mut payload)
        .await
        .expect("frame payload");
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % mask.len()];
        }
    }
    ClientFrame {
        first_byte,
        masked,
        mask_len: if masked { mask.len() } else { 0 },
        payload,
    }
}

fn decompress_permessage_deflate(payload: &[u8]) -> Vec<u8> {
    let mut compressed = Vec::with_capacity(payload.len() + 4);
    compressed.extend_from_slice(payload);
    compressed.extend_from_slice(&[0x00, 0x00, 0xff, 0xff]);
    let mut output = vec![0_u8; 64 * 1024];
    let mut decompressor = Decompress::new(false);
    decompressor
        .decompress(&compressed, &mut output, FlushDecompress::Sync)
        .expect("decompress response.create frame");
    output.truncate(
        usize::try_from(decompressor.total_out()).expect("decompressed length fits usize"),
    );
    output
}

async fn write_server_text_frame(stream: &mut TcpStream, payload: &str) {
    let payload = payload.as_bytes();
    let mut frame = Vec::with_capacity(payload.len() + 10);
    frame.push(0x81);
    match payload.len() {
        len @ 0..=125 => frame.push(u8::try_from(len).expect("short frame length")),
        len @ 126..=65_535 => {
            frame.push(126);
            frame.extend_from_slice(
                &u16::try_from(len)
                    .expect("16-bit frame length")
                    .to_be_bytes(),
            );
        }
        len => {
            frame.push(127);
            frame.extend_from_slice(
                &u64::try_from(len)
                    .expect("64-bit frame length")
                    .to_be_bytes(),
            );
        }
    }
    frame.extend_from_slice(payload);
    stream
        .write_all(&frame)
        .await
        .expect("write completed frame");
}
