// Probe: a Vulkan buffer exported as a dmabuf, handed to the compositor through
// zwp_linux_dmabuf_v1 `create`, shown, and read back off Xvfb. Usage: show (WAYLAND_DISPLAY,
// XDG_RUNTIME_DIR and TAUT_XVFB set by the runner). Prints RESULT lines.
use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::ptr::{null, null_mut};

unsafe extern "C" {
    fn sendmsg(fd: c_int, message: *const Letter, flags: c_int) -> isize;
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(lib: *mut c_void, name: *const c_char) -> *mut c_void;
    fn readlink(path: *const c_char, buf: *mut c_char, size: usize) -> isize;
}

#[repr(C)]
struct Letter { name: *const c_void, name_len: u32, iov: *const [usize; 2], iov_len: usize, control: *const c_void, control_len: usize, flags: c_int }
#[repr(C)]
struct Cmsg { len: usize, level: c_int, kind: c_int, fd: c_int, pad: c_int }

type Get = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;
#[repr(C)]
struct App { kind: u32, next: *const c_void, name: *const c_char, version: u32, engine: *const c_char, engine_version: u32, api: u32 }
#[repr(C)]
struct InstanceInfo { kind: u32, next: *const c_void, flags: u32, app: *const App, layers: u32, layer_names: *const *const c_char, exts: u32, ext_names: *const *const c_char }
#[repr(C)]
struct QueueInfo { kind: u32, next: *const c_void, flags: u32, family: u32, count: u32, priorities: *const f32 }
#[repr(C)]
struct DeviceInfo { kind: u32, next: *const c_void, flags: u32, queues: u32, queue_infos: *const QueueInfo, layers: u32, layer_names: *const *const c_char, exts: u32, ext_names: *const *const c_char, features: *const c_void }
#[repr(C)]
struct ExtMemBuffer { kind: u32, next: *const c_void, handles: u32 }
#[repr(C)]
struct BufferInfo { kind: u32, next: *const c_void, flags: u32, size: u64, usage: u32, sharing: u32, families: u32, family_list: *const u32 }
#[repr(C)]
#[derive(Default)]
struct Needs { size: u64, alignment: u64, bits: u32 }
#[repr(C)]
struct Export { kind: u32, next: *const c_void, handles: u32 }
#[repr(C)]
struct Alloc { kind: u32, next: *const c_void, size: u64, index: u32 }
#[repr(C)]
struct GetFd { kind: u32, next: *const c_void, memory: u64, handle: u32 }

const DMA_BUF: u32 = 0x200;

