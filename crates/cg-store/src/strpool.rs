//! String pools: sorted, deduplicated byte strings addressed by dense `StrId`s, so exact
//! lookup is a binary search and prefix scans are a contiguous range.

use crate::part::Part;
use cg_core::{StrId, NONE};
use std::ops::Range;

#[derive(Clone, Default, Debug)]
pub struct StrPool {
    /// `len + 1` offsets into `data`.
    pub off: Part<u32>,
    pub data: Part<u8>,
}

impl StrPool {
    pub fn len(&self) -> usize {
        self.off.len().saturating_sub(1)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn bytes(&self, id: StrId) -> &[u8] {
        let i = id.0 as usize;
        if id.0 == NONE || i + 1 >= self.off.len() {
            return &[];
        }
        &self.data[self.off[i] as usize..self.off[i + 1] as usize]
    }

    /// The string for `id`; empty for `NONE` / out of range.
    #[inline]
    pub fn get(&self, id: StrId) -> &str {
        // Pools are built from `&str`, so slices on entry boundaries are valid UTF-8.
        std::str::from_utf8(self.bytes(id)).unwrap_or("")
    }

    pub fn opt(&self, id: StrId) -> Option<&str> {
        (id.0 != NONE).then(|| self.get(id))
    }

    /// First index whose string is `>= s`.
    pub fn lower_bound(&self, s: &[u8]) -> usize {
        let (mut lo, mut hi) = (0usize, self.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.bytes(StrId(mid as u32)) < s {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    pub fn find(&self, s: &[u8]) -> Option<StrId> {
        let i = self.lower_bound(s);
        (i < self.len() && self.bytes(StrId(i as u32)) == s).then_some(StrId(i as u32))
    }

    /// Ids of all strings starting with `prefix`.
    pub fn prefix_range(&self, prefix: &[u8]) -> Range<usize> {
        let lo = self.lower_bound(prefix);
        let mut hi = lo;
        while hi < self.len() && self.bytes(StrId(hi as u32)).starts_with(prefix) {
            hi += 1;
        }
        lo..hi
    }

    pub fn iter(&self) -> impl Iterator<Item = (StrId, &str)> + '_ {
        (0..self.len()).map(move |i| (StrId(i as u32), self.get(StrId(i as u32))))
    }

    /// Build from strings that are already sorted by bytes and unique.
    pub fn from_sorted_unique<S: AsRef<str>>(items: &[S]) -> StrPool {
        let total: usize = items.iter().map(|s| s.as_ref().len()).sum();
        let mut off = Vec::with_capacity(items.len() + 1);
        let mut data = Vec::with_capacity(total);
        off.push(0u32);
        for s in items {
            data.extend_from_slice(s.as_ref().as_bytes());
            off.push(data.len() as u32);
        }
        StrPool { off: Part::owned(off), data: Part::owned(data) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_and_prefix() {
        let p = StrPool::from_sorted_unique(&["alpha", "beta", "betamax", "gamma"]);
        assert_eq!(p.len(), 4);
        assert_eq!(p.find(b"beta"), Some(StrId(1)));
        assert_eq!(p.find(b"bet"), None);
        assert_eq!(p.prefix_range(b"beta"), 1..3);
        assert_eq!(p.prefix_range(b"zzz"), 4..4);
        assert_eq!(p.get(StrId(3)), "gamma");
        assert_eq!(p.get(StrId::NONE), "");
    }
}
