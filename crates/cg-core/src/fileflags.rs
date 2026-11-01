//! Bits of `FileRec::flags` (design doc 5.3).

pub const PARSE_ERROR: u8 = 1;
pub const GENERATED: u8 = 2;
pub const TEST: u8 = 4;
pub const DEPRIORITIZED: u8 = 8;
pub const PANICKED: u8 = 16;
pub const TIMED_OUT: u8 = 32;