/// Makes a dmabuf of `size` bytes through Vulkan, fills it with `fill`, and returns its fd.
fn vulkan_dmabuf(size: u64, fill: &dyn Fn(*mut u32)) -> c_int {
    unsafe {
        let lib = dlopen(c"libvulkan.so.1".as_ptr(), 2);
        assert!(!lib.is_null(), "no loader");
        let get: Get = std::mem::transmute(dlsym(lib, c"vkGetInstanceProcAddr".as_ptr()));
        let f = |instance: *mut c_void, name: &CStr| -> *mut c_void { get(instance, name.as_ptr()) };
        let create: unsafe extern "C" fn(*const InstanceInfo, *const c_void, *mut *mut c_void) -> i32 = std::mem::transmute(f(null_mut(), c"vkCreateInstance"));
        let app = App { kind: 0, next: null(), name: null(), version: 0, engine: null(), engine_version: 0, api: (1 << 22) | (1 << 12) };
        let info = InstanceInfo { kind: 1, next: null(), flags: 0, app: &app, layers: 0, layer_names: null(), exts: 0, ext_names: null() };
        let mut instance = null_mut();
        assert_eq!(create(&info, null(), &mut instance), 0);
        let enumerate: unsafe extern "C" fn(*mut c_void, *mut u32, *mut *mut c_void) -> i32 = std::mem::transmute(f(instance, c"vkEnumeratePhysicalDevices"));
        let mut count = 1u32;
        let mut physical = null_mut();
        enumerate(instance, &mut count, &mut physical);
        let memory_properties: unsafe extern "C" fn(*mut c_void, *mut [u32; 130]) = std::mem::transmute(f(instance, c"vkGetPhysicalDeviceMemoryProperties"));
        let mut kinds = [0u32; 130];
        memory_properties(physical, &mut kinds);
        let make: unsafe extern "C" fn(*mut c_void, *const DeviceInfo, *const c_void, *mut *mut c_void) -> i32 = std::mem::transmute(f(instance, c"vkCreateDevice"));
        let exts = [c"VK_KHR_external_memory_fd".as_ptr(), c"VK_EXT_external_memory_dma_buf".as_ptr()];
        let priority = 1.0f32;
        let queue = QueueInfo { kind: 2, next: null(), flags: 0, family: 0, count: 1, priorities: &priority };
        let dinfo = DeviceInfo { kind: 3, next: null(), flags: 0, queues: 1, queue_infos: &queue, layers: 0, layer_names: null(), exts: 2, ext_names: exts.as_ptr(), features: null() };
        let mut device = null_mut();
        assert_eq!(make(physical, &dinfo, null(), &mut device), 0);
        let dget: Get = std::mem::transmute(f(instance, c"vkGetDeviceProcAddr"));
        let d = |name: &CStr| -> *mut c_void { dget(device, name.as_ptr()) };
        let create_buffer: unsafe extern "C" fn(*mut c_void, *const BufferInfo, *const c_void, *mut u64) -> i32 = std::mem::transmute(d(c"vkCreateBuffer"));
        let needs: unsafe extern "C" fn(*mut c_void, u64, *mut Needs) = std::mem::transmute(d(c"vkGetBufferMemoryRequirements"));
        let alloc: unsafe extern "C" fn(*mut c_void, *const Alloc, *const c_void, *mut u64) -> i32 = std::mem::transmute(d(c"vkAllocateMemory"));
        let map: unsafe extern "C" fn(*mut c_void, u64, u64, u64, u32, *mut *mut c_void) -> i32 = std::mem::transmute(d(c"vkMapMemory"));
        let get_fd: unsafe extern "C" fn(*mut c_void, *const GetFd, *mut c_int) -> i32 = std::mem::transmute(d(c"vkGetMemoryFdKHR"));
        let ext = ExtMemBuffer { kind: 1000072000, next: null(), handles: DMA_BUF };
        let binfo = BufferInfo { kind: 12, next: (&raw const ext).cast(), flags: 0, size, usage: 0x2, sharing: 0, families: 0, family_list: null() };
        let mut buffer = 0;
        assert_eq!(create_buffer(device, &binfo, null(), &mut buffer), 0);
        let mut need = Needs::default();
        needs(device, buffer, &mut need);
        // (A kind of memory the buffer may use that the program can write: host visible, 0x2.)
        let index = (0..kinds[0]).find(|&i| need.bits & (1 << i) != 0 && kinds[1 + 2 * i as usize] & 0x2 != 0).expect("no host-visible memory");
        let export = Export { kind: 1000072002, next: null(), handles: DMA_BUF };
        let ainfo = Alloc { kind: 5, next: (&raw const export).cast(), size: need.size, index };
        let mut memory = 0;
        let allocated = alloc(device, &ainfo, null(), &mut memory);
        println!("allocate exportable memory: {allocated}");
        assert_eq!(allocated, 0);
        let mut at = null_mut();
        assert_eq!(map(device, memory, 0, !0, 0, &mut at), 0);
        fill(at.cast());
        let mut fd = -1;
        let asked = GetFd { kind: 1000074002, next: null(), memory, handle: DMA_BUF };
        let exported = get_fd(device, &asked, &mut fd);
        let mut link = [0 as c_char; 256];
        let path = std::ffi::CString::new(format!("/proc/self/fd/{fd}")).unwrap();
        let n = readlink(path.as_ptr(), link.as_mut_ptr(), 255);
        let what = if n > 0 { CStr::from_ptr(link.as_ptr()).to_string_lossy().into_owned() } else { "-".into() };
        println!("vkGetMemoryFdKHR {exported} -> fd {fd} = {what}");
        assert_eq!(exported, 0);
        fd
    }
}

struct W { s: UnixStream, buf: Vec<u8> }

fn s(m: &mut Vec<u8>, t: &str) {
    m.extend(((t.len() + 1) as u32).to_le_bytes());
    m.extend(t.as_bytes());
    m.push(0);
    while m.len() % 4 != 0 { m.push(0); }
}

