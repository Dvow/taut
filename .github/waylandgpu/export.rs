// Probe: can this system's Vulkan device export its memory as a dmabuf (what
// zwp_linux_dmabuf_v1 takes)? Prints what each step says. Build: rustc -O dmabuf.rs
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr::{null, null_mut};

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(lib: *mut c_void, name: *const c_char) -> *mut c_void;
    fn readlink(path: *const c_char, buf: *mut c_char, size: usize) -> isize;
}

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
struct ExtBufferInfo { kind: u32, next: *const c_void, flags: u32, usage: u32, handle: u32 }
#[repr(C)]
#[derive(Default)]
struct ExtBufferProps { kind: u32, next: usize, features: u32, export_from: u32, compatible: u32 }
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

fn main() {
    unsafe {
        let lib = dlopen(c"libvulkan.so.1".as_ptr(), 2);
        assert!(!lib.is_null(), "no loader");
        let get: Get = std::mem::transmute(dlsym(lib, c"vkGetInstanceProcAddr".as_ptr()));
        let f = |instance: *mut c_void, name: &CStr| -> *mut c_void { get(instance, name.as_ptr()) };
        let create: unsafe extern "C" fn(*const InstanceInfo, *const c_void, *mut *mut c_void) -> i32 = std::mem::transmute(f(null_mut(), c"vkCreateInstance"));
        let app = App { kind: 0, next: null(), name: null(), version: 0, engine: null(), engine_version: 0, api: (1 << 22) | (1 << 12) };
        let info = InstanceInfo { kind: 1, next: null(), flags: 0, app: &app, layers: 0, layer_names: null(), exts: 0, ext_names: null() };
        let mut instance = null_mut();
        println!("vkCreateInstance {}", create(&info, null(), &mut instance));
        let enumerate: unsafe extern "C" fn(*mut c_void, *mut u32, *mut *mut c_void) -> i32 = std::mem::transmute(f(instance, c"vkEnumeratePhysicalDevices"));
        let mut count = 1u32;
        let mut physical = null_mut();
        println!("devices {} count {count}", enumerate(instance, &mut count, &mut physical));
        let external: unsafe extern "C" fn(*mut c_void, *const ExtBufferInfo, *mut ExtBufferProps) = std::mem::transmute(f(instance, c"vkGetPhysicalDeviceExternalBufferProperties"));
        for (name, handle) in [("dmabuf", 0x200u32), ("opaque fd", 0x1)] {
            let asked = ExtBufferInfo { kind: 1000071002, next: null(), flags: 0, usage: 0x1, handle };
            let mut props = ExtBufferProps { kind: 1000071003, ..Default::default() };
            external(physical, &asked, &mut props);
            println!("{name}: features {:#x} (exportable 0x2, importable 0x4), compatible {:#x}", props.features, props.compatible);
        }
        let make: unsafe extern "C" fn(*mut c_void, *const DeviceInfo, *const c_void, *mut *mut c_void) -> i32 = std::mem::transmute(f(instance, c"vkCreateDevice"));
        let exts = [c"VK_KHR_external_memory_fd".as_ptr(), c"VK_EXT_external_memory_dma_buf".as_ptr()];
        let priority = 1.0f32;
        let queue = QueueInfo { kind: 2, next: null(), flags: 0, family: 0, count: 1, priorities: &priority };
        let dinfo = DeviceInfo { kind: 3, next: null(), flags: 0, queues: 1, queue_infos: &queue, layers: 0, layer_names: null(), exts: 2, ext_names: exts.as_ptr(), features: null() };
        let mut device = null_mut();
        println!("vkCreateDevice {}", make(physical, &dinfo, null(), &mut device));
        let dget: Get = std::mem::transmute(f(instance, c"vkGetDeviceProcAddr"));
        let d = |name: &CStr| -> *mut c_void { dget(device, name.as_ptr()) };
        let create_buffer: unsafe extern "C" fn(*mut c_void, *const BufferInfo, *const c_void, *mut u64) -> i32 = std::mem::transmute(d(c"vkCreateBuffer"));
        let needs: unsafe extern "C" fn(*mut c_void, u64, *mut Needs) = std::mem::transmute(d(c"vkGetBufferMemoryRequirements"));
        let alloc: unsafe extern "C" fn(*mut c_void, *const Alloc, *const c_void, *mut u64) -> i32 = std::mem::transmute(d(c"vkAllocateMemory"));
        let get_fd: unsafe extern "C" fn(*mut c_void, *const GetFd, *mut c_int) -> i32 = std::mem::transmute(d(c"vkGetMemoryFdKHR"));
        for (name, handle) in [("dmabuf", 0x200u32), ("opaque fd", 0x1)] {
            let ext = ExtMemBuffer { kind: 1000072000, next: null(), handles: handle };
            let binfo = BufferInfo { kind: 12, next: (&raw const ext).cast(), flags: 0, size: 1 << 20, usage: 0x1, sharing: 0, families: 0, family_list: null() };
            let mut buffer = 0;
            let said = create_buffer(device, &binfo, null(), &mut buffer);
            let mut need = Needs::default();
            needs(device, buffer, &mut need);
            let export = Export { kind: 1000072002, next: null(), handles: handle };
            let index = need.bits.trailing_zeros();
            let ainfo = Alloc { kind: 5, next: (&raw const export).cast(), size: need.size, index };
            let mut memory = 0;
            let allocated = alloc(device, &ainfo, null(), &mut memory);
            let mut fd = -1;
            let asked = GetFd { kind: 1000074002, next: null(), memory, handle };
            let exported = if allocated == 0 { get_fd(device, &asked, &mut fd) } else { allocated };
            let mut link = [0 as c_char; 256];
            let path = std::ffi::CString::new(format!("/proc/self/fd/{fd}")).unwrap();
            let n = readlink(path.as_ptr(), link.as_mut_ptr(), 255);
            let what = if n > 0 { CStr::from_ptr(link.as_ptr()).to_string_lossy().into_owned() } else { "-".into() };
            println!("{name}: buffer {said}, memory {allocated} (type {index}), fd {exported} -> {fd} = {what}");
        }
    }
}
