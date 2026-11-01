//! Repack a live base + delta view without reading source or running resolution.
//! Only reachable records and strings survive; all dense ids are reassigned in path order.

use crate::build::{postings, BuildBase, Draft, EdgeBuild};
use crate::intern::Interner;
use crate::{finish, outkind, status, FileFacts, FileNameRec, FinishInput, Part, Seg, SegParts, StrPool, View};
use cg_core::{FileId, ListId, NodeIdx, NodeKind, ScopeId, StrId, NONE};
use rustc_hash::FxHashMap;

fn remap(map: &[u32], id: u32) -> u32 {
    if id == NONE {
        NONE
    } else {
        map[id as usize]
    }
}

fn text<'a>(view: &'a View, pool: &mut Interner<'a>, id: StrId) -> StrId {
    if id.0 == NONE {
        StrId::NONE
    } else {
        StrId(pool.intern(view.text_str(id)))
    }
}

/// Produce an ordinary base segment. This operation needs no project directory, parsers or
/// resolver and preserves reference outcomes, provenance, source locations and metadata.
pub fn compact<'a>(view: &'a View) -> Seg {
    let mut files: Vec<_> = view.files().collect();
    files.sort_by(|a, b| view.file(*a).path().as_bytes().cmp(view.file(*b).path().as_bytes()));
    // Edges-only files keep base import/export facts. Those advisory targets can still
    // name a removed base file, including the last id; map them to NONE rather than panic.
    let mut fm = vec![NONE; view.base.p.files.len().max(files.iter().map(|f| f.idx() + 1).max().unwrap_or(0))];
    let mut nm = vec![NONE; view.base.p.nodes_hot.len() + view.delta.as_ref().map_or(0, |d| d.seg.p.nodes_hot.len())];
    let mut rm = vec![NONE; view.n_refs()];
    let (mut nn, mut nr) = (0, 0);
    for (i, &f) in files.iter().enumerate() {
        fm[f.idx()] = i as u32;
        for n in view.nodes_in_file(f) {
            nm[n as usize] = nn;
            nn += 1;
        }
        for r in view.ref_range(f) {
            rm[r as usize] = nr;
            nr += 1;
        }
    }
    let mut p = SegParts::default();
    let (mut names, mut paths, mut texts) = (Interner::default(), Interner::default(), Interner::default());
    let (mut frs, mut hot, mut cold, mut refs) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut imports, mut exports, mut scopes, mut bindings, mut sites, mut facts) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut list_off, mut list_items) = (vec![0u32], Vec::new());
    let mut list_map: FxHashMap<u32, ListId> = FxHashMap::default();
    let mut copy_list = |id: ListId, texts: &mut Interner<'a>| -> ListId {
        if id.0 == NONE {
            return id;
        }
        *list_map.entry(id.0).or_insert_with(|| {
            let new = ListId((list_off.len() - 1) as u32);
            for s in view.list(id) {
                list_items.push(texts.intern(s));
            }
            list_off.push(list_items.len() as u32);
            new
        })
    };
    let (mut edges, mut emeta) = (Vec::new(), Vec::new());
    let mut emap = FxHashMap::default();
    let mut fnames = Vec::new();
    for &f in &files {
        let file = FileId(fm[f.idx()]);
        let rec = *view.file(f).rec;
        let path = view.file(f).path();
        let base = path.rsplit('/').next().unwrap_or(path);
        fnames.push(FileNameRec { name: StrId(names.intern(base)), file });
        if let Some((stem, _)) = base.rsplit_once('.') {
            fnames.push(FileNameRec { name: StrId(names.intern(stem)), file });
        }
        let mut new_rec = rec;
        new_rec.path = StrId(paths.intern(path));
        new_rec.errors = text(view, &mut texts, rec.errors);
        new_rec.node_start = NodeIdx(hot.len() as u32);
        new_rec.ref_start = refs.len() as u32;
        frs.push(new_rec);
        for old in view.nodes_in_file(f) {
            let n = NodeIdx(old);
            let (mut h, mut c) = (*view.hot(n), *view.cold(n));
            h.file = file;
            h.parent = NodeIdx(remap(&nm, h.parent.0));
            h.name = StrId(names.intern(view.name_str(h.name)));
            h.qname = StrId(names.intern(view.name_str(h.qname)));
            c.signature = text(view, &mut texts, c.signature);
            c.docstring = text(view, &mut texts, c.docstring);
            c.return_type = text(view, &mut texts, c.return_type);
            c.extra = text(view, &mut texts, c.extra);
            c.decorators = copy_list(c.decorators, &mut texts);
            c.type_params = copy_list(c.type_params, &mut texts);
            hot.push(h);
            cold.push(c);
            for e in view.out_edges(n) {
                let meta = match view.edge_meta(e.eid) {
                    Some(s) => *emap.entry(s).or_insert_with(|| {
                        emeta.push(s.to_owned());
                        (emeta.len() - 1) as u32
                    }),
                    None => NONE,
                };
                edges.push(EdgeBuild {
                    src: remap(&nm, old),
                    dst: remap(&nm, e.dst.0),
                    kind: outkind::pack(e.kind, e.provenance, e.class),
                    line: e.line.unwrap_or(NONE),
                    col: e.col.unwrap_or(NONE),
                    res: e.res,
                    ref_idx: e.ref_idx.map_or(NONE, |r| remap(&rm, r)),
                    meta,
                    order: 0,
                });
            }
        }
        for old in view.refs_of_file(f) {
            let mut r = *old;
            r.from = NodeIdx(remap(&nm, r.from.0));
            if r.status == status::RESOLVED {
                r.target = NodeIdx(remap(&nm, r.target.0));
            }
            r.name = text(view, &mut texts, r.name);
            r.tail = if r.tail.0 == NONE { StrId::NONE } else { StrId(names.intern(view.name_str(r.tail))) };
            r.candidates = copy_list(r.candidates, &mut texts);
            refs.push(r);
        }
        let mut ff = FileFacts {
            imp_start: imports.len() as u32,
            imp_count: view.imports_of(f).len() as u32,
            exp_start: exports.len() as u32,
            exp_count: view.exports_of(f).len() as u32,
            scope_start: scopes.len() as u32,
            scope_count: view.scopes_of(f).len() as u32,
            bind_start: bindings.len() as u32,
            bind_count: view.bindings_of(f).len() as u32,
            site_start: sites.len() as u32,
            site_count: view.sites_of(f).len() as u32,
        };
        let old_scope = view.scope_base(f);
        let scope = |id: ScopeId| if id.0 == NONE { id } else { ScopeId(id.0 - old_scope + ff.scope_start) };
        for old in view.imports_of(f) {
            let mut r = *old;
            r.file = file;
            r.target = FileId(remap(&fm, r.target.0));
            r.scope = scope(r.scope);
            r.local = text(view, &mut texts, r.local);
            r.imported = text(view, &mut texts, r.imported);
            r.spec = text(view, &mut texts, r.spec);
            imports.push(r);
        }
        for old in view.exports_of(f) {
            let mut r = *old;
            r.file = file;
            r.node = NodeIdx(remap(&nm, r.node.0));
            r.target = FileId(remap(&fm, r.target.0));
            r.name = text(view, &mut texts, r.name);
            r.from_spec = text(view, &mut texts, r.from_spec);
            r.from_name = text(view, &mut texts, r.from_name);
            exports.push(r);
        }
        for old in view.scopes_of(f) {
            let mut r = *old;
            r.file = file;
            r.parent = scope(r.parent);
            scopes.push(r);
        }
        for old in view.bindings_of(f) {
            let mut r = *old;
            r.scope = scope(r.scope);
            r.node = NodeIdx(remap(&nm, r.node.0));
            r.name = text(view, &mut texts, r.name);
            r.decl_type = text(view, &mut texts, r.decl_type);
            r.init_ctor = text(view, &mut texts, r.init_ctor);
            bindings.push(r);
        }
        for old in view.sites_of(f) {
            let mut r = *old;
            r.node = NodeIdx(remap(&nm, r.node.0));
            r.key_a = text(view, &mut texts, StrId(r.key_a)).0;
            r.key_b = text(view, &mut texts, StrId(r.key_b)).0;
            r.payload = text(view, &mut texts, r.payload);
            sites.push(r);
        }
        // Scope ids are local to their owning segment; all are dense in the new base.
        ff.scope_count = view.scopes_of(f).len() as u32;
        facts.push(ff);
    }
    let (ns, nmap) = names.finalize();
    let (ps, pmap) = paths.finalize();
    let (ts, tmap) = texts.finalize();
    let rt = |id: &mut StrId| id.0 = remap(&tmap, id.0);
    for f in &mut frs {
        f.path.0 = remap(&pmap, f.path.0);
        rt(&mut f.errors);
    }
    for h in &mut hot {
        h.name.0 = remap(&nmap, h.name.0);
        h.qname.0 = remap(&nmap, h.qname.0);
    }
    for c in &mut cold {
        rt(&mut c.signature);
        rt(&mut c.docstring);
        rt(&mut c.return_type);
        rt(&mut c.extra);
    }
    for r in &mut refs {
        rt(&mut r.name);
        r.tail.0 = remap(&nmap, r.tail.0);
    }
    for r in &mut imports {
        rt(&mut r.local);
        rt(&mut r.imported);
        rt(&mut r.spec);
    }
    for r in &mut exports {
        rt(&mut r.name);
        rt(&mut r.from_spec);
        rt(&mut r.from_name);
    }
    for r in &mut bindings {
        rt(&mut r.name);
        rt(&mut r.decl_type);
        rt(&mut r.init_ctor);
    }
    for r in &mut sites {
        r.key_a = remap(&tmap, r.key_a);
        r.key_b = remap(&tmap, r.key_b);
        rt(&mut r.payload);
    }
    for r in &mut list_items {
        *r = remap(&tmap, *r);
    }
    for r in &mut fnames {
        r.name.0 = remap(&nmap, r.name.0);
    }
    fnames.sort_unstable_by_key(|r| (r.name.0, r.file.0));
    fnames.dedup();
    let names = StrPool::from_sorted_unique(&ns);
    let paths = StrPool::from_sorted_unique(&ps);
    let texts = StrPool::from_sorted_unique(&ts);
    let skip = |kind| !NodeKind::from_u8(kind).is_some_and(|k| k.is_definition());
    let (no, np) = postings(names.len(), &hot, BuildBase::default(), |h| h.name, &skip);
    let (qo, qp) = postings(names.len(), &hot, BuildBase::default(), |h| h.qname, &skip);
    p.names_off = names.off;
    p.names_data = names.data;
    p.paths_off = paths.off;
    p.paths_data = paths.data;
    p.text_off = texts.off;
    p.text_data = texts.data;
    p.files = Part::owned(frs);
    p.nodes_hot = Part::owned(hot);
    p.nodes_cold = Part::owned(cold);
    p.refs = Part::owned(refs);
    p.imports = Part::owned(imports);
    p.exports = Part::owned(exports);
    p.scopes = Part::owned(scopes);
    p.bindings = Part::owned(bindings);
    p.sites = Part::owned(sites);
    p.file_facts = Part::owned(facts);
    p.lists_off = Part::owned(list_off);
    p.lists = Part::owned(list_items);
    p.name_post_off = Part::owned(no);
    p.name_post = Part::owned(np);
    p.qname_post_off = Part::owned(qo);
    p.qname_post = Part::owned(qp);
    p.fname_idx = Part::owned(fnames);
    let mut meta = view.base.meta.clone();
    meta.langs = files.iter().map(|f| view.file(*f).lang().as_str().to_owned()).collect();
    meta.langs.sort();
    meta.langs.dedup();
    meta.kind = "base".into();
    meta.base_gen = 0;
    meta.n_files = files.len() as u32;
    meta.n_nodes = nn;
    meta.n_refs = nr;
    finish(Draft { seg: Seg::from_parts(meta, p), edges, emeta }, FinishInput::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cg_core::Lang;
    use cg_extract::{extract_bytes, ExtractConfig};

    #[test]
    fn spilled_and_resident_extraction_produce_identical_segments() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ExtractConfig::new(dir.path());
        let mut batches = vec![
            extract_bytes("lib.ts", b"export class Service { run(): number { return 1; } }\n", 7, Lang::TypeScript, &cfg),
            extract_bytes("app.ts", b"import { Service } from './lib';\nexport function start() { new Service().run(); }\n", 8, Lang::TypeScript, &cfg),
        ];
        batches.sort_by(|a, b| a.rel.cmp(&b.rel));
        let build = |b: &[cg_extract::FileBatch]| finish(crate::build_draft(b, &crate::ProjectMeta::default()), FinishInput::default()).to_bytes();
        let resident = build(&batches);
        assert!(cg_extract::spill::spill(&mut batches, dir.path()).unwrap() > 0);
        assert_eq!(resident, build(&batches));
    }
}
