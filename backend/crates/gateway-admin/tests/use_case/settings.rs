use async_trait::async_trait;
use gateway_admin::model::{
    Revision,
    pricing::{PricingChange, PricingSyncPreview, StoredPricing, SyncPricing, UpdatePricing},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use gateway_admin::{
    model::{
        AdminErrorKind, MutationContext,
        settings::{
            AdminApiKey, AdminApiKeyMutation, ReplaceRuntimeSettings, RequestTuningOverrides,
            RotationStrategy, RuntimeSettings,
        },
    },
    ports::store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult, SettingsStore},
};

struct UnusedSettingsStore;

#[async_trait]
impl SettingsStore for UnusedSettingsStore {
    async fn load_runtime_settings(&self) -> AdminStoreResult<RuntimeSettings> {
        Err(unused())
    }

    async fn admin_api_key_exists(&self) -> AdminStoreResult<bool> {
        Err(unused())
    }

    async fn replace_runtime_settings(
        &self,
        _: ReplaceRuntimeSettings,
        _: &MutationContext,
    ) -> AdminStoreResult<RuntimeSettings> {
        Err(unused())
    }

    async fn replace_admin_api_key(
        &self,
        _: AdminApiKey,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        Err(unused())
    }

    async fn delete_admin_api_key(
        &self,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        Err(unused())
    }
}

#[tokio::test]
async fn settings_should_reject_invalid_margin_or_probe_concurrency_before_store_call() {
    let services = super::AdminHarness::new()
        .settings(std::sync::Arc::new(UnusedSettingsStore))
        .build()
        .await;
    for (refresh_margin_seconds, turn_state_probe_concurrency) in
        [(0, None), (3600, Some(0)), (3600, Some(11))]
    {
        let error = services
            .settings()
            .replace(
                &MutationContext {
                    actor: gateway_admin::model::MutationActor::System,
                    request_id: "request-settings".to_owned(),
                },
                ReplaceRuntimeSettings {
                    request_profile_updates: Default::default(),
                    openai_request_settings: Default::default(),
                    openai_websocket_pool: Default::default(),
                    codex_privacy_policy: Default::default(),

                    expected_revision: gateway_admin::model::Revision::new(7).unwrap(),
                    account_affinity: None,
                    session_affinity_ttl_hours: None,
                    turn_state_probe_proxy_id: None,
                    turn_state_probe_concurrency,
                    turn_state_injection_enabled: None,
                    turn_state_models: Some(vec![
                        gateway_core::routing::UpstreamModelId::new("gpt-6-astra".to_owned())
                            .expect("model"),
                    ]),
                    responses_max_decompressed_body_bytes: Some(64 * 1024 * 1024),
                    model_mappings: Default::default(),
                    refresh_margin_seconds,
                    refresh_concurrency: 1,
                    max_concurrent_per_account: 1,
                    openai_guardian_reserved_concurrency: None,
                    account_warmup_enabled: None,
                    account_warmup_cron: None,
                    account_warmup_model: None,
                    codex_quota_overdraft_enabled: None,
                    codex_quota_overdraft_probe_model: None,
                    request_interval_ms: 0,
                    quota_reset_window_weights: Default::default(),
                    rotation_strategy: RotationStrategy::Smart,
                    min_codex_desktop_version: None,
                    min_codex_cli_version: None,
                    usage_retention_days: 31,
                    ops_event_retention_days: 30,
                    audit_retention_days: 30,
                    request_tuning: Default::default(),
                },
            )
            .await
            .expect_err("invalid settings");

        assert_eq!(error.kind(), AdminErrorKind::Invalid);
    }
}

fn unused() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "settings",
        "unused in this test",
    )
}

