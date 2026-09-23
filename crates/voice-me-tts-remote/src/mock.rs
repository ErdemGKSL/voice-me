//! A loopback HTTP/1.1 server the tests point DeepInfra at — a `tokio`
//! listener on `127.0.0.1`, no mock crate, no real network.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

/// One request as the server saw it.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// What the server answers.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub delay: Duration,
}

impl Reply {
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

type Route = dyn Fn(&Recorded) -> Reply + Send + Sync;

pub struct MockServer {
    pub base_url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl MockServer {
    /// Serve every request with `route`, on its own thread and runtime.
    pub fn start(route: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let route: Arc<Route> = Arc::new(route);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", std_listener.local_addr().unwrap());

        let recorded = requests.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = TcpListener::from_std(std_listener).unwrap();
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        return;
                    };
                    let route = route.clone();
                    let recorded = recorded.clone();
                    tokio::spawn(async move {
                        let Some(request) = read_request(&mut stream).await else {
                            return;
                        };
                        recorded.lock().unwrap().push(request.clone());
                        let reply = route(&request);
                        tokio::time::sleep(reply.delay).await;
                        let response = format!(
                            "HTTP/1.1 {} X\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n{}",
                            reply.status,
                            reply.body.len(),
                            reply.body
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    });
                }
            });
        });

        Self { base_url, requests }
    }

    /// Every request so far, in arrival order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    /// `"POST /v1/voices/add"`-style lines, in arrival order.
    pub fn calls(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|request| format!("{} {}", request.method, request.path))
            .collect()
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<Recorded> {
    let mut buffer = Vec::new();
    let head_end = loop {
        if let Some(end) = find(&buffer, b"\r\n\r\n") {
            break end;
        }
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect();
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };

    let mut body = buffer[head_end + 4..].to_vec();
    if let Some(length) = header("content-length").and_then(|v| v.parse::<usize>().ok()) {
        while body.len() < length {
            let mut chunk = [0_u8; 8192];
            let read = stream.read(&mut chunk).await.ok()?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read]);
        }
    } else if header("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
        while find(&body, b"0\r\n\r\n").is_none() {
            let mut chunk = [0_u8; 8192];
            let read = stream.read(&mut chunk).await.ok()?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read]);
        }
        body = dechunk(&body);
    }

    Some(Recorded {
        method,
        path,
        headers,
        body,
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn dechunk(mut raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(line_end) = find(raw, b"\r\n") {
        let size = usize::from_str_radix(
            String::from_utf8_lossy(&raw[..line_end])
                .split(';')
                .next()
                .unwrap_or("0")
                .trim(),
            16,
        )
        .unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = line_end + 2;
        out.extend_from_slice(&raw[start..(start + size).min(raw.len())]);
        raw = &raw[(start + size + 2).min(raw.len())..];
    }
    out
}
