//! Request-local managed State evidence, never part of routing or trace output.

use std::sync::Mutex;

use gateway_core::provider_ports::OpaqueTurnState;
use serde_json::{Value, json};

use super::client::CodexBackendTransport;

#[derive(Debug, Default)]
pub(crate) struct TurnStateCapture(Mutex<Option<TurnStateSnapshot>>);

#[derive(Debug)]
struct TurnStateSnapshot {
    injected: Option<OpaqueTurnState>,
    transport: CodexBackendTransport,
    returned_chars: Option<usize>,
    returned_same: Option<bool>,
}

impl TurnStateCapture {
    pub(crate) fn dispatch(&self, transport: CodexBackendTransport, injected: Option<&str>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(TurnStateSnapshot {
            injected: injected
                .filter(|value| !value.is_empty() && value.len() <= 2048)
                .map(|value| OpaqueTurnState::new(value.to_owned())),
            transport,
            returned_chars: None,
            returned_same: None,
        });
    }

    pub(crate) fn returned(&self, value: Option<&str>) {
        let Some(value) = value else { return };
        let mut snapshot = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(snapshot) = snapshot.as_mut() else {
            return;
        };
        snapshot.returned_chars = Some(value.chars().count());
        snapshot.returned_same = snapshot
            .injected
            .as_ref()
            .map(|injected| injected.expose_to_provider() == value);
    }

    /// Full value is restricted to authenticated usage detail, not the list summary.
    pub(crate) fn snapshot(&self) -> Option<Value> {
        let snapshot = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = snapshot.as_ref()?;
        let injected = snapshot
            .injected
            .as_ref()
            .map(OpaqueTurnState::expose_to_provider);
        let chars = injected.map(|value| value.chars().count());
        let preview = injected.map(|value| {
            let head: String = value.chars().take(8).collect();
            let tail: String = value
                .chars()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("{head}...{tail}")
        });
        Some(json!({
            "injectedState": injected,
            "summary": {
                "injected": injected.is_some(),
                "preview": preview,
                "chars": chars,
                "returnedChars": snapshot.returned_chars,
                "returnedSame": snapshot.returned_same,
                "transport": match snapshot.transport {
                    CodexBackendTransport::HttpSse => "http_sse",
                    CodexBackendTransport::WebSocket => "websocket",
                    CodexBackendTransport::HttpJson => "http_json",
                },
            },
        }))
    }
}
