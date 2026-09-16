//! Opt-in, approximate QX TLS using public OpenSSL APIs.

use std::{
    collections::VecDeque,
    error::Error,
    fmt, io,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
};

use openssl::{
    ssl::{
        ExtensionContext, SslConnector, SslMethod, SslOptions, SslSessionCacheMode, SslVerifyMode,
        SslVersion, StatusType,
    },
    x509::{X509, store::X509StoreBuilder},
};
use rustls_pki_types::CertificateDer;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_openssl::SslStream;

use super::egress::CodexEgressError;

type BoxError = Box<dyn Error + Send + Sync>;

/// Display and Debug deliberately exclude endpoint, proxy and source details.
#[derive(thiserror::Error)]
pub enum NativeTransportError {
    #[error("native transport configuration failed")]
    Configuration {
        #[source]
        source: BoxError,
    },
    #[error("native transport connection failed before sending the request")]
    Connect {
        #[source]
        source: BoxError,
        timeout: bool,
    },
    #[error("native transport request failed; delivery may have occurred")]
    Request {
        #[source]
        source: BoxError,
        timeout: bool,
    },
}

impl fmt::Debug for NativeTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Configuration { .. } => "Configuration",
            Self::Connect { .. } => "Connect",
            Self::Request { .. } => "Request",
        };
        f.debug_struct(kind)
            .field("timeout", &self.is_timeout())
            .finish_non_exhaustive()
    }
}

impl NativeTransportError {
    pub fn is_connect(&self) -> bool {
        matches!(self, Self::Connect { .. })
    }

    pub fn is_configuration(&self) -> bool {
        matches!(self, Self::Configuration { .. })
    }

    pub fn is_timeout(&self) -> bool {
        match self {
            Self::Configuration { .. } => false,
            Self::Connect { timeout, .. } | Self::Request { timeout, .. } => *timeout,
        }
    }

    pub fn egress_error(&self) -> Option<CodexEgressError> {
        let mut source = self.source();
        while let Some(error) = source {
            if let Some(error) = error.downcast_ref::<CodexEgressError>() {
                return Some(*error);
            }
            if let Some(error) = error.downcast_ref::<Self>() {
                return error.egress_error();
            }
            source = error.source();
        }
        None
    }

    pub(crate) fn configuration(error: impl Into<BoxError>) -> Self {
        Self::Configuration {
            source: error.into(),
        }
    }

    pub(crate) fn connect_error(error: impl Into<BoxError>) -> Self {
        Self::Connect {
            source: error.into(),
            timeout: false,
        }
    }

    pub(crate) fn request_error(error: impl Into<BoxError>) -> Self {
        Self::Request {
            source: error.into(),
            timeout: false,
        }
    }

    pub(crate) fn connect_timeout() -> Self {
        Self::Connect {
            source: io::Error::new(io::ErrorKind::TimedOut, "connection deadline elapsed").into(),
            timeout: true,
        }
    }

    pub(crate) fn request_timeout() -> Self {
        Self::Request {
            source: io::Error::new(io::ErrorKind::TimedOut, "request deadline elapsed").into(),
            timeout: true,
        }
    }
}

const GROUPS: &str =
    "*X25519MLKEM768:SecP256r1MLKEM768:SecP384r1MLKEM1024:*X25519:P-256:P-384:P-521";
const CIPHERS12: &str = "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:ECDHE-ECDSA-AES128-SHA:ECDHE-RSA-AES128-SHA:ECDHE-ECDSA-AES256-SHA:ECDHE-RSA-AES256-SHA";
const CIPHERS13: &str =
    "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256";
