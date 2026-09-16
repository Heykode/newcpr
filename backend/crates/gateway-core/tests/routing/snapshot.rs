use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use futures::future::BoxFuture;

use gateway_core::account::ProviderAccountId;
use gateway_core::operation::OperationKind;
use gateway_core::policy::{ClientApiKeyId, PlaintextClientApiKey, RateLimits};
use gateway_core::routing::snapshot::{
    RuntimeSnapshotCompileError, RuntimeSnapshotCompiler, SnapshotAccountGroupFacts,
    SnapshotAccountGroupMemberFacts, SnapshotClientPolicyFacts, SnapshotFacts,
    SnapshotProviderAccountFacts, SnapshotSettingsFacts, SnapshotStoreError, SnapshotStorePort,
};
use gateway_core::routing::{
    ConfigRevision, ModelCapabilities, ModelPresentation, ProviderCatalogGeneration,
    ProviderCatalogPort, ProviderCatalogUnavailable, ProviderKind, ProviderModelCapabilities,
    PublicModelId, UpstreamModelId,
};

#[derive(Clone)]
struct TestSnapshotStore {
    facts: Arc<Mutex<Result<SnapshotFacts, SnapshotStoreError>>>,
    current_revision: Arc<Mutex<Result<ConfigRevision, SnapshotStoreError>>>,
}

impl TestSnapshotStore {
    fn new(facts: Result<SnapshotFacts, SnapshotStoreError>) -> Self {
        let current_revision = facts.as_ref().map(facts_revision).map_err(Clone::clone);
        Self {
            facts: Arc::new(Mutex::new(facts)),
            current_revision: Arc::new(Mutex::new(current_revision)),
        }
    }
}

impl SnapshotStorePort for TestSnapshotStore {
    fn load_snapshot_facts(&self) -> BoxFuture<'_, Result<SnapshotFacts, SnapshotStoreError>> {
        Box::pin(async move { self.facts.lock().expect("facts lock").clone() })
    }

    fn current_config_revision(&self) -> BoxFuture<'_, Result<ConfigRevision, SnapshotStoreError>> {
        Box::pin(async move { self.current_revision.lock().expect("revision lock").clone() })
    }
}

struct PublishingCatalog {
    generation: AtomicU64,
    queries: AtomicUsize,
}

enum TestCatalog {
    NoProviders,
    Unavailable,
    Empty,
}

impl ProviderCatalogPort for TestCatalog {
    fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        match self {
            Self::NoProviders => BTreeMap::new(),
            Self::Unavailable | Self::Empty => catalog_generations(0),
        }
    }

    fn query_model_capabilities(
        &self,
        _: &ProviderKind,
    ) -> BoxFuture<'_, Result<Vec<ProviderModelCapabilities>, ProviderCatalogUnavailable>> {
        Box::pin(async move {
            match self {
                Self::NoProviders | Self::Unavailable => Err(ProviderCatalogUnavailable),
                Self::Empty => Ok(Vec::new()),
            }
        })
    }
}

fn catalog_generations(generation: u64) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
    BTreeMap::from([(
        ProviderKind::new("alpha").expect("provider"),
        ProviderCatalogGeneration::new(generation),
    )])
}

impl ProviderCatalogPort for PublishingCatalog {
    fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        catalog_generations(self.generation.load(Ordering::SeqCst))
    }

    fn query_model_capabilities(
        &self,
        _: &ProviderKind,
    ) -> BoxFuture<'_, Result<Vec<ProviderModelCapabilities>, ProviderCatalogUnavailable>> {
        Box::pin(async move {
            if self.queries.fetch_add(1, Ordering::SeqCst) == 0 {
                self.generation.store(1, Ordering::SeqCst);
            }
            Ok(vec![
                ProviderModelCapabilities::new(
                    UpstreamModelId::new("upstream-model").expect("model"),
                    ModelCapabilities::new(
                        std::collections::BTreeSet::from([OperationKind::Generate]),
                        None,
                    ),
                )
                .with_presentation(ModelPresentation::new(
                    Some("Upstream Model".to_owned()),
                    None,
                )),
            ])
        })
    }
}

#[test]
fn compiler_should_reject_revision_changed_during_consistent_read() {
    let facts = facts(1, 2);
    let compiler = compiler(Arc::new(TestSnapshotStore::new(Ok(facts))));

    let error = block_on(compiler.compile()).expect_err("revision drift must fail closed");

    assert_eq!(error, RuntimeSnapshotCompileError::RevisionChanged);
}

