//! Brotli decompression (RFC 7932): WOFF2 fonts (see `woff2.rs`) keep all their tables in one
//! Brotli stream.
//!
//! A stream is a sequence of meta-blocks. Each is stored uncompressed, or brings its own prefix
//! codes and is a sequence of commands: insert so many literals, then copy so many bytes from
//! so far back. The codes are deflate-like (shorter bit strings for commoner symbols), but more
//! of them: a literal's code depends on the two previous bytes, a distance's on the copy
//! length, and a meta-block switches between code sets as it goes. A distance can also reach
//! past the start, into a built-in dictionary of 13,504 words, each available in 121
//! transforms: as is, with a space after it, capitalized, and so on.
//!
//! Every count and offset is checked, and work is bounded: output never exceeds the length the
//! caller expects, and code reading (and commands writing nothing) never costs more than a few
//! thousand steps per input byte: a damaged 1 MB stream is rejected within seconds, where a
//! real one decodes at 100 MB/s or more. The dictionary is stored deflate-compressed (about 59
//! KB of the program, half its size) and inflated per stream, costing about as much as decoding
//! a few-hundred-KB font.
use crate::png::inflate;

/// The code-reading budget, besides a per-byte allowance: enough for a few large meta-blocks'
/// codes. (A code has up to 704 symbols and a lookup table of hundreds, and a meta-block may
/// bring hundreds of codes. A stream flushed every few bytes brings codes that often: a real
/// one used 530 per byte.)
const WORK: usize = 1 << 20;
/// The extra budget per input byte: a code takes at least a byte or two.
const WORK_PER_BYTE: usize = 2048;

/// Decompresses a stream of `long` bytes: those bytes, or `None` if damaged, of another length,
/// or over budget.
pub(crate) fn unpacked(packed: &[u8], long: usize) -> Option<Vec<u8>> {
    let words = inflate(&PACKED, WORDS)?;
    let mut stream = Stream {
        bits: Bits { data: packed, at: 0, held: 0, count: 0 },
        out: vec![0; long],
        at: 0,
        window: 0,
        last: [4, 11, 15, 16],
        work: WORK.saturating_add(packed.len().saturating_mul(WORK_PER_BYTE)),
    };
    stream.window = stream.window_size()?;
    while stream.block(&words)? {}
    // The last byte's remaining bits are zeros; anything after it is the caller's.
    let rest = (8 - stream.bits.taken() % 8) % 8;
    stream.bits.fill();
    (stream.at == long && stream.bits.take(rest as u32) == 0 && stream.bits.whole()).then_some(stream.out)
}

/// A stream's bits, from each byte's low bit up.
struct Bits<'a> {
    data: &'a [u8],
    /// The next byte to load, how many bits are buffered, and the bits.
    at: usize,
    count: u32,
    held: u64,
}

impl Bits<'_> {
    /// Loads bytes until at least 48 bits are buffered: past the data's end, zeros (`whole`
    /// tells those from real bits).
    fn fill(&mut self) {
        // No read takes more than 48 bits after a fill.
        if self.count >= 48 {
            return;
        }
        // Eight bytes at a time, of which as many are kept as fit: the bits of the next, above
        // them, are loaded again by the next fill, identically.
        if let Some(eight) = self.data.get(self.at..).and_then(|rest| rest.first_chunk::<8>()) {
            self.held |= u64::from_le_bytes(*eight) << self.count;
            (self.at, self.count) = (self.at + ((63 - self.count) >> 3) as usize, self.count | 56);
            return;
        }
        while self.count <= 56 {
            self.held |= (self.data.get(self.at).copied().unwrap_or(0) as u64) << self.count;
            (self.at, self.count) = (self.at + 1, self.count + 8);
        }
    }

    /// The next `n` bits (at most those buffered) as a number.
    fn take(&mut self, n: u32) -> u32 {
        let value = (self.held & ((1 << n) - 1)) as u32;
        (self.held, self.count) = (self.held >> n, self.count - n);
        value
    }

    /// The next `n` bits, up to 32.
    fn read(&mut self, n: u32) -> u32 {
        self.fill();
        self.take(n)
    }

    /// Bits consumed so far.
    fn taken(&self) -> usize {
        self.at * 8 - self.count as usize
    }

    /// Whether no more bits were consumed than the data has.
    fn whole(&self) -> bool {
        self.taken() <= self.data.len() * 8
    }

    /// Skips to the next byte boundary: the skipped bits must be zero. Returns that byte's
    /// offset.
    fn at_byte(&mut self) -> Option<usize> {
        let rest = (8 - self.taken() % 8) % 8;
        (self.read(rest as u32) == 0).then_some(())?;
        let at = self.taken() / 8;
        (self.at, self.count, self.held) = (at, 0, 0);
        (at <= self.data.len()).then_some(at)
    }
}

/// A prefix code as a lookup table: by the next eight bits, and for codes longer than eight, by
/// the following bits in a second-level table further on. Each entry holds a symbol (above 16
/// bits) and its length; or, with 0x10 set, the second-level table's offset and its extra bit
/// count.
struct Code {
    table: Vec<u32>,
}

impl Code {
    /// The code of one symbol, which takes no bits.
    fn single(symbol: usize) -> Code {
        Code { table: vec![(symbol as u32) << 16; 256] }
    }

    /// The next symbol (at least 15 bits must be buffered).
    fn read(&self, bits: &mut Bits) -> usize {
        let entry = self.table[(bits.held & 255) as usize];
        if entry & 0x10 == 0 {
            bits.take(entry & 15);
            return (entry >> 16) as usize;
        }
        let next = self.table[(entry >> 16) as usize + (bits.held >> 8 & ((1 << (entry & 15)) - 1)) as usize];
        bits.take(8 + (next & 15));
        (next >> 16) as usize
    }
}

/// The canonical code whose symbol lengths are `lengths` (0 for unused), shorter codes first,
/// same-length ones by symbol. The lengths must fill the code exactly: no bit string may be
/// left unassigned.
fn code(lengths: &[u8]) -> Option<Code> {
    let mut counts = [0usize; 16];
    lengths.iter().for_each(|length| counts[*length as usize] += 1);
    let filled: usize = (1..16).map(|length| counts[length] << (15 - length)).sum();
    (filled == 1 << 15 && lengths.iter().all(|length| *length < 16)).then_some(())?;
    // Symbols in code order, each with its length and code.
    let mut starts = [0usize; 16];
    for length in 1..15 {
        starts[length + 1] = starts[length] + counts[length];
    }
    let mut sorted = vec![(0u32, 0u32, 0u32); lengths.len() - counts[0]];
    for (symbol, length) in lengths.iter().enumerate().filter(|(_, length)| **length != 0) {
        sorted[starts[*length as usize]] = (symbol as u32, *length as u32, 0);
        starts[*length as usize] += 1;
    }
    let mut next = 0u32;
    for n in 0..sorted.len() {
        let grown = if n == 0 { 0 } else { sorted[n].1 - sorted[n - 1].1 };
        next <<= grown;
        sorted[n].2 = next;
        next += 1;
    }
    let mut table = vec![0u32; 256];
    let mut n = 0;
    while n < sorted.len() {
        n = fill(&mut table, &sorted, n);
    }
    Some(Code { table })
}

/// A code's bits in stream order: reversed.
fn turned(code: u32, length: u32) -> usize {
    (code.reverse_bits() >> (32 - length.max(1)) & ((1 << length) - 1)) as usize
}

