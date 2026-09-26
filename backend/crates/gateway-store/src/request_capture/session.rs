//! Bounded request-owned buffers. Successful requests never reach disk.

use chrono::Utc;
use gateway_admin::model::request_capture::{CaptureScope, CaptureTask};
use gateway_core::diagnostics::request_capture::{RequestCaptureFactory, RequestCaptureObserver};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering},
    },
};

use super::CaptureManager;

pub(super) const BUFFER_LIMIT: usize = 64 * 1024 * 1024;
const RECORD_LIMIT: usize = 128 * 1024 * 1024;
const MAX_FRAGMENTS: usize = 512;
const MAX_SESSIONS: usize = 256;
const MAX_ATTEMPTS: usize = 256;

pub(super) struct ActiveTask {
    pub task: CaptureTask,
    pub stopped: AtomicBool,
}
impl ActiveTask {
    fn accepts(&self) -> bool {
        !self.stopped.load(Ordering::Acquire) && self.task.expires_at > Utc::now()
    }
}

pub(super) struct Budget(pub AtomicUsize);
pub(super) struct Reservation {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Budget {
    fn reserve(self: &Arc<Self>, bytes: usize) -> Option<Reservation> {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= BUFFER_LIMIT)
            })
            .ok()?;
        Some(Reservation {
            budget: self.clone(),
            bytes,
        })
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.0.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

pub(super) struct Fragment {
    pub stage: &'static str,
    pub attempt: u32,
    pub exchange: Option<u64>,
    pub account: Option<String>,
    pub task_mask: u16,
    pub bytes: Vec<u8>,
    pub _reservation: Reservation,
}

#[derive(Default)]
struct Observations {
    fragments: Vec<Fragment>,
    accounts: BTreeMap<u32, String>,
    completed: BTreeSet<u32>,
    failed_accounts: BTreeSet<String>,
    error: bool,
    finished: bool,
    bytes: usize,
}

pub(super) struct CaptureBundle {
    pub request_id: String,
    pub tasks: Vec<(usize, Arc<ActiveTask>)>,
    pub fragments: Vec<Fragment>,
    pub failed_accounts: BTreeSet<String>,
    pub accounts: BTreeSet<String>,
    pub incomplete: bool,
    pub incomplete_tasks: u16,
}

struct Session {
    manager: Arc<super::Shared>,
    request_id: String,
    tasks: Vec<Arc<ActiveTask>>,
    state: Mutex<Observations>,
    incomplete: AtomicBool,
    incomplete_tasks: AtomicU16,
}

impl RequestCaptureFactory for CaptureManager {
    fn start(
        &self,
        request_id: &str,
        key_id: &str,
        groups: &[&str],
    ) -> Option<Arc<dyn RequestCaptureObserver>> {
        let Ok(control) = self.shared.control.try_read() else {
            self.shared.skipped.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        if !control.config.enabled || self.shared.fault.load(Ordering::Acquire) {
            return None;
        }
        let tasks: Vec<_> = control
            .tasks
            .iter()
            .filter(|task| {
                task.accepts()
                    && match task.task.scope {
                        CaptureScope::Key => task.task.target_id == key_id,
                        CaptureScope::Group => groups.contains(&task.task.target_id.as_str()),
                        CaptureScope::Account => true,
                    }
            })
            .cloned()
            .collect();
        if tasks.is_empty() {
            return None;
        }
        if self
            .shared
            .sessions
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_SESSIONS).then_some(count + 1)
            })
            .is_err()
        {
            self.shared.skipped.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Some(Arc::new(Session {
            manager: self.shared.clone(),
            request_id: request_id.into(),
            tasks,
            state: Mutex::new(Observations::default()),
            incomplete: AtomicBool::new(false),
            incomplete_tasks: AtomicU16::new(0),
        }))
    }
}

impl RequestCaptureObserver for Session {
    fn body(&self, stage: &'static str, attempt: u32, exchange: Option<u64>, bytes: &[u8]) {
        if !matches!(
            stage,
            "client.request.body"
                | "upstream.request.body"
                | "upstream.error.body"
                | "upstream.response.body"
                | "upstream.chunk"
                | "upstream.event"
                | "upstream.binary"
                | "downstream.body"
                | "downstream.chunk"
                | "downstream.event"
        ) || bytes.is_empty()
            || self.manager.fault.load(Ordering::Acquire)
        {
            return;
        }
        let Ok(mut state) = self.state.try_lock() else {
            self.incomplete.store(true, Ordering::Release);
            return;
        };
        let account = state.accounts.get(&attempt).cloned();
        let mut task_mask = 0u16;
        for (index, task) in self.tasks.iter().enumerate() {
            if !task.accepts() {
                self.incomplete_tasks
                    .fetch_or(1 << index, Ordering::Relaxed);
            } else if !stage.starts_with("upstream.")
                || task.task.scope != CaptureScope::Account
                || account.as_deref() == Some(task.task.target_id.as_str())
            {
                task_mask |= 1 << index;
            }
        }
        if task_mask == 0 {
            return;
        }
        if state.fragments.len() >= MAX_FRAGMENTS
            || bytes.len() > RECORD_LIMIT.saturating_sub(state.bytes)
        {
            self.incomplete.store(true, Ordering::Release);
            return;
        }
        let Some(reservation) = self.manager.budget.reserve(bytes.len().saturating_add(512)) else {
            self.incomplete.store(true, Ordering::Release);
            return;
        };
        state.bytes += bytes.len();
        state.fragments.push(Fragment {
            stage,
            attempt,
            exchange,
            account,
            task_mask,
            bytes: bytes.to_vec(),
            _reservation: reservation,
        });
    }

