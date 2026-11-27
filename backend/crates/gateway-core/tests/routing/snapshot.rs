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
    Discovery,
}

enum MixedCatalog {
    Available,
    Unavailable,
}

impl ProviderCatalogPort for MixedCatalog {
    fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        ["openai", "xai"]
            .into_iter()
            .map(|provider| {
                (
                    ProviderKind::new(provider).unwrap(),
                    ProviderCatalogGeneration::new(1),
                )
            })
            .collect()
    }

    fn model_catalog_is_exhaustive(&self, provider: &ProviderKind) -> bool {
        provider.as_str() != "openai"
    }

    fn query_model_capabilities(
        &self,
        provider: &ProviderKind,
    ) -> BoxFuture<'_, Result<Vec<ProviderModelCapabilities>, ProviderCatalogUnavailable>> {
        if matches!(self, Self::Unavailable) {
            return Box::pin(async { Err(ProviderCatalogUnavailable) });
        }
        let model = match provider.as_str() {
            "openai" => "gpt-5.5",
            "xai" => "grok-4.7",
            _ => return Box::pin(async { Err(ProviderCatalogUnavailable) }),
        };
        Box::pin(async move {
            Ok([model, "shared-model"]
                .into_iter()
                .map(|model| {
                    ProviderModelCapabilities::new(
                        UpstreamModelId::new(model).unwrap(),
                        ModelCapabilities::new([OperationKind::Generate].into(), None),
                    )
                })
                .collect())
        })
    }
}

fn mixed_catalog_snapshot() -> gateway_core::routing::RuntimeSnapshot {
    mixed_catalog_snapshot_with(MixedCatalog::Available)
}

fn mixed_catalog_snapshot_with(catalog: MixedCatalog) -> gateway_core::routing::RuntimeSnapshot {
    let facts = SnapshotFacts::new(
        revision(1),
        revision(1),
        SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None),
        vec![],
        vec![],
        ["openai", "xai"]
            .into_iter()
            .map(|provider| {
                SnapshotProviderAccountFacts::new(
                    ProviderAccountId::new(format!("acct_{provider}")).unwrap(),
                    provider,
                )
            })
            .collect(),
        vec![],
    );
    block_on(
        RuntimeSnapshotCompiler::new(
            Arc::new(TestSnapshotStore::new(Ok(facts))),
            Arc::new(catalog),
        )
        .compile(),
    )
    .unwrap()
}

impl ProviderCatalogPort for TestCatalog {
    fn model_catalog_is_exhaustive(&self, _: &ProviderKind) -> bool {
        !matches!(self, Self::Discovery)
    }

    fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        match self {
            Self::NoProviders => BTreeMap::new(),
            Self::Unavailable | Self::Empty | Self::Discovery => catalog_generations(0),
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
                Self::Discovery => Ok(vec![ProviderModelCapabilities::new(
                    UpstreamModelId::new("listed-model").unwrap(),
                    ModelCapabilities::new(
                        std::collections::BTreeSet::from([OperationKind::Generate]),
                        None,
                    ),
                )]),
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
fn discovered_models_require_an_authorized_discovering_account() {
    use gateway_core::routing::{
        ClientRoutingScope, FrozenAccountScope, RuntimeAccount, RuntimeAccountDirectory,
    };
    struct ScopedCatalog;
    impl ProviderCatalogPort for ScopedCatalog {
        fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
            catalog_generations(1)
        }
        fn model_catalog_is_exhaustive(&self, _: &ProviderKind) -> bool {
            false
        }
        fn query_model_capabilities(
            &self,
            _: &ProviderKind,
        ) -> BoxFuture<'_, Result<Vec<ProviderModelCapabilities>, ProviderCatalogUnavailable>>
        {
            Box::pin(async {
                Ok(vec![
                    ProviderModelCapabilities::new(
                        UpstreamModelId::new("private-model").unwrap(),
                        ModelCapabilities::new([OperationKind::Generate].into(), None),
                    )
                    .with_presentation(ModelPresentation::new(Some("Private model".into()), None))
                    .with_catalog_accounts([ProviderAccountId::new("acct_source").unwrap()].into()),
                ])
            })
        }
    }
    let facts = SnapshotFacts::new(
        revision(1),
        revision(1),
        SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None),
        vec![],
        vec![],
        vec![
            SnapshotProviderAccountFacts::new(
                ProviderAccountId::new("acct_source").unwrap(),
                "alpha",
            ),
            SnapshotProviderAccountFacts::new(
                ProviderAccountId::new("acct_other").unwrap(),
                "alpha",
            ),
        ],
        vec![],
    );
    let snapshot = block_on(
        RuntimeSnapshotCompiler::new(
            Arc::new(TestSnapshotStore::new(Ok(facts))),
            Arc::new(ScopedCatalog),
        )
        .compile(),
    )
    .unwrap();
    for (id, visible) in [("acct_source", true), ("acct_other", false)] {
        let scope = Arc::new(FrozenAccountScope::new(
            Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([(
                ProviderAccountId::new(id).unwrap(),
                RuntimeAccount::new(ProviderKind::new("alpha").unwrap(), Default::default()),
            )]))),
            ClientRoutingScope::all_accounts(),
        ));
        let model = PublicModelId::new("private-model").unwrap();
        assert_eq!(
            snapshot.public_models_for_scope(&scope).contains(&model),
            visible
        );
        assert_eq!(
            snapshot.contains_public_model_for_scope(&model, &scope),
            visible
        );
        assert_eq!(
            !snapshot.public_model_profiles_for_scope(&scope).is_empty(),
            visible
        );
        assert!(
            snapshot
                .plan(&model, &super::operation(), scope, &Default::default())
                .is_ok(),
            "discovery must not become an inference allowlist"
        );
    }
}

#[test]
fn state_probe_concurrency_defaults_and_snapshot_bounds_are_enforced() {
    assert_eq!(
        gateway_core::routing::OpenAiTurnStatePolicy::default().probe_concurrency(),
        3
    );
    for value in [None, Some(1), Some(3), Some(10), Some(0), Some(11)] {
        let mut settings = SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None);
        if let Some(value) = value {
            settings = settings.with_turn_state_probe_concurrency(value);
        }
        let facts = SnapshotFacts::new(
            revision(1),
            revision(1),
            settings,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let result = block_on(compiler(Arc::new(TestSnapshotStore::new(Ok(facts)))).compile())
            .map(|snapshot| snapshot.openai_turn_state_policy().probe_concurrency());
        let expected = match value {
            Some(0 | 11) => Err(RuntimeSnapshotCompileError::InvalidData),
            value => Ok(value.unwrap_or(3)),
        };
        assert_eq!(result, expected, "{value:?}");
    }
}

#[test]
fn compiler_should_reject_revision_changed_during_consistent_read() {
    let store = Arc::new(SequencedSnapshotStore {
        facts: vec![Ok(facts(1, 2))],
        loads: AtomicUsize::new(0),
    });
    let compiler = compiler(store.clone());

    let error = block_on(compiler.compile()).expect_err("revision drift must fail closed");

    assert_eq!(error, RuntimeSnapshotCompileError::RevisionChanged);
    assert_eq!(store.loads.load(Ordering::SeqCst), 4);
}

struct SequencedSnapshotStore {
    facts: Vec<Result<SnapshotFacts, SnapshotStoreError>>,
    loads: AtomicUsize,
}

