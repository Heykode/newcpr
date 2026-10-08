//! 下游客户端 WebSocket 的单 owner pump 与有界收发边界。

use std::{
    collections::VecDeque,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::extract::ws::{CloseFrame, Message, WebSocket, close_code};
use futures::{Sink, SinkExt, Stream, StreamExt};
use gateway_core::lifecycle::CancellationToken;
use thiserror::Error;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot},
    task::JoinHandle,
    time::{Instant, timeout},
};

const OUTBOUND_COMMAND_BUFFER: usize = 32;
const INBOUND_EVENT_BUFFER: usize = 32;
const DOWNSTREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const CONNECTION_MAX_AGE: Duration = Duration::from_secs(60 * 60);
const DOWNSTREAM_PING_INTERVAL: Duration = Duration::from_secs(25);

/// WebSocket pump 的写入与生命周期预算。
#[derive(Clone, Copy)]
pub struct ConnectionConfig {
    write_timeout: Duration,
    max_age: Duration,
}

impl ConnectionConfig {
    /// 生产环境固定预算。
    pub const PRODUCTION: Self = Self {
        write_timeout: DOWNSTREAM_WRITE_TIMEOUT,
        max_age: CONNECTION_MAX_AGE,
    };
}

/// 业务层可观察的客户端输入；Ping/Pong 始终由 pump 消费。
pub enum ConnectionEvent {
    Text(String),
    Binary,
    Expired,
    Exited(PumpExitReason),
}

pub(super) struct PendingConnectionEvent {
    pub(super) event: ConnectionEvent,
    _permit: Option<OwnedSemaphorePermit>,
}

impl PendingConnectionEvent {
    fn exited(reason: PumpExitReason) -> Self {
        Self {
            event: ConnectionEvent::Exited(reason),
            _permit: None,
        }
    }
}

/// 下游写入阶段；名称刻意使用 write，而不是暗示客户端已消费的 delivery。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePhase {
    Metadata,
    First,
    Data,
    Terminal,
    FirstAndTerminal,
    Error,
    ConnectionLimit,
    Close,
}

impl FramePhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::First => "first",
            Self::Data => "data",
            Self::Terminal => "terminal",
            Self::FirstAndTerminal => "first_and_terminal",
            Self::Error => "error",
            Self::ConnectionLimit => "connection_limit",
            Self::Close => "close",
        }
    }

    const fn is_milestone(self) -> bool {
        matches!(
            self,
            Self::First
                | Self::Terminal
                | Self::FirstAndTerminal
                | Self::Error
                | Self::ConnectionLimit
                | Self::Close
        )
    }
}

/// 一次下游写入的请求归属与协议阶段。
#[derive(Clone)]
pub struct WriteContext {
    request_id: Option<Arc<str>>,
    phase: FramePhase,
}

impl WriteContext {
    /// 创建归属于某个请求的写入上下文。
    #[must_use]
    pub fn request(request_id: &Arc<str>, phase: FramePhase) -> Self {
        Self {
            request_id: Some(Arc::clone(request_id)),
            phase,
        }
    }

    /// 创建连接级写入上下文。
    #[must_use]
    pub const fn connection(phase: FramePhase) -> Self {
        Self {
            request_id: None,
            phase,
        }
    }
}

/// WebSocket pump 停止的稳定原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpExitReason {
    ClientClose,
    PeerEof,
    ReadError,
    WriteError,
    WriteTimeout,
    LifecycleShutdown,
    ConnectionMaxAge,
    CoordinatorDropped,
    InboundOverload,
    ServerClose,
    PumpStopped,
}

impl PumpExitReason {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::ClientClose => "client_close",
            Self::PeerEof => "peer_eof",
            Self::ReadError => "read_error",
            Self::WriteError => "write_error",
            Self::WriteTimeout => "write_timeout",
            Self::LifecycleShutdown => "lifecycle_shutdown",
            Self::ConnectionMaxAge => "connection_max_age",
            Self::CoordinatorDropped => "coordinator_dropped",
            Self::InboundOverload => "inbound_overload",
            Self::ServerClose => "server_close",
            Self::PumpStopped => "pump_stopped",
        }
    }
}

/// 下游 WebSocket 写入失败。
#[derive(Debug, Error)]
pub enum ConnectionWriteError {
    #[error("downstream WebSocket pump is closed")]
    Closed,
    #[error("downstream WebSocket write timed out after {timeout:?}")]
    Timeout { timeout: Duration },
    #[error("downstream WebSocket transport write failed: {message}")]
    Transport { message: String },
}

