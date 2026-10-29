use super::*;
use gateway_core::account::{AccountModelAccess, AccountModelAccessMode};
use gateway_core::operation::{
    GenerateRequest, ImageRequest, ImageRequestKind, Operation, OperationKind, ProtocolPayload,
    RawJsonPayload,
};
use gateway_core::routing::{ModelPresentation, PublicModelId};
use std::sync::atomic::AtomicBool;

type CatalogResult = Result<Vec<ProviderModelCapabilities>, ProviderCatalogUnavailable>;

struct Catalogs {
    models: Mutex<BTreeMap<ProviderKind, CatalogResult>>,
    generation: AtomicU64,
    queries: AtomicUsize,
    advance_on_query: AtomicBool,
}

impl Catalogs {
    fn new() -> Self {
        Self {
            models: Mutex::new(BTreeMap::from([
                (
                    kind("audit"),
                    Ok(vec![model("audit-model", "acct_removed")]),
                ),
                (
                    kind("healthy"),
                    Ok(vec![model("healthy-old", "acct_healthy")]),
                ),
            ])),
            generation: AtomicU64::new(0),
            queries: AtomicUsize::new(0),
            advance_on_query: AtomicBool::new(false),
        }
    }

    fn set(&self, provider: &str, result: CatalogResult) {
        self.models.lock().unwrap().insert(kind(provider), result);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

impl ProviderCatalogPort for Catalogs {
    fn model_catalog_is_exhaustive(&self, provider: &ProviderKind) -> bool {
        provider.as_str() == "audit"
    }

    fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        self.models
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .map(|provider| {
                (
                    provider,
                    ProviderCatalogGeneration::new(self.generation.load(Ordering::SeqCst)),
                )
            })
            .collect()
    }

    fn query_model_capabilities(&self, provider: &ProviderKind) -> BoxFuture<'_, CatalogResult> {
        let result = self.models.lock().unwrap()[provider].clone();
        Box::pin(async move {
            self.queries.fetch_add(1, Ordering::SeqCst);
            if self.advance_on_query.swap(false, Ordering::SeqCst) {
                self.generation.fetch_add(1, Ordering::SeqCst);
            }
            result
        })
    }
}

fn kind(name: &str) -> ProviderKind {
    ProviderKind::new(name).unwrap()
}
fn public(name: &str) -> PublicModelId {
    PublicModelId::new(name).unwrap()
}
fn id(name: &str) -> ProviderAccountId {
    ProviderAccountId::new(name).unwrap()
}

fn model(name: &str, account: &str) -> ProviderModelCapabilities {
    ProviderModelCapabilities::new(
        UpstreamModelId::new(name).unwrap(),
        ModelCapabilities::new([OperationKind::Generate].into(), Some(16_000)),
    )
    .with_presentation(ModelPresentation::new(
        Some(format!("Label {name}")),
        Some("Model detail".to_owned()),
    ))
    .with_catalog_accounts([id(account)].into())
}

fn facts(value: u64, removed: bool, deny: bool) -> SnapshotFacts {
    let mut accounts = vec![
        SnapshotProviderAccountFacts::new(id("acct_keep"), "audit").with_model_access(if deny {
            AccountModelAccess::new(
                AccountModelAccessMode::Denylist,
                vec!["audit-model".to_owned()],
            )
            .unwrap()
        } else {
            AccountModelAccess::all()
        }),
        SnapshotProviderAccountFacts::new(id("acct_healthy"), "healthy"),
    ];
    if removed {
        accounts.push(SnapshotProviderAccountFacts::new(
            id("acct_removed"),
            "audit",
        ));
    }
    SnapshotFacts::new(
        revision(value),
        revision(value),
        SnapshotSettingsFacts::new(
            if removed { 3 } else { 7 },
            0,
            "smart",
            BTreeMap::new(),
            None,
            None,
        ),
        vec![],
        vec![],
        accounts,
        vec![],
    )
}

fn generate() -> Operation {
    Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object(
            "openai",
            serde_json::json!({"model":"audit-model","input":"hello"})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap(),
    ))
}

fn publisher(
    store: &Arc<TestSnapshotStore>,
    catalog: &Arc<Catalogs>,
    handle: &RuntimeSnapshotHandle,
) -> RuntimeSnapshotPublisher {
    RuntimeSnapshotPublisher::new(
        Arc::new(RuntimeSnapshotCompiler::new(store.clone(), catalog.clone())),
        handle.clone(),
        Arc::new(TestSnapshotSubscriptions::default()),
    )
}