impl SnapshotStorePort for SequencedSnapshotStore {
    fn load_snapshot_facts(&self) -> BoxFuture<'_, Result<SnapshotFacts, SnapshotStoreError>> {
        Box::pin(async move {
            let index = self.loads.fetch_add(1, Ordering::SeqCst);
            self.facts[index.min(self.facts.len() - 1)].clone()
        })
    }

    fn current_config_revision(&self) -> BoxFuture<'_, Result<ConfigRevision, SnapshotStoreError>> {
        Box::pin(async move {
            self.facts
                .last()
                .unwrap()
                .as_ref()
                .map(facts_revision)
                .map_err(Clone::clone)
        })
    }
}

#[test]
fn compiler_reloads_complete_facts_after_revision_changes_during_read() {
    let store = Arc::new(SequencedSnapshotStore {
        facts: vec![Ok(facts(1, 2)), Ok(facts(2, 2))],
        loads: AtomicUsize::new(0),
    });
    let snapshot = block_on(compiler(store.clone()).compile()).expect("latest consistent facts");
    assert_eq!(snapshot.revision(), revision(2));
    assert_eq!(store.loads.load(Ordering::SeqCst), 2);
}

#[test]
fn compiler_does_not_hide_storage_failure_after_a_revision_race() {
    let store = Arc::new(SequencedSnapshotStore {
        facts: vec![Ok(facts(1, 2)), Err(SnapshotStoreError)],
        loads: AtomicUsize::new(0),
    });
    assert_eq!(
        block_on(compiler(store.clone()).compile()).unwrap_err(),
        RuntimeSnapshotCompileError::StoreUnavailable
    );
    assert_eq!(store.loads.load(Ordering::SeqCst), 2);
}

#[test]
fn compiler_keeps_model_policy_when_applying_group_membership() {
    use gateway_core::account::{AccountModelAccess, AccountModelAccessMode};
    let group =
        gateway_core::routing::AccountGroupId::new("grp_00000000000000000000000000000001").unwrap();
    let account = ProviderAccountId::new("acct_model_group").unwrap();
    let facts = SnapshotFacts::new(
        revision(1),
        revision(1),
        SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None),
        Vec::new(),
        vec![SnapshotAccountGroupFacts::new(
            group.clone(),
            "Model group".into(),
            true,
        )],
        vec![
            SnapshotProviderAccountFacts::new(account.clone(), "alpha").with_model_access(
                AccountModelAccess::new(AccountModelAccessMode::Allowlist, vec!["model-a".into()])
                    .unwrap(),
            ),
        ],
        vec![SnapshotAccountGroupMemberFacts::new(group, account.clone())],
    );
    let snapshot = block_on(
        RuntimeSnapshotCompiler::new(
            Arc::new(TestSnapshotStore::new(Ok(facts))),
            Arc::new(TestCatalog::Unavailable),
        )
        .compile(),
    )
    .unwrap();
    let scope = snapshot.all_account_scope();
    assert!(scope.allows(&account));
    assert!(scope.allows_model(&account, "model-a"));
    assert!(!scope.allows_model(&account, "model-b"));
}

#[test]
fn scheduling_settings_are_frozen_in_existing_plans() {
    use gateway_core::account::scheduling::SmartSchedulingConfig;
    use gateway_core::routing::RequestTuning;
    let make = |number, config, reset_weights| {
        SnapshotFacts::new(
            revision(number),
            revision(number),
            SnapshotSettingsFacts::new(3, 0, "sticky", BTreeMap::new(), None, None)
                .with_request_tuning(RequestTuning {
                    smart_scheduling: config,
                    ..Default::default()
                })
                .with_quota_reset_window_weights(reset_weights),
            vec![],
            vec![],
            vec![SnapshotProviderAccountFacts::new(
                ProviderAccountId::new("acct_frozen").unwrap(),
                "alpha",
            )],
            vec![],
        )
    };
    let original = SmartSchedulingConfig::default();
    let updated = SmartSchedulingConfig::new([0.0, 2.0, 0.0, 0.0, 0.0, 1.0], true).unwrap();
    let store = Arc::new(TestSnapshotStore::new(Ok(make(
        1,
        original,
        Default::default(),
    ))));
    let compiler = RuntimeSnapshotCompiler::new(store.clone(), Arc::new(TestCatalog::Unavailable));
    let first = block_on(compiler.compile()).unwrap();
    let plan = first
        .plan(
            &PublicModelId::new("new-model").unwrap(),
            &super::operation(),
            first.all_account_scope(),
            &Default::default(),
        )
        .unwrap();
    *store.facts.lock().unwrap() = Ok(make(
        2,
        updated,
        gateway_core::account::ResetWindowWeights::new([1.0, 0.0, 2.0]).unwrap(),
    ));
    *store.current_revision.lock().unwrap() = Ok(revision(2));
    let second = block_on(compiler.compile()).unwrap();
    assert_eq!(second.request_tuning().smart_scheduling, updated);
    assert_eq!(plan.account_selection_policy().smart_scheduling(), original);
    assert_eq!(plan.request_tuning().smart_scheduling, original);
    assert_eq!(
        plan.account_selection_policy().quota_reset_window_weights(),
        Default::default()
    );
    let next = second
        .plan(
            &PublicModelId::new("new-model").unwrap(),
            &super::operation(),
            second.all_account_scope(),
            &Default::default(),
        )
        .unwrap();
    assert_eq!(
        next.account_selection_policy().quota_reset_window_weights(),
        gateway_core::account::ResetWindowWeights::new([1.0, 0.0, 2.0]).unwrap()
    );
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
fn discovery_catalog_does_not_reject_an_unlisted_or_mapped_model() {
    use gateway_core::routing::{RuntimeAccount, RuntimeAccountDirectory};
    let compiler = RuntimeSnapshotCompiler::new(
        Arc::new(TestSnapshotStore::new(Ok(facts(3, 3)))),
        Arc::new(TestCatalog::Discovery),
    );
    let snapshot = block_on(compiler.compile())
        .unwrap()
        .with_account_directory(Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([(
            ProviderAccountId::new("acct_alpha").unwrap(),
            RuntimeAccount::new(ProviderKind::new("alpha").unwrap(), Default::default()),
        )]))));
    let provider = ProviderKind::new("alpha").unwrap();
    for name in ["unlisted-model", "public-model"] {
        let model = PublicModelId::new(name).unwrap();
        assert!(snapshot.contains_public_model_for_provider(&model, &provider));
        snapshot
            .plan(
                &model,
                &super::operation(),
                snapshot.all_account_scope(),
                &gateway_core::routing::RoutingContext {
                    required_provider: Some(provider.clone()),
                    ..gateway_core::routing::RoutingContext::default()
                },
            )
            .expect("upstream judges undiscovered models");
        assert!(
            snapshot
                .plan(
                    &model,
                    &super::operation(),
                    snapshot.all_account_scope(),
                    &gateway_core::routing::RoutingContext {
                        required_provider: Some(provider.clone()),
                        blocked_providers: std::collections::BTreeSet::from([provider.clone()]),
                    },
                )
                .is_err(),
            "discovery does not bypass blocked providers"
        );
    }
}