/// Puts code `n`'s symbol in its table, with the following symbols sharing its second-level
/// table; returns the next symbol to place.
fn fill(table: &mut Vec<u32>, sorted: &[(u32, u32, u32)], n: usize) -> usize {
    let (symbol, length, code) = sorted[n];
    if length <= 8 {
        for at in (turned(code, length)..256).step_by(1 << length) {
            table[at] = symbol << 16 | length;
        }
        return n + 1;
    }
    // Codes sharing their first eight bits are consecutive, the longest last.
    let first = code >> (length - 8);
    let end = n + sorted[n..].iter().take_while(|(_, length, code)| code >> (length - 8) == first).count();
    let more = sorted[end - 1].1 - 8;
    let (start, size) = (table.len(), 1usize << more);
    table[turned(first, 8)] = (start as u32) << 16 | 0x10 | more;
    table.resize(start + size, 0);
    for &(symbol, length, code) in &sorted[n..end] {
        for at in (turned(code & ((1 << (length - 8)) - 1), length - 8)..size).step_by(1 << (length - 8)) {
            table[start + at] = symbol << 16 | (length - 8);
        }
    }
    end
}

/// The order of the code length code's lengths.
const ORDER: [usize; 18] = [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];
/// The code length code's lengths are coded with a fixed 2- to 4-bit code: by the next four
/// bits, its length and the value.
const SHORT: [(u32, u8); 16] = [(2, 0), (2, 4), (2, 3), (3, 2), (2, 0), (2, 4), (2, 3), (4, 1), (2, 0), (2, 4), (2, 3), (3, 2), (2, 0), (2, 4), (2, 3), (4, 5)];

/// Insert and copy length codes: base value and extra bit count.
const INSERTS: [(u32, u32); 24] = [
    (0, 0),
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 1),
    (8, 1),
    (10, 2),
    (14, 2),
    (18, 3),
    (26, 3),
    (34, 4),
    (50, 4),
    (66, 5),
    (98, 5),
    (130, 6),
    (194, 7),
    (322, 8),
    (578, 9),
    (1090, 10),
    (2114, 12),
    (6210, 14),
    (22594, 24),
];
const COPIES: [(u32, u32); 24] = [
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (8, 0),
    (9, 0),
    (10, 1),
    (12, 1),
    (14, 2),
    (18, 2),
    (22, 3),
    (30, 3),
    (38, 4),
    (54, 4),
    (70, 5),
    (102, 5),
    (134, 6),
    (198, 7),
    (326, 8),
    (582, 9),
    (1094, 10),
    (2118, 24),
];
/// Block length codes: base value and extra bits.
const LENGTHS: [(u32, u32); 26] = [
    (1, 2),
    (5, 2),
    (9, 2),
    (13, 2),
    (17, 3),
    (25, 3),
    (33, 3),
    (41, 3),
    (49, 4),
    (65, 4),
    (81, 4),
    (97, 4),
    (113, 5),
    (145, 5),
    (177, 5),
    (209, 5),
    (241, 6),
    (305, 6),
    (369, 7),
    (497, 8),
    (753, 9),
    (1265, 10),
    (2289, 11),
    (4337, 12),
    (8433, 13),
    (16625, 24),
];
/// Of the 704 insert-and-copy symbols, in eleven runs of 64, which insert and copy length codes
/// each run starts at. (The first two runs reuse the last distance.)
const RUNS: [(usize, usize); 11] = [(0, 0), (0, 8), (0, 0), (0, 8), (8, 0), (8, 8), (0, 16), (16, 0), (8, 16), (16, 8), (16, 16)];

/// A meta-block's block types for one category (literals, commands, distances): the type count,
/// the type and length codes, the current and previous type, and how many items the current
/// block has left.
struct Types {
    count: usize,
    codes: Option<(Code, Code)>,
    now: usize,
    before: usize,
    left: usize,
}

/// A meta-block's codes and context maps.
struct Codes {
    bytes: Types,
    commands: Types,
    distances: Types,
    /// Per literal type, the context mode (how the two previous bytes choose a code).
    modes: Vec<u8>,
    /// The literal context map (64 contexts per type) and the distance context map (four per
    /// type).
    byte_map: Vec<u8>,
    distance_map: Vec<u8>,
    byte_codes: Vec<Code>,
    command_codes: Vec<Code>,
    distance_codes: Vec<Code>,
    /// Distance coding: postfix bits, and the number of direct distance codes.
    postfix: u32,
    direct: u32,
}

/// A stream being decompressed.
struct Stream<'a> {
    bits: Bits<'a>,
    /// The output, and how far it got.
    out: Vec<u8>,
    at: usize,
    /// How far back a copy may reach.
    window: usize,
    /// The last four distances, most recent first.
    last: [usize; 4],
    /// The code-reading budget left.
    work: usize,
}