impl W {
    fn send(&mut self, obj: u32, op: u16, args: &[u8], fd: Option<c_int>) {
        let mut m = obj.to_le_bytes().to_vec();
        m.extend((((8 + args.len()) as u32) << 16 | op as u32).to_le_bytes());
        m.extend(args);
        let iov = [m.as_ptr() as usize, m.len()];
        let c = Cmsg { len: 20, level: 1, kind: 1, fd: fd.unwrap_or(-1), pad: 0 };
        let l = Letter { name: null(), name_len: 0, iov: &iov, iov_len: 1, control: if fd.is_some() { (&raw const c).cast() } else { null() }, control_len: if fd.is_some() { 24 } else { 0 }, flags: 0 };
        let n = unsafe { sendmsg(self.s.as_raw_fd(), &l, 0) };
        assert_eq!(n as usize, m.len());
    }
    fn next(&mut self) -> (u32, u16, Vec<u8>) {
        loop {
            if self.buf.len() >= 8 {
                let obj = u32::from_le_bytes(self.buf[0..4].try_into().unwrap());
                let w = u32::from_le_bytes(self.buf[4..8].try_into().unwrap());
                let len = (w >> 16) as usize;
                if self.buf.len() >= len {
                    let body = self.buf[8..len].to_vec();
                    self.buf.drain(..len);
                    return (obj, (w & 0xffff) as u16, body);
                }
            }
            let mut b = [0u8; 4096];
            let n = self.s.read(&mut b).expect("read");
            assert!(n > 0, "compositor hung up");
            self.buf.extend(&b[..n]);
        }
    }
}

fn u(b: &[u8], i: usize) -> u32 { u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()) }
fn text(b: &[u8], at: usize) -> String { let n = u(b, at) as usize; String::from_utf8_lossy(&b[at * 4 + 4..at * 4 + 3 + n]).into() }

