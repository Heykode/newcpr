//! OpenAI Provider 专属能力。

mod admin;
mod admin_user_agent;
pub mod config;
mod provider;
mod session_transport;

use std::sync::Arc;

use gateway_admin::ports::provider::ProviderAdmin;
use gateway_core::account::ProviderAccountStore;
use gateway_core::engine::provider::Provider;
use gateway_core::provider_ports::ProviderStorePorts;
use gateway_core::routing::ProviderKind;
use gateway_core::runtime::RequestTuningHandle;
use gateway_core::task::WorkerContribution;

use crate::admin::{OpenAiAdminProvider, OpenAiAdminServices, OpenAiOAuthPendingStore};
use crate::credential::token_client::{AuthorizationCodeExchanger, TokenRefresher};
use crate::credential::{
    CodexCookiePolicy, CodexCredentialAdmin, CodexCredentialAdminService,
    CodexCredentialCatalogService, CodexCredentialProfileService, CodexCredentialQuotaService,
    CodexCredentialRefreshService, CodexCredentialRepository, CodexCredentialSelector,
    CodexDeviceCodec, CodexOAuthAdmin, CodexOAuthAdminService,
};
use crate::transport::egress::CodexEgressRuntime;
use crate::transport::profile::{
    CodexArtifactProfileCache, CodexDesktopReleaseService, OfficialCodexDesktopReleaseTransport,
};
use crate::transport::{CodexWebSocketPool, build_reqwest_client};

pub use config::{CodexWireProfileConfig, OpenAiConfig, OpenAiConfigError};
pub use provider::{
    CodexProvider, CodexProviderConfigError, CodexProviderTransport, OFFICIAL_CODEX_BASE_PATH,
    OFFICIAL_CODEX_BASE_URL, openai_failure_affects_account_score,
};

pub mod credential;
pub mod transport;

pub use transport::tls::{
    build_reqwest_client_with_custom_ca, build_reqwest_native_client_with_custom_ca,
    ensure_rustls_provider,
};
pub use transport::{
    CodexCanonicalDecoder, CodexCanonicalError, CodexRequestEncodeError, OpenAiBillingUsage,
    encode_generate_request, openai_billing_breakdown,
};

/// OpenAI 初始化后交给组装根的最小能力集。
pub struct ProviderBundle {
    image_relay: Arc<dyn gateway_core::provider_ports::TemporaryImageSource>,
    core_provider: Arc<dyn Provider>,
    admin_provider: Arc<dyn ProviderAdmin>,
    worker_contributions: Vec<WorkerContribution>,
}

/// 构造 OpenAI 数据面、Provider-owned 后台任务与 Redis OAuth pending owner。
pub async fn initialize(
    config: OpenAiConfig,
    ports: ProviderStorePorts,
) -> Result<ProviderBundle, OpenAiInitializeError> {
    initialize_with_request_tuning_mode(config, ports, RequestTuningHandle::default(), false).await
}

/// Initialize OpenAI with a process-shared live request-tuning handle.
pub async fn initialize_with_request_tuning(
    config: OpenAiConfig,
    ports: ProviderStorePorts,
    request_tuning: RequestTuningHandle,
) -> Result<ProviderBundle, OpenAiInitializeError> {
    initialize_with_request_tuning_mode(config, ports, request_tuning, true).await
}

