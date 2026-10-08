//! Codex 的 `gateway-core` Provider adapter。

use std::collections::BTreeSet;
use std::fmt;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, future::BoxFuture};
use gateway_core::account::{AccountFeedbackStats, ProviderAccount};
use gateway_core::engine::continuation::{ContinuationBinding, NativeContinuationScope};
use gateway_core::engine::provider::{
    ContinuationRequestObservation, EventStream, Provider, ProviderCallMetadata, ProviderRequest,
    ProviderRequestObservation, ProviderSelectionObservation, ProviderStream,
};
use gateway_core::engine::{AttemptContext, AttemptTransport, ContinuationAttempt};
use gateway_core::error::{
    ClientVisibleUpstreamError, ClientVisibleUpstreamResponse, ContinuationFailure,
    ContinuationRecoveryDisposition, OpaqueUpstreamValue, ProviderConnectionObservation,
    ProviderDiagnostic, ProviderError, ProviderErrorKind, RawUpstreamError,
};
use gateway_core::event::{
    FinishReason, GatewayEvent, ProtocolWireEvent, ProviderEvent, ProviderResponseHeader,
    ProviderResponseMetadata, ProviderResponseObservation, ProviderResponseTimings, ResponseMeta,
    UpstreamHttpVersion, WebSocketPoolKind,
};
use gateway_core::lifecycle::{CancellationToken, Deadline};
use gateway_core::operation::{
    GenerateRequest, ImageRequest, ImageRequestKind, Operation, OperationKind,
    ProviderSessionState, StandaloneSearchRequest,
};
use gateway_core::provider_ports::ProviderSessionAffinityKey;
use gateway_core::routing::{
    ModelCapabilities, ModelPresentation, ProviderCandidate, ProviderCatalogGeneration,
    ProviderKind, ProviderModelCapabilities, UpstreamModelId,
};
use gateway_core::task::{
    DaemonRestartPolicy, DaemonTask, ScheduledTask, WorkerContribution, WorkerCycleContext,
    WorkerDefinitionError, WorkerId, WorkerKind, WorkerLeaseRequest, WorkerRegistration,
    WorkerRunnable, WorkerSchedule, WorkerTaskError,
};
use gateway_core::time::DeploymentTimeZone;
use gateway_core::upstream::{UpstreamSendState, UpstreamTransport};
use gateway_protocol::openai::events::rate_limits_to_header_pairs;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use url::Url;
use uuid::Uuid;

use crate::credential::{
    CodexAccountFailure, CodexCredentialCatalogError, CodexCredentialCatalogService,
    CodexCredentialLease, CodexCredentialQuotaService, CodexCredentialRefreshOutcome,
    CodexCredentialRefreshService, CodexCredentialRepository, CodexCredentialSelector,
    CodexCyberPolicyScope, CodexQuotaRefreshPolicy, CodexSessionAffinity, CredentialSelectionError,
    RuntimeCodexCookie, SelectCodexCredential, SelectCodexProviderEndpointCredential,
    derive_codex_cyber_policy_session_key, derive_codex_endpoint_session_affinity,
    derive_codex_session_affinity, derive_previous_response_id_hash,
};
use crate::session_transport::CodexSessionTransportRecovery;
use crate::transport::canonical::{
    CodexCanonicalDecoder, CodexCanonicalError, CodexCanonicalOutcome,
};
use crate::transport::catalog::{
    CodexCatalogCapabilityEvidence, CodexCatalogModel, CodexCatalogVisibility,
};
use crate::transport::diagnostics::{
    CodexFailureCategory, CodexUpstreamFailure, CodexUpstreamSendPhase,
};
use crate::transport::profile::{
    APPCAST_POLL_INTERVAL, CodexDesktopReleaseService, CodexRequestLocation, CodexWireProfileState,
};
use crate::transport::protocol::responses::{
    CodexResponsesRequest, PREVIOUS_RESPONSE_NOT_FOUND_CODE, PREVIOUS_RESPONSE_NOT_FOUND_MESSAGE,
    PreviousResponseScope, ResponseEventSignals, STREAM_REPLAY_GRACE, TransportRequirement,
    transport_requirement,
};
use crate::transport::protocol::websocket::WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE;
use crate::transport::request::{
    CodexRequestEncodeError, RequestAccountScope,
    encode_generate_request_with_location as encode_generate_request, scope_request_to_account,
};
use crate::transport::session::CodexSessionIdentity;
use crate::transport::usage::normalize_service_tier;
use crate::transport::websocket::{CodexWebSocketExchangeError, PreviousResponseUnavailableReason};
use crate::transport::{
    CODEX_ALPHA_SEARCH_PATH, CODEX_IMAGE_EDITS_PATH, CODEX_IMAGE_GENERATIONS_PATH,
    CODEX_REALTIME_CALLS_PATH, CODEX_RESPONSES_PATH, CodexAccountSelectionTelemetry,
    CodexBackendClient, CodexBackendJsonResponse, CodexBackendStreamingResponse,
    CodexBackendTransport, CodexClientError, CodexRateLimitUpdates, CodexRequestContext,
    CodexResponseMetadata, CodexResponseMetadataUpdates, CodexTransportMetrics,
    CodexUpstreamDiagnostics, CodexWebSocketPool, endpoint_url,
};