#[test]
fn known_models_keep_declared_ownership_and_unique_unknown_passthrough() {
    let snapshot = mixed_catalog_snapshot();
    for (model, expected) in [
        ("grok-4.7", vec!["xai"]),
        ("shared-model", vec!["openai", "xai"]),
        ("unlisted-model", vec!["openai"]),
    ] {
        let plan = snapshot
            .plan(
                &PublicModelId::new(model).unwrap(),
                &super::operation(),
                snapshot.all_account_scope(),
                &Default::default(),
            )
            .unwrap();
        assert_eq!(
            plan.candidates()
                .iter()
                .map(|candidate| candidate.provider().as_str())
                .collect::<Vec<_>>(),
            expected,
        );
    }

    let error = snapshot
        .plan(
            &PublicModelId::new("grok-4.7").unwrap(),
            &super::operation(),
            snapshot.all_account_scope(),
            &gateway_core::routing::RoutingContext {
                required_provider: Some(ProviderKind::new("openai").unwrap()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        gateway_core::error::RoutingError::NoCapableProvider { .. }
    ));
}

#[test]
fn known_model_ownership_survives_mapping_and_owner_restrictions() {
    use gateway_core::account::{AccountModelAccess, AccountModelAccessMode};
    use gateway_core::routing::{
        ClientRoutingScope, FrozenAccountScope, RoutingContext, RuntimeAccount,
        RuntimeAccountDirectory,
    };
    let snapshot = mixed_catalog_snapshot().with_model_mappings(BTreeMap::from([
        ("public-model".to_owned(), "middle-model".to_owned()),
        ("middle-model".to_owned(), "grok-4.7".to_owned()),
    ]));
    let model = PublicModelId::new("public-model").unwrap();
    let xai = ProviderKind::new("xai").unwrap();
    let openai = ProviderKind::new("openai").unwrap();
    assert!(snapshot.contains_public_model_for_provider(&model, &xai));
    assert!(!snapshot.contains_public_model_for_provider(&model, &openai));
    let plan = snapshot
        .plan(
            &model,
            &super::operation(),
            snapshot.all_account_scope(),
            &Default::default(),
        )
        .unwrap();
    assert_eq!(plan.candidates().len(), 1);
    assert_eq!(plan.candidates()[0].provider(), &xai);
    assert_eq!(
        plan.candidates()[0].upstream_model().unwrap().as_str(),
        "grok-4.7"
    );

    for (operation, context) in [
        (
            super::operation(),
            RoutingContext {
                blocked_providers: [xai.clone()].into(),
                ..Default::default()
            },
        ),
        (super::image_operation(), RoutingContext::default()),
    ] {
        assert!(matches!(
            snapshot.plan(&model, &operation, snapshot.all_account_scope(), &context),
            Err(gateway_core::error::RoutingError::NoCapableProvider { .. })
        ));
    }
    for deny_owner in [false, true] {
        let mut accounts = BTreeMap::from([(
            ProviderAccountId::new("acct_openai").unwrap(),
            RuntimeAccount::new(openai.clone(), Default::default()),
        )]);
        if deny_owner {
            accounts.insert(
                ProviderAccountId::new("acct_xai").unwrap(),
                RuntimeAccount::new(xai.clone(), Default::default()).with_model_access(
                    AccountModelAccess::new(
                        AccountModelAccessMode::Denylist,
                        vec!["grok-4.7".to_owned()],
                    )
                    .unwrap(),
                ),
            );
        }
        let scope = Arc::new(FrozenAccountScope::new(
            Arc::new(RuntimeAccountDirectory::new(accounts)),
            ClientRoutingScope::all_accounts(),
        ));
        assert!(!snapshot.contains_public_model_for_scope(&model, &scope));
        for required_provider in [None, Some(xai.clone()), Some(openai.clone())] {
            assert!(matches!(
                snapshot.plan(
                    &model,
                    &super::operation(),
                    scope.clone(),
                    &RoutingContext {
                        required_provider,
                        ..Default::default()
                    }
                ),
                Err(gateway_core::error::RoutingError::NoCapableProvider { .. })
            ));
        }
    }
}

#[test]
fn unknown_models_require_one_eligible_discovery_provider() {
    use gateway_core::account::{AccountModelAccess, AccountModelAccessMode};
    use gateway_core::routing::{
        ClientRoutingScope, FrozenAccountScope, RoutingContext, RuntimeAccount,
        RuntimeAccountDirectory,
    };
    let snapshot = mixed_catalog_snapshot_with(MixedCatalog::Unavailable);
    for provider in ["openai", "xai"] {
        assert!(
            snapshot
                .public_models_for_provider(&ProviderKind::new(provider).unwrap())
                .is_empty()
        );
    }
    let model = PublicModelId::new("future-model").unwrap();
    assert!(matches!(
        snapshot.plan(
            &model,
            &super::operation(),
            snapshot.all_account_scope(),
            &Default::default()
        ),
        Err(gateway_core::error::RoutingError::NoCapableProvider { .. })
    ));
    for provider in ["openai", "xai"] {
        let provider = ProviderKind::new(provider).unwrap();
        let plan = snapshot
            .plan(
                &model,
                &super::operation(),
                snapshot.all_account_scope(),
                &RoutingContext {
                    required_provider: Some(provider.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(plan.candidates().len(), 1);
        assert_eq!(plan.candidates()[0].provider(), &provider);
    }
    let openai = ProviderKind::new("openai").unwrap();
    let scope = Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
            (
                ProviderAccountId::new("acct_openai").unwrap(),
                RuntimeAccount::new(openai.clone(), Default::default()),
            ),
            (
                ProviderAccountId::new("acct_xai").unwrap(),
                RuntimeAccount::new(ProviderKind::new("xai").unwrap(), Default::default())
                    .with_model_access(
                        AccountModelAccess::new(
                            AccountModelAccessMode::Denylist,
                            vec!["future-model".to_owned()],
                        )
                        .unwrap(),
                    ),
            ),
        ]))),
        ClientRoutingScope::all_accounts(),
    ));
    let plan = snapshot
        .plan(
            &model,
            &super::operation(),
            scope.clone(),
            &Default::default(),
        )
        .unwrap();
    assert_eq!(plan.candidates().len(), 1);
    assert_eq!(plan.candidates()[0].provider(), &openai);
    assert!(
        snapshot
            .plan(
                &model,
                &super::operation(),
                scope,
                &RoutingContext {
                    required_provider: Some(ProviderKind::new("xai").unwrap()),
                    ..Default::default()
                }
            )
            .is_err()
    );
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

#[test]
fn decompression_limit_is_frozen_and_rejects_zero_or_oversized_values() {
    for (bytes, expected) in [
        (1_u64, Ok(1_usize)),
        (64 * 1024 * 1024, Ok(64 * 1024 * 1024)),
        (256 * 1024 * 1024, Ok(256 * 1024 * 1024)),
        (0, Err(RuntimeSnapshotCompileError::InvalidData)),
        (
            256 * 1024 * 1024 + 1,
            Err(RuntimeSnapshotCompileError::InvalidData),
        ),
    ] {
        let facts = SnapshotFacts::new(
            revision(1),
            revision(1),
            SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None)
                .with_responses_max_decompressed_body_bytes(bytes),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let result = block_on(compiler(Arc::new(TestSnapshotStore::new(Ok(facts)))).compile())
            .map(|snapshot| snapshot.responses_max_decompressed_body_bytes());
        assert_eq!(result, expected, "{bytes}");
    }
}

#[test]
fn fast_policy_uses_bound_groups_without_changing_scope() {
    let group_id =
        gateway_core::routing::AccountGroupId::new("grp_00000000000000000000000000000001").unwrap();
    let account_id = ProviderAccountId::new("acct_fast").unwrap();
    for (group, expected) in [(false, false), (true, true)] {
        let facts = SnapshotFacts::new(
            revision(1),
            revision(1),
            SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None),
            vec![SnapshotClientPolicyFacts::new(
                ClientApiKeyId::new("key_fast").unwrap(),
                PlaintextClientApiKey::new("sk_fast").unwrap(),
                vec![group_id.clone()],
                RateLimits::unlimited(),
            )],
            vec![
                SnapshotAccountGroupFacts::new(group_id.clone(), "Fast policy".to_owned(), true)
                    .with_disable_fast(group),
            ],
            vec![SnapshotProviderAccountFacts::new(
                account_id.clone(),
                "alpha",
            )],
            vec![SnapshotAccountGroupMemberFacts::new(
                group_id.clone(),
                account_id.clone(),
            )],
        );
        let snapshot = block_on(
            RuntimeSnapshotCompiler::new(
                Arc::new(TestSnapshotStore::new(Ok(facts))),
                Arc::new(TestCatalog::Unavailable),
            )
            .compile(),
        )
        .expect("snapshot");
        let policy = snapshot.client_policies().next().expect("client policy");
        assert!(policy.account_scope().allows(&account_id));
        let plan = snapshot
            .plan(
                &PublicModelId::new("unlisted-model").unwrap(),
                &super::operation(),
                Arc::clone(policy.account_scope()),
                &gateway_core::routing::RoutingContext::default(),
            )
            .expect("routing plan");
        assert_eq!(plan.account_scope().disable_fast(), expected);
    }
}

