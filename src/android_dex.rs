//! A DEX file (Android's compiled Java) made in memory: the few classes Android's own Java
//! interfaces need from an app with no Java of its own. Each class extends a system class; its
//! constructor only hands its arguments to the parent's, and every other method is `native`,
//! answered by the library through JNI (see `android_jni`). The file is loaded with
//! `InMemoryDexClassLoader`: nothing is written to disk, and the app's package holds no Java.
use std::collections::BTreeSet;

/// One class to write.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Class<'a> {
    /// Its name, with slashes: `taut/Host`.
    pub name: &'a str,
    /// The class it extends: `android/view/View`.
    pub parent: &'a str,
    /// The descriptors of what its constructor takes and hands on to the parent's (at most four
    /// words: `this` takes the fifth register).
    pub made_with: &'a [&'a str],
    /// Its native methods, public: name and descriptor, as `("findFocus",
    /// "(I)Landroid/view/accessibility/AccessibilityNodeInfo;")`.
    pub natives: &'a [(&'a str, &'a str)],
}

/// A method as the file names it: its class, name, return type and parameters (descriptors).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Called {
    class: String,
    name: String,
    proto: Proto,
}

/// A method's types: what it returns, and what it takes (descriptors).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Proto {
    returns: String,
    takes: Vec<String>,
}

/// Access flags: public, native, constructor.
const PUBLIC: u32 = 0x1;
const NATIVE: u32 = 0x100;
const CONSTRUCTOR: u32 = 0x1_0000;
/// Map item kinds.
const HEADER: u16 = 0x0000;
const STRING_IDS: u16 = 0x0001;
const TYPE_IDS: u16 = 0x0002;
const PROTO_IDS: u16 = 0x0003;
const METHOD_IDS: u16 = 0x0005;
const CLASS_DEFS: u16 = 0x0006;
const MAP: u16 = 0x1000;
const TYPE_LISTS: u16 = 0x1001;
const CLASS_DATA: u16 = 0x2000;
const CODE: u16 = 0x2001;
const STRING_DATA: u16 = 0x2002;
/// No index (a class without a source file).
const NONE: u32 = u32::MAX;
/// The header's size.
const HEADER_SIZE: usize = 0x70;

/// The length of the first type descriptor in `text`: `I`, `[B`, `Landroid/view/View;`.
fn type_length(text: &str) -> usize {
    let arrays = text.bytes().take_while(|byte| *byte == b'[').count();
    match text.as_bytes().get(arrays) {
        Some(b'L') => arrays + text[arrays..].find(';').map_or(text.len() - arrays, |end| end + 1),
        _ => arrays + 1,
    }
}

/// A method descriptor's types: `(IZ)V` takes `I` and `Z` and returns `V`.
fn proto_of(descriptor: &str) -> Proto {
    let (inside, returns) = descriptor.trim_start_matches('(').split_once(')').unwrap_or(("", "V"));
    let mut takes = Vec::new();
    let mut rest = inside;
    while !rest.is_empty() {
        let long = type_length(rest).min(rest.len());
        takes.push(rest[..long].to_string());
        rest = &rest[long..];
    }
    Proto { returns: returns.to_string(), takes }
}

/// A type's letter in a short signature: `L` for any object or array.
fn short(descriptor: &str) -> char {
    match descriptor.as_bytes().first() {
        Some(b'L' | b'[') | None => 'L',
        Some(letter) => *letter as char,
    }
}

impl Proto {
    /// Its short signature: return letter, then a letter a parameter.
    fn shorty(&self) -> String {
        std::iter::once(short(&self.returns)).chain(self.takes.iter().map(|one| short(one))).collect()
    }
    /// The registers its parameters take (`long` and `double` two each).
    fn words(&self) -> usize {
        self.takes.iter().map(|one| if matches!(one.as_str(), "J" | "D") { 2 } else { 1 }).sum()
    }
}

/// Every method the classes define or call.
fn methods_of(classes: &[Class]) -> Vec<Called> {
    let mut all = BTreeSet::new();
    for class in classes {
        let made = Proto { returns: "V".into(), takes: class.made_with.iter().map(|one| one.to_string()).collect() };
        all.insert(Called { class: format!("L{};", class.name), name: "<init>".into(), proto: made.clone() });
        all.insert(Called { class: format!("L{};", class.parent), name: "<init>".into(), proto: made });
        all.extend(class.natives.iter().map(|(name, descriptor)| Called { class: format!("L{};", class.name), name: name.to_string(), proto: proto_of(descriptor) }));
    }
    all.into_iter().collect()
}