fn main() {
    let path = format!("{}/{}", std::env::var("XDG_RUNTIME_DIR").unwrap(), std::env::var("WAYLAND_DISPLAY").unwrap());
    let mut w = W { s: UnixStream::connect(path).expect("connect"), buf: vec![] };
    w.send(1, 1, &2u32.to_le_bytes(), None); // get_registry -> 2
    w.send(1, 0, &3u32.to_le_bytes(), None); // sync -> 3
    let (mut comp, mut dmabuf, mut wm) = ((0, 0), (0, 0), (0, 0));
    loop {
        let (o, op, b) = w.next();
        if o == 2 && op == 0 {
            let name = u(&b, 0);
            let iface = text(&b, 1);
            let ver = u(&b, b.len() / 4 - 1);
            match iface.as_str() {
                "wl_compositor" => comp = (name, ver),
                "zwp_linux_dmabuf_v1" => dmabuf = (name, ver),
                "xdg_wm_base" => wm = (name, ver),
                _ => {}
            }
        }
        if o == 3 { break; }
    }
    println!("compositor {comp:?} dmabuf {dmabuf:?} xdg {wm:?}");
    if dmabuf.0 == 0 { println!("RESULT no dmabuf protocol"); return; }
    let bind = |w: &mut W, name: u32, iface: &str, ver: u32, id: u32| {
        let mut a = name.to_le_bytes().to_vec();
        s(&mut a, iface);
        a.extend(ver.to_le_bytes());
        a.extend(id.to_le_bytes());
        w.send(2, 0, &a, None);
    };
    bind(&mut w, comp.0, "wl_compositor", 4, 4);
    bind(&mut w, dmabuf.0, "zwp_linux_dmabuf_v1", 3, 5);
    bind(&mut w, wm.0, "xdg_wm_base", 1, 6);
    w.send(1, 0, &7u32.to_le_bytes(), None); // sync -> 7
    let mut formats = vec![];
    loop {
        let (o, op, b) = w.next();
        if o == 5 && op == 1 { formats.push((u(&b, 0), (u(&b, 1) as u64) << 32 | u(&b, 2) as u64)); }
        if o == 1 && op == 0 { println!("RESULT error {}", text(&b, 2)); return; }
        if o == 7 { break; }
    }
    let fourcc = |f: u32| String::from_utf8_lossy(&f.to_le_bytes()).to_string();
    println!("formats: {}", formats.iter().map(|(f, m)| format!("{}:{m:#x}", fourcc(*f))).collect::<Vec<_>>().join(" "));
    // surface 8, xdg_surface 9, toplevel 10
    w.send(4, 0, &8u32.to_le_bytes(), None);
    w.send(6, 2, &[9u32.to_le_bytes(), 8u32.to_le_bytes()].concat(), None);
    w.send(9, 1, &10u32.to_le_bytes(), None);
    w.send(8, 6, &[], None);
    loop {
        let (o, op, b) = w.next();
        if o == 6 && op == 0 { w.send(6, 3, &b, None); }
        if o == 9 && op == 0 { w.send(9, 4, &b, None); break; }
        if o == 1 && op == 0 { println!("RESULT error {}", text(&b, 2)); return; }
    }
    let (wd, ht) = (256u32, 128u32);
    let stride = wd * 4;
    let fd = vulkan_dmabuf((stride * ht) as u64, &|p| {
        for i in 0..(wd * ht) as usize { unsafe { *p.add(i) = if i < (wd * ht / 2) as usize { 0xffff0000 } else { 0xff0000ff } }; }
    });
    let xr = u32::from_le_bytes(*b"XR24");
    let modifier = 0u64; // linear
    w.send(5, 1, &11u32.to_le_bytes(), None); // create_params -> 11
    let a = [0u32, 0, stride, (modifier >> 32) as u32, modifier as u32].iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>();
    w.send(11, 1, &a, Some(fd));
    let a = [wd, ht, xr, 0].iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>();
    w.send(11, 2, &a, None); // create: answered by created or failed
    w.s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let buffer = loop {
        let (o, op, b) = w.next();
        if o == 1 && op == 0 { println!("RESULT error obj {} code {} {}", u(&b, 0), u(&b, 1), text(&b, 2)); return; }
        if o == 11 && op == 1 { println!("RESULT failed"); return; }
        if o == 11 && op == 0 { break u(&b, 0); }
    };
    println!("created buffer {buffer:#x}");
    w.send(8, 1, &[buffer.to_le_bytes(), 0u32.to_le_bytes(), 0u32.to_le_bytes()].concat(), None);
    w.send(8, 2, &[0i32, 0, wd as i32, ht as i32].iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>(), None);
    w.send(8, 3, &12u32.to_le_bytes(), None); // frame -> 12
    w.send(8, 6, &[], None);
    let start = std::time::Instant::now();
    loop {
        let (o, op, b) = w.next();
        if o == 1 && op == 0 { println!("RESULT error obj {} code {} {}", u(&b, 0), u(&b, 1), text(&b, 2)); return; }
        if o == 12 { println!("frame done ({:?})", start.elapsed()); look(); return; }
    }
}

#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(name: *const i8) -> *mut c_void;
    fn XDefaultRootWindow(d: *mut c_void) -> u64;
    fn XGetImage(d: *mut c_void, w: u64, x: c_int, y: c_int, wd: u32, ht: u32, mask: u64, format: c_int) -> *mut c_void;
    fn XGetPixel(i: *mut c_void, x: c_int, y: c_int) -> u64;
}

fn look() {
    let Ok(name) = std::env::var("TAUT_XVFB") else { println!("RESULT shown (no screen to read)"); return };
    std::thread::sleep(std::time::Duration::from_millis(500));
    let name = std::ffi::CString::new(name).unwrap();
    unsafe {
        let d = XOpenDisplay(name.as_ptr());
        let i = XGetImage(d, XDefaultRootWindow(d), 0, 0, 900, 600, !0, 2);
        let (mut red, mut blue, mut first) = (0, 0, None);
        for y in 0..600 { for x in 0..900 { match XGetPixel(i, x, y) & 0xffffff { 0xff0000 => { red += 1; first.get_or_insert((x, y)); } 0x0000ff => blue += 1, _ => {} } } }
        let exact = red == 16384 && blue == 16384;
        println!("RESULT {} red {red} blue {blue} (want 16384 each), red starts at {first:?}", if exact { "EXACT" } else { "WRONG" });
    }
}