#[test]
fn image_generation_policy_includes_disabled_groups_without_expanding_account_scope() {
    use gateway_core::routing::AccountGroupId;
    let group = AccountGroupId::new("grp_00000000000000000000000000000001").unwrap();
    let denied = AccountGroupId::new("grp_00000000000000000000000000000002").unwrap();
    let account = ProviderAccountId::new("acct_image_policy").unwrap();
    for allowed in [false, true] {
        let facts = SnapshotFacts::new(
            revision(1),
            revision(1),
            SnapshotSettingsFacts::new(5, 0, "smart", BTreeMap::new(), None, None),
            vec![SnapshotClientPolicyFacts::new(
                ClientApiKeyId::new("key_images").unwrap(),
                PlaintextClientApiKey::new("sk_images").unwrap(),
                vec![group.clone(), denied.clone()],
                RateLimits::unlimited(),
            )],
            vec![
                SnapshotAccountGroupFacts::new(group.clone(), "Enabled".into(), true),
                SnapshotAccountGroupFacts::new(denied.clone(), "Disabled policy".into(), false)
                    .with_image_generation(allowed),
            ],
            vec![SnapshotProviderAccountFacts::new(account.clone(), "alpha")],
            vec![SnapshotAccountGroupMemberFacts::new(
                group.clone(),
                account.clone(),
            )],
        );
        let snapshot = block_on(
            RuntimeSnapshotCompiler::new(
                Arc::new(TestSnapshotStore::new(Ok(facts))),
                Arc::new(TestCatalog::Unavailable),
            )
            .compile(),
        )
        .unwrap();
        let scope = snapshot.client_policies().next().unwrap().account_scope();
        assert_eq!(scope.allows_image_generation(), allowed);
        assert!(scope.allows(&account));
    }
}

#[test]
fn fast_modes_merge_without_changing_account_scope() {
    use gateway_core::account::FastMode;
    let group_id =
        gateway_core::routing::AccountGroupId::new("grp_00000000000000000000000000000001").unwrap();
    let second_group =
        gateway_core::routing::AccountGroupId::new("grp_00000000000000000000000000000002").unwrap();
    let account_id = ProviderAccountId::new("acct_fast").unwrap();
    for mode in [FastMode::Default, FastMode::Enabled, FastMode::Disabled] {
        let facts = SnapshotFacts::new(
            revision(1),
            revision(1),
            SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None),
            vec![SnapshotClientPolicyFacts::new(
                ClientApiKeyId::new("key_fast").unwrap(),
                PlaintextClientApiKey::new("sk_fast").unwrap(),
                vec![group_id.clone(), second_group.clone()],
                RateLimits::unlimited(),
            )],
            vec![
                SnapshotAccountGroupFacts::new(group_id.clone(), "Fast".to_owned(), true)
                    .with_fast_mode(FastMode::Enabled),
                SnapshotAccountGroupFacts::new(
                    second_group.clone(),
                    "Policy only".to_owned(),
                    false,
                )
                .with_fast_mode(mode),
            ],
            vec![SnapshotProviderAccountFacts::new(
                account_id.clone(),
                "alpha",
            )],
            vec![SnapshotAccountGroupMemberFacts::new(
                group_id.clone(),
                account_id.clone(),
            )],
        );
        let snapshot = block_on(
            RuntimeSnapshotCompiler::new(
                Arc::new(TestSnapshotStore::new(Ok(facts))),
                Arc::new(TestCatalog::Unavailable),
            )
            .compile(),
        )
        .unwrap();
        let scope = snapshot.client_policies().next().unwrap().account_scope();
        assert!(scope.allows(&account_id));
        assert_eq!(scope.fast_mode(), FastMode::Enabled.merge(mode));
        let plan = snapshot
            .plan(
                &PublicModelId::new("unlisted-model").unwrap(),
                &super::operation(),
                Arc::clone(scope),
                &gateway_core::routing::RoutingContext::default(),
            )
            .unwrap();
        assert_eq!(
            plan.account_scope().fast_mode(),
            FastMode::Enabled.merge(mode)
        );
    }
}