struct ConnectionCommand {
    message: Message,
    context: WriteContext,
    acknowledged: oneshot::Sender<Result<(), ConnectionWriteError>>,
}

#[derive(Default)]
struct ConnectionStats {
    command_queue_high_water: AtomicUsize,
    command_backpressure_count: AtomicU64,
    ping_received_count: AtomicU64,
    ping_written_count: AtomicU64,
    pong_received_count: AtomicU64,
    last_read_ms: AtomicU64,
    last_write_ms: AtomicU64,
}

impl ConnectionStats {
    fn record_read(&self, opened_at: Instant) {
        self.last_read_ms.store(
            u64::try_from(opened_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    fn record_write(&self, opened_at: Instant) {
        self.last_write_ms.store(
            u64::try_from(opened_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    fn observe_command_queue(&self, sender: &mpsc::Sender<ConnectionCommand>) {
        let queued = OUTBOUND_COMMAND_BUFFER
            .saturating_sub(sender.capacity())
            .saturating_add(1)
            .min(OUTBOUND_COMMAND_BUFFER);
        self.command_queue_high_water
            .fetch_max(queued, Ordering::Relaxed);
        if sender.capacity() == 0 {
            self.command_backpressure_count
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// 协调层持有的单 owner WebSocket 连接句柄。
pub struct ResponsesWebSocketConnection {
    request_trace: gateway_core::diagnostics::TraceContext,
    connection_id: Arc<str>,
    opened_at: Instant,
    expired: Arc<AtomicBool>,
    commands: Option<mpsc::Sender<ConnectionCommand>>,
    incoming: mpsc::Receiver<PendingConnectionEvent>,
    deferred: VecDeque<PendingConnectionEvent>,
    exited: oneshot::Receiver<PumpExitReason>,
    pump_task: Option<JoinHandle<()>>,
    stats: Arc<ConnectionStats>,
    config: ConnectionConfig,
    exit_reason: Option<PumpExitReason>,
}

impl ResponsesWebSocketConnection {
    pub(super) fn set_request_trace(&mut self, trace: gateway_core::diagnostics::TraceContext) {
        self.request_trace = trace;
    }
    pub(super) fn new(
        socket: WebSocket,
        connection_id: String,
        cancellation: CancellationToken,
    ) -> Self {
        spawn_connection(
            socket,
            Arc::<str>::from(connection_id),
            cancellation,
            ConnectionConfig::PRODUCTION,
        )
    }

    pub(super) fn id(&self) -> &str {
        &self.connection_id
    }

    /// 返回连接是否已经达到生命周期上限。
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expired.load(Ordering::Acquire)
    }

    pub(super) fn age(&self) -> Duration {
        self.opened_at.elapsed()
    }

    /// 等待下一个需要业务层处理的客户端事件。
    pub async fn next_event(&mut self) -> Option<ConnectionEvent> {
        if self.exit_reason.is_none() {
            match self.exited.try_recv() {
                Ok(reason) => self.exit_reason = Some(reason),
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.exit_reason = Some(PumpExitReason::PumpStopped)
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if let Some(reason) = self.exit_reason {
            return Some(ConnectionEvent::Exited(reason));
        }
        if let Some(event) = self.deferred.pop_front() {
            return Some(event.event);
        }
        self.next_active_event().await.map(|event| event.event)
    }

    pub(super) async fn next_active_event(&mut self) -> Option<PendingConnectionEvent> {
        if let Some(reason) = self.exit_reason {
            return Some(PendingConnectionEvent::exited(reason));
        }
        let event = tokio::select! {
            biased;
            reason = &mut self.exited => {
                Some(PendingConnectionEvent::exited(reason.unwrap_or(PumpExitReason::PumpStopped)))
            }
            event = self.incoming.recv() => event,
        };
        if let Some(PendingConnectionEvent {
            event: ConnectionEvent::Exited(reason),
            ..
        }) = event.as_ref()
        {
            self.exit_reason.get_or_insert(*reason);
        } else if event.is_none() {
            self.exit_reason.get_or_insert(PumpExitReason::PumpStopped);
        }
        event
    }

    pub(super) fn defer(&mut self, event: PendingConnectionEvent) {
        self.deferred.push_back(event);
    }

    /// 等待连接退出，不消费留给后续串行请求的业务帧。
    pub async fn wait_for_exit(&mut self) -> PumpExitReason {
        if let Some(reason) = self.exit_reason {
            return reason;
        }
        let reason = (&mut self.exited)
            .await
            .unwrap_or(PumpExitReason::PumpStopped);
        self.exit_reason = Some(reason);
        reason
    }

    /// 串行写入文本帧，并等待 pump 确认 transport 写入结果。
    ///
    /// # Errors
    ///
    /// pump 已关闭、写入超时或 transport 失败时返回稳定错误。
    pub async fn send_text(
        &mut self,
        payload: String,
        context: WriteContext,
    ) -> Result<(), ConnectionWriteError> {
        if context.request_id.is_some() {
            self.request_trace
                .dump("downstream.event", payload.as_bytes());
        }
        let result = self.send(Message::Text(payload.into()), context).await;
        if self.request_trace.captures_bodies()
            && matches!(
                result,
                Err(ConnectionWriteError::Timeout { .. } | ConnectionWriteError::Transport { .. })
            )
        {
            self.request_trace
                .record("downstream.write.failed", serde_json::json!({}));
        }
        result
    }

    pub(super) async fn close_policy(
        &mut self,
        reason: &'static str,
        request_id: Option<&Arc<str>>,
    ) {
        let context = request_id.map_or_else(
            || WriteContext::connection(FramePhase::Close),
            |request_id| WriteContext::request(request_id, FramePhase::Close),
        );
        self.close(
            CloseFrame {
                code: close_code::POLICY,
                reason: reason.into(),
            },
            context,
            PumpExitReason::ServerClose,
        )
        .await;
    }

    pub(super) async fn close_for_connection_limit(&mut self, reason: &'static str) {
        self.close(
            CloseFrame {
                code: close_code::NORMAL,
                reason: reason.into(),
            },
            WriteContext::connection(FramePhase::Close),
            PumpExitReason::ConnectionMaxAge,
        )
        .await;
    }

    async fn close(
        &mut self,
        frame: CloseFrame,
        context: WriteContext,
        exit_reason: PumpExitReason,
    ) {
        if self
            .send(Message::Close(Some(frame)), context)
            .await
            .is_ok()
        {
            self.exit_reason.get_or_insert(exit_reason);
        }
    }

    async fn send(
        &mut self,
        message: Message,
        context: WriteContext,
    ) -> Result<(), ConnectionWriteError> {
        let Some(commands) = self.commands.as_ref().cloned() else {
            return Err(ConnectionWriteError::Closed);
        };
        self.stats.observe_command_queue(&commands);
        let (acknowledged, acknowledgement) = oneshot::channel();
        let started_at = Instant::now();
        let request_id = context.request_id.clone();
        let phase = context.phase;
        let command = ConnectionCommand {
            message,
            context,
            acknowledged,
        };
        let write = async move {
            commands
                .send(command)
                .await
                .map_err(|_| ConnectionWriteError::Closed)?;
            acknowledgement
                .await
                .map_err(|_| ConnectionWriteError::Closed)?
        };
        let result = match timeout(self.config.write_timeout, write).await {
            Ok(result) => result,
            Err(_) => Err(ConnectionWriteError::Timeout {
                timeout: self.config.write_timeout,
            }),
        };
        let duration = started_at.elapsed();
        match &result {
            Ok(()) if phase.is_milestone() => tracing::info!(
                websocket_connection_id = %self.connection_id,
                request_id = request_id.as_deref().unwrap_or(""),
                frame_phase = phase.as_str(),
                write_duration_ms = duration.as_millis(),
                "Responses WebSocket frame write succeeded"
            ),
            Ok(()) => tracing::debug!(
                websocket_connection_id = %self.connection_id,
                request_id = request_id.as_deref().unwrap_or(""),
                frame_phase = phase.as_str(),
                write_duration_ms = duration.as_millis(),
                "Responses WebSocket frame write succeeded"
            ),
            Err(error) => tracing::info!(
                websocket_connection_id = %self.connection_id,
                request_id = request_id.as_deref().unwrap_or(""),
                frame_phase = phase.as_str(),
                write_duration_ms = duration.as_millis(),
                error = %error,
                "Responses WebSocket frame write failed"
            ),
        }
        if let Err(error) = &result {
            let reason = match error {
                ConnectionWriteError::Timeout { .. } => PumpExitReason::WriteTimeout,
                ConnectionWriteError::Transport { .. } => PumpExitReason::WriteError,
                ConnectionWriteError::Closed => PumpExitReason::PumpStopped,
            };
            self.terminate(reason);
        }
        result
    }

    fn terminate(&mut self, reason: PumpExitReason) {
        self.exit_reason.get_or_insert(reason);
        self.commands.take();
        if let Some(task) = self.pump_task.take() {
            task.abort();
        }
    }

    pub(super) fn log_summary(&self, request_count: u64) {
        let connection_age_ms = self.age().as_millis();
        let read_idle_ms = connection_age_ms
            .saturating_sub(u128::from(self.stats.last_read_ms.load(Ordering::Relaxed)));
        let write_idle_ms = connection_age_ms
            .saturating_sub(u128::from(self.stats.last_write_ms.load(Ordering::Relaxed)));
        tracing::info!(
            websocket_connection_id = %self.connection_id,
            request_count,
            connection_age_ms,
            pump_exit_reason = self
                .exit_reason
                .map_or("unknown", PumpExitReason::as_str),
            command_queue_high_water = self
                .stats
                .command_queue_high_water
                .load(Ordering::Relaxed),
            command_backpressure_count = self
                .stats
                .command_backpressure_count
                .load(Ordering::Relaxed),
            ping_received_count = self.stats.ping_received_count.load(Ordering::Relaxed),
            ping_written_count = self.stats.ping_written_count.load(Ordering::Relaxed),
            pong_received_count = self.stats.pong_received_count.load(Ordering::Relaxed),
            read_idle_ms,
            write_idle_ms,
            "Responses WebSocket disconnected"
        );
    }
}

impl Drop for ResponsesWebSocketConnection {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(task) = self.pump_task.take() {
            task.abort();
        }
    }
}

/// 为一个 socket 启动单 owner pump，并返回协调层句柄。
pub fn spawn_connection<S, E>(
    socket: S,
    connection_id: Arc<str>,
    cancellation: CancellationToken,
    config: ConnectionConfig,
) -> ResponsesWebSocketConnection
where
    S: Stream<Item = Result<Message, E>> + Sink<Message, Error = E> + Unpin + Send + 'static,
    E: fmt::Display + Send + 'static,
{
    let opened_at = Instant::now();
    let expired = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(ConnectionStats::default());
    let (command_tx, command_rx) = mpsc::channel(OUTBOUND_COMMAND_BUFFER);
    let (incoming_tx, incoming_rx) = mpsc::channel(INBOUND_EVENT_BUFFER);
    let (exit_tx, exit_rx) = oneshot::channel();
    let pump_task = tokio::spawn(run_pump(
        socket,
        command_rx,
        incoming_tx,
        exit_tx,
        Arc::clone(&connection_id),
        cancellation,
        opened_at,
        Arc::clone(&expired),
        Arc::clone(&stats),
        config,
    ));
    ResponsesWebSocketConnection {
        request_trace: Default::default(),
        connection_id,
        opened_at,
        expired,
        commands: Some(command_tx),
        incoming: incoming_rx,
        deferred: VecDeque::new(),
        exited: exit_rx,
        pump_task: Some(pump_task),
        stats,
        config,
        exit_reason: None,
    }
}

#[expect(clippy::too_many_arguments)]
async fn run_pump<S, E>(
    socket: S,
    mut commands: mpsc::Receiver<ConnectionCommand>,
    incoming: mpsc::Sender<PendingConnectionEvent>,
    exited: oneshot::Sender<PumpExitReason>,
    connection_id: Arc<str>,
    cancellation: CancellationToken,
    opened_at: Instant,
    expired: Arc<AtomicBool>,
    stats: Arc<ConnectionStats>,
    config: ConnectionConfig,
) where
    S: Stream<Item = Result<Message, E>> + Sink<Message, Error = E> + Unpin,
    E: fmt::Display,
{
    let (mut writer, mut reader) = socket.split();
    // Both halves belong to this task. A blocked write cannot hide close or cancellation.
    let read = async {
        let event_slots = Arc::new(Semaphore::new(INBOUND_EVENT_BUFFER));
        let deadline = tokio::time::sleep_until(opened_at + config.max_age);
        tokio::pin!(deadline);
        let mut deadline_elapsed = false;
        loop {
            tokio::select! {
                biased;
                () = &mut deadline, if !deadline_elapsed => {
                    deadline_elapsed = true;
                    expired.store(true, Ordering::Release);
                    // Expiry prevents the next request without interrupting the active response.
                    match emit_incoming(&incoming, &event_slots, ConnectionEvent::Expired) {
                        Ok(()) | Err(PumpExitReason::InboundOverload) => {}
                        Err(reason) => break reason,
                    }
                }
                message = reader.next() => {
                    if matches!(&message, Some(Ok(_))) {
                        stats.record_read(opened_at);
                    }
                    let event = match message {
                        Some(Ok(Message::Text(payload))) => Some(ConnectionEvent::Text(payload.to_string())),
                        Some(Ok(Message::Binary(_))) => Some(ConnectionEvent::Binary),
                        Some(Ok(Message::Ping(_))) => {
                            // Axum/tungstenite queues Pong and flushes it when reading continues.
                            stats.ping_received_count.fetch_add(1, Ordering::Relaxed);
                            None
                        }
                        Some(Ok(Message::Pong(_))) => {
                            stats.pong_received_count.fetch_add(1, Ordering::Relaxed);
                            None
                        }
                        Some(Ok(Message::Close(_))) => break PumpExitReason::ClientClose,
                        Some(Err(error)) => {
                            tracing::info!(
                                websocket_connection_id = %connection_id,
                                error = %error,
                                "Responses WebSocket receive failed"
                            );
                            break PumpExitReason::ReadError;
                        }
                        None => break PumpExitReason::PeerEof,
                    };
                    if let Some(event) = event
                        && let Err(reason) = emit_incoming(&incoming, &event_slots, event)
                    {
                        break reason;
                    }
                }
            }
        }
    };
    let write = async {
        let mut heartbeat = tokio::time::interval_at(
            opened_at + DOWNSTREAM_PING_INTERVAL,
            DOWNSTREAM_PING_INTERVAL,
        );
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut ping_sequence = 0_u64;
        loop {
            let command = tokio::select! {
                biased;
                command = commands.recv() => {
                    let Some(command) = command else {
                        break PumpExitReason::CoordinatorDropped;
                    };
                    Some(command)
                }
                _ = heartbeat.tick() => None,
            };
            let (message, acknowledgement) = if let Some(command) = command {
                (
                    command.message,
                    Some((command.context, command.acknowledged)),
                )
            } else {
                ping_sequence = ping_sequence.wrapping_add(1);
                (
                    Message::Ping(ping_sequence.to_be_bytes().to_vec().into()),
                    None,
                )
            };
            let closing = matches!(message, Message::Close(_));
            let result = match timeout(config.write_timeout, writer.send(message)).await {
                Ok(Ok(())) => {
                    stats.record_write(opened_at);
                    if acknowledgement.is_none() {
                        stats.ping_written_count.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(())
                }
                Ok(Err(error)) => Err(ConnectionWriteError::Transport {
                    message: error.to_string(),
                }),
                Err(_) => Err(ConnectionWriteError::Timeout {
                    timeout: config.write_timeout,
                }),
            };
            let failure = result.as_ref().err().map(|error| match error {
                ConnectionWriteError::Timeout { .. } => PumpExitReason::WriteTimeout,
                ConnectionWriteError::Transport { .. } => PumpExitReason::WriteError,
                ConnectionWriteError::Closed => PumpExitReason::PumpStopped,
            });
            if let Some((context, acknowledged)) = acknowledgement {
                if let Err(error) = &result {
                    tracing::debug!(
                        websocket_connection_id = %connection_id,
                        request_id = context.request_id.as_deref().unwrap_or(""),
                        frame_phase = context.phase.as_str(),
                        error = %error,
                        "Responses WebSocket pump transport write failed"
                    );
                }
                let _ = acknowledged.send(result);
            }
            if let Some(reason) = failure {
                break reason;
            }
            if closing {
                break PumpExitReason::ServerClose;
            }
        }
    };
    let reason = tokio::select! {
        biased;
        () = cancellation.cancelled() => PumpExitReason::LifecycleShutdown,
        reason = read => reason,
        reason = write => reason,
    };
    if reason == PumpExitReason::ClientClose {
        // Reading Close queues the acknowledgement. Flush it once, without waiting
        // for the ordinary five-minute write budget or sending a duplicate Close.
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {}
            result = timeout(Duration::from_secs(1), writer.flush()) => {
                if !matches!(result, Ok(Ok(()))) {
                    tracing::debug!(websocket_connection_id = %connection_id, "Responses WebSocket close acknowledgement could not be flushed");
                }
            }
        }
    }
    let _ = exited.send(reason);
    tracing::debug!(
        websocket_connection_id = %connection_id,
        pump_exit_reason = reason.as_str(),
        "Responses WebSocket pump exited"
    );
}

fn emit_incoming(
    incoming: &mpsc::Sender<PendingConnectionEvent>,
    slots: &Arc<Semaphore>,
    event: ConnectionEvent,
) -> Result<(), PumpExitReason> {
    let permit = Arc::clone(slots)
        .try_acquire_owned()
        .map_err(|_| PumpExitReason::InboundOverload)?;
    incoming
        .try_send(PendingConnectionEvent {
            event,
            _permit: Some(permit),
        })
        .map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => PumpExitReason::InboundOverload,
            mpsc::error::TrySendError::Closed(_) => PumpExitReason::CoordinatorDropped,
        })
}
