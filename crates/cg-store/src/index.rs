//! Search index construction (design doc 5.4): everything a query needs is computed here,
//! once, at index time. Queries are then lookups; nothing depends on statistics or an
//! optimiser.

use crate::records::*;
use crate::strpool::StrPool;
use crate::tokenize::{self, split_identifier};
use cg_core::{NodeKind, StrId, NONE};
use rayon::prelude::*;
use rustc_hash::FxHashMap;

pub const F_NAME: u8 = 1;
pub const F_QNAME: u8 = 2;
pub const F_SIG: u8 = 4;
pub const F_DOC: u8 = 8;
pub const F_PATH: u8 = 16;

pub struct NameIndexes {
    pub names_fst: Vec<u8>,
    pub lower_fst: Vec<u8>,
    pub lower_post_off: Vec<u32>,
    pub lower_post: Vec<u32>,
    pub segs_fst: Vec<u8>,
    pub seg_post_off: Vec<u32>,
    pub seg_post: Vec<u32>,
    pub tri_keys: Vec<u32>,
    pub tri_off: Vec<u32>,
    pub tri_post: Vec<u32>,
}

fn fst_from_sorted<'a>(keys: impl Iterator<Item = (&'a [u8], u64)>) -> Vec<u8> {
    let mut b = fst::MapBuilder::memory();
    for (k, v) in keys {
        b.insert(k, v).expect("keys must be sorted and unique");
    }
    b.into_inner().expect("fst finish")
}

/// Group sorted `(key, value)` pairs into (unique keys, offsets, values).
fn group_pairs<K: PartialEq + Clone>(pairs: &[(K, u32)]) -> (Vec<K>, Vec<u32>, Vec<u32>) {
    let mut keys: Vec<K> = Vec::new();
    let mut off = vec![0u32];
    let mut vals = Vec::with_capacity(pairs.len());
    for (k, v) in pairs {
        if keys.last() != Some(k) {
            if !keys.is_empty() {
                off.push(vals.len() as u32);
            }
            keys.push(k.clone());
        }
        vals.push(*v);
    }
    if !keys.is_empty() {
        off.push(vals.len() as u32);
    }
    (keys, off, vals)
}

/// `has_post(id)`: does the name string have any node posting (definitions only)?
pub fn build_name_indexes(names: &StrPool, has_post: impl Fn(usize) -> bool + Sync) -> NameIndexes {
    let ids: Vec<u32> = (0..names.len() as u32).into_par_iter().filter(|i| has_post(*i as usize)).collect();
    let names_fst = fst_from_sorted(ids.iter().map(|i| (names.bytes(StrId(*i)), *i as u64)));

    // lower-case -> original names
    let mut lower: Vec<(String, u32)> = ids.par_iter().map(|i| (names.get(StrId(*i)).to_lowercase(), *i)).collect();
    lower.par_sort_unstable();
    let (lkeys, lower_post_off, lower_post) = group_pairs(&lower);
    let lower_fst = fst_from_sorted(lkeys.iter().enumerate().map(|(i, k)| (k.as_bytes(), i as u64)));
    drop(lower);

    // identifier segments -> names containing them
    let mut segs: Vec<(String, u32)> = ids
        .par_iter()
        .flat_map_iter(|i| {
            let mut v = Vec::new();
            split_identifier(names.get(StrId(*i)), &mut v);
            v.sort();
            v.dedup();
            v.into_iter().filter(|s| !s.is_empty()).map(move |s| (s, *i))
        })
        .collect();
    segs.par_sort_unstable();
    let (skeys, seg_post_off, seg_post) = group_pairs(&segs);
    let segs_fst = fst_from_sorted(skeys.iter().enumerate().map(|(i, k)| (k.as_bytes(), i as u64)));
    drop(segs);

    // trigrams of lower-cased names
    let mut tris: Vec<(u32, u32)> = ids
        .par_iter()
        .flat_map_iter(|i| {
            let l = names.get(StrId(*i)).to_lowercase();
            let b = l.as_bytes();
            let mut v: Vec<u32> = b.windows(3).map(|w| (w[0] as u32) << 16 | (w[1] as u32) << 8 | w[2] as u32).collect();
            v.sort_unstable();
            v.dedup();
            v.into_iter().map(move |t| (t, *i))
        })
        .collect();
    tris.par_sort_unstable();
    let (tri_keys, tri_off, tri_post) = group_pairs(&tris);

    NameIndexes { names_fst, lower_fst, lower_post_off, lower_post, segs_fst, seg_post_off, seg_post, tri_keys, tri_off, tri_post }
}

