//! In-process TLS with rustls (ring provider; no OpenSSL): TLS 1.2 and 1.3, optional mutual TLS
//! (client certificates signed by `client_ca`), HTTP/1.1 over ALPN.
//!
//! The certificate, key and client CA are re-read on every hot reload, so a renewed certificate
//! takes effect without a restart; connections already open keep the one they negotiated.

use std::sync::{Arc, RwLock};

use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::server::WebPkiClientVerifier;
use tokio_rustls::rustls::{self, RootCertStore, ServerConfig, SupportedProtocolVersion};

use crate::config::{ClientAuth, TlsConfig};

/// The current TLS configuration of the listener, replaceable at runtime.
pub struct TlsState {
    current: RwLock<Arc<ServerConfig>>,
}

impl TlsState {
    pub fn load(config: &TlsConfig) -> Result<TlsState, String> {
        Ok(TlsState {
            current: RwLock::new(server_config(config)?),
        })
    }

    /// An acceptor for the next connection.
    pub fn acceptor(&self) -> TlsAcceptor {
        TlsAcceptor::from(
            self.current
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
    }

    /// Use `config` for new connections (built with [`server_config`]).
    pub fn replace(&self, config: Arc<ServerConfig>) {
        *self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = config;
    }
}

/// Build the rustls server configuration from `[server.tls]`.
pub fn server_config(config: &TlsConfig) -> Result<Arc<ServerConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let versions: &[&'static SupportedProtocolVersion] = match config.min_version.as_str() {
        "1.3" => &[&rustls::version::TLS13],
        _ => &[&rustls::version::TLS12, &rustls::version::TLS13],
    };
    let certs = read_certs(&config.cert, "server.tls.cert")?;
    let key = PrivateKeyDer::from_pem_file(&config.key).map_err(|e| {
        format!(
            "server.tls.key `{}`: no usable private key ({e})",
            config.key.display()
        )
    })?;
    let builder = ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(versions)
        .map_err(|e| format!("server.tls: {e}"))?;
    let builder = match &config.client_ca {
        Some(path) => {
            let mut roots = RootCertStore::empty();
            for cert in read_certs(path, "server.tls.client_ca")? {
                roots
                    .add(cert)
                    .map_err(|e| format!("server.tls.client_ca `{}`: {e}", path.display()))?;
            }
            let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider);
            let verifier = match config.client_auth {
                ClientAuth::Required => verifier,
                ClientAuth::Optional => verifier.allow_unauthenticated(),
            };
            builder.with_client_cert_verifier(
                verifier
                    .build()
                    .map_err(|e| format!("server.tls.client_ca `{}`: {e}", path.display()))?,
            )
        }
        None => builder.with_no_client_auth(),
    };
    let mut server = builder.with_single_cert(certs, key).map_err(|e| {
        format!(
            "server.tls: the certificate `{}` and key `{}` do not form a usable pair: {e}",
            config.cert.display(),
            config.key.display()
        )
    })?;
    server.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(server))
}

fn read_certs(path: &std::path::Path, key: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    let at = || format!("{key} `{}`", path.display());
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|e| format!("{}: {e}", at()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{}: {e}", at()))?;
    if certs.is_empty() {
        return Err(format!("{}: no PEM certificate found", at()));
    }
    Ok(certs)
}
