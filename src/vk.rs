//! Vulkan's definitions, as vulkan_core.h declares them, for what in taut uses Vulkan: the layer
//! over a program's own Vulkan, and windows shown through Vulkan.
//!
//! Nothing is linked. `loader` opens the system's loader (vulkan-1.dll, libvulkan.so.1) and
//! gives its `vkGetInstanceProcAddr`; every other function is found by name through it, into a
//! table each user declares with `functions!`. Structures are declared with `structures!`: each
//! starts all zero, which leaves every pointer null, every flag off and nothing chained.
// Every function here is `#[inline]`: only a program that calls one compiles it. (Compiled in
// the library itself, even unused, this code moved how the rest is grouped and optimised.)
use std::ffi::{CStr, c_char, c_void};

/// A handle that is a number (an image, a buffer, a pipeline): 0 is none.
pub(crate) type Handle = u64;
/// A handle that is a pointer (the instance, a device, a queue, a command buffer).
pub(crate) type Object = *mut c_void;
/// `vkGetInstanceProcAddr`: an instance's functions by name (with no instance, those that make
/// one).
pub(crate) type Find = unsafe extern "system" fn(Object, *const c_char) -> *const c_void;

/// The system's Vulkan loader, opened (and kept open), and its `vkGetInstanceProcAddr`; nothing
/// where the system has none.
#[inline]
pub(crate) fn loader() -> Option<Find> {
    #[cfg(windows)]
    let found = {
        let name: Vec<u16> = "vulkan-1.dll\0".encode_utf16().collect();
        // SAFETY: the names end in a zero; the loader comes from the system folder (0x800:
        // `LOAD_LIBRARY_SEARCH_SYSTEM32`) and is never closed.
        let library = unsafe { LoadLibraryExW(name.as_ptr(), 0, 0x800) };
        if library == 0 { std::ptr::null_mut() } else { unsafe { GetProcAddress(library, c"vkGetInstanceProcAddr".as_ptr().cast()) as *mut c_void } }
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let found = {
        // SAFETY: the name ends in a zero (2: `RTLD_NOW`); the library is never closed.
        let library = unsafe { crate::x11::dlopen(c"libvulkan.so.1".as_ptr(), 2) };
        (!library.is_null()).then(|| crate::x11::symbol(library, "vkGetInstanceProcAddr\0")).flatten().unwrap_or(std::ptr::null_mut())
    };
    // macOS has no Vulkan of its own.
    #[cfg(target_os = "macos")]
    let found = std::ptr::null_mut::<c_void>();
    // SAFETY: the loader's function is declared as vulkan_core.h declares it.
    (!found.is_null()).then(|| unsafe { std::mem::transmute::<*const c_void, Find>(found.cast_const()) })
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: isize, flags: u32) -> isize;
    fn GetProcAddress(library: isize, name: *const u8) -> usize;
}

/// Declares a table of Vulkan's functions, each found by name.
macro_rules! functions {
    ($set:ident { $($name:ident: fn($($arg:ty),*) $(-> $out:ty)?;)* }) => {
        /// Vulkan's functions, found by name.
        #[allow(non_snake_case)]
        pub(crate) struct $set {
            $(pub(crate) $name: unsafe extern "system" fn($($arg),*) $(-> $out)?,)*
        }
        impl $set {
            /// Each function, found through `find`; or the name of the first one missing.
            #[inline]
            pub(crate) fn load(find: &mut dyn FnMut(&std::ffi::CStr) -> *const std::ffi::c_void) -> Result<$set, &'static str> {
                // SAFETY: each function is declared as vulkan_core.h declares it.
                Ok($set { $($name: unsafe { std::mem::transmute::<*const std::ffi::c_void, unsafe extern "system" fn($($arg),*) $(-> $out)?>($crate::vk::found(find, concat!(stringify!($name), "\0"))?) },)* })
            }
        }
    };
}
pub(crate) use functions;

