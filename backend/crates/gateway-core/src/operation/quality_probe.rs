//! Trusted, request-local quality evidence. Opaque state is never serialized to a result.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::ProviderSessionState;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateProbeVerdict {
    Healthy,
    Degraded,
    #[default]
    Inconclusive,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateProbeReason {
    Unchanged,
    Changed,
    #[default]
    MissingEvidence,
    MissingTicket,
    UnsupportedTransport,
    UnverifiedEgress,
    AccountChanged,
    ExcelEnabled,
    UnsupportedAccount,
    RequestFailed,
    UpstreamOverloaded,
    Cancelled,
    RepeatedAttempt,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateProbeShot {
    pub transport: Option<String>,
    pub status: Option<u16>,
    pub ticket_length: usize,
    pub changed: Option<bool>,
    pub reason: Option<StateProbeReason>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateProbeReport {
    pub verdict: StateProbeVerdict,
    pub reason: StateProbeReason,
    pub shots: Vec<StateProbeShot>,
}

#[derive(Default)]
struct Exchange {
    attempts: [bool; 2],
    state: Option<ProviderSessionState>,
    shots: [Option<StateProbeShot>; 2],
}

/// Only Core creates this context; no request body/header can opt into it.
#[derive(Clone, Default)]
pub struct QualityProbeExchange(Arc<Mutex<Exchange>>);

impl std::fmt::Debug for QualityProbeExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("QualityProbeExchange(<private>)")
    }
}

impl PartialEq for QualityProbeExchange {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl QualityProbeExchange {
    #[must_use]
    pub fn step(&self, continuation: bool) -> QualityProbeStep {
        QualityProbeStep {
            exchange: self.clone(),
            continuation,
        }
    }

    #[must_use]
    pub fn report(&self) -> StateProbeReport {
        let Ok(exchange) = self.0.lock() else {
            return StateProbeReport::default();
        };
        let shots = exchange.shots.iter().flatten().cloned().collect::<Vec<_>>();
        let reason = shots.iter().find_map(|shot| shot.reason);
        let (verdict, reason) = if let Some(reason) = reason {
            (StateProbeVerdict::Inconclusive, reason)
        } else if exchange.shots.iter().all(Option::is_some) {
            match shots[1].changed {
                Some(true) => (StateProbeVerdict::Degraded, StateProbeReason::Changed),
                Some(false) => (StateProbeVerdict::Healthy, StateProbeReason::Unchanged),
                None => (
                    StateProbeVerdict::Inconclusive,
                    StateProbeReason::MissingEvidence,
                ),
            }
        } else {
            (
                StateProbeVerdict::Inconclusive,
                StateProbeReason::MissingEvidence,
            )
        };
        StateProbeReport {
            verdict,
            reason,
            shots,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QualityProbeStep {
    exchange: QualityProbeExchange,
    continuation: bool,
}

impl QualityProbeStep {
    #[must_use]
    pub const fn is_continuation(&self) -> bool {
        self.continuation
    }

    /// Retries must not turn two observations into an unbounded harvesting loop.
    pub fn begin(&self) -> bool {
        let Ok(mut exchange) = self.exchange.0.lock() else {
            return false;
        };
        let index = usize::from(self.continuation);
        if exchange.attempts[index] {
            exchange.shots[index]
                .get_or_insert_with(StateProbeShot::default)
                .reason = Some(StateProbeReason::RepeatedAttempt);
            return false;
        }
        exchange.attempts[index] = true;
        true
    }

    #[must_use]
    pub fn input(&self) -> Option<ProviderSessionState> {
        self.continuation
            .then(|| self.exchange.0.lock().ok()?.state.clone())
            .flatten()
    }

    pub fn observe(&self, shot: StateProbeShot, state: Option<ProviderSessionState>) {
        if let Ok(mut exchange) = self.exchange.0.lock() {
            exchange.shots[usize::from(self.continuation)] = Some(shot);
            if !self.continuation {
                exchange.state = state;
            }
        }
    }
}
