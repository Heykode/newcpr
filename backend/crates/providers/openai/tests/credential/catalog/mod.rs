use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{TimeZone as _, Utc};
use gateway_core::account::{ProviderAccount, ProviderAccountId, ProviderAccountStore};
use gateway_core::provider_ports::ProviderCatalogCachePort;
use gateway_core::routing::{
    ClientRoutingScope, FrozenAccountScope, ModelServiceTier, ProviderKind, RuntimeAccount,
    RuntimeAccountDirectory,
};
use provider_openai::OFFICIAL_CODEX_BASE_URL;
use provider_openai::credential::{
    CodexCredentialCatalogError, CodexCredentialCatalogService, ImportCodexOAuthCredential,
};
use provider_openai::transport::CodexModelCatalogError;
use provider_openai::transport::profile::{CodexWireProfile, CodexWireProfileState};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::support::{MemoryAccountStore, catalog_cache, profile, secret};

pub(super) const OFFICIAL_FIXTURE: &[u8] =
    include_bytes!("../../transport/fixtures/official_models_snapshot.json");

struct ReplacingCatalogResponder {
    calls: Arc<AtomicUsize>,
}

impl Respond for ReplacingCatalogResponder {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let body = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            OFFICIAL_FIXTURE.to_vec()
        } else {
            br#"{"models":[{"slug":"gpt-5.5","display_name":"GPT-5.5"}]}"#.to_vec()
        };
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/json")
            .set_body_raw(body, "application/json")
    }
}

/// 按调用顺序为不同套餐 scope 返回不同的目录正文。
struct SequencedCatalogResponder {
    calls: Arc<AtomicUsize>,
    bodies: [&'static [u8]; 2],
}

impl Respond for SequencedCatalogResponder {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let index = self.calls.fetch_add(1, Ordering::SeqCst).min(1);
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/json")
            .set_body_raw(self.bodies[index].to_vec(), "application/json")
    }
}

pub(super) fn wire_profile() -> CodexWireProfileState {
    CodexWireProfileState::new(CodexWireProfile {
        installation_seed: None,
        raw_user_agent: None,
        originator: "codex_cli_rs".to_owned(),
        codex_version: "0.144.0".to_owned(),
        desktop_version: "1.0.0".to_owned(),
        desktop_build: "1".to_owned(),
        os_type: "linux".to_owned(),
        os_version: "6.8".to_owned(),
        arch: "x86_64".to_owned(),
        terminal: "catalog-contract".to_owned(),
        residency: None,
        account_routing_override: None,
        location: Default::default(),
        verified_at: Utc
            .with_ymd_and_hms(2026, 7, 18, 0, 0, 0)
            .single()
            .expect("fixture time"),
    })
}

fn service(store: &Arc<MemoryAccountStore>) -> CodexCredentialCatalogService {
    service_with_catalog_cache(store, OFFICIAL_CODEX_BASE_URL.to_owned(), catalog_cache())
}

pub(super) fn service_with_catalog_cache(
    store: &Arc<MemoryAccountStore>,
    base_url: String,
    catalog_cache: Arc<dyn ProviderCatalogCachePort>,
) -> CodexCredentialCatalogService {
    CodexCredentialCatalogService::new(
        store.repository(),
        wire_profile(),
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        base_url,
        catalog_cache,
    )
}

pub(super) async fn seed_account(
    store: &Arc<MemoryAccountStore>,
    account_id: &str,
) -> ProviderAccount {
    seed_account_with_plan(store, account_id, "pro").await
}

async fn seed_account_with_plan(
    store: &Arc<MemoryAccountStore>,
    account_id: &str,
    plan: &str,
) -> ProviderAccount {
    let mut verified_account = profile(&format!("chatgpt-{account_id}"));
    verified_account.plan_type = Some(plan.to_owned());
    store
        .seed_oauth_credential(ImportCodexOAuthCredential {
            account_id: account_id.to_owned(),
            name: account_id.to_owned(),
            secret: secret(&format!("access-{account_id}")),
            verified_account,
            next_refresh_at: Some(Utc::now() + chrono::Duration::minutes(30)),
            enabled: true,
        })
        .await;
    store.account(account_id).expect("seeded account")
}