mod cache_diagnostics;
mod compact;
mod encrypted_history;
mod excel;
mod execution;
mod failure;
mod live;
mod observation;
mod quality_probe;
mod request_proxy;
mod workers;
pub(crate) use live::{CodexLiveGateway, CodexLiveRegistry};

use excel::prepare_excel;
use execution::*;
#[doc(hidden)]
pub use failure::openai_failure_affects_account_score;
use failure::*;
use observation::*;
pub(crate) use workers::worker_contributions;

const PROVIDER_NAME: &str = "openai";
const HTTP_SSE_TRANSPORT: &str = "http_sse";
const HTTP_JSON_TRANSPORT: &str = "http_json";
const WEBSOCKET_TRANSPORT: &str = "websocket";
const MAX_COOKIE_HEADER_BYTES: usize = 16 * 1024;
const QUOTA_FAILURE_REFRESH_TIMEOUT: Duration = Duration::from_secs(5);
pub const OFFICIAL_CODEX_BASE_PATH: &str = "/backend-api";
pub const OFFICIAL_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexProviderTransport {
    HttpOnly,
    PreferWebSocket,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodexProviderConfigError {
    #[error("Codex provider URL is invalid")]
    InvalidBaseUrl,
}

pub struct CodexProvider {
    selector: Arc<CodexCredentialSelector>,
    catalog: Arc<CodexCredentialCatalogService>,
    quota: Arc<CodexCredentialQuotaService>,
    account_feedback: Arc<AccountFeedbackStats>,
    client: CodexBackendClient,
    location: CodexRequestLocation,
    responses_url: Url,
    compact_url: Url,
    image_generations_url: Url,
    image_edits_url: Url,
    search_url: Url,
    live_calls_url: Url,
    session_identity: Option<CodexSessionIdentity>,
    session_transport_recovery: CodexSessionTransportRecovery,
    encrypted_history: encrypted_history::InvalidEncryptedHistory,
    stream_max_retries: u32,
    request_tuning: Option<gateway_core::runtime::RequestTuningHandle>,
    excel_replay: Arc<dyn gateway_core::provider_ports::ProviderReplayPort>,
    session_proxy_pool:
        Option<Arc<dyn gateway_core::provider_ports::session_proxy::SessionProxyPool>>,
    excel_image_relay: Arc<crate::transport::excel::image_relay::ImageRelay>,
    live_registry: Arc<CodexLiveRegistry>,
    live_gateway: Option<Arc<CodexLiveGateway>>,
}

impl CodexProvider {
    pub(crate) fn with_session_proxy_pool(
        mut self,
        pool: Option<Arc<dyn gateway_core::provider_ports::session_proxy::SessionProxyPool>>,
    ) -> Self {
        self.session_proxy_pool = pool;
        self
    }
    pub(crate) fn with_excel_image_relay(
        mut self,
        relay: Arc<crate::transport::excel::image_relay::ImageRelay>,
    ) -> Self {
        self.excel_image_relay = relay;
        self
    }
    pub(crate) fn with_excel_replay(
        mut self,
        replay: Arc<dyn gateway_core::provider_ports::ProviderReplayPort>,
    ) -> Self {
        self.excel_replay = replay;
        self
    }
    pub(super) fn client_for_request(
        &self,
        context: &AttemptContext,
    ) -> Result<CodexBackendClient, ProviderError> {
        match context.request_profile() {
            Some(snapshot) => CodexWireProfileState::from_request_snapshot(snapshot)
                .map(|profile| self.client.with_request_profile(profile))
                .map_err(|_| {
                    provider_error(ProviderErrorKind::Protocol, UpstreamSendState::NotSent)
                }),
            None => Ok(self.client.clone()),
        }
    }

    #[must_use]
    pub fn with_egress_runtime(
        mut self,
        runtime: Arc<crate::transport::egress::CodexEgressRuntime>,
    ) -> Self {
        self.client = self.client.with_egress_runtime(runtime);
        self
    }

    pub(crate) fn with_request_tuning(
        mut self,
        request_tuning: gateway_core::runtime::RequestTuningHandle,
    ) -> Self {
        self.client = self.client.with_request_tuning(request_tuning.clone());
        self.request_tuning = Some(request_tuning);
        self
    }

    // Provider 构造集中装配独立领域服务和透明传输依赖，拆分参数会模糊所有权。
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        selector: Arc<CodexCredentialSelector>,
        catalog: Arc<CodexCredentialCatalogService>,
        quota: Arc<CodexCredentialQuotaService>,
        account_feedback: Arc<AccountFeedbackStats>,
        http: Client,
        profile: CodexWireProfileState,
        base_url: String,
        websocket_pool: Arc<CodexWebSocketPool>,
        _stream_max_retries: u32,
        timezone: DeploymentTimeZone,
    ) -> Result<Self, CodexProviderConfigError> {
        let responses_url = Url::parse(&endpoint_url(&base_url, CODEX_RESPONSES_PATH))
            .map_err(|_| CodexProviderConfigError::InvalidBaseUrl)?;
        let compact_url = Url::parse(&endpoint_url(
            &base_url,
            crate::transport::endpoints::CODEX_RESPONSES_COMPACT_PATH,
        ))
        .map_err(|_| CodexProviderConfigError::InvalidBaseUrl)?;
        let image_generations_url =
            Url::parse(&endpoint_url(&base_url, CODEX_IMAGE_GENERATIONS_PATH))
                .map_err(|_| CodexProviderConfigError::InvalidBaseUrl)?;
        let image_edits_url = Url::parse(&endpoint_url(&base_url, CODEX_IMAGE_EDITS_PATH))
            .map_err(|_| CodexProviderConfigError::InvalidBaseUrl)?;
        let search_url = Url::parse(&endpoint_url(&base_url, CODEX_ALPHA_SEARCH_PATH))
            .map_err(|_| CodexProviderConfigError::InvalidBaseUrl)?;
        let live_calls_url = Url::parse(&endpoint_url(&base_url, CODEX_REALTIME_CALLS_PATH))
            .map_err(|_| CodexProviderConfigError::InvalidBaseUrl)?;
        let location = profile.snapshot().location;
        let client = CodexBackendClient::new(http, base_url, profile)
            .with_websocket_pool(websocket_pool)
            .with_timezone(timezone);
        Ok(Self {
            session_proxy_pool: None,
            selector,
            catalog,
            quota,
            account_feedback,
            client,
            location,
            responses_url,
            compact_url,
            image_generations_url,
            image_edits_url,
            search_url,
            live_calls_url,
            session_identity: None,
            session_transport_recovery: CodexSessionTransportRecovery::default(),
            encrypted_history: encrypted_history::InvalidEncryptedHistory::default(),
            stream_max_retries: _stream_max_retries,
            request_tuning: None,
            excel_replay: Arc::new(gateway_core::provider_ports::UnavailableProviderReplay),
            excel_image_relay: Arc::new(crate::transport::excel::image_relay::ImageRelay::new(
                None,
            )),
            live_registry: Arc::new(CodexLiveRegistry::default()),
            live_gateway: None,
        })
    }

    pub(crate) fn with_session_identity(mut self, identity: CodexSessionIdentity) -> Self {
        self.session_identity = Some(identity);
        self
    }

    /// 绑定 Live 语音支持：call 注册表 + 账号级 sideband 网关。
    /// 需要凭据仓库句柄；未绑定时不暴露 sideband 与 hangup 能力。
    pub(crate) fn with_live_support(mut self, repository: CodexCredentialRepository) -> Self {
        let gateway = Arc::new(CodexLiveGateway::new(
            Arc::clone(&self.live_registry),
            repository,
            self.client.clone(),
        ));
        self.live_gateway = Some(gateway);
        self
    }
}

