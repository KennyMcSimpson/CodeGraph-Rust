//! `cgr export --sqlite`: dump the graph of the current snapshot (base plus delta) into a plain
//! SQLite file so people can query it with SQL.
//!
//! The schema is versioned (`meta.schema_version`) and only ever extended compatibly:
//! new columns and tables may appear, existing ones keep their meaning.
//!
//! ```sql
//! files(id, path, language, size, flags, parse_errors)
//! nodes(id, file_id, kind, name, qualified_name, start_line, end_line, start_col, end_col,
//!       visibility, exported, async, static, abstract, signature, return_type, docstring,
//!       decorators, type_params, parent_id)
//! edges(id, source, target, kind, line, col, provenance, class, resolved_by, confidence, meta)
//! refs(id, file_id, from_node, name, kind, line, col, status, resolved_by, target, candidates, tail)
//! imports(file_id, local_name, imported_name, spec, kind, type_only, line, target_file)
//! symbols    -- view: nodes joined with their file path
//! relations  -- view: edges with both endpoints spelled out
//! ```

use anyhow::{anyhow, Context, Result};
use cg_core::{brand, NodeIdx, NONE};
use cg_query::Project;
use cg_store::{outkind, status, View};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

const SCHEMA: &str = "
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE files(
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    language TEXT NOT NULL,
    size INTEGER NOT NULL,
    flags INTEGER NOT NULL,
    parse_errors TEXT
);
CREATE TABLE nodes(
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    start_col INTEGER NOT NULL,
    end_col INTEGER NOT NULL,
    visibility TEXT,
    exported INTEGER,
    async INTEGER,
    static INTEGER,
    abstract INTEGER,
    signature TEXT,
    return_type TEXT,
    docstring TEXT,
    decorators TEXT,
    type_params TEXT,
    parent_id INTEGER
);
CREATE TABLE edges(
    id INTEGER PRIMARY KEY,
    source INTEGER NOT NULL,
    target INTEGER NOT NULL,
    kind TEXT NOT NULL,
    line INTEGER,
    col INTEGER,
    provenance TEXT,
    class TEXT NOT NULL,
    resolved_by TEXT,
    confidence INTEGER,
    meta TEXT
);
CREATE TABLE refs(
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL,
    from_node INTEGER NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    line INTEGER NOT NULL,
    col INTEGER NOT NULL,
    status TEXT NOT NULL,
    resolved_by TEXT,
    target INTEGER,
    candidates TEXT,
    tail TEXT
);
CREATE TABLE imports(
    file_id INTEGER NOT NULL,
    local_name TEXT NOT NULL,
    imported_name TEXT NOT NULL,
    spec TEXT NOT NULL,
    kind INTEGER NOT NULL,
    type_only INTEGER NOT NULL,
    line INTEGER NOT NULL,
    target_file INTEGER
);
";

const INDEXES: &str = "
CREATE INDEX nodes_file ON nodes(file_id);
CREATE INDEX nodes_name ON nodes(name);
CREATE INDEX nodes_qname ON nodes(qualified_name);
CREATE INDEX nodes_kind ON nodes(kind);
CREATE INDEX edges_source ON edges(source, kind);
CREATE INDEX edges_target ON edges(target, kind);
CREATE INDEX refs_file ON refs(file_id);
CREATE INDEX refs_status ON refs(status);
CREATE INDEX refs_target ON refs(target);
CREATE INDEX imports_file ON imports(file_id);
CREATE VIEW symbols AS
    SELECT n.id, f.path AS file, n.kind, n.name, n.qualified_name, n.start_line, n.end_line,
           n.visibility, n.signature
    FROM nodes n JOIN files f ON f.id = n.file_id WHERE n.kind <> 'file';
CREATE VIEW relations AS
    SELECT e.id, e.kind, s.qualified_name AS source, sf.path AS source_file,
           t.qualified_name AS target, tf.path AS target_file, e.line, e.class, e.resolved_by, e.confidence
    FROM edges e
    JOIN nodes s ON s.id = e.source JOIN files sf ON sf.id = s.file_id
    JOIN nodes t ON t.id = e.target JOIN files tf ON tf.id = t.file_id;
";

fn json_list(v: Vec<&str>) -> Option<String> {
    if v.is_empty() {
        None
    } else {
        serde_json::to_string(&v).ok()
    }
}

fn class_name(c: u8) -> &'static str {
    match c {
        outkind::CLASS_EXTRACT => "extracted",
        outkind::CLASS_RESOLVE => "resolved",
        outkind::CLASS_SYNTH => "synthesised",
        _ => "framework",
    }
}

fn opt<T>(b: Option<T>) -> Option<T> {
    b
}

fn node_id(n: NodeIdx) -> Option<i64> {
    if n.0 == NONE {
        None
    } else {
        Some(n.0 as i64)
    }
}

pub struct ExportStats {
    pub files: u64,
    pub nodes: u64,
    pub edges: u64,
    pub refs: u64,
    pub imports: u64,
    pub bytes: u64,
}

/// Write the snapshot into `out` (replaced if `force`). The file appears atomically: it is
/// built under a temporary name next to the target and renamed when complete.
pub fn export_sqlite(proj: &Project, out: &Path, force: bool) -> Result<ExportStats> {
    if out.exists() && !force {
        return Err(anyhow!("{} already exists (use --force to replace it)", out.display()));
    }
    let tmp: PathBuf = {
        let mut name = out.file_name().map(|s| s.to_os_string()).unwrap_or_default();
        name.push(".tmp");
        out.with_file_name(name)
    };
    let _ = std::fs::remove_file(&tmp);
    let stats = write_db(proj, &tmp).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    std::fs::rename(&tmp, out).with_context(|| format!("cannot move the export to {}", out.display()))?;
    Ok(ExportStats { bytes: std::fs::metadata(out).map(|m| m.len()).unwrap_or(0), ..stats })
}

