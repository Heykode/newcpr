use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use futures::future::BoxFuture;
use gateway_core::engine::admission::{
    ClientAdmissionDecision, ClientAdmissionError, ClientAdmissionPort, ClientAdmissionRecovery,
    ClientAdmissionRecoveryPort, ClientAdmissionRequest, ClientAdmissionRestoreResult,
    RecentAdmissionFact, RunningAdmissionFact, restore_client_admission_startup,
};
use gateway_core::engine::{
    AttemptRecord, ExecutionStore, IntermediateFailure, ModelRequestFinalization, ModelRequestId,
    NewModelRequest, RecoveryReport,
};
use gateway_core::error::{StoreError, StoreErrorKind};
use gateway_core::policy::ClientApiKeyId;
use gateway_core::upstream::UpstreamSendState;

#[test]
fn client_admission_startup_recovery_should_preserve_order_and_exact_facts() {
    futures::executor::block_on(async {
        let now = SystemTime::now();
        let request = ModelRequestId::new("req_recovery").expect("request id");
        let recovery = ClientAdmissionRecovery {
            client_api_key_id: ClientApiKeyId::new("key_recovery").expect("key id"),
            recent_requests: vec![RecentAdmissionFact {
                model_request_id: request.clone(),
                started_at: now - Duration::from_secs(2),
            }],
            running_requests: vec![RunningAdmissionFact {
                model_request_id: request,
                expires_at: now + Duration::from_secs(30),
            }],
        };
        let recoveries = ScriptedRecoveries::new(Ok(vec![recovery.clone()]));
        let admissions = RecordingAdmissions::default();
        let report = restore_client_admission_startup(
            &RecoveryStore::success(2),
            &recoveries,
            &admissions,
            now,
        )
        .await
        .expect("startup recovery");

        assert_eq!(report.expired_model_requests, 2);
        assert_eq!(report.restored_clients, 1);
        assert_eq!(
            admissions.restored.lock().expect("restored").as_slice(),
            &[recovery]
        );
    });
}

#[test]
fn client_admission_startup_recovery_should_fail_closed_at_each_boundary() {
    futures::executor::block_on(async {
        let now = SystemTime::now();
        assert!(
            restore_client_admission_startup(
                &RecoveryStore::failure(),
                &ScriptedRecoveries::new(Ok(Vec::new())),
                &RecordingAdmissions::default(),
                now,
            )
            .await
            .is_err()
        );
        assert!(
            restore_client_admission_startup(
                &RecoveryStore::success(0),
                &ScriptedRecoveries::new(Err(ClientAdmissionError)),
                &RecordingAdmissions::default(),
                now,
            )
            .await
            .is_err()
        );
        let admissions = RecordingAdmissions {
            fail_restore: true,
            ..RecordingAdmissions::default()
        };
        assert!(
            restore_client_admission_startup(
                &RecoveryStore::success(0),
                &ScriptedRecoveries::new(Ok(vec![empty_recovery()])),
                &admissions,
                now,
            )
            .await
            .is_err()
        );
    });
}

fn empty_recovery() -> ClientAdmissionRecovery {
    ClientAdmissionRecovery {
        client_api_key_id: ClientApiKeyId::new("key_empty").expect("key id"),
        recent_requests: Vec::new(),
        running_requests: Vec::new(),
    }
}

struct RecoveryStore(Result<RecoveryReport, StoreError>);

impl RecoveryStore {
    fn success(requests: u64) -> Self {
        Self(Ok(RecoveryReport { requests }))
    }

    fn failure() -> Self {
        Self(Err(StoreError::new(StoreErrorKind::Unavailable)))
    }
}

#[async_trait]
impl ExecutionStore for RecoveryStore {
    async fn create_model_request(&self, _: NewModelRequest) -> Result<(), StoreError> {
        unreachable!()
    }
    async fn record_attempt(&self, _: AttemptRecord) -> Result<(), StoreError> {
        unreachable!()
    }
    async fn mark_send_state(
        &self,
        _: &ModelRequestId,
        _: UpstreamSendState,
    ) -> Result<(), StoreError> {
        unreachable!()
    }
    async fn mark_downstream_committed(
        &self,
        _: &ModelRequestId,
        _: SystemTime,
        _: Option<u16>,
    ) -> Result<(), StoreError> {
        unreachable!()
    }
    async fn record_client_status(&self, _: &ModelRequestId, _: u16) -> Result<(), StoreError> {
        unreachable!()
    }
    async fn record_intermediate_failure(&self, _: IntermediateFailure) -> Result<(), StoreError> {
        unreachable!()
    }
    async fn finalize_model_request(&self, _: ModelRequestFinalization) -> Result<(), StoreError> {
        unreachable!()
    }
    async fn recover_expired(&self, _: SystemTime) -> Result<RecoveryReport, StoreError> {
        self.0.clone()
    }
}

struct ScriptedRecoveries(
    Mutex<VecDeque<Result<Vec<ClientAdmissionRecovery>, ClientAdmissionError>>>,
);

impl ScriptedRecoveries {
    fn new(result: Result<Vec<ClientAdmissionRecovery>, ClientAdmissionError>) -> Self {
        Self(Mutex::new(VecDeque::from([result])))
    }
}

impl ClientAdmissionRecoveryPort for ScriptedRecoveries {
    fn load_recovery(
        &self,
        _: SystemTime,
    ) -> BoxFuture<'_, Result<Vec<ClientAdmissionRecovery>, ClientAdmissionError>> {
        Box::pin(async move {
            self.0
                .lock()
                .expect("recovery script")
                .pop_front()
                .expect("script")
        })
    }
}

