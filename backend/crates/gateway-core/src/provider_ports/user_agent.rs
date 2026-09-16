//! Explicit outbound client identity policy, separate from transient sessions.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ProviderTlsProfile {
    #[default]
    Cpr,
    QxCompatible,
}

impl ProviderTlsProfile {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpr => "cpr",
            Self::QxCompatible => "qx-compatible",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ProviderSessionPolicy {
    #[default]
    Native,
    QxCompatible,
}

impl ProviderSessionPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::QxCompatible => "qx-compatible",
        }
    }
}

/// A user-agent override is explicit even when it equals the current default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProviderUserAgentOverride {
    #[default]
    Default,
    Custom {
        user_agent: String,
    },
    QxCompatible {
        user_agent: Option<String>,
    },
    Independent {
        user_agent: Option<String>,
        tls_profile: ProviderTlsProfile,
        session_policy: ProviderSessionPolicy,
    },
}

impl ProviderUserAgentOverride {
    #[must_use]
    pub fn custom_user_agent(&self) -> Option<&str> {
        match self {
            Self::Default => None,
            Self::Custom { user_agent } => Some(user_agent),
            Self::QxCompatible { user_agent } | Self::Independent { user_agent, .. } => {
                user_agent.as_deref()
            }
        }
    }

    #[must_use]
    pub const fn tls_profile(&self) -> ProviderTlsProfile {
        match self {
            Self::Default | Self::Custom { .. } => ProviderTlsProfile::Cpr,
            Self::QxCompatible { .. } => ProviderTlsProfile::QxCompatible,
            Self::Independent { tls_profile, .. } => *tls_profile,
        }
    }

    #[must_use]
    pub const fn session_policy(&self) -> ProviderSessionPolicy {
        match self {
            Self::Default | Self::Custom { .. } => ProviderSessionPolicy::Native,
            Self::QxCompatible { .. } => ProviderSessionPolicy::QxCompatible,
            Self::Independent { session_policy, .. } => *session_policy,
        }
    }

    /// Empty QX overrides inherit the pinned QX default; nonblank grammar stays intact.
    #[must_use]
    pub fn normalized(self) -> Self {
        match self {
            Self::QxCompatible { user_agent } => Self::QxCompatible {
                user_agent: user_agent.filter(|value| !value.bytes().all(|byte| byte == b' ')),
            },
            selection => selection,
        }
    }
}