#[test]
fn affinity_ttl_and_guardian_policy_are_validated_and_frozen_together() {
    use gateway_core::account::{AccountAffinity, parse_session_affinity_ttl_hours};
    for (hours, affinity) in [
        (1, AccountAffinity::Relaxed),
        (24, AccountAffinity::Strict),
        (720, AccountAffinity::Strict),
    ] {
        let facts = SnapshotFacts::new(
            revision(1),
            revision(1),
            SnapshotSettingsFacts::new(5, 42, "smart", BTreeMap::new(), None, None)
                .with_openai_guardian_reserved_concurrency(2)
                .with_session_affinity_ttl_hours(hours)
                .with_account_affinity(affinity),
            Vec::new(),
            Vec::new(),
            vec![SnapshotProviderAccountFacts::new(
                ProviderAccountId::new("acct_policy").unwrap(),
                "alpha",
            )],
            Vec::new(),
        );
        let snapshot = block_on(
            RuntimeSnapshotCompiler::new(
                Arc::new(TestSnapshotStore::new(Ok(facts))),
                Arc::new(TestCatalog::Unavailable),
            )
            .compile(),
        )
        .unwrap();
        let plan = snapshot
            .plan(
                &PublicModelId::new("unlisted-model").unwrap(),
                &super::operation(),
                snapshot.all_account_scope(),
                &gateway_core::routing::RoutingContext::default(),
            )
            .unwrap();
        let policy = plan.account_selection_policy();
        assert_eq!(
            policy.session_affinity_ttl(),
            parse_session_affinity_ttl_hours(hours).unwrap()
        );
        assert_eq!(policy.account_affinity(), affinity);
        assert_eq!(policy.openai_guardian_reserved_concurrency(), 2);
        assert_eq!(
            policy.request_interval(),
            std::time::Duration::from_millis(42)
        );
    }
    for hours in [0, 721, u32::MAX] {
        assert!(parse_session_affinity_ttl_hours(hours).is_none());
        let facts = SnapshotFacts::new(
            revision(1),
            revision(1),
            SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None)
                .with_session_affinity_ttl_hours(hours),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        assert!(matches!(
            block_on(
                RuntimeSnapshotCompiler::new(
                    Arc::new(TestSnapshotStore::new(Ok(facts))),
                    Arc::new(TestCatalog::NoProviders),
                )
                .compile()
            ),
            Err(RuntimeSnapshotCompileError::InvalidData)
        ));
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

#[test]
fn group_model_access_requires_a_common_group_for_account_and_mapped_model() {
    use gateway_core::{
        account::{AccountModelAccess, AccountModelAccessMode, ProviderAccountId},
        error::RoutingError,
        routing::AccountGroupId,
    };
    let group = |n| AccountGroupId::new(format!("grp_{n:032x}")).unwrap();
    let account = |id| ProviderAccountId::new(id).unwrap();
    let policy = |mode, models: &[&str]| {
        AccountModelAccess::new(mode, models.iter().map(|id| (*id).to_owned()).collect()).unwrap()
    };
    let (a, b, shared) = (account("acct_a"), account("acct_b"), account("acct_shared"));
    let (first, second, disabled) = (group(1), group(2), group(3));
    let key = ClientApiKeyId::new("key_groups").unwrap();
    let facts = SnapshotFacts::new(
        revision(1),
        revision(1),
        SnapshotSettingsFacts::new(
            3,
            0,
            "smart",
            [("alias".into(), "listed-model".into())].into(),
            None,
            None,
        ),
        vec![SnapshotClientPolicyFacts::new(
            key.clone(),
            PlaintextClientApiKey::new("sk_groups").unwrap(),
            vec![first.clone(), second.clone(), disabled.clone()],
            RateLimits::unlimited(),
        )],
        vec![
            SnapshotAccountGroupFacts::new(first.clone(), "First".into(), true)
                .with_model_access(policy(AccountModelAccessMode::Allowlist, &["listed-model"])),
            SnapshotAccountGroupFacts::new(second.clone(), "Second".into(), true)
                .with_model_access(policy(
                    AccountModelAccessMode::Denylist,
                    &["listed-model", "blocked"],
                )),
            SnapshotAccountGroupFacts::new(disabled.clone(), "Disabled".into(), false),
        ],
        vec![
            SnapshotProviderAccountFacts::new(a.clone(), "alpha"),
            SnapshotProviderAccountFacts::new(b.clone(), "alpha"),
            SnapshotProviderAccountFacts::new(shared.clone(), "alpha").with_model_access(policy(
                AccountModelAccessMode::Allowlist,
                &["listed-model", "image-model"],
            )),
        ],
        vec![
            SnapshotAccountGroupMemberFacts::new(first.clone(), a.clone()),
            SnapshotAccountGroupMemberFacts::new(second.clone(), b.clone()),
            SnapshotAccountGroupMemberFacts::new(first, shared.clone()),
            SnapshotAccountGroupMemberFacts::new(second.clone(), shared.clone()),
            SnapshotAccountGroupMemberFacts::new(disabled.clone(), b.clone()),
        ],
    );
    let snapshot = block_on(
        RuntimeSnapshotCompiler::new(
            Arc::new(TestSnapshotStore::new(Ok(facts))),
            Arc::new(TestCatalog::Discovery),
        )
        .compile(),
    )
    .unwrap();
    let scope = snapshot
        .client_policies()
        .find(|policy| policy.key_id() == &key)
        .unwrap()
        .account_scope();
    let alias = PublicModelId::new("alias").unwrap();
    let plan = snapshot
        .plan(
            &alias,
            &super::operation(),
            Arc::clone(scope),
            &Default::default(),
        )
        .unwrap();
    let candidate = &plan.candidates()[0];
    assert_eq!(candidate.upstream_model().unwrap().as_str(), "listed-model");
    assert!(candidate.account_scope().allows_model(&a, "listed-model"));
    assert!(!candidate.account_scope().allows_model(&b, "listed-model"));
    assert!(scope.allows_model(&b, "other"));
    assert!(scope.allows_model(&shared, "listed-model"));
    assert!(scope.allows_model(&shared, "image-model"));
    assert!(!scope.allows_model(&shared, "other"));
    assert!(!scope.allows_unidentified_model(&b));
    assert!(scope.groups_deny_model(None));
    assert!(snapshot.contains_public_model_for_scope(&alias, scope));
    let b_only = scope.restricted_to_accounts(&[b.clone()].into());
    assert!(!snapshot.public_models_for_scope(&b_only).contains(&alias));
    assert!(!snapshot.contains_public_model_for_scope(&alias, &b_only));
    let blocked = PublicModelId::new("blocked").unwrap();
    assert!(matches!(
        snapshot.plan(
            &blocked,
            &super::operation(),
            Arc::clone(scope),
            &Default::default()
        ),
        Err(RoutingError::GroupModelDenied)
    ));
    assert!(matches!(
        snapshot.plan_provider_endpoint(
            &ProviderKind::new("alpha").unwrap(),
            Some(&UpstreamModelId::new("blocked").unwrap()),
            &super::operation(),
            Arc::clone(scope),
            &Default::default()
        ),
        Err(RoutingError::GroupModelDenied)
    ));
    assert!(snapshot.all_account_scope().allows_model(&b, "blocked"));
    assert!(snapshot.all_account_scope().allows_unidentified_model(&b));
    assert_eq!(
        snapshot.upstream_models_for_group(&second),
        ["image-model", "listed-model"].map(|id| UpstreamModelId::new(id).unwrap())
    );
    assert_eq!(
        snapshot.upstream_models_for_group(&disabled),
        vec![UpstreamModelId::new("listed-model").unwrap()]
    );
    assert!(snapshot.upstream_models_for_group(&group(4)).is_empty());
}

#[test]
fn group_feature_permissions_merge_independently_with_deny_precedence() {
    use gateway_core::account::ProviderAccountId;
    use gateway_core::routing::AccountGroupId;
    for (allowed, other_allowed) in [(true, true), (true, false), (false, true), (false, false)] {
        for group_enabled in [false, true] {
            for bound in [false, true] {
                let group_id = AccountGroupId::new("grp_00000000000000000000000000000001").unwrap();
                let open_group_id =
                    AccountGroupId::new("grp_00000000000000000000000000000002").unwrap();
                let account_id = ProviderAccountId::new("acct_fast_policy").unwrap();
                let facts = SnapshotFacts::new(
                    revision(1),
                    revision(1),
                    SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None),
                    vec![SnapshotClientPolicyFacts::new(
                        ClientApiKeyId::new("key_fast_policy").unwrap(),
                        PlaintextClientApiKey::new("sk_fast_policy").unwrap(),
                        if bound {
                            vec![group_id.clone(), open_group_id.clone()]
                        } else {
                            Vec::new()
                        },
                        RateLimits::unlimited(),
                    )],
                    vec![
                        SnapshotAccountGroupFacts::new(
                            group_id.clone(),
                            "Restricted".to_owned(),
                            group_enabled,
                        )
                        .with_image_generation(allowed)
                        .with_subagents(!allowed),
                        SnapshotAccountGroupFacts::new(
                            open_group_id.clone(),
                            "Open".to_owned(),
                            true,
                        )
                        .with_image_generation(other_allowed)
                        .with_subagents(!other_allowed),
                    ],
                    vec![SnapshotProviderAccountFacts::new(
                        account_id.clone(),
                        "alpha",
                    )],
                    vec![
                        SnapshotAccountGroupMemberFacts::new(group_id, account_id.clone()),
                        SnapshotAccountGroupMemberFacts::new(open_group_id, account_id.clone()),
                    ],
                );
                let snapshot = block_on(
                    RuntimeSnapshotCompiler::new(
                        Arc::new(TestSnapshotStore::new(Ok(facts))),
                        Arc::new(TestCatalog::Unavailable),
                    )
                    .compile(),
                )
                .unwrap();
                let scope = snapshot
                    .client_policies()
                    .next()
                    .unwrap()
                    .account_scope()
                    .clone();
                assert!(scope.allows(&account_id));
                assert_eq!(
                    scope.allows_subagents(),
                    !bound || (!allowed && !other_allowed)
                );
                assert_eq!(
                    scope.allows_image_generation(),
                    !bound || (allowed && other_allowed),
                    "allowed={allowed}, other={other_allowed}, enabled={group_enabled}, bound={bound}"
                );
            }
        }
    }
}

#[test]
fn direct_reviewer_model_freezes_only_authorized_native_catalog_accounts() {
    use gateway_core::account::{AccountModelAccess, AccountModelAccessMode, ProviderAccountId};
    const TARGET: &str = "codex-auto-review";
    const LUNA: &str = "gpt-5.6-luna";
    struct Catalog;
    impl ProviderCatalogPort for Catalog {
        fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
            [(
                ProviderKind::new("openai").unwrap(),
                ProviderCatalogGeneration::new(1),
            )]
            .into()
        }
        fn query_model_capabilities(
            &self,
            _: &ProviderKind,
        ) -> BoxFuture<'_, Result<Vec<ProviderModelCapabilities>, ProviderCatalogUnavailable>>
        {
            Box::pin(async {
                Ok(vec![
                    ProviderModelCapabilities::new(
                        UpstreamModelId::new(TARGET).unwrap(),
                        super::capabilities(),
                    )
                    .with_catalog_accounts(
                        [
                            "acct_oauth",
                            "acct_oauth_backup",
                            "acct_denied",
                            "acct_blocked",
                            "acct_excel",
                        ]
                        .map(|id| ProviderAccountId::new(id).unwrap())
                        .into(),
                    ),
                    ProviderModelCapabilities::new(
                        UpstreamModelId::new(LUNA).unwrap(),
                        super::capabilities(),
                    ),
                ])
            })
        }
    }
    let facts = SnapshotFacts::new(
        revision(1),
        revision(1),
        SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None),
        vec![],
        vec![],
        [
            "acct_oauth",
            "acct_oauth_backup",
            "acct_denied",
            "acct_blocked",
            "acct_unpublished",
            "acct_excel",
        ]
        .map(|id| {
            let mut account =
                SnapshotProviderAccountFacts::new(ProviderAccountId::new(id).unwrap(), "openai");
            if id == "acct_excel" {
                account = account.with_excel_models([TARGET.to_owned(), LUNA.to_owned()].into());
            }
            if matches!(id, "acct_denied" | "acct_blocked") {
                let allowed = if id == "acct_denied" {
                    LUNA
                } else {
                    "unrelated-model"
                };
                account.with_model_access(
                    AccountModelAccess::new(
                        AccountModelAccessMode::Allowlist,
                        vec![allowed.into()],
                    )
                    .unwrap(),
                )
            } else {
                account
            }
        })
        .into(),
        vec![],
    );
    let snapshot = block_on(
        RuntimeSnapshotCompiler::new(
            Arc::new(TestSnapshotStore::new(Ok(facts))),
            Arc::new(Catalog),
        )
        .compile(),
    )
    .unwrap();
    let plan = |fixed: Option<&str>, owner: Option<&str>| {
        let scope = snapshot.all_account_scope();
        let provider = ProviderKind::new("openai").unwrap();
        let target = UpstreamModelId::new(TARGET).unwrap();
        let fixed = fixed.map(|id| ProviderAccountId::new(id).unwrap());
        let owner = owner.map(|id| ProviderAccountId::new(id).unwrap());
        let scope = match fixed {
            Some(id) => Arc::new(scope.restricted_to_accounts(&[id].into())),
            None => scope,
        };
        snapshot.plan_with_direct_account_model(
            &PublicModelId::new(LUNA).unwrap(),
            &super::operation(),
            scope,
            &Default::default(),
            &provider,
            &target,
            owner.as_ref(),
        )
    };
    let oauth_accounts = &["acct_oauth", "acct_oauth_backup"][..];
    for (fixed, owner, expected_model, expected_accounts) in [
        (None, None, TARGET, oauth_accounts),
        (None, Some("acct_oauth"), TARGET, oauth_accounts),
        (Some("acct_oauth"), None, TARGET, &["acct_oauth"][..]),
        (Some("acct_excel"), None, LUNA, &["acct_excel"][..]),
        (
            None,
            Some("acct_excel"),
            LUNA,
            &[
                "acct_oauth",
                "acct_oauth_backup",
                "acct_denied",
                "acct_unpublished",
                "acct_excel",
            ][..],
        ),
    ] {
        let frozen = plan(fixed, owner).unwrap();
        assert_eq!(
            frozen.candidates().len(),
            1,
            "retry must retain the selected model"
        );
        let candidate = &frozen.candidates()[0];
        assert_eq!(
            candidate.requires_direct_authentication(),
            expected_model == TARGET
        );
        assert_eq!(candidate.upstream_model().unwrap().as_str(), expected_model);
        for id in [
            "acct_excel",
            "acct_oauth",
            "acct_oauth_backup",
            "acct_denied",
            "acct_unpublished",
            "acct_blocked",
        ] {
            assert_eq!(
                candidate
                    .account_scope()
                    .allows_model(&ProviderAccountId::new(id).unwrap(), expected_model),
                expected_accounts.contains(&id)
            );
        }
    }
    for id in ["acct_denied", "acct_unpublished"] {
        let preserved = plan(Some(id), None).unwrap();
        let candidate = &preserved.candidates()[0];
        assert_eq!(candidate.upstream_model().unwrap().as_str(), LUNA);
        assert!(
            candidate
                .account_scope()
                .allows_model(&ProviderAccountId::new(id).unwrap(), LUNA)
        );
    }
    assert!(plan(Some("acct_blocked"), None).is_err());
    let configured = snapshot
        .clone()
        .with_model_mappings([(LUNA.into(), "custom".into())].into());
    assert!(
        configured
            .plan_with_direct_account_model(
                &PublicModelId::new(LUNA).unwrap(),
                &super::operation(),
                configured.all_account_scope(),
                &Default::default(),
                &ProviderKind::new("openai").unwrap(),
                &UpstreamModelId::new(TARGET).unwrap(),
                None,
            )
            .is_err(),
        "explicit unavailable mapping must not be bypassed"
    );
}