impl Stream<'_> {
    /// Spends `work` from the budget, if there is that much.
    fn spend(&mut self, work: usize) -> Option<()> {
        self.work = self.work.checked_sub(work)?;
        Some(())
    }

    /// The window size, from the stream header.
    fn window_size(&mut self) -> Option<usize> {
        if self.bits.read(1) == 0 {
            return Some((1 << 16) - 16);
        }
        // Seven bits meaning 9 signal large-window Brotli, not read here.
        let bits = match self.bits.read(3) {
            0 => [17, 0, 10, 11, 12, 13, 14, 15][self.bits.read(3) as usize],
            n => 17 + n,
        };
        (bits != 0).then(|| (1 << bits) - 16)
    }

    /// Decodes the next meta-block; returns whether another follows.
    fn block(&mut self, words: &[u8]) -> Option<bool> {
        self.bits.whole().then_some(())?;
        let last = self.bits.read(1) == 1;
        if last && self.bits.read(1) == 1 {
            return Some(false);
        }
        let nibbles = [4, 5, 6, 0][self.bits.read(2) as usize];
        if nibbles == 0 {
            self.skip_notes()?;
            return Some(!last);
        }
        let long = self.bits.read(nibbles * 4) as usize + 1;
        if nibbles > 4 && (long - 1) >> (nibbles * 4 - 4) == 0 {
            return None;
        }
        let end = self.at.checked_add(long).filter(|end| *end <= self.out.len())?;
        if !last && self.bits.read(1) == 1 {
            let from = self.bits.at_byte()?;
            self.out[self.at..end].copy_from_slice(self.bits.data.get(from..from + long)?);
            (self.bits.at, self.at) = (from + long, end);
            return Some(true);
        }
        let mut codes = self.codes()?;
        self.commands(&mut codes, end, words)?;
        Some(!last)
    }

    /// Skips a metadata block, which decodes to nothing.
    fn skip_notes(&mut self) -> Option<()> {
        (self.bits.read(1) == 0).then_some(())?;
        let bytes = self.bits.read(2);
        let long = if bytes == 0 { 0 } else { self.bits.read(bytes * 8) as usize + 1 };
        if bytes > 1 && (long - 1) >> (bytes * 8 - 8) == 0 {
            return None;
        }
        let from = self.bits.at_byte()?;
        self.bits.at = from.checked_add(long).filter(|end| *end <= self.bits.data.len())?;
        Some(())
    }

    /// A type or tree count: 1, or up to 256, in a short code of up to 11 bits.
    fn count(&mut self) -> usize {
        if self.bits.read(1) == 0 {
            return 1;
        }
        match self.bits.read(3) {
            0 => 2,
            n => (1 << n) + self.bits.read(n) as usize + 1,
        }
    }

    /// One category's block types in a meta-block, with the codes switching between them.
    fn types(&mut self) -> Option<Types> {
        let count = self.count();
        if count == 1 {
            return Some(Types { count, codes: None, now: 0, before: 1, left: usize::MAX });
        }
        let (kinds, lengths) = (self.prefix_code(count + 2)?, self.prefix_code(26)?);
        let left = self.block_length(&lengths);
        Some(Types { count, codes: Some((kinds, lengths)), now: 0, before: 1, left })
    }

    /// How many items the next block of a category lasts.
    fn block_length(&mut self, code: &Code) -> usize {
        self.bits.fill();
        let (first, more) = LENGTHS[code.read(&mut self.bits)];
        (first + self.bits.read(more)) as usize
    }

    /// Switches to the next block type of a category, when the current one runs out.
    fn switch(&mut self, types: &mut Types) {
        let Some((kinds, lengths)) = &types.codes else { return };
        self.bits.fill();
        let next = match kinds.read(&mut self.bits) {
            0 => types.before,
            1 => types.now + 1,
            symbol => symbol - 2,
        };
        let next = if next >= types.count { next - types.count } else { next };
        (types.before, types.now) = (types.now, next);
        types.left = self.block_length(lengths);
    }

    /// A meta-block's codes and context maps.
    fn codes(&mut self) -> Option<Codes> {
        let (bytes, commands, distances) = (self.types()?, self.types()?, self.types()?);
        let postfix = self.bits.read(2);
        let direct = self.bits.read(4) << postfix;
        let modes: Vec<u8> = (0..bytes.count).map(|_| self.bits.read(2) as u8).collect();
        let byte_trees = self.count();
        let byte_map = self.context_map(byte_trees, bytes.count * 64)?;
        let distance_trees = self.count();
        let distance_map = self.context_map(distance_trees, distances.count * 4)?;
        let byte_codes = (0..byte_trees).map(|_| self.prefix_code(256)).collect::<Option<Vec<_>>>()?;
        let command_codes = (0..commands.count).map(|_| self.prefix_code(704)).collect::<Option<Vec<_>>>()?;
        let distance_size = 16 + direct as usize + (48 << postfix);
        let distance_codes = (0..distance_trees).map(|_| self.prefix_code(distance_size)).collect::<Option<Vec<_>>>()?;
        Some(Codes { bytes, commands, distances, modes, byte_map, distance_map, byte_codes, command_codes, distance_codes, postfix, direct })
    }

    /// A context map: which of `trees` codes each of its `size` entries uses. Zero runs may be
    /// run-length coded, and entries move-to-front coded.
    fn context_map(&mut self, trees: usize, size: usize) -> Option<Vec<u8>> {
        let mut map = vec![0u8; size];
        if trees < 2 {
            return Some(map);
        }
        self.spend(size)?;
        let most = if self.bits.read(1) == 1 { self.bits.read(4) as usize + 1 } else { 0 };
        let code = self.prefix_code(trees + most)?;
        let mut at = 0;
        while at < size {
            self.bits.fill();
            match code.read(&mut self.bits) {
                0 => at += 1,
                run if run <= most => at += (1 << run) + self.bits.take(run as u32) as usize,
                tree => (map[at], at) = ((tree - most) as u8, at + 1),
            }
        }
        (at == size).then_some(())?;
        if self.bits.read(1) == 1 {
            to_front(&mut map);
        }
        Some(map)
    }

    /// Reads a prefix code over `size` symbols.
    fn prefix_code(&mut self, size: usize) -> Option<Code> {
        self.spend(size + 1024)?;
        match self.bits.read(2) {
            1 => self.simple_code(size),
            skip => self.complex_code(size, skip as usize),
        }
    }

    /// A simple code of one to four literal symbols.
    fn simple_code(&mut self, size: usize) -> Option<Code> {
        let width = usize::BITS - (size - 1).leading_zeros();
        let count = self.bits.read(2) as usize + 1;
        let symbols: Vec<usize> = (0..count).map(|_| self.bits.read(width) as usize).collect();
        let doubled = (1..count).any(|n| symbols[..n].contains(&symbols[n]));
        (symbols.iter().all(|symbol| *symbol < size) && !doubled).then_some(())?;
        let lengths: [u8; 4] = match count {
            1 => return Some(Code::single(symbols[0])),
            2 => [1, 1, 0, 0],
            3 => [1, 2, 2, 0],
            _ if self.bits.read(1) == 1 => [1, 2, 3, 3],
            _ => [2, 2, 2, 2],
        };
        let mut all = vec![0u8; size];
        symbols.iter().zip(lengths).for_each(|(symbol, length)| all[*symbol] = length);
        code(&all)
    }

    /// A complex code: each symbol's length, coded with a code length code (lengths 0 to 15, or
    /// "repeat the last" or "zeros" so many times), whose own lengths come first (except the
    /// `skip` first ones, which are zero).
    fn complex_code(&mut self, size: usize, skip: usize) -> Option<Code> {
        let (mut first, mut room, mut used) = ([0u8; 18], 32i32, 0);
        for at in &ORDER[skip..] {
            self.bits.fill();
            let (bits, length) = SHORT[(self.bits.held & 15) as usize];
            self.bits.take(bits);
            first[*at] = length;
            if length != 0 {
                (room, used) = (room - (32 >> length), used + 1);
            }
            if room <= 0 {
                break;
            }
        }
        let first = match used {
            1 => Code::single(first.iter().position(|length| *length != 0)?),
            _ if room == 0 => code(&first)?,
            _ => return None,
        };
        let mut lengths = vec![0u8; size];
        let (mut symbol, mut room) = (0, 32768i32);
        // The last nonzero length, and the current repeat run of "repeat" or "zeros".
        let (mut previous, mut repeat, mut repeated) = (8u8, 0usize, 0u8);
        while symbol < size && room > 0 {
            self.bits.fill();
            let length = first.read(&mut self.bits) as u8;
            if length < 16 {
                (lengths[symbol], symbol, repeat) = (length, symbol + 1, 0);
                (previous, room) = if length == 0 { (previous, room) } else { (length, room - (32768 >> length)) };
                continue;
            }
            let (extra, again) = if length == 16 { (2, previous) } else { (3, 0) };
            if repeated != again {
                (repeat, repeated) = (0, again);
            }
            let before = repeat;
            repeat = (if repeat > 0 { (repeat - 2) << extra } else { 0 }) + self.bits.take(extra) as usize + 3;
            let more = repeat - before;
            lengths.get_mut(symbol..symbol + more)?.fill(again);
            room -= if again == 0 { 0 } else { (more as i32) << (15 - again) };
            symbol += more;
        }
        (room == 0).then_some(())?;
        code(&lengths)
    }

    /// Runs a meta-block's commands until it has decoded to `end`.
    fn commands(&mut self, codes: &mut Codes, end: usize, words: &[u8]) -> Option<()> {
        while self.at < end {
            // A truncated stream reads zeros, which come to nothing long before this. A command
            // may write nothing at all (a dictionary word with all letters cut off): each costs
            // a little budget, so no stream loops for ever.
            (self.bits.at <= self.bits.data.len() + 8).then_some(())?;
            self.spend(1)?;
            if codes.commands.left == 0 {
                self.switch(&mut codes.commands);
            }
            codes.commands.left -= 1;
            self.bits.fill();
            let command = codes.command_codes[codes.commands.now].read(&mut self.bits);
            let (inserts, copies) = RUNS[command >> 6];
            let (insert, copy) = (INSERTS[inserts + ((command >> 3) & 7)], COPIES[copies + (command & 7)]);
            self.bits.fill();
            let insert = (insert.0 + self.bits.take(insert.1)) as usize;
            let copy = (copy.0 + self.bits.take(copy.1)) as usize;
            (insert <= end - self.at).then_some(())?;
            self.bytes(codes, insert);
            if self.at == end {
                break;
            }
            let distance = if command < 128 { Some(self.last[0]) } else { self.distance(codes, copy) };
            self.copy(distance?, copy, end, words)?;
        }
        Some(())
    }

    /// Writes `count` literals, each with the code its type and the two previous bytes choose.
    fn bytes(&mut self, codes: &mut Codes, count: usize) {
        let before = |back: usize| self.at.checked_sub(back).map_or(0, |at| self.out[at] as usize);
        let (mut one, mut two) = (before(1), before(2));
        for _ in 0..count {
            if codes.bytes.left == 0 {
                self.switch(&mut codes.bytes);
            }
            codes.bytes.left -= 1;
            let context = match codes.modes[codes.bytes.now] {
                0 => one & 63,
                1 => one >> 2,
                2 => (LETTERS[one] | LETTERS[256 + two]) as usize,
                _ => signed(one) << 3 | signed(two),
            };
            self.bits.fill();
            let code = &codes.byte_codes[codes.byte_map[codes.bytes.now * 64 + context] as usize];
            (two, one) = (one, code.read(&mut self.bits));
            self.out[self.at] = one as u8;
            self.at += 1;
        }
    }

    /// A command's explicit distance: one of the last four (as is, or slightly adjusted), a
    /// direct distance code, or one coded in extra bits. `None` if zero or negative.
    fn distance(&mut self, codes: &mut Codes, copy: usize) -> Option<usize> {
        if codes.distances.left == 0 {
            self.switch(&mut codes.distances);
        }
        codes.distances.left -= 1;
        self.bits.fill();
        let tree = codes.distance_map[codes.distances.now * 4 + copy.min(5) - 2] as usize;
        let symbol = codes.distance_codes[tree].read(&mut self.bits);
        let (postfix, direct) = (codes.postfix, codes.direct as usize);
        let distance = match symbol {
            0..16 => {
                let changes = [0, 0, 0, 0, -1, 1, -2, 2, -3, 3, -1, 1, -2, 2, -3, 3];
                let last = self.last[[0, 1, 2, 3, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1][symbol]];
                last.checked_add_signed(changes[symbol]).filter(|distance| *distance > 0)?
            }
            _ if symbol < 16 + direct => symbol - 15,
            _ => {
                let code = (symbol - direct - 16) as u32;
                let extra = 1 + (code >> (postfix + 1));
                let offset = ((2 + ((code >> postfix) & 1)) << extra) - 4;
                (((offset + self.bits.read(extra)) as usize) << postfix) + (code & ((1 << postfix) - 1)) as usize + direct + 1
            }
        };
        // A repeated distance, or a dictionary reference, isn't pushed onto the last distances.
        if symbol != 0 && distance <= self.window.min(self.at) {
            self.last = [distance, self.last[0], self.last[1], self.last[2]];
        }
        Some(distance)
    }

    /// Copies `copy` bytes from `distance` back, or, beyond the window, a dictionary word in
    /// one of its transforms.
    fn copy(&mut self, distance: usize, copy: usize, end: usize, words: &[u8]) -> Option<()> {
        let reach = self.window.min(self.at);
        if distance > reach {
            let word = word(words, copy, distance - reach - 1)?;
            let to = self.at.checked_add(word.len()).filter(|to| *to <= end)?;
            self.out[self.at..to].copy_from_slice(&word);
            self.at = to;
            return Some(());
        }
        (copy <= end - self.at).then_some(())?;
        // A distance shorter than the copy overlaps what is being written: the bytes between
        // repeat. Each pass copies everything so far, from its start.
        let (from, mut done) = (self.at - distance, 0);
        while done < copy {
            let long = (copy - done).min(distance + done);
            self.out.copy_within(from..from + long, self.at + done);
            done += long;
        }
        self.at += copy;
        Some(())
    }
}