/// The function `name` (ending in a zero) as `find` gives it; or its name, if it gives none.
#[inline]
pub(crate) fn found(find: &mut dyn FnMut(&CStr) -> *const c_void, name: &'static str) -> Result<*const c_void, &'static str> {
    let found = find(CStr::from_bytes_with_nul(name.as_bytes()).unwrap_or_default());
    if found.is_null() { Err(name.trim_end_matches('\0')) } else { Ok(found) }
}

/// Declares structures of Vulkan's, laid out as vulkan_core.h lays them out, each all zero by
/// default.
macro_rules! structures {
    ($($name:ident { $($field:ident: $type:ty,)* })*) => {$(
        #[repr(C)]
        #[derive(Clone, Copy)]
        pub(crate) struct $name {
            $(pub(crate) $field: $type,)*
        }
        impl Default for $name {
            #[inline]
            fn default() -> Self {
                // SAFETY: every field is a number, a pointer or an array of them, for which all
                // zero is a value.
                unsafe { std::mem::zeroed() }
            }
        }
    )*};
}
pub(crate) use structures;

// `kind` is each structure's `sType`, `next` its `pNext`.
structures! {
    VkMemoryType { flags: u32, heap: u32, }
    VkMemoryHeap { size: u64, flags: u32, }
    VkPhysicalDeviceMemoryProperties { type_count: u32, types: [VkMemoryType; 32], heap_count: u32, heaps: [VkMemoryHeap; 16], }
    VkMemoryRequirements { size: u64, alignment: u64, type_bits: u32, }
    VkMemoryAllocateInfo { kind: u32, next: *const c_void, size: u64, type_index: u32, }
    VkBufferCreateInfo { kind: u32, next: *const c_void, flags: u32, size: u64, usage: u32, sharing_mode: u32, queue_family_count: u32, queue_families: *const u32, }
    VkImageCreateInfo { kind: u32, next: *const c_void, flags: u32, image_type: u32, format: u32, extent: [u32; 3], mip_levels: u32, array_layers: u32, samples: u32, tiling: u32, usage: u32, sharing_mode: u32, queue_family_count: u32, queue_families: *const u32, initial_layout: u32, }
    VkImageViewCreateInfo { kind: u32, next: *const c_void, flags: u32, image: Handle, view_type: u32, format: u32, components: [u32; 4], range: [u32; 5], }
    VkImageMemoryBarrier { kind: u32, next: *const c_void, source_access: u32, destination_access: u32, old_layout: u32, new_layout: u32, source_family: u32, destination_family: u32, image: Handle, range: [u32; 5], }
    VkBufferImageCopy { buffer_offset: u64, row_length: u32, image_height: u32, layers: [u32; 4], offset: [i32; 3], extent: [u32; 3], }
    VkCommandPoolCreateInfo { kind: u32, next: *const c_void, flags: u32, family: u32, }
    VkCommandBufferAllocateInfo { kind: u32, next: *const c_void, pool: Handle, level: u32, count: u32, }
    VkCommandBufferBeginInfo { kind: u32, next: *const c_void, flags: u32, inheritance: *const c_void, }
    VkSubmitInfo { kind: u32, next: *const c_void, wait_count: u32, waits: *const Handle, wait_stages: *const u32, buffer_count: u32, buffers: *const Object, signal_count: u32, signals: *const Handle, }
    VkFenceCreateInfo { kind: u32, next: *const c_void, flags: u32, }
    VkViewport { x: f32, y: f32, width: f32, height: f32, min_depth: f32, max_depth: f32, }
    VkRect2D { offset: [i32; 2], extent: [u32; 2], }
}

/// Pixels of four bytes in memory: blue, green, red, alpha, as taut's are.
pub(crate) const B8G8R8A8_UNORM: u32 = 44;
// Image layouts.
pub(crate) const LAYOUT_UNDEFINED: u32 = 0;
pub(crate) const LAYOUT_SHADER_READ_ONLY: u32 = 5;
pub(crate) const LAYOUT_TRANSFER_DESTINATION: u32 = 7;
// Kinds of access.
pub(crate) const ACCESS_SHADER_READ: u32 = 0x20;
pub(crate) const ACCESS_TRANSFER_WRITE: u32 = 0x1000;
// Pipeline stages.
pub(crate) const STAGE_TOP: u32 = 0x1;
pub(crate) const STAGE_FRAGMENT_SHADER: u32 = 0x80;
pub(crate) const STAGE_TRANSFER: u32 = 0x1000;
// Kinds of memory.
pub(crate) const DEVICE_LOCAL: u32 = 0x1;
pub(crate) const HOST_VISIBLE_AND_COHERENT: u32 = 0x2 | 0x4;
/// All of an image's colour: aspect, first level, levels, first layer, layers.
pub(crate) const WHOLE: [u32; 5] = [0x1, 0, 1, 0, 1];

/// The first kind of memory of `properties` that `type_bits` allows and that has all of
/// `wanted`.
#[inline]
pub(crate) fn memory_type(properties: &VkPhysicalDeviceMemoryProperties, type_bits: u32, wanted: u32) -> Option<u32> {
    (0..properties.type_count.min(32)).find(|at| type_bits >> at & 1 == 1 && properties.types[*at as usize].flags & wanted == wanted)
}

/// An image's layout changing, all of its colour, with what was done to it before made
/// visible to what is done after.
#[inline]
pub(crate) fn barrier(image: Handle, old_layout: u32, new_layout: u32, source_access: u32, destination_access: u32) -> VkImageMemoryBarrier {
    VkImageMemoryBarrier { kind: 45, source_access, destination_access, old_layout, new_layout, source_family: !0, destination_family: !0, image, range: WHOLE, ..Default::default() }
}

/// Vulkan's name of a result, as "VK_ERROR_DEVICE_LOST".
#[inline]
pub(crate) fn named(result: i32) -> &'static str {
    match result {
        0 => "VK_SUCCESS",
        1 => "VK_NOT_READY",
        2 => "VK_TIMEOUT",
        -1 => "VK_ERROR_OUT_OF_HOST_MEMORY",
        -2 => "VK_ERROR_OUT_OF_DEVICE_MEMORY",
        -3 => "VK_ERROR_INITIALIZATION_FAILED",
        -4 => "VK_ERROR_DEVICE_LOST",
        -5 => "VK_ERROR_MEMORY_MAP_FAILED",
        -7 => "VK_ERROR_EXTENSION_NOT_PRESENT",
        -8 => "VK_ERROR_FEATURE_NOT_PRESENT",
        -9 => "VK_ERROR_INCOMPATIBLE_DRIVER",
        -10 => "VK_ERROR_TOO_MANY_OBJECTS",
        -11 => "VK_ERROR_FORMAT_NOT_SUPPORTED",
        -13 => "VK_ERROR_UNKNOWN",
        -1000069000 => "VK_ERROR_OUT_OF_POOL_MEMORY",
        -1000000000 => "VK_ERROR_SURFACE_LOST_KHR",
        1000001003 => "VK_SUBOPTIMAL_KHR",
        -1000001004 => "VK_ERROR_OUT_OF_DATE_KHR",
        _ => "an error Vulkan has no name for here",
    }
}

