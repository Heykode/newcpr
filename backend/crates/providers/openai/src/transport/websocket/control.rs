//! Control messages share the original socket and its compression policy.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use gateway_core::engine::response_control::{
    ResponseControlTransport, ResponseControlUnavailable,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::{
    self, Message,
    protocol::frame::{
        Frame,
        coding::{Data, OpCode},
    },
};

use super::CodexWebSocketExchangeError;

pub(super) enum PumpCommand {
    Send {
        message: Message,
        ack: oneshot::Sender<Result<(), CodexWebSocketExchangeError>>,
    },
}

pub(super) type SharedMessages = Arc<Mutex<mpsc::Receiver<Result<Message, tungstenite::Error>>>>;

pub(super) struct PumpControl {
    pub(super) tx_command: mpsc::Sender<PumpCommand>,
    pub(super) rx_message: SharedMessages,
    pub(super) closed: Arc<AtomicBool>,
    pub(super) compression_threshold: Option<usize>,
}

pub(super) async fn send_message(
    commands: &mpsc::Sender<PumpCommand>,
    closed: &AtomicBool,
    compression_threshold: Option<usize>,
    message: Message,
) -> Result<(), CodexWebSocketExchangeError> {
    if closed.load(Ordering::Acquire) || commands.is_closed() {
        return Err(CodexWebSocketExchangeError::SendNotStarted);
    }
    let message = match message {
        Message::Text(text)
            if compression_threshold.is_some_and(|threshold| text.len() < threshold) =>
        {
            Message::Frame(Frame::message(text, OpCode::Data(Data::Text), true))
        }
        Message::Binary(bytes)
            if compression_threshold.is_some_and(|threshold| bytes.len() < threshold) =>
        {
            Message::Frame(Frame::message(bytes, OpCode::Data(Data::Binary), true))
        }
        other => other,
    };
    let (ack, rx_ack) = oneshot::channel();
    commands
        .send(PumpCommand::Send { message, ack })
        .await
        .map_err(|_| CodexWebSocketExchangeError::SendNotStarted)?;
    // A missing acknowledgement cannot prove that the socket write never started.
    rx_ack
        .await
        .unwrap_or_else(|_| Err(tungstenite::Error::ConnectionClosed.into()))
}

#[async_trait]
impl ResponseControlTransport for PumpControl {
    async fn send(&self, payload: &str) -> Result<(), ResponseControlUnavailable> {
        tokio::time::timeout(
            std::time::Duration::from_secs(5 * 60),
            send_message(
                &self.tx_command,
                &self.closed,
                self.compression_threshold,
                Message::Text(payload.to_owned().into()),
            ),
        )
        .await
        .map_err(|_| ResponseControlUnavailable)?
        .map_err(|_| ResponseControlUnavailable)
    }

    async fn receive(&self) -> Result<String, ResponseControlUnavailable> {
        match self.rx_message.lock().await.recv().await {
            Some(Ok(Message::Text(text))) => Ok(text.to_string()),
            _ => Err(ResponseControlUnavailable),
        }
    }
}
