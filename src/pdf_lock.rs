//! Encrypted PDF: a file that opens only with a password, or opens for anyone but lets only
//! its owner change it. PDF's strongest encryption (revision 6, as PDF 2.0 defines it, read by
//! every current viewer): AES-256, keys checked with SHA-256, -384 and -512, all written here.
//! Only a program that asks for it ([`Paper::encrypt`]) carries this.
//!
//! The file has a random 256-bit key. Each string and stream is encrypted with it in CBC mode,
//! after a random 16-byte start. The key itself is stored twice, encrypted with a hash of each
//! password; each hash takes a salt and some 64 rounds of AES-128 and SHA-2 (ISO 32000-2,
//! 7.6.4.3.4), so guessing passwords is slow. What a user may do is stored too, encrypted, so it
//! can't be changed unnoticed.
//!
//! Randomness comes from the standard library's hash keys, which the system seeds: each block is
//! a keyed hash of a count.
use crate::paper::Paper;
use crate::pdf::Lock;
use std::hash::{BuildHasher, Hasher};

impl Paper {
    /// Encrypts the file (AES-256). It opens with `password`, or for anyone where that is "".
    /// Then it may be printed, its forms filled and its text read aloud; only `owner` (its
    /// password) lets a viewer change or copy it too. Viewers keep to those limits; the
    /// encryption itself only keeps out those without a password. (PDF/A forbids encryption:
    /// [`archive`](Self::archive) undoes it.)
    pub fn encrypt(&mut self, password: &str, owner: &str) {
        // Printing (bit 3, and 12 for full quality), filling forms (9), and reading for
        // accessibility (10); the bits PDF reserves set.
        let allowed = (!0u32 << 12 | 0xC0) | 1 << 2 | 1 << 8 | 1 << 9 | 1 << 11;
        self.lock = Some(Lock { user: password.to_string(), owner: owner.to_string(), allowed, make });
        self.archive = None;
    }
}

/// Makes the file's key and the encryption dictionary for a lock.
fn make(lock: &Lock) -> ([u8; 32], crate::pdf::Seal, String) {
    let mut random = Random::new();
    let key: [u8; 32] = random.bytes();
    let (user, owner) = (password(&lock.user), password(&lock.owner));
    // The user's check: a hash of the password and a salt, then that salt, then the salt of the
    // key's own hash.
    let (check, salt): ([u8; 8], [u8; 8]) = (random.bytes(), random.bytes());
    let u = [&hash(user, &check, &[])[..], &check, &salt].concat();
    let ue = cbc_plain(&hash(user, &salt, &[]), &key);
    let (check, salt): ([u8; 8], [u8; 8]) = (random.bytes(), random.bytes());
    let o = [&hash(owner, &check, &u)[..], &check, &salt].concat();
    let oe = cbc_plain(&hash(owner, &salt, &u), &key);
    // What may be done, then "T" for metadata encrypted too, "adb", and four random bytes.
    let mut perms = [0u8; 16];
    perms[..4].copy_from_slice(&lock.allowed.to_le_bytes());
    perms[4..12].copy_from_slice(b"\xff\xff\xff\xffTadb");
    perms[12..].copy_from_slice(&random.bytes::<4>());
    let perms = Aes::new(&key).encrypt(perms);
    let hex = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let said = format!(
        "<< /Filter /Standard /V 5 /R 6 /Length 256 /CF << /StdCF << /AuthEvent /DocOpen /CFM /AESV3 /Length 32 >> >> /StmF /StdCF /StrF /StdCF /O <{}> /U <{}> /OE <{}> /UE <{}> /Perms <{}> /P {} /EncryptMetadata true >>",
        hex(&o),
        hex(&u),
        hex(&oe),
        hex(&ue),
        hex(&perms),
        lock.allowed as i32
    );
    (key, seal, said)
}

/// A password as the hash takes it: UTF-8, at most 127 bytes (cut at a character).
fn password(said: &str) -> &[u8] {
    let mut end = said.len().min(127);
    while !said.is_char_boundary(end) {
        end -= 1;
    }
    &said.as_bytes()[..end]
}

