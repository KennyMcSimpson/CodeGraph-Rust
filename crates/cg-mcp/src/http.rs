//! HTTP transport (design doc 15.2 D4): MCP "streamable HTTP" in its simplest form. A `POST` to
//! the endpoint carries a JSON-RPC message (or a batch) and is answered with one JSON body;
//! notifications get `202 Accepted`. The server binds to loopback by default, rejects browser
//! origins that are not loopback (DNS-rebinding protection) and can require a bearer token.

use crate::server::{Reply, Server};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const MAX_BODY: usize = 4 << 20;
const MAX_CONNECTIONS: usize = 64;

#[derive(Clone, Default)]
pub struct HttpConfig {
    pub addr: String,
    pub token: Option<String>,
    /// Path of the endpoint, default `/mcp`.
    pub endpoint: String,
}

struct ChanReply(Mutex<mpsc::Sender<Value>>);

impl Reply for ChanReply {
    fn send(&self, msg: Value) {
        let _ = self.0.lock().unwrap_or_else(|e| e.into_inner()).send(msg);
    }
}

fn is_loopback_origin(origin: &str) -> bool {
    let host = origin.split("://").nth(1).unwrap_or(origin);
    let host = host.split('/').next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { host });
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

fn respond(stream: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n", body.len());
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn handle_conn(server: Arc<Server>, cfg: HttpConfig, stream: TcpStream, session: String) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
    let mut w = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let mut r = BufReader::new(stream);
    loop {
        let mut request_line = String::new();
        match r.read_line(&mut request_line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let mut parts = request_line.split_whitespace();
        let (method, path) = (parts.next().unwrap_or("").to_owned(), parts.next().unwrap_or("").to_owned());
        let mut headers: Vec<(String, String)> = Vec::new();
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
            }
        }
        let header = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let len: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        if len > MAX_BODY {
            let _ = respond(&mut w, "413 Payload Too Large", &[], b"");
            return;
        }
        let mut body = vec![0u8; len];
        if len > 0 && r.read_exact(&mut body).is_err() {
            return;
        }
        let path_only = path.split('?').next().unwrap_or("");
        if let Some(o) = header("origin") {
            if !is_loopback_origin(o) {
                let _ = respond(&mut w, "403 Forbidden", &[], b"origin not allowed");
                continue;
            }
        }
        if let Some(t) = &cfg.token {
            let ok = header("authorization").is_some_and(|a| a.strip_prefix("Bearer ").is_some_and(|x| x == t));
            if !ok {
                let _ = respond(&mut w, "401 Unauthorized", &[("WWW-Authenticate", "Bearer".into())], b"");
                continue;
            }
        }
        if path_only != cfg.endpoint {
            let _ = respond(&mut w, "404 Not Found", &[], b"");
            continue;
        }
        match method.as_str() {
            "POST" => {
                let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
                    let e = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}});
                    let _ = respond(&mut w, "400 Bad Request", &[("Content-Type", "application/json".into())], e.to_string().as_bytes());
                    continue;
                };
                let items: Vec<Value> = match msg {
                    Value::Array(a) => a,
                    m => vec![m],
                };
                let wants_response = items.iter().any(|m| m.get("method").is_some() && m.get("id").is_some());
                let (tx, rx) = mpsc::channel::<Value>();
                let reply: Arc<dyn Reply> = Arc::new(ChanReply(Mutex::new(tx)));
                let expected = items.iter().filter(|m| m.get("method").is_some() && m.get("id").is_some()).count();
                let is_init = items.iter().any(|m| m.get("method").and_then(|v| v.as_str()) == Some("initialize"));
                for m in items {
                    server.handle(m, reply.clone());
                }
                drop(reply);
                if !wants_response {
                    let _ = respond(&mut w, "202 Accepted", &[], b"");
                    continue;
                }
                let mut out: Vec<Value> = Vec::new();
                while out.len() < expected {
                    match rx.recv_timeout(Duration::from_secs(40)) {
                        Ok(v) => out.push(v),
                        Err(_) => break,
                    }
                }
                let body = if out.len() == 1 { out.remove(0) } else { Value::Array(out) };
                let mut hdrs: Vec<(&str, String)> = vec![("Content-Type", "application/json".into())];
                if is_init {
                    hdrs.push(("Mcp-Session-Id", session.clone()));
                }
                let _ = respond(&mut w, "200 OK", &hdrs, body.to_string().as_bytes());
            }
            "DELETE" => {
                let _ = respond(&mut w, "200 OK", &[], b"");
            }
            "OPTIONS" => {
                let _ = respond(&mut w, "204 No Content", &[("Allow", "POST, DELETE, OPTIONS".into())], b"");
            }
            _ => {
                let _ = respond(&mut w, "405 Method Not Allowed", &[("Allow", "POST, DELETE, OPTIONS".into())], b"");
            }
        }
    }
}

/// Serve until the process is stopped. Returns the bound address through `bound` so callers
/// (and tests) can use port 0.
pub fn serve_http(server: Arc<Server>, mut cfg: HttpConfig, bound: Option<mpsc::Sender<std::net::SocketAddr>>) -> std::io::Result<()> {
    if cfg.endpoint.is_empty() {
        cfg.endpoint = "/mcp".into();
    }
    let listener = TcpListener::bind(&cfg.addr)?;
    if let Some(b) = bound {
        let _ = b.send(listener.local_addr()?);
    }
    let active = Arc::new(AtomicUsize::new(0));
    let session = format!("{:x}{:x}", std::process::id(), server.started.elapsed().as_nanos());
    for conn in listener.incoming() {
        if server.is_shutdown() {
            break;
        }
        let Ok(stream) = conn else { continue };
        if active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            let mut s = stream;
            let _ = respond(&mut s, "503 Service Unavailable", &[], b"");
            continue;
        }
        active.fetch_add(1, Ordering::SeqCst);
        let (srv, c, a, sess) = (server.clone(), cfg.clone(), active.clone(), session.clone());
        std::thread::spawn(move || {
            handle_conn(srv, c, stream, sess);
            a.fetch_sub(1, Ordering::SeqCst);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert!(is_loopback_origin("http://localhost:3000"));
        assert!(is_loopback_origin("http://127.0.0.1"));
        assert!(!is_loopback_origin("https://evil.example"));
        assert!(!is_loopback_origin("http://localhost.evil.example"));
    }
}
