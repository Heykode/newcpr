//! Pooled QX HTTP transport with the existing reqwest response/body interface.

use std::{
    collections::VecDeque,
    error::Error,
    future::Future,
    io,
    net::Ipv6Addr,
    pin::Pin,
    sync::{Mutex, OnceLock},
    task::{Context, Poll},
    time::Duration,
};

use base64::Engine;
use bytes::Bytes;
use gateway_core::account::OutboundProxy;
use hyper::{
    Uri,
    body::{Body, Frame, Incoming, SizeHint},
    rt::{Read, ReadBufCursor, Write},
};
use hyper_util::{
    client::legacy::{
        Client,
        connect::{Connected, Connection},
    },
    rt::{TokioExecutor, TokioIo, TokioTimer},
};
use reqwest::ResponseBuilderExt;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::{Instant, Sleep},
};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};
use tower_service::Service;

pub use super::native_tls::NativeTransportError;
use super::{
    dial::{self, DialConfig},
    native_tls::{NativeAlpn, NativeTlsConnector},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CACHED_CLIENTS: usize = 128;
type HttpClient = Client<NativeConnector, reqwest::Body>;
type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeHttpConfig {
    pub cache_key: String,
    pub proxy: Option<OutboundProxy>,
    pub source: Option<Ipv6Addr>,
    pub fresh: bool,
    pub timeout: Option<Duration>,
}

/// Optional per-execution bounds, also enforced while polling the response body.
#[derive(Clone, Default)]
pub struct NativeRequestPolicy {
    pub deadline: Option<Instant>,
    pub cancellation: Option<CancellationToken>,
}

#[derive(Clone)]
pub struct NativeHttpClient {
    client: HttpClient,
    config: NativeHttpConfig,
}

struct CacheEntry {
    ca_key: Option<String>,
    config: NativeHttpConfig,
    client: NativeHttpClient,
}

static CLIENTS: OnceLock<Mutex<VecDeque<CacheEntry>>> = OnceLock::new();

impl NativeHttpClient {
    pub fn cached(config: NativeHttpConfig) -> Result<Self, NativeTransportError> {
        if config.fresh {
            return Self::new(config);
        }
        let ca_key = super::tls::custom_ca_env_cache_key();
        if let Some(client) = cached_entry(&config, &ca_key) {
            return Ok(client);
        }
        // Trust loading and OpenSSL configuration do not hold the cache lock.
        let client = Self::new(config.clone())?;
        let mut entries = CLIENTS
            .get_or_init(|| Mutex::new(VecDeque::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries
            .iter()
            .find(|e| e.config == config && e.ca_key == ca_key)
        {
            return Ok(entry.client.clone());
        }
        while entries.len() >= MAX_CACHED_CLIENTS {
            entries.pop_front();
        }
        entries.push_back(CacheEntry {
            ca_key,
            config,
            client: client.clone(),
        });
        Ok(client)
    }

    /// Hot hits do not schedule work; cold trust loading never blocks the runtime.
    pub async fn cached_async(config: NativeHttpConfig) -> Result<Self, NativeTransportError> {
        if !config.fresh
            && let Some(client) = cached_entry(&config, &super::tls::custom_ca_env_cache_key())
        {
            return Ok(client);
        }
        tokio::task::spawn_blocking(move || Self::cached(config))
            .await
            .map_err(NativeTransportError::configuration)?
    }

    pub fn new(config: NativeHttpConfig) -> Result<Self, NativeTransportError> {
        Self::with_tls(config, NativeTlsConnector::from_environment()?)
    }

    pub fn with_tls(
        config: NativeHttpConfig,
        tls: NativeTlsConnector,
    ) -> Result<Self, NativeTransportError> {
        if config.cache_key.len() > 8192 {
            return Err(NativeTransportError::configuration(io::Error::other(
                "transport cache identity is too large",
            )));
        }
        let dial =
            DialConfig::with_proxy_tls(config.proxy.clone(), config.source, tls.proxy_tls())?;
        let connector = NativeConnector { dial, tls };
        let mut builder = Client::builder(TokioExecutor::new());
        builder
            .pool_timer(TokioTimer::new())
            .timer(TokioTimer::new())
            .pool_idle_timeout(None)
            .pool_max_idle_per_host(if config.fresh { 0 } else { 4 })
            // In particular, never replay a refresh token on a stale pooled connection.
            .retry_canceled_requests(false)
            .http2_keep_alive_interval(Some(Duration::from_secs(30)))
            .http2_keep_alive_timeout(Duration::from_secs(5))
            .http2_keep_alive_while_idle(true);
        Ok(Self {
            client: builder.build(connector),
            config,
        })
    }

    pub async fn execute(
        &self,
        request: reqwest::Request,
    ) -> Result<reqwest::Response, NativeTransportError> {
        self.execute_with_policy(request, NativeRequestPolicy::default())
            .await
    }

    pub async fn execute_with_policy(
        &self,
        mut request: reqwest::Request,
        policy: NativeRequestPolicy,
    ) -> Result<reqwest::Response, NativeTransportError> {
        let timeout = request.timeout().copied().or(self.config.timeout);
        let timeout_deadline = timeout.and_then(|duration| Instant::now().checked_add(duration));
        let deadline = match (policy.deadline, timeout_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        request.url_mut().set_fragment(None);
        let url = request.url().clone();
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(NativeTransportError::configuration(io::Error::other(
                "unsupported origin URL",
            )));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(NativeTransportError::configuration(io::Error::other(
                "origin URL must not contain credentials",
            )));
        }
        let mut request: hyper::Request<reqwest::Body> = request
            .try_into()
            .map_err(NativeTransportError::configuration)?;
        // Proxy credentials belong only on a cleartext HTTP forward request.
        request
            .headers_mut()
            .remove(hyper::header::PROXY_AUTHORIZATION);
        if url.scheme() == "http"
            && let Some(proxy) = &self.config.proxy
            && (proxy.expose_url().starts_with("http:") || proxy.expose_url().starts_with("https:"))
        {
            let parsed = proxy.expose_url().replacen("https:", "http:", 1);
            let proxy = tungstenite::proxy::ProxyConfig::parse(&parsed).map_err(|_| {
                NativeTransportError::configuration(io::Error::other("invalid proxy"))
            })?;
            if let Some(auth) = proxy.auth {
                let token = base64::engine::general_purpose::STANDARD
                    .encode(format!("{}:{}", auth.username, auth.password));
                let mut value = hyper::header::HeaderValue::from_str(&format!("Basic {token}"))
                    .map_err(NativeTransportError::configuration)?;
                value.set_sensitive(true);
                request
                    .headers_mut()
                    .insert(hyper::header::PROXY_AUTHORIZATION, value);
            }
        }
        let cancelled = async {
            match &policy.cancellation {
                Some(token) => token.cancelled().await,
                None => std::future::pending::<()>().await,
            }
        };
        let elapsed = async {
            match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
        };
        let response = tokio::select! {
            biased;
            _ = cancelled => return Err(NativeTransportError::request_error(
                io::Error::new(io::ErrorKind::Interrupted, "request cancelled"),
            )),
            _ = elapsed => return Err(NativeTransportError::request_timeout()),
            response = self.client.request(request) => response.map_err(classify_request_error)?,
        };
        let (parts, body) = response.into_parts();
        let mut response = hyper::Response::builder()
            .url(url)
            .status(parts.status)
            .version(parts.version)
            .body(reqwest::Body::wrap(ControlledBody::new(
                body,
                deadline,
                policy.cancellation,
            )))
            .map_err(NativeTransportError::configuration)?;
        *response.headers_mut() = parts.headers;
        response.extensions_mut().extend(parts.extensions);
        Ok(response.into())
    }
}

fn cached_entry(config: &NativeHttpConfig, ca_key: &Option<String>) -> Option<NativeHttpClient> {
    let mut entries = CLIENTS
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = entries
        .iter()
        .position(|entry| &entry.config == config && &entry.ca_key == ca_key)?;
    let entry = entries.remove(index)?;
    let client = entry.client.clone();
    entries.push_back(entry);
    Some(client)
}

fn classify_request_error(error: hyper_util::client::legacy::Error) -> NativeTransportError {
    let mut source = error.source();
    let mut timeout = false;
    let mut configuration = false;
    while let Some(cause) = source {
        if let Some(native) = cause.downcast_ref::<NativeTransportError>() {
            timeout |= native.is_timeout();
            configuration |= native.is_configuration();
        }
        source = cause.source();
    }
    if configuration {
        NativeTransportError::configuration(error)
    } else if error.is_connect() {
        NativeTransportError::Connect {
            source: Box::new(error),
            timeout,
        }
    } else {
        NativeTransportError::Request {
            source: Box::new(error),
            timeout,
        }
    }
}

#[derive(Clone)]
struct NativeConnector {
    dial: DialConfig,
    tls: NativeTlsConnector,
}

impl Service<Uri> for NativeConnector {
    type Response = NativeIo;
    type Error = NativeTransportError;
    type Future = Pin<Box<dyn Future<Output = Result<NativeIo, NativeTransportError>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let connector = self.clone();
        Box::pin(async move {
            tokio::time::timeout(CONNECT_TIMEOUT, async {
                let host = uri.host().ok_or_else(|| {
                    NativeTransportError::configuration(io::Error::other("missing origin host"))
                })?;
                let secure = uri.scheme_str() == Some("https");
                let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
                let forward = !secure && connector.dial.is_http_proxy();
                let stream = if forward {
                    dial::connect_inner(host, port, &connector.dial, false).await?
                } else {
                    dial::connect(host, port, &connector.dial).await?
                };
                let (stream, h2): (Box<dyn AsyncIo>, bool) = if secure {
                    let stream = connector
                        .tls
                        .connect(stream, host, NativeAlpn::Http)
                        .await?;
                    let h2 = stream.ssl().selected_alpn_protocol() == Some(b"h2");
                    (Box::new(stream), h2)
                } else {
                    (Box::new(stream), false)
                };
                Ok(NativeIo {
                    inner: TokioIo::new(stream),
                    h2,
                    forward,
                })
            })
            .await
            .map_err(|_| NativeTransportError::connect_timeout())?
        })
    }
}

trait AsyncIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncIo for T {}

struct NativeIo {
    inner: TokioIo<Box<dyn AsyncIo>>,
    h2: bool,
    forward: bool,
}

impl Connection for NativeIo {
    fn connected(&self) -> Connected {
        let connected = Connected::new().proxy(self.forward);
        if self.h2 {
            connected.negotiated_h2()
        } else {
            connected
        }
    }
}

impl Read for NativeIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}

impl Write for NativeIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, buffers)
    }
}

