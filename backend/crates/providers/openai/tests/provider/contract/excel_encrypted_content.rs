use super::*;
use gateway_core::{account::ResponsesUpstream, operation::CompactRequest};

#[tokio::test]
async fn excel_omission_is_opt_in_before_generate_websocket_and_compact_preparation() {
    for enabled in [false, true] {
        for mode in ["http", "websocket", "compact"] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            store.set_responses_upstream("acct_provider_contract", ResponsesUpstream::Excel);
            store.set_excel_encrypted_content_policy(
                "acct_provider_contract",
                enabled,
                vec!["gpt-5.4".into()],
            );
            let server = MockServer::start().await;
            let provider = provider_with_base_url(&store, server.uri());
            let source = json!({"model":"gpt-5.4","input":[{"role":"user","content":[
                {"type":"input_text","text":"keep this"},
                {"type":"encrypted_content","encrypted_content":"opaque-fixture"}
            ]}]});
            let operation = if mode == "compact" {
                Operation::Compact(CompactRequest::from_raw_json(
                    RawJsonPayload::new(
                        "openai",
                        Bytes::from(serde_json::to_vec(&source).unwrap()),
                    )
                    .unwrap(),
                ))
            } else {
                let mut payload =
                    ProtocolPayload::json_object("openai", source.as_object().unwrap().clone())
                        .unwrap();
                if mode == "websocket" {
                    payload = payload.with_context(Map::from_iter([
                        ("use_websocket".into(), json!(true)),
                        (
                            "downstream_websocket_connection_id".into(),
                            json!("encrypted-fixture"),
                        ),
                    ]));
                }
                Operation::Generate(GenerateRequest::from_protocol_payload(payload))
            };
            let result = provider
                .execute(
                    planned_request("openai", operation),
                    context("req_excel_encrypted", CancellationToken::new()),
                )
                .await;
            if enabled {
                let stream = result.expect("opted-in Excel request prepares");
                assert_eq!(stream.metadata().transport().as_str(), "excel_http_sse");
                // Streams are lazy: do not poll the actual Excel endpoint.
                drop(stream);
            } else {
                let error = match result {
                    Err(error) => error,
                    Ok(_) => panic!("default must reject"),
                };
                assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
                assert_eq!(error.send_state(), UpstreamSendState::NotSent);
                assert!(
                    error
                        .client_visible_upstream_error()
                        .unwrap()
                        .message()
                        .contains("input[0].content[1]")
                );
            }
            assert_eq!(
                source["input"][0]["content"][1]["type"],
                "encrypted_content"
            );
            assert!(server.received_requests().await.unwrap().is_empty());
        }
    }
}
