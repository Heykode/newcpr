use std::fmt;

use serde_json::{Map, Value, json};

/// Local scheduling hints, ordered from highest to lowest priority.
pub const OPENAI_SCHEDULING_SESSION_HINT_HEADERS: [&str; 6] = [
    "x-session-affinity",
    "x-session-id",
    "x-opencode-session",
    "x-conversation-id",
    "x-qx-affinity-codex-session",
    "x-qx-affinity-claude-session",
];

/// Non-wire protocol context key populated only by the ingress transport.
pub const OPENAI_SCHEDULING_SESSION_HINT_CONTEXT_KEY: &str = "openai_scheduling_session_hint";

/// Classify a lower-case local scheduling header that must not be forwarded.
#[must_use]
pub fn is_openai_scheduling_session_hint_header(name: &str) -> bool {
    OPENAI_SCHEDULING_SESSION_HINT_HEADERS.contains(&name)
}

/// A source-qualified hint for account scheduling, never an upstream identity.
#[derive(Clone, PartialEq, Eq)]
pub struct OpenAiSchedulingSessionHint {
    source: &'static str,
    id: String,
}

impl OpenAiSchedulingSessionHint {
    /// Validate a known source and a trimmed, bounded ASCII graphic identifier.
    #[must_use]
    pub fn new(source: &str, value: &str) -> Option<Self> {
        let source = OPENAI_SCHEDULING_SESSION_HINT_HEADERS
            .iter()
            .copied()
            .find(|candidate| *candidate == source)?;
        let id = value.trim();
        if id.is_empty() || id.len() > 1024 || !id.bytes().all(|byte| byte.is_ascii_graphic()) {
            return None;
        }
        Some(Self {
            source,
            id: id.to_owned(),
        })
    }

    /// Return the canonical header source.
    #[must_use]
    pub const fn source(&self) -> &'static str {
        self.source
    }

    /// Return the normalized scheduling identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Project the validated value into non-wire protocol context.
    #[must_use]
    pub fn to_context_value(&self) -> Value {
        json!({"source": self.source, "id": self.id})
    }

    /// Read only API-created protocol context, never the client's wire body.
    #[must_use]
    pub fn from_context(context: &Map<String, Value>) -> Option<Self> {
        let hint = context
            .get(OPENAI_SCHEDULING_SESSION_HINT_CONTEXT_KEY)?
            .as_object()?;
        Self::new(hint.get("source")?.as_str()?, hint.get("id")?.as_str()?)
    }
}

impl fmt::Debug for OpenAiSchedulingSessionHint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiSchedulingSessionHint")
            .field("source", &self.source)
            .field("id", &"<redacted>")
            .finish()
    }
}