#[derive(Default)]
struct RecordingAdmissions {
    restored: Mutex<Vec<ClientAdmissionRecovery>>,
    fail_restore: bool,
}

impl ClientAdmissionPort for RecordingAdmissions {
    fn admit(
        &self,
        _: ClientAdmissionRequest,
    ) -> BoxFuture<'_, Result<ClientAdmissionDecision, ClientAdmissionError>> {
        unreachable!()
    }
    fn release<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a ModelRequestId,
    ) -> BoxFuture<'a, Result<bool, ClientAdmissionError>> {
        unreachable!()
    }
    fn restore(
        &self,
        recovery: ClientAdmissionRecovery,
    ) -> BoxFuture<'_, Result<ClientAdmissionRestoreResult, ClientAdmissionError>> {
        Box::pin(async move {
            if self.fail_restore {
                return Err(ClientAdmissionError);
            }
            self.restored.lock().expect("restored").push(recovery);
            Ok(ClientAdmissionRestoreResult {
                restored_recent_requests: 1,
                restored_running_requests: 1,
            })
        })
    }
}

#[test]
fn initial_runtime_publication_uses_persisted_openai_pool_before_any_settings_refresh() {
    futures::executor::block_on(async {
        use gateway_core::engine::continuation::{
            NativeContinuationPin, NativeContinuationPort, NativeContinuationStoreError,
            PreviousResponseId,
        };
        use gateway_core::engine::execution::{
            ClientApiKeyUsageSink, ProviderCircuitDecision, ProviderCircuitError,
            ProviderCircuitPort,
        };
        use gateway_core::routing::snapshot::{
            SnapshotFacts, SnapshotSettingsFacts, SnapshotStoreError, SnapshotStorePort,
        };
        use gateway_core::routing::{ConfigRevision, ProviderKind};
        use gateway_core::runtime::{
            RequestTuningHandle, SnapshotRevisionStream, SnapshotSubscriptionError,
            SnapshotSubscriptionPort,
        };
        use gateway_core::settings::OpenAiWebSocketPoolSettings;
        use std::sync::Arc;
        struct Ports(OpenAiWebSocketPoolSettings);
        impl SnapshotStorePort for Ports {
            fn load_snapshot_facts(
                &self,
            ) -> BoxFuture<'_, Result<SnapshotFacts, SnapshotStoreError>> {
                Box::pin(async move {
                    Ok(SnapshotFacts::new(
                        ConfigRevision::new(1).unwrap(),
                        ConfigRevision::new(1).unwrap(),
                        SnapshotSettingsFacts::new(3, 0, "smart", Default::default(), None, None)
                            .with_openai_runtime_settings(Default::default(), self.0),
                        vec![],
                        vec![],
                        vec![],
                        vec![],
                    ))
                })
            }
            fn current_config_revision(
                &self,
            ) -> BoxFuture<'_, Result<ConfigRevision, SnapshotStoreError>> {
                Box::pin(async { Ok(ConfigRevision::new(1).unwrap()) })
            }
        }
        impl SnapshotSubscriptionPort for Ports {
            fn publish_snapshot_revision(
                &self,
                _: ConfigRevision,
            ) -> BoxFuture<'_, Result<(), SnapshotSubscriptionError>> {
                Box::pin(async { Ok(()) })
            }
            fn subscribe_snapshot_revisions(
                &self,
            ) -> BoxFuture<'_, Result<SnapshotRevisionStream, SnapshotSubscriptionError>>
            {
                Box::pin(async {
                    Ok(Box::pin(futures::stream::pending()) as SnapshotRevisionStream)
                })
            }
        }
        impl ProviderCircuitPort for Ports {
            fn decision<'a>(
                &'a self,
                _: &'a ProviderKind,
            ) -> BoxFuture<'a, Result<ProviderCircuitDecision, ProviderCircuitError>> {
                unreachable!()
            }
            fn observe_failure<'a>(
                &'a self,
                _: &'a ProviderKind,
            ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
                unreachable!()
            }
            fn observe_success<'a>(
                &'a self,
                _: &'a ProviderKind,
            ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
                unreachable!()
            }
        }
        impl NativeContinuationPort for Ports {
            fn resolve<'a>(
                &'a self,
                _: &'a ClientApiKeyId,
                _: &'a PreviousResponseId,
            ) -> BoxFuture<'a, Result<Option<NativeContinuationPin>, NativeContinuationStoreError>>
            {
                unreachable!()
            }
            fn record<'a>(
                &'a self,
                _: NativeContinuationPin,
            ) -> BoxFuture<'a, Result<(), NativeContinuationStoreError>> {
                unreachable!()
            }
        }
        impl ClientApiKeyUsageSink for Ports {
            fn record_used(&self, _: &ClientApiKeyId) {}
        }
        let settings = OpenAiWebSocketPoolSettings {
            enabled: false,
            max_age_ms: 60000,
            fast_path_budget_ms: 123,
            idle_timeout_ms: 456000,
            stream_idle_timeout_ms: 789000,
        };
        let ports = Arc::new(Ports(settings));
        let tuning = RequestTuningHandle::default();
        let bundle = gateway_core::initialize_with_request_tuning(
            gateway_core::CoreStorePorts::new(
                Arc::new(RecoveryStore::success(0)),
                (
                    Arc::new(RecordingAdmissions::default()),
                    Arc::new(ScriptedRecoveries::new(Ok(vec![]))),
                ),
                ports.clone(),
                ports.clone(),
                (ports.clone(), ports.clone()),
                ports,
            ),
            gateway_core::engine::provider::ProviderRegistry::default(),
            tuning.clone(),
        )
        .await
        .unwrap();
        assert_eq!(tuning.openai_websocket_pool(), Some(settings));
        drop(bundle);
    });
}