/// Encrypts a string's or stream's bytes with the file's key: a random start, then the bytes
/// padded to whole blocks (each pad byte the pad's length), in CBC mode.
fn seal(key: &[u8; 32], bytes: &[u8]) -> Vec<u8> {
    let start: [u8; 16] = Random::new().bytes();
    let pad = 16 - bytes.len() % 16;
    let padded: Vec<u8> = bytes.iter().copied().chain(std::iter::repeat_n(pad as u8, pad)).collect();
    [&start[..], &cbc(&Aes::new(key), start, &padded)].concat()
}

/// CBC mode over whole blocks: each block mixed with the last encrypted one (the first with
/// `start`), then encrypted.
fn cbc(aes: &Aes, start: [u8; 16], bytes: &[u8]) -> Vec<u8> {
    let (mut last, mut out) = (start, Vec::with_capacity(bytes.len()));
    for block in bytes.as_chunks::<16>().0 {
        let mixed: [u8; 16] = std::array::from_fn(|at| block[at] ^ last[at]);
        last = aes.encrypt(mixed);
        out.extend_from_slice(&last);
    }
    out
}

/// The file's key encrypted with a 256-bit key, CBC from zero, no padding: as the dictionary
/// stores it.
fn cbc_plain(key: &[u8; 32], bytes: &[u8; 32]) -> Vec<u8> {
    cbc(&Aes::new(key), [0; 16], bytes)
}

/// A password's hash, ISO 32000-2's algorithm 2.B: SHA-256 of the password, salt and `more`;
/// then rounds of: the password, hash and `more` 64 times over, encrypted by AES-128 (key and
/// start from the hash); the next hash SHA-256, -384 or -512 of that, chosen by its first 16
/// bytes. At least 64 rounds, until the last byte encrypted is at most the round count less 32.
fn hash(password: &[u8], salt: &[u8], more: &[u8]) -> [u8; 32] {
    let mut k = sha256(&[password, salt, more].concat()).to_vec();
    let mut round = 0;
    loop {
        round += 1;
        let once = [password, &k, more].concat();
        let all = once.repeat(64);
        let (key, start): ([u8; 16], [u8; 16]) = (std::array::from_fn(|at| k[at]), std::array::from_fn(|at| k[16 + at]));
        let e = cbc(&Aes::new128(&key), start, &all);
        k = match e[..16].iter().map(|byte| *byte as u32).sum::<u32>() % 3 {
            0 => sha256(&e).to_vec(),
            1 => sha512(&e, 384).to_vec(),
            _ => sha512(&e, 512).to_vec(),
        };
        if round >= 64 && e[e.len() - 1] as u32 <= round - 32 {
            break;
        }
    }
    std::array::from_fn(|at| k[at])
}

/// Random bytes: a hash keyed by the system's randomness (as the standard library seeds its
/// hash maps), of a count.
struct Random {
    keyed: std::collections::hash_map::RandomState,
    count: u64,
}

impl Random {
    fn new() -> Random {
        Random { keyed: std::collections::hash_map::RandomState::new(), count: 0 }
    }
    /// `N` random bytes.
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        for eight in out.chunks_mut(8) {
            let mut hasher = self.keyed.build_hasher();
            hasher.write_u64(self.count);
            self.count += 1;
            eight.copy_from_slice(&hasher.finish().to_le_bytes()[..eight.len()]);
        }
        out
    }
}

// ---- AES ----

/// AES (FIPS 197), encrypting only: its round keys, 11 for a 128-bit key, 15 for a 256-bit one.
struct Aes {
    keys: Vec<[u8; 16]>,
}

/// AES's S-box: each byte's inverse in GF(2⁸), transformed. Built once.
fn sbox() -> &'static [u8; 256] {
    static BOX: std::sync::OnceLock<[u8; 256]> = std::sync::OnceLock::new();
    BOX.get_or_init(|| {
        let mut table = [0u8; 256];
        for (byte, entry) in table.iter_mut().enumerate() {
            // (The inverse is the 254th power: no byte is its own problem.)
            let inverse = (0..254).fold(1u8, |power, _| times(power, byte as u8));
            let inverse = if byte == 0 { 0 } else { inverse };
            *entry = inverse ^ inverse.rotate_left(1) ^ inverse.rotate_left(2) ^ inverse.rotate_left(3) ^ inverse.rotate_left(4) ^ 0x63;
        }
        table
    })
}