// Windows shown through Vulkan (feature `gpu`: `gpu_vulkan_windows`, `gpu_vulkan_x11`): the
// instance, devices, surfaces and swap chains.
#[cfg(feature = "gpu")]
structures! {
    VkApplicationInfo { kind: u32, next: *const c_void, name: *const c_char, version: u32, engine: *const c_char, engine_version: u32, api_version: u32, }
    VkInstanceCreateInfo { kind: u32, next: *const c_void, flags: u32, application: *const VkApplicationInfo, layer_count: u32, layers: *const *const c_char, extension_count: u32, extensions: *const *const c_char, }
    VkExtensionProperties { name: [c_char; 256], version: u32, }
    VkPhysicalDeviceProperties { api_version: u32, driver_version: u32, vendor: u32, device: u32, device_type: u32, name: [c_char; 256], uuid: [u8; 16], limits: [u64; 63], sparse: [u32; 5], }
    VkQueueFamilyProperties { flags: u32, count: u32, timestamp_bits: u32, granularity: [u32; 3], }
    VkDeviceQueueCreateInfo { kind: u32, next: *const c_void, flags: u32, family: u32, count: u32, priorities: *const f32, }
    VkDeviceCreateInfo { kind: u32, next: *const c_void, flags: u32, queue_count: u32, queues: *const VkDeviceQueueCreateInfo, layer_count: u32, layers: *const *const c_char, extension_count: u32, extensions: *const *const c_char, features: *const c_void, }
    VkSemaphoreCreateInfo { kind: u32, next: *const c_void, flags: u32, }
    VkSurfaceCapabilitiesKHR { min_images: u32, max_images: u32, current: [u32; 2], least: [u32; 2], most: [u32; 2], max_layers: u32, transforms: u32, transform: u32, alphas: u32, usage: u32, }
    VkSurfaceFormatKHR { format: u32, colour_space: u32, }
    VkSwapchainCreateInfoKHR { kind: u32, next: *const c_void, flags: u32, surface: Handle, min_images: u32, format: u32, colour_space: u32, extent: [u32; 2], layers: u32, usage: u32, sharing_mode: u32, family_count: u32, families: *const u32, transform: u32, alpha: u32, present_mode: u32, clipped: u32, old: Handle, }
    VkRectLayerKHR { offset: [i32; 2], extent: [u32; 2], layer: u32, }
    VkPresentRegionKHR { count: u32, rects: *const VkRectLayerKHR, }
    VkPresentRegionsKHR { kind: u32, next: *const c_void, count: u32, regions: *const VkPresentRegionKHR, }
    VkPresentInfoKHR { kind: u32, next: *const c_void, wait_count: u32, waits: *const Handle, count: u32, swapchains: *const Handle, indices: *const u32, results: *mut i32, }
}

