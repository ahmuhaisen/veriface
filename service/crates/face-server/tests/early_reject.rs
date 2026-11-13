//! Early rejections over a real socket: a client that sends its whole upload before reading the
//! response (no `Expect: 100-continue`) must still receive the problem response, not a reset.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use face_server::api::{App, Settings};
use face_server::engine::StubEngine;
use face_server::server;
use tokio::net::TcpListener;

use common::*;

/// Serve a stub app on a free port until the test ends.
async fn start() -> (Arc<App>, SocketAddr) {
    let (app, _) = test_app(Arc::new(StubEngine::new()), Settings::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(server::serve(
        app.clone(),
        listener,
        std::future::pending(),
        Duration::from_secs(1),
        None,
    ));
    while !app.is_ready() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (app, addr)
}

struct Head {
    status: String,
    headers: Vec<String>,
    body: String,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        let prefix = format!("{name}:");
        self.headers.iter().find_map(|h| {
            h.to_ascii_lowercase()
                .starts_with(&prefix)
                .then(|| h[prefix.len()..].trim())
        })
    }
}

/// POST `/v1/quality` with a wrong key: the headers, then `send` body bytes of a declared
/// `length` (all of them unless `expect_continue`), then read the response.
fn post_with_wrong_key(
    addr: SocketAddr,
    length: usize,
    send: usize,
    expect_continue: bool,
) -> std::io::Result<Head> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(20)))?;
    let expect = if expect_continue {
        "Expect: 100-continue\r\n"
    } else {
        ""
    };
    write!(
        stream,
        "POST /v1/quality HTTP/1.1\r\nHost: test\r\nX-API-Key: wrong\r\n\
         Content-Type: multipart/form-data; boundary=x\r\nContent-Length: {length}\r\n{expect}\r\n"
    )?;
    let chunk = vec![b'a'; 64 * 1024];
    let mut sent = 0;
    while sent < send {
        let n = chunk.len().min(send - sent);
        stream.write_all(&chunk[..n])?;
        sent += n;
    }
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status)?;
    let mut headers = Vec::new();
    let mut content_length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end().to_owned();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
        headers.push(line);
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    Ok(Head {
        status: status.trim_end().to_owned(),
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_uploads_are_drained_and_answered() {
    let (app, addr) = start().await;
    // 3 MiB: far more than the socket buffers absorb, so without the drain the client's write
    // or read fails with a reset.
    let size = 3 << 20;
    for _ in 0..3 {
        let head = blocking(move || post_with_wrong_key(addr, size, size, false))
            .await
            .expect("the 401 arrives; the upload is not reset");
        assert_eq!(head.status, "HTTP/1.1 401 Unauthorized");
        assert!(head.body.contains("unauthorized"), "{}", head.body);
        assert_eq!(
            head.header("connection"),
            None,
            "the connection stays usable"
        );
    }
    let metrics = app.metrics.render();
    assert!(
        metrics.contains("face_unread_body_total{outcome=\"drained\"} 3"),
        "{metrics}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expect_continue_is_answered_without_reading_the_body() {
    let (app, addr) = start().await;
    let head = blocking(move || post_with_wrong_key(addr, 3 << 20, 0, true))
        .await
        .unwrap();
    assert_eq!(head.status, "HTTP/1.1 401 Unauthorized", "no 100 Continue");
    assert_eq!(head.header("connection"), Some("close"));
    assert!(
        app.metrics
            .render()
            .contains("face_unread_body_total{outcome=\"not_sent\"} 1")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bodies_above_the_limit_are_not_drained() {
    let (app, addr) = start().await;
    let limit = app.body_limit();
    let head = blocking(move || post_with_wrong_key(addr, limit + 1, 64 * 1024, false))
        .await
        .unwrap();
    assert_eq!(head.status, "HTTP/1.1 401 Unauthorized");
    assert_eq!(head.header("connection"), Some("close"));
    assert!(
        app.metrics
            .render()
            .contains("face_unread_body_total{outcome=\"too_large\"} 1")
    );
}