pub(super) fn client_scope(accounts: &[ProviderAccount]) -> FrozenAccountScope {
    FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(
            accounts
                .iter()
                .map(|account| {
                    (
                        account.id().clone(),
                        RuntimeAccount::new(
                            ProviderKind::new("openai").expect("provider"),
                            Default::default(),
                        )
                        .with_model_access(account.model_access().clone()),
                    )
                })
                .collect(),
        )),
        ClientRoutingScope::all_accounts(),
    )
}

#[tokio::test]
async fn restored_catalog_is_account_scoped_and_keeps_original_objects() {
    let store = Arc::new(MemoryAccountStore::default());
    let first = seed_account(&store, "acct_client_a").await;
    let second = seed_account(&store, "acct_client_b").await;
    let denied = second.clone().with_model_access(
        gateway_core::account::AccountModelAccess::new(
            gateway_core::account::AccountModelAccessMode::Denylist,
            vec!["gpt-extra".into()],
        )
        .unwrap(),
    );
    let server = MockServer::start().await;
    for (account, text) in [
        ("acct_client_a", "a-original"),
        ("acct_client_b", "b-original"),
    ] {
        Mock::given(method("GET")).and(path("/codex/models"))
            .and(header("authorization", format!("Bearer access-{account}")))
            .and(header("chatgpt-account-id", format!("chatgpt-{account}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"models":[{
                "slug":"gpt-native", "display_name":"Native", "base_instructions":text,
                "future_field":{"null":null,"list":[1,true]}, "model_messages":{"new_template":text}
            }, {"slug": if account == "acct_client_b" {"gpt-extra"} else {"gpt-native-a"}, "display_name":"Additional"}]}))).expect(1).mount(&server).await;
    }
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    service.refresh_catalogs().await.unwrap();
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    service.restore().await.unwrap();
    for (accounts, text, expected) in [
        (
            vec![second.clone(), first.clone()],
            "a-original",
            vec!["gpt-native", "gpt-native-a", "gpt-extra"],
        ),
        (vec![second], "b-original", vec!["gpt-native", "gpt-extra"]),
        (
            vec![first.clone(), denied],
            "a-original",
            vec!["gpt-native", "gpt-native-a"],
        ),
        (
            vec![first],
            "a-original",
            vec!["gpt-native", "gpt-native-a"],
        ),
    ] {
        let models = service
            .client_model_catalog(&client_scope(&accounts))
            .await
            .expect("catalog");
        assert_eq!(
            models.iter().map(|m| m.model.as_str()).collect::<Vec<_>>(),
            expected
        );
        let document: serde_json::Value =
            serde_json::from_slice(models[0].payload.body()).expect("document");
        assert_eq!(
            document,
            serde_json::json!({
                "slug":"gpt-native", "display_name":"Native", "base_instructions":text,
                "future_field":{"null":null,"list":[1,true]}, "model_messages":{"new_template":text}
            })
        );
    }
    assert!(
        service
            .client_model_catalog(&client_scope(&[]))
            .await
            .is_err()
    );
    server.verify().await;
}

#[tokio::test]
async fn client_catalog_cache_does_not_survive_credential_rotation() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_client_rotate").await;
    let scope = client_scope(std::slice::from_ref(&account));
    let server = MockServer::start().await;
    for (token, text) in [
        ("access-acct_client_rotate", "old"),
        ("rotated-access", "new"),
    ] {
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .and(header("authorization", format!("Bearer {token}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"models":[{
                    "slug":"gpt-native", "display_name":"Native", "base_instructions":text
                }]})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    service.refresh_catalogs().await.unwrap();
    let first = service.client_model_catalog(&scope).await.expect("old");
    store
        .repository()
        .rotate_refreshed_oauth_secret(&account, secret("rotated-access"), None, None)
        .await
        .expect("rotate");
    assert!(service.client_model_catalog(&scope).await.is_err());
    service.refresh_catalogs().await.unwrap();
    let second = service.client_model_catalog(&scope).await.expect("new");
    assert_ne!(first, second);
    server.verify().await;
}

