//! MCP server core (design doc 10): JSON-RPC dispatch, project location and snapshot caching,
//! a blocking pool for tool calls, per-call timeout, success-shaped errors.

use crate::defs;
use cg_core::brand;
use cg_query::explore::History;
use cg_query::strings;
use cg_query::tools::{self, StatusInfo, ToolEnv, ToolOutput};
use cg_query::Project;
use cg_store::Store;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const SUPPORTED_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const POOL_SIZE: usize = 4;
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PROJECTS: usize = 8;

/// Where responses and server-initiated requests go (stdout, or one HTTP response).
pub trait Reply: Send + Sync {
    fn send(&self, msg: Value);
}

#[derive(Clone, Default)]
pub struct Config {
    /// `--path`: default project directory when the client sends no roots.
    pub path: Option<PathBuf>,
    /// Short tool names enabled in addition to / instead of the default surface.
    pub tools: Option<Vec<String>>,
    pub dedup: bool,
}

type StatusHook = Arc<dyn Fn(&Project) -> Option<String> + Send + Sync>;
type RebuildHook = Arc<dyn Fn(&Path) + Send + Sync>;
type OpenHook = Arc<dyn Fn(&Path) + Send + Sync>;

struct Entry {
    project: Arc<Project>,
    history: Arc<Mutex<History>>,
    noticed: Arc<AtomicBool>,
    last_used: u64,
}

#[derive(Default)]
struct Cache {
    map: HashMap<PathBuf, Entry>,
    tick: u64,
}

#[derive(Default)]
struct RootsState {
    /// A `roots/list` request is outstanding.
    pending: bool,
    roots: Vec<PathBuf>,
}

type Job = Box<dyn FnOnce() + Send>;

pub struct Server {
    cfg: Config,
    cache: Mutex<Cache>,
    roots: Mutex<RootsState>,
    roots_cv: Condvar,
    client_roots: AtomicBool,
    protocol: Mutex<String>,
    pool_tx: Mutex<Option<mpsc::Sender<Job>>>,
    outbound: OnceLock<Arc<dyn Reply>>,
    shutdown_flag: AtomicBool,
    shutdown_hooks: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    status_hook: Mutex<Option<StatusHook>>,
    rebuild_hook: Mutex<Option<RebuildHook>>,
    open_hook: Mutex<Option<OpenHook>>,
    next_req: AtomicU64,
    pub started: Instant,
}

fn tool_result(o: ToolOutput) -> Value {
    let mut v = json!({"content": [{"type": "text", "text": o.text}]});
    if o.is_error {
        v["isError"] = json!(true);
    }
    v
}

/// `file:///a/b%20c` -> `/a/b c`
pub fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8(out).ok()?;
    // Windows drive paths come as /C:/...
    let s = if s.len() > 2 && s.as_bytes()[0] == b'/' && s.as_bytes()[2] == b':' { s[1..].to_owned() } else { s };
    Some(PathBuf::from(s))
}

impl Server {
    pub fn new(cfg: Config) -> Arc<Server> {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..POOL_SIZE {
            let rx = Arc::clone(&rx);
            let _ = std::thread::Builder::new().name(format!("cgr-mcp-{i}")).stack_size(8 << 20).spawn(move || loop {
                let job = { rx.lock().unwrap_or_else(|e| e.into_inner()).recv() };
                match job {
                    Ok(j) => j(),
                    Err(_) => break,
                }
            });
        }
        Arc::new(Server {
            cfg,
            cache: Mutex::new(Cache::default()),
            roots: Mutex::new(RootsState::default()),
            roots_cv: Condvar::new(),
            client_roots: AtomicBool::new(false),
            protocol: Mutex::new(SUPPORTED_PROTOCOLS[0].to_owned()),
            pool_tx: Mutex::new(Some(tx)),
            outbound: OnceLock::new(),
            shutdown_flag: AtomicBool::new(false),
            shutdown_hooks: Mutex::new(Vec::new()),
            status_hook: Mutex::new(None),
            rebuild_hook: Mutex::new(None),
            open_hook: Mutex::new(None),
            next_req: AtomicU64::new(1),
            started: Instant::now(),
        })
    }