#[test]
fn request_location_is_frozen_in_snapshot_and_routing_plan() {
    use gateway_core::account::RequestLocation;

    let location = RequestLocation::default();
    let make_facts = |revision_number, location| {
        SnapshotFacts::new(
            revision(revision_number),
            revision(revision_number),
            SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None)
                .with_request_location(location),
            Vec::new(),
            Vec::new(),
            vec![SnapshotProviderAccountFacts::new(
                ProviderAccountId::new("acct_location").unwrap(),
                "alpha",
            )],
            Vec::new(),
        )
    };
    let store = Arc::new(TestSnapshotStore::new(Ok(make_facts(
        1,
        Some(location.clone()),
    ))));
    let compiler = RuntimeSnapshotCompiler::new(store.clone(), Arc::new(TestCatalog::Unavailable));
    let first = block_on(compiler.compile()).expect("first snapshot");
    let plan = first
        .plan(
            &PublicModelId::new("test-model").unwrap(),
            &super::operation(),
            first.all_account_scope(),
            &gateway_core::routing::RoutingContext::default(),
        )
        .expect("routing plan");
    *store.facts.lock().unwrap() = Ok(make_facts(2, None));
    *store.current_revision.lock().unwrap() = Ok(revision(2));
    let second = block_on(compiler.compile()).expect("updated snapshot");
    assert_eq!(second.request_location(), None);
    assert_eq!(first.request_location(), Some(&location));
    assert_eq!(plan.request_location(), Some(&location));
}

#[test]
fn compiler_should_preserve_passthrough_when_provider_catalog_is_unavailable() {
    let compiler = RuntimeSnapshotCompiler::new(
        Arc::new(TestSnapshotStore::new(Ok(facts(3, 3)))),
        Arc::new(TestCatalog::Unavailable),
    );

    let snapshot = block_on(compiler.compile()).expect("compile snapshot");
    let provider = ProviderKind::new("alpha").expect("provider");

    assert_eq!(snapshot.revision().get(), 3);
    assert!(snapshot.contains_public_model_for_provider(
        &PublicModelId::new("unknown-upstream-model").expect("model"),
        &provider,
    ));
    assert_eq!(snapshot.mapped_model("public-model"), "upstream-model");
    assert_eq!(snapshot.client_policies().count(), 1);
}

#[test]
fn known_empty_catalog_should_report_model_not_found() {
    let compiler = RuntimeSnapshotCompiler::new(
        Arc::new(TestSnapshotStore::new(Ok(facts(3, 3)))),
        Arc::new(TestCatalog::Empty),
    );
    let snapshot = block_on(compiler.compile()).expect("compile empty catalog");
    let error = snapshot
        .plan(
            &PublicModelId::new("public-model").expect("model"),
            &super::operation(),
            snapshot.all_account_scope(),
            &gateway_core::routing::RoutingContext {
                required_provider: Some(ProviderKind::new("alpha").expect("provider")),
                ..gateway_core::routing::RoutingContext::default()
            },
        )
        .expect_err("a successfully published empty catalog proves model absence");

    assert_eq!(
        error,
        gateway_core::error::RoutingError::ModelNotFound {
            model: "public-model".to_owned(),
            mapped_model: "upstream-model".to_owned(),
        },
    );
}

#[test]
fn compiler_retries_when_provider_publishes_catalog_during_compilation() {
    let catalog = Arc::new(PublishingCatalog {
        generation: AtomicU64::new(0),
        queries: AtomicUsize::new(0),
    });
    let compiler = RuntimeSnapshotCompiler::new(
        Arc::new(TestSnapshotStore::new(Ok(facts(3, 3)))),
        catalog.clone(),
    );

    let snapshot = block_on(compiler.compile()).expect("stable catalog snapshot");

    assert_eq!(catalog.queries.load(Ordering::SeqCst), 2);
    assert_eq!(
        snapshot
            .provider_catalog_generations()
            .get(&ProviderKind::new("alpha").expect("provider"))
            .map(|generation| generation.get()),
        Some(1),
    );
    let profiles =
        snapshot.public_model_profiles_for_provider(&ProviderKind::new("alpha").expect("provider"));
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.model().as_str())
            .collect::<Vec<_>>(),
        vec!["public-model", "upstream-model"],
    );
}