#[tokio::test]
async fn failed_client_catalog_never_falls_back_to_an_out_of_scope_account_or_cached_profile() {
    let store = Arc::new(MemoryAccountStore::default());
    let allowed = seed_account(&store, "acct_client_allowed").await;
    seed_account(&store, "acct_client_outside").await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .and(header("authorization", "Bearer access-acct_client_allowed"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    let scope = client_scope(&[allowed]);
    assert!(service.refresh_catalogs().await.is_err());
    for _ in 0..2 {
        assert!(service.client_model_catalog(&scope).await.is_err());
    }
    assert_eq!(server.received_requests().await.expect("requests").len(), 2);
    server.verify().await;
}

#[tokio::test]
async fn raw_instructions_only_change_advances_catalog_generation() {
    let store = Arc::new(MemoryAccountStore::default());
    seed_account(&store, "acct_raw_generation").await;
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/codex/models"))
        .respond_with(SequencedCatalogResponder {
            calls: Arc::new(AtomicUsize::new(0)),
            bodies: [
                br#"{"models":[{"slug":"gpt-native","display_name":"Native","model_messages":{"future":"old"}}]}"#,
                br#"{"models":[{"slug":"gpt-native","display_name":"Native","model_messages":{"future":"new"}}]}"#,
            ],
        }).expect(2).mount(&server).await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    service.refresh_catalogs().await.expect("initial");
    let generation = service.catalog_generation();
    service
        .observe_response_etag(&store.account("acct_raw_generation").unwrap(), "new")
        .unwrap();
    service.refresh_catalogs().await.expect("refresh");
    assert!(service.catalog_generation() > generation);
    server.verify().await;
}

#[test]
fn catalog_starts_without_a_snapshot_or_generation() {
    let store = Arc::new(MemoryAccountStore::default());
    let service = service(&store);

    assert!(service.cached().expect("cache read").is_none());
    assert_eq!(service.catalog_generation().get(), 0);
}

#[tokio::test]
async fn catalog_without_openai_accounts_fails_before_network_io() {
    let store = Arc::new(MemoryAccountStore::default());

    let error = service(&store)
        .refresh_catalogs()
        .await
        .expect_err("empty account set");

    assert!(matches!(
        error,
        CodexCredentialCatalogError::NoEligibleCredential
    ));
}

#[tokio::test]
async fn missing_account_refresh_fails_before_network_io() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = ProviderAccountId::new("acct_missing").expect("account id");

    let error = service(&store)
        .refresh_account_catalog(&account)
        .await
        .expect_err("missing account");

    assert!(matches!(
        error,
        CodexCredentialCatalogError::NoEligibleCredential
    ));
}

#[tokio::test]
async fn disabled_account_refresh_discovers_models_with_the_pinned_account() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_disabled_models").await;
    store
        .set_enabled(account.id(), false)
        .await
        .expect("disable account");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_raw(
                    br#"{"models":[{"slug":"gpt-5.4","display_name":"GPT-5.4"}]}"#.to_vec(),
                    "application/json",
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());

    let catalog = service
        .refresh_account_catalog(account.id())
        .await
        .expect("disabled account refresh still discovers models");

    assert_eq!(catalog.models(), ["gpt-5.4"]);
    server.verify().await;
}

#[tokio::test]
async fn refreshing_one_account_does_not_replace_a_peers_cached_catalog() {
    let store = Arc::new(MemoryAccountStore::default());
    let first = seed_account(&store, "acct_catalog_a").await;
    let second = seed_account(&store, "acct_catalog_b").await;
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(ReplacingCatalogResponder {
            calls: Arc::clone(&calls),
        })
        .expect(2)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());

    let initial = service
        .cached_or_refresh_account_catalog(&first)
        .await
        .expect("cold cache fetch");
    let cached = service
        .cached_or_refresh_account_catalog(&first)
        .await
        .expect("account cache hit");
    let refreshed = service
        .refresh_account_catalog(second.id())
        .await
        .expect("manual refresh");
    let replaced = service
        .cached_or_refresh_account_catalog(&first)
        .await
        .expect("peer cache hit");

    assert_eq!(initial.models(), ["gpt-5.4"]);
    assert_eq!(cached.models(), ["gpt-5.4"]);
    assert_eq!(refreshed.models(), ["gpt-5.5"]);
    assert_eq!(replaced.models(), ["gpt-5.4"]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.verify().await;
}

