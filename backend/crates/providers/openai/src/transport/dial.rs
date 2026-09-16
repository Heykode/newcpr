//! Shared direct and explicit-proxy dialer; no process proxy discovery.

use std::{io, net::Ipv6Addr, sync::Arc, time::Duration};

use gateway_core::account::OutboundProxy;
use hyper_util::client::legacy::connect::HttpConnector;
use tokio::net::{TcpStream, lookup_host};
use tokio_tungstenite::MaybeTlsStream;
use tower_service::Service;
use tungstenite::proxy::ProxyConfig;

use super::{egress::CodexEgressError, native_tls::NativeTransportError};

#[derive(Clone)]
pub struct DialConfig {
    proxy: Option<OutboundProxy>,
    source: Option<Ipv6Addr>,
    proxy_tls: Option<Arc<rustls::ClientConfig>>,
}

impl DialConfig {
    pub fn new(
        proxy: Option<OutboundProxy>,
        source: Option<Ipv6Addr>,
    ) -> Result<Self, NativeTransportError> {
        if proxy.is_some() && source.is_some() {
            return Err(NativeTransportError::configuration(
                CodexEgressError::ProxyConflict,
            ));
        }
        Ok(Self {
            proxy,
            source,
            proxy_tls: None,
        })
    }

    pub(crate) fn is_http_proxy(&self) -> bool {
        self.proxy.as_ref().is_some_and(|proxy| {
            proxy.expose_url().starts_with("http:") || proxy.expose_url().starts_with("https:")
        })
    }

    pub(crate) fn with_proxy_tls(
        proxy: Option<OutboundProxy>,
        source: Option<Ipv6Addr>,
        proxy_tls: Arc<rustls::ClientConfig>,
    ) -> Result<Self, NativeTransportError> {
        if proxy.is_some() && source.is_some() {
            return Err(NativeTransportError::configuration(
                CodexEgressError::ProxyConflict,
            ));
        }
        Ok(Self {
            proxy,
            source,
            proxy_tls: Some(proxy_tls),
        })
    }
}

/// Dial a tunnel to an origin. The caller owns the TLS/upgrade deadline.
pub async fn connect(
    host: &str,
    port: u16,
    config: &DialConfig,
) -> Result<MaybeTlsStream<TcpStream>, NativeTransportError> {
    tokio::time::timeout(
        Duration::from_secs(15),
        connect_inner(host, port, config, true),
    )
    .await
    .map_err(|_| NativeTransportError::connect_timeout())?
}

pub(crate) async fn connect_inner(
    host: &str,
    port: u16,
    config: &DialConfig,
    tunnel: bool,
) -> Result<MaybeTlsStream<TcpStream>, NativeTransportError> {
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    let Some(proxy) = &config.proxy else {
        let stream = if let Some(source) = config.source {
            super::egress::connect_source_bound(host, port, source)
                .await
                .map_err(NativeTransportError::connect_error)?
        } else {
            connect_tcp(host, port).await?
        };
        stream
            .set_nodelay(true)
            .map_err(NativeTransportError::connect_error)?;
        return Ok(MaybeTlsStream::Plain(stream));
    };
    let invalid = || NativeTransportError::configuration(io::Error::other("invalid proxy"));
    let mut proxy_url = url::Url::parse(proxy.expose_url()).map_err(|_| invalid())?;
    let proxy_port = proxy_url.port_or_known_default().ok_or_else(invalid)?;
    let tls_proxy = proxy_url.scheme() == "https";
    if tls_proxy {
        proxy_url.set_scheme("http").map_err(|_| invalid())?;
        proxy_url
            .set_port(Some(proxy_port))
            .map_err(|_| invalid())?;
    }
    let proxy_config = ProxyConfig::parse(proxy_url.as_str()).map_err(|_| invalid())?;
    let proxy_host = match proxy_url.host() {
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        _ => proxy_config.host.clone(),
    };
    let tcp = connect_tcp(&proxy_host, proxy_config.port).await?;
    let stream = if tls_proxy {
        let roots = match &config.proxy_tls {
            Some(roots) => roots.clone(),
            None => tokio::task::spawn_blocking(|| {
                super::native_tls::NativeTlsConnector::from_environment()
                    .map(|connector| connector.proxy_tls())
            })
            .await
            .map_err(NativeTransportError::configuration)??,
        };
        let name = rustls_pki_types::ServerName::try_from(proxy_host).map_err(|_| invalid())?;
        MaybeTlsStream::Rustls(
            tokio_rustls::TlsConnector::from(roots)
                .connect(name, tcp)
                .await
                .map_err(NativeTransportError::connect_error)?,
        )
    } else {
        MaybeTlsStream::Plain(tcp)
    };
    if !tunnel && config.is_http_proxy() {
        return Ok(stream);
    }
    let target = if proxy_url.scheme() == "socks5" {
        lookup_host((host, port))
            .await
            .map_err(NativeTransportError::connect_error)?
            .next()
            .ok_or_else(|| {
                NativeTransportError::connect_error(io::Error::other("empty DNS result"))
            })?
            .ip()
            .to_string()
    } else if config.is_http_proxy() && host.parse::<Ipv6Addr>().is_ok() {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    tokio_tungstenite::proxy::connect_via_proxy(stream, &proxy_config, &target, port)
        .await
        .map_err(|_| NativeTransportError::connect_error(io::Error::other("proxy tunnel failed")))
}

async fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, NativeTransportError> {
    let authority = host.parse::<std::net::IpAddr>().map_or_else(
        |_| format!("{host}:{port}"),
        |ip| std::net::SocketAddr::new(ip, port).to_string(),
    );
    let uri = format!("http://{authority}")
        .parse::<hyper::Uri>()
        .map_err(NativeTransportError::configuration)?;
    let mut connector = HttpConnector::new();
    connector.set_nodelay(true);
    connector.set_keepalive(Some(Duration::from_secs(30)));
    connector
        .call(uri)
        .await
        .map(|stream| stream.into_inner())
        .map_err(NativeTransportError::connect_error)
}
