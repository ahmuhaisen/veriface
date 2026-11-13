//! The serve loop over a real socket: warm-up gates readiness, shutdown is graceful, and a failed
//! warm-up stops the service with an error.

mod common;

use std::sync::Arc;
use std::time::Duration;

use face_server::api::Settings;
use face_server::dto::{DocumentPortraitBody, EngineInfo, QualityBody, VerificationBody};
use face_server::engine::{
    DocumentPortraitRequest, Engine, EngineError, QualityRequest, Stages, StubEngine,
    VerificationRequest,
};
use face_server::server;
use tokio::net::TcpListener;

use common::*;

/// Raw HTTP/1.1 GET over a blocking socket; returns the status line.
async fn http_get(addr: std::net::SocketAddr, path: &str) -> String {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response.lines().next().unwrap_or_default().to_owned()
    })
    .await
    .unwrap()
}

/// The stub engine with a slow or failing warm-up.
struct WarmUp {
    stub: StubEngine,
    delay: Duration,
    fail: bool,
}

impl Engine for WarmUp {
    fn info(&self) -> &EngineInfo {
        self.stub.info()
    }

    fn warm_up(&self) -> Result<(), EngineError> {
        std::thread::sleep(self.delay);
        if self.fail {
            Err(EngineError::Unavailable("model checksum mismatch".into()))
        } else {
            Ok(())
        }
    }

    fn assess_quality(
        &self,
        r: &QualityRequest<'_>,
        s: &mut Stages,
    ) -> Result<QualityBody, EngineError> {
        self.stub.assess_quality(r, s)
    }

    fn extract_document_portrait(
        &self,
        r: &DocumentPortraitRequest<'_>,
        s: &mut Stages,
    ) -> Result<DocumentPortraitBody, EngineError> {
        self.stub.extract_document_portrait(r, s)
    }

    fn verify(
        &self,
        r: &VerificationRequest<'_>,
        s: &mut Stages,
    ) -> Result<VerificationBody, EngineError> {
        self.stub.verify(r, s)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_after_warm_up_and_graceful_shutdown() {
    let engine = Arc::new(WarmUp {
        stub: StubEngine::new(),
        delay: Duration::from_millis(300),
        fail: false,
    });
    let (app, _) = test_app(engine, Settings::default());
    app.set_ready(false); // `serve` decides readiness
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(server::serve(
        app.clone(),
        listener,
        async move {
            let _ = stopped.await;
        },
        Duration::from_secs(5),
        None,
    ));

    assert_eq!(http_get(addr, "/health/live").await, "HTTP/1.1 200 OK");
    assert_eq!(
        http_get(addr, "/health/ready").await,
        "HTTP/1.1 503 Service Unavailable",
        "not ready while warming up"
    );
    while !app.is_ready() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(http_get(addr, "/health/ready").await, "HTTP/1.1 200 OK");

    stop.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .expect("stops within the grace period")
        .unwrap();
    assert_eq!(result, Ok(()));
    assert!(!app.is_ready());
    let closed = tokio::task::spawn_blocking(move || {
        std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_err()
    });
    assert!(closed.await.unwrap(), "listener closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_warm_up_stops_the_service_with_an_error() {
    let engine = Arc::new(WarmUp {
        stub: StubEngine::new(),
        delay: Duration::ZERO,
        fail: true,
    });
    let (app, _) = test_app(engine, Settings::default());
    app.set_ready(false);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        server::serve(
            app.clone(),
            listener,
            std::future::pending(),
            Duration::from_secs(1),
            None,
        ),
    )
    .await
    .expect("stops on its own");
    let message = result.unwrap_err();
    assert!(message.contains("warm-up failed"), "{message}");
    assert!(!app.is_ready());
}
