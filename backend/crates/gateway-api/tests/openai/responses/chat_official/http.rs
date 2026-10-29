//! Native execution fixture for the unmodified upstream Chat protocol tests.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::SystemTime;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{
        Request,
        header::{AUTHORIZATION, CONTENT_TYPE},
    },
    response::Response,
};
use futures::future::BoxFuture;
use gateway_core::engine::execution::{
    AuthenticatedClient, ClientAuthenticationError, ExecutionService, ExecutionSession,
    StartExecution, StartProviderExecution, StartedExecution,
};
use gateway_core::engine::{CommitRequirement, CoordinatedEvent, EngineError, ModelRequestId};
use gateway_core::error::GatewayError;
use gateway_core::event::{GatewayEvent, ProviderEvent, ProviderResponseHeader};
use gateway_core::operation::Operation;
use gateway_core::routing::PublicModelId;
use serde_json::Value;
use tokio::sync::oneshot;
use tower::ServiceExt;

use super::super::http::{FakeSession, NextStep, Trace as NativeTrace, delivery_provider};
use crate::openai::{api_router, authenticated_client_for_provider};

pub(super) const KEY: &str = "sk_chat_fixture";

#[derive(Default)]
pub(super) struct Trace {
    pub calls: Arc<NativeTrace>,
    pub commits: AtomicUsize,
    pub discarded: AtomicUsize,
    pub finalized: AtomicBool,
}

pub(super) struct Session {
    events: Vec<ProviderEvent>,
    batches: Vec<Vec<ProviderEvent>>,
    pub steps: Option<Vec<NextStep>>,
    pub collect_error: Option<EngineError>,
    pub final_step: NextStep,
    pub fail_commit: bool,
    pub headers: Vec<ProviderResponseHeader>,
    pub gate: Option<(usize, oneshot::Receiver<()>)>,
}

impl Session {
    pub fn new(events: Vec<ProviderEvent>) -> Self {
        Self {
            events,
            batches: Vec::new(),
            steps: None,
            collect_error: None,
            final_step: NextStep::FinalizeSuccess,
            fail_commit: false,
            headers: Vec::new(),
            gate: None,
        }
    }

    pub fn with_batch(mut self, events: Vec<ProviderEvent>) -> Self {
        self.batches.push(events);
        self
    }

    fn bind(self, trace: Arc<Trace>, streaming: bool) -> ChatSession {
        let inner = if streaming {
            let mut steps = self.steps.unwrap_or_else(|| {
                let mut steps: Vec<_> = self
                    .events
                    .into_iter()
                    .map(|event| {
                        NextStep::Event(delivery_provider(
                            event,
                            CommitRequirement::CommitBeforeDelivery,
                        ))
                    })
                    .collect();
                steps.extend(self.batches.into_iter().map(|batch| {
                    NextStep::Event(
                        CoordinatedEvent::try_batch(batch, CommitRequirement::CommitBeforeDelivery)
                            .unwrap(),
                    )
                }));
                steps
            });
            steps.push(self.final_step);
            FakeSession::streaming(Arc::clone(&trace.calls), steps)
        } else {
            let events = self
                .events
                .into_iter()
                .chain(self.batches.into_iter().flatten())
                .collect();
            let inner = FakeSession::buffered_provider(Arc::clone(&trace.calls), events);
            match self.collect_error {
                Some(error) => inner.with_collect_error(error),
                None => inner,
            }
        };
        let inner = inner.with_response_headers(self.headers);
        let inner = if self.fail_commit {
            inner.with_commit_failure()
        } else {
            inner
        };
        ChatSession {
            gate: self.gate,
            ..ChatSession::new(inner, trace)
        }
    }
}

#[derive(Clone)]
pub(super) struct Captured {
    pub body: Value,
    pub context: Value,
    pub endpoint: String,
    pub transport: String,
    pub client_ip: Option<std::net::IpAddr>,
    pub user_agent: Option<String>,
}

pub(super) struct Execution {
    pub client: AuthenticatedClient,
    pub captured: Mutex<Option<Captured>>,
    session: Mutex<Option<Session>>,
    trace: Arc<Trace>,
}

impl Execution {
    pub fn new(session: Session) -> Self {
        Self {
            client: authenticated_client_for_provider(KEY, "openai"),
            captured: Mutex::new(None),
            session: Mutex::new(Some(session)),
            trace: Arc::new(Trace::default()),
        }
    }
}

impl ExecutionService for Execution {
    fn authenticate(
        &self,
        plaintext: &str,
    ) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        (plaintext == KEY)
            .then(|| self.client.clone())
            .ok_or(ClientAuthenticationError::InvalidKey)
    }

    fn public_models(&self, _: &AuthenticatedClient) -> Vec<PublicModelId> {
        vec![PublicModelId::new("model-a").unwrap()]
    }

    fn contains_public_model(&self, _: &AuthenticatedClient, model: &PublicModelId) -> bool {
        model.as_str() == "model-a"
    }

    fn start(
        &self,
        request: StartExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move {
            let Operation::Generate(generation) = &request.operation else {
                panic!("Chat must use the existing Generate operation");
            };
            *self.captured.lock().unwrap() = Some(Captured {
                body: Value::Object(generation.protocol_payload().body().clone()),
                context: Value::Object(generation.protocol_payload().context().clone()),
                endpoint: request.metadata.endpoint,
                transport: format!("{:?}", request.metadata.transport),
                client_ip: request.metadata.client_ip,
                user_agent: request.metadata.user_agent,
            });
            let session = self
                .session
                .lock()
                .unwrap()
                .take()
                .expect("one execution")
                .bind(Arc::clone(&self.trace), request.metadata.stream);
            Ok(StartedExecution {
                request_id: ModelRequestId::new("req_chat_fixture").unwrap(),
                created_at: SystemTime::now(),
                stream: request.metadata.stream,
                session: Box::new(session),
            })
        })
    }

    fn start_provider_endpoint(
        &self,
        _: StartProviderExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        panic!("Chat must not bypass model routing via a provider endpoint")
    }
}

