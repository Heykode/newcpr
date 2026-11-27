//! OAuth 只执行 Responses/Chat；原生 Messages 和计数不再跨协议转换。
use super::*;
use gateway_core::operation::TokenCountRequest;

#[tokio::test]
async fn oauth_rejects_native_messages_and_count_tokens_before_account_or_upstream_io() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let server = MockServer::start().await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    let body = json!({"model":"public-model","max_tokens":128,"messages":[{"role":"user","content":"hello"}],
        "metadata":{"user_id":"session_fixture"},"tools":[{"name":"lookup","input_schema":{"type":"object"}}]});
    for count in [false, true] {
        for mode in [
            gateway_core::account::AccountAffinity::Strict,
            gateway_core::account::AccountAffinity::Preferred,
            gateway_core::account::AccountAffinity::Relaxed,
        ] {
            let operation = if count {
                Operation::CountTokens(TokenCountRequest::from_raw_json(
                    RawJsonPayload::new("anthropic", serde_json::to_vec(&body).unwrap().into())
                        .unwrap(),
                ))
            } else {
                Operation::Generate(GenerateRequest::from_protocol_payload(
                    ProtocolPayload::json_object("anthropic", body.as_object().unwrap().clone())
                        .unwrap(),
                ))
            };
            let error = match provider
                .execute(
                    planned_request("openai", operation),
                    context_with_policy(
                        "req_retired_messages",
                        CancellationToken::new(),
                        contract_account_scope(),
                        account_policy().with_account_affinity(mode),
                    ),
                )
                .await
            {
                Ok(_) => panic!("OAuth must not execute native Messages"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), ProviderErrorKind::Unsupported);
            assert_eq!(error.send_state(), UpstreamSendState::NotSent);
        }
    }
    assert!(affinity.lookup_keys().is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
}