#[cfg(all(windows, feature = "gpu"))]
pub(crate) const LAYOUT_TRANSFER_SOURCE: u32 = 6;
#[cfg(feature = "gpu")]
pub(crate) const LAYOUT_PRESENT: u32 = 1_000_001_002;
#[cfg(all(windows, feature = "gpu"))]
pub(crate) const ACCESS_TRANSFER_READ: u32 = 0x800;
#[cfg(feature = "gpu")]
pub(crate) const STAGE_BOTTOM: u32 = 0x2000;
// Kinds of use of an image or buffer.
#[cfg(feature = "gpu")]
pub(crate) const USAGE_TRANSFER_SOURCE: u32 = 0x1;
#[cfg(feature = "gpu")]
pub(crate) const USAGE_TRANSFER_DESTINATION: u32 = 0x2;
// Results.
#[cfg(all(windows, feature = "gpu"))]
pub(crate) const DEVICE_LOST: i32 = -4;
#[cfg(feature = "gpu")]
pub(crate) const OUT_OF_DATE: i32 = -1_000_001_004;
#[cfg(feature = "gpu")]
/// One layer of an image's colour: aspect, level, first layer, layers.
pub(crate) const LAYER: [u32; 4] = [0x1, 0, 0, 1];

#[cfg(all(windows, feature = "gpu"))]
/// The name in a Vulkan structure (`char[256]`), up to its zero.
#[inline]
pub(crate) fn name(of: &[c_char; 256]) -> &[u8] {
    // SAFETY: `c_char` and `u8` have one layout.
    let bytes: &[u8; 256] = unsafe { &*(of as *const [c_char; 256]).cast() };
    bytes.split(|byte| *byte == 0).next().unwrap_or_default()
}

#[cfg(all(windows, feature = "gpu"))]
structures! {
    VkImageCopy { source_layers: [u32; 4], source_offset: [i32; 3], destination_layers: [u32; 4], destination_offset: [i32; 3], extent: [u32; 3], }
    VkWin32SurfaceCreateInfoKHR { kind: u32, next: *const c_void, flags: u32, instance: isize, hwnd: isize, }
}

// X windows shown through Vulkan (`gpu_vulkan_x11`): their surfaces, and what a device's driver
// is.
#[cfg(all(unix, not(target_os = "macos"), feature = "gpu"))]
structures! {
    VkXlibSurfaceCreateInfoKHR { kind: u32, next: *const c_void, flags: u32, display: *mut c_void, window: std::ffi::c_ulong, }
    VkPhysicalDeviceProperties2 { kind: u32, next: *mut c_void, properties: VkPhysicalDeviceProperties, }
    VkPhysicalDeviceDriverProperties { kind: u32, next: *mut c_void, id: u32, name: [c_char; 256], info: [c_char; 256], conformance: [u8; 4], }
}

#[cfg(all(unix, not(target_os = "macos"), feature = "gpu"))]
pub(crate) const SUBOPTIMAL: i32 = 1_000_001_003;

#[cfg(test)]
mod tests {
    use super::*;