async fn initialize_with_request_tuning_mode(
    config: OpenAiConfig,
    ports: ProviderStorePorts,
    request_tuning: RequestTuningHandle,
    enforce_account_concurrency: bool,
) -> Result<ProviderBundle, OpenAiInitializeError> {
    if config
        .excel_image_relay_public_url
        .as_deref()
        .is_some_and(|url| !transport::excel::image_relay::validate_origin(url))
    {
        return Err(OpenAiInitializeError::Config(
            OpenAiConfigError::InvalidExcelImageRelay,
        ));
    }
    let image_relay = Arc::new(
        transport::excel::image_relay::ImageRelay::new(config.excel_image_relay_public_url.clone())
            .with_request_tuning(request_tuning.clone()),
    );
    let provider_kind =
        ProviderKind::new("openai").map_err(|_| OpenAiInitializeError::InvalidProviderKind)?;
    let accounts: Arc<dyn ProviderAccountStore> = ports.accounts();
    accounts
        .initialize_device_registry(Arc::new(CodexDeviceCodec))
        .await
        .map_err(|_| OpenAiInitializeError::DeviceRegistry)?;
    let egress_runtime = match ports.egress() {
        Some(store) => Some(
            CodexEgressRuntime::load(store)
                .await
                .map_err(|_| OpenAiInitializeError::Egress)?,
        ),
        None => None,
    };
    let leases = ports.leases();
    let session_affinity = ports.session_affinity();
    let session_exclusions = ports.session_exclusions();
    let account_feedback = ports.account_feedback();
    let runtime_policy = ports.runtime_policy();
    let credential_state = ports.credential_state();
    let profile = config.wire_profile_state();
    let user_agent_override = runtime_policy
        .load_user_agent_override(&provider_kind)
        .await
        .map_err(|_| OpenAiInitializeError::RuntimePolicy)?;
    profile
        .apply_user_agent_override(&user_agent_override)
        .map_err(|_| OpenAiInitializeError::UserAgent)?;
    let artifact_cache =
        CodexArtifactProfileCache::new(provider_kind.clone(), ports.artifact_profiles());
    let configured_build = profile.default_snapshot().desktop_build.parse::<u64>().ok();
    let verified_profile = match artifact_cache.load().await {
        Ok(cached) => cached.filter(|cached| {
            cached
                .desktop_build
                .parse::<u64>()
                .ok()
                .zip(configured_build)
                .is_some_and(|(cached, configured)| cached >= configured)
        }),
        Err(error) => {
            tracing::warn!(error = %error, "OpenAI artifact profile cache could not be loaded");
            None
        }
    };
    let session_identity = config
        .session_identity()
        .map_err(|_| OpenAiInitializeError::SessionIdentity)?;
    let http = build_reqwest_client().map_err(|_| OpenAiInitializeError::Transport)?;
    let desktop_release = Arc::new(CodexDesktopReleaseService::new(
        profile.clone(),
        Arc::new(
            OfficialCodexDesktopReleaseTransport::new()
                .map_err(|_| OpenAiInitializeError::DesktopRelease)?,
        ),
        artifact_cache,
        verified_profile,
    ));
    let desktop_release_status = desktop_release.status();
    let repository = CodexCredentialRepository::new(Arc::clone(&accounts));
    let websocket_pool = Arc::new({
        let pool = CodexWebSocketPool::with_config(config.websocket_pool_config());
        if enforce_account_concurrency {
            pool.with_request_tuning(request_tuning.clone())
        } else {
            pool.with_request_tuning_without_account_concurrency(request_tuning.clone())
        }
    });
    let catalog = CodexCredentialCatalogService::new(
        repository.clone(),
        profile.clone(),
        http.clone(),
        config.base_url().to_owned(),
        ports.catalog_cache(),
    );
    let catalog = Arc::new(match &egress_runtime {
        Some(runtime) => catalog.with_egress_runtime(Arc::clone(runtime)),
        None => catalog,
    });
    let quota = CodexCredentialQuotaService::new(
        repository.clone(),
        profile.clone(),
        http.clone(),
        config.base_url().to_owned(),
        ports.cooldowns(),
    )
    .with_request_tuning(request_tuning.clone());
    let quota = Arc::new(match &egress_runtime {
        Some(runtime) => quota.with_egress_runtime(Arc::clone(runtime)),
        None => quota,
    });
    let profile_statistics = CodexCredentialProfileService::new(
        repository.clone(),
        profile.clone(),
        http.clone(),
        config.base_url().to_owned(),
    );
    let profile_statistics = Arc::new(match &egress_runtime {
        Some(runtime) => profile_statistics.with_egress_runtime(Arc::clone(runtime)),
        None => profile_statistics,
    });
    let selector = Arc::new(
        CodexCredentialSelector::new(
            provider_kind.clone(),
            repository.clone(),
            Arc::clone(&leases),
            session_affinity,
            session_exclusions,
            Arc::clone(&quota),
            Arc::clone(&account_feedback),
            CodexCookiePolicy::official().map_err(|_| OpenAiInitializeError::CookiePolicy)?,
        )
        .with_account_concurrency(request_tuning.account_concurrency()),
    );
    let core_provider = CodexProvider::new(
        Arc::clone(&selector),
        Arc::clone(&catalog),
        Arc::clone(&quota),
        account_feedback,
        http,
        profile.clone(),
        config.base_url().to_owned(),
        Arc::clone(&websocket_pool),
        config.stream_max_retries(),
    )
    .map_err(OpenAiInitializeError::Provider)?
    .with_request_tuning(request_tuning.clone())
    .with_excel_replay(ports.replay())
    .with_excel_image_relay(Arc::clone(&image_relay))
    .with_session_identity(session_identity);
    let core_provider: Arc<dyn Provider> = Arc::new(match &egress_runtime {
        Some(runtime) => core_provider.with_egress_runtime(Arc::clone(runtime)),
        None => core_provider,
    });
    let token_client = Arc::new(
        credential::token_client::openai_token_client(
            config.token_client_config(),
            profile.clone(),
        )
        .map_err(|_| OpenAiInitializeError::TokenClient)?,
    );
    let refresher: Arc<dyn TokenRefresher> = token_client.clone();
    let exchanger: Arc<dyn AuthorizationCodeExchanger> = token_client.clone();
    let credential_admin = Arc::new(
        CodexCredentialAdminService::new(
            Arc::clone(&refresher),
            Arc::clone(&leases),
            Arc::clone(&runtime_policy),
        )
        .with_personal_access_token_client(token_client),
    );
    let refresh = CodexCredentialRefreshService::new(
        repository,
        refresher,
        Arc::clone(&leases),
        credential_state,
        Arc::clone(&runtime_policy),
    );
    let refresh = Arc::new(match &egress_runtime {
        Some(runtime) => refresh.with_egress_runtime(Arc::clone(runtime)),
        None => refresh,
    });
    let pending = Arc::new(OpenAiOAuthPendingStore::new(
        ports.oauth_pending(),
        provider_kind.clone(),
    ));
    let oauth_admin: Arc<dyn CodexOAuthAdmin> = Arc::new(
        CodexOAuthAdminService::new(
            pending,
            exchanger,
            Arc::clone(&accounts),
            CodexCredentialAdmin,
            profile.clone(),
        )
        .with_oauth_client_id(config.oauth_client_id()),
    );
    let admin_provider = OpenAiAdminProvider::new(
        provider_kind,
        profile,
        accounts,
        OpenAiAdminServices {
            credentials: credential_admin,
            oauth: oauth_admin,
            profile_statistics,
            quota: Arc::clone(&quota),
            catalog: Arc::clone(&catalog),
        },
        websocket_pool,
        desktop_release_status,
    );
    let admin_provider: Arc<dyn ProviderAdmin> = Arc::new(match &egress_runtime {
        Some(runtime) => admin_provider.with_egress_runtime(Arc::clone(runtime)),
        None => admin_provider,
    });
    let worker_contributions = provider::worker_contributions(
        refresh,
        quota,
        catalog,
        config.quota_refresh_policy(),
        config.oauth_refresh_enabled(),
        desktop_release,
    )
    .map_err(|_| OpenAiInitializeError::Worker)?;

    Ok(ProviderBundle {
        image_relay,
        core_provider,
        admin_provider,
        worker_contributions,
    })
}

