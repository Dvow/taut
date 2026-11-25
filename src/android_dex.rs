//! A DEX file (Android's compiled Java) made in memory: the few classes Android's own Java
//! interfaces need from an app with no Java of its own. Each class extends a system class; its
//! constructor only hands its arguments to the parent's, and every other method is `native`,
//! answered by the library through JNI (see `android_jni`). The file is loaded with
//! `InMemoryDexClassLoader`: nothing is written to disk, and the app's package holds no Java.
//!
//! Off Android the writer can do more, for the package's own `classes.dex` (`examples/apk.rs`,
//! see `APP`): interfaces, instance fields, and methods with code of their own. On Android those
//! parts are empty, and the writer an app carries is no bigger for them.
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
    /// The interfaces it implements, with slashes.
    #[cfg(not(target_os = "android"))]
    pub interfaces: &'a [&'a str],
    /// Its instance fields, public: name and type descriptor.
    #[cfg(not(target_os = "android"))]
    pub fields: &'a [(&'a str, &'a str)],
    /// Its methods with code, public.
    #[cfg(not(target_os = "android"))]
    pub methods: &'a [Method<'a>],
}

/// A method with code of its own: its name, descriptor, how many registers it uses (its
/// parameters, `this` first, are the last ones), and its instructions.
#[cfg(not(target_os = "android"))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Method<'a> {
    pub name: &'a str,
    pub descriptor: &'a str,
    pub registers: u16,
    pub code: &'a [Op<'a>],
}

/// An instruction, or part of one.
#[cfg(not(target_os = "android"))]
#[derive(Clone, Copy, Debug)]
pub(crate) enum Op<'a> {
    /// Code units as they are.
    Units(&'a [u16]),
    /// A unit, then the index of what it refers to (formats 21c and 22c).
    With(u16, Ref<'a>),
    /// `invoke-*` (its opcode) of a method, with these registers (at most five).
    Invoke(u8, Ref<'a>, &'a [u8]),
}

/// What an instruction refers to.
#[cfg(not(target_os = "android"))]
#[derive(Clone, Copy, Debug)]
pub(crate) enum Ref<'a> {
    /// A method: its class (with slashes), name and descriptor.
    Method(&'a str, &'a str, &'a str),
    /// A field: its class, name and type descriptor.
    Field(&'a str, &'a str, &'a str),
    /// A type, by descriptor.
    Type(&'a str),
    /// A string.
    Text(&'a str),
}

/// A method as the file names it: its class, name, return type and parameters (descriptors).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Called {
    class: String,
    name: String,
    proto: Proto,
}

/// A field as the file names it: its class, name and type (descriptors).
#[cfg(not(target_os = "android"))]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Kept {
    class: String,
    name: String,
    kind: String,
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
const FIELD_IDS: u16 = 0x0004;
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

/// The method a reference names, if it names one.
#[cfg(not(target_os = "android"))]
fn called(of: &Ref) -> Option<Called> {
    match of {
        Ref::Method(class, name, descriptor) => Some(Called { class: format!("L{class};"), name: name.to_string(), proto: proto_of(descriptor) }),
        _ => None,
    }
}

/// The field a reference names, if it names one.
#[cfg(not(target_os = "android"))]
fn kept(of: &Ref) -> Option<Kept> {
    match of {
        Ref::Field(class, name, kind) => Some(Kept { class: format!("L{class};"), name: name.to_string(), kind: kind.to_string() }),
        _ => None,
    }
}

/// What an instruction refers to, if anything.
#[cfg(not(target_os = "android"))]
fn reference<'a>(op: &Op<'a>) -> Option<Ref<'a>> {
    match op {
        Op::Units(_) => None,
        Op::With(_, to) | Op::Invoke(_, to, _) => Some(*to),
    }
}

/// Everything the code of the classes refers to.
#[cfg(not(target_os = "android"))]
fn references<'a>(classes: &[Class<'a>]) -> Vec<Ref<'a>> {
    classes.iter().flat_map(|class| class.methods.iter().flat_map(|method| method.code.iter().filter_map(reference))).collect()
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
    #[cfg(not(target_os = "android"))]
    all.extend(package_methods(classes));
    all.into_iter().collect()
}

/// The methods with code the classes define, and those their code calls.
#[cfg(not(target_os = "android"))]
fn package_methods(classes: &[Class]) -> Vec<Called> {
    let defined = classes.iter().flat_map(|class| class.methods.iter().map(|method| Called { class: format!("L{};", class.name), name: method.name.to_string(), proto: proto_of(method.descriptor) }));
    defined.chain(references(classes).iter().filter_map(called)).collect()
}

