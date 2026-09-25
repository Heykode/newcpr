use async_trait::async_trait;

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
                    turn_state_probe_proxy_id: None,
                    turn_state_probe_concurrency,
                    disable_fast: None,
                    turn_state_injection_enabled: None,
                    excel_default_models: Default::default(),
                    turn_state_models: Some(vec![
                        gateway_core::routing::UpstreamModelId::new("gpt-6-astra".to_owned())
                            .expect("model"),
                    ]),
                    responses_max_decompressed_body_bytes: Some(64 * 1024 * 1024),
                    model_mappings: Default::default(),
                    refresh_margin_seconds,
                    refresh_concurrency: 1,
                    max_concurrent_per_account: 1,
                    request_interval_ms: 0,
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
            account_busy_wait_sticky_max_waiting: Some(0),
            ..Default::default()
        },
        RequestTuningOverrides {
            account_busy_wait_sticky_timeout_seconds: Some(601),
            ..Default::default()
        },
        RequestTuningOverrides {
            account_busy_wait_fallback_max_waiting: Some(1001),
            ..Default::default()
        },
        RequestTuningOverrides {
            account_busy_wait_fallback_timeout_seconds: Some(0),
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
                        turn_state_probe_proxy_id: None,
                        turn_state_probe_concurrency: None,
                        disable_fast: None,
                        turn_state_injection_enabled: None,
                        excel_default_models: Default::default(),
                        turn_state_models: Some(vec![
                            gateway_core::routing::UpstreamModelId::new("gpt-6-astra".to_owned())
                                .expect("model"),
                        ]),
                        responses_max_decompressed_body_bytes: Some(64 * 1024 * 1024),
                        model_mappings: Default::default(),
                        refresh_margin_seconds: 3600,
                        refresh_concurrency: 1,
                        max_concurrent_per_account: 1,
                        request_interval_ms: 0,
                        rotation_strategy: RotationStrategy::Smart,
                        min_codex_desktop_version: None,
                        min_codex_cli_version: None,
                        usage_retention_days: 31,
                        ops_event_retention_days: 30,
                        audit_retention_days: 90,
                        request_tuning: RequestTuningOverrides {
                            account_busy_wait_enabled: Some(enabled),
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
