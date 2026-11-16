//! Box ids, kept from one run to the next, and the hash that makes them.
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};

/// A box's id from one run to the next: a hash of its place in the window's description, or of
/// its `.key(..)`.
pub type Id = u64;

/// Things kept by id.
pub(crate) type ById<V> = HashMap<Id, V, BuildHasherDefault<Plain>>;

/// A hash that is the same on every run, so ids stay put. A word at a time (rotate, xor,
/// multiply), stirred at the end so every output bit depends on every input bit.
struct Mix(u64);

impl Hasher for Mix {
    fn finish(&self) -> u64 {
        stir(self.0)
    }
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        words.iter().for_each(|word| self.write_u64(u64::from_le_bytes(*word)));
        // Leftover bytes make one more zero-padded word, built a byte at a time: a short copy
        // is a call in a size build.
        if !rest.is_empty() {
            self.write_u64(rest.iter().rev().fold(0, |word, byte| word << 8 | *byte as u64));
        }
    }
    fn write_u64(&mut self, word: u64) {
        self.0 = fold(self.0, word);
    }
    fn write_u8(&mut self, v: u8) {
        self.write_u64(v as u64);
    }
    fn write_u32(&mut self, v: u32) {
        self.write_u64(v as u64);
    }
    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
}

/// Folds one more word into a running hash.
#[inline]
pub(crate) fn fold(so_far: u64, word: u64) -> u64 {
    (so_far.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95)
}

/// Mixes a number with its kind, so mixed numbers of different kinds can be xor-ed together and
/// any one taken out or replaced.
#[inline(always)]
pub(crate) const fn mix(kind: u64, number: u64) -> u64 {
    let m = (number ^ kind.wrapping_mul(0x9e37_79b9_7f4a_7c15)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    (m ^ m >> 32).wrapping_mul(0x94d0_49bb_1331_11eb)
}

/// Finishes a `fold` hash: every bit depends on every input bit.
#[inline]
pub(crate) fn stir(folded: u64) -> u64 {
    let h = (folded ^ folded >> 33).wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^ h >> 33
}

pub(crate) fn hash(of: impl Hash) -> u64 {
    let mut mix = Mix(0xcbf2_9ce4_8422_2325);
    of.hash(&mut mix);
    mix.finish()
}

/// Hasher for tables keyed by id: an id is already well mixed, so it is used as is.
#[derive(Default)]
pub(crate) struct Plain(u64);

impl Hasher for Plain {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, _: &[u8]) {
        unreachable!("a table by id is only asked about ids");
    }
    fn write_u64(&mut self, id: u64) {
        self.0 = id;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes go in a word at a time, the last word zero-padded.
    #[test]
    fn bytes_are_taken_in_a_word_at_a_time() {
        let text = "a box is known by where it stands, or by its key".as_bytes();
        for end in 0..=text.len() {
            let mut copied = 0xcbf2_9ce4_8422_2325;
            for chunk in text[..end].chunks(8) {
                let mut word = [0; 8];
                word[..chunk.len()].copy_from_slice(chunk);
                copied = fold(copied, u64::from_le_bytes(word));
            }
            let mut mix = Mix(0xcbf2_9ce4_8422_2325);
            mix.write(&text[..end]);
            assert_eq!(mix.0, copied, "{end} bytes");
        }
    }
}