#[test]
fn reasoning_groups_resolve_before_account_and_group_model_policies() {
    use gateway_core::{
        account::{AccountModelAccess, AccountModelAccessMode, ProviderAccountId},
        error::RoutingError,
        operation::{
            GenerateRequest, Operation, ProtocolPayload, RawJsonPayload, TokenCountRequest,
        },
        routing::{
            AccountModelMetadata, ClientRoutingScope, FrozenAccountScope, ProviderModel,
            ReasoningModelVariant, RoutingContext, RoutingGroupSnapshot, RuntimeAccount,
            RuntimeAccountDirectory, RuntimeSnapshot,
        },
    };
    use std::collections::BTreeSet;
    let provider = ProviderKind::new("alpha").unwrap();
    let declared = ProviderAccountId::new("acct_declared").unwrap();
    let other = ProviderAccountId::new("acct_other").unwrap();
    let high_only = ProviderAccountId::new("acct_high").unwrap();
    let group = gateway_core::routing::AccountGroupId::new(format!("grp_{:032x}", 1)).unwrap();
    let scope = Arc::new(FrozenAccountScope::new(
        Arc::new(
            RuntimeAccountDirectory::new(BTreeMap::from_iter(
                [declared.clone(), other.clone(), high_only.clone()]
                    .into_iter()
                    .map(|id| {
                        (
                            id,
                            RuntimeAccount::new(provider.clone(), BTreeSet::from([group.clone()])),
                        )
                    }),
            ))
            .with_group_model_access(BTreeMap::from([(
                group.clone(),
                AccountModelAccess::new(
                    AccountModelAccessMode::Allowlist,
                    vec!["family-medium".into()],
                )
                .unwrap(),
            )])),
        ),
        ClientRoutingScope::restricted(
            vec![RoutingGroupSnapshot::new(group.clone(), "Group".into())],
            BTreeSet::from([group]),
            BTreeSet::from([provider.clone()]),
        )
        .unwrap(),
    ));
    let caps = || {
        ModelCapabilities::new(
            BTreeSet::from([OperationKind::Generate, OperationKind::CountTokens]),
            None,
        )
        .with_generation_protocol("genai")
        .with_translated_generation_protocols(["openai"])
        .with_upstream_feature_validation()
    };
    let variants = BTreeMap::from_iter(["medium", "high"].map(|level| {
        (
            level.into(),
            ReasoningModelVariant {
                model: UpstreamModelId::new(format!("family-{level}")).unwrap(),
                accounts: if level == "medium" {
                    BTreeSet::from([declared.clone()])
                } else {
                    BTreeSet::from([declared.clone(), high_only.clone()])
                },
            },
        )
    }));
    let snapshot = RuntimeSnapshot::new(
        revision(1),
        super::scheduling(),
        vec![provider.clone()],
        vec![
            ProviderModel::new(
                provider.clone(),
                UpstreamModelId::new("family").unwrap(),
                caps().with_reasoning_variants(variants),
            )
            .with_presentation(
                ModelPresentation::default()
                    .with_reasoning(Some("high".into()), vec!["medium".into(), "high".into()]),
            ),
            ProviderModel::new(
                provider.clone(),
                UpstreamModelId::new("family-medium").unwrap(),
                caps()
                    .with_dedicated_web_search(BTreeMap::from([(
                        declared.clone(),
                        UpstreamModelId::new("search").unwrap(),
                    )]))
                    .with_account_metadata(BTreeMap::from([
                        (
                            declared.clone(),
                            AccountModelMetadata {
                                agent_listing: Default::default(),
                                context_window_tokens: Some(1000),
                                max_context_window_tokens: Some(1500),
                                image_input: true,
                                description: Some("Medium".into()),
                                max_output_tokens: Some(25),
                            },
                        ),
                        (other.clone(), AccountModelMetadata::default()),
                    ])),
            ),
            ProviderModel::new(
                provider.clone(),
                UpstreamModelId::new("family-high").unwrap(),
                caps().with_account_metadata(BTreeMap::from([
                    (
                        declared.clone(),
                        AccountModelMetadata {
                            max_context_window_tokens: Some(2000),
                            ..Default::default()
                        },
                    ),
                    (
                        high_only.clone(),
                        AccountModelMetadata {
                            agent_listing: Default::default(),
                            context_window_tokens: Some(800),
                            max_context_window_tokens: Some(1800),
                            image_input: true,
                            description: Some("High".into()),
                            max_output_tokens: Some(100),
                        },
                    ),
                ])),
            ),
            ProviderModel::new(
                provider.clone(),
                UpstreamModelId::new("pinned").unwrap(),
                caps(),
            )
            .with_presentation(ModelPresentation::default()),
        ],
        vec![],
    )
    .unwrap()
    .with_model_mappings(BTreeMap::from([
        ("alias".into(), "family".into()),
        ("pinned".into(), "family-medium".into()),
    ]));
    let search_body = serde_json::json!({"input":"query","tools":[{"type":"web_search"}],"tool_choice":"required"});
    let search = Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", search_body.as_object().unwrap().clone()).unwrap(),
    ));
    assert!(matches!(
        snapshot.plan(
            &PublicModelId::new("family-medium").unwrap(),
            &search,
            scope.clone(),
            &RoutingContext::default()
        ),
        Err(RoutingError::GroupModelDenied)
    ));
    let old_account = Arc::new(scope.restricted_to_accounts(&BTreeSet::from([other.clone()])));
    assert!(matches!(
        snapshot.plan(
            &PublicModelId::new("family-medium").unwrap(),
            &search,
            old_account,
            &RoutingContext::default()
        ),
        Err(RoutingError::WebSearchUnavailable)
    ));
    let operation = |level: &str, count: bool| {
        let body = serde_json::json!({"contents":[], "generationConfig":{"thinkingConfig":{"thinkingLevel":level}}});
        if count {
            let body = serde_json::json!({"generateContentRequest": body});
            Operation::CountTokens(
                TokenCountRequest::from_raw_json(
                    RawJsonPayload::new(
                        "genai",
                        bytes::Bytes::from(serde_json::to_vec(&body).unwrap()),
                    )
                    .unwrap(),
                )
                .requiring_native_protocol(),
            )
        } else {
            Operation::Generate(
                GenerateRequest::from_protocol_payload(
                    ProtocolPayload::json_object("genai", body.as_object().unwrap().clone())
                        .unwrap(),
                )
                .requiring_native_protocol(),
            )
        }
    };
    let base = PublicModelId::new("family").unwrap();
    let profiles = snapshot.public_model_profiles_for_scope(&scope);
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.model().as_str())
            .collect::<Vec<_>>(),
        ["alias", "family"]
    );
    for profile in &profiles {
        assert_eq!(profile.presentation().context_window_tokens(), Some(1000));
        assert_eq!(
            profile.presentation().max_context_window_tokens(),
            Some(1500)
        );
        assert!(profile.presentation().image_input());
        assert_eq!(profile.presentation().description(), Some("Medium"));
        assert_eq!(
            profile.presentation().supported_reasoning_efforts(),
            ["medium"]
        );
        assert_eq!(
            profile.presentation().default_reasoning_effort(),
            Some("medium")
        );
    }
    let all = FrozenAccountScope::new(
        Arc::clone(scope.directory()),
        ClientRoutingScope::all_accounts(),
    );
    let mixed = snapshot.public_model_profiles_for_scope(&all);
    assert!(mixed.iter().all(|p| !p.presentation().image_input()
        && p.presentation().context_window_tokens().is_none()
        && p.presentation().max_context_window_tokens() == Some(1500)
        && p.presentation().description().is_none()));
    let only_high_scope = all.restricted_to_accounts(&BTreeSet::from([high_only]));
    let only_high = snapshot.public_model_profiles_for_scope(&only_high_scope);
    assert_eq!(only_high.len(), 2);
    for profile in only_high {
        assert_eq!(
            profile.presentation().supported_reasoning_efforts(),
            ["high"]
        );
        assert_eq!(profile.presentation().context_window_tokens(), Some(800));
        assert_eq!(
            profile.presentation().max_context_window_tokens(),
            Some(1800)
        );
        assert!(profile.presentation().image_input());
        assert_eq!(profile.presentation().description(), Some("High"));
    }
    // 显式输出只约束返回已知上限的账号，旧目录仍能由上游校验
    let output_request = |tokens| {
        let body = serde_json::json!({"contents":[],"generationConfig":{"maxOutputTokens":tokens,"thinkingConfig":{"thinkingLevel":"MEDIUM"}}});
        Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object("genai", body.as_object().unwrap().clone()).unwrap(),
        ))
    };
    assert!(
        snapshot
            .plan(
                &base,
                &output_request(25),
                scope.clone(),
                &RoutingContext::default()
            )
            .is_ok()
    );
    assert!(
        snapshot
            .plan(
                &base,
                &output_request(26),
                scope.clone(),
                &RoutingContext::default()
            )
            .is_err()
    );
    let direct = snapshot
        .plan(
            &PublicModelId::new("family-medium").unwrap(),
            &output_request(26),
            scope.clone(),
            &RoutingContext::default(),
        )
        .unwrap();
    assert!(!direct.candidates()[0].account_scope().allows(&declared));
    assert!(direct.candidates()[0].account_scope().allows(&other));
    for count in [false, true] {
        for name in ["family", "alias"] {
            for level in ["MEDIUM", "medium"] {
                let request = operation(level, count);
                let plan = snapshot
                    .plan(
                        &PublicModelId::new(name).unwrap(),
                        &request,
                        scope.clone(),
                        &RoutingContext::default(),
                    )
                    .unwrap();
                let candidate = &plan.candidates()[0];
                assert_eq!(
                    candidate.upstream_model().unwrap().as_str(),
                    "family-medium"
                );
                assert!(candidate.account_scope().allows(&declared));
                assert!(!candidate.account_scope().allows(&other));
            }
        }
        assert!(matches!(
            snapshot.plan(
                &base,
                &operation("HIGH", count),
                scope.clone(),
                &RoutingContext::default()
            ),
            Err(RoutingError::GroupModelDenied)
        ));
        for level in ["", "unsupported"] {
            assert!(matches!(
                snapshot.plan(
                    &base,
                    &operation(level, count),
                    scope.clone(),
                    &RoutingContext::default()
                ),
                Err(RoutingError::InvalidReasoningVariant)
            ));
        }
        let listed = snapshot.public_models_for_operation(&scope, &operation("", count));
        assert!(listed.contains(&base));
        let other_only = Arc::new(scope.restricted_to_accounts(&BTreeSet::from([other.clone()])));
        assert!(
            !snapshot
                .public_models_for_operation(&other_only, &operation("", count))
                .contains(&base)
        );
        assert!(
            snapshot
                .plan(
                    &base,
                    &operation("MEDIUM", count),
                    other_only,
                    &RoutingContext::default()
                )
                .is_err()
        );
    }
    // 完整型号不受正文中的其他档位重定向
    let direct = PublicModelId::new("family-medium").unwrap();
    assert_eq!(
        snapshot
            .plan(
                &direct,
                &operation("HIGH", false),
                scope.clone(),
                &RoutingContext::default()
            )
            .unwrap()
            .candidates()[0]
            .upstream_model()
            .unwrap()
            .as_str(),
        "family-medium"
    );
    let incompatible = Operation::Generate(
        GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object("other-wire", Default::default()).unwrap(),
        )
        .requiring_native_protocol(),
    );
    assert!(
        !snapshot
            .public_models_for_operation(&scope, &incompatible)
            .contains(&base)
    );
    let blocked = Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
            (
                declared,
                RuntimeAccount::new(provider.clone(), BTreeSet::new()).with_model_access(
                    AccountModelAccess::new(
                        AccountModelAccessMode::Allowlist,
                        vec!["unrelated".into()],
                    )
                    .unwrap(),
                ),
            ),
            (other, RuntimeAccount::new(provider, BTreeSet::new())),
        ]))),
        ClientRoutingScope::all_accounts(),
    ));
    assert!(
        !snapshot
            .public_models_for_operation(&blocked, &operation("", false))
            .contains(&base)
    );
    assert!(
        snapshot
            .public_model_profiles_for_scope(&blocked)
            .is_empty()
    );
    assert!(
        snapshot
            .plan(
                &base,
                &operation("MEDIUM", false),
                blocked,
                &RoutingContext::default()
            )
            .is_err()
    );
}