    pub fn set_outbound(&self, r: Arc<dyn Reply>) {
        let _ = self.outbound.set(r);
    }

    pub fn set_status_hook(&self, h: StatusHook) {
        *self.status_hook.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
    }

    pub fn set_rebuild_hook(&self, h: RebuildHook) {
        *self.rebuild_hook.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
    }

    /// Called with the project root the first time a project is opened (starts its watcher).
    pub fn set_open_hook(&self, h: OpenHook) {
        *self.open_hook.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
    }

    /// Open the project at `start` ahead of the first tool call (so it is watched from the start).
    pub fn preopen(&self, start: &Path) {
        let _ = self.project_for(Some(&start.to_string_lossy()));
    }

    pub fn on_shutdown(&self, f: Box<dyn FnOnce() + Send>) {
        self.shutdown_hooks.lock().unwrap_or_else(|e| e.into_inner()).push(f);
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown_flag.load(Ordering::SeqCst)
    }

    /// Stop accepting work and run shutdown hooks (stop the watcher, release locks).
    pub fn shutdown(&self) {
        if self.shutdown_flag.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.pool_tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let hooks: Vec<_> = self.shutdown_hooks.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
        for h in hooks {
            h();
        }
    }

    fn submit(&self, job: Job) -> bool {
        match self.pool_tx.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            Some(tx) => tx.send(job).is_ok(),
            None => false,
        }
    }

    // ------------------------------------------------------------------ roots
    fn default_root(&self) -> PathBuf {
        // Wait briefly for an outstanding roots/list answer so the first call sees the roots.
        {
            let mut r = self.roots.lock().unwrap_or_else(|e| e.into_inner());
            let deadline = Instant::now() + Duration::from_millis(1500);
            while r.pending {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                let (g, _) = self.roots_cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner());
                r = g;
            }
            if let Some(first) = r.roots.first() {
                return first.clone();
            }
        }
        self.cfg.path.clone().or_else(|| std::env::current_dir().ok()).unwrap_or_else(|| PathBuf::from("."))
    }

    fn request_roots(&self) {
        if !self.client_roots.load(Ordering::SeqCst) {
            return;
        }
        let Some(out) = self.outbound.get() else { return };
        let id = format!("cgr-roots-{}", self.next_req.fetch_add(1, Ordering::SeqCst));
        self.roots.lock().unwrap_or_else(|e| e.into_inner()).pending = true;
        out.send(json!({"jsonrpc": "2.0", "id": id, "method": "roots/list"}));
    }

    fn on_roots_response(&self, msg: &Value) {
        let mut r = self.roots.lock().unwrap_or_else(|e| e.into_inner());
        r.pending = false;
        if let Some(list) = msg.pointer("/result/roots").and_then(|v| v.as_array()) {
            r.roots = list.iter().filter_map(|x| x.get("uri").and_then(|u| u.as_str())).filter_map(file_uri_to_path).collect();
        }
        self.roots_cv.notify_all();
    }

    // --------------------------------------------------------------- projects
    fn guidance(&self, start: &Path, explicit: bool) -> ToolOutput {
        let init = format!("{} init", brand::BIN_NAME);
        let text = if explicit {
            format!(
                "No {dir}/ index found at or above `{p}`. Indexing is the user's decision — if it comes up, they can run `{init}` in that project. Until then use your built-in tools (Read/Grep/Glob) for it. If this is a monorepo, pass `projectPath` for a sub-project that has its own {dir}/ index.",
                dir = brand::DATA_DIR,
                p = start.display()
            )
        } else {
            format!(
                "This project (`{p}`) has no {dir}/ index, so codegraph-rust has nothing to answer from. Indexing is the user's decision — if it comes up, they can run `{init}` in the project; don't run it yourself. Use your built-in tools (Read/Grep/Glob) here. In a monorepo, pass `projectPath` for a sub-project that has its own {dir}/ index.",
                dir = brand::DATA_DIR,
                p = start.display()
            )
        };
        ToolOutput::ok(text)
    }

    /// Open (or refresh) the project for a tool call.
    fn project_for(&self, project_path: Option<&str>) -> Result<(Arc<Project>, Arc<Mutex<History>>, Arc<AtomicBool>), ToolOutput> {
        let default = self.default_root();
        let (start, explicit) = match project_path.map(str::trim).filter(|s| !s.is_empty()) {
            Some(p) => {
                let pb = PathBuf::from(p);
                (if pb.is_absolute() { pb } else { default.join(pb) }, true)
            }
            None => (default, false),
        };
        let Some(store) = Store::locate(&start) else { return Err(self.guidance(&start, explicit)) };
        let root = store.root.clone();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.tick += 1;
        let tick = cache.tick;
        let gen = store.generation();
        if let Some(e) = cache.map.get_mut(&root) {
            if e.project.generation() == gen || gen == 0 {
                e.last_used = tick;
                return Ok((e.project.clone(), e.history.clone(), e.noticed.clone()));
            }
        }
        let first_open = !cache.map.contains_key(&root);
        match Project::open(store) {
            Ok(p) => {
                let p = Arc::new(p);
                let (history, noticed) = match cache.map.get(&root) {
                    Some(old) => (old.history.clone(), old.noticed.clone()),
                    None => (Arc::new(Mutex::new(History::new())), Arc::new(AtomicBool::new(false))),
                };
                cache.map.insert(root.clone(), Entry { project: p.clone(), history: history.clone(), noticed: noticed.clone(), last_used: tick });
                if cache.map.len() > MAX_PROJECTS {
                    if let Some(oldest) = cache.map.iter().min_by_key(|(_, e)| e.last_used).map(|(k, _)| k.clone()) {
                        cache.map.remove(&oldest);
                    }
                }
                drop(cache);
                if first_open {
                    if let Some(h) = self.open_hook.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                        h(&root);
                    }
                }
                Ok((p, history, noticed))
            }
            Err(e) => {
                cache.map.remove(&root);
                drop(cache);
                if let Some(h) = self.rebuild_hook.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                    let r = root.clone();
                    std::thread::spawn(move || h(&r));
                }
                Err(ToolOutput::err(format!("The index could not be opened ({e}). It is being rebuilt in the background; retry in a few seconds. If this persists the user can run `{} index`.", brand::BIN_NAME)))
            }
        }
    }

    fn run_tool(&self, name: &str, args: &Value) -> ToolOutput {
        let short = name.strip_prefix(brand::MCP_TOOL_PREFIX).or_else(|| name.strip_prefix("codegraph_")).unwrap_or(name);
        if !defs::ALL_TOOLS.contains(&short) || !defs::is_enabled(short, &self.cfg.tools) {
            return ToolOutput::err(format!("Unknown tool: {name}"));
        }
        let (project, history, noticed) = match self.project_for(args.get("projectPath").and_then(|v| v.as_str())) {
            Ok(x) => x,
            Err(o) => return o,
        };
        let mut status: StatusInfo = project.status_info();
        if short == "status" {
            if let Some(h) = self.status_hook.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                status.changes = h(&project);
            }
        }
        let env = ToolEnv { graph: &project.graph, history: if self.cfg.dedup { Some(history) } else { None }, status };
        let mut out = tools::call(&env, short, args);
        if !out.is_error && project.snap.manifest.engine_version != brand::ENGINE_VERSION && !noticed.swap(true, Ordering::SeqCst) {
            out.text = format!("> Note: this index was built by an older {} engine. It still answers; `{} index` rebuilds it with the current resolver.\n\n{}", brand::PROJECT_NAME, brand::BIN_NAME, out.text);
        }
        out
    }

    // --------------------------------------------------------------- dispatch
    fn instructions(&self) -> String {
        let root = self.default_root();
        if Store::locate(&root).is_some() {
            strings::server_instructions()
        } else {
            strings::server_instructions_no_root_index()
        }
    }

    /// Handle one JSON-RPC message; responses go to `reply`.
    pub fn handle(self: &Arc<Self>, msg: Value, reply: Arc<dyn Reply>) {
        let method = msg.get("method").and_then(|m| m.as_str()).map(str::to_owned);
        let id = msg.get("id").cloned();
        match (method, id) {
            (Some(m), Some(id)) => self.handle_request(&m, id, msg.get("params").cloned().unwrap_or(Value::Null), reply),
            (Some(m), None) => self.handle_notification(&m),
            (None, Some(id)) => {
                if id.as_str().map_or(false, |s| s.starts_with("cgr-roots-")) {
                    self.on_roots_response(&msg);
                }
            }
            (None, None) => {}
        }
    }

    fn handle_notification(self: &Arc<Self>, method: &str) {
        match method {
            "notifications/initialized" | "notifications/roots/list_changed" => self.request_roots(),
            _ => {}
        }
    }

    fn handle_request(self: &Arc<Self>, method: &str, id: Value, params: Value, reply: Arc<dyn Reply>) {
        let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id.clone(), "result": result});
        match method {
            "initialize" => {
                let want = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
                let proto = if SUPPORTED_PROTOCOLS.contains(&want) { want } else { SUPPORTED_PROTOCOLS[0] };
                *self.protocol.lock().unwrap_or_else(|e| e.into_inner()) = proto.to_owned();
                let has_roots = params.pointer("/capabilities/roots").is_some();
                self.client_roots.store(has_roots, Ordering::SeqCst);
                if has_roots {
                    // roots/list may only be requested after `initialized`; mark pending now so a
                    // fast first tool call waits for it.
                    self.roots.lock().unwrap_or_else(|e| e.into_inner()).pending = true;
                }
                reply.send(ok(json!({
                    "protocolVersion": proto,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": brand::MCP_SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
                    "instructions": self.instructions(),
                })));
            }
            "ping" => reply.send(ok(json!({}))),
            "tools/list" => reply.send(ok(json!({"tools": defs::definitions(&self.cfg.tools)}))),
            "resources/list" => reply.send(ok(json!({"resources": []}))),
            "resources/templates/list" => reply.send(ok(json!({"resourceTemplates": []}))),
            "prompts/list" => reply.send(ok(json!({"prompts": []}))),
            "logging/setLevel" => reply.send(ok(json!({}))),
            "tools/call" => self.tools_call(id, params, reply),
            _ => reply.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("Method not found: {method}")}})),
        }
    }

    fn tools_call(self: &Arc<Self>, id: Value, params: Value, reply: Arc<dyn Reply>) {
        let Some(name) = params.get("name").and_then(|n| n.as_str()).map(str::to_owned) else {
            reply.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "tools/call needs a tool name"}}));
            return;
        };
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let (tx, rx) = mpsc::channel::<Value>();
        let me = Arc::clone(self);
        let submitted = self.submit(Box::new(move || {
            let r = std::panic::catch_unwind(AssertUnwindSafe(|| me.run_tool(&name, &args)));
            let out = match r {
                Ok(o) => o,
                Err(_) => {
                    me.cache.lock().unwrap_or_else(|e| e.into_inner()).map.clear();
                    ToolOutput::err("Internal error while answering (the cached snapshot was dropped). Retry once.")
                }
            };
            let _ = tx.send(tool_result(out));
        }));
        if !submitted {
            reply.send(json!({"jsonrpc": "2.0", "id": id, "result": tool_result(ToolOutput::err("The server is shutting down."))}));
            return;
        }
        // The watchdog answers with a success-shaped note if the call overruns its budget.
        std::thread::spawn(move || {
            let result = match rx.recv_timeout(CALL_TIMEOUT) {
                Ok(v) => v,
                Err(_) => tool_result(ToolOutput::ok(
                    "This call took longer than 30 seconds and was stopped. Narrow the query — name specific symbols or files — and try again.",
                )),
            };
            reply.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
        });
    }
}