/// Undoes a context map's move-to-front coding.
fn to_front(map: &mut [u8]) {
    let mut list: [u8; 256] = std::array::from_fn(|n| n as u8);
    for value in map.iter_mut() {
        let at = *value as usize;
        let named = list[at];
        list.copy_within(0..at, 1);
        (list[0], *value) = (named, named);
    }
}

/// A signed-mode context from one previous byte: which of eight value ranges it is in.
fn signed(byte: usize) -> usize {
    [1, 16, 64, 128, 192, 240, 255].iter().filter(|start| byte >= **start).count()
}

/// UTF-8-mode literal contexts, by the previous byte (the first 256) and the one before (the
/// second): space, punctuation, digit, upper or lower case letter, or a multibyte character's
/// byte.
#[rustfmt::skip]
static LETTERS: [u8; 512] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    8, 12, 16, 12, 12, 20, 12, 16, 24, 28, 12, 12, 32, 12, 36, 12, 44, 44, 44, 44, 44, 44, 44, 44, 44, 44, 32, 32, 24, 40, 28, 12,
    12, 48, 52, 52, 52, 48, 52, 52, 52, 48, 52, 52, 52, 52, 52, 48, 52, 52, 52, 52, 52, 48, 52, 52, 52, 52, 52, 24, 12, 28, 12, 12,
    12, 56, 60, 60, 60, 56, 60, 60, 60, 56, 60, 60, 60, 60, 60, 56, 60, 60, 60, 60, 60, 56, 60, 60, 60, 60, 60, 24, 12, 28, 12, 0,
    0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1,
    0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1,
    2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3,
    2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1,
    1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1,
    1, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 1, 1, 1, 1, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
];

/// Each word length's word count, as a power of two (none shorter than 4).
const BITS: [u32; 25] = [0, 0, 0, 0, 10, 10, 11, 11, 10, 10, 10, 10, 10, 9, 9, 8, 7, 7, 8, 7, 7, 6, 6, 5, 5];
/// The decompressed dictionary's length.
const WORDS: usize = 122_784;

/// Dictionary word `id` among those `long` bytes long, in the transform `id` selects. `None`
/// for a length without words, or an unknown transform.
fn word(words: &[u8], long: usize, id: usize) -> Option<Vec<u8>> {
    let bits = *BITS.get(long).filter(|bits| **bits != 0)?;
    let start: usize = (4..long).map(|shorter| shorter << BITS[shorter]).sum();
    let (index, form) = (id & ((1 << bits) - 1), id >> bits);
    let [before, change, after] = *FORMS.get(form)?;
    let mut word = words.get(start + index * long..start + (index + 1) * long)?.to_vec();
    match change {
        0 => {}
        1 => _ = capital(&mut word, 0),
        2 => {
            let mut at = 0;
            while at < word.len() {
                at += capital(&mut word, at);
            }
        }
        3..=11 => _ = word.drain(..(change as usize - 2).min(word.len())),
        _ => word.truncate(word.len().saturating_sub(change as usize - 11)),
    }
    Some([AFFIXES[before as usize], &word, AFFIXES[after as usize]].concat())
}

/// Uppercases the character at `at` as the format does (a two-byte character's second byte, a
/// three-byte one's third, one bit flipped): returns its byte length.
fn capital(word: &mut [u8], at: usize) -> usize {
    let (bytes, changed, bit) = match word[at] {
        0..192 => (1, 0, if word[at].is_ascii_lowercase() { 32 } else { 0 }),
        192..224 => (2, 1, 32),
        _ => (3, 2, 5),
    };
    if let Some(byte) = word.get_mut(at + changed) {
        *byte ^= bit;
    }
    bytes
}

