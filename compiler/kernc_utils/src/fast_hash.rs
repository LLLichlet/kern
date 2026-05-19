//! Fast non-cryptographic hash maps used on hot compiler paths.
//!
//! The hasher is intentionally deterministic and simple.  It is appropriate
//! for internal compiler tables where the keys are trusted compiler data, not
//! for adversarial input exposed as a network boundary.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x00000100000001B3;

#[derive(Debug, Default, Clone)]
pub struct FastHasher(u64);

impl FastHasher {
    #[inline]
    fn mix_bytes(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            // `BuildHasherDefault` constructs the hasher with zeroed state, so
            // initialize lazily to the standard FNV offset basis on first use.
            self.0 = FNV_OFFSET_BASIS;
        }

        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn finish(&self) -> u64 {
        if self.0 == 0 {
            FNV_OFFSET_BASIS
        } else {
            self.0
        }
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.mix_bytes(bytes);
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.mix_bytes(&[i]);
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.mix_bytes(&i.to_le_bytes());
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.mix_bytes(&i.to_le_bytes());
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.mix_bytes(&i.to_le_bytes());
    }

    #[inline]
    fn write_u128(&mut self, i: u128) {
        self.mix_bytes(&i.to_le_bytes());
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.mix_bytes(&i.to_le_bytes());
    }

    #[inline]
    fn write_i8(&mut self, i: i8) {
        self.mix_bytes(&[i as u8]);
    }

    #[inline]
    fn write_i16(&mut self, i: i16) {
        self.mix_bytes(&i.to_le_bytes());
    }

    #[inline]
    fn write_i32(&mut self, i: i32) {
        self.mix_bytes(&i.to_le_bytes());
    }

    #[inline]
    fn write_i64(&mut self, i: i64) {
        self.mix_bytes(&i.to_le_bytes());
    }

    #[inline]
    fn write_i128(&mut self, i: i128) {
        self.mix_bytes(&i.to_le_bytes());
    }

    #[inline]
    fn write_isize(&mut self, i: isize) {
        self.mix_bytes(&i.to_le_bytes());
    }
}

pub type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;
pub type FastHashSet<T> = HashSet<T, BuildHasherDefault<FastHasher>>;