const SIGNATURES: &str = "mldsa44:mldsa65:mldsa87:rsa_pss_rsae_sha256:ecdsa_secp256r1_sha256:ed25519:rsa_pss_rsae_sha384:rsa_pss_rsae_sha512:rsa_pkcs1_sha256:rsa_pkcs1_sha384:rsa_pkcs1_sha512:ecdsa_secp384r1_sha384:ecdsa_secp521r1_sha512";
const CERT_SIGNATURES: &[u16] = &[
    2308, 2309, 2310, 2052, 1027, 2055, 2053, 2054, 1025, 1281, 1537, 1283, 1539, 513, 515,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NativeAlpn {
    #[default]
    Http,
    Http1,
    WebSocket,
}

/// A frozen trust store and TLS profile; reusable across asynchronous handshakes.
#[derive(Clone)]
pub struct NativeTlsConnector {
    connector: Arc<SslConnector>,
    proxy_tls: Arc<rustls::ClientConfig>,
}

impl NativeTlsConnector {
    pub fn from_environment() -> Result<Self, NativeTransportError> {
        type Cache = Mutex<VecDeque<(Option<String>, NativeTlsConnector)>>;
        static CACHE: OnceLock<Cache> = OnceLock::new();
        let key = super::tls::custom_ca_env_cache_key();
        let cache = CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
        {
            let entries = cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((_, connector)) = entries.iter().find(|(entry, _)| entry == &key) {
                return Ok(connector.clone());
            }
        }
        let connector = Self::load_environment()?;
        let mut entries = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, connector)) = entries.iter().find(|(entry, _)| entry == &key) {
            return Ok(connector.clone());
        }
        while entries.len() >= 16 {
            entries.pop_front();
        }
        entries.push_back((key, connector.clone()));
        Ok(connector)
    }

    fn load_environment() -> Result<Self, NativeTransportError> {
        let rustls_native_certs::CertificateResult {
            mut certs, errors, ..
        } = rustls_native_certs::load_native_certs();
        if !errors.is_empty() || certs.is_empty() {
            return Err(NativeTransportError::configuration(io::Error::other(
                "system trust store could not be loaded",
            )));
        }
        certs.extend(
            super::tls::configured_ca_certificates()
                .map_err(NativeTransportError::configuration)?,
        );
        Self::from_certificates(&certs)
    }

    /// Explicit trust anchors, also used by isolated local transport tests.
    pub fn from_certificates(
        certificates: &[CertificateDer<'_>],
    ) -> Result<Self, NativeTransportError> {
        if certificates.is_empty() {
            return Err(NativeTransportError::configuration(io::Error::other(
                "empty trust store",
            )));
        }
        super::tls::ensure_rustls_provider();
        let mut roots = rustls::RootCertStore::empty();
        for certificate in certificates {
            roots
                .add(certificate.clone().into_owned())
                .map_err(NativeTransportError::configuration)?;
        }
        let proxy_tls = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let build = || -> Result<SslConnector, openssl::error::ErrorStack> {
            let mut store = X509StoreBuilder::new()?;
            for certificate in certificates {
                store.add_cert(X509::from_der(certificate.as_ref())?)?;
            }
            let mut cfg = SslConnector::builder(SslMethod::tls_client())?;
            // Replace OpenSSL's implicit filesystem roots with the project's roots.
            cfg.set_cert_store(store.build());
            cfg.set_verify(SslVerifyMode::PEER);
            cfg.set_min_proto_version(Some(SslVersion::TLS1_2))?;
            cfg.set_max_proto_version(Some(SslVersion::TLS1_3))?;
            cfg.set_cipher_list(CIPHERS12)?;
            cfg.set_ciphersuites(CIPHERS13)?;
            cfg.set_groups_list(GROUPS)?;
            cfg.set_sigalgs_list(SIGNATURES)?;
            cfg.set_session_cache_mode(SslSessionCacheMode::OFF);
            // Public OpenSSL option bits not named by openssl 0.10.81.
            let no_etm_or_cert_compression = SslOptions::from_bits_retain((1 << 19) | (1 << 33));
            cfg.set_options(SslOptions::NO_TICKET | no_etm_or_cert_compression);
            cfg.add_custom_ext(
                18,
                ExtensionContext::CLIENT_HELLO,
                |_, _, _| Ok(Some(Vec::<u8>::new())),
                |_, _, _, _| Ok(()),
            )?;
            cfg.add_custom_ext(
                50,
                ExtensionContext::CLIENT_HELLO,
                |_, _, _| {
                    let mut bytes = ((CERT_SIGNATURES.len() * 2) as u16).to_be_bytes().to_vec();
                    bytes.extend(CERT_SIGNATURES.iter().flat_map(|value| value.to_be_bytes()));
                    Ok(Some(bytes))
                },
                |_, _, _, _| Ok(()),
            )?;
            Ok(cfg.build())
        };
        build()
            .map(|connector| Self {
                connector: Arc::new(connector),
                proxy_tls,
            })
            .map_err(NativeTransportError::configuration)
    }

    pub(crate) fn proxy_tls(&self) -> Arc<rustls::ClientConfig> {
        self.proxy_tls.clone()
    }

    pub async fn connect<S>(
        &self,
        stream: S,
        host: &str,
        alpn: NativeAlpn,
    ) -> Result<SslStream<S>, NativeTransportError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let host = host
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(host);
        let mut ssl = self
            .connector
            .configure()
            .and_then(|cfg| cfg.into_ssl(host))
            .map_err(NativeTransportError::configuration)?;
        if alpn == NativeAlpn::Http {
            ssl.set_alpn_protos(b"\x02h2\x08http/1.1")
                .map_err(NativeTransportError::configuration)?;
        }
        ssl.set_status_type(StatusType::OCSP)
            .map_err(NativeTransportError::configuration)?;
        let mut stream =
            SslStream::new(ssl, stream).map_err(NativeTransportError::configuration)?;
        Pin::new(&mut stream)
            .connect()
            .await
            .map_err(NativeTransportError::connect_error)?;
        Ok(stream)
    }
}

/// Convenience WS entry point; callers bound the complete dial/upgrade lifetime.
pub async fn connect<S>(
    stream: S,
    host: &str,
    alpn: NativeAlpn,
) -> Result<SslStream<S>, NativeTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // A cold CA load may touch the filesystem and OS trust service.
    let connector = tokio::task::spawn_blocking(NativeTlsConnector::from_environment)
        .await
        .map_err(NativeTransportError::configuration)??;
    connector.connect(stream, host, alpn).await
}