/// The prefixes and suffixes of the transforms.
#[rustfmt::skip]
const AFFIXES: [&[u8]; 50] = [
    b"", b" ", b" the ", b"s ", b" of ", b" and ", b", ", b" in ", b" to ", b"e ",
    b"\"", b".", b"\">", b"\n", b"]", b" for ", b" a ", b" that ", b". ", b" with ",
    b"'", b" from ", b" by ", b". The ", b" on ", b" as ", b" is ", b"ing ", b"\n\t", b":",
    b"ed ", b"(", b" at ", b"ly ", b"=\"", b".com/", b" of the ", b". This ", b",", b" not ",
    b"er ", b"al ", b"='", b"ful ", b"ive ", b"less ", b"est ", b"ize ", b"\xc2\xa0", b"ous ",
];
/// The 121 transforms: a prefix (in `AFFIXES`), an operation (identity, uppercase first,
/// uppercase all, omit first 1 to 9 bytes, omit last 1 to 9), and a suffix.
#[rustfmt::skip]
const FORMS: [[u8; 3]; 121] = [
    [0, 0, 0], [0, 0, 1], [1, 0, 1], [0, 3, 0], [0, 1, 1], [0, 0, 2], [1, 0, 0], [3, 0, 1], [0, 0, 4], [0, 1, 0],
    [0, 0, 5], [0, 4, 0], [0, 12, 0], [6, 0, 1], [0, 0, 6], [1, 1, 1], [0, 0, 7], [0, 0, 8], [9, 0, 1], [0, 0, 10],
    [0, 0, 11], [0, 0, 12], [0, 0, 13], [0, 14, 0], [0, 0, 14], [0, 0, 15], [0, 5, 0], [0, 13, 0], [0, 0, 16], [0, 0, 17],
    [1, 1, 0], [0, 0, 18], [11, 0, 0], [1, 0, 6], [0, 6, 0], [0, 0, 19], [0, 0, 20], [0, 0, 21], [0, 0, 22], [0, 7, 0],
    [0, 8, 0], [2, 0, 0], [0, 15, 0], [0, 0, 23], [0, 2, 0], [0, 0, 24], [0, 0, 25], [0, 0, 26], [0, 18, 0], [0, 12, 27],
    [0, 0, 28], [0, 0, 29], [1, 0, 18], [0, 0, 30], [0, 11, 0], [0, 9, 0], [0, 17, 0], [0, 0, 31], [0, 1, 6], [0, 19, 0],
    [0, 0, 32], [0, 0, 33], [2, 0, 4], [0, 16, 0], [0, 20, 0], [1, 1, 6], [0, 1, 10], [11, 0, 31], [0, 2, 1], [0, 1, 12],
    [0, 0, 34], [1, 0, 11], [35, 0, 0], [2, 0, 36], [0, 1, 20], [0, 0, 37], [0, 0, 38], [11, 0, 1], [0, 1, 31], [0, 1, 11],
    [0, 0, 39], [1, 0, 34], [0, 0, 40], [1, 2, 1], [0, 0, 41], [1, 2, 0], [0, 0, 42], [0, 2, 10], [0, 1, 18], [1, 0, 31],
    [0, 0, 43], [1, 1, 18], [0, 0, 44], [0, 0, 45], [0, 2, 20], [0, 0, 46], [1, 1, 11], [0, 2, 12], [1, 0, 42], [0, 1, 38],
    [0, 0, 47], [0, 2, 11], [48, 0, 0], [1, 0, 38], [0, 1, 34], [0, 2, 34], [0, 0, 49], [0, 2, 6], [0, 1, 42], [1, 1, 38],
    [1, 2, 34], [1, 2, 6], [0, 2, 38], [0, 2, 31], [0, 2, 18], [1, 2, 11], [0, 2, 42], [1, 2, 18], [1, 1, 34], [1, 2, 42],
    [1, 1, 42],
];

/// Bytes from base64 text (four characters per three bytes).
const fn from_base64<const N: usize>(text: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    let (mut at, mut held, mut count, mut n) = (0, 0u32, 0, 0);
    while at < text.len() {
        let value = match text[at] {
            letter @ b'A'..=b'Z' => letter - b'A',
            letter @ b'a'..=b'z' => letter - b'a' + 26,
            letter @ b'0'..=b'9' => letter - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => 64,
        };
        at += 1;
        if value == 64 {
            continue;
        }
        (held, count) = (held << 6 | value as u32, count + 6);
        if count >= 8 && n < N {
            (out[n], n, count) = ((held >> (count - 8)) as u8, n + 1, count - 8);
        }
    }
    out
}

