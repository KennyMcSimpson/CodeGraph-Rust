//! Route extraction for web frameworks (design doc 6.7): Express, Flask, FastAPI, Django, Gin,
//! Spring, Axum and Next.js file routes. A route becomes a `route` node (`GET /users/:id`) that
//! `references` its handler functions, so a request path can be followed to the code that
//! serves it. Scanners are pure functions over the source text.

use cg_core::{EdgeKind, Lang, LangFamily, NodeKind, RefKind};
use cg_extract::post::node_digest;
use cg_extract::{ExtraNode, ExtraRef, FileHook, HookCtx, HookOut};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framework {
    Express,
    Flask,
    FastApi,
    Django,
    Gin,
    Spring,
    Axum,
    Next,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Handler {
    pub name: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Route {
    pub method: String,
    pub path: String,
    pub line: u32,
    pub handlers: Vec<Handler>,
}

struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    fn new(src: &str) -> Lines {
        let mut starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Lines { starts }
    }
    /// 1-based line and UTF-16 column of a byte offset.
    fn at(&self, src: &str, off: usize) -> (u32, u32) {
        let line = self.starts.partition_point(|s| *s <= off) - 1;
        let start = self.starts[line];
        let end = src[start..].find('\n').map_or(src.len(), |e| start + e);
        (line as u32 + 1, cg_core::text::col16(&src[start..end], off - start))
    }
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| Regex::new($pat).expect("static regex"))
        }
    };
}

