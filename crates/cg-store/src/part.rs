//! `Part<T>`: a typed, immutable array that is either owned (built in memory) or a slice of
//! an mmap'd segment file. Both deref to `&[T]`, so query code never knows the difference.

use bytemuck::Pod;
use cg_core::{CgError, Result};
use memmap2::Mmap;
use std::ops::Deref;
use std::sync::Arc;

pub enum Part<T: Pod> {
    Owned(Arc<Vec<T>>),
    Mapped { ptr: *const T, len: usize, _keep: Arc<Mmap> },
}

// SAFETY: the data behind `Mapped` is immutable and kept alive by `_keep`.
unsafe impl<T: Pod + Send + Sync> Send for Part<T> {}
unsafe impl<T: Pod + Send + Sync> Sync for Part<T> {}

impl<T: Pod> Clone for Part<T> {
    fn clone(&self) -> Self {
        match self {
            Part::Owned(v) => Part::Owned(v.clone()),
            Part::Mapped { ptr, len, _keep } => Part::Mapped { ptr: *ptr, len: *len, _keep: _keep.clone() },
        }
    }
}

impl<T: Pod> Default for Part<T> {
    fn default() -> Self {
        Part::Owned(Arc::new(Vec::new()))
    }
}

impl<T: Pod> Part<T> {
    pub fn owned(v: Vec<T>) -> Self {
        Part::Owned(Arc::new(v))
    }

    /// View `bytes` bytes at `offset` of the map as `[T]`; checks bounds, size and alignment.
    pub fn from_map(map: &Arc<Mmap>, offset: usize, bytes: usize) -> Result<Self> {
        let sz = std::mem::size_of::<T>();
        if sz == 0 || !bytes.is_multiple_of(sz) {
            return Err(CgError::corrupt(format!("partition size {bytes} is not a multiple of {sz}")));
        }
        let end = offset.checked_add(bytes).ok_or_else(|| CgError::corrupt("partition overflows"))?;
        if end > map.len() {
            return Err(CgError::corrupt("partition extends past the end of the file"));
        }
        let base = map.as_ptr() as usize + offset;
        if !base.is_multiple_of(std::mem::align_of::<T>()) {
            return Err(CgError::corrupt("misaligned partition"));
        }
        Ok(Part::Mapped { ptr: base as *const T, len: bytes / sz, _keep: map.clone() })
    }

    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(self.deref())
    }
}

impl<T: Pod> Deref for Part<T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        match self {
            Part::Owned(v) => v.as_slice(),
            // SAFETY: validated in `from_map`; the mmap outlives `self`.
            Part::Mapped { ptr, len, .. } => unsafe { std::slice::from_raw_parts(*ptr, *len) },
        }
    }
}

impl AsRef<[u8]> for Part<u8> {
    fn as_ref(&self) -> &[u8] {
        self.deref()
    }
}

impl<T: Pod> From<Vec<T>> for Part<T> {
    fn from(v: Vec<T>) -> Self {
        Part::owned(v)
    }
}

impl<T: Pod> std::fmt::Debug for Part<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Part<{}>[{}]", std::any::type_name::<T>(), self.len())
    }
}