    fn fact(&self, stage: &'static str, attempt: u32, data: &Value) {
        if matches!(
            stage,
            "excel.transport" | "excel.transport.failed" | "upstream.business_result"
        ) {
            // Fixed-label diagnostic facts only. Never copy arbitrary error text.
            let mut summary = json!({"stage":stage,"attempt":attempt});
            for key in ["phase", "cause", "outcome"] {
                if let Some(value) = data.get(key).and_then(Value::as_str).filter(|value| {
                    matches!(
                        *value,
                        "prepare"
                            | "connect"
                            | "exchange"
                            | "receive"
                            | "decode"
                            | "response_headers"
                            | "attachment_upload_started"
                            | "attachment_upload_completed"
                            | "exchange_started"
                            | "request_build_failed"
                            | "connect_timeout"
                            | "connect_failed"
                            | "timeout"
                            | "body_read_failed"
                            | "decode_failed"
                            | "transport_failed"
                            | "upstream_rejected"
                            | "invalid_stream"
                            | "connection_refused"
                            | "connection_reset"
                            | "unexpected_eof"
                            | "broken_pipe"
                            | "completed"
                            | "failed"
                            | "incomplete"
                            | "unverified"
                    )
                }) {
                    summary[key] = value.into();
                }
            }
            if let Some(status) = data["status"]
                .as_u64()
                .filter(|status| (100..=599).contains(status))
            {
                summary["status"] = status.into();
            }
            if let Ok(bytes) = serde_json::to_vec(&summary) {
                self.body("downstream.event", attempt, None, &bytes);
            }
            return;
        }
        let Ok(mut state) = self.state.try_lock() else {
            self.incomplete.store(true, Ordering::Release);
            return;
        };
        if stage == "account.selection" {
            if let Some(account) = data["selectedAccountId"].as_str() {
                if state.accounts.len() < MAX_ATTEMPTS || state.accounts.contains_key(&attempt) {
                    state.accounts.insert(attempt, account.to_owned());
                } else {
                    self.incomplete.store(true, Ordering::Release);
                }
            }
            return;
        }
        if stage == "upstream.completed" {
            if state.completed.len() < MAX_ATTEMPTS {
                state.completed.insert(attempt);
            }
            return;
        }
        // Completion is an authoritative protocol fact, not a prefix/SSE heuristic.
        if stage == "request.finished" {
            state.finished = true;
        }
        let failed = matches!(stage, "attempt.failed" | "downstream.write.failed")
            || stage == "downstream.status"
                && data["status"].as_u64().is_some_and(|status| status >= 400)
            || stage == "request.finished"
                && matches!(data["outcome"].as_str(), Some("Failed" | "Incomplete"))
                && (state.completed.is_empty() || data["errorKind"] != "cancelled");
        if failed {
            state.error = true;
            if let Some(account) = state.accounts.get(&attempt).cloned() {
                state.failed_accounts.insert(account);
            }
            drop(state);
            // Retain only a bounded summary, never raw error text or headers.
            let summary = json!({"stage":stage,"attempt":attempt,
                "errorKind":data.get("kind").or_else(|| data.get("errorKind")),
                "upstreamStatus":data.get("upstreamStatus")});
            if let Ok(bytes) = serde_json::to_vec(&summary) {
                self.body("downstream.event", attempt, None, &bytes);
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.manager.sessions.fetch_sub(1, Ordering::AcqRel);
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let incomplete = self.incomplete.load(Ordering::Acquire) || !state.finished;
        if !state.error {
            if incomplete {
                self.manager.skipped.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        let tasks: Vec<_> = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| {
                task.task.scope != CaptureScope::Account
                    || state.failed_accounts.contains(&task.task.target_id)
                    || state
                        .accounts
                        .values()
                        .any(|account| account == &task.task.target_id)
            })
            .map(|(index, task)| (index, task.clone()))
            .collect();
        if tasks.is_empty() {
            return;
        }
        let bundle = CaptureBundle {
            request_id: self.request_id.clone(),
            tasks,
            fragments: std::mem::take(&mut state.fragments),
            failed_accounts: std::mem::take(&mut state.failed_accounts),
            accounts: state.accounts.values().cloned().collect(),
            incomplete,
            incomplete_tasks: self.incomplete_tasks.load(Ordering::Acquire),
        };
        if self.manager.sender.try_send(bundle).is_err() {
            self.manager.skipped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