impl fmt::Debug for CodexProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexProvider")
            .field("selector", &"<account-selector>")
            .field("catalog", &"<ttl-catalog>")
            .finish()
    }
}

#[async_trait]
impl Provider for CodexProvider {
    fn resolve_request_profile(
        &self,
    ) -> Result<Option<gateway_core::account::OpaqueProviderData>, ProviderError> {
        self.client
            .request_profile()
            .map(Some)
            .map_err(|_| provider_error(ProviderErrorKind::Protocol, UpstreamSendState::NotSent))
    }

    fn name(&self) -> &'static str {
        PROVIDER_NAME
    }

    fn live_gateway(&self) -> Option<Arc<dyn gateway_core::live::LiveGateway>> {
        self.live_gateway
            .clone()
            .map(|gateway| gateway as Arc<dyn gateway_core::live::LiveGateway>)
    }

    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        self.catalog.catalog_generation()
    }

    fn model_catalog_is_exhaustive(&self) -> bool {
        false
    }

    fn request_observation(
        &self,
        operation: &Operation,
        client_api_key_id: &gateway_core::policy::ClientApiKeyId,
    ) -> ProviderRequestObservation {
        let Operation::Generate(request) = operation else {
            let affinity = match operation {
                Operation::Compact(request) => {
                    crate::credential::derive_codex_compact_session_affinity(
                        request.payload(),
                        client_api_key_id,
                    )
                }
                Operation::Search(request) => derive_codex_endpoint_session_affinity(
                    request.payload(),
                    client_api_key_id,
                    "id",
                ),
                Operation::GenerateImage(request) => derive_codex_endpoint_session_affinity(
                    request.payload(),
                    client_api_key_id,
                    "session_id",
                ),
                _ => None,
            };
            return ProviderRequestObservation {
                compact: matches!(operation, Operation::Compact(_)),
                requested_model: match operation {
                    Operation::Search(request) => {
                        observation::endpoint_requested_model(request.payload())
                    }
                    Operation::GenerateImage(request) => {
                        observation::endpoint_requested_model(request.payload())
                    }
                    _ => None,
                },
                continuation: ContinuationRequestObservation {
                    affinity_hash: affinity
                        .and_then(|affinity| affinity.persistence_hash().map(ToOwned::to_owned)),
                    ..Default::default()
                },
                ..Default::default()
            };
        };
        let location = self
            .request_tuning
            .as_ref()
            .is_some_and(|tuning| tuning.load().openai_location_override_enabled)
            .then_some(&self.location);
        let Ok(encoded) = encode_generate_request(request, "observability", location) else {
            return ProviderRequestObservation::default();
        };
        let semantics = encoded.semantics();
        let reasoning_effort = semantics.reasoning_effort.clone();
        let previous_response_id = encoded.previous_response_id();
        let continuation = ContinuationRequestObservation {
            affinity_hash: derive_codex_session_affinity(&encoded, client_api_key_id)
                .and_then(|affinity| affinity.persistence_hash().map(ToOwned::to_owned)),
            previous_response_id_hash: previous_response_id.map(|response_id| {
                derive_previous_response_id_hash(response_id, client_api_key_id)
            }),
            requested: previous_response_id.is_some(),
        };
        ProviderRequestObservation {
            requested_model: None,
            reasoning_effort,
            reasoning_preset: semantics.reasoning_preset.map(str::to_owned),
            request_kind: semantics.request_kind,
            subagent_kind: semantics.subagent_kind,
            compact: semantics.compact,
            continuation,
        }
    }

    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        let snapshot = self.catalog.synchronize().await.map_err(|error| {
            provider_error(ProviderErrorKind::Unavailable, UpstreamSendState::NotSent)
                .with_source(error)
        })?;
        let mut accounts = snapshot.model_catalog_accounts();
        Ok(snapshot
            .models()
            .iter()
            .map(|model| {
                compile_model_capabilities(model).with_catalog_accounts(
                    accounts
                        .remove(model.request_model().as_str())
                        .unwrap_or_default(),
                )
            })
            .collect())
    }

    async fn query_client_model_catalog(
        &self,
        scope: &gateway_core::account::scope::FrozenAccountScope,
        protocol: &str,
        client_version: &str,
    ) -> Result<
        Option<Vec<gateway_core::routing::ProviderModelDescriptor>>,
        gateway_core::routing::ProviderCatalogUnavailable,
    > {
        if protocol != "codex" {
            return Ok(None);
        }
        self.catalog
            .client_model_catalog(scope, client_version)
            .await
            .map(Some)
            .map_err(|_| gateway_core::routing::ProviderCatalogUnavailable)
    }

    async fn execute(
        &self,
        request: ProviderRequest,
        context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        if request.candidate().provider().as_str() != PROVIDER_NAME {
            return Err(provider_error(
                ProviderErrorKind::InvalidRequest,
                UpstreamSendState::NotSent,
            ));
        }
        let candidate = request.candidate();
        if context.cancellation().is_cancelled() {
            return Err(provider_error(
                ProviderErrorKind::Cancelled,
                UpstreamSendState::NotSent,
            ));
        }
        if context.deadline().is_elapsed() {
            return Err(provider_error(
                ProviderErrorKind::Timeout,
                UpstreamSendState::NotSent,
            ));
        }
        if let Operation::GenerateImage(image) = request.operation() {
            return self.execute_image(image, candidate, context).await;
        }
        if let Operation::Search(search) = request.operation() {
            return self.execute_search(search, candidate, context).await;
        }
        if let Operation::Compact(compact) = request.operation() {
            return self.execute_compact(compact, candidate, context).await;
        }
        if let Operation::ProviderHttp(request) = request.operation() {
            return self
                .execute_live_call(request.clone(), candidate.upstream_model(), context)
                .await;
        }
        let Operation::Generate(generate) = request.operation() else {
            return Err(provider_error(
                ProviderErrorKind::Unsupported,
                UpstreamSendState::NotSent,
            ));
        };
        let Some(upstream_model) = candidate.upstream_model() else {
            return Err(provider_error(
                ProviderErrorKind::Protocol,
                UpstreamSendState::NotSent,
            ));
        };
        let previous_session = decode_openai_session_state(generate);
        // Validate replay before account selection can mutate the shared binding.
        let native_scope = if let Some(continuation) =
            context.continuation().and_then(ContinuationBinding::pinned)
        {
            let native_scope = match previous_session
                .as_ref()
                .map(|state| state.continuation_scope)
            {
                Some(OpenAiContinuationScope::Persisted) => PreviousResponseScope::Persisted,
                Some(OpenAiContinuationScope::ConnectionLocal) => {
                    PreviousResponseScope::ConnectionLocal
                }
                Some(OpenAiContinuationScope::ReplayRequired) => {
                    PreviousResponseScope::ExternalUnknown
                }
                None => match continuation.scope() {
                    NativeContinuationScope::Persisted => PreviousResponseScope::Persisted,
                    NativeContinuationScope::ConnectionLocal => {
                        PreviousResponseScope::ConnectionLocal
                    }
                },
            };
            if matches!(
                context.continuation_attempt(),
                ContinuationAttempt::ReplayOwner | ContinuationAttempt::ReplayAny
            ) && native_scope == PreviousResponseScope::ConnectionLocal
            {
                tracing::warn!(
                    request_id = context.request_id().as_str(),
                    attempt_index = context.attempt_index().get(),
                    continuation_scope = "connection_local",
                    continuation_attempt = context.continuation_attempt().as_str(),
                    continuation_recovery_disposition = "client_replay_required",
                    continuation_recovery_action = "stop_proxy_recovery",
                    "OpenAI connection-local continuation replay was rejected before send"
                );
                return Err(continuation_replay_required_error("scope_unavailable"));
            }
            native_scope
        } else {
            PreviousResponseScope::ExternalUnknown
        };
        let continuation_requested = generate.native_continuation_requested();
        let location = context
            .request_tuning()
            .openai_location_override_enabled
            .then_some(&self.location);
        let mut upstream_request =
            encode_generate_request(generate, upstream_model.as_str(), location)
                .map_err(map_request_error)?;
        upstream_request.quality_probe = generate.quality_probe().cloned();
        upstream_request.excel_recovery_nonce = generate.excel_recovery_nonce().map(str::to_owned);
        quality_probe::initialize(
            &mut upstream_request,
            context.is_quality_check(),
            context.request_id().as_str(),
        )?;
        // 编码已生成独立请求；HTTP、WS 与重试在头部和计量之前共用此策略。
        let supports_priority = candidate.model_presentation().is_some_and(|model| {
            model
                .service_tiers()
                .iter()
                .any(|tier| tier.id() == "priority")
        });
        let fast_mode = if context.disable_fast() {
            gateway_core::account::FastMode::Disabled
        } else {
            candidate.account_scope().fast_mode()
        };
        upstream_request.apply_fast_policy(fast_mode, supports_priority);
        let client_key_id = context.client_api_key_ref().as_str();
        if previous_session
            .as_ref()
            .and_then(|state| state.client_api_key_id.as_deref())
            .is_some_and(|owner| owner != client_key_id)
        {
            return Err(provider_error(
                ProviderErrorKind::InvalidRequest,
                UpstreamSendState::NotSent,
            ));
        }
        // This value is authoritative even if a protocol adapter carried a client-supplied alias.
        upstream_request.client_api_key_id = Some(client_key_id.to_owned());
        upstream_request.identity_seed =
            (!crate::transport::qx_application::has_explicit_identity(&upstream_request))
                .then(|| {
                    previous_session
                        .as_ref()
                        .and_then(|state| state.identity_seed.clone())
                })
                .flatten()
                .or_else(|| {
                    Some(crate::transport::qx_application::identity_seed(
                        &upstream_request,
                        context.request_id().as_str(),
                    ))
                });
        if previous_session.is_some()
            || context.continuation_attempt() != ContinuationAttempt::None
            || continuation_requested
        {
            // Ordinary retries acquire an account_state_owner too; retain their routing key.
            // Only restored/native continuation identity takes precedence over local hints.
            upstream_request.scheduling_session_hint = None;
        }
        if let Some(conversation_id) = previous_session
            .as_ref()
            .and_then(|state| state.conversation_id.as_ref())
        {
            upstream_request.local_conversation_id = Some(conversation_id.clone());
        }
        if let Some(identity) = &self.session_identity {
            identity.prepare_local_conversation(&mut upstream_request);
        }
        if let Some(previous_session) = previous_session.as_ref() {
            if same_client_turn(
                previous_session.client_turn_id.as_deref(),
                upstream_request.client_turn_id.as_deref(),
            ) {
                upstream_request.turn_state = upstream_request
                    .turn_state
                    .take()
                    .or_else(|| previous_session.turn_state.clone());
            } else {
                crate::transport::request::clear_turn_state(&mut upstream_request);
            }
        }
        let session_affinity = derive_codex_session_affinity(
            &upstream_request,
            context.client_api_key_ref(),
        )
        .map(|affinity| {
            affinity.with_policy(context.account_selection_policy().openai_account_affinity())
        });
        // Restored and native owners take precedence over a client turn alias.
        let session_affinity = if previous_session.is_some()
            || continuation_requested
            || context.continuation_attempt() != ContinuationAttempt::None
        {
            session_affinity.map(CodexSessionAffinity::with_advisory_turn_alias)
        } else {
            self.resolve_turn_affinity(session_affinity, &context)
                .await?
        };
        let cyber_policy_session_key =
            derive_codex_cyber_policy_session_key(&upstream_request, context.client_api_key_ref());
        let guardian = upstream_request.guardian_parent_thread_id().is_some();

        let selection_started_at = Instant::now();
        let lease = self
            .selector
            .select_with_cyber_policy(
                &SelectCodexCredential {
                    upstream_model: upstream_model.as_str(),
                    request_url: &self.responses_url,
                    attempt: &context,
                    session_affinity_key: session_affinity
                        .as_ref()
                        .and_then(CodexSessionAffinity::key),
                    guardian,
                },
                cyber_policy_session_key.as_ref(),
                session_affinity.as_ref(),
            )
            .await
            .map_err(map_selection_error)?;
        let account_selection_wait_ms =
            u64::try_from(selection_started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let lease = Arc::new(lease);
        let responses_upstream = if context.is_native_quality_probe() {
            gateway_core::account::ResponsesUpstream::Codex
        } else {
            lease
                .account()
                .responses_upstream_for_model(upstream_model.as_str())
        };
        let excel = responses_upstream == gateway_core::account::ResponsesUpstream::Excel;
        if previous_session
            .as_ref()
            .is_some_and(|state| state.responses_upstream != responses_upstream)
        {
            return Err(continuation_replay_required_error("upstream_changed"));
        }
        let provider_kind = ProviderKind::new(PROVIDER_NAME)
            .map_err(|_| provider_error(ProviderErrorKind::Protocol, UpstreamSendState::NotSent))?;
        let state_owner_cross_account = context
            .account_state_owner()
            .is_some_and(|owner| !owner.matches(&provider_kind, lease.account_id()))
            || previous_session
                .as_ref()
                .is_some_and(|state| state.account_id != lease.account_id().as_str());
        let has_explicit_state_owner =
            context.account_state_owner().is_some() || previous_session.is_some();
        let account_scope =
            if state_owner_cross_account || (!has_explicit_state_owner && lease.account_switch()) {
                RequestAccountScope::Different
            } else if has_explicit_state_owner || lease.affinity_hit() {
                RequestAccountScope::Same
            } else {
                RequestAccountScope::Unknown
            };
        if context.continuation_attempt() != ContinuationAttempt::None
            && let Some(continuation) = context.continuation()
        {
            match continuation {
                ContinuationBinding::Pinned(continuation) => {
                    let previous_response_scope = match context.continuation_attempt() {
                        ContinuationAttempt::Native => native_scope,
                        ContinuationAttempt::ReplayOwner | ContinuationAttempt::ReplayAny => {
                            PreviousResponseScope::ExternalUnknown
                        }
                        ContinuationAttempt::None => PreviousResponseScope::ExternalUnknown,
                    };
                    upstream_request.set_previous_response_id(Some(
                        continuation.upstream_response_id().as_str().to_owned(),
                    ));
                    upstream_request.previous_response_scope = Some(previous_response_scope);
                }
                ContinuationBinding::External(previous_response_id) => {
                    upstream_request
                        .set_previous_response_id(Some(previous_response_id.as_str().to_owned()));
                    upstream_request.previous_response_scope =
                        Some(PreviousResponseScope::ExternalUnknown);
                }
            }
        }
        if upstream_request.previous_response_id().is_some()
            && !account_scope.can_reuse_account_state()
        {
            return Err(continuation_replay_required_error("scope_unavailable"));
        }
        if !excel
            && upstream_request
                .downstream_websocket_connection_id
                .is_some()
            && upstream_request.previous_response_id().is_some()
            && upstream_request.generate()
            && previous_session.as_ref().is_some_and(|state| {
                state.continuation_scope == OpenAiContinuationScope::ReplayRequired
            })
        {
            return Err(continuation_replay_required_error("scope_unavailable"));
        }
        scope_request_to_account(
            &mut upstream_request,
            lease.installation_id(),
            account_scope,
        );
        if matches!(
            lease.authentication(),
            crate::credential::CodexRuntimeAuthentication::OAuth(_)
        ) {
            crate::transport::request::compatibility::default_missing_format_name(
                upstream_request.body_mut(),
            );
            crate::transport::request::compatibility::normalize_custom_history_ids(
                upstream_request.body_mut(),
            );
            crate::transport::request::compatibility::normalize_message_history_ids(
                upstream_request.body_mut(),
            );
            crate::transport::request::normalize_reasoning_replay(upstream_request.body_mut());
            if !excel {
                crate::transport::request::compatibility::normalize_minimal_effort(
                    &mut upstream_request,
                );
            }
        }
        // Preserve the established identity/affinity inputs above. Only the
        // selected account's outbound copy receives the effective location.
        if context.request_tuning().openai_location_override_enabled {
            let location = lease
                .account()
                .request_location()
                .or(context.request_location())
                .unwrap_or(&self.location);
            crate::transport::request::align_structured_location_fields(
                upstream_request.body_mut(),
                chrono::Utc::now(),
                location,
            );
        }
        quality_probe::prepare(&mut upstream_request, &context, lease.account())?;
        if excel {
            prepare_excel(
                &mut upstream_request,
                &lease,
                &context,
                Arc::clone(&self.excel_replay),
                &self.excel_image_relay,
            )
            .await?;
        }
        let requirement = transport_requirement(&upstream_request);
        let requested_transport = selected_transport(&upstream_request);
        let session_http_fallback = requirement.allows_pre_send_http_fallback()
            && session_affinity.as_ref().is_some_and(|affinity| {
                affinity
                    .transport_key()
                    .is_some_and(|key| self.session_transport_recovery.uses_http(key))
            });
        let transport = if requirement.requires_websocket() {
            CodexProviderTransport::PreferWebSocket
        } else if context.transport() == AttemptTransport::Fallback || session_http_fallback {
            CodexProviderTransport::HttpOnly
        } else {
            requested_transport
        };
        apply_transport(&mut upstream_request, transport);
        let metadata = ProviderCallMetadata::new(
            provider_kind,
            upstream_model.clone(),
            lease.account_id().clone(),
            UpstreamTransport::new(if excel {
                "excel_http_sse"
            } else {
                transport_name(transport)
            })
            .map_err(|_| provider_error(ProviderErrorKind::Protocol, UpstreamSendState::NotSent))?,
        )
        .with_selection_observation(ProviderSelectionObservation::new(
            account_selection_wait_ms,
            lease.capacity_snapshot(),
        ));
        let response_store = upstream_request.store();
        let session_capture = (upstream_request.quality_probe.is_none()
            && (!continuation_requested || previous_session.is_some()))
        .then(|| OpenAiSessionCapture {
            responses_upstream,
            account_id: lease.account_id().as_str().to_owned(),
            conversation_id: upstream_request.local_conversation_id.clone(),
            turn_state: upstream_request.turn_state.clone(),
            client_turn_id: upstream_request.client_turn_id.clone(),
            response_store,
            continuation_scope: None,
            client_api_key_id: upstream_request.client_api_key_id.clone(),
            identity_seed: upstream_request.identity_seed.clone(),
        });
        let allows_account_state_mutation = lease.allows_account_state_mutation();
        let session_affinity_key_hash = session_affinity
            .as_ref()
            .map(|affinity| affinity.key_hash().to_owned());
        let transport_affinity_key = session_affinity
            .as_ref()
            .and_then(CodexSessionAffinity::transport_key)
            .cloned();
        let session_affinity_key = session_affinity.and_then(CodexSessionAffinity::into_key);
        let request_tuning = context.request_tuning();
        let websocket_retry_count = match context.transport() {
            AttemptTransport::Retry(retry_index) => retry_index.get(),
            AttemptTransport::Default | AttemptTransport::Fallback => 0,
        };
        let session = upstream_request
            .local_conversation_id
            .as_deref()
            .or_else(|| {
                upstream_request
                    .body()
                    .get("prompt_cache_key")
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                upstream_request
                    .body()
                    .get("session_id")
                    .and_then(Value::as_str)
            });
        let client = self.account_exit_client(&context, lease.account(), excel, session)?;
        if let Some(prepared) = upstream_request.excel.as_mut() {
            prepared.exit_lease = client.session_proxy_lease();
        }
        let events = cold_response_stream(ColdResponse {
            encrypted_history: self.encrypted_history.clone(),
            client: client.with_response_control(context.response_control().cloned()),
            response_origin: if excel {
                Url::parse(crate::transport::excel::RESPONSES_URL).map_err(|_| {
                    provider_error(ProviderErrorKind::Protocol, UpstreamSendState::NotSent)
                })?
            } else {
                self.responses_url.clone()
            },
            request: Arc::new(upstream_request),
            upstream_model: upstream_model.clone(),
            transport_policy: transport,
            context,
            selector: Arc::clone(&self.selector),
            quota: Arc::clone(&self.quota),
            catalog: Arc::clone(&self.catalog),
            lease: Arc::clone(&lease),
            session_affinity_key,
            session_affinity_key_hash,
            transport_affinity_key,
            session_transport_recovery: self.session_transport_recovery.clone(),
            websocket_retry_count,
            stream_max_retries: if self.request_tuning.is_some() {
                request_tuning.websocket_max_retries
            } else {
                self.stream_max_retries
            },
            session_capture,
        });
        let stream = ProviderStream::new(metadata, events, lease);
        Ok(if allows_account_state_mutation {
            stream.with_filtered_account_feedback(
                Arc::clone(&self.account_feedback),
                openai_failure_affects_account_score,
            )
        } else {
            stream
        })
    }
}