#[tokio::test]
async fn settings_should_reject_invalid_account_busy_wait_before_store_call() {
    let services = super::AdminHarness::new()
        .settings(std::sync::Arc::new(UnusedSettingsStore))
        .build()
        .await;
    for request_tuning in [
        RequestTuningOverrides {
            max_waiting_per_account: Some(0),
            ..Default::default()
        },
        RequestTuningOverrides {
            account_concurrency_wait_timeout_seconds: Some(601),
            ..Default::default()
        },
        RequestTuningOverrides {
            max_waiting_per_account: Some(1001),
            ..Default::default()
        },
        RequestTuningOverrides {
            account_concurrency_wait_timeout_seconds: Some(0),
            ..Default::default()
        },
    ] {
        for enabled in [false, true] {
            let error = services
                .settings()
                .replace(
                    &MutationContext {
                        actor: gateway_admin::model::MutationActor::System,
                        request_id: "request-account-busy-wait-settings".to_owned(),
                    },
                    ReplaceRuntimeSettings {
                        request_profile_updates: Default::default(),
                        openai_request_settings: Default::default(),
                        openai_websocket_pool: Default::default(),
                        codex_privacy_policy: Default::default(),

                        expected_revision: gateway_admin::model::Revision::new(7).unwrap(),
                        account_affinity: None,
                        session_affinity_ttl_hours: None,
                        turn_state_probe_proxy_id: None,
                        turn_state_probe_concurrency: None,
                        turn_state_injection_enabled: None,
                        turn_state_models: Some(vec![
                            gateway_core::routing::UpstreamModelId::new("gpt-6-astra".to_owned())
                                .expect("model"),
                        ]),
                        responses_max_decompressed_body_bytes: Some(64 * 1024 * 1024),
                        model_mappings: Default::default(),
                        refresh_margin_seconds: 3600,
                        refresh_concurrency: 1,
                        max_concurrent_per_account: 1,
                        openai_guardian_reserved_concurrency: None,
                        account_warmup_enabled: None,
                        account_warmup_cron: None,
                        account_warmup_model: None,
                        codex_quota_overdraft_enabled: None,
                        codex_quota_overdraft_probe_model: None,
                        request_interval_ms: 0,
                        quota_reset_window_weights: Default::default(),
                        rotation_strategy: RotationStrategy::Smart,
                        min_codex_desktop_version: None,
                        min_codex_cli_version: None,
                        usage_retention_days: 31,
                        ops_event_retention_days: 30,
                        audit_retention_days: 90,
                        request_tuning: RequestTuningOverrides {
                            account_busy_wait_enabled: Some(enabled),
                            switch_account_on_queue_failure: None,
                            ..request_tuning.clone()
                        },
                    },
                )
                .await
                .expect_err("invalid wait settings rejected before unavailable store");
            assert_eq!(error.kind(), AdminErrorKind::Invalid);
        }
    }
}

#[derive(Default)]
struct PricingSettingsStore {
    synced: Mutex<Option<gateway_admin::model::pricing::PricingSyncChanges>>,
    updated: Mutex<Option<UpdatePricing>>,
}

#[tokio::test]
async fn pricing_validates_the_bands_declared_by_the_registered_provider() {
    for (provider, supported) in [
        ("text-provider", &["standard", "fast"][..]),
        ("image-provider", &["standard", "image"][..]),
    ] {
        let store = Arc::new(PricingSettingsStore::default());
        let services = super::AdminHarness::new()
            .settings(store.clone())
            .provider(Arc::new(super::UnavailableProvider {
                pricing_bands: supported,
                ..super::UnavailableProvider::new(provider)
            }))
            .build()
            .await;
        assert_eq!(
            services.settings().pricing().await.unwrap().supported_bands[provider],
            supported
        );
        let context = MutationContext {
            actor: gateway_admin::model::MutationActor::System,
            request_id: "request-band-pricing".to_owned(),
        };
        for band in [
            "standard",
            "fast",
            "ultrafast",
            "flex",
            "long_standard",
            "long_fast",
            "long_ultrafast",
            "long_flex",
            "image",
        ] {
            let pricing = serde_json::from_value(serde_json::json!({
                "multiplierBps": 10000,
                "bands": {
                    "standard": {"input":"2", "output":"10", "cacheRead":"0.1", "cacheWrite":"2.5"},
                    (band): {"input":"12", "output":"60", "cacheRead":"0.6", "cacheWrite":"15"}
                }
            }))
            .expect("valid token price override");
            let update = UpdatePricing {
                provider: provider.to_owned(),
                models: vec!["example-model".to_owned()],
                change: PricingChange::Replace(pricing),
            };
            let result = services
                .settings()
                .update_pricing(&context, update.clone())
                .await;
            if supported.contains(&band) {
                result.expect("provider accepts its supported band");
                assert_eq!(store.updated.lock().unwrap().take(), Some(update));
            } else {
                assert_eq!(
                    result
                        .expect_err("provider rejects unsupported band")
                        .kind(),
                    AdminErrorKind::Invalid
                );
                assert!(store.updated.lock().unwrap().is_none());
            }
        }
    }
}

#[tokio::test]
async fn pricing_accepts_native_provider_and_rejects_unknown_sync_provider() {
    let store = Arc::new(PricingSettingsStore::default());
    let services = super::AdminHarness::new()
        .settings(store.clone())
        .provider(Arc::new(super::UnavailableProvider::new("xai")))
        .build()
        .await;
    let context = MutationContext {
        actor: gateway_admin::model::MutationActor::System,
        request_id: "request-native-pricing".to_owned(),
    };
    let pricing = serde_json::from_value(serde_json::json!({
        "multiplierBps": 10000,
        "bands": {"standard": {
            "input": "1", "output": "2", "cacheRead": "0", "cacheWrite": "0"
        }}
    }))
    .expect("native pricing");
    let update = UpdatePricing {
        provider: "xai".to_owned(),
        models: vec!["example-model".to_owned()],
        change: PricingChange::Replace(pricing),
    };

    services
        .settings()
        .update_pricing(&context, update.clone())
        .await
        .expect("native provider pricing");
    assert_eq!(
        store.updated.lock().expect("updated pricing").as_ref(),
        Some(&update)
    );

    let error = services
        .settings()
        .sync_pricing(
            &context,
            SyncPricing {
                preview: PricingSyncPreview {
                    prices: BTreeMap::new(),
                    skipped: Vec::new(),
                },
                models: BTreeMap::from([(
                    "unregistered-provider".to_owned(),
                    BTreeSet::from(["example-model".to_owned()]),
                )]),
            },
        )
        .await
        .expect_err("unknown provider cannot enter pricing sync");
    assert_eq!(error.kind(), AdminErrorKind::Invalid);
}