/// The file's tables, each sorted as the format asks.
struct Pools {
    strings: Vec<String>,
    types: Vec<String>,
    protos: Vec<Proto>,
    methods: Vec<Called>,
}

impl Pools {
    fn of(classes: &[Class]) -> Pools {
        let methods = methods_of(classes);
        let mut types: BTreeSet<String> = classes.iter().flat_map(|class| [format!("L{};", class.name), format!("L{};", class.parent)]).collect();
        types.extend(methods.iter().flat_map(|method| std::iter::once(method.proto.returns.clone()).chain(method.proto.takes.iter().cloned())));
        let protos: BTreeSet<Proto> = methods.iter().map(|method| method.proto.clone()).collect();
        let mut strings: BTreeSet<String> = types.iter().cloned().collect();
        strings.extend(methods.iter().map(|method| method.name.clone()));
        strings.extend(protos.iter().map(Proto::shorty));
        // Sorted by string, type indices follow string order, so the protos' and methods' own
        // orders (by type and string index) are their derived orders.
        let methods = Pools::sorted_methods(methods, &strings.iter().cloned().collect::<Vec<_>>(), &types.iter().cloned().collect::<Vec<_>>(), &protos.iter().cloned().collect::<Vec<_>>());
        Pools { strings: strings.into_iter().collect(), types: types.into_iter().collect(), protos: protos.into_iter().collect(), methods }
    }

    /// Methods by class, then name, then proto, each by its index.
    fn sorted_methods(mut methods: Vec<Called>, strings: &[String], types: &[String], protos: &[Proto]) -> Vec<Called> {
        let at = |pool: &[String], one: &String| pool.binary_search(one).unwrap_or(0);
        methods.sort_by_key(|method| (at(types, &method.class), at(strings, &method.name), protos.binary_search(&method.proto).unwrap_or(0)));
        methods
    }

    fn string(&self, text: &str) -> u32 {
        self.strings.binary_search_by(|one| one.as_str().cmp(text)).unwrap_or(0) as u32
    }
    fn type_of(&self, descriptor: &str) -> u32 {
        self.types.binary_search_by(|one| one.as_str().cmp(descriptor)).unwrap_or(0) as u32
    }
    fn proto(&self, proto: &Proto) -> u32 {
        self.protos.binary_search(proto).unwrap_or(0) as u32
    }
    fn method(&self, class: &str, name: &str, proto: &Proto) -> u32 {
        self.methods.iter().position(|one| one.class == class && one.name == name && one.proto == *proto).unwrap_or(0) as u32
    }
}

/// Bytes being written, little-endian.
#[derive(Default)]
struct Out {
    bytes: Vec<u8>,
}