fn write_db(proj: &Project, path: &Path) -> Result<ExportStats> {
    let view: &View = &proj.snap.view;
    let mut conn = Connection::open(path).with_context(|| format!("cannot create {}", path.display()))?;
    // the file is rebuilt from scratch on failure, so durability buys nothing here
    conn.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA locking_mode = EXCLUSIVE;")?;
    conn.execute_batch(SCHEMA)?;
    let tx = conn.transaction()?;
    let mut st = ExportStats { files: 0, nodes: 0, edges: 0, refs: 0, imports: 0, bytes: 0 };
    {
        let mut ins_file = tx.prepare("INSERT INTO files VALUES (?1,?2,?3,?4,?5,?6)")?;
        let mut ins_node = tx.prepare("INSERT INTO nodes VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)")?;
        let mut ins_edge = tx.prepare("INSERT INTO edges VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)")?;
        let mut ins_ref = tx.prepare("INSERT INTO refs VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)")?;
        let mut ins_imp = tx.prepare("INSERT INTO imports VALUES (?1,?2,?3,?4,?5,?6,?7,?8)")?;
        let mut edge_id: i64 = 0;
        let mut ref_id: i64 = 0;
        for f in view.files() {
            let fr = view.file(f);
            ins_file.execute(params![f.0 as i64, fr.path(), fr.lang().as_str(), fr.rec.size as i64, fr.flags() as i64, fr.errors()])?;
            st.files += 1;
            for n in view.nodes_in_file(f) {
                let ni = NodeIdx(n);
                if !view.is_alive(ni) {
                    continue;
                }
                let node = view.node(ni);
                ins_node.execute(params![
                    n as i64,
                    f.0 as i64,
                    node.kind().as_str(),
                    node.name(),
                    node.qname(),
                    node.start_line() as i64,
                    node.end_line() as i64,
                    node.start_col() as i64,
                    node.end_col() as i64,
                    node.visibility().as_str(),
                    opt(node.is_exported()),
                    opt(node.is_async()),
                    opt(node.is_static()),
                    opt(node.is_abstract()),
                    node.signature(),
                    node.return_type(),
                    node.docstring(),
                    json_list(node.decorators()),
                    json_list(node.type_params()),
                    node.parent().and_then(node_id),
                ])?;
                st.nodes += 1;
                for e in view.out_edges(ni) {
                    if !view.is_alive(e.dst) {
                        continue;
                    }
                    edge_id += 1;
                    let strategy = (e.res & 0xFF) as u8;
                    let conf = (e.res >> 8) as i64;
                    ins_edge.execute(params![
                        edge_id,
                        n as i64,
                        e.dst.0 as i64,
                        e.kind.as_str(),
                        e.line.map(|l| l as i64),
                        e.col.map(|c| c as i64),
                        e.provenance.as_str(),
                        class_name(e.class),
                        if strategy == 0 { None } else { Some(cg_resolve::by::name(strategy)) },
                        if strategy == 0 { None } else { Some(conf) },
                        view.edge_meta(e.eid),
                    ])?;
                    st.edges += 1;
                }
            }
            for r in view.refs_of_file(f) {
                ref_id += 1;
                let resolved = r.status == status::RESOLVED;
                ins_ref.execute(params![
                    ref_id,
                    f.0 as i64,
                    r.from.0 as i64,
                    view.text_str(r.name),
                    cg_core::RefKind::from_wire(r.kind).map_or("?", |k| k.as_str()),
                    r.line as i64,
                    r.col as i64,
                    status::name(r.status),
                    if r.by == 0 { None } else { Some(cg_resolve::by::name(r.by)) },
                    if resolved { Some(r.target.0 as i64) } else { None },
                    json_list(view.list(r.candidates)),
                    view.name_str(r.tail),
                ])?;
                st.refs += 1;
            }
            for i in view.imports_of(f) {
                ins_imp.execute(params![
                    f.0 as i64,
                    view.text_str(i.local),
                    view.text_str(i.imported),
                    view.text_str(i.spec),
                    i.kind as i64,
                    i.type_only as i64,
                    i.line as i64,
                    if i.target.0 == NONE { None } else { Some(i.target.0 as i64) },
                ])?;
                st.imports += 1;
            }
        }
    }
    {
        let m = &proj.snap.manifest;
        let mut meta = tx.prepare("INSERT INTO meta VALUES (?1, ?2)")?;
        for (k, v) in [
            ("schema_version", SCHEMA_VERSION.to_string()),
            ("generator", format!("{} {}", brand::BIN_NAME, env!("CARGO_PKG_VERSION"))),
            ("engine_version", m.engine_version.to_string()),
            ("generation", m.generation.to_string()),
            ("project_root", proj.root().display().to_string()),
            ("files", st.files.to_string()),
            ("nodes", st.nodes.to_string()),
            ("edges", st.edges.to_string()),
        ] {
            meta.execute(params![k, v])?;
        }
    }
    tx.commit()?;
    conn.execute_batch(INDEXES)?;
    conn.execute_batch("PRAGMA optimize;")?;
    drop(conn);
    Ok(st)
}