#[async_trait]
impl SettingsStore for PricingSettingsStore {
    async fn load_pricing(&self) -> AdminStoreResult<StoredPricing> {
        Ok(StoredPricing::default())
    }

    async fn sync_pricing(
        &self,
        command: gateway_admin::model::pricing::PricingSyncChanges,
        _: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        *self.synced.lock().unwrap() = Some(command);
        Ok(Revision::new(2).unwrap())
    }

    async fn update_pricing(
        &self,
        command: UpdatePricing,
        _: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        *self.updated.lock().expect("updated pricing") = Some(command);
        Ok(Revision::new(2).expect("revision"))
    }

    async fn load_runtime_settings(&self) -> AdminStoreResult<RuntimeSettings> {
        Err(unused())
    }

    async fn admin_api_key_exists(&self) -> AdminStoreResult<bool> {
        Err(unused())
    }

    async fn replace_runtime_settings(
        &self,
        _: ReplaceRuntimeSettings,
        _: &MutationContext,
    ) -> AdminStoreResult<RuntimeSettings> {
        Err(unused())
    }

    async fn replace_admin_api_key(
        &self,
        _: AdminApiKey,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        Err(unused())
    }

    async fn delete_admin_api_key(
        &self,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        Err(unused())
    }
}

struct ReferencePrices(Mutex<PricingSyncPreview>);
#[async_trait]
impl gateway_admin::ports::pricing::PricingSource for ReferencePrices {
    async fn fetch(
        &self,
        _: &gateway_admin::ports::pricing::PricingSourceScope,
    ) -> Result<PricingSyncPreview, gateway_admin::model::AdminError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn external_reference_prices_project_to_exact_provider_ids_in_preview_and_sync() {
    let price: gateway_core::metering::ModelPriceOverride = serde_json::from_value(serde_json::json!({"multiplierBps":10000,"bands":{"standard":{"input":"1","output":"2","cacheRead":"0.1","cacheWrite":"1"}}})).unwrap();
    let source = Arc::new(ReferencePrices(Mutex::new(PricingSyncPreview {
        prices: BTreeMap::from([(
            "google".into(),
            BTreeMap::from([
                ("base-model".into(), price.clone()),
                ("unmapped".into(), price),
            ]),
        )]),
        skipped: vec!["google/missing".into(), "google/unrelated-image".into()],
    })));
    let mut provider = super::UnavailableProvider::new("antigravity");
    provider.pricing_references = BTreeMap::from([
        ("ag-high".into(), ("google".into(), "base-model".into())),
        ("ag-low".into(), ("google".into(), "base-model".into())),
        ("ag-image".into(), ("google".into(), "missing".into())),
    ]);
    let store = Arc::new(PricingSettingsStore::default());
    let services = super::AdminHarness::new()
        .settings(store.clone())
        .provider(Arc::new(provider))
        .pricing_source(source.clone())
        .build()
        .await;
    let service = services.settings();
    let preview = service.preview_pricing_sync().await.unwrap();
    assert_eq!(
        preview.prices.keys().cloned().collect::<Vec<_>>(),
        ["antigravity"]
    );
    assert_eq!(
        preview.prices["antigravity"]
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["ag-high", "ag-low"]
    );
    assert_eq!(preview.skipped, ["antigravity/ag-image"]);
    // 未登记外部型号的变化不会污染同步确认，但实际保存仍重新读取并校验投影价目
    source
        .0
        .lock()
        .unwrap()
        .prices
        .get_mut("google")
        .unwrap()
        .remove("unmapped");
    service
        .sync_pricing(
            &MutationContext {
                actor: gateway_admin::model::MutationActor::System,
                request_id: "reference-sync".into(),
            },
            SyncPricing {
                preview,
                models: BTreeMap::from([(
                    "antigravity".into(),
                    BTreeSet::from(["ag-high".into()]),
                )]),
            },
        )
        .await
        .unwrap();
    let saved = store.synced.lock().unwrap();
    let saved = saved.as_ref().unwrap();
    assert_eq!(
        saved["antigravity"].keys().cloned().collect::<Vec<_>>(),
        ["ag-high"]
    );
}