    /// Each structure is as large as vulkan_core.h makes it on 64-bit systems.
    #[test]
    fn the_structures_are_laid_out_as_vulkan_lays_them_out() {
        let sizes = [
            (size_of::<VkPhysicalDeviceMemoryProperties>(), 520),
            (size_of::<VkMemoryRequirements>(), 24),
            (size_of::<VkMemoryAllocateInfo>(), 32),
            (size_of::<VkBufferCreateInfo>(), 56),
            (size_of::<VkImageCreateInfo>(), 88),
            (size_of::<VkImageViewCreateInfo>(), 80),
            (size_of::<VkImageMemoryBarrier>(), 72),
            (size_of::<VkBufferImageCopy>(), 56),
            (size_of::<VkCommandPoolCreateInfo>(), 24),
            (size_of::<VkCommandBufferAllocateInfo>(), 32),
            (size_of::<VkCommandBufferBeginInfo>(), 32),
            (size_of::<VkSubmitInfo>(), 72),
            (size_of::<VkFenceCreateInfo>(), 24),
            (size_of::<VkViewport>(), 24),
            (size_of::<VkRect2D>(), 16),
        ];
        for (at, (size, wanted)) in sizes.into_iter().enumerate() {
            assert_eq!(size, wanted, "structure {at}");
        }
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn what_showing_a_window_takes_is_laid_out_as_vulkan_lays_it_out() {
        let sizes = [
            (size_of::<VkApplicationInfo>(), 48),
            (size_of::<VkInstanceCreateInfo>(), 64),
            (size_of::<VkExtensionProperties>(), 260),
            (size_of::<VkPhysicalDeviceProperties>(), 824),
            (size_of::<VkQueueFamilyProperties>(), 24),
            (size_of::<VkDeviceQueueCreateInfo>(), 40),
            (size_of::<VkDeviceCreateInfo>(), 72),
            (size_of::<VkSemaphoreCreateInfo>(), 24),
            (size_of::<VkSurfaceCapabilitiesKHR>(), 52),
            (size_of::<VkSurfaceFormatKHR>(), 8),
            (size_of::<VkSwapchainCreateInfoKHR>(), 104),
            (size_of::<VkRectLayerKHR>(), 20),
            (size_of::<VkPresentRegionKHR>(), 16),
            (size_of::<VkPresentRegionsKHR>(), 32),
            (size_of::<VkPresentInfoKHR>(), 64),
        ];
        for (at, (size, wanted)) in sizes.into_iter().enumerate() {
            assert_eq!(size, wanted, "structure {at}");
        }
    }

    #[cfg(all(windows, feature = "gpu"))]
    #[test]
    fn a_windows_surface_is_laid_out_as_vulkan_lays_it_out() {
        let sizes = [(size_of::<VkImageCopy>(), 68), (size_of::<VkWin32SurfaceCreateInfoKHR>(), 40)];
        for (at, (size, wanted)) in sizes.into_iter().enumerate() {
            assert_eq!(size, wanted, "structure {at}");
        }
    }

    #[cfg(all(unix, not(target_os = "macos"), feature = "gpu"))]
    #[test]
    fn an_x_surface_and_a_drivers_name_are_laid_out_as_vulkan_lays_them_out() {
        let sizes = [
            (size_of::<VkXlibSurfaceCreateInfoKHR>(), 40),
            (size_of::<VkPhysicalDeviceProperties2>(), 840),
            (size_of::<VkPhysicalDeviceDriverProperties>(), 536),
            (std::mem::offset_of!(VkPhysicalDeviceDriverProperties, info), 276),
        ];
        for (at, (size, wanted)) in sizes.into_iter().enumerate() {
            assert_eq!(size, wanted, "structure {at}");
        }
    }

    /// Where the system has Vulkan, its loader is found, and makes instances.
    #[test]
    fn the_loader_is_found_where_there_is_one() {
        let Some(find) = loader() else { return println!("no Vulkan loader on this system") };
        // SAFETY: with no instance, the loader gives the functions that make one.
        let made = unsafe { find(std::ptr::null_mut(), c"vkCreateInstance".as_ptr()) };
        assert!(!made.is_null());
        let mut missing = |name: &CStr| if name == c"vkNothing" { std::ptr::null() } else { made };
        assert_eq!(found(&mut missing, "vkNothing\0"), Err("vkNothing"));
        assert_eq!(found(&mut missing, "vkCreateInstance\0"), Ok(made));
    }
}
