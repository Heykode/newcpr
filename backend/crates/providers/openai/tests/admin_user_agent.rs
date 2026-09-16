use std::sync::Arc;

use futures::future::BoxFuture;
use gateway_core::{
    provider_ports::{
        ProviderRefreshPolicy, ProviderRuntimePolicyPort, ProviderSessionPolicy,
        ProviderStoreError, ProviderStorePorts, ProviderTlsProfile, ProviderUserAgentOverride,
    },
    routing::ProviderKind,
};

use super::admin::{provider_ports, valid_config};

const CUSTOM: &str =
    "Codex Desktop/0.153.4 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.901.51231)";

struct CustomRuntimePolicy(ProviderUserAgentOverride);

impl ProviderRuntimePolicyPort for CustomRuntimePolicy {
    fn load_refresh_policy(
        &self,
    ) -> BoxFuture<'_, Result<ProviderRefreshPolicy, ProviderStoreError>> {
        Box::pin(async {
            ProviderRefreshPolicy::try_new(
                std::time::Duration::from_secs(300),
                std::num::NonZeroU32::new(4).unwrap(),
            )
        })
    }

    fn load_user_agent_override<'a>(
        &'a self,
        _: &'a ProviderKind,
    ) -> BoxFuture<'a, Result<ProviderUserAgentOverride, ProviderStoreError>> {
        Box::pin(async { Ok(self.0.clone()) })
    }
}

fn ports(selection: ProviderUserAgentOverride) -> ProviderStorePorts {
    let original = provider_ports();
    ProviderStorePorts::new(
        original.accounts(),
        original.leases(),
        original.session_affinity(),
        original.session_exclusions(),
        original.catalog_cache(),
        original.artifact_profiles(),
        original.credential_state(),
        original.cooldowns(),
        Arc::new(CustomRuntimePolicy(selection)),
        original.oauth_pending(),
    )
}

#[tokio::test]
async fn initializer_restores_custom_ua_without_claiming_artifact_verification() {
    let config = valid_config();
    let ports = ports(ProviderUserAgentOverride::Custom {
        user_agent: CUSTOM.to_owned(),
    });
    assert!(
        ports.egress().is_none(),
        "legacy stores do not require an egress port"
    );
    let bundle = provider_openai::initialize(config.config.clone(), ports)
        .await
        .expect("initialized with stored custom profile");
    let provider = bundle.admin_provider();
    let settings = provider.outbound_user_agent().unwrap();
    assert_eq!(settings.effective_user_agent, CUSTOM);
    assert!(!settings.verified);
    assert!(
        provider
            .dashboard_wire_profile()
            .unwrap()
            .verified_at
            .is_none()
    );
    let restored = provider
        .apply_outbound_user_agent(ProviderUserAgentOverride::Default)
        .unwrap();
    assert!(restored.verified);
    assert_eq!(restored.effective_user_agent, restored.default_user_agent);
}

#[tokio::test]
async fn initializer_rejects_invalid_persisted_ua_instead_of_silent_default() {
    let config = valid_config();
    let result = provider_openai::initialize(
        config.config.clone(),
        ports(ProviderUserAgentOverride::Custom {
            user_agent: "invalid persisted UA".to_owned(),
        }),
    )
    .await;
    assert!(matches!(
        result,
        Err(provider_openai::OpenAiInitializeError::UserAgent)
    ));
}

#[tokio::test]
async fn qx_initializer_preview_and_apply_preserve_explicit_selection_without_verification_claims()
{
    use provider_openai::transport::profile::qx;

    let config = valid_config();
    let bundle = provider_openai::initialize(
        config.config.clone(),
        ports(ProviderUserAgentOverride::QxCompatible { user_agent: None }),
    )
    .await
    .unwrap();
    let provider = bundle.admin_provider();
    let current = provider.outbound_user_agent().unwrap();
    assert_eq!(
        current.selection,
        ProviderUserAgentOverride::QxCompatible { user_agent: None }
    );
    assert_eq!(current.effective_user_agent, qx::DEFAULT_USER_AGENT);
    assert_eq!(current.qx_default_user_agent, qx::DEFAULT_USER_AGENT);
    assert!(!current.verified);
    assert!(
        current
            .effective_desktop_user_agent
            .starts_with("Codex Desktop/")
    );
    assert!(
        provider
            .dashboard_wire_profile()
            .unwrap()
            .verified_at
            .is_none()
    );

    let custom = ProviderUserAgentOverride::QxCompatible {
        user_agent: Some("codex_cli_rs/0.146.0 (Linux 6.8.0; x86_64) unknown".to_owned()),
    };
    let preview = provider.preview_outbound_user_agent(&custom).unwrap();
    assert_eq!(preview.selection, custom);
    assert!(!preview.verified);
    assert_eq!(provider.outbound_user_agent().unwrap(), current);
    assert_eq!(provider.apply_outbound_user_agent(custom).unwrap(), preview);

    let blank = provider
        .apply_outbound_user_agent(ProviderUserAgentOverride::QxCompatible {
            user_agent: Some("   ".to_owned()),
        })
        .unwrap();
    assert_eq!(blank, current);
    assert!(
        provider
            .apply_outbound_user_agent(ProviderUserAgentOverride::QxCompatible {
                user_agent: Some(CUSTOM.to_owned()),
            })
            .is_err()
    );
    assert_eq!(provider.outbound_user_agent().unwrap(), current);
    assert!(
        provider
            .apply_outbound_user_agent(ProviderUserAgentOverride::Default)
            .unwrap()
            .verified
    );
}

#[tokio::test]
async fn independent_initializer_preview_and_apply_round_trip_every_dimension() {
    for user_agent in [
        None,
        Some(CUSTOM.to_owned()),
        Some(provider_openai::transport::profile::qx::DEFAULT_USER_AGENT.to_owned()),
    ] {
        for tls_profile in [ProviderTlsProfile::Cpr, ProviderTlsProfile::QxCompatible] {
            for session_policy in [
                ProviderSessionPolicy::Native,
                ProviderSessionPolicy::QxCompatible,
            ] {
                let selection = ProviderUserAgentOverride::Independent {
                    user_agent: user_agent.clone(),
                    tls_profile,
                    session_policy,
                };
                let config = valid_config();
                let bundle =
                    provider_openai::initialize(config.config.clone(), ports(selection.clone()))
                        .await
                        .unwrap();
                let provider = bundle.admin_provider();
                let loaded = provider.outbound_user_agent().unwrap();
                assert_eq!(loaded.selection, selection);
                assert_eq!(
                    loaded.effective_user_agent,
                    user_agent.as_deref().unwrap_or(&loaded.default_user_agent)
                );
                assert_eq!(
                    loaded.verified,
                    user_agent.is_none()
                        && tls_profile == ProviderTlsProfile::Cpr
                        && session_policy == ProviderSessionPolicy::Native
                );
                let preview = provider.preview_outbound_user_agent(&selection).unwrap();
                assert_eq!(preview, loaded);
                assert_eq!(provider.outbound_user_agent().unwrap(), loaded);
                assert_eq!(
                    provider.apply_outbound_user_agent(selection).unwrap(),
                    preview
                );
                let invalid = ProviderUserAgentOverride::Independent {
                    user_agent: Some(String::new()),
                    tls_profile: ProviderTlsProfile::Cpr,
                    session_policy: ProviderSessionPolicy::Native,
                };
                assert!(provider.preview_outbound_user_agent(&invalid).is_err());
                assert!(provider.apply_outbound_user_agent(invalid).is_err());
                assert_eq!(provider.outbound_user_agent().unwrap(), loaded);
            }
        }
    }
}
