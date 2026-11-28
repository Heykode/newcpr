//! 首次目录查询失败后的周期恢复与能力发布

use crate::support::Fixture;
use gateway_core::{engine::provider::Provider, lifecycle::CancellationToken};
use serde_json::json;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

#[tokio::test]
async fn missing_catalog_is_retried_and_published_after_transient_failure() {
    let upstream = MockServer::start().await;
    let fixture = Fixture::new(&upstream.uri());
    *fixture.store.primary().catalog.lock().unwrap() = None;
    let failed = Mock::given(path("/v1internal:loadCodeAssist"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount_as_scoped(&upstream)
        .await;
    assert!(
        fixture
            .catalog
            .reconcile(&CancellationToken::new())
            .await
            .is_err()
    );
    assert!(
        fixture
            .provider
            .query_model_capabilities()
            .await
            .unwrap()
            .is_empty()
    );
    drop(failed);
    Mock::given(path("/v1internal:loadCodeAssist"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"cloudaicompanionProject":"fixture-project"})),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    Mock::given(path("/v1internal:fetchAvailableModels"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"models":{"fixture-model":{"displayName":"Fixture Model"}}})),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    let generation = fixture.provider.catalog_generation();
    fixture
        .catalog
        .reconcile(&CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        fixture
            .provider
            .query_model_capabilities()
            .await
            .unwrap()
            .len(),
        1
    );
    assert_ne!(fixture.provider.catalog_generation(), generation);
    fixture
        .catalog
        .reconcile(&CancellationToken::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn credential_revision_change_replaces_the_cached_catalog() {
    use gateway_core::account::{CredentialRevision, CredentialState, ProviderAccount, QuotaState};
    let upstream = MockServer::start().await;
    let fixture = Fixture::new(&upstream.uri());
    fixture
        .catalog
        .reconcile(&CancellationToken::new())
        .await
        .unwrap();
    assert!(upstream.received_requests().await.unwrap().is_empty());
    let generation = fixture.provider.catalog_generation();
    {
        let mut loaded = fixture.store.primary().loaded.lock().unwrap();
        let account = &loaded.account;
        loaded.account = ProviderAccount::new(
            account.id().clone(),
            account.provider().clone(),
            account.name().into(),
            account.upstream_user_id().map(str::to_owned),
            "oauth".into(),
            CredentialRevision::new(2).unwrap(),
            account.access_token_expires_at(),
        )
        .with_account_facts(
            true,
            CredentialState::Ready,
            QuotaState::unknown(),
            None,
            None,
        );
    }
    Mock::given(path("/v1internal:loadCodeAssist"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"cloudaicompanionProject":"rotated-project"})),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    Mock::given(path("/v1internal:fetchAvailableModels"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"models":{"rotated-model":{"displayName":"Rotated Model"}}})),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    fixture
        .catalog
        .reconcile(&CancellationToken::new())
        .await
        .unwrap();
    assert_ne!(fixture.provider.catalog_generation(), generation);
    {
        let stored = fixture.store.primary().catalog.lock().unwrap();
        let (observation, catalog) = stored.as_ref().unwrap();
        assert_eq!(observation.revision, CredentialRevision::new(2).unwrap());
        let catalog: serde_json::Value = serde_json::from_slice(catalog).unwrap();
        assert_eq!(catalog["project"], "rotated-project");
        assert!(catalog["models"].get("rotated-model").is_some());
        assert!(catalog["models"].get("fixture-model").is_none());
    }
    fixture
        .catalog
        .reconcile(&CancellationToken::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn renewed_identity_rebinds_catalog_without_waiting_for_another_upstream_query() {
    let upstream = MockServer::start().await;
    let fixture = Fixture::new(&upstream.uri());
    let old = fixture.store.primary().loaded.lock().unwrap().clone();
    let mut captured = fixture
        .catalog
        .capture_refresh_catalogs(std::slice::from_ref(&old))
        .await
        .unwrap();
    let revision = gateway_core::account::CredentialRevision::new(2).unwrap();
    fixture.store.primary().loaded.lock().unwrap().account =
        gateway_core::account::ProviderAccount::new(
            old.account.id().clone(),
            old.account.provider().clone(),
            old.account.name().into(),
            old.account.upstream_user_id().map(str::to_owned),
            "oauth".into(),
            revision,
            old.account.access_token_expires_at(),
        );
    assert!(fixture.catalog.catalogs().await.unwrap().is_empty());
    fixture
        .catalog
        .after_credential_refresh(
            old.account.id(),
            revision,
            captured.remove(old.account.id()),
        )
        .await
        .unwrap();
    let catalogs = fixture.catalog.catalogs().await.unwrap();
    assert_eq!(catalogs[0].0[0].revision, revision);
    assert!(catalogs[0].1.models.contains_key("fixture-model"));
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn native_catalog_preserves_upstream_objects_without_chat_menu_filtering() {
    let upstream = MockServer::start().await;
    let fixture = Fixture::new(&upstream.uri());
    *fixture.store.primary().catalog.lock().unwrap() = None;
    let document = json!({
        "models": {
            "chat-model": {"displayName":"Chat","tagTitle":"New","tagDescription":"Leaving on Nov 30.","quotaInfo":{"remainingFraction":0.5}},
            "gemini-3.1-flash-image": {"displayName":"Image","unknownImageMetadata":{"future":true}},
            "gemini-3.5-flash-lite": {"displayName":"Search"},
            "future-helper": {"unknown":[1,{"newCapability":true}]}
        },
        "agentModelSorts":[{"groups":[{"modelIds":["chat-model"],"futureGroupField":"keep"}]}],
        "imageGenerationModelIds":["gemini-3.1-flash-image"],
        "webSearchModelIds":["gemini-3.5-flash-lite"],
        "futureCatalogField":{"values":[1,2,3]}
    });
    Mock::given(path("/v1internal:loadCodeAssist"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"cloudaicompanionProject":"fixture-project"})),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    Mock::given(path("/v1internal:fetchAvailableModels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&document))
        .expect(1)
        .mount(&upstream)
        .await;
    fixture
        .catalog
        .reconcile(&CancellationToken::new())
        .await
        .unwrap();
    let mut expected = document;
    expected["models"]["chat-model"]
        .as_object_mut()
        .unwrap()
        .remove("quotaInfo");
    {
        let stored = fixture.store.primary().catalog.lock().unwrap();
        let mut actual: serde_json::Value =
            serde_json::from_slice(&stored.as_ref().unwrap().1).unwrap();
        assert_eq!(
            actual.as_object_mut().unwrap().remove("project"),
            Some(json!("fixture-project"))
        );
        assert_eq!(actual, expected);
    }
    let entries = fixture
        .provider
        .query_client_model_catalog(
            &gateway_core::policy::ClientApiKeyId::new("key_fixture").unwrap(),
            &fixture.store.account_scope(),
            gateway_protocol::genai::GENAI_PROTOCOL,
            "",
        )
        .await
        .unwrap()
        .expect("native GenAI catalog");
    assert_eq!(entries.len(), expected["models"].as_object().unwrap().len());
    for entry in entries {
        let payload = entry.payload;
        let actual: serde_json::Value = serde_json::from_slice(payload.body()).unwrap();
        assert_eq!(actual, expected["models"][entry.model.as_str()]);
    }
}

#[tokio::test]
async fn native_catalog_selects_whole_objects_only_from_authorized_sources() {
    use gateway_core::account::ProviderAccountStore;
    let upstream = MockServer::start().await;
    let fixture = Fixture::with_two_accounts(&upstream.uri());
    for (accounts, catalog) in fixture.catalog.catalogs().await.unwrap() {
        let mut document = serde_json::to_value(catalog).unwrap();
        document["models"]["fixture-model"] = if accounts[0].account_id.as_str() == "acct_fixture" {
            json!({"displayName":"Primary","primaryOnly":true})
        } else {
            json!({"displayName":"Standby","standbyOnly":true})
        };
        assert!(
            fixture
                .store
                .save_account_catalog(&accounts[0], &serde_json::to_vec(&document).unwrap())
                .await
                .unwrap()
        );
    }
    let scope = fixture.store.account_scope();
    for (scope, expected) in [
        (
            scope.as_ref().clone(),
            json!({"displayName":"Primary","primaryOnly":true}),
        ),
        (
            scope.restricted_to_accounts(
                &scope
                    .allowed_account_ids()
                    .filter(|id| id.as_str() != "acct_fixture")
                    .cloned()
                    .collect(),
            ),
            json!({"displayName":"Standby","standbyOnly":true}),
        ),
    ] {
        let entries = fixture
            .provider
            .query_client_model_catalog(
                &gateway_core::policy::ClientApiKeyId::new("key_fixture").unwrap(),
                &scope,
                gateway_protocol::genai::GENAI_PROTOCOL,
                "",
            )
            .await
            .unwrap()
            .expect("native GenAI catalog");
        assert_eq!(entries.len(), 1);
        let payload = &entries[0].payload;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(payload.body()).unwrap(),
            expected
        );
    }
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn legacy_catalog_restores_known_fields_and_refreshes_full_discovery_metadata() {
    use gateway_core::policy::ClientApiKeyId;
    let upstream = MockServer::start().await;
    let fixture = Fixture::new(&upstream.uri());
    let old_model = json!({
        "displayName":"Legacy", "quotaInfo":null, "description":null,
        "tagDescription":"Known", "maxTokens":1000, "maxOutputTokens":100,
        "supportsImages":true
    });
    {
        let mut stored = fixture.store.primary().catalog.lock().unwrap();
        let (observation, document) = stored.as_mut().unwrap();
        observation.observed_at =
            std::time::SystemTime::now() - std::time::Duration::from_secs(3601);
        *document = serde_json::to_vec(&json!({
            "project":"fixture-project", "agentModelOrder":{"fixture-model":0},
            "webSearchModelIds":[], "groupedModels":null,
            "models":{"fixture-model":old_model}
        }))
        .unwrap()
        .into();
    }
    let scope = fixture.store.account_scope();
    let key = ClientApiKeyId::new("key_fixture").unwrap();
    let entries = fixture
        .provider
        .query_client_model_catalog(&key, &scope, "genai", "")
        .await
        .unwrap()
        .unwrap();
    let mut expected = old_model;
    expected.as_object_mut().unwrap().remove("quotaInfo");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(entries[0].payload.body()).unwrap(),
        expected
    );
    for protocol in ["codex", "anthropic"] {
        assert!(
            fixture
                .provider
                .query_client_model_catalog(&key, &scope, protocol, "")
                .await
                .unwrap()
                .is_none()
        );
    }
    assert!(upstream.received_requests().await.unwrap().is_empty());
    Mock::given(path("/v1internal:loadCodeAssist"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"cloudaicompanionProject":"fixture-project"})),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    Mock::given(path("/v1internal:fetchAvailableModels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "models":{
                "fixture-model":{"displayName":"Updated", "maxOutputTokens":200, "future":null},
                "fixture-image":{"displayName":"Image"}
            },
            "agentModelSorts":[{"groups":[{"modelIds":["fixture-model"]}]}],
            "imageGenerationModelIds":["fixture-image", "fixture-model", "fixture-image"]
        })))
        .expect(1)
        .mount(&upstream)
        .await;
    fixture
        .catalog
        .reconcile(&CancellationToken::new())
        .await
        .unwrap();
    let catalogs = fixture.catalog.catalogs().await.unwrap();
    assert_eq!(
        catalogs[0].1.discovery_order().unwrap(),
        [("fixture-model".into(), 0), ("fixture-image".into(), 1)].into()
    );
    assert!(!catalogs[0].1.metadata.contains_key("agentModelOrder"));
    let entries = fixture
        .provider
        .query_client_model_catalog(&key, &scope, "genai", "")
        .await
        .unwrap()
        .unwrap();
    let model = entries
        .iter()
        .find(|m| m.model.as_str() == "fixture-model")
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(model.payload.body()).unwrap(),
        json!({"displayName":"Updated", "maxOutputTokens":200, "future":null})
    );
    fixture
        .catalog
        .reconcile(&CancellationToken::new())
        .await
        .unwrap();
}