/// The dictionary, deflate-compressed as a zlib stream and stored as base64: it inflates to the
/// 122,784 bytes of RFC 7932 Appendix A (CRC-32 0x5136cb04).
static PACKED: [u8; 58714] = from_base64(
    b"eNps/WmXFFe2LYie219FlarvW0vPe1JwThABKBslEUQOhKQUedRQgjxZ95x7noa5u0W4gbmbp233CJxMjUEjeiSkVC+Q\
EBII1NCJRkE/xuPe7+jTReN9yVEPjwjeqPcb3ptzrr3NXbcqlUiBh7nZtr3XXns1c83VSZtJPZ9rZel0kiXTnWpc21rL\
60k97sSukc/lrazn0k5SSzu9vJ20tnRdJ0u3JtNFkszlxdZOsq3TS+Iin02Kal7vZfhhOi+a1Tzf2s7iXpbOJlnaShpJ\
1m7kzcSl9aSZF/xuPctbM51G0pxNk7nptFVvxzN4bs9Nd7OskcT1TlI0k7jWiIskni7yZqfoJs242BpXs6TbzluNdKaB\
cSZZ3Kq3kjmXzCatFsZTi11SzTuNdu46XZfUm3E9aeCaRoLnNuJOK24mz6WtrdUsn3Hp9qSK6zE+XLcV909b+E7x2PLo\
rxt5Vk9adTeXdhov4P4YRn0O93DpTKuDaxvxbDKDe7kkadXiLGvHncZcgv9mXddMWt3pNGu246KzJU9bnUbqstR1ZvIc\
Y03qc3jPucThd1XXxLvHmcsxnLxIaw0832Wx63SSGK/b7E3jO1vT1sxcmmUJPp+Li3oV351Oi2Qj5mxrK5+LccOxdmum\
ifnv4GWzPK7PYO5dkk238k7S7NYa0wnnotUr8trWtJa38lYtybBODaxJPU3qz2J9im6WNDBv8ZZ4W9qazmtZt5rFcy5L\
nGvE2bTjGuJe2/NWsmrlyr/Ef1wNC7QZclTEtaSadbH+3WIuSbZO4+8NCM0M5gmLiPXG2BtJC3OydWvS7rRjh5um7SLP\
m89ufv45yEtrc68NecRN8Z2tSdKezuIZyM9WzE59Gu/TwXgLrHkHMrul22x3MFt13A9jwO8hwZBFjL63FXOFeehMY13i\
bieHbBRjNchQnG2FXLdnMSd13L8JWceKdjrdolXN8Qdygp2QFblLukW2zGHiizxL8FWIYsdhurG2CdYxq49tac9gazRn\
46KH73Qgyy0HgakXeftvIdO1vN1bNbayMjkeT7kOJiLDaqR4GOanjX02NpNOYw46Ned+NVPEPTyh9ZOZzkSB9cAYOxne\
08UpbhfX8YwI65dtyRstyMjWdl7g3Vznsd+uW1GHXE6OV6faeV5U+d6Yw9+ObRqbg9w0sV9Xt7dNbMBaFnFr6xzuPRe3\
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\
A4NcdLnWvYtayxdUjioebTSNBp2OQgx2jae9znrrdXzipOANjX6X1e2cI4qfjlr+Sua3QQSNgd2YMcjD48rlZrg9ePTs\
WWJuiCebbiTPY6t0myILJtUVdvmyEBdrsnGByztxO5RhxpFfsaeYHOAdjT5ap6dvSWENWzBEsir/kceRitHNoMLDqJ4q\
kcwN2CS08jgbHs+alhVFzpM/1PIQ84xCcVAsbgQrNSITEIQyVjW8+kZ4yKXnNE072qzExkUBfB/2RWRsNJAQg8d7XKPM\
XcXCqXclABzhoyHAvtYTZY7VE7Ork4A6jyyYQC2B+6gXzuS4rzjxk2oSTmKSRDV4HAO2vn1fjSKe3fz8c6KFoO/aW+O5\
u9c3GBz0GlKHiZjImMEL7kC0IqBX/UEP1zfU77HkRqU1cTibhI1jcfV0SFJkLNtK/t2JLo6xTnc2/XeX/u1h/JUNs//t\
a/6HP1979c/z7/15/tOF904tHPt66cuPDbDXP/ueIfbww//3nWP/nytn/893Lz+Yv/7gxo4H1/YvnTuxeO499qr/+u2l\
cxf4xTOvL331bv+1HQvHXusfPNH/4MyD639a2vn24q03F/bf6R+78OD66Qd3Lj649sbD3Wf6+64vHti/ePL2wut/Wjpx\
eOHzY/hw6Y1bC5/cXLpz9MH8joWzpxbePv/gxvv9cx89/PLw0vmdi5/s7J9+u//m+w9ufrjwzoWF1871r7+1cHX/0uVr\
Szv29A9+3N/zdf/cyaVzZ5bOXe8fvr7w7cX+sUv9+V0Lfzq78PZtjKd/+L0H17/GJ/2Tp/tnP198+/jS1YMLH53Cvxfx\
Om/s5c3vnFjYeR5jDnBCZBM6d8/CZ8hpulGQkQ8hbJsZkq5AcOx8LwxYvQQJwr7KDOZEW4t5W0i4d2qwpbtCeFXvXmdO\
V9XnhhZSp3axBJh3w2CsoQc9/DSt58F4K2aDk1UiCQm8IqhPNCkCQ7VozKo23c7lAZyxxEW61Ae4cquMFvSu1hX8zmqd\
756N0+bdE7QvAko2LgK8qlDOhUhLltvroTDj7n4xzSp9tsjymCwDkVkxrT1CODlK6omMvep9/TmUrgdQOR8xS2Oof6Yq\
YudrtnPnQVmhay1mHoO5e6LWsboSASP9HKYeSYZPOMuaVW4cDTW8hUd5C64mGB8iVlb3IuYFfEAuBLFW0kuPyeLUYj/D\
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",
);

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    #[test]
    fn the_list_of_words_and_the_tables_are_the_rfcs() {
        // The RFC gives each one's length and CRC-32, as bytes.
        let words = inflate(&PACKED, WORDS).expect("the list of words unpacks");
        assert_eq!((words.len(), crc32(&words)), (122_784, 0x5136_cb04));
        let forms: Vec<u8> = FORMS.iter().flat_map(|[before, change, after]| [AFFIXES[*before as usize], &[0, *change], AFFIXES[*after as usize], &[0]].concat()).collect();
        assert_eq!((forms.len(), crc32(&forms)), (648, 0x3d96_5f81));
        assert_eq!((crc32(&LETTERS[..256]), crc32(&LETTERS[256..])), (0x8e91_efb7, 0xd01a_32f4));
        assert_eq!(crc32(&(0..256).map(|byte| signed(byte) as u8).collect::<Vec<u8>>()), 0x0dd7_a0d6);
        // As many words per length as the RFC says, at the right offsets: the first and last
        // 4-letter words, and the dictionary's last.
        assert_eq!((word(&words, 4, 0), word(&words, 4, 1023)), (Some(b"time".to_vec()), Some("\u{648}\u{634}".as_bytes().to_vec())));
        assert_eq!(word(&words, 24, 31).map(|word| word.len()), Some(24));
        assert_eq!((word(&words, 3, 0), word(&words, 25, 0), word(&words, 4, 121 << 10)), (None, None, None));
    }

    #[test]
    fn a_word_takes_each_of_its_forms() {
        let words = inflate(&PACKED, WORDS).unwrap();
        let form = |n: usize| String::from_utf8(word(&words, 4, n << 10).unwrap()).unwrap();
        // "time" as is, with a space after, capitalized, all caps, without its first letter or
        // last three, inside other words.
        let forms = [0, 1, 9, 44, 3, 23, 73, 102].map(form);
        assert_eq!(forms, ["time", "time ", "Time", "TIME", "ime", "t", " the time of the ", "\u{a0}time"]);
        // Uppercasing a two-byte character changes its second byte: "é" (c3 a9) to "É" (c3 89).
        let mut accented = "été".as_bytes().to_vec();
        assert_eq!((capital(&mut accented, 0), &accented[..]), (2, "Été".as_bytes()));
    }

    /// Streams from the reference encoder (libbrotlienc 1.2, quality 11 unless noted), and
    /// their contents: empty; dictionary words; text in a 1 KB window; incompressible bytes,
    /// stored uncompressed; and (quality 5) text flushed every 60 bytes, with metadata between
    /// meta-blocks.
    #[test]
    fn streams_the_reference_made_unpack_to_what_was_packed() {
        let noise: Vec<u8> = (0..64u32).map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let text = b"In the beginning the time of the world was a time of words, and the words were written with the time of the world in mind; the end of the words was not the end of the world.";
        #[rustfmt::skip]
        let cases: [(&[u8], &[u8]); 5] = [
            (&[0x3b], b""),
            (&[0x1b, 0x13, 0x00, 0xf8, 0x25, 0x00, 0xca, 0x10, 0x60, 0x6a, 0x01, 0x6c, 0x1e, 0x02], b"the time of the time"),
            (&[
                0xa1, 0x60, 0x05, 0x00, 0xe3, 0x38, 0x70, 0x63, 0xca, 0x73, 0xdf, 0x25, 0x73, 0x11, 0xa5, 0x5b, 0xd2, 0x5c, 0x87, 0x95, 0xa5, 0xdd, 0x69, 0x10,
                0x97, 0x1d, 0xb2, 0x50, 0x93, 0x03, 0x87, 0x83, 0xb6, 0xb6, 0x40, 0x12, 0x09, 0xf4, 0x21, 0x49, 0xb3, 0x16, 0xd4, 0x0d, 0x87, 0x5f, 0x16, 0xc5,
                0x91, 0x81, 0x32, 0xf8, 0x63, 0x35, 0xd0, 0x92, 0xdb, 0x18, 0x53, 0x33, 0x0c, 0x44, 0x90, 0x76, 0x42, 0xc3, 0x17, 0x38, 0x0d, 0x5f, 0x19, 0x79,
                0x6d, 0x43, 0x0e, 0x76, 0x38, 0x69, 0xbd, 0x3a, 0x86, 0x07,
            ], text),
            (&[
                0x8b, 0x1f, 0x80, 0x00, 0xbb, 0x77, 0x33, 0xef, 0xab, 0x66, 0x22, 0xde, 0x9a, 0x56, 0x11, 0xcd, 0x89, 0x45, 0x01, 0xbc, 0x78, 0x34, 0xf0, 0xac,
                0x67, 0x23, 0xdf, 0x9b, 0x57, 0x12, 0xce, 0x8a, 0x46, 0x02, 0xbd, 0x79, 0x35, 0xf1, 0xad, 0x68, 0x24, 0xe0, 0x9c, 0x58, 0x13, 0xcf, 0x8b, 0x47,
                0x03, 0xbe, 0x7a, 0x36, 0xf2, 0xae, 0x69, 0x25, 0xe1, 0x9d, 0x59, 0x14, 0xd0, 0x8c, 0x48, 0x04, 0xbf, 0x7b, 0x37, 0x03,
            ], &noise),
            (&[
                0xb0, 0x03, 0x00, 0x80, 0x80, 0x2a, 0x47, 0xda, 0xf8, 0x48, 0x46, 0xb4, 0x5c, 0xaa, 0x07, 0x87, 0x1c, 0xb0, 0xf7, 0x2d, 0x4b, 0x02, 0x0d, 0x25,
                0xf0, 0x90, 0x85, 0x28, 0xdd, 0x68, 0xba, 0xaf, 0x3f, 0x17, 0x49, 0x9c, 0xe7, 0x24, 0x12, 0xd6, 0x0a, 0xe3, 0xf3, 0x3f, 0x1b, 0xcd, 0x28, 0x69,
                0x30, 0x00, 0x96, 0x0b, 0x61, 0x20, 0x6e, 0x6f, 0x74, 0x65, 0x20, 0x62, 0x65, 0x74, 0x77, 0x65, 0x65, 0x6e, 0x20, 0x62, 0x6c, 0x6f, 0x63, 0x6b,
                0x73, 0x2c, 0x20, 0x77, 0x68, 0x69, 0x63, 0x68, 0x20, 0x75, 0x6e, 0x70, 0x61, 0x63, 0x6b, 0x73, 0x20, 0x74, 0x6f, 0x20, 0x6e, 0x6f, 0x74, 0x68,
                0x69, 0x6e, 0x67, 0xd8, 0x01, 0x00, 0x40, 0x60, 0x03, 0x8e, 0x34, 0x97, 0x61, 0x78, 0x58, 0x06, 0x87, 0x1c, 0xb0, 0xff, 0x5b, 0x08, 0x01, 0x87,
                0x10, 0x4c, 0xe4, 0xe6, 0x81, 0xb4, 0xe0, 0xfe, 0xcc, 0x51, 0x0e, 0xd7, 0xc4, 0xd1, 0x1b, 0x91, 0x8b, 0xae, 0x5c, 0xe2, 0x1a, 0x41, 0x03, 0x00,
                0x40, 0x9c, 0x72, 0xa4, 0x01, 0x05, 0x92, 0x51, 0x5b, 0xd8, 0x7e, 0xf8, 0x61, 0x03, 0x0e, 0xdc, 0x02, 0xc9, 0x32, 0x1a, 0xfd, 0xa6, 0x2c, 0xcc,
                0x63, 0x84, 0xe7, 0x04, 0x4a, 0x97, 0x0f, 0xb3, 0x2f, 0xd4, 0x10,
            ], text),
        ];
        for (packed, whole) in cases {
            assert_eq!(unpacked(packed, whole.len()).as_deref(), Some(whole), "{whole:?}");
            // And not if a byte more or less was expected, or it is truncated.
            assert_eq!((unpacked(packed, whole.len() + 1), unpacked(&packed[..packed.len() - 1], whole.len())), (None, None));
            if !whole.is_empty() {
                assert_eq!(unpacked(packed, whole.len() - 1), None);
            }
        }
    }

    /// A bit writer in stream order, each value from its low bit up.
    pub(crate) struct Writer {
        bytes: Vec<u8>,
        count: usize,
    }

    impl Writer {
        pub(crate) fn new() -> Writer {
            Writer { bytes: Vec::new(), count: 0 }
        }
        pub(crate) fn put(&mut self, value: u32, bits: usize) {
            for bit in 0..bits {
                if self.count.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                *self.bytes.last_mut().unwrap() |= ((value >> bit & 1) as u8) << (self.count % 8);
                self.count += 1;
            }
        }
        pub(crate) fn bytes(self) -> Vec<u8> {
            self.bytes
        }
    }

    /// A stream built to cost the most for its length: one-byte meta-blocks, each bringing 256
    /// block types per category, a context map per category, and 768 codes (of one symbol each,
    /// the shortest encoding).
    pub(crate) fn costly(blocks: usize) -> Vec<u8> {
        let mut w = Writer::new();
        w.put(0, 1);
        // A one-symbol code over an alphabet of `bits` bits per symbol.
        let one = |w: &mut Writer, symbol: u32, bits: usize| {
            w.put(1, 2);
            w.put(0, 2);
            w.put(symbol, bits);
        };
        for _ in 0..blocks {
            // Not last; four length nibbles, the length minus one being zero.
            w.put(0, 1);
            w.put(0, 2);
            w.put(0, 16);
            w.put(0, 1);
            for _ in 0..3 {
                // 256 types, a type code (258 symbols) and a length code (26), and the first
                // block's length: 2 more bits.
                w.put(1, 1);
                w.put(7, 3);
                w.put(127, 7);
                one(&mut w, 0, 9);
                one(&mut w, 0, 5);
                w.put(0, 2);
            }
            w.put(0, 2);
            w.put(0, 4);
            (0..16).for_each(|_| w.put(0, 32));
            // 256 literal and distance codes, with their context maps: no runs, a one-symbol
            // code, no move-to-front.
            for _ in 0..2 {
                w.put(1, 1);
                w.put(7, 3);
                w.put(127, 7);
                w.put(0, 1);
                one(&mut w, 0, 8);
                w.put(0, 1);
            }
            (0..256).for_each(|_| one(&mut w, 0, 8));
            // Commands inserting one byte with a copy that never happens (8: insert 1, copy 2).
            (0..256).for_each(|_| one(&mut w, 8, 10));
            (0..256).for_each(|_| one(&mut w, 0, 6));
        }
        // The last meta-block, empty.
        w.put(1, 1);
        w.put(1, 1);
        w.bytes()
    }

    #[test]
    fn a_stream_made_to_cost_the_most_costs_no_more_than_its_bound() {
        // A stream of nothing but codes decodes, at a per-byte cost under the budget (so no
        // stream can cost more than a few thousand steps per byte).
        let stream = costly(40);
        let started = std::time::Instant::now();
        assert_eq!(unpacked(&stream, 40), Some(vec![0; 40]));
        let took = started.elapsed();
        let words = inflate(&PACKED, WORDS).unwrap();
        let mut reading = Stream { bits: Bits { data: &stream, at: 0, held: 0, count: 0 }, out: vec![0; 40], at: 0, window: 0, last: [4, 11, 15, 16], work: usize::MAX };
        reading.window = reading.window_size().unwrap();
        while reading.block(&words).unwrap() {}
        let spent = (usize::MAX - reading.work) / stream.len();
        eprintln!("{} bytes of codes alone: {spent} steps a byte, {:.1} ms", stream.len(), took.as_secs_f64() * 1000.0);
        assert!(spent > 500 && spent < WORK_PER_BYTE, "{spent}");
    }

    /// The system's Brotli libraries: to compress with every setting, and to decompress damaged
    /// streams, checking this agrees.
    #[cfg(target_os = "linux")]
    pub(crate) mod system {
        use std::ffi::{c_char, c_int, c_void};
        unsafe extern "C" {
            fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
            fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
        }
        type Create = extern "C" fn(*const c_void, *const c_void, *const c_void) -> *mut c_void;
        type Set = extern "C" fn(*mut c_void, c_int, u32) -> c_int;
        type Stream = extern "C" fn(*mut c_void, c_int, *mut usize, *mut *const u8, *mut usize, *mut *mut u8, *mut usize) -> c_int;
        type Done = extern "C" fn(*mut c_void) -> c_int;
        type Destroy = extern "C" fn(*mut c_void);
        type Unpack = extern "C" fn(usize, *const u8, *mut usize, *mut u8) -> c_int;

        /// Encoder settings: quality, window, mode (generic, text, font), postfix bits and
        /// direct distance count, flush interval (0, none), and whether metadata goes between
        /// flushes.
        #[derive(Clone, Copy, Debug)]
        pub(crate) struct Settings {
            pub quality: u32,
            pub window: u32,
            pub mode: u32,
            pub postfix: u32,
            pub direct: u32,
            pub flushed: usize,
            pub notes: bool,
        }

        pub(crate) struct Brotli {
            create: Create,
            set: Set,
            stream: Stream,
            done: Done,
            more: Done,
            destroy: Destroy,
            unpack: Unpack,
        }

        /// The system's Brotli, if installed.
        pub(crate) fn brotli() -> Option<Brotli> {
            // SAFETY: the libraries are opened by name, each function looked up by name and
            // called as `brotli/encode.h` and `brotli/decode.h` declare it.
            unsafe {
                let (packer, unpacker) = (dlopen(c"libbrotlienc.so.1".as_ptr(), 2), dlopen(c"libbrotlidec.so.1".as_ptr(), 2));
                let found = |library: *mut c_void, name: &std::ffi::CStr| Some(dlsym(library, name.as_ptr())).filter(|found| !library.is_null() && !found.is_null());
                Some(Brotli {
                    create: std::mem::transmute::<*mut c_void, Create>(found(packer, c"BrotliEncoderCreateInstance")?),
                    set: std::mem::transmute::<*mut c_void, Set>(found(packer, c"BrotliEncoderSetParameter")?),
                    stream: std::mem::transmute::<*mut c_void, Stream>(found(packer, c"BrotliEncoderCompressStream")?),
                    done: std::mem::transmute::<*mut c_void, Done>(found(packer, c"BrotliEncoderIsFinished")?),
                    more: std::mem::transmute::<*mut c_void, Done>(found(packer, c"BrotliEncoderHasMoreOutput")?),
                    destroy: std::mem::transmute::<*mut c_void, Destroy>(found(packer, c"BrotliEncoderDestroyInstance")?),
                    unpack: std::mem::transmute::<*mut c_void, Unpack>(found(unpacker, c"BrotliDecoderDecompress")?),
                })
            }
        }

        impl Brotli {
            /// `bytes` compressed with these settings.
            pub(crate) fn pack(&self, bytes: &[u8], settings: Settings) -> Vec<u8> {
                let Settings { quality, window, mode, postfix, direct, flushed, notes } = settings;
                let mut out = vec![0u8; bytes.len() * 2 + 1024 + bytes.len() / 8];
                let (mut total, state) = (0usize, (self.create)(std::ptr::null(), std::ptr::null(), std::ptr::null()));
                for (parameter, value) in [(0, mode), (1, quality), (2, window), (7, postfix), (8, direct)] {
                    (self.set)(state, parameter, value);
                }
                let pieces: Vec<&[u8]> = if flushed == 0 { vec![bytes] } else { bytes.chunks(flushed).collect() };
                let note = b"a note between blocks, which unpacks to nothing";
                for (n, piece) in pieces.iter().enumerate() {
                    if notes && n % 2 == 1 {
                        self.run(state, 3, note, &mut out, &mut total);
                    }
                    let last = n + 1 == pieces.len();
                    self.run(state, if last { 2 } else { 1 }, piece, &mut out, &mut total);
                }
                if pieces.is_empty() {
                    self.run(state, 2, &[], &mut out, &mut total);
                }
                (self.destroy)(state);
                out.truncate(total);
                out
            }

            /// Feeds `bytes` to the encoder with an operation until it has consumed them all
            /// (and, when finishing, until done).
            fn run(&self, state: *mut c_void, operation: c_int, bytes: &[u8], out: &mut [u8], total: &mut usize) {
                let (mut left, mut next) = (bytes.len(), bytes.as_ptr());
                for _ in 0..1_000_000 {
                    let mut room = out.len() - *total;
                    let mut to = out[*total..].as_mut_ptr();
                    let before = room;
                    let fine = (self.stream)(state, operation, &mut left, &mut next, &mut room, &mut to, std::ptr::null_mut());
                    assert_eq!(fine, 1, "the packer refused operation {operation}");
                    *total += before - room;
                    let finished = operation != 2 || (self.done)(state) == 1;
                    if left == 0 && finished && (self.more)(state) == 0 {
                        return;
                    }
                }
                panic!("the packer never finished");
            }

            /// The system's decompression of `packed` as `long` bytes: `None` if damaged or of
            /// another length.
            pub(crate) fn unpack(&self, packed: &[u8], long: usize) -> Option<Vec<u8>> {
                let mut out = vec![0u8; long + 1];
                let mut size = out.len();
                let fine = (self.unpack)(packed.len(), packed.as_ptr(), &mut size, out.as_mut_ptr()) == 1;
                out.truncate(size);
                (fine && size == long).then_some(out)
            }
        }
    }

    /// Inputs of many kinds: text in several languages, a font, an already compressed image,
    /// zeros, noise, a short repeated pattern, and nothing.
    #[cfg(target_os = "linux")]
    fn kinds() -> Vec<(&'static str, Vec<u8>)> {
        let noise: Vec<u8> = (0..70_000u32).map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let text = include_str!("../GUIDE.md").as_bytes().to_vec();
        let languages = "Привет, мир! 你好，世界。 こんにちは世界 مرحبا بالعالم नमस्ते दुनिया Γειά σου Κόσμε ".repeat(40).into_bytes();
        let picture = include_bytes!("../docs/windows.png").to_vec();
        let font = crate::font_file::tests::file(&[0, 1, 0, 0], &crate::font_file::tests::tables());
        let pattern = b"abcabcabd".repeat(3000);
        let signed: Vec<u8> = (0..20_000i32).flat_map(|n| ((n % 300 - 150) as i16).to_be_bytes()).collect();
        let mixed = [&text[..text.len().min(30_000)], &noise[..20_000], &signed[..], &languages].concat();
        vec![
            ("nothing", Vec::new()),
            ("one", vec![42]),
            ("text", text),
            ("languages", languages),
            ("picture", picture),
            ("font", font),
            ("noughts", vec![0; 100_000]),
            ("noise", noise),
            ("pattern", pattern),
            ("signed", signed),
            ("mixed", mixed),
        ]
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn every_kind_of_stream_the_system_makes_unpacks_to_what_was_packed() {
        let Some(brotli) = system::brotli() else { return eprintln!("no Brotli on this system: not checked") };
        let mut checked = 0;
        for (name, bytes) in kinds() {
            for quality in 0..=11 {
                // The highest qualities are slow on large inputs: those use only their smallest
                // windows.
                for window in [10, 12, 16, 18, 22, 24].into_iter().filter(|window| quality < 10 || *window <= 16 || bytes.len() < 40_000) {
                    for mode in [0, 1, 2] {
                        let settings = system::Settings { quality, window, mode, postfix: 0, direct: 0, flushed: 0, notes: false };
                        let packed = brotli.pack(&bytes, settings);
                        assert_eq!(unpacked(&packed, bytes.len()).as_deref(), Some(&bytes[..]), "{name} {settings:?}");
                        checked += 1;
                    }
                }
            }
            // Distances with postfix bits and direct codes, and flushes (with uncompressed
            // blocks) and metadata between them.
            for (postfix, direct, flushed, notes) in [(1, 2, 0, false), (2, 12, 0, false), (3, 120, 0, false), (3, 8, 1000, true), (0, 0, 333, true), (0, 0, 7, false)] {
                for quality in [2, 5, 9, 11] {
                    let settings = system::Settings { quality, window: 18, mode: 0, postfix, direct, flushed, notes };
                    let packed = brotli.pack(&bytes[..bytes.len().min(40_000)], settings);
                    assert_eq!(unpacked(&packed, bytes.len().min(40_000)).as_deref(), Some(&bytes[..bytes.len().min(40_000)]), "{name} {settings:?}");
                    checked += 1;
                }
            }
        }
        eprintln!("{checked} streams unpacked as they were packed");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_damaged_stream_is_read_as_the_systems_brotli_reads_it() {
        let Some(brotli) = system::brotli() else { return eprintln!("no Brotli on this system: not checked") };
        let (mut same, mut whole) = (0, 0);
        for (name, bytes) in kinds().into_iter().filter(|(name, _)| ["text", "languages", "font", "mixed", "pattern"].contains(name)) {
            let bytes = &bytes[..bytes.len().min(6000)];
            for quality in [1, 5, 11] {
                let settings = system::Settings { quality, window: 16, mode: 1, postfix: 0, direct: 0, flushed: 2000, notes: true };
                let packed = brotli.pack(bytes, settings);
                // Truncated anywhere, with any one byte changed, with a bit flipped.
                let mut damaged: Vec<Vec<u8>> = (0..packed.len()).map(|long| packed[..long].to_vec()).collect();
                for (at, change) in (0..packed.len()).flat_map(|at| [(at, 0), (at, 255), (at, packed[at] ^ 1 << (at % 8)), (at, packed[at] ^ 0x10)]) {
                    let mut changed = packed.clone();
                    changed[at] = change;
                    damaged.push(changed);
                }
                for stream in damaged {
                    let (ours, theirs) = (unpacked(&stream, bytes.len()), brotli.unpack(&stream, bytes.len()));
                    assert_eq!(ours, theirs, "{name} q{quality}: {} bytes", stream.len());
                    (same, whole) = (same + 1, whole + ours.is_some() as usize);
                }
            }
        }
        eprintln!("{same} damaged streams read alike, {whole} of them still whole");
    }
}