re!(express_re, r#"(?m)\b([A-Za-z_$][\w$]*)\s*\.\s*(get|post|put|delete|patch|options|head|all|use)\s*\(\s*(['"`])(/[^'"`\n]*)['"`]\s*,"#);
re!(
    py_decorator_re,
    r#"(?m)^[ \t]*@[ \t]*([A-Za-z_]\w*)[ \t]*\.[ \t]*(route|get|post|put|delete|patch|head|options|websocket)[ \t]*\(\s*(?:[rbuf]*)(['"])([^'"\n]*)['"]"#
);
re!(py_methods_re, r#"methods\s*=\s*[\[\(]([^\]\)]*)[\]\)]"#);
re!(py_def_re, r"(?m)^[ \t]*(?:async[ \t]+)?def[ \t]+(\w+)");
re!(django_re, r#"(?m)\b(?:path|re_path|url)\s*\(\s*[rbuf]*(['"])([^'"\n]*)['"]\s*,\s*([A-Za-z_][\w.]*)"#);
re!(gin_re, r#"(?m)\b([A-Za-z_]\w*)\s*\.\s*(GET|POST|PUT|DELETE|PATCH|OPTIONS|HEAD|Any)\s*\(\s*"([^"\n]*)"\s*,"#);
re!(spring_re, r#"(?m)^[ \t]*@(Get|Post|Put|Delete|Patch|Request)Mapping\b(?:\s*\(([^)]*)\))?"#);
re!(spring_path_re, r#"(?:(?:value|path)\s*=\s*)?\{?\s*"([^"]*)""#);
re!(spring_http_re, r"RequestMethod\.(\w+)");
re!(
    java_method_re,
    r"(?m)^[ \t]*(?:@\w+(?:\([^)]*\))?\s*)*(?:public|private|protected)?[ \t]*(?:static[ \t]+)?(?:final[ \t]+)?[\w<>\[\],.? ]+?[ \t]+(\w+)[ \t]*\("
);
re!(kotlin_fun_re, r"(?m)^[ \t]*(?:(?:public|private|protected|internal|override|suspend)[ \t]+)*fun[ \t]+(?:<[^>]*>[ \t]*)?(\w+)[ \t]*\(");
re!(spring_class_re, r"(?m)^[ \t]*(?:public[ \t]+|open[ \t]+|abstract[ \t]+)*(?:class|interface)[ \t]+\w+");
re!(axum_route_re, r#"\.route\(\s*"([^"\n]*)"\s*,\s*"#);
re!(axum_method_re, r"(get|post|put|delete|patch|head|options|any)\s*\(\s*([\w:]+)\s*\)");
re!(ident_path_re, r"^[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*$");
re!(next_default_re, r"(?m)export\s+default\s+(?:async\s+)?(?:function\s+)?([A-Za-z_$][\w$]*)");
re!(next_method_re, r"(?m)export\s+(?:async\s+)?(?:function|const)\s+(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS)\b");

/// Argument list of the call whose `(` is at/after `from`: top-level arguments as
/// `(offset, text)`, plus the byte offset after the closing parenthesis. Strings and comments
/// are skipped when matching parentheses.
fn call_args(src: &str, open: usize) -> Option<(Vec<(usize, &str)>, usize)> {
    let b = src.as_bytes();
    if b.get(open) != Some(&b'(') {
        return None;
    }
    let (mut depth, mut i) = (0i32, open);
    let mut args: Vec<(usize, &str)> = Vec::new();
    let mut arg_start = open + 1;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' | b'`' => {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    args.push((arg_start, &src[arg_start..i]));
                    return Some((args, i + 1));
                }
            }
            b',' if depth == 1 => {
                args.push((arg_start, &src[arg_start..i]));
                arg_start = i + 1;
            }
            _ => {}
        }
        i += 1;
        if i - open > 4000 {
            return None; // not a plain route registration
        }
    }
    None
}

/// Handler names among call arguments: bare identifiers (`list`, `ctrl.list`) and the same
/// inside an array of middleware; function literals have no name.
fn handlers_in(src: &str, lines: &Lines, args: &[(usize, &str)]) -> Vec<Handler> {
    let mut out = Vec::new();
    for (off, text) in args {
        let t = text.trim();
        let inner: Vec<(usize, &str)> = if t.starts_with('[') && t.ends_with(']') {
            let base = off + text.find('[').unwrap_or(0) + 1;
            t[1..t.len() - 1]
                .split(',')
                .scan(base, |pos, s| {
                    let o = *pos;
                    *pos += s.len() + 1;
                    Some((o, s))
                })
                .collect()
        } else {
            vec![(*off, *text)]
        };
        for (o, s) in inner {
            let name = s.trim();
            if ident_path_re().is_match(name) && !matches!(name, "true" | "false" | "null" | "undefined" | "nil" | "this") {
                let lead = s.len() - s.trim_start().len();
                let (line, col) = lines.at(src, o + lead);
                out.push(Handler { name: name.to_owned(), line, col });
            }
        }
    }
    out
}

pub fn scan_express(src: &str) -> Vec<Route> {
    let lines = Lines::new(src);
    let mut out = Vec::new();
    for m in express_re().captures_iter(src) {
        let recv = &m[1];
        let looks_like_router = matches!(recv, "app" | "router" | "server" | "api" | "route" | "routes" | "r")
            || recv.ends_with("Router")
            || recv.ends_with("App")
            || recv.ends_with("Api")
            || recv.ends_with("Routes");
        if !looks_like_router {
            continue;
        }
        let method = m[2].to_ascii_uppercase();
        let path = m[4].to_owned();
        let whole = m.get(0).unwrap();
        let open = src[whole.start()..].find('(').map(|i| whole.start() + i).unwrap_or(whole.start());
        let (line, _) = lines.at(src, whole.start());
        let Some((args, _)) = call_args(src, open) else { continue };
        let handlers = handlers_in(src, &lines, &args[1..]);
        out.push(Route { method, path, line, handlers });
    }
    out
}

/// Blank out comments and triple-quoted strings (docstrings are full of `@app.route` examples),
/// keeping every offset and newline.
pub fn strip_python_noise(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for k in from..to.min(out.len()) {
            if out[k] != b'\n' {
                out[k] = b' ';
            }
        }
    };
    while i < b.len() {
        match b[i] {
            b'#' => {
                let end = src[i..].find('\n').map_or(b.len(), |e| i + e);
                blank(&mut out, i, end);
                i = end;
            }
            b'"' | b'\'' => {
                let q = b[i];
                if b.get(i + 1) == Some(&q) && b.get(i + 2) == Some(&q) {
                    let close = [q, q, q];
                    let end = src[i + 3..].find(std::str::from_utf8(&close).unwrap_or("\"\"\"")).map_or(b.len(), |e| i + 3 + e + 3);
                    blank(&mut out, i, end);
                    i = end;
                } else {
                    // ordinary string: skip it unchanged
                    i += 1;
                    while i < b.len() && b[i] != q && b[i] != b'\n' {
                        if b[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| src.to_owned())
}

pub fn scan_python_decorators(src: &str) -> Vec<Route> {
    let clean = strip_python_noise(src);
    let src = clean.as_str();
    let lines = Lines::new(src);
    let mut out = Vec::new();
    for m in py_decorator_re().captures_iter(src) {
        let whole = m.get(0).unwrap();
        let kind = &m[2];
        let path = m[4].to_owned();
        // the decorator call may span lines: look for `methods=[...]` up to the closing paren
        let open = src[whole.start()..].find('(').map(|i| whole.start() + i).unwrap_or(whole.start());
        let call_end = call_args(src, open).map(|x| x.1).unwrap_or(whole.end());
        let call_text = &src[open..call_end];
        let methods: Vec<String> = if kind == "route" {
            match py_methods_re().captures(call_text) {
                Some(mm) => mm[1].split(',').map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_uppercase()).filter(|s| !s.is_empty()).collect(),
                None => vec!["GET".to_owned()],
            }
        } else if kind == "websocket" {
            vec!["WS".to_owned()]
        } else {
            vec![kind.to_ascii_uppercase()]
        };
        // the decorated function: the first `def` after the decorator
        let Some(d) = py_def_re().captures(&src[call_end..]) else { continue };
        let dm = d.get(1).unwrap();
        let (hline, hcol) = lines.at(src, call_end + dm.start());
        let (line, _) = lines.at(src, whole.start());
        for method in methods {
            out.push(Route { method, path: path.clone(), line, handlers: vec![Handler { name: dm.as_str().to_owned(), line: hline, col: hcol }] });
        }
    }
    out
}

pub fn scan_django(src: &str) -> Vec<Route> {
    let clean = strip_python_noise(src);
    let src = clean.as_str();
    let lines = Lines::new(src);
    django_re()
        .captures_iter(src)
        .filter(|m| !matches!(&m[3], "include" | "admin") && !m[3].ends_with(".urls") && !m[3].starts_with("admin."))
        .map(|m| {
            let whole = m.get(0).unwrap();
            let (line, _) = lines.at(src, whole.start());
            let h = m.get(3).unwrap();
            let (hl, hc) = lines.at(src, h.start());
            let mut name = h.as_str().to_owned();
            name = name.trim_end_matches(".as_view").to_owned();
            let path = if m[2].starts_with('/') { m[2].to_owned() } else { format!("/{}", &m[2]) };
            Route { method: "ANY".into(), path, line, handlers: vec![Handler { name, line: hl, col: hc }] }
        })
        .collect()
}

pub fn scan_gin(src: &str) -> Vec<Route> {
    let lines = Lines::new(src);
    let mut out = Vec::new();
    for m in gin_re().captures_iter(src) {
        let whole = m.get(0).unwrap();
        let open = src[whole.start()..].find('(').map(|i| whole.start() + i).unwrap_or(whole.start());
        let Some((args, _)) = call_args(src, open) else { continue };
        let handlers = handlers_in(src, &lines, &args[1..]);
        let (line, _) = lines.at(src, whole.start());
        out.push(Route { method: m[2].to_ascii_uppercase(), path: m[3].to_owned(), line, handlers });
    }
    out
}

pub fn scan_spring(src: &str, kotlin: bool) -> Vec<Route> {
    let lines = Lines::new(src);
    // class-level @RequestMapping("/prefix") before the first class declaration
    let class_at = spring_class_re().find(src).map_or(usize::MAX, |m| m.start());
    let mut prefix = String::new();
    let mut out = Vec::new();
    for m in spring_re().captures_iter(src) {
        let whole = m.get(0).unwrap();
        let args = m.get(2).map_or("", |a| a.as_str());
        let path = spring_path_re().captures(args).map(|p| p[1].to_owned()).unwrap_or_default();
        if &m[1] == "Request" && whole.start() < class_at {
            prefix = path;
            continue;
        }
        let method = match &m[1] {
            "Request" => spring_http_re().captures(args).map(|h| h[1].to_ascii_uppercase()).unwrap_or_else(|| "ANY".into()),
            other => other.to_ascii_uppercase(),
        };
        // the handler method that follows (other annotations in between are skipped)
        let after = whole.end();
        let tail = &src[after..];
        let hm = if kotlin { kotlin_fun_re().captures(tail).and_then(|c| c.get(1)) } else { java_method_re().captures(tail).and_then(|c| c.get(1)) };
        let Some(hm) = hm else { continue };
        let (hl, hc) = lines.at(src, after + hm.start());
        let (line, _) = lines.at(src, whole.start());
        let full = format!("{}{}{}", prefix, if path.starts_with('/') || path.is_empty() || prefix.ends_with('/') { "" } else { "/" }, path);
        out.push(Route {
            method,
            path: if full.is_empty() { "/".into() } else { full },
            line,
            handlers: vec![Handler { name: hm.as_str().to_owned(), line: hl, col: hc }],
        });
    }
    out
}

pub fn scan_axum(src: &str) -> Vec<Route> {
    let lines = Lines::new(src);
    let mut out = Vec::new();
    for m in axum_route_re().captures_iter(src) {
        let whole = m.get(0).unwrap();
        let (line, _) = lines.at(src, whole.start());
        let open = whole.start() + whole.as_str().find('(').unwrap_or(0);
        let Some((_, end)) = call_args(src, open) else { continue };
        let body = &src[whole.end()..end.saturating_sub(1)];
        for mm in axum_method_re().captures_iter(body) {
            let h = mm.get(2).unwrap();
            let (hl, hc) = lines.at(src, whole.end() + h.start());
            out.push(Route {
                method: mm[1].to_ascii_uppercase(),
                path: m[1].to_owned(),
                line,
                handlers: vec![Handler { name: h.as_str().to_owned(), line: hl, col: hc }],
            });
        }
    }
    out
}

/// Route path of a Next.js file (`pages/users/[id].tsx` -> `/users/[id]`), or `None`.
pub fn next_route_of(rel: &str) -> Option<(String, bool)> {
    let strip_ext = |s: &str| -> Option<String> {
        let (stem, ext) = s.rsplit_once('.')?;
        matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs").then(|| stem.to_owned())
    };
    let parts: Vec<&str> = rel.split('/').collect();
    let at = parts.iter().position(|p| *p == "pages" || *p == "app")?;
    let (kind, rest) = (parts[at], &parts[at + 1..]);
    if rest.is_empty() {
        return None;
    }
    let last = strip_ext(rest[rest.len() - 1])?;
    let mut segs: Vec<String> = rest[..rest.len() - 1].iter().filter(|s| !(s.starts_with('(') && s.ends_with(')'))).map(|s| s.to_string()).collect();
    if kind == "pages" {
        if last.starts_with('_') {
            return None;
        }
        if last != "index" {
            segs.push(last);
        }
        Some((format!("/{}", segs.join("/")), false))
    } else {
        match last.as_str() {
            "page" => Some((format!("/{}", segs.join("/")), false)),
            "route" => Some((format!("/{}", segs.join("/")), true)),
            _ => None,
        }
    }
}

pub fn scan_next(rel: &str, src: &str) -> Vec<Route> {
    let Some((path, is_handler)) = next_route_of(rel) else { return Vec::new() };
    let lines = Lines::new(src);
    let mut out = Vec::new();
    let is_pages_api = rel.contains("pages/api/") || is_handler;
    if is_handler && !rel.contains("pages/") {
        for m in next_method_re().captures_iter(src) {
            let g = m.get(1).unwrap();
            let (l, c) = lines.at(src, g.start());
            out.push(Route {
                method: g.as_str().to_owned(),
                path: path.clone(),
                line: l,
                handlers: vec![Handler { name: g.as_str().to_owned(), line: l, col: c }],
            });
        }
        return out;
    }
    if let Some(m) = next_default_re().captures(src) {
        let g = m.get(1).unwrap();
        if !matches!(g.as_str(), "function" | "async" | "class") {
            let (l, c) = lines.at(src, g.start());
            out.push(Route {
                method: if is_pages_api { "ANY".into() } else { "GET".into() },
                path,
                line: l,
                handlers: vec![Handler { name: g.as_str().to_owned(), line: l, col: c }],
            });
        }
    }
    out
}

pub fn scan(fw: Framework, lang: Lang, rel: &str, src: &str) -> Vec<Route> {
    match fw {
        Framework::Express => scan_express(src),
        Framework::Flask | Framework::FastApi => scan_python_decorators(src),
        Framework::Django => {
            if rel.ends_with("urls.py") || src.contains("urlpatterns") {
                scan_django(src)
            } else {
                Vec::new()
            }
        }
        Framework::Gin => scan_gin(src),
        Framework::Spring => scan_spring(src, lang == Lang::Kotlin),
        Framework::Axum => scan_axum(src),
        Framework::Next => scan_next(rel, src),
    }
}

pub struct RouteHook {
    pub fw: Framework,
}

impl FileHook for RouteHook {
    fn name(&self) -> &'static str {
        match self.fw {
            Framework::Express => "express-routes",
            Framework::Flask => "flask-routes",
            Framework::FastApi => "fastapi-routes",
            Framework::Django => "django-routes",
            Framework::Gin => "gin-routes",
            Framework::Spring => "spring-routes",
            Framework::Axum => "axum-routes",
            Framework::Next => "next-routes",
        }
    }
    fn applies(&self, lang: Lang, _rel: &str) -> bool {
        match self.fw {
            Framework::Express | Framework::Next => lang.family() == LangFamily::Js,
            Framework::Flask | Framework::FastApi | Framework::Django => lang == Lang::Python,
            Framework::Gin => lang == Lang::Go,
            Framework::Spring => matches!(lang, Lang::Java | Lang::Kotlin),
            Framework::Axum => lang == Lang::Rust,
        }
    }
    fn triggers(&self) -> &'static [&'static str] {
        match self.fw {
            Framework::Express => &[".get(", ".post(", ".put(", ".delete(", ".patch(", ".all(", ".use(", ".options(", ".head("],
            Framework::Flask | Framework::FastApi => &["@"],
            Framework::Django => &["path(", "url("],
            Framework::Gin => &[".GET(", ".POST(", ".PUT(", ".DELETE(", ".PATCH(", ".Any(", ".OPTIONS(", ".HEAD("],
            Framework::Spring => &["Mapping"],
            Framework::Axum => &[".route("],
            Framework::Next => &["export"],
        }
    }
    fn run(&self, ctx: &HookCtx<'_>, out: &mut HookOut) {
        let routes = scan(self.fw, ctx.lang, ctx.rel, ctx.src);
        let kcount = ctx.rows.node_count() as u32;
        for r in routes {
            let name = format!("{} {}", r.method, r.path);
            let row = kcount + out.nodes.len() as u32;
            out.nodes.push(ExtraNode {
                kind: NodeKind::Route,
                qname: format!("{}::{}:{}", ctx.rel, r.method, r.path),
                digest: node_digest(ctx.rel, NodeKind::Route, &name, r.line),
                name,
                start_line: r.line,
                end_line: r.line,
                start_col: 0,
                end_col: 0,
                signature: None,
                parent_row: 0,
                exported: None,
            });
            for h in r.handlers {
                out.refs.push(ExtraRef { from_row: row, kind: RefKind::FunctionRef, name: h.name, line: h.line, col: h.col });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(r: &[Route]) -> Vec<String> {
        r.iter().map(|r| format!("{} {} -> {}", r.method, r.path, r.handlers.iter().map(|h| h.name.as_str()).collect::<Vec<_>>().join("|"))).collect()
    }

    #[test]
    fn express() {
        let src = "const app = express();\napp.get('/users/:id', auth, ctrl.show);\nrouter.post(\"/users\", [validate, rate], create);\napp.use('/api', apiRouter);\napp.get('/inline', (req, res) => res.send('x'));\nfoo.get('/nope', h);\napp.get(`/tpl`, tplHandler)";
        assert_eq!(
            names(&scan_express(src)),
            vec![
                "GET /users/:id -> auth|ctrl.show",
                "POST /users -> validate|rate|create",
                "USE /api -> apiRouter",
                "GET /inline -> ",
                "GET /tpl -> tplHandler"
            ]
        );
    }

    #[test]
    fn flask_and_fastapi() {
        let src = "@app.route('/a', methods=['GET', 'POST'])\n@login_required\ndef a():\n    pass\n\n@router.get(\"/items/{id}\")\nasync def read_item(id: int):\n    pass\n\n@app.route('/plain')\ndef plain(): pass\n";
        assert_eq!(names(&scan_python_decorators(src)), vec!["GET /a -> a", "POST /a -> a", "GET /items/{id} -> read_item", "GET /plain -> plain"]);
    }

    #[test]
    fn docstring_examples_are_not_routes() {
        let src = "def f():\n    \"\"\"\n    @app.route('/example')\n    def hello(): pass\n    \"\"\"\n# @app.route('/commented')\n@app.route('/real')\ndef real(): pass\n";
        assert_eq!(names(&scan_python_decorators(src)), vec!["GET /real -> real"]);
    }

    #[test]
    fn django() {
        let src = "urlpatterns = [\n    path('users/', views.user_list, name='users'),\n    path('x/', XView.as_view()),\n    path('admin/', admin.site.urls),\n    path('api/', include('api.urls')),\n]";
        assert_eq!(names(&scan_django(src)), vec!["ANY /users/ -> views.user_list", "ANY /x/ -> XView"]);
    }

    #[test]
    fn gin() {
        let src = "r.GET(\"/ping\", ping)\nv1.POST(\"/users\", auth(), handlers.Create)\nr.Any(\"/any\", func(c *gin.Context) {})";
        assert_eq!(names(&scan_gin(src)), vec!["GET /ping -> ping", "POST /users -> handlers.Create", "ANY /any -> "]);
    }

    #[test]
    fn spring() {
        let src = "@RestController\n@RequestMapping(\"/api\")\npublic class UserController {\n    @GetMapping(\"/users/{id}\")\n    public User get(@PathVariable Long id) { return null; }\n\n    @RequestMapping(value = \"/save\", method = RequestMethod.POST)\n    public void save() {}\n}\n";
        assert_eq!(names(&scan_spring(src, false)), vec!["GET /api/users/{id} -> get", "POST /api/save -> save"]);
        let kt = "@GetMapping(\"/k\")\nfun listAll(): List<X> = emptyList()\n";
        assert_eq!(names(&scan_spring(kt, true)), vec!["GET /k -> listAll"]);
    }

    #[test]
    fn axum() {
        let src = "let app = Router::new()\n    .route(\"/\", get(root))\n    .route(\"/users\", get(list).post(handlers::create));";
        assert_eq!(names(&scan_axum(src)), vec!["GET / -> root", "GET /users -> list", "POST /users -> handlers::create"]);
    }

    #[test]
    fn next() {
        assert_eq!(next_route_of("pages/users/[id].tsx"), Some(("/users/[id]".into(), false)));
        assert_eq!(next_route_of("src/pages/index.tsx"), Some(("/".into(), false)));
        assert_eq!(next_route_of("app/(shop)/cart/page.tsx"), Some(("/cart".into(), false)));
        assert_eq!(next_route_of("app/api/x/route.ts"), Some(("/api/x".into(), true)));
        assert_eq!(next_route_of("pages/_app.tsx"), None);
        let page = "export default function Cart() { return null }";
        assert_eq!(names(&scan_next("app/cart/page.tsx", page)), vec!["GET /cart -> Cart"]);
        let api = "export async function GET(req) {}\nexport const POST = async () => {}";
        assert_eq!(names(&scan_next("app/api/x/route.ts", api)), vec!["GET /api/x -> GET", "POST /api/x -> POST"]);
    }
}
