//! FNV-1a, 64 bit. Cache keys must be the same on every run, so the standard
//! library's randomly seeded hasher can't be used for them. It's also quicker
//! than SipHash for welding vertices.

use std::hash::Hasher;

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Copy)]
pub(crate) struct Fnv1a(u64);

impl Default for Fnv1a {
    fn default() -> Self {
        Self(OFFSET)
    }
}

impl Hasher for Fnv1a {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(PRIME);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = Fnv1a::default();
    h.write(bytes);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference_vectors() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x85944171f73967e8);
    }
}