impl ProviderBundle {
    pub fn image_relay(&self) -> Arc<dyn gateway_core::provider_ports::TemporaryImageSource> {
        Arc::clone(&self.image_relay)
    }
    #[must_use]
    pub fn core_provider(&self) -> Arc<dyn Provider> {
        Arc::clone(&self.core_provider)
    }

    #[must_use]
    pub fn admin_provider(&self) -> Arc<dyn ProviderAdmin> {
        Arc::clone(&self.admin_provider)
    }

    /// 一次性移交 Host 任务计划，防止同一 owner 被重复注册。
    pub fn take_worker_contributions(&mut self) -> Vec<WorkerContribution> {
        std::mem::take(&mut self.worker_contributions)
    }
}

/// OpenAI 初始化失败的脱敏分类。
#[derive(Debug, thiserror::Error)]
pub enum OpenAiInitializeError {
    #[error("OpenAI outbound egress configuration is unavailable")]
    Egress,
    #[error(transparent)]
    Config(OpenAiConfigError),
    #[error("OpenAI runtime policy is unavailable")]
    RuntimePolicy,
    #[error("OpenAI persisted outbound user-agent is invalid")]
    UserAgent,
    #[error("OpenAI Provider kind is invalid")]
    InvalidProviderKind,
    #[error("OpenAI local session identity is unavailable")]
    SessionIdentity,
    #[error("OpenAI transport could not initialize")]
    Transport,
    #[error(transparent)]
    Provider(CodexProviderConfigError),
    #[error("OpenAI cookie policy could not initialize")]
    CookiePolicy,
    #[error("OpenAI token client could not initialize")]
    TokenClient,
    #[error("OpenAI credential administration could not initialize")]
    CredentialAdmin,
    #[error("OpenAI credential refresh could not initialize")]
    Refresh,
    #[error("OpenAI Desktop release service could not initialize")]
    DesktopRelease,
    #[error("OpenAI durable device identity registry could not initialize")]
    DeviceRegistry,
    #[error("OpenAI worker plan is invalid")]
    Worker,
}
