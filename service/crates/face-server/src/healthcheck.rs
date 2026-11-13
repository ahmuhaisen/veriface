//! `veriface healthcheck`: a tiny HTTP client for container `HEALTHCHECK`s and install scripts,
//! so that images without a shell or curl can probe the service. Exit code 0 when the probe
//! answers 200, 1 otherwise.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::config::Config;

/// Environment variable holding an API key to send, for `health_requires_key = true`.
pub const KEY_ENV: &str = "VERIFACE_HEALTHCHECK_KEY";
/// The probe used when only a configuration is given.
pub const READY_PATH: &str = "/health/ready";

/// What to probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// `host[:port]` for the `Host` header.
    pub host: String,
    pub addr: String,
    pub path: String,
}

impl Target {
    /// From an `http://host[:port][/path]` URL (default path `/health/ready`).
    pub fn from_url(url: &str) -> Result<Target, String> {
        let rest = url.strip_prefix("http://").ok_or_else(|| {
            format!("`{url}`: only http:// URLs are supported (probe the plain listener)")
        })?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, READY_PATH),
        };
        if authority.is_empty() || authority.contains('@') {
            return Err(format!("`{url}`: expected http://host[:port][/path]"));
        }
        let has_port = match authority.rfind(']') {
            Some(close) => authority[close..].contains(':'),
            None => authority.contains(':'),
        };
        let addr = if has_port {
            authority.to_owned()
        } else {
            format!("{authority}:80")
        };
        Ok(Target {
            host: authority.to_owned(),
            addr,
            path: path.to_owned(),
        })
    }

    /// `/health/ready` on the configured listener; an unspecified bind address (`0.0.0.0`,
    /// `[::]`) is probed on the loopback address of the same family.
    pub fn from_config(config: &Config) -> Result<Target, String> {
        let bind = config.server.bind_addr()?;
        let ip = match bind.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        let addr = SocketAddr::new(ip, bind.port()).to_string();
        Ok(Target {
            host: addr.clone(),
            addr,
            path: READY_PATH.to_owned(),
        })
    }
}

/// GET the target; `Ok` with the status line when it is 200, `Err` with a reason otherwise.
pub fn probe(target: &Target, api_key: Option<&str>, timeout: Duration) -> Result<String, String> {
    let addrs: Vec<SocketAddr> = target
        .addr
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {}: {e}", target.addr))?
        .collect();
    let mut last = format!("no address for {}", target.addr);
    for addr in addrs {
        match probe_addr(addr, target, api_key, timeout) {
            Ok(line) => return Ok(line),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn probe_addr(
    addr: SocketAddr,
    target: &Target,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("cannot connect to {addr}: {e}"))?;
    let io = |e: std::io::Error| format!("{addr}: {e}");
    stream.set_read_timeout(Some(timeout)).map_err(io)?;
    stream.set_write_timeout(Some(timeout)).map_err(io)?;
    let mut request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: veriface-healthcheck\r\nConnection: close\r\n",
        target.path, target.host
    );
    if let Some(key) = api_key {
        request.push_str(&format!("X-API-Key: {key}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).map_err(io)?;
    // The status line is all we need; read a bounded prefix of the response.
    let mut head = Vec::new();
    stream.take(4096).read_to_end(&mut head).map_err(io)?;
    let text = String::from_utf8_lossy(&head);
    let line = text.lines().next().unwrap_or_default().trim().to_owned();
    match line.split_whitespace().nth(1) {
        Some("200") => Ok(line),
        Some(_) => Err(format!("{addr}{}: {line}", target.path)),
        None => Err(format!("{addr}{}: not an HTTP response", target.path)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn urls() {
        let t = Target::from_url("http://127.0.0.1:18080/health/live").unwrap();
        assert_eq!(t.addr, "127.0.0.1:18080");
        assert_eq!(t.host, "127.0.0.1:18080");
        assert_eq!(t.path, "/health/live");
        let t = Target::from_url("http://localhost").unwrap();
        assert_eq!(t.addr, "localhost:80");
        assert_eq!(t.path, READY_PATH);
        let t = Target::from_url("http://[::1]:9000").unwrap();
        assert_eq!(t.addr, "[::1]:9000");
        assert_eq!(Target::from_url("http://[::1]").unwrap().addr, "[::1]:80");
        assert!(Target::from_url("https://x").is_err());
        assert!(Target::from_url("http:///x").is_err());
        assert!(Target::from_url("http://u@x").is_err());
    }

    #[test]
    fn config_targets_use_loopback_for_unspecified_binds() {
        let config = |bind: &str| {
            let text = format!(
                "[server]\nbind = \"{bind}\"\n[paths]\nmodels = \"m\"\nprofiles = \"p\"\n\
                 [[api_keys]]\nproject = \"p\"\nprofiles = [\"*\"]\n\
                 sha256 = \"2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824\"\n"
            );
            Config::from_toml(&text, Path::new(".")).unwrap()
        };
        assert_eq!(
            Target::from_config(&config("0.0.0.0:8080")).unwrap().addr,
            "127.0.0.1:8080"
        );
        assert_eq!(
            Target::from_config(&config("[::]:8080")).unwrap().addr,
            "[::1]:8080"
        );
        assert_eq!(
            Target::from_config(&config("10.1.2.3:81")).unwrap().addr,
            "10.1.2.3:81"
        );
    }

    #[test]
    fn probes_a_listener() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for status in ["200 OK", "503 Service Unavailable"] {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).unwrap();
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                write!(s, "HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").unwrap();
                assert!(
                    request.starts_with("GET /health/ready HTTP/1.1\r\n"),
                    "{request}"
                );
                assert!(request.contains("X-API-Key: k\r\n"), "{request}");
            }
        });
        let target = Target::from_url(&format!("http://{addr}")).unwrap();
        let timeout = Duration::from_secs(5);
        assert!(probe(&target, Some("k"), timeout).is_ok());
        let e = probe(&target, Some("k"), timeout).unwrap_err();
        assert!(e.contains("503"), "{e}");
        server.join().unwrap();
        // Nothing listening any more.
        assert!(probe(&target, None, Duration::from_millis(500)).is_err());
    }
}