#[tokio::test]
async fn service_tier_changes_should_replace_cached_metadata_and_advance_generation() {
    let store = Arc::new(MemoryAccountStore::default());
    seed_account(&store, "acct_service_tiers").await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(SequencedCatalogResponder {
            calls: Arc::new(AtomicUsize::new(0)),
            bodies: [
                br#"{"models":[{"slug":"gpt-5.4","display_name":"GPT-5.4","service_tiers":[{"id":"priority","name":"Fast","description":"Priority processing."}]}]}"#,
                br#"{"models":[{"slug":"gpt-5.4","display_name":"GPT-5.4","service_tiers":[]}]}"#,
            ],
        })
        .expect(2)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());

    let initial = service.refresh_catalogs().await.expect("initial catalog");
    let generation = service.catalog_generation().get();
    let cached = service.refresh_catalogs().await.expect("cached catalog");
    assert_eq!(cached.models(), initial.models());
    assert_eq!(
        cached.models()[0].metadata().service_tiers(),
        [ModelServiceTier::new(
            "priority",
            "Fast",
            "Priority processing."
        )]
    );
    assert_eq!(service.catalog_generation().get(), generation);

    service
        .observe_response_etag(&store.account("acct_service_tiers").unwrap(), "new")
        .unwrap();
    let refreshed = service.refresh_catalogs().await.expect("updated catalog");
    assert!(refreshed.models()[0].metadata().service_tiers().is_empty());
    assert_eq!(service.catalog_generation().get(), generation + 1);
    assert_eq!(
        service
            .cached()
            .expect("cache read")
            .expect("cached snapshot")
            .models(),
        refreshed.models()
    );
}

#[tokio::test]
async fn failed_oauth_catalog_does_not_hide_healthy_peers_or_borrow_their_models() {
    let store = Arc::new(MemoryAccountStore::default());
    let failed = seed_account(&store, "acct_failed").await;
    let healthy = seed_account(&store, "acct_healthy").await;
    let server = MockServer::start().await;
    for (account, status) in [(&failed, 503), (&healthy, 200)] {
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .and(header(
                "chatgpt-account-id",
                format!("chatgpt-{}", account.id()),
            ))
            .respond_with(
                ResponseTemplate::new(status).set_body_raw(OFFICIAL_FIXTURE, "application/json"),
            )
            .expect(if status == 503 { 2 } else { 1 })
            .mount(&server)
            .await;
    }
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    service.refresh_catalogs().await.unwrap();
    let models = service
        .client_model_catalog(&client_scope(&[failed.clone(), healthy]))
        .await
        .unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].model.as_str(), "gpt-5.4");
    let error = service
        .refresh_account_catalog(failed.id())
        .await
        .expect_err("account refresh must not use a peer");

    assert!(matches!(
        error,
        CodexCredentialCatalogError::Upstream { .. }
    ));
    server.verify().await;
}

#[tokio::test]
async fn cross_plan_presentation_drift_unions_on_the_first_seen_model() {
    let store = Arc::new(MemoryAccountStore::default());
    seed_account_with_plan(&store, "acct_union_plus", "plus").await;
    seed_account_with_plan(&store, "acct_union_pro", "pro").await;
    let server = MockServer::start().await;
    for (account, name) in [
        ("acct_union_plus", "GPT-5.4 Plus"),
        ("acct_union_pro", "GPT-5.4 Pro"),
    ] {
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .and(header("chatgpt-account-id", format!("chatgpt-{account}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "models":[{"slug":"gpt-5.4","display_name":name,"supported_in_api":true}]
            })))
            .expect(1)
            .mount(&server)
            .await;
    }
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());

    let snapshot = service
        .refresh_catalogs()
        .await
        .expect("presentation drift must not wedge the union");

    assert_eq!(snapshot.models().len(), 1);
    assert_eq!(snapshot.models()[0].display_name(), "GPT-5.4 Plus");
    server.verify().await;
}