pub(super) struct Fixture {
    pub router: Router,
    pub execution: Arc<Execution>,
    pub trace: Arc<Trace>,
}

impl Fixture {
    pub async fn new(session: Session) -> Self {
        Self::with_execution(Execution::new(session)).await
    }

    pub async fn with_execution(execution: Execution) -> Self {
        let execution = Arc::new(execution);
        let router = api_router(execution.clone()).await;
        let trace = Arc::clone(&execution.trace);
        Self {
            router,
            execution,
            trace,
        }
    }

    pub async fn post(&self, body: Value) -> Response {
        self.router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(AUTHORIZATION, format!("Bearer {KEY}"))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    pub fn captured(&self) -> Captured {
        self.execution
            .captured
            .lock()
            .unwrap()
            .clone()
            .expect("captured")
    }
}

pub(super) async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

// Keep the shared fake's trace, but enforce Core's pending-delivery handshake.
pub(super) struct ChatSession {
    inner: FakeSession,
    trace: Arc<Trace>,
    committed: bool,
    pending: bool,
    terminal: bool,
    pending_failure: Option<GatewayError>,
    cancelled: AtomicBool,
    delivered: usize,
    gate: Option<(usize, oneshot::Receiver<()>)>,
}

impl ChatSession {
    pub(super) fn new(inner: FakeSession, trace: Arc<Trace>) -> Self {
        Self {
            inner,
            trace,
            committed: false,
            pending: false,
            terminal: false,
            pending_failure: None,
            cancelled: AtomicBool::new(false),
            delivered: 0,
            gate: None,
        }
    }
}

impl ExecutionSession for ChatSession {
    fn next_event(&mut self) -> BoxFuture<'_, Result<Option<CoordinatedEvent>, EngineError>> {
        Box::pin(async move {
            let cancelled = self.cancelled.load(Ordering::Acquire);
            if self.pending && !cancelled {
                return Err(EngineError::DownstreamCommitRequired);
            }
            if let Some((after_events, release)) = self.gate.as_mut()
                && self.delivered == *after_events
                && !cancelled
            {
                release.await.map_err(|_| EngineError::Cancelled)?;
                self.gate = None;
            }
            let Some(event) = self.inner.next_event().await? else {
                return Ok(None);
            };
            self.delivered += 1;
            let events = event.into_provider_events();
            self.terminal = events.iter().any(|event| {
                event
                    .canonical_facts()
                    .iter()
                    .any(|fact| matches!(fact, GatewayEvent::Completed(_)))
                    || event.wire_event().is_some_and(|wire| {
                        matches!(
                            wire.event_type(),
                            Some(
                                "response.completed"
                                    | "response.incomplete"
                                    | "response.failed"
                                    | "error"
                            )
                        )
                    })
            });
            self.pending = !self.committed;
            CoordinatedEvent::try_batch(
                events,
                if self.committed {
                    CommitRequirement::AlreadyCommitted
                } else {
                    CommitRequirement::CommitBeforeDelivery
                },
            )
            .map(Some)
        })
    }

    fn collect_uncommitted(&mut self) -> BoxFuture<'_, Result<Vec<ProviderEvent>, EngineError>> {
        Box::pin(async move {
            let events = self.inner.collect_uncommitted().await?;
            self.pending = true;
            self.terminal = true;
            Ok(events)
        })
    }

    fn response_headers(&self) -> &[ProviderResponseHeader] {
        self.inner.response_headers()
    }

    fn defer_downstream_commit(&mut self) -> Result<(), EngineError> {
        if !self.pending || self.committed || self.terminal || self.pending_failure.is_some() {
            return Err(EngineError::InvalidDeliveryState);
        }
        self.inner.defer_downstream_commit()?;
        self.trace.discarded.fetch_add(1, Ordering::SeqCst);
        self.pending = false;
        Ok(())
    }

    fn commit_downstream(
        &mut self,
        client_status_code: Option<u16>,
    ) -> BoxFuture<'_, Result<(), EngineError>> {
        Box::pin(async move {
            if !self.pending || self.committed {
                return Err(EngineError::InvalidDeliveryState);
            }
            self.inner.commit_downstream(client_status_code).await?;
            self.trace.commits.fetch_add(1, Ordering::SeqCst);
            self.pending = false;
            self.committed = true;
            Ok(())
        })
    }

    fn record_client_status(
        &mut self,
        client_status_code: u16,
    ) -> BoxFuture<'_, Result<(), EngineError>> {
        self.inner.record_client_status(client_status_code)
    }

    fn is_finalized(&self) -> bool {
        let finalized = self.inner.is_finalized();
        self.trace.finalized.store(finalized, Ordering::SeqCst);
        finalized
    }

    fn fail_delivery(&mut self, error: GatewayError) -> BoxFuture<'_, Result<(), EngineError>> {
        // Mirror Core's pending-provider-error precedence without polling an
        // additional event after the adapter has already rejected delivery.
        self.inner
            .fail_delivery(self.pending_failure.take().unwrap_or(error))
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.inner.cancel();
    }

    fn detach_finalize(self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::new(self.inner).detach_finalize()
    }
}