impl Out {
    fn u16(&mut self, value: u16) {
        self.bytes.extend(value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.bytes.extend(value.to_le_bytes());
    }
    fn uleb(&mut self, mut value: u32) {
        while value >= 0x80 {
            self.bytes.push(value as u8 | 0x80);
            value >>= 7;
        }
        self.bytes.push(value as u8);
    }
    fn align(&mut self) {
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
    }
    fn at(&self) -> u32 {
        self.bytes.len() as u32
    }
    fn set(&mut self, at: usize, value: u32) {
        self.bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
}

/// Where each part of the data section landed.
struct Placed {
    lists: Vec<u32>,
    codes: Vec<u32>,
    strings: Vec<u32>,
    data: Vec<u32>,
}

/// The DEX file holding `classes`.
pub(crate) fn dex(classes: &[Class]) -> Vec<u8> {
    let pools = Pools::of(classes);
    let (s, t, p, m, c) = (pools.strings.len(), pools.types.len(), pools.protos.len(), pools.methods.len(), classes.len());
    let ids = [HEADER_SIZE, HEADER_SIZE + s * 4, HEADER_SIZE + s * 4 + t * 4, HEADER_SIZE + s * 4 + t * 4 + p * 12, HEADER_SIZE + s * 4 + t * 4 + p * 12 + m * 8];
    let data_at = ids[4] + c * 32;
    let mut out = Out { bytes: vec![0; data_at] };
    let placed = data(&mut out, &pools, classes);
    out.align();
    let map_at = out.at();
    write_map(&mut out, [s, t, p, m, c], ids, &placed, map_at);
    write_ids(&mut out, &pools, ids, &placed);
    write_classes(&mut out, &pools, classes, ids[4], &placed);
    let size = out.at();
    let header: [u32; 26] = [
        size,
        HEADER_SIZE as u32,
        0x1234_5678,
        0,
        0,
        map_at,
        s as u32,
        ids[0] as u32,
        t as u32,
        ids[1] as u32,
        p as u32,
        ids[2] as u32,
        0,
        0,
        m as u32,
        ids[3] as u32,
        c as u32,
        ids[4] as u32,
        size - data_at as u32,
        data_at as u32,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    out.bytes[..8].copy_from_slice(b"dex\n035\0");
    header[..20].iter().enumerate().for_each(|(n, value)| out.set(32 + n * 4, *value));
    let signature = sha1(&out.bytes[32..]);
    out.bytes[12..32].copy_from_slice(&signature);
    let sum = adler32(&out.bytes[12..]);
    out.set(8, sum);
    out.bytes
}

/// The data section: parameter lists, constructor code, string data and class data.
fn data(out: &mut Out, pools: &Pools, classes: &[Class]) -> Placed {
    let mut placed = Placed { lists: Vec::new(), codes: Vec::new(), strings: Vec::new(), data: Vec::new() };
    for proto in &pools.protos {
        out.align();
        placed.lists.push(if proto.takes.is_empty() { 0 } else { out.at() });
        if proto.takes.is_empty() {
            continue;
        }
        out.u32(proto.takes.len() as u32);
        proto.takes.iter().for_each(|one| out.u16(pools.type_of(one) as u16));
    }
    for class in classes {
        out.align();
        placed.codes.push(out.at());
        write_constructor(out, pools, class);
    }
    for text in &pools.strings {
        placed.strings.push(out.at());
        out.uleb(text.encode_utf16().count() as u32);
        out.bytes.extend(text.as_bytes());
        out.bytes.push(0);
    }
    for class in classes {
        placed.data.push(out.at());
        write_class_data(out, pools, class, placed.codes[placed.data.len() - 1]);
    }
    placed
}

/// A constructor's code: `invoke-direct {v0 .. vn}, parent.<init>`, then `return-void`.
fn write_constructor(out: &mut Out, pools: &Pools, class: &Class) {
    let proto = Proto { returns: "V".into(), takes: class.made_with.iter().map(|one| one.to_string()).collect() };
    let words = (proto.words() + 1).min(5) as u16;
    let parent = pools.method(&format!("L{};", class.parent), "<init>", &proto);
    [words, words, words, 0].into_iter().for_each(|value| out.u16(value));
    out.u32(0);
    out.u32(4);
    let low = (0..words.min(4)).fold(0u16, |nibbles, register| nibbles | register << (register * 4));
    let fifth = if words == 5 { 4 << 8 } else { 0 };
    [0x70 | words << 12 | fifth, parent as u16, low, 0x0e].into_iter().for_each(|unit| out.u16(unit));
}

/// A class's methods: its constructor (direct), then its natives (virtual), in index order.
fn write_class_data(out: &mut Out, pools: &Pools, class: &Class, code: u32) {
    let name = format!("L{};", class.name);
    let made = Proto { returns: "V".into(), takes: class.made_with.iter().map(|one| one.to_string()).collect() };
    let mut natives: Vec<u32> = class.natives.iter().map(|(called, descriptor)| pools.method(&name, called, &proto_of(descriptor))).collect();
    natives.sort_unstable();
    [0, 0, 1, natives.len() as u32].into_iter().for_each(|count| out.uleb(count));
    out.uleb(pools.method(&name, "<init>", &made));
    out.uleb(PUBLIC | CONSTRUCTOR);
    out.uleb(code);
    let mut last = 0;
    for method in natives {
        out.uleb(method - last);
        out.uleb(PUBLIC | NATIVE);
        out.uleb(0);
        last = method;
    }
}

/// The map of every section, by place.
fn write_map(out: &mut Out, [s, t, p, m, c]: [usize; 5], ids: [usize; 5], placed: &Placed, map_at: u32) {
    let lists: Vec<u32> = placed.lists.iter().copied().filter(|at| *at != 0).collect();
    let mut items = vec![(HEADER, 1, 0), (STRING_IDS, s, ids[0] as u32), (TYPE_IDS, t, ids[1] as u32), (PROTO_IDS, p, ids[2] as u32), (METHOD_IDS, m, ids[3] as u32), (CLASS_DEFS, c, ids[4] as u32)];
    items.extend(lists.first().map(|first| (TYPE_LISTS, lists.len(), *first)));
    items.extend([(CODE, c, placed.codes[0]), (STRING_DATA, s, placed.strings[0]), (CLASS_DATA, c, placed.data[0]), (MAP, 1, map_at)]);
    items.retain(|(_, count, _)| *count > 0);
    items.sort_by_key(|(_, _, at)| *at);
    out.u32(items.len() as u32);
    for (kind, count, at) in items {
        out.u16(kind);
        out.u16(0);
        out.u32(count as u32);
        out.u32(at);
    }
}

/// The id tables: strings, types, protos and methods.
fn write_ids(out: &mut Out, pools: &Pools, ids: [usize; 5], placed: &Placed) {
    placed.strings.iter().enumerate().for_each(|(n, at)| out.set(ids[0] + n * 4, *at));
    pools.types.iter().enumerate().for_each(|(n, one)| out.set(ids[1] + n * 4, pools.string(one)));
    for (n, proto) in pools.protos.iter().enumerate() {
        let at = ids[2] + n * 12;
        out.set(at, pools.string(&proto.shorty()));
        out.set(at + 4, pools.type_of(&proto.returns));
        out.set(at + 8, placed.lists[n]);
    }
    for (n, method) in pools.methods.iter().enumerate() {
        let (at, class, proto) = (ids[3] + n * 8, pools.type_of(&method.class), pools.proto(&method.proto));
        out.bytes[at..at + 4].copy_from_slice(&[class as u8, (class >> 8) as u8, proto as u8, (proto >> 8) as u8]);
        out.set(at + 4, pools.string(&method.name));
    }
}

/// The class definitions.
fn write_classes(out: &mut Out, pools: &Pools, classes: &[Class], first: usize, placed: &Placed) {
    for (n, class) in classes.iter().enumerate() {
        let fields = [pools.type_of(&format!("L{};", class.name)), PUBLIC, pools.type_of(&format!("L{};", class.parent)), 0, NONE, 0, placed.data[n], 0];
        fields.iter().enumerate().for_each(|(k, value)| out.set(first + n * 32 + k * 4, *value));
    }
}

/// Adler-32, the file's checksum.
fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in bytes.chunks(5552) {
        chunk.iter().for_each(|byte| (a, b) = (a + u32::from(*byte), b + a + u32::from(*byte)));
        (a, b) = (a % 65521, b % 65521);
    }
    b << 16 | a
}

/// SHA-1, the file's signature.
fn sha1(bytes: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend((bytes.len() as u64 * 8).to_be_bytes());
    padded.chunks(64).for_each(|block| sha1_block(&mut state, block));
    let mut digest = [0u8; 20];
    digest.chunks_mut(4).zip(state).for_each(|(out, word)| out.copy_from_slice(&word.to_be_bytes()));
    digest
}

/// One 64-byte block of SHA-1.
fn sha1_block(state: &mut [u32; 5], block: &[u8]) {
    let mut w = [0u32; 80];
    (0..16).for_each(|n| w[n] = u32::from_be_bytes([block[n * 4], block[n * 4 + 1], block[n * 4 + 2], block[n * 4 + 3]]));
    (16..80).for_each(|n| w[n] = (w[n - 3] ^ w[n - 8] ^ w[n - 14] ^ w[n - 16]).rotate_left(1));
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (n, word) in w.iter().enumerate() {
        let (f, k) = match n / 20 {
            0 => (b & c | !b & d, 0x5A82_7999),
            1 => (b ^ c ^ d, 0x6ED9_EBA1),
            2 => (b & c | b & d | c & d, 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6),
        };
        let next = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*word);
        (e, d, c, b, a) = (d, c, b.rotate_left(30), a, next);
    }
    for (kept, now) in state.iter_mut().zip([a, b, c, d, e]) {
        *kept = kept.wrapping_add(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDER: Class = Class {
        name: "taut/Nodes",
        parent: "android/view/accessibility/AccessibilityNodeProvider",
        made_with: &[],
        natives: &[("createAccessibilityNodeInfo", "(I)Landroid/view/accessibility/AccessibilityNodeInfo;"), ("performAction", "(IILandroid/os/Bundle;)Z")],
    };
    const VIEW: Class = Class { name: "taut/Host", parent: "android/view/View", made_with: &["Landroid/content/Context;"], natives: &[("onCheckIsTextEditor", "()Z")] };

    fn u32_at(file: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(file[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn sha1_and_adler32_give_the_known_answers() {
        let hex = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(&[b'a'; 1000])), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn descriptors_are_split_into_types() {
        let proto = proto_of("(IILandroid/os/Bundle;[J)Z");
        assert_eq!(proto.takes, ["I", "I", "Landroid/os/Bundle;", "[J"]);
        assert_eq!((proto.returns.as_str(), proto.shorty().as_str(), proto.words()), ("Z", "ZIILL", 4));
        assert_eq!(proto_of("()V").shorty(), "V");
    }

    #[test]
    fn a_file_has_its_header_tables_and_sums() {
        let file = dex(&[PROVIDER, VIEW]);
        assert_eq!(&file[..8], b"dex\n035\0");
        assert_eq!(u32_at(&file, 32) as usize, file.len());
        assert_eq!((u32_at(&file, 36), u32_at(&file, 40)), (0x70, 0x1234_5678));
        assert_eq!(u32_at(&file, 8), adler32(&file[12..]));
        assert_eq!(file[12..32], sha1(&file[32..]));
        // Two classes; their methods: two constructors, two parents' and three natives.
        assert_eq!((u32_at(&file, 96), u32_at(&file, 88)), (2, 7));
        // Strings are sorted, and every one is in the file.
        let strings: Vec<String> = (0..u32_at(&file, 56) as usize)
            .map(|n| {
                let at = u32_at(&file, u32_at(&file, 60) as usize + n * 4) as usize + 1;
                String::from_utf8(file[at..].iter().take_while(|byte| **byte != 0).copied().collect()).unwrap()
            })
            .collect();
        assert!(strings.windows(2).all(|pair| pair[0] < pair[1]), "{strings:?}");
        assert!(strings.iter().any(|one| one == "Ltaut/Host;") && strings.iter().any(|one| one == "<init>"));
        // The map ends the file and lists every section.
        let map = u32_at(&file, 52) as usize;
        assert_eq!(map + 4 + u32_at(&file, map) as usize * 12, file.len());
    }

    #[test]
    fn a_constructor_hands_its_arguments_to_its_parents() {
        let file = dex(&[VIEW]);
        let pools = Pools::of(&[VIEW]);
        let code = file.windows(8).position(|w| w == [2, 0, 2, 0, 2, 0, 0, 0]).expect("a code item with two registers");
        let parent = pools.method("Landroid/view/View;", "<init>", &Proto { returns: "V".into(), takes: vec!["Landroid/content/Context;".into()] });
        let units: Vec<u16> = file[code + 16..code + 24].chunks(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        assert_eq!(units, [0x2070, parent as u16, 0x0010, 0x000e]);
    }

    /// The SDK's own `dexdump` reads the file and finds the classes, where the SDK is here
    /// (`ANDROID_HOME`).
    #[test]
    fn the_sdks_dexdump_reads_the_file() {
        let Some(tool) = std::env::var_os("ANDROID_HOME").and_then(|home| {
            let tools = std::path::Path::new(&home).join("build-tools");
            let newest = std::fs::read_dir(tools).ok()?.filter_map(Result::ok).map(|entry| entry.path()).max()?;
            [newest.join("dexdump"), newest.join("dexdump.exe")].into_iter().find(|path| path.exists())
        }) else {
            return eprintln!("no Android SDK here");
        };
        let file = std::env::temp_dir().join(format!("taut-dex-{}.dex", std::process::id()));
        std::fs::write(&file, dex(&[PROVIDER, VIEW])).unwrap();
        let said = std::process::Command::new(tool).arg("-d").arg(&file).output().unwrap();
        let _ = std::fs::remove_file(&file);
        let text = String::from_utf8_lossy(&said.stdout).into_owned() + &String::from_utf8_lossy(&said.stderr);
        assert!(said.status.success() && !text.contains("ERROR") && !text.contains("Failure"), "{text}");
        assert!(text.contains("Class descriptor  : 'Ltaut/Nodes;'") && text.contains("Superclass        : 'Landroid/view/View;'"), "{text}");
        assert!(text.contains("invoke-direct {v0, v1}, Landroid/view/View;.<init>:(Landroid/content/Context;)V"), "{text}");
        assert!(text.contains("name          : 'performAction'") && text.contains("access        : 0x0101 (PUBLIC NATIVE)"), "{text}");
    }
}