/// Multiplication in GF(2⁸) modulo AES's polynomial.
fn times(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0;
    while b != 0 {
        if b & 1 != 0 {
            product ^= a;
        }
        a = (a << 1) ^ if a & 0x80 != 0 { 0x1b } else { 0 };
        b >>= 1;
    }
    product
}

impl Aes {
    /// With a 256-bit key.
    fn new(key: &[u8; 32]) -> Aes {
        Aes::expand(key)
    }
    /// With a 128-bit key.
    fn new128(key: &[u8; 16]) -> Aes {
        Aes::expand(key)
    }
    /// The round keys from a key of four or eight words.
    fn expand(key: &[u8]) -> Aes {
        let (s, n) = (sbox(), key.len() / 4);
        let rounds = n + 6;
        let mut words: Vec<[u8; 4]> = key.as_chunks::<4>().0.to_vec();
        let mut constant = 1u8;
        for at in n..4 * (rounds + 1) {
            let mut word = words[at - 1];
            if at % n == 0 {
                word = [s[word[1] as usize] ^ constant, s[word[2] as usize], s[word[3] as usize], s[word[0] as usize]];
                constant = times(constant, 2);
            } else if n > 6 && at % n == 4 {
                word = word.map(|byte| s[byte as usize]);
            }
            words.push(std::array::from_fn(|byte| word[byte] ^ words[at - n][byte]));
        }
        Aes { keys: words.as_chunks::<4>().0.iter().map(|four| std::array::from_fn(|at| four[at / 4][at % 4])).collect() }
    }
    /// Encrypts one block.
    fn encrypt(&self, block: [u8; 16]) -> [u8; 16] {
        let (s, last) = (sbox(), self.keys.len() - 1);
        let mut state: [u8; 16] = std::array::from_fn(|at| block[at] ^ self.keys[0][at]);
        for (round, key) in self.keys.iter().enumerate().skip(1) {
            // Each byte through the S-box, rows shifted left by their number (the state is
            // column by column), then each column mixed, except in the last round.
            let shifted: [u8; 16] = std::array::from_fn(|at| s[state[(at + 4 * (at % 4)) % 16] as usize]);
            state = if round == last { shifted } else { mixed(shifted) };
            state.iter_mut().zip(key).for_each(|(byte, key)| *byte ^= key);
        }
        state
    }
}

/// AES's column mixing.
fn mixed(state: [u8; 16]) -> [u8; 16] {
    std::array::from_fn(|at| {
        let (column, row) = (at / 4 * 4, at % 4);
        let byte = |n: usize| state[column + (row + n) % 4];
        times(byte(0), 2) ^ times(byte(1), 3) ^ byte(2) ^ byte(3)
    })
}

// ---- SHA-2 ----

/// SHA-256's round constants: the first 32 bits of the fractional parts of the cube roots of
/// the first 64 primes.
#[rustfmt::skip]
const ROUNDS_256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-512's round constants: 64 bits of the cube roots of the first 80 primes.
#[rustfmt::skip]
const ROUNDS_512: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc, 0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2, 0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65, 0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4, 0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df, 0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30, 0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8, 0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec, 0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178, 0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c, 0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

/// A message padded as SHA-2 pads it, to whole blocks of `block` bytes: a one bit, zeros, and
/// its length in bits in the last `long` bytes.
fn padded(bytes: &[u8], block: usize, long: usize) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out.push(0x80);
    out.resize((out.len() + long).next_multiple_of(block) - long, 0);
    let bits = (bytes.len() as u128 * 8).to_be_bytes();
    out.extend_from_slice(&bits[16 - long..]);
    out
}

