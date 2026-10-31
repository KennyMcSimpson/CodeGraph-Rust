//! Dense integer identifiers (design doc 5.2). All are `u32` with `NONE = u32::MAX`
//! meaning "absent", so they can live in `#[repr(C)]` records and be mmap'd directly.

use bytemuck::{Pod, Zeroable};

pub const NONE: u32 = 0xFFFF_FFFF;

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[repr(transparent)]
        #[derive(Pod, Zeroable, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub u32);
        impl $name {
            pub const NONE: $name = $name(NONE);
            #[inline] pub const fn new(i: usize) -> Self { $name(i as u32) }
            #[inline] pub const fn idx(self) -> usize { self.0 as usize }
            #[inline] pub const fn is_none(self) -> bool { self.0 == NONE }
            #[inline] pub const fn is_some(self) -> bool { self.0 != NONE }
            #[inline] pub const fn opt(self) -> Option<$name> { if self.0 == NONE { None } else { Some(self) } }
        }
        impl Default for $name { fn default() -> Self { $name::NONE } }
    };
}

id_type!(
    /// Index into the file table; files are sorted by path bytes.
    FileId
);
id_type!(
    /// Index into the node table, ordered by (FileId, extraction order).
    NodeIdx
);
id_type!(
    /// Index into a string pool.
    StrId
);
id_type!(
    /// Index of a `[StrId]` list in the shared list table.
    ListId
);
id_type!(
    /// Index into the scope table.
    ScopeId
);
