//! 隐私策略的数据合同，以及快照与请求计划中的冻结、复用及关闭行为

use super::*;
use gateway_core::settings::privacy::*;

#[derive(Debug, Default)]
struct Compiler(AtomicUsize);

#[derive(Debug)]
struct Compiled(serde_json::Value);

impl PrivacyPolicyCompiler for Compiler {
    fn compile(
        &self,
        policy: &CodexPrivacyPolicy,
    ) -> Result<Arc<dyn CompiledPrivacyPolicy>, PrivacyError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if policy.rules[0].steps[0].value == "invalid" {
            return Err(PrivacyError {
                rule_index: 0,
                reason: "synthetic invalid policy",
            });
        }
        Ok(Arc::new(Compiled(policy.rules[0].steps[0].value.clone())))
    }
}

impl CompiledPrivacyPolicy for Compiled {
    fn apply(
        &self,
        body: &mut serde_json::Value,
        _: &mut http::HeaderMap,
        _: &mut Option<String>,
        _: Option<&str>,
        _: &dyn Fn() -> bool,
    ) -> Result<Vec<PrivacyRuleOutcome>, PrivacyError> {
        *body = self.0.clone();
        Ok(Vec::new())
    }
}

fn policy(value: &str) -> CodexPrivacyPolicy {
    CodexPrivacyPolicy {
        enabled: true,
        on_error: PrivacyFailureMode::RejectRequest,
        rules: vec![PrivacyRule {
            id: "rule".into(),
            name: "规则".into(),
            enabled: true,
            steps: vec![PrivacyStep {
                scope: PrivacyScope::RequestBody,
                selector: "$.metadata".into(),
                action: PrivacyAction::SetValue,
                pattern: None,
                replacement: String::new(),
                value: serde_json::json!(value),
                replace_all: false,
                case_insensitive: false,
                multi_line: false,
            }],
        }],
    }
}

#[test]
fn privacy_rule_requires_steps_and_rejects_unknown_fields() {
    let expected = policy("user").rules.remove(0);
    let value = serde_json::to_value(&expected).unwrap();
    assert_eq!(
        serde_json::from_value::<PrivacyRule>(value.clone()).unwrap(),
        expected
    );

    let mut missing_steps = value.clone();
    missing_steps.as_object_mut().unwrap().remove("steps");
    assert!(serde_json::from_value::<PrivacyRule>(missing_steps).is_err());
    let mut unknown_rule_field = value.clone();
    unknown_rule_field["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PrivacyRule>(unknown_rule_field).is_err());
    let mut unknown_step_field = value;
    unknown_step_field["steps"][0]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PrivacyRule>(unknown_step_field).is_err());
}

fn configured_facts(version: u64, policy: CodexPrivacyPolicy) -> SnapshotFacts {
    SnapshotFacts::new(
        revision(version),
        revision(version),
        SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None)
            .with_codex_privacy_policy(policy),
        Vec::new(),
        Vec::new(),
        vec![SnapshotProviderAccountFacts::new(
            ProviderAccountId::new("acct_fixture").unwrap(),
            "audit",
        )],
        Vec::new(),
    )
}

fn compiled(snapshot: &RuntimeSnapshot) -> Option<Arc<dyn CompiledPrivacyPolicy>> {
    use gateway_core::operation::{ImageRequest, ImageRequestKind, Operation, RawJsonPayload};
    let operation = Operation::GenerateImage(ImageRequest::from_raw_json(
        ImageRequestKind::Generation,
        RawJsonPayload::new("openai", bytes::Bytes::from_static(br#"{"prompt":"draw"}"#)).unwrap(),
    ));
    snapshot
        .plan_provider_endpoint(
            &ProviderKind::new("audit").unwrap(),
            None,
            &operation,
            snapshot.all_account_scope(),
            &Default::default(),
        )
        .unwrap()
        .privacy()
}

#[test]
fn privacy_publication_reuses_unchanged_rules_and_freezes_existing_plans() {
    block_on(async {
        let store = Arc::new(TestSnapshotStore::new(Ok(configured_facts(
            1,
            policy("first"),
        ))));
        let rules = Arc::new(Compiler::default());
        let compiler = Arc::new(compiler(store.clone()).with_privacy_compiler(rules.clone()));
        let handle = RuntimeSnapshotHandle::new(compiler.compile().await.unwrap());
        let original = compiled(&handle.acquire().unwrap()).unwrap();
        let publisher = RuntimeSnapshotPublisher::new(
            compiler,
            handle.clone(),
            Arc::new(TestSnapshotSubscriptions::default()),
        );
        for (version, value) in [(2, "first"), (3, "second")] {
            *store.facts.lock().unwrap() = Ok(configured_facts(version, policy(value)));
            *store.current_revision.lock().unwrap() = Ok(revision(version));
            publisher.publish_committed(revision(version)).await;
            let current = compiled(&handle.acquire().unwrap()).unwrap();
            assert_eq!(Arc::ptr_eq(&original, &current), value == "first");
            let mut body = serde_json::Value::Null;
            current
                .apply(
                    &mut body,
                    &mut http::HeaderMap::new(),
                    &mut None,
                    None,
                    &|| false,
                )
                .unwrap();
            assert_eq!(body, value);
        }
        assert_eq!(rules.0.load(Ordering::SeqCst), 2);
        let mut body = serde_json::Value::Null;
        original
            .apply(
                &mut body,
                &mut http::HeaderMap::new(),
                &mut None,
                None,
                &|| false,
            )
            .unwrap();
        assert_eq!(body, "first");
    });
}

#[test]
fn privacy_disabled_rules_skip_outbound_and_invalid_rules_fail_closed() {
    block_on(async {
        for disabled in [false, true] {
            let mut config = policy("unused");
            if disabled {
                config.rules[0].enabled = false;
            } else {
                config.enabled = false;
            }
            let rules = Arc::new(Compiler::default());
            let snapshot = compiler(Arc::new(TestSnapshotStore::new(Ok(configured_facts(
                1, config,
            )))))
            .with_privacy_compiler(rules.clone())
            .compile()
            .await
            .unwrap();
            assert!(compiled(&snapshot).is_none());
            assert_eq!(rules.0.load(Ordering::SeqCst), 1);
        }
        let store = Arc::new(TestSnapshotStore::new(Ok(configured_facts(
            1,
            policy("invalid"),
        ))));
        assert!(matches!(
            compiler(store.clone()).compile().await,
            Err(RuntimeSnapshotCompileError::InvalidData)
        ));
        assert!(matches!(
            compiler(store)
                .with_privacy_compiler(Arc::new(Compiler::default()))
                .compile()
                .await,
            Err(RuntimeSnapshotCompileError::InvalidData)
        ));
    });
}