#[tokio::test]
async fn cross_plan_supported_in_api_drift_unions_on_the_first_seen_model() {
    let store = Arc::new(MemoryAccountStore::default());
    seed_account_with_plan(&store, "acct_conflict_plus", "plus").await;
    seed_account_with_plan(&store, "acct_conflict_pro", "pro").await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(SequencedCatalogResponder {
            calls: Arc::new(AtomicUsize::new(0)),
            bodies: [
                br#"{"models":[{"slug":"gpt-5.4","display_name":"GPT-5.4","supported_in_api":true}]}"#,
                br#"{"models":[{"slug":"gpt-5.4","display_name":"GPT-5.4","supported_in_api":false}]}"#,
            ],
        })
        .expect(2)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());

    let snapshot = service
        .refresh_catalogs()
        .await
        .expect("supported_in_api drift must not block catalog publication");

    assert_eq!(snapshot.models().len(), 1);
    server.verify().await;
}

#[tokio::test]
async fn account_entitlements_keep_the_discovering_account_across_plans() {
    let store = Arc::new(MemoryAccountStore::default());
    let first_free = seed_account_with_plan(&store, "acct_free_a", "free").await;
    let second_free = seed_account_with_plan(&store, "acct_free_b", "free").await;
    let k12 = seed_account_with_plan(&store, "acct_k12", "k12").await;
    let server = MockServer::start().await;
    for (account, extra) in [(&first_free, false), (&second_free, true), (&k12, true)] {
        let mut models = vec![serde_json::json!({"slug":"gpt-5.4","display_name":"GPT-5.4"})];
        if extra {
            models.push(serde_json::json!({"slug":"gpt-5.6-sol","display_name":"Sol"}));
        }
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .and(header(
                "chatgpt-account-id",
                format!("chatgpt-{}", account.id()),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"models":models})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());

    let snapshot = service
        .refresh_catalogs()
        .await
        .expect("mixed-plan catalog");

    assert_eq!(snapshot.models().len(), 2);
    assert_eq!(
        service
            .cached_account_models(&first_free)
            .expect("first free entitlement"),
        Some(vec!["gpt-5.4".to_owned()])
    );
    assert_eq!(
        service
            .cached_account_models(&second_free)
            .expect("second free entitlement"),
        Some(vec!["gpt-5.4".to_owned(), "gpt-5.6-sol".to_owned()])
    );
    assert_eq!(
        service.cached_account_models(&k12).expect("K12 catalog"),
        Some(vec!["gpt-5.4".to_owned(), "gpt-5.6-sol".to_owned()])
    );
    assert_eq!(
        snapshot.model_catalog_accounts()["gpt-5.6-sol"],
        std::collections::BTreeSet::from([second_free.id().clone(), k12.id().clone()])
    );
    let restarted = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    restarted.restore().await.unwrap();
    let restored = restarted.refresh_catalogs().await.unwrap();
    assert_eq!(
        restored.model_catalog_accounts(),
        snapshot.model_catalog_accounts()
    );
    server.verify().await;
}

#[tokio::test]
async fn invalidation_queues_background_refresh_without_an_upstream_etag() {
    let store = Arc::new(MemoryAccountStore::default());
    let service = service(&store);
    service
        .invalidate_accounts(&[ProviderAccountId::new("acct_first").unwrap()])
        .await
        .unwrap();
    service
        .invalidate_accounts(&[ProviderAccountId::new("acct_first").unwrap()])
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        service.wait_for_refresh(),
    )
    .await
    .expect("configuration invalidation must wake the catalog worker");
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            service.wait_for_refresh(),
        )
        .await
        .is_err(),
        "duplicate invalidations should share one pending refresh"
    );
}

