//! The accept loop: HTTP/1.1 connections, plain or TLS, with the limits a public listener needs.
//!
//! - **Connection limit** (`server.max_connections`): at most that many open connections per
//!   listener; further clients wait in the operating system's backlog, nothing is dropped.
//! - **Header timeout** (`server.header_timeout_ms`): a client must send its request headers,
//!   and with TLS finish the handshake, within it. Slow-header connections cannot pin the limit.
//! - **Peer address**: every request carries `ConnectInfo<SocketAddr>` (client address for the
//!   logs; see `trusted_proxies`).
//! - **Graceful shutdown**: on stop, accepting ends, idle connections close, and each busy one
//!   finishes its current request first; the call returns when every connection has closed.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};
use tower::ServiceExt;
use tracing::{debug, warn};

use crate::metrics::Metrics;
use crate::tls::TlsState;

/// How a listener serves its connections.
#[derive(Clone)]
pub struct ListenOptions {
    /// `api` or `management`, for metrics and logs.
    pub name: &'static str,
    pub max_connections: usize,
    pub header_timeout: Duration,
    pub tls: Option<Arc<TlsState>>,
}

/// A plain or TLS connection.
pub enum Conn {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl AsyncRead for Conn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Conn::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Conn::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_flush(cx),
            Conn::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Conn::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Serve `router` on `listener` until `stop` is true, then shut down gracefully (see the module
/// documentation). Returns when every connection has closed.
pub async fn serve_connections(
    listener: TcpListener,
    router: Router,
    options: ListenOptions,
    metrics: &Metrics,
    mut stop: watch::Receiver<bool>,
) {
    let slots = Arc::new(Semaphore::new(options.max_connections));
    let open = metrics.open_connections.with_label_values(&[options.name]);
    let tls_errors = metrics.tls_handshake_errors.clone();
    loop {
        let permit = tokio::select! {
            permit = slots.clone().acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
            () = stopped(&mut stop) => break,
        };
        let (tcp, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(e) => {
                    // Typically EMFILE / ENFILE: back off instead of spinning.
                    warn!(listener = options.name, error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            () = stopped(&mut stop) => break,
        };
        let _ = tcp.set_nodelay(true);
        let (router, options, tls_errors, open, mut stop) = (
            router.clone(),
            options.clone(),
            tls_errors.clone(),
            open.clone(),
            stop.clone(),
        );
        tokio::spawn(async move {
            let _permit = permit;
            open.inc();
            let io = match &options.tls {
                None => Conn::Plain(tcp),
                Some(tls) => {
                    let handshake = tls.acceptor().accept(tcp);
                    match tokio::time::timeout(options.header_timeout, handshake).await {
                        Ok(Ok(stream)) => Conn::Tls(Box::new(stream)),
                        Ok(Err(e)) => {
                            tls_errors.with_label_values(&["error"]).inc();
                            debug!(peer = %peer, error = %e, "TLS handshake failed");
                            open.dec();
                            return;
                        }
                        Err(_) => {
                            tls_errors.with_label_values(&["timeout"]).inc();
                            debug!(peer = %peer, "TLS handshake timed out");
                            open.dec();
                            return;
                        }
                    }
                }
            };
            let service =
                hyper::service::service_fn(move |mut request: hyper::Request<Incoming>| {
                    request.extensions_mut().insert(ConnectInfo(peer));
                    router.clone().oneshot(request)
                });
            let connection = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(options.header_timeout)
                .keep_alive(true)
                .serve_connection(TokioIo::new(io), service);
            tokio::pin!(connection);
            tokio::select! {
                result = connection.as_mut() => {
                    if let Err(e) = result {
                        debug!(peer = %peer, error = %e, "connection ended with an error");
                    }
                }
                () = stopped(&mut stop) => {
                    connection.as_mut().graceful_shutdown();
                    let _ = connection.await;
                }
            }
            open.dec();
        });
    }
    // Every connection holds a slot: all slots back means all connections closed.
    let all = u32::try_from(options.max_connections).unwrap_or(u32::MAX);
    let _ = slots.acquire_many(all).await;
}

/// The address a probe of `bind` should use: the loopback address for unspecified binds.
pub fn local_probe_addr(bind: SocketAddr) -> SocketAddr {
    let ip = match bind.ip() {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => std::net::Ipv4Addr::LOCALHOST.into(),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => std::net::Ipv6Addr::LOCALHOST.into(),
        ip => ip,
    };
    SocketAddr::new(ip, bind.port())
}

/// Resolves once `stop` is true (or its sender is gone).
async fn stopped(stop: &mut watch::Receiver<bool>) {
    // The read guard `wait_for` returns must not live across an await (it is not `Send`).
    let _ = stop.wait_for(|stop| *stop).await.map(|_| ());
}