/// SHA-256 (FIPS 180-4).
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    for block in padded(bytes, 64, 8).as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for at in 0..64 {
            let (a, b) = (w[at.max(15) - 15], w[at.max(2) - 2]);
            let s0 = a.rotate_right(7) ^ a.rotate_right(18) ^ a >> 3;
            let s1 = b.rotate_right(17) ^ b.rotate_right(19) ^ b >> 10;
            w[at] = if at < 16 { u32::from_be_bytes(std::array::from_fn(|n| block[at * 4 + n])) } else { w[at - 16].wrapping_add(s0).wrapping_add(w[at - 7]).wrapping_add(s1) };
        }
        let mut v = h;
        for at in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let choose = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let one = v[7].wrapping_add(s1).wrapping_add(choose).wrapping_add(ROUNDS_256[at]).wrapping_add(w[at]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let most = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let two = s0.wrapping_add(most);
            v = [one.wrapping_add(two), v[0], v[1], v[2], v[3].wrapping_add(one), v[4], v[5], v[6]];
        }
        h.iter_mut().zip(v).for_each(|(h, v)| *h = h.wrapping_add(v));
    }
    std::array::from_fn(|at| h[at / 4].to_be_bytes()[at % 4])
}

/// SHA-512, or SHA-384 (its own start, the hash cut to 48 bytes) where `bits` is 384.
pub(crate) fn sha512(bytes: &[u8], bits: usize) -> Vec<u8> {
    let mut h: [u64; 8] = match bits {
        384 => [0xcbbb9d5dc1059ed8, 0x629a292a367cd507, 0x9159015a3070dd17, 0x152fecd8f70e5939, 0x67332667ffc00b31, 0x8eb44a8768581511, 0xdb0c2e0d64f98fa7, 0x47b5481dbefa4fa4],
        _ => [0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1, 0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179],
    };
    for block in padded(bytes, 128, 16).as_chunks::<128>().0 {
        let mut w = [0u64; 80];
        for at in 0..80 {
            let (a, b) = (w[at.max(15) - 15], w[at.max(2) - 2]);
            let s0 = a.rotate_right(1) ^ a.rotate_right(8) ^ a >> 7;
            let s1 = b.rotate_right(19) ^ b.rotate_right(61) ^ b >> 6;
            w[at] = if at < 16 { u64::from_be_bytes(std::array::from_fn(|n| block[at * 8 + n])) } else { w[at - 16].wrapping_add(s0).wrapping_add(w[at - 7]).wrapping_add(s1) };
        }
        let mut v = h;
        for at in 0..80 {
            let s1 = v[4].rotate_right(14) ^ v[4].rotate_right(18) ^ v[4].rotate_right(41);
            let choose = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let one = v[7].wrapping_add(s1).wrapping_add(choose).wrapping_add(ROUNDS_512[at]).wrapping_add(w[at]);
            let s0 = v[0].rotate_right(28) ^ v[0].rotate_right(34) ^ v[0].rotate_right(39);
            let most = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let two = s0.wrapping_add(most);
            v = [one.wrapping_add(two), v[0], v[1], v[2], v[3].wrapping_add(one), v[4], v[5], v[6]];
        }
        h.iter_mut().zip(v).for_each(|(h, v)| *h = h.wrapping_add(v));
    }
    h.iter().flat_map(|word| word.to_be_bytes()).take(bits / 8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn sha2_gives_the_standards_own_answers() {
        // FIPS 180-4's examples: "abc", and a two-block message.
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha512(b"abc", 512)), "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f");
        assert_eq!(hex(&sha512(b"abc", 384)), "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7");
        // (A message filling its block exactly needs another for its length.)
        assert_eq!(hex(&sha256(&[b'a'; 64])), "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb");
    }

    #[test]
    fn aes_gives_the_standards_own_answers() {
        // FIPS 197, appendix C.1 and C.3.
        let block: [u8; 16] = std::array::from_fn(|at| (at as u8) * 0x11);
        let key128: [u8; 16] = std::array::from_fn(|at| at as u8);
        assert_eq!(hex(&Aes::new128(&key128).encrypt(block)), "69c4e0d86a7b0430d8cdb78070b4c55a");
        let key256: [u8; 32] = std::array::from_fn(|at| at as u8);
        assert_eq!(hex(&Aes::new(&key256).encrypt(block)), "8ea2b7ca516745bfeafc49904b496089");
    }

    #[test]
    fn what_is_sealed_is_random_whole_blocks() {
        let key = [7u8; 32];
        let (one, two) = (seal(&key, b"sixteen bytes!!!"), seal(&key, b"sixteen bytes!!!"));
        assert_eq!(one.len(), 16 + 32, "a start, and a whole block of padding after a whole block");
        assert_ne!(one, two, "each starts anew");
        assert_eq!(seal(&key, b"").len(), 32);
    }
}