#[test]
fn compiler_should_freeze_valid_client_min_versions() {
    let store = Arc::new(TestSnapshotStore::new(Ok(facts_with_min_versions(
        1,
        1,
        Some("26.825.6671".to_owned()),
        Some("0.40.0".to_owned()),
    ))));

    let snapshot = block_on(compiler(store).compile()).expect("valid min versions");

    assert_eq!(
        snapshot
            .min_codex_client_versions()
            .desktop()
            .map(ToString::to_string)
            .as_deref(),
        Some("26.825.6671")
    );
    assert_eq!(
        snapshot
            .min_codex_client_versions()
            .cli()
            .map(ToString::to_string)
            .as_deref(),
        Some("0.40.0")
    );
}

#[test]
fn compiler_should_reject_invalid_persisted_client_min_version() {
    let store = Arc::new(TestSnapshotStore::new(Ok(facts_with_min_versions(
        1,
        1,
        None,
        Some("v0.40.0".to_owned()),
    ))));

    assert_eq!(
        block_on(compiler(store).compile()).expect_err("invalid min version"),
        RuntimeSnapshotCompileError::InvalidData
    );
}

#[test]
fn compiler_should_resolve_account_concurrency_overrides_and_inherited_defaults() {
    let facts = account_concurrency_facts(5, Some(2));
    let compiler = RuntimeSnapshotCompiler::new(
        Arc::new(TestSnapshotStore::new(Ok(facts))),
        Arc::new(TestCatalog::Empty),
    );

    let snapshot = block_on(compiler.compile()).expect("compile account limits");
    let concurrency = snapshot.account_concurrency();
    assert_eq!(concurrency.revision(), snapshot.revision());
    assert_eq!(concurrency.limit_for("acct_inherited"), NonZeroU32::new(5));
    assert_eq!(concurrency.limit_for("acct_overridden"), NonZeroU32::new(2));
    assert_eq!(concurrency.limit_for("absent"), None);
}

#[test]
fn compiler_should_reject_zero_default_or_account_concurrency_override() {
    for (default_limit, override_limit) in [(0, Some(2)), (5, Some(0))] {
        let compiler = RuntimeSnapshotCompiler::new(
            Arc::new(TestSnapshotStore::new(Ok(account_concurrency_facts(
                default_limit,
                override_limit,
            )))),
            Arc::new(TestCatalog::Empty),
        );
        assert_eq!(
            block_on(compiler.compile()).expect_err("zero capacity"),
            RuntimeSnapshotCompileError::InvalidData
        );
    }
}

fn account_concurrency_facts(default_limit: u32, override_limit: Option<u32>) -> SnapshotFacts {
    SnapshotFacts::new(
        revision(1),
        revision(1),
        SnapshotSettingsFacts::new(default_limit, 0, "smart", BTreeMap::new(), None, None),
        Vec::new(),
        Vec::new(),
        vec![
            SnapshotProviderAccountFacts::new(
                ProviderAccountId::new("acct_inherited").expect("account ID"),
                "alpha",
            ),
            SnapshotProviderAccountFacts::new(
                ProviderAccountId::new("acct_overridden").expect("account ID"),
                "alpha",
            )
            .with_concurrency_limit(override_limit),
        ],
        Vec::new(),
    )
}

fn facts(config_revision: u64, observed_current_revision: u64) -> SnapshotFacts {
    facts_with_min_versions(config_revision, observed_current_revision, None, None)
}

fn facts_with_min_versions(
    config_revision: u64,
    observed_current_revision: u64,
    desktop: Option<String>,
    cli: Option<String>,
) -> SnapshotFacts {
    SnapshotFacts::new(
        revision(config_revision),
        revision(observed_current_revision),
        SnapshotSettingsFacts::new(
            3,
            50,
            "smart",
            BTreeMap::from([("public-model".to_owned(), "upstream-model".to_owned())]),
            desktop,
            cli,
        ),
        vec![SnapshotClientPolicyFacts::new(
            ClientApiKeyId::new("key_one").expect("key ID"),
            PlaintextClientApiKey::new("sk_test").expect("plaintext key"),
            Vec::new(),
            RateLimits::unlimited(),
        )],
        Vec::<SnapshotAccountGroupFacts>::new(),
        Vec::<SnapshotProviderAccountFacts>::new(),
        Vec::<SnapshotAccountGroupMemberFacts>::new(),
    )
}

fn facts_revision(facts: &SnapshotFacts) -> ConfigRevision {
    facts.config_revision()
}

fn compiler(store: Arc<dyn SnapshotStorePort>) -> RuntimeSnapshotCompiler {
    RuntimeSnapshotCompiler::new(store, Arc::new(TestCatalog::NoProviders))
}

fn revision(value: u64) -> ConfigRevision {
    ConfigRevision::new(value).expect("positive revision")
}