struct ControlledBody {
    inner: Option<Pin<Box<Incoming>>>,
    deadline: Option<Pin<Box<Sleep>>>,
    cancellation: Option<Pin<Box<WaitForCancellationFutureOwned>>>,
}

impl ControlledBody {
    fn new(body: Incoming, deadline: Option<Instant>, token: Option<CancellationToken>) -> Self {
        Self {
            inner: Some(Box::pin(body)),
            deadline: deadline.map(|deadline| Box::pin(tokio::time::sleep_until(deadline))),
            cancellation: token.map(|token| Box::pin(token.cancelled_owned())),
        }
    }
}

impl Body for ControlledBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        if self.inner.is_none() {
            return Poll::Ready(None);
        }
        let failure = if self
            .cancellation
            .as_mut()
            .is_some_and(|f| f.as_mut().poll(cx).is_ready())
        {
            Some(io::Error::new(
                io::ErrorKind::Interrupted,
                "response cancelled",
            ))
        } else if self
            .deadline
            .as_mut()
            .is_some_and(|f| f.as_mut().poll(cx).is_ready())
        {
            Some(io::Error::new(
                io::ErrorKind::TimedOut,
                "response deadline elapsed",
            ))
        } else {
            None
        };
        if let Some(error) = failure {
            // Drop Incoming immediately, not only when the caller drops this wrapper.
            self.inner.take();
            return Poll::Ready(Some(Err(error.into())));
        }
        match self
            .inner
            .as_mut()
            .expect("body present")
            .as_mut()
            .poll_frame(cx)
        {
            Poll::Ready(None) => {
                self.inner.take();
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                self.inner.take();
                Poll::Ready(Some(Err(Box::new(error))))
            }
            Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.as_ref().is_none_or(|body| body.is_end_stream())
    }

    fn size_hint(&self) -> SizeHint {
        self.inner
            .as_ref()
            .map_or_else(|| SizeHint::with_exact(0), |body| body.size_hint())
    }
}
