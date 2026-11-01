//! String interning for the builder: strings borrowed from the per-file arenas (zero copy),
//! plus a small arena for strings the builder derives itself. Temp ids are assigned in first-seen
//! order; `finalize` sorts the unique strings and returns the remap table, which makes the
//! final ids independent of thread scheduling and of first-seen order.

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::cell::UnsafeCell;

/// Append-only owner of derived strings. Handed-out `&str`s stay valid until the arena drops
/// because each string lives in its own heap allocation that is never touched again.
#[derive(Default)]
pub struct StrArena {
    v: UnsafeCell<Vec<Box<str>>>,
}

impl StrArena {
    pub fn alloc(&self, s: String) -> &str {
        // SAFETY: single-threaded use (`UnsafeCell` makes the type !Sync); the Box<str>
        // allocation is never moved, mutated or dropped before `self` is.
        unsafe {
            let v = &mut *self.v.get();
            v.push(s.into_boxed_str());
            &*(v.last().unwrap().as_ref() as *const str)
        }
    }
}

#[derive(Default)]
pub struct Interner<'a> {
    map: FxHashMap<&'a str, u32>,
    items: Vec<&'a str>,
}

impl<'a> Interner<'a> {
    pub fn with_capacity(n: usize) -> Self {
        Interner { map: FxHashMap::with_capacity_and_hasher(n, Default::default()), items: Vec::with_capacity(n) }
    }

    #[inline]
    pub fn intern(&mut self, s: &'a str) -> u32 {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = self.items.len() as u32;
        self.items.push(s);
        self.map.insert(s, i);
        i
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, s: &str) -> Option<u32> {
        self.map.get(s).copied()
    }

    /// Sorted unique strings and `remap[temp] = final`.
    pub fn finalize(self) -> (Vec<&'a str>, Vec<u32>) {
        let items = self.items;
        let mut order: Vec<u32> = (0..items.len() as u32).collect();
        order.par_sort_unstable_by(|a, b| items[*a as usize].as_bytes().cmp(items[*b as usize].as_bytes()));
        let mut remap = vec![0u32; items.len()];
        let mut sorted = Vec::with_capacity(items.len());
        for (final_id, &temp) in order.iter().enumerate() {
            remap[temp as usize] = final_id as u32;
            sorted.push(items[temp as usize]);
        }
        (sorted, remap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_is_order_independent() {
        let mut a = Interner::default();
        let (x, y, z) = (a.intern("pear"), a.intern("apple"), a.intern("fig"));
        assert_eq!(a.intern("apple"), y);
        let (sorted, remap) = a.finalize();
        assert_eq!(sorted, vec!["apple", "fig", "pear"]);
        assert_eq!((remap[x as usize], remap[y as usize], remap[z as usize]), (2, 0, 1));
    }

    #[test]
    fn arena_strings_survive_growth() {
        let arena = StrArena::default();
        let first = arena.alloc("first".to_string());
        for i in 0..1000 {
            arena.alloc(format!("s{i}"));
        }
        assert_eq!(first, "first");
    }
}