/// Every field the classes have or use.
#[cfg(not(target_os = "android"))]
fn fields_of(classes: &[Class]) -> Vec<Kept> {
    let mut all = BTreeSet::new();
    for class in classes {
        all.extend(class.fields.iter().map(|(name, kind)| Kept { class: format!("L{};", class.name), name: name.to_string(), kind: kind.to_string() }));
    }
    all.extend(references(classes).iter().filter_map(kept));
    all.into_iter().collect()
}

/// The file's tables, each sorted as the format asks.
struct Pools {
    strings: Vec<String>,
    types: Vec<String>,
    protos: Vec<Proto>,
    #[cfg(not(target_os = "android"))]
    fields: Vec<Kept>,
    methods: Vec<Called>,
}

impl Pools {
    fn of(classes: &[Class]) -> Pools {
        let methods = methods_of(classes);
        let mut types: BTreeSet<String> = classes.iter().flat_map(|class| [format!("L{};", class.name), format!("L{};", class.parent)]).collect();
        types.extend(methods.iter().flat_map(|method| std::iter::once(method.proto.returns.clone()).chain(method.proto.takes.iter().cloned())));
        #[cfg(not(target_os = "android"))]
        let fields = fields_of(classes);
        #[cfg(not(target_os = "android"))]
        types.extend(package_types(classes, &methods, &fields));
        let protos: BTreeSet<Proto> = methods.iter().map(|method| method.proto.clone()).collect();
        let mut strings: BTreeSet<String> = types.iter().cloned().collect();
        strings.extend(methods.iter().map(|method| method.name.clone()));
        strings.extend(protos.iter().map(Proto::shorty));
        #[cfg(not(target_os = "android"))]
        strings.extend(package_strings(classes, &fields));
        // Sorted by string, type indices follow string order, so the protos' and methods' own
        // orders (by type and string index) are their derived orders.
        let methods = Pools::sorted_methods(methods, &strings.iter().cloned().collect::<Vec<_>>(), &types.iter().cloned().collect::<Vec<_>>(), &protos.iter().cloned().collect::<Vec<_>>());
        Pools {
            strings: strings.into_iter().collect(),
            types: types.into_iter().collect(),
            protos: protos.into_iter().collect(),
            #[cfg(not(target_os = "android"))]
            fields,
            methods,
        }
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
    /// A field's index. (By class, name and type, the order they are kept in: a string's index
    /// follows its order.)
    #[cfg(not(target_os = "android"))]
    fn field(&self, kept: &Kept) -> u32 {
        self.fields.binary_search(kept).unwrap_or(0) as u32
    }
    /// The index an instruction refers to.
    #[cfg(not(target_os = "android"))]
    fn index(&self, of: &Ref) -> u16 {
        let index = match of {
            Ref::Method(..) => called(of).map_or(0, |one| self.method(&one.class, &one.name, &one.proto)),
            Ref::Field(..) => kept(of).map_or(0, |one| self.field(&one)),
            Ref::Type(kind) => self.type_of(kind),
            Ref::Text(text) => self.string(text),
        };
        index as u16
    }
}

/// The types the package's classes add: interfaces, the classes of methods called, fields' and
/// those code names.
#[cfg(not(target_os = "android"))]
fn package_types(classes: &[Class], methods: &[Called], fields: &[Kept]) -> Vec<String> {
    let interfaces = classes.iter().flat_map(|class| class.interfaces.iter().map(|one| format!("L{one};")));
    let called = methods.iter().map(|method| method.class.clone());
    let kept = fields.iter().flat_map(|field| [field.class.clone(), field.kind.clone()]);
    let named = references(classes).into_iter().filter_map(|one| if let Ref::Type(kind) = one { Some(kind.to_string()) } else { None });
    interfaces.chain(called).chain(kept).chain(named).collect()
}

/// The strings the package's classes add: fields' names and the texts code uses.
#[cfg(not(target_os = "android"))]
fn package_strings(classes: &[Class], fields: &[Kept]) -> Vec<String> {
    let texts = references(classes).into_iter().filter_map(|one| if let Ref::Text(text) = one { Some(text.to_string()) } else { None });
    fields.iter().map(|field| field.name.clone()).chain(texts).collect()
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

/// Where each part of the data section landed: type lists (the protos' then the interfaces'),
/// code (each class's constructor, then its methods), strings and class data.
struct Placed {
    lists: Vec<u32>,
    interfaces: Vec<u32>,
    codes: Vec<u32>,
    bodies: Vec<Vec<u32>>,
    strings: Vec<u32>,
    data: Vec<u32>,
}

/// The DEX file holding `classes`.
pub(crate) fn dex(classes: &[Class]) -> Vec<u8> {
    let pools = Pools::of(classes);
    #[cfg(not(target_os = "android"))]
    let f = pools.fields.len();
    #[cfg(target_os = "android")]
    let f = 0;
    let (s, t, p, m, c) = (pools.strings.len(), pools.types.len(), pools.protos.len(), pools.methods.len(), classes.len());
    let fields_at = HEADER_SIZE + s * 4 + t * 4 + p * 12;
    let ids = [HEADER_SIZE, HEADER_SIZE + s * 4, HEADER_SIZE + s * 4 + t * 4, fields_at + f * 8, fields_at + f * 8 + m * 8];
    let data_at = ids[4] + c * 32;
    let mut out = Out { bytes: vec![0; data_at] };
    let placed = data(&mut out, &pools, classes);
    out.align();
    let map_at = out.at();
    write_map(&mut out, [s, t, p, m, c], ids, (f, fields_at), &placed, map_at);
    write_ids(&mut out, &pools, ids, &placed);
    #[cfg(not(target_os = "android"))]
    write_field_ids(&mut out, &pools, fields_at);
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
        f as u32,
        if f > 0 { fields_at as u32 } else { 0 },
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

/// A list of types at the end of the data written so far: its place.
#[cfg(not(target_os = "android"))]
fn write_list(out: &mut Out, pools: &Pools, types: &[String]) -> u32 {
    out.align();
    let at = out.at();
    out.u32(types.len() as u32);
    types.iter().for_each(|one| out.u16(pools.type_of(one) as u16));
    at
}

/// The data section: parameter and interface lists, code, string data and class data.
fn data(out: &mut Out, pools: &Pools, classes: &[Class]) -> Placed {
    let mut placed = Placed { lists: Vec::new(), interfaces: Vec::new(), codes: Vec::new(), bodies: Vec::new(), strings: Vec::new(), data: Vec::new() };
    for proto in &pools.protos {
        out.align();
        placed.lists.push(if proto.takes.is_empty() { 0 } else { out.at() });
        if proto.takes.is_empty() {
            continue;
        }
        out.u32(proto.takes.len() as u32);
        proto.takes.iter().for_each(|one| out.u16(pools.type_of(one) as u16));
    }
    #[cfg(not(target_os = "android"))]
    for class in classes {
        let named: Vec<String> = class.interfaces.iter().map(|one| format!("L{one};")).collect();
        let at = if named.is_empty() { 0 } else { write_list(out, pools, &named) };
        placed.interfaces.push(at);
    }
    for class in classes {
        out.align();
        placed.codes.push(out.at());
        write_constructor(out, pools, class);
        #[cfg(not(target_os = "android"))]
        placed.bodies.push(class.methods.iter().map(|method| write_code(out, pools, method)).collect());
    }
    for text in &pools.strings {
        placed.strings.push(out.at());
        out.uleb(text.encode_utf16().count() as u32);
        out.bytes.extend(text.as_bytes());
        out.bytes.push(0);
    }
    for class in classes {
        placed.data.push(out.at());
        let n = placed.data.len() - 1;
        #[cfg(not(target_os = "android"))]
        let (fields, coded) = package_members(pools, class, &placed.bodies[n]);
        #[cfg(target_os = "android")]
        let (fields, coded) = (Vec::new(), Vec::new());
        write_class_data(out, pools, class, placed.codes[n], &fields, &coded);
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

/// A method's code item, aligned: its place.
#[cfg(not(target_os = "android"))]
fn write_code(out: &mut Out, pools: &Pools, method: &Method) -> u32 {
    let mut units = Vec::new();
    let mut outs = 0;
    for op in method.code {
        match op {
            Op::Units(some) => units.extend_from_slice(some),
            Op::With(first, to) => units.extend([*first, pools.index(to)]),
            Op::Invoke(code, to, registers) => {
                let count = registers.len() as u16;
                let fifth = registers.get(4).map_or(0, |one| u16::from(*one));
                let low = registers.iter().take(4).enumerate().fold(0u16, |nibbles, (n, one)| nibbles | u16::from(*one) << (n * 4));
                units.extend([u16::from(*code) | count << 12 | fifth << 8, pools.index(to), low]);
                outs = outs.max(count);
            }
        }
    }
    out.align();
    let at = out.at();
    let ins = proto_of(method.descriptor).words() as u16 + 1;
    [method.registers, ins, outs, 0].into_iter().for_each(|value| out.u16(value));
    out.u32(0);
    out.u32(units.len() as u32);
    units.into_iter().for_each(|unit| out.u16(unit));
    at
}

/// A class's fields (by index, sorted), then its methods: its constructor (direct), then its
/// natives and methods with code (`coded`: by index, with where the code is) (virtual), in index
/// order.
fn write_class_data(out: &mut Out, pools: &Pools, class: &Class, code: u32, fields: &[u32], coded: &[(u32, u32)]) {
    let name = format!("L{};", class.name);
    let made = Proto { returns: "V".into(), takes: class.made_with.iter().map(|one| one.to_string()).collect() };
    let mut virtual_ones: Vec<u32> = class.natives.iter().map(|(called, descriptor)| pools.method(&name, called, &proto_of(descriptor))).collect();
    virtual_ones.extend(coded.iter().map(|(method, _)| *method));
    virtual_ones.sort_unstable();
    [0, fields.len() as u32, 1, virtual_ones.len() as u32].into_iter().for_each(|count| out.uleb(count));
    let mut last = 0;
    for field in fields.iter().copied() {
        out.uleb(field - last);
        out.uleb(PUBLIC);
        last = field;
    }
    out.uleb(pools.method(&name, "<init>", &made));
    out.uleb(PUBLIC | CONSTRUCTOR);
    out.uleb(code);
    let mut last = 0;
    for method in virtual_ones {
        let code = coded.iter().find(|(one, _)| *one == method).map(|(_, at)| *at);
        out.uleb(method - last);
        out.uleb(if code.is_some() { PUBLIC } else { PUBLIC | NATIVE });
        out.uleb(code.unwrap_or(0));
        last = method;
    }
}

/// A package class's fields (by index, sorted), and each of its methods with code (by index)
/// with where its code is.
#[cfg(not(target_os = "android"))]
fn package_members(pools: &Pools, class: &Class, bodies: &[u32]) -> (Vec<u32>, Vec<(u32, u32)>) {
    let name = format!("L{};", class.name);
    let mut fields: Vec<u32> = class.fields.iter().map(|(called, kind)| pools.field(&Kept { class: name.clone(), name: called.to_string(), kind: kind.to_string() })).collect();
    fields.sort_unstable();
    let coded = class.methods.iter().zip(bodies).map(|(method, at)| (pools.method(&name, method.name, &proto_of(method.descriptor)), *at)).collect();
    (fields, coded)
}

/// The map of every section, by place.
fn write_map(out: &mut Out, [s, t, p, m, c]: [usize; 5], ids: [usize; 5], (f, fields_at): (usize, usize), placed: &Placed, map_at: u32) {
    let lists: Vec<u32> = placed.lists.iter().chain(&placed.interfaces).copied().filter(|at| *at != 0).collect();
    let codes = c + placed.bodies.iter().map(Vec::len).sum::<usize>();
    let mut items = vec![(HEADER, 1, 0), (STRING_IDS, s, ids[0] as u32), (TYPE_IDS, t, ids[1] as u32), (PROTO_IDS, p, ids[2] as u32), (FIELD_IDS, f, fields_at as u32)];
    items.extend([(METHOD_IDS, m, ids[3] as u32), (CLASS_DEFS, c, ids[4] as u32)]);
    items.extend(lists.iter().min().map(|first| (TYPE_LISTS, lists.len(), *first)));
    items.extend([(CODE, codes, placed.codes[0]), (STRING_DATA, s, placed.strings[0]), (CLASS_DATA, c, placed.data[0]), (MAP, 1, map_at)]);
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

/// The field ids, from `fields_at`.
#[cfg(not(target_os = "android"))]
fn write_field_ids(out: &mut Out, pools: &Pools, fields_at: usize) {
    for (n, field) in pools.fields.iter().enumerate() {
        let (at, class, kind) = (fields_at + n * 8, pools.type_of(&field.class), pools.type_of(&field.kind));
        out.bytes[at..at + 4].copy_from_slice(&[class as u8, (class >> 8) as u8, kind as u8, (kind >> 8) as u8]);
        out.set(at + 4, pools.string(&field.name));
    }
}

/// The class definitions.
fn write_classes(out: &mut Out, pools: &Pools, classes: &[Class], first: usize, placed: &Placed) {
    for (n, class) in classes.iter().enumerate() {
        let fields = [pools.type_of(&format!("L{};", class.name)), PUBLIC, pools.type_of(&format!("L{};", class.parent)), placed.interfaces.get(n).copied().unwrap_or(0), NONE, 0, placed.data[n], 0];
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

// ---- the package's own classes ----

/// Dalvik's instructions, as the methods of `APP` use them.
#[cfg(not(target_os = "android"))]
pub(crate) mod opcodes {
    pub const RETURN_VOID: u16 = 0x0e;
    pub const CONST_STRING: u8 = 0x1a;
    pub const NEW_INSTANCE: u8 = 0x22;
    pub const SGET_OBJECT: u8 = 0x62;
    pub const VIRTUAL: u8 = 0x6e;
    pub const SUPER: u8 = 0x6f;
    pub const DIRECT: u8 = 0x70;
    pub const STATIC: u8 = 0x71;
    pub const INTERFACE: u8 = 0x72;
    /// An instruction of format 11x or 21c: the opcode and register `a`.
    pub const fn op(code: u8, a: u8) -> u16 {
        code as u16 | (a as u16) << 8
    }
    /// An instruction of format 12x, 11n or 22c: the opcode and registers (or values) `a`, `b`.
    pub const fn op2(code: u8, a: u8, b: u8) -> u16 {
        code as u16 | (a as u16) << 8 | (b as u16) << 12
    }
    pub const fn const4(register: u8, value: u8) -> u16 {
        op2(0x12, register, value)
    }
    pub const fn move_result(register: u8) -> u16 {
        op(0x0a, register)
    }
    pub const fn move_result_wide(register: u8) -> u16 {
        op(0x0b, register)
    }
    pub const fn move_result_object(register: u8) -> u16 {
        op(0x0c, register)
    }
    pub const fn return_int(register: u8) -> u16 {
        op(0x0f, register)
    }
    pub const fn return_object(register: u8) -> u16 {
        op(0x11, register)
    }
    /// `iput` and `iput-object`: register `value` into a field of `object`.
    pub const fn iput(value: u8, object: u8) -> u16 {
        op2(0x59, value, object)
    }
    pub const fn iput_object(value: u8, object: u8) -> u16 {
        op2(0x5b, value, object)
    }
    /// `new-array` of `size` elements into `into`.
    pub const fn new_array(into: u8, size: u8) -> u16 {
        op2(0x23, into, size)
    }
}

#[cfg(not(target_os = "android"))]
use opcodes::*;

/// What nothing comes with: a class that extends `java/lang/Object` and has nothing of its own.
#[cfg(not(target_os = "android"))]
pub(crate) const BARE: Class<'static> = Class { name: "", parent: "java/lang/Object", made_with: &[], natives: &[], interfaces: &[], fields: &[], methods: &[] };

/// The classes of the package's own `classes.dex`, which `examples/apk.rs` writes:
///
/// - `taut.Activity`, the app's activity: a `NativeActivity` that keeps what the system answers
///   (the result of an activity it started, of a permission it asked for, and the intent that
///   brought it to the front) in fields the library reads;
/// - `taut.Files`, a content provider handing out the app's files by address (read only, to
///   whom the library grants an address);
/// - the listeners the system parts hand Android, whose natives the library registers the
///   first time the part is used: drops and drags (`taut.Drops`, `taut.Shadow`), the spell
///   checker (`taut.Spelling`), and printing (`taut.Pages`).
#[cfg(not(target_os = "android"))]
pub(crate) const APP: [Class; 6] = [
    Class {
        name: "taut/Activity",
        parent: "android/app/NativeActivity",
        fields: &[("request", "I"), ("result", "I"), ("answer", "Landroid/content/Intent;"), ("permitted", "I")],
        methods: &ACTIVITY,
        ..BARE
    },
    Class { name: "taut/Files", parent: "android/content/ContentProvider", methods: &FILES, ..BARE },
    Class { name: "taut/Drops", interfaces: &["android/view/View$OnDragListener"], natives: &[("onDrag", "(Landroid/view/View;Landroid/view/DragEvent;)Z")], ..BARE },
    Class {
        name: "taut/Shadow",
        parent: "android/view/View$DragShadowBuilder",
        natives: &[("onProvideShadowMetrics", "(Landroid/graphics/Point;Landroid/graphics/Point;)V"), ("onDrawShadow", "(Landroid/graphics/Canvas;)V")],
        ..BARE
    },
    Class {
        name: "taut/Spelling",
        interfaces: &["android/view/textservice/SpellCheckerSession$SpellCheckerSessionListener", "java/lang/Runnable"],
        natives: &[("onGetSuggestions", "([Landroid/view/textservice/SuggestionsInfo;)V"), ("onGetSentenceSuggestions", "([Landroid/view/textservice/SentenceSuggestionsInfo;)V"), ("run", "()V")],
        ..BARE
    },
    Class {
        name: "taut/Pages",
        parent: "android/print/PrintDocumentAdapter",
        natives: &[
            (
                "onLayout",
                "(Landroid/print/PrintAttributes;Landroid/print/PrintAttributes;Landroid/os/CancellationSignal;Landroid/print/PrintDocumentAdapter$LayoutResultCallback;Landroid/os/Bundle;)V",
            ),
            ("onWrite", "([Landroid/print/PageRange;Landroid/os/ParcelFileDescriptor;Landroid/os/CancellationSignal;Landroid/print/PrintDocumentAdapter$WriteResultCallback;)V"),
            ("onFinish", "()V"),
        ],
        ..BARE
    },
];

/// The activity's methods: what the system answers is kept in its fields.
#[cfg(not(target_os = "android"))]
const ACTIVITY: [Method; 3] = [
    Method {
        name: "onNewIntent",
        descriptor: "(Landroid/content/Intent;)V",
        registers: 2,
        code: &[
            Op::Invoke(SUPER, Ref::Method("android/app/NativeActivity", "onNewIntent", "(Landroid/content/Intent;)V"), &[0, 1]),
            Op::Invoke(VIRTUAL, Ref::Method("android/app/Activity", "setIntent", "(Landroid/content/Intent;)V"), &[0, 1]),
            Op::Units(&[RETURN_VOID]),
        ],
    },
    Method {
        name: "onActivityResult",
        descriptor: "(IILandroid/content/Intent;)V",
        registers: 4,
        code: &[
            Op::With(iput(2, 0), Ref::Field("taut/Activity", "result", "I")),
            Op::With(iput_object(3, 0), Ref::Field("taut/Activity", "answer", "Landroid/content/Intent;")),
            Op::With(iput(1, 0), Ref::Field("taut/Activity", "request", "I")),
            Op::Units(&[RETURN_VOID]),
        ],
    },
    Method {
        name: "onRequestPermissionsResult",
        descriptor: "(I[Ljava/lang/String;[I)V",
        registers: 4,
        code: &[Op::With(iput(1, 0), Ref::Field("taut/Activity", "permitted", "I")), Op::Units(&[RETURN_VOID])],
    },
];

/// The provider's methods: a file by its address, read only; its name and size; its type by
/// its ending. Nothing is inserted, deleted or changed.
#[cfg(not(target_os = "android"))]
const FILES: [Method; 7] = [
    Method { name: "onCreate", descriptor: "()Z", registers: 2, code: &[Op::Units(&[const4(0, 1), return_int(0)])] },
    Method {
        name: "getType",
        descriptor: "(Landroid/net/Uri;)Ljava/lang/String;",
        registers: 5,
        code: &[
            Op::Invoke(VIRTUAL, Ref::Method("android/net/Uri", "getPath", "()Ljava/lang/String;"), &[4]),
            Op::Units(&[move_result_object(0), op(0x13, 1), b'.' as u16]),
            Op::Invoke(VIRTUAL, Ref::Method("java/lang/String", "lastIndexOf", "(I)I"), &[0, 1]),
            Op::Units(&[move_result(1), op(0xd8, 1), 1 << 8 | 1]),
            Op::Invoke(VIRTUAL, Ref::Method("java/lang/String", "substring", "(I)Ljava/lang/String;"), &[0, 1]),
            Op::Units(&[move_result_object(0)]),
            Op::With(op(SGET_OBJECT, 2), Ref::Field("java/util/Locale", "ROOT", "Ljava/util/Locale;")),
            Op::Invoke(VIRTUAL, Ref::Method("java/lang/String", "toLowerCase", "(Ljava/util/Locale;)Ljava/lang/String;"), &[0, 2]),
            Op::Units(&[move_result_object(0)]),
            Op::Invoke(STATIC, Ref::Method("android/webkit/MimeTypeMap", "getSingleton", "()Landroid/webkit/MimeTypeMap;"), &[]),
            Op::Units(&[move_result_object(1)]),
            Op::Invoke(VIRTUAL, Ref::Method("android/webkit/MimeTypeMap", "getMimeTypeFromExtension", "(Ljava/lang/String;)Ljava/lang/String;"), &[1, 0]),
            Op::Units(&[move_result_object(0), return_object(0)]),
        ],
    },
    Method {
        name: "openFile",
        descriptor: "(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
        registers: 6,
        code: &[
            Op::Invoke(VIRTUAL, Ref::Method("android/net/Uri", "getPath", "()Ljava/lang/String;"), &[4]),
            Op::Units(&[move_result_object(0)]),
            Op::With(op(NEW_INSTANCE, 1), Ref::Type("Ljava/io/File;")),
            Op::Invoke(DIRECT, Ref::Method("java/io/File", "<init>", "(Ljava/lang/String;)V"), &[1, 0]),
            // `MODE_READ_ONLY`, 0x1000_0000.
            Op::Units(&[op(0x15, 2), 0x1000]),
            Op::Invoke(STATIC, Ref::Method("android/os/ParcelFileDescriptor", "open", "(Ljava/io/File;I)Landroid/os/ParcelFileDescriptor;"), &[1, 2]),
            Op::Units(&[move_result_object(0), return_object(0)]),
        ],
    },
    Method {
        name: "query",
        descriptor: "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
        registers: 12,
        code: &[
            Op::Invoke(VIRTUAL, Ref::Method("android/net/Uri", "getPath", "()Ljava/lang/String;"), &[7]),
            Op::Units(&[move_result_object(0)]),
            Op::With(op(NEW_INSTANCE, 1), Ref::Type("Ljava/io/File;")),
            Op::Invoke(DIRECT, Ref::Method("java/io/File", "<init>", "(Ljava/lang/String;)V"), &[1, 0]),
            // The columns: `_display_name`, `_size`.
            Op::Units(&[const4(2, 2)]),
            Op::With(new_array(3, 2), Ref::Type("[Ljava/lang/String;")),
            Op::Units(&[const4(4, 0)]),
            Op::With(op(CONST_STRING, 5), Ref::Text("_display_name")),
            Op::Units(&[op(0x4d, 5), 4 << 8 | 3, const4(4, 1)]),
            Op::With(op(CONST_STRING, 5), Ref::Text("_size")),
            Op::Units(&[op(0x4d, 5), 4 << 8 | 3]),
            Op::With(op(NEW_INSTANCE, 0), Ref::Type("Landroid/database/MatrixCursor;")),
            Op::Invoke(DIRECT, Ref::Method("android/database/MatrixCursor", "<init>", "([Ljava/lang/String;)V"), &[0, 3]),
            // The one row: the file's name and length.
            Op::With(new_array(3, 2), Ref::Type("[Ljava/lang/Object;")),
            Op::Invoke(VIRTUAL, Ref::Method("java/io/File", "getName", "()Ljava/lang/String;"), &[1]),
            Op::Units(&[move_result_object(5), const4(4, 0), op(0x4d, 5), 4 << 8 | 3]),
            Op::Invoke(VIRTUAL, Ref::Method("java/io/File", "length", "()J"), &[1]),
            Op::Units(&[move_result_wide(4)]),
            Op::Invoke(STATIC, Ref::Method("java/lang/Long", "valueOf", "(J)Ljava/lang/Long;"), &[4, 5]),
            Op::Units(&[move_result_object(5), const4(4, 1), op(0x4d, 5), 4 << 8 | 3]),
            Op::Invoke(VIRTUAL, Ref::Method("android/database/MatrixCursor", "addRow", "([Ljava/lang/Object;)V"), &[0, 3]),
            Op::Units(&[return_object(0)]),
        ],
    },
    Method { name: "insert", descriptor: "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;", registers: 4, code: &[Op::Units(&[const4(0, 0), return_object(0)])] },
    Method { name: "delete", descriptor: "(Landroid/net/Uri;Ljava/lang/String;[Ljava/lang/String;)I", registers: 5, code: &[Op::Units(&[const4(0, 0), return_int(0)])] },
    Method { name: "update", descriptor: "(Landroid/net/Uri;Landroid/content/ContentValues;Ljava/lang/String;[Ljava/lang/String;)I", registers: 6, code: &[Op::Units(&[const4(0, 0), return_int(0)])] },
];

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;

    const PROVIDER: Class = Class {
        name: "taut/Nodes",
        parent: "android/view/accessibility/AccessibilityNodeProvider",
        natives: &[("createAccessibilityNodeInfo", "(I)Landroid/view/accessibility/AccessibilityNodeInfo;"), ("performAction", "(IILandroid/os/Bundle;)Z")],
        ..BARE
    };
    const VIEW: Class = Class { name: "taut/Host", parent: "android/view/View", made_with: &["Landroid/content/Context;"], natives: &[("onCheckIsTextEditor", "()Z")], ..BARE };

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

    #[test]
    fn a_method_with_code_has_its_registers_and_resolved_references() {
        let file = dex(&APP);
        let pools = Pools::of(&APP);
        let field = pools.field(&Kept { class: "Ltaut/Activity;".into(), name: "permitted".into(), kind: "I".into() });
        // `onRequestPermissionsResult`: four registers, four in, none out; `iput v1, v0, permitted`.
        let header = [4u16, 4, 0, 0, 0, 0, 3, 0, 0x0159, field as u16, 0x000e];
        let bytes: Vec<u8> = header.iter().flat_map(|unit| unit.to_le_bytes()).collect();
        assert!(file.windows(bytes.len()).any(|window| window == bytes), "the method's code item");
        // Fields are sorted by class, then name.
        let names: Vec<&str> = pools.fields.iter().filter(|one| one.class == "Ltaut/Activity;").map(|one| one.name.as_str()).collect();
        assert_eq!(names, ["answer", "permitted", "request", "result"]);
    }

    /// The SDK's own `dexdump`, where the SDK is here (`ANDROID_HOME`): what it says of `file`.
    fn dexdump(file: &[u8]) -> Option<String> {
        let tool = std::env::var_os("ANDROID_HOME").and_then(|home| {
            let tools = std::path::Path::new(&home).join("build-tools");
            let newest = std::fs::read_dir(tools).ok()?.filter_map(Result::ok).map(|entry| entry.path()).max()?;
            [newest.join("dexdump"), newest.join("dexdump.exe")].into_iter().find(|path| path.exists())
        })?;
        let path = std::env::temp_dir().join(format!("taut-dex-{}-{}.dex", std::process::id(), file.len()));
        std::fs::write(&path, file).unwrap();
        let said = std::process::Command::new(tool).arg("-d").arg(&path).output().unwrap();
        let _ = std::fs::remove_file(&path);
        let text = String::from_utf8_lossy(&said.stdout).into_owned() + &String::from_utf8_lossy(&said.stderr);
        assert!(said.status.success() && !text.contains("ERROR") && !text.contains("Failure"), "{text}");
        Some(text)
    }

    #[test]
    fn the_sdks_dexdump_reads_the_file() {
        let Some(text) = dexdump(&dex(&[PROVIDER, VIEW])) else { return eprintln!("no Android SDK here") };
        assert!(text.contains("Class descriptor  : 'Ltaut/Nodes;'") && text.contains("Superclass        : 'Landroid/view/View;'"), "{text}");
        assert!(text.contains("invoke-direct {v0, v1}, Landroid/view/View;.<init>:(Landroid/content/Context;)V"), "{text}");
        assert!(text.contains("name          : 'performAction'") && text.contains("access        : 0x0101 (PUBLIC NATIVE)"), "{text}");
    }

    #[test]
    fn the_sdks_dexdump_reads_the_packages_classes() {
        let Some(text) = dexdump(&dex(&APP)) else { return eprintln!("no Android SDK here") };
        for said in [
            "Superclass        : 'Landroid/app/NativeActivity;'",
            "#0              : 'Landroid/view/View$OnDragListener;'",
            "name          : 'permitted'",
            "iput-object v3, v0, Ltaut/Activity;.answer:Landroid/content/Intent;",
            "invoke-super {v0, v1}, Landroid/app/NativeActivity;.onNewIntent:(Landroid/content/Intent;)V",
            "invoke-static {v1, v2}, Landroid/os/ParcelFileDescriptor;.open:(Ljava/io/File;I)Landroid/os/ParcelFileDescriptor;",
            "const-string v5, \"_display_name\"",
            "aput-object v5, v3, v4",
            "add-int/lit8 v1, v1, #int 1",
            "sget-object v2, Ljava/util/Locale;.ROOT:Ljava/util/Locale;",
            "invoke-static {v4, v5}, Ljava/lang/Long;.valueOf:(J)Ljava/lang/Long;",
        ] {
            assert!(text.contains(said), "{said}\n{text}");
        }
    }
}