#[test]
fn provider_failure_retains_model_facts_but_publishes_fresh_permissions_and_capacity() {
    block_on(async {
        let catalog = Arc::new(Catalogs::new());
        let store = Arc::new(TestSnapshotStore::new(Ok(facts(1, true, false))));
        let handle = RuntimeSnapshotHandle::default();
        let publisher = publisher(&store, &catalog, &handle);
        publisher.refresh().await.unwrap();
        let initial = handle.acquire().unwrap();
        let profiles = initial.public_model_profiles_for_provider(&kind("audit"));

        catalog.set("audit", Err(ProviderCatalogUnavailable));
        catalog.set("healthy", Ok(vec![model("healthy-new", "acct_healthy")]));
        *store.facts.lock().unwrap() = Ok(facts(2, false, true));
        publisher.refresh().await.unwrap();
        let retained = handle.acquire().unwrap();
        assert_eq!(retained.revision(), revision(2));
        assert_eq!(
            retained.public_model_profiles_for_provider(&kind("audit")),
            profiles
        );
        assert!(
            retained.contains_public_model_for_provider(&public("audit-model"), &kind("audit"))
        );
        assert!(
            !retained.contains_public_model_for_provider(&public("unknown"), &kind("audit")),
            "exhaustive flag survives failure"
        );
        assert_eq!(
            retained.public_models_for_provider(&kind("healthy")),
            vec![public("healthy-new")]
        );
        assert!(!retained.all_account_scope().allows(&id("acct_removed")));
        assert!(
            !retained
                .all_account_scope()
                .allows_model(&id("acct_keep"), "audit-model")
        );
        assert_eq!(
            retained.account_concurrency().limit_for("acct_keep"),
            NonZeroU32::new(7)
        );
        assert_eq!(
            retained.account_concurrency().limit_for("acct_removed"),
            None
        );
        assert_eq!(
            retained.public_models_for_scope(&retained.all_account_scope()),
            vec![public("healthy-new")]
        );
        assert!(
            retained
                .plan(
                    &public("audit-model"),
                    &generate(),
                    retained.all_account_scope(),
                    &Default::default()
                )
                .is_err()
        );
        assert!(
            initial.all_account_scope().allows(&id("acct_removed")),
            "old request scope stays frozen"
        );
        assert_eq!(
            retained.provider_catalog_generations(),
            &catalog.catalog_generations()
        );

        // A current authorized account may still generate; stale catalog ownership
        // cannot expose a model discovered only by the removed account in /models.
        *store.facts.lock().unwrap() = Ok(facts(3, false, false));
        publisher.refresh().await.unwrap();
        let allowed = handle.acquire().unwrap();
        assert!(
            !allowed
                .public_models_for_scope(&allowed.all_account_scope())
                .contains(&public("audit-model"))
        );
        allowed
            .plan(
                &public("audit-model"),
                &generate(),
                allowed.all_account_scope(),
                &Default::default(),
            )
            .unwrap();
        let image = Operation::GenerateImage(ImageRequest::from_raw_json(
            ImageRequestKind::Generation,
            RawJsonPayload::new(
                "openai",
                bytes::Bytes::from_static(br#"{"model":"audit-model","prompt":"draw"}"#),
            )
            .unwrap(),
        ));
        assert!(
            allowed
                .plan(
                    &public("audit-model"),
                    &image,
                    allowed.all_account_scope(),
                    &Default::default()
                )
                .is_err(),
            "retained capabilities still reject images"
        );

        catalog.set("audit", Ok(Vec::new()));
        publisher.refresh().await.unwrap();
        let cleared = handle.acquire().unwrap();
        assert!(
            cleared
                .public_models_for_provider(&kind("audit"))
                .is_empty()
        );
        assert!(
            cleared
                .public_model_profiles_for_provider(&kind("audit"))
                .is_empty()
        );
        assert!(
            !cleared.contains_public_model_for_provider(&public("audit-model"), &kind("audit"))
        );
        assert_eq!(
            cleared.provider_catalog_generations(),
            &catalog.catalog_generations()
        );
    });
}

#[test]
fn fallback_live_refresh_still_retries_catalog_generation_changes() {
    block_on(async {
        let catalog = Arc::new(Catalogs::new());
        let store = Arc::new(TestSnapshotStore::new(Ok(facts(1, true, false))));
        let handle = RuntimeSnapshotHandle::default();
        let publisher = publisher(&store, &catalog, &handle);
        publisher.refresh().await.unwrap();
        catalog.set("audit", Err(ProviderCatalogUnavailable));
        catalog.advance_on_query.store(true, Ordering::SeqCst);
        let before = catalog.queries.load(Ordering::SeqCst);
        publisher.refresh().await.unwrap();
        assert_eq!(catalog.queries.load(Ordering::SeqCst) - before, 4);
        let retained = handle.acquire().unwrap();
        assert_eq!(
            retained.public_models_for_provider(&kind("audit")),
            vec![public("audit-model")]
        );
        assert_eq!(
            retained.provider_catalog_generations(),
            &catalog.catalog_generations()
        );
    });
}

#[test]
fn cold_start_errors_have_no_fabricated_catalog_and_cached_commits_do_not_query() {
    block_on(async {
        let catalog = Arc::new(Catalogs::new());
        catalog.set("audit", Err(ProviderCatalogUnavailable));
        let store = Arc::new(TestSnapshotStore::new(Ok(facts(1, true, false))));
        let handle = RuntimeSnapshotHandle::default();
        let publisher = publisher(&store, &catalog, &handle);
        publisher.refresh().await.unwrap();
        assert!(
            handle
                .acquire()
                .unwrap()
                .public_models_for_provider(&kind("audit"))
                .is_empty()
        );
        catalog.set("audit", Ok(vec![model("audit-model", "acct_removed")]));
        publisher.refresh().await.unwrap();
        catalog.set("audit", Err(ProviderCatalogUnavailable));
        let before = catalog.queries.load(Ordering::SeqCst);
        *store.facts.lock().unwrap() = Ok(facts(2, false, true));
        *store.current_revision.lock().unwrap() = Ok(revision(2));
        publisher.publish_committed(revision(2)).await;
        let current = handle.acquire().unwrap();
        assert_eq!(catalog.queries.load(Ordering::SeqCst), before);
        assert_eq!(
            current.public_models_for_provider(&kind("audit")),
            vec![public("audit-model")]
        );
        assert!(!current.all_account_scope().allows(&id("acct_removed")));
        assert!(
            !current
                .all_account_scope()
                .allows_model(&id("acct_keep"), "audit-model")
        );
        assert!(current.provider_catalog_generations().is_empty());
    });
}