#[tokio::test]
async fn simultaneous_periodic_refreshes_share_the_completed_scan() {
    let store = Arc::new(MemoryAccountStore::default());
    for index in 0..64 {
        seed_account(&store, &format!("acct_refresh_{index:02}")).await;
    }
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(OFFICIAL_FIXTURE.to_vec(), "application/json")
                .set_delay(std::time::Duration::from_millis(20)),
        )
        .expect(64)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    let (first, second, third) = tokio::join!(
        service.refresh_catalogs(),
        service.refresh_catalogs(),
        service.refresh_catalogs(),
    );
    let first = first.expect("first refresh");
    assert_eq!(first.models(), second.expect("shared refresh").models());
    assert_eq!(first.models(), third.expect("shared refresh").models());
    server.verify().await;
}

#[tokio::test]
async fn account_catalog_documents_keep_native_objects_of_the_account_plan() {
    let store = Arc::new(MemoryAccountStore::default());
    let plus = seed_account_with_plan(&store, "acct_catalog_plus", "plus").await;
    let pro = seed_account_with_plan(&store, "acct_catalog_pro", "pro").await;
    let server = MockServer::start().await;
    for (account, unique_model, name, context_window) in [
        ("acct_catalog_plus", "gpt-plus", "Plus", 128_000),
        ("acct_catalog_pro", "gpt-pro", "Pro", 272_000),
    ] {
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .and(header("authorization", format!("Bearer access-{account}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "models": [
                    {"slug": unique_model, "display_name": name, "context_window": context_window},
                    {"slug": "gpt-shared", "display_name": name, "context_window": context_window}
                ]
            })))
            .expect(2)
            .mount(&server)
            .await;
    }
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    let snapshot = service
        .refresh_catalogs()
        .await
        .expect("shared union snapshot");
    assert_eq!(snapshot.models().len(), 3);
    assert_eq!(snapshot.models()[1].display_name(), "Plus");

    let (models, _) = service
        .account_catalog_documents(&pro)
        .await
        .expect("pro catalog");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].request_model().as_str(), "gpt-pro");
    assert_eq!(models[1].request_model().as_str(), "gpt-shared");
    // 导出的目录文件靠原生对象携带上下文窗口等元数据，正文必须原样保留
    assert_eq!(models[0].document().protocol(), "codex");
    let document: serde_json::Value =
        serde_json::from_slice(models[1].document().body()).expect("native document");
    assert_eq!(document["context_window"], 272_000);
    assert_eq!(document["display_name"], "Pro");

    // 另一套餐的账号不能拿到别的套餐条目，否则客户端会列出自己用不了的模型
    let (plus_models, _) = service
        .account_catalog_documents(&plus)
        .await
        .expect("plus catalog");
    assert_eq!(plus_models.len(), 2);
    assert_eq!(plus_models[0].request_model().as_str(), "gpt-plus");
    let plus_document: serde_json::Value =
        serde_json::from_slice(plus_models[1].document().body()).expect("plus native document");
    assert_eq!(plus_document["context_window"], 128_000);

    server.verify().await;
}

#[tokio::test]
async fn disabled_account_can_export_native_catalog_without_a_cached_snapshot() {
    let store = Arc::new(MemoryAccountStore::default());
    let account = seed_account(&store, "acct_disabled_export").await;
    store
        .set_enabled(account.id(), false)
        .await
        .expect("disable account");
    let disabled = store
        .account("acct_disabled_export")
        .expect("disabled account");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .and(header(
            "authorization",
            "Bearer access-acct_disabled_export",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [{"slug": "gpt-5.4", "display_name": "GPT-5.4", "context_window": 272000}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());

    let (models, _) = service
        .account_catalog_documents(&disabled)
        .await
        .expect("disabled account export");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].document().protocol(), "codex");
    server.verify().await;
}

