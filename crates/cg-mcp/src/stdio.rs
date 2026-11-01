//! stdio transport: newline-delimited JSON-RPC. Nothing but JSON-RPC messages is ever written
//! to stdout (design doc 10.5).

use crate::server::{Reply, Server};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct StdoutReply {
    out: Mutex<std::io::Stdout>,
}

impl Reply for StdoutReply {
    fn send(&self, msg: Value) {
        let mut line = serde_json::to_vec(&msg).unwrap_or_default();
        line.push(b'\n');
        let mut o = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = o.write_all(&line);
        let _ = o.flush();
    }
}

/// Exit when the parent process goes away (the agent crashed): its pid changes to 1/subreaper.
fn watch_parent(server: Arc<Server>) {
    #[cfg(unix)]
    {
        let original = unsafe { libc::getppid() };
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(200));
            if server.is_shutdown() {
                break;
            }
            if unsafe { libc::getppid() } != original {
                server.shutdown();
                std::process::exit(0);
            }
        });
    }
    #[cfg(not(unix))]
    {
        let _ = server;
        let _ = Duration::from_secs(0);
    }
}

/// Serve MCP over stdin/stdout until stdin closes.
pub fn serve_stdio(server: Arc<Server>) -> std::io::Result<()> {
    let reply: Arc<dyn Reply> = Arc::new(StdoutReply { out: Mutex::new(std::io::stdout()) });
    server.set_outbound(reply.clone());
    watch_parent(server.clone());
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                for m in batch {
                    server.handle(m, reply.clone());
                }
            }
            Ok(msg) => server.handle(msg, reply.clone()),
            Err(e) => reply.send(serde_json::json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("Parse error: {e}")}})),
        }
    }
    // Give the final small request a chance to answer, then cancel outstanding work and
    // stop all watchers against a shared deadline. EOF must not wait for a 30 s query.
    server.drain(Duration::from_millis(200));
    server.shutdown();
    Ok(())
}