#[test]
fn dedicated_search_requires_both_models_in_the_same_account_and_group() {
    use gateway_core::{
        account::{AccountModelAccess, AccountModelAccessMode},
        operation::{GenerateRequest, Operation, ProtocolPayload},
        routing::{
            AccountGroupId, AccountModelMetadata, ClientRoutingScope, FrozenAccountScope,
            ProviderModel, RoutingContext, RoutingGroupSnapshot, RuntimeAccount,
            RuntimeAccountDirectory, RuntimeSnapshot,
        },
    };
    use std::collections::BTreeSet;
    let provider = ProviderKind::new("antigravity").unwrap();
    let account = ProviderAccountId::new("acct_search").unwrap();
    let other = ProviderAccountId::new("acct_other").unwrap();
    let original_group = AccountGroupId::new(format!("grp_{:032x}", 1)).unwrap();
    let search_group = AccountGroupId::new(format!("grp_{:032x}", 2)).unwrap();
    let caps = || {
        ModelCapabilities::new([OperationKind::Generate].into(), None)
            .with_generation_protocol("genai")
            .with_translated_generation_protocols(["openai"])
            .with_upstream_feature_validation()
    };
    let snapshot = RuntimeSnapshot::new(
        revision(1),
        super::scheduling(),
        vec![provider.clone()],
        vec![
            ProviderModel::new(
                provider.clone(),
                UpstreamModelId::new("coding").unwrap(),
                caps()
                    .with_account_metadata(BTreeMap::from([
                        (account.clone(), AccountModelMetadata::default()),
                        (other.clone(), AccountModelMetadata::default()),
                    ]))
                    .with_dedicated_web_search(BTreeMap::from([(
                        account.clone(),
                        UpstreamModelId::new("search").unwrap(),
                    )])),
            ),
            ProviderModel::new(
                provider.clone(),
                UpstreamModelId::new("search").unwrap(),
                caps().with_account_metadata(BTreeMap::from([(
                    account.clone(),
                    AccountModelMetadata {
                        max_output_tokens: Some(40),
                        ..Default::default()
                    },
                )])),
            ),
        ],
        vec![],
    )
    .unwrap();
    let access = |models: &[&str]| {
        AccountModelAccess::new(
            AccountModelAccessMode::Allowlist,
            models.iter().map(|v| (*v).to_owned()).collect(),
        )
        .unwrap()
    };
    for together in [false, true] {
        let directory = RuntimeAccountDirectory::new(BTreeMap::from([
            (
                account.clone(),
                RuntimeAccount::new(
                    provider.clone(),
                    [original_group.clone(), search_group.clone()].into(),
                ),
            ),
            (
                other.clone(),
                RuntimeAccount::new(provider.clone(), [original_group.clone()].into()),
            ),
        ]))
        .with_group_model_access(BTreeMap::from([
            (
                original_group.clone(),
                access(if together {
                    &["coding", "search"]
                } else {
                    &["coding"]
                }),
            ),
            (search_group.clone(), access(&["search"])),
        ]));
        let scope = FrozenAccountScope::new(
            Arc::new(directory),
            ClientRoutingScope::restricted(
                vec![
                    RoutingGroupSnapshot::new(original_group.clone(), "Coding".into()),
                    RoutingGroupSnapshot::new(search_group.clone(), "Search".into()),
                ],
                [original_group.clone(), search_group.clone()].into(),
                [provider.clone()].into(),
            )
            .unwrap(),
        );
        for limit in [40, 41] {
            let body = serde_json::json!({"input":"query", "tools":[{"type":"web_search"}], "tool_choice":"required", "max_output_tokens":limit});
            let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object("openai", body.as_object().unwrap().clone()).unwrap(),
            ));
            let plan = snapshot.plan(
                &PublicModelId::new("coding").unwrap(),
                &operation,
                Arc::new(scope.clone()),
                &RoutingContext::default(),
            );
            if !together || limit > 40 {
                assert!(plan.is_err());
                continue;
            }
            let plan = plan.unwrap();
            assert_eq!(plan.candidates().len(), 1);
            let selected = &plan.candidates()[0];
            assert_eq!(selected.upstream_model().unwrap().as_str(), "search");
            assert!(selected.account_scope().allows(&account));
            assert!(!selected.account_scope().allows(&other));
            let fixed_other =
                Arc::new(scope.restricted_to_accounts(&BTreeSet::from([other.clone()])));
            assert!(
                snapshot
                    .plan(
                        &PublicModelId::new("coding").unwrap(),
                        &operation,
                        fixed_other,
                        &RoutingContext::default()
                    )
                    .is_err()
            );
        }
    }
}