pub struct TokenIndex {
    pub fst: Vec<u8>,
    pub post_off: Vec<u32>,
    pub post: Vec<PostEntry>,
    pub node_norm: Vec<u16>,
}

#[derive(Clone, Copy)]
struct Entry {
    tok: u32,
    node: u32,
    mask: u8,
    tf: u8,
}

struct Dict {
    map: FxHashMap<Box<str>, u32>,
    toks: Vec<Box<str>>,
}

impl Dict {
    fn id(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = self.toks.len() as u32;
        let b: Box<str> = s.into();
        self.toks.push(b.clone());
        self.map.insert(b, i);
        i
    }
}

pub struct TokenInputs<'a> {
    pub hot: &'a [NodeHot],
    pub cold: &'a [NodeCold],
    pub files: &'a [FileRec],
    pub names: &'a StrPool,
    pub paths: &'a StrPool,
    pub text: &'a StrPool,
    /// Delta layers: ids in the records are global (base length + local id); these are the
    /// base lengths (all zero for a base segment).
    pub names_base: u32,
    pub paths_base: u32,
    pub text_base: u32,
    pub node_base: u32,
    /// Delta layers: global `FileId` -> position in `files`.
    pub file_slot: Option<&'a FxHashMap<u32, usize>>,
}

pub fn build_token_index(inp: &TokenInputs<'_>) -> TokenIndex {
    let mut dict = Dict { map: FxHashMap::default(), toks: Vec::new() };
    let mut name_cache: FxHashMap<u32, Box<[u32]>> = FxHashMap::default();
    let mut qname_cache: FxHashMap<u32, Box<[u32]>> = FxHashMap::default();
    let mut entries: Vec<Entry> = Vec::with_capacity(inp.hot.len() * 12);
    let mut norm = vec![0u16; inp.hot.len()];
    let mut scratch: Vec<String> = Vec::new();
    let mut per_node: Vec<(u32, u8)> = Vec::new();

    for (n, (h, c)) in inp.hot.iter().zip(inp.cold.iter()).enumerate() {
        let kind = NodeKind::from_u8(h.kind);
        if matches!(kind, Some(NodeKind::Import | NodeKind::Export | NodeKind::Parameter) | None) {
            continue;
        }
        per_node.clear();
        let mut len = 0usize;
        // name: whole lower-cased name plus identifier segments
        let nt = name_cache.entry(h.name.0).or_insert_with(|| {
            let name = inp.names.get(StrId(h.name.0 - inp.names_base));
            scratch.clear();
            split_identifier(name, &mut scratch);
            let mut ids: Vec<u32> = scratch.iter().map(|s| dict.id(s)).collect();
            let whole = name.to_lowercase();
            if scratch.len() != 1 || scratch[0] != whole {
                ids.push(dict.id(&whole));
            }
            ids.sort_unstable();
            ids.dedup();
            ids.into_boxed_slice()
        });
        for t in nt.iter() {
            per_node.push((*t, F_NAME));
        }
        len += nt.len();
        if h.qname.0 != h.name.0 && h.qname.0 != NONE {
            let qt = qname_cache.entry(h.qname.0).or_insert_with(|| {
                scratch.clear();
                split_identifier(inp.names.get(StrId(h.qname.0 - inp.names_base)), &mut scratch);
                let mut ids: Vec<u32> = scratch.iter().map(|s| dict.id(s)).collect();
                ids.sort_unstable();
                ids.dedup();
                ids.into_boxed_slice()
            });
            for t in qt.iter() {
                per_node.push((*t, F_QNAME));
            }
            len += qt.len();
        }
        if c.signature.0 != NONE {
            scratch.clear();
            tokenize::words(inp.text.get(StrId(c.signature.0 - inp.text_base)), 32, &mut scratch);
            for w in scratch.iter() {
                if !tokenize::is_stopword(w) {
                    per_node.push((dict.id(w), F_SIG));
                    len += 1;
                }
            }
        }
        if c.docstring.0 != NONE {
            scratch.clear();
            tokenize::words(inp.text.get(StrId(c.docstring.0 - inp.text_base)), 64, &mut scratch);
            for w in scratch.iter() {
                if !tokenize::is_stopword(w) {
                    per_node.push((dict.id(w), F_DOC));
                    len += 1;
                }
            }
        }
        if kind == Some(NodeKind::File) {
            let slot = inp.file_slot.map_or(h.file.0 as usize, |m| m[&h.file.0]);
            let f = &inp.files[slot];
            scratch.clear();
            for seg in inp.paths.get(StrId(f.path.0 - inp.paths_base)).split('/') {
                split_identifier(seg, &mut scratch);
            }
            for w in scratch.iter() {
                per_node.push((dict.id(w), F_PATH));
                len += 1;
            }
        }
        norm[n] = len.min(u16::MAX as usize) as u16;
        per_node.sort_unstable();
        let mut i = 0;
        while i < per_node.len() {
            let tok = per_node[i].0;
            let (mut mask, mut tf) = (0u8, 0u32);
            while i < per_node.len() && per_node[i].0 == tok {
                mask |= per_node[i].1;
                tf += 1;
                i += 1;
            }
            entries.push(Entry { tok, node: n as u32 + inp.node_base, mask, tf: tf.min(255) as u8 });
        }
    }

    // rank tokens lexicographically; the FST value is the rank = posting list index
    let mut order: Vec<u32> = (0..dict.toks.len() as u32).collect();
    order.par_sort_unstable_by(|a, b| dict.toks[*a as usize].as_bytes().cmp(dict.toks[*b as usize].as_bytes()));
    let mut rank = vec![0u32; order.len()];
    for (r, t) in order.iter().enumerate() {
        rank[*t as usize] = r as u32;
    }
    let fst = fst_from_sorted(order.iter().enumerate().map(|(r, t)| (dict.toks[*t as usize].as_bytes(), r as u64)));
    let mut post_off = vec![0u32; order.len() + 1];
    for e in &entries {
        post_off[rank[e.tok as usize] as usize + 1] += 1;
    }
    for i in 0..order.len() {
        post_off[i + 1] += post_off[i];
    }
    let mut fill = post_off.clone();
    let mut post = vec![PostEntry { node: 0, mask: 0, tf: 0, _p: 0 }; entries.len()];
    // entries were produced in ascending node order, so each list ends up sorted by node
    for e in &entries {
        let r = rank[e.tok as usize] as usize;
        post[fill[r] as usize] = PostEntry { node: e.node, mask: e.mask, tf: e.tf, _p: 0 };
        fill[r] += 1;
    }
    TokenIndex { fst, post_off, post, node_norm: norm }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_indexes_find_by_segment_and_trigram() {
        let names = StrPool::from_sorted_unique(&["Alpha", "getUserName", "render", "renderStaticScene"]);
        let ix = build_name_indexes(&names, |_| true);
        let segs = fst::Map::new(ix.segs_fst).unwrap();
        let user = segs.get("user").unwrap() as usize;
        let list = &ix.seg_post[ix.seg_post_off[user] as usize..ix.seg_post_off[user + 1] as usize];
        assert_eq!(list, &[1]);
        let lower = fst::Map::new(ix.lower_fst).unwrap();
        assert!(lower.get("getusername").is_some());
        let tri = (b'n' as u32) << 16 | (b'd' as u32) << 8 | b'e' as u32; // "nde" in renderStaticScene? no: "nde"
        let _ = tri;
        let key = (b'e' as u32) << 16 | (b'n' as u32) << 8 | b'd' as u32; // "end" in render
        let pos = ix.tri_keys.binary_search(&key).unwrap();
        let hit = &ix.tri_post[ix.tri_off[pos] as usize..ix.tri_off[pos + 1] as usize];
        assert_eq!(hit, &[2, 3]);
    }
}
