//! Explicit outbound UA policy, separate from transport and transient sessions.

/// A user-agent override is explicit even when it equals the current default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProviderUserAgentOverride {
    #[default]
    Default,
    Custom {
        user_agent: String,
    },
}

impl ProviderUserAgentOverride {
    #[must_use]
    pub fn custom_user_agent(&self) -> Option<&str> {
        match self {
            Self::Default => None,
            Self::Custom { user_agent } => Some(user_agent),
        }
    }
}
