//! Content hashing: xxh3 (design doc 4.5). Not cryptographic; collisions only cost a
//! missed change, and the (mtime, size) pre-check makes that vanishingly rare.

use xxhash_rust::xxh3::{xxh3_128, xxh3_64};

#[inline]
pub fn hash128(bytes: &[u8]) -> [u8; 16] {
    xxh3_128(bytes).to_le_bytes()
}

#[inline]
pub fn hash64(bytes: &[u8]) -> u64 {
    xxh3_64(bytes)
}

/// Hex form used in debug output.
pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 15) as usize] as char);
    }
    s
}