#[tokio::test]
async fn catalog_account_membership_changes_advance_generation_without_model_changes() {
    use std::collections::BTreeSet;
    let store = Arc::new(MemoryAccountStore::default());
    let first = seed_account(&store, "acct_source_a").await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [{"slug": "gpt-5.4", "display_name": "GPT-5.4"}]
        })))
        .expect(2)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    let initial = service.refresh_catalogs().await.unwrap();
    let generation = service.catalog_generation();
    let second = seed_account(&store, "acct_source_b").await;
    let refreshed = service.refresh_catalogs().await.unwrap();
    assert_eq!(initial.models(), refreshed.models());
    assert!(service.catalog_generation() > generation);
    assert_eq!(
        initial.model_catalog_accounts()["gpt-5.4"],
        BTreeSet::from([first.id().clone()])
    );
    assert_eq!(
        refreshed.model_catalog_accounts()["gpt-5.4"],
        BTreeSet::from([first.id().clone(), second.id().clone()])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ten_thousand_restored_accounts_serve_concurrent_native_reads_while_upstream_stalls() {
    use futures::{StreamExt as _, stream};
    use gateway_core::account::AccountCatalogObservation;
    use std::time::{Duration, Instant, SystemTime};
    let store = Arc::new(MemoryAccountStore::default());
    let document = [&[0_u8][..], OFFICIAL_FIXTURE].concat();
    let mut accounts = Vec::new();
    for index in 0..10_000 {
        let account = seed_account(&store, &format!("acct_load_{index:05}")).await;
        assert!(
            store
                .save_account_catalog(
                    &AccountCatalogObservation {
                        account_id: account.id().clone(),
                        revision: account.revision(),
                        plan: account.plan_type().map(str::to_owned),
                        observed_at: SystemTime::now(),
                        etag: None
                    },
                    &document
                )
                .await
                .unwrap()
        );
        accounts.push(account);
    }
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_secs(30)))
        .expect(1)
        .mount(&server)
        .await;
    let catalog = Arc::new(service_with_catalog_cache(
        &store,
        server.uri(),
        catalog_cache(),
    ));
    let start = Instant::now();
    catalog.restore().await.unwrap();
    let restore = start.elapsed();
    catalog.refresh_catalogs().await.unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
    let scope = Arc::new(client_scope(&accounts));
    catalog
        .observe_response_etag(&accounts[0], "\"load-change\"")
        .unwrap();
    let refresh = tokio::spawn({
        let catalog = catalog.clone();
        async move { catalog.refresh_catalogs().await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.received_requests().await.unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    const REQUESTS: usize = 2048;
    let start = Instant::now();
    let reads = stream::iter(0..REQUESTS)
        .map(|_| {
            let catalog = catalog.clone();
            let scope = scope.clone();
            tokio::spawn(async move {
                let started = Instant::now();
                let models = catalog.client_model_catalog(&scope).await.unwrap();
                assert!(!models.is_empty());
                started.elapsed()
            })
        })
        .buffer_unordered(64)
        .collect::<Vec<_>>();
    let mut samples = tokio::time::timeout(Duration::from_secs(10), reads)
        .await
        .unwrap()
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    samples.sort_unstable();
    eprintln!(
        "catalog load: 10000 accounts, {REQUESTS} successful requests, concurrency 64, restore={} ms, elapsed={} ms, p50={} us, p95={} us, p99={} us",
        restore.as_millis(),
        start.elapsed().as_millis(),
        samples[REQUESTS / 2].as_micros(),
        samples[REQUESTS * 95 / 100].as_micros(),
        samples[REQUESTS * 99 / 100].as_micros()
    );
    assert!(!refresh.is_finished());
    refresh.abort();
    assert!(refresh.await.unwrap_err().is_cancelled());
    server.verify().await;
}

#[tokio::test]
async fn account_catalog_export_uses_selected_disabled_account_and_keeps_native_objects() {
    let store = Arc::new(MemoryAccountStore::default());
    let selected = seed_account(&store, "acct_export_selected").await;
    seed_account(&store, "acct_export_unrelated").await;
    store
        .set_enabled(selected.id(), false)
        .await
        .expect("disable");
    let selected = store
        .account(selected.id().as_str())
        .expect("disabled account");
    let server = MockServer::start().await;
    let native = serde_json::json!({"models":[
        {"slug":"native-z","display_name":"Selected Z","base_instructions":"selected-account","unknown":{"nested":[true,null,3]}},
        {"slug":"native-a","display_name":"Selected","context_window":98765}
    ]});
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .and(header(
            "authorization",
            "Bearer access-acct_export_selected",
        ))
        .and(header("chatgpt-account-id", "chatgpt-acct_export_selected"))
        .respond_with(ResponseTemplate::new(200).set_body_json(native.clone()))
        .expect(1)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    let (models, _) = service
        .account_catalog_documents(&selected)
        .await
        .expect("export");
    let documents = models
        .iter()
        .map(|model| {
            assert_eq!(model.document().protocol(), "codex");
            serde_json::from_slice::<serde_json::Value>(model.document().body()).unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(serde_json::json!({"models":documents}), native);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(
        service.cached().unwrap().is_none(),
        "export must not publish a plan union"
    );
    assert_eq!(service.catalog_generation().get(), 0);
    assert!(
        service
            .read_account_catalog(&selected)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!store.account(selected.id().as_str()).unwrap().enabled());
    server.verify().await;
}

#[tokio::test]
async fn account_catalog_export_rejects_invalid_or_empty_wire_without_fallback() {
    for (case, native, expected) in [
        (
            "missing_name",
            serde_json::json!({"models":[{"slug":"native-z","base_instructions":"selected-account"}]}),
            CodexModelCatalogError::InvalidWire,
        ),
        (
            "empty",
            serde_json::json!({"models":[]}),
            CodexModelCatalogError::EmptySnapshot,
        ),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        let selected_id = format!("acct_export_invalid_{case}");
        let other_id = format!("acct_export_fallback_{case}");
        let selected = seed_account(&store, &selected_id).await;
        let other = seed_account(&store, &other_id).await;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .and(header("authorization", format!("Bearer access-{other_id}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(OFFICIAL_FIXTURE, "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/codex/models"))
            .and(header(
                "authorization",
                format!("Bearer access-{selected_id}"),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(native))
            .expect(1)
            .mount(&server)
            .await;
        let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
        service
            .refresh_account_catalog(other.id())
            .await
            .expect("populate plan cache");
        let cached_plan = service
            .read_account_catalog(&selected)
            .await
            .expect("plan cache");
        assert!(cached_plan.is_none());
        let generation = service.catalog_generation();

        let error = service
            .account_catalog_documents(&selected)
            .await
            .expect_err(case);
        assert!(
            matches!(
                error,
                CodexCredentialCatalogError::Upstream { detail, status: None, egress: None }
                    if detail == format!("invalid Codex model catalog: {expected}")
            ),
            "{case}: invalid native wire must fail"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        assert_eq!(service.catalog_generation(), generation);
        assert!(service.cached().unwrap().is_some());
        assert_eq!(
            service.read_account_catalog(&selected).await.unwrap(),
            cached_plan
        );
        server.verify().await;
    }
}

#[tokio::test]
async fn account_catalog_export_failure_never_uses_an_unrelated_account_or_plan_cache() {
    let store = Arc::new(MemoryAccountStore::default());
    let selected = seed_account(&store, "acct_export_failure").await;
    let other = seed_account(&store, "acct_export_other").await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .and(header("authorization", "Bearer access-acct_export_failure"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(header("authorization", "Bearer access-acct_export_other"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(OFFICIAL_FIXTURE, "application/json"))
        .expect(1)
        .mount(&server)
        .await;
    let service = service_with_catalog_cache(&store, server.uri(), catalog_cache());
    service
        .refresh_account_catalog(other.id())
        .await
        .expect("populate a successful catalog for the same plan");
    assert!(
        service
            .read_account_catalog(&selected)
            .await
            .expect("account cache")
            .is_none()
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(matches!(
        service.account_catalog_documents(&selected).await,
        Err(CodexCredentialCatalogError::Upstream {
            status: Some(403),
            ..
        })
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    server.verify().await;
}
