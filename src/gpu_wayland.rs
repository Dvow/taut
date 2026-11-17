//! Wayland windows shown through Vulkan, with the feature `gpu`, where a window asks for a GPU
//! path (`Renderer::Vulkan`, `Gpu`, or another named): `Auto` keeps shared memory. The CPU
//! draws every pixel as without it, into a buffer the GPU reads; what changed is copied from
//! there into one of three images, with what that image missed since it was last shown, and the
//! image is handed to the compositor as a dmabuf (`zwp_linux_dmabuf_v1`). No shader touches a
//! pixel: the compositor gets the CPU's pixels, byte for byte.
//!
//! - **Layout:** a DRM format modifier both sides take (`VK_EXT_image_drm_format_modifier`),
//!   of one plane the device copies into and exports. The compositor's are from its default
//!   feedback (v4: format table and tranches), else its `modifier` events (v3). A device that
//!   drives the compositor's main device comes first.
//! - **Buffers:** made with `create`, answered later by `created` or `failed`; never
//!   `create_immed`, whose failure ends the connection.
//! - **Frames:** the copy's fence is waited for before the commit. An image is drawn into again
//!   only once the compositor releases it; one of an old size goes when released.
//! - **Failures:** each falls back to shared memory (`wl_shm`) for good, and the surface is
//!   drawn whole there: no Vulkan 1.2, no device exporting dmabufs, a compositor without the
//!   protocol or a common layout, an image or buffer refused, a lost device.
use super::{Host, Link, Sheet, on_pixels, paint_area};
use crate::canvas::Canvas;
use crate::geom::{Rect, rect};
use crate::platform::Renderer;
use crate::vk::*;
use crate::wayland_wire::{Arg, Interface, Message, Wire, read_file};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr::{null, null_mut};

/// DRM's codes of the formats handed over: XRGB8888 and ARGB8888, as `wl_shm`'s.
const SOLID: u32 = u32::from_le_bytes(*b"XR24");
const SEE_THROUGH: u32 = u32::from_le_bytes(*b"AR24");
/// A window's images: the one shown, one the compositor may still hold, one to draw into.
const IMAGES: usize = 3;
/// Most places copied one by one; beyond, the area around them all.
const MOST: usize = 16;
/// How long a copy may take before the device is taken for lost: a second, in nanoseconds.
const WAITED: u64 = 1_000_000_000;
/// Vulkan 1.2, as `VK_MAKE_API_VERSION` makes it.
const VULKAN_1_2: u32 = (1 << 22) | (2 << 12);
/// Memory as a dmabuf; exportable so.
const DMA_BUF: u32 = 0x200;
const EXPORTABLE: u32 = 0x2;
/// Images laid out by a DRM format modifier; the memory of their first plane.
const BY_MODIFIER: u32 = 1_000_158_000;
const PLANE_0: u32 = 0x80;
/// A layout's feature: copied into (`VK_FORMAT_FEATURE_TRANSFER_DST_BIT`).
const COPIED_INTO: u32 = 0x8000;
/// The image layout outside this program, and the queue families there: another API's, another
/// device's (`VK_QUEUE_FAMILY_FOREIGN_EXT`), else another instance's (`VK_QUEUE_FAMILY_EXTERNAL`).
const LAYOUT_GENERAL: u32 = 1;
const FOREIGN: u32 = !2;
const EXTERNAL: u32 = !1;
/// Device extensions: needed, then wanted.
const NEEDED: [&CStr; 3] = [c"VK_KHR_external_memory_fd", c"VK_EXT_external_memory_dma_buf", c"VK_EXT_image_drm_format_modifier"];
const TO_OTHERS: &CStr = c"VK_EXT_queue_family_foreign";
const NUMBERED: &CStr = c"VK_EXT_physical_device_drm";

functions!(Global {
    vkCreateInstance: fn(*const VkInstanceCreateInfo, *const c_void, *mut Object) -> i32;
});

functions!(Instance {
    vkDestroyInstance: fn(Object, *const c_void);
    vkEnumeratePhysicalDevices: fn(Object, *mut u32, *mut Object) -> i32;
    vkGetPhysicalDeviceProperties: fn(Object, *mut VkPhysicalDeviceProperties);
    vkGetPhysicalDeviceProperties2: fn(Object, *mut VkPhysicalDeviceProperties2);
    vkGetPhysicalDeviceQueueFamilyProperties: fn(Object, *mut u32, *mut VkQueueFamilyProperties);
    vkGetPhysicalDeviceMemoryProperties: fn(Object, *mut VkPhysicalDeviceMemoryProperties);
    vkGetPhysicalDeviceFormatProperties2: fn(Object, u32, *mut VkFormatProperties2);
    vkGetPhysicalDeviceImageFormatProperties2: fn(Object, *const VkPhysicalDeviceImageFormatInfo2, *mut VkImageFormatProperties2) -> i32;
    vkEnumerateDeviceExtensionProperties: fn(Object, *const c_char, *mut u32, *mut VkExtensionProperties) -> i32;
    vkCreateDevice: fn(Object, *const VkDeviceCreateInfo, *const c_void, *mut Object) -> i32;
    vkGetDeviceProcAddr: fn(Object, *const c_char) -> *const c_void;
});

functions!(Device {
    vkDestroyDevice: fn(Object, *const c_void);
    vkGetDeviceQueue: fn(Object, u32, u32, *mut Object);
    vkDeviceWaitIdle: fn(Object) -> i32;
    vkQueueSubmit: fn(Object, u32, *const VkSubmitInfo, Handle) -> i32;
    vkCreateBuffer: fn(Object, *const VkBufferCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyBuffer: fn(Object, Handle, *const c_void);
    vkGetBufferMemoryRequirements: fn(Object, Handle, *mut VkMemoryRequirements);
    vkCreateImage: fn(Object, *const VkImageCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyImage: fn(Object, Handle, *const c_void);
    vkGetImageMemoryRequirements: fn(Object, Handle, *mut VkMemoryRequirements);
    vkGetImageSubresourceLayout: fn(Object, Handle, *const VkImageSubresource, *mut VkSubresourceLayout);
    vkGetImageDrmFormatModifierPropertiesEXT: fn(Object, Handle, *mut VkImageDrmFormatModifierPropertiesEXT) -> i32;
    vkAllocateMemory: fn(Object, *const VkMemoryAllocateInfo, *const c_void, *mut Handle) -> i32;
    vkFreeMemory: fn(Object, Handle, *const c_void);
    vkBindBufferMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkBindImageMemory: fn(Object, Handle, Handle, u64) -> i32;
    vkMapMemory: fn(Object, Handle, u64, u64, u32, *mut *mut c_void) -> i32;
    vkGetMemoryFdKHR: fn(Object, *const VkMemoryGetFdInfoKHR, *mut c_int) -> i32;
    vkCreateCommandPool: fn(Object, *const VkCommandPoolCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyCommandPool: fn(Object, Handle, *const c_void);
    vkAllocateCommandBuffers: fn(Object, *const VkCommandBufferAllocateInfo, *mut Object) -> i32;
    vkFreeCommandBuffers: fn(Object, Handle, u32, *const Object);
    vkBeginCommandBuffer: fn(Object, *const VkCommandBufferBeginInfo) -> i32;
    vkEndCommandBuffer: fn(Object) -> i32;
    vkCmdPipelineBarrier: fn(Object, u32, u32, u32, u32, *const c_void, u32, *const c_void, u32, *const VkImageMemoryBarrier);
    vkCmdCopyBufferToImage: fn(Object, Handle, Handle, u32, u32, *const VkBufferImageCopy);
    vkCreateFence: fn(Object, *const VkFenceCreateInfo, *const c_void, *mut Handle) -> i32;
    vkDestroyFence: fn(Object, Handle, *const c_void);
    vkWaitForFences: fn(Object, u32, *const Handle, u32, u64) -> i32;
    vkResetFences: fn(Object, u32, *const Handle) -> i32;
});

/// What a program's windows have of dmabufs: the compositor's side, and the device, once asked
/// for.
pub(super) struct Gpu {
    /// `zwp_linux_dmabuf_v1`.
    dmabuf: u32,
    /// The formats and modifiers the compositor takes; and with v4, its format table, the
    /// tranches told since its last `done`, and its main device.
    offered: Vec<(u32, u64)>,
    table: Vec<(u32, u64)>,
    told: Vec<(u32, u64)>,
    main: Option<u64>,
    /// The device (none: none would do), once asked for.
    vulkan: Option<Vulkan>,
    tried: bool,
    /// The compositor's answers: each request's object, and the buffer made (0: refused).
    answers: Vec<(u32, u32)>,
    /// Requests whose images are gone: their buffers go as soon as made.
    orphans: Vec<u32>,
    /// The buffers the compositor holds; and images of an old size it holds, let go once
    /// released.
    held: Vec<u32>,
    retired: Vec<Image>,
}

/// The device, its queue and the layouts both it and the compositor take.
struct Vulkan {
    instance: Object,
    calls: Instance,
    device: Object,
    on: Device,
    queue: Object,
    pool: Handle,
    family: u32,
    /// The queue family images are handed to, and taken back from.
    outside: u32,
    /// Kinds of memory, one bit each: the CPU writes and the GPU reads; the GPU's own.
    writable: u32,
    local: u32,
    /// The modifiers both take, for solid pixels and for see-through ones.
    solid: Vec<u64>,
    clear: Vec<u64>,
    /// The device failed, or was lost: every window goes back to shared memory.
    lost: bool,
    /// Tests: every copy from now on says the device was lost.
    #[cfg(test)]
    faked: bool,
}

/// A device that could do: its queue family, its layouts, whether it hands images over to
/// another device, and how good it is (0: it drives the compositor's main device; 1: a GPU; 2:
/// the CPU at work).
struct Candidate {
    physical: Object,
    family: u32,
    solid: Vec<u64>,
    clear: Vec<u64>,
    foreign: bool,
    rank: u8,
}

/// How a surface is shown: not yet asked, through the GPU, or by shared memory.
#[derive(Default)]
pub(super) enum Way {
    #[default]
    Unasked,
    Gpu(Shows),
    Shm,
}

/// A surface shown through the GPU: its pixels in a buffer the CPU writes, its images, and the
/// commands copying from one to the other.
pub(super) struct Shows {
    size: [i32; 2],
    clear: bool,
    buffer: Handle,
    memory: Handle,
    pixels: *mut u32,
    images: Vec<Image>,
    commands: Object,
    fence: Handle,
    /// Tests: the places the last frame copied.
    #[cfg(test)]
    copied: Vec<[i32; 4]>,
}

/// An image handed over: its memory, the request for its buffer and the buffer (0: not yet
/// made), whether it was ever copied into, and the places it lacks (left, top, right, bottom in
/// pixels).
struct Image {
    image: Handle,
    memory: Handle,
    params: u32,
    buffer: u32,
    shown: bool,
    missing: Vec<[i32; 4]>,
    /// Tests: its first plane's offset and row pitch in its memory.
    #[cfg(test)]
    layout: (u64, u64),
}

/// What a surface's draw came to.
enum Drawn {
    /// Shared memory shows it.
    Shm,
    /// The GPU failed: shared memory shows it from now on, all of it to be drawn there.
    Fell,
    /// No image is free yet.
    Waits,
    /// This buffer is to be handed over.
    Buffer(u32),
}

impl<App> Host<App> {
    /// Asks the compositor what it takes as dmabufs, where windows ask for a GPU path.
    pub(super) fn ask_gpu(&mut self, renderer: Renderer) {
        ask(&self.link, renderer);
    }

    /// A message to `zwp_linux_dmabuf_v1` or what it made.
    pub(super) fn heard_by_gpu(&mut self, message: Message) {
        if let Some(gpu) = self.link.gpu.borrow_mut().as_mut() {
            gpu.heard(&self.link.wire, message);
        }
    }

    /// Tells the interface which path shows the first window (`Ui::renderer`); it runs again
    /// where that changed.
    pub(super) fn tell_renderer(&mut self) {
        let renderer = if matches!(self.windows[0].sheet.way, Way::Gpu(_)) { Renderer::Vulkan } else { Renderer::Cpu };
        self.rerun |= std::mem::replace(&mut self.ui.renderer, renderer) != renderer;
    }

    /// Whether buffer `id`, released, was an image's (which may be drawn into again).
    pub(super) fn released_by_gpu(&mut self, id: u32) -> bool {
        self.link.gpu.borrow_mut().as_mut().is_some_and(|gpu| gpu.released(&self.link.wire, id))
    }
}

/// Binds `zwp_linux_dmabuf_v1` where windows ask for a GPU path (`renderer`, or
/// `TAUT_RENDERER`) and the compositor offers it, and asks for its default feedback (v4).
fn ask(link: &Link, renderer: Renderer) {
    let ((name, version), wire) = (link.has.dmabuf, &link.wire);
    if name == 0 || crate::platform::wants_gpu(renderer) != Some(true) {
        return;
    }
    let version = version.min(4);
    let dmabuf = wire.bind(link.has.registry, name, "zwp_linux_dmabuf_v1", version, Interface::Dmabuf);
    let feedback = if version >= 4 { wire.make(Interface::DmabufFeedback) } else { 0 };
    if feedback != 0 {
        wire.send(dmabuf, 2, &[Arg::Uint(feedback)]);
    }
    let gpu = Gpu {
        dmabuf,
        offered: Vec::new(),
        table: Vec::new(),
        told: Vec::new(),
        main: None,
        vulkan: None,
        tried: false,
        answers: Vec::new(),
        orphans: Vec::new(),
        held: Vec::new(),
        retired: Vec::new(),
    };
    *link.gpu.borrow_mut() = Some(gpu);
}

impl Sheet {
    /// Draws what changed through the GPU where it shows this surface: painted into its
    /// pixels, copied into a free image, handed over. Says whether it committed; nothing where
    /// shared memory shows it (all of it, where the GPU just failed).
    pub(super) fn draw_by_gpu(&mut self, link: &Link, scale: f32, paint: &mut dyn FnMut(&mut Canvas)) -> Option<bool> {
        if matches!(self.way, Way::Shm) {
            return None;
        }
        let (units, size, clear) = (rect(0.0, 0.0, self.units[0] as f32, self.units[1] as f32), self.pixels, self.clear);
        let areas: Vec<Rect> = self.changed.iter().map(|area| area.intersect(units)).filter(|area| !area.is_empty()).collect();
        let places: Vec<[i32; 4]> = areas.iter().map(|area| on_pixels(*area, scale, size)).collect();
        let mut painted = |pixels: &mut [u32]| areas.iter().for_each(|area| paint_area(pixels, size, *area, scale, clear, paint));
        let drawn = match link.gpu.borrow_mut().as_mut() {
            Some(gpu) => gpu.draw(&link.wire, &mut self.way, size, clear, &places, &mut painted),
            None => Drawn::Shm,
        };
        match drawn {
            Drawn::Shm => self.way = Way::Shm,
            Drawn::Fell => (self.way, self.changed) = (Way::Shm, vec![units]),
            Drawn::Waits => return Some(self.pay(link)),
            Drawn::Buffer(buffer) => {
                let changed = std::mem::take(&mut self.changed);
                self.commit(link, buffer, &changed, scale);
                return Some(true);
            }
        }
        None
    }

    /// Lets go of what the GPU has of a surface.
    pub(super) fn forget_gpu(link: &Link, way: Way) {
        if let (Way::Gpu(shows), Some(gpu)) = (way, link.gpu.borrow_mut().as_mut()) {
            gpu.forget(&link.wire, shows);
        }
    }
}

impl Gpu {
    /// Takes a message to `zwp_linux_dmabuf_v1` (v3: a format's modifier), its feedback (v4:
    /// the format table, the main device, a tranche's formats, the end of a tranche and of all),
    /// or a buffer request (made, or refused).
    fn heard(&mut self, wire: &Wire, mut message: Message) {
        match (message.of, message.opcode) {
            (Interface::Dmabuf, 1) => {
                let (format, high, low) = (message.uint(), message.uint(), message.uint());
                self.offered.push((format, (high as u64) << 32 | low as u64));
            }
            (Interface::DmabufFeedback, 0) => self.offered = std::mem::take(&mut self.told),
            (Interface::DmabufFeedback, 1) => {
                let (size, file) = (message.uint() as usize, message.file.take());
                self.table = file.and_then(|file| read_file(&file, size.min(1 << 20))).map(|bytes| table(&bytes)).unwrap_or_default();
            }
            (Interface::DmabufFeedback, 2) => self.main = message.bytes().first_chunk::<8>().map(|number| u64::from_ne_bytes(*number)),
            (Interface::DmabufFeedback, 5) => {
                let table = &self.table;
                let found = message.bytes().as_chunks::<2>().0.iter().filter_map(|index| table.get(u16::from_ne_bytes(*index) as usize));
                self.told.extend(found);
            }
            (Interface::DmabufParams, 0) => {
                let buffer = message.uint();
                wire.note(buffer, Interface::Buffer);
                self.made(wire, message.object, buffer);
            }
            (Interface::DmabufParams, 1) => self.made(wire, message.object, 0),
            _ => {}
        }
    }

    /// The compositor answered request `params`: `buffer` made (0: refused). The answer waits
    /// for its image (the request lives until then: its number is not used again meanwhile);
    /// where the image is gone, the request and the buffer go.
    fn made(&mut self, wire: &Wire, params: u32, buffer: u32) {
        let Some(at) = self.orphans.iter().position(|orphan| *orphan == params) else { return self.answers.push((params, buffer)) };
        self.orphans.swap_remove(at);
        wire.send(params, 0, &[]);
        unbuffer(wire, buffer);
    }

    /// The answer to an image's request, if there is one yet: the request goes.
    fn answer(&mut self, wire: &Wire, image: &mut Image) -> bool {
        let Some(at) = self.answers.iter().position(|(params, _)| *params == image.params) else { return false };
        wire.send(image.params, 0, &[]);
        (image.params, image.buffer) = (0, self.answers.swap_remove(at).1);
        true
    }

    /// Buffer `id` was released. Says whether it was an image's: one of an old size goes.
    fn released(&mut self, wire: &Wire, id: u32) -> bool {
        let Some(at) = self.held.iter().position(|held| *held == id) else { return false };
        self.held.swap_remove(at);
        if let Some(at) = self.retired.iter().position(|image| image.buffer == id) {
            let image = self.retired.swap_remove(at);
            self.unmake(wire, image);
        }
        true
    }

    /// Draws a surface `size` pixels big through the GPU, if it shows it: `paint` paints
    /// `places` into its pixels, which are copied into a free image.
    fn draw(&mut self, wire: &Wire, way: &mut Way, size: [i32; 2], clear: bool, places: &[[i32; 4]], paint: &mut dyn FnMut(&mut [u32])) -> Drawn {
        if matches!(way, Way::Unasked) {
            *way = self.shows(clear).map_or(Way::Shm, Way::Gpu);
        }
        let Way::Gpu(shows) = way else { return Drawn::Shm };
        match self.frame(wire, shows, size, places, paint) {
            Ok(Some(buffer)) => Drawn::Buffer(buffer),
            Ok(None) => Drawn::Waits,
            Err(()) => self.fall(wire, way),
        }
    }

    /// The GPU failed a surface: shared memory shows it from now on.
    fn fall(&mut self, wire: &Wire, way: &mut Way) -> Drawn {
        if let Way::Gpu(shows) = std::mem::replace(way, Way::Shm) {
            self.forget(wire, shows);
        }
        Drawn::Fell
    }

    /// A surface's part on the GPU, its images not yet made. Nothing where the device can't
    /// show it (opened first, if it was not yet).
    fn shows(&mut self, clear: bool) -> Option<Shows> {
        if !std::mem::replace(&mut self.tried, true) {
            self.vulkan = Vulkan::open(&self.offered, self.main);
        }
        let vulkan = self.vulkan.as_mut().filter(|vulkan| !vulkan.lost)?;
        if (if clear { &vulkan.clear } else { &vulkan.solid }).is_empty() {
            return None;
        }
        let (commands, fence) = vulkan.parts()?;
        Some(Shows {
            size: [0; 2],
            clear,
            buffer: 0,
            memory: 0,
            pixels: null_mut(),
            images: Vec::new(),
            commands,
            fence,
            #[cfg(test)]
            copied: Vec::new(),
        })
    }

    /// One frame of a surface: `places` painted into its pixels and copied, with what the
    /// image lacks, into the first image made and free. The buffer to hand over; nothing where
    /// none is free yet. An error where the device or the compositor failed.
    fn frame(&mut self, wire: &Wire, shows: &mut Shows, size: [i32; 2], places: &[[i32; 4]], paint: &mut dyn FnMut(&mut [u32])) -> Result<Option<u32>, ()> {
        if self.vulkan.as_ref().is_none_or(|vulkan| vulkan.lost) {
            return Err(());
        }
        if shows.size != size {
            self.fit(wire, shows, size)?;
        }
        self.answered(wire, shows)?;
        let held = &self.held;
        let Some(at) = shows.images.iter().position(|image| image.buffer != 0 && !held.contains(&image.buffer)) else { return Ok(None) };
        // SAFETY: the pixels are mapped, `size` big, and the GPU is done with them (every copy
        // is waited for).
        paint(unsafe { std::slice::from_raw_parts_mut(shows.pixels, size[0] as usize * size[1] as usize) });
        let mut wanted = std::mem::take(&mut shows.images[at].missing);
        wanted.extend_from_slice(places);
        let wanted = bounded(wanted);
        if !self.vulkan.as_mut().is_some_and(|vulkan| vulkan.copy(shows, at, &wanted)) {
            return Err(());
        }
        for (_, image) in shows.images.iter_mut().enumerate().filter(|(other, _)| *other != at) {
            image.missing.extend_from_slice(places);
            image.missing = bounded(std::mem::take(&mut image.missing));
        }
        #[cfg(test)]
        {
            shows.copied = wanted;
        }
        shows.images[at].shown = true;
        self.held.push(shows.images[at].buffer);
        Ok(Some(shows.images[at].buffer))
    }

    /// Takes the compositor's answers for a surface's images. An error where it refused one.
    fn answered(&mut self, wire: &Wire, shows: &mut Shows) -> Result<(), ()> {
        for image in shows.images.iter_mut().filter(|image| image.buffer == 0) {
            if self.answer(wire, image) && image.buffer == 0 {
                return Err(());
            }
        }
        Ok(())
    }

    /// Gives a surface pixels and images `size` big; those of the old size go (as the
    /// compositor releases them). An error where any can't be made.
    fn fit(&mut self, wire: &Wire, shows: &mut Shows, size: [i32; 2]) -> Result<(), ()> {
        std::mem::take(&mut shows.images).into_iter().for_each(|image| self.let_go(wire, image));
        let (vulkan, dmabuf) = (self.vulkan.as_mut().ok_or(())?, self.dmabuf);
        vulkan.unbuffer(shows);
        shows.size = size;
        if !vulkan.buffer(shows) {
            return Err(());
        }
        for _ in 0..IMAGES {
            let image = vulkan.image(wire, dmabuf, size, shows.clear).ok_or(())?;
            shows.images.push(image);
        }
        Ok(())
    }

    /// Lets go of an image: at once, or once the compositor releases its buffer.
    fn let_go(&mut self, wire: &Wire, mut image: Image) {
        if image.buffer != 0 && self.held.contains(&image.buffer) {
            return self.retired.push(image);
        }
        if image.buffer == 0 && image.params != 0 && !self.answer(wire, &mut image) {
            self.orphans.push(image.params);
        }
        self.unmake(wire, image);
    }

    /// Destroys an image and its buffer.
    fn unmake(&mut self, wire: &Wire, image: Image) {
        unbuffer(wire, image.buffer);
        if let Some(vulkan) = &self.vulkan {
            vulkan.unmake(&image);
        }
    }

    /// Lets go of a surface's part on the GPU.
    fn forget(&mut self, wire: &Wire, mut shows: Shows) {
        std::mem::take(&mut shows.images).into_iter().for_each(|image| self.let_go(wire, image));
        if let Some(vulkan) = &mut self.vulkan {
            vulkan.unbuffer(&mut shows);
            vulkan.unpart(&shows);
        }
    }

    /// Tests: the device is lost, as when it is removed: the next copy says so.
    #[cfg(test)]
    fn lose(&mut self) {
        if let Some(vulkan) = &mut self.vulkan {
            vulkan.faked = true;
        }
    }
}

impl Drop for Gpu {
    /// Lets go of the images still held. (The device goes after.)
    fn drop(&mut self) {
        if let Some(vulkan) = &self.vulkan {
            self.retired.iter().for_each(|image| vulkan.unmake(image));
        }
    }
}

/// Destroys a buffer the compositor made (0: none).
fn unbuffer(wire: &Wire, buffer: u32) {
    if buffer != 0 {
        wire.send(buffer, 0, &[]);
        wire.note(buffer, Interface::Free);
    }
}

/// Places (left, top, right, bottom), none inside another, up to `MOST`; else the area around
/// them all.
fn bounded(places: Vec<[i32; 4]>) -> Vec<[i32; 4]> {
    let inside = |inner: &[i32; 4], outer: &[i32; 4]| outer[0] <= inner[0] && outer[1] <= inner[1] && inner[2] <= outer[2] && inner[3] <= outer[3];
    let mut kept: Vec<[i32; 4]> = Vec::new();
    for place in places {
        if !kept.iter().any(|other| inside(&place, other)) {
            kept.retain(|other| !inside(other, &place));
            kept.push(place);
        }
    }
    if kept.len() <= MOST {
        return kept;
    }
    vec![kept.iter().fold([i32::MAX, i32::MAX, 0, 0], |all, place| [all[0].min(place[0]), all[1].min(place[1]), all[2].max(place[2]), all[3].max(place[3])])]
}

/// The compositor's format table: a format and a modifier each 16 bytes.
fn table(bytes: &[u8]) -> Vec<(u32, u64)> {
    let entries = bytes.as_chunks::<16>().0.iter().filter_map(|entry| Some((u32::from_ne_bytes(*entry.first_chunk()?), u64::from_ne_bytes(*entry.last_chunk()?))));
    entries.collect()
}

/// A device number (`dev_t`) from its major and minor numbers, as glibc's `makedev` makes it.
fn device_number(major: u64, minor: u64) -> u64 {
    (major & 0xffff_f000) << 32 | (major & 0xfff) << 8 | (minor & 0xffff_ff00) << 12 | (minor & 0xff)
}

impl Vulkan {
    /// Opens the loader, makes an instance of Vulkan 1.2, and a device on the best that hands
    /// over images the compositor takes (`offered`; driving its `main` device first). Nothing
    /// where none does.
    fn open(offered: &[(u32, u64)], main: Option<u64>) -> Option<Vulkan> {
        let get = loader()?;
        // SAFETY: the instance is null or Vulkan's.
        let find = move |instance: Object, name: &CStr| unsafe { get(instance, name.as_ptr()) };
        let global = Global::load(&mut |name| find(null_mut(), name)).ok()?;
        let application = VkApplicationInfo { kind: 0, api_version: VULKAN_1_2, ..Default::default() };
        let asked = VkInstanceCreateInfo { kind: 1, application: &application, ..Default::default() };
        let mut instance = null_mut();
        // SAFETY: Vulkan's documented call.
        if unsafe { (global.vkCreateInstance)(&asked, null(), &mut instance) } != 0 {
            return None;
        }
        let Ok(calls) = Instance::load(&mut |name| find(instance, name)) else { return unmade(find(instance, c"vkDestroyInstance"), instance) };
        // SAFETY: Vulkan's documented enumeration.
        let devices = listed(null_mut(), |count, list| unsafe { (calls.vkEnumeratePhysicalDevices)(instance, count, list) });
        let best = devices.into_iter().filter_map(|physical| Candidate::of(&calls, physical, offered, main)).min_by_key(|candidate| candidate.rank);
        match best {
            Some(best) => Vulkan::make(instance, calls, best),
            None => unmade(calls.vkDestroyInstance as *const c_void, instance),
        }
    }

    /// A device on `chosen`, with its queue and a pool of commands. Nothing (and the instance
    /// gone) where any of it can't be made.
    fn make(instance: Object, calls: Instance, chosen: Candidate) -> Option<Vulkan> {
        let mut names: Vec<*const c_char> = NEEDED.iter().map(|name| name.as_ptr()).collect();
        if chosen.foreign {
            names.push(TO_OTHERS.as_ptr());
        }
        let (physical, family, priority) = (chosen.physical, chosen.family, 1.0f32);
        let queue = VkDeviceQueueCreateInfo { kind: 2, family, count: 1, priorities: &priority, ..Default::default() };
        let asked = VkDeviceCreateInfo { kind: 3, queue_count: 1, queues: &queue, extension_count: names.len() as u32, extensions: names.as_ptr(), ..Default::default() };
        let (mut device, mut memory) = (null_mut(), VkPhysicalDeviceMemoryProperties::default());
        // SAFETY: Vulkan's documented calls; the names end in zeros.
        let made = unsafe {
            (calls.vkGetPhysicalDeviceMemoryProperties)(physical, &mut memory);
            (calls.vkCreateDevice)(physical, &asked, null(), &mut device)
        };
        let find = calls.vkGetDeviceProcAddr;
        // SAFETY: the device was just made.
        let on = (made == 0).then(|| Device::load(&mut |name| unsafe { find(device, name.as_ptr()) }).ok()).flatten();
        let Some(on) = on else {
            if made == 0 {
                // SAFETY: the device was just made.
                unmade(unsafe { find(device, c"vkDestroyDevice".as_ptr()) }, device);
            }
            return unmade(calls.vkDestroyInstance as *const c_void, instance);
        };
        let kinds = |wanted: u32| memory.types.iter().take(memory.type_count as usize).enumerate().filter(|(_, kind)| kind.flags & wanted == wanted).fold(0, |all, (at, _)| all | 1 << at);
        let outside = if chosen.foreign { FOREIGN } else { EXTERNAL };
        let (writable, local) = (kinds(HOST_VISIBLE_AND_COHERENT), kinds(DEVICE_LOCAL));
        let mut vulkan = Vulkan {
            instance,
            calls,
            device,
            on,
            queue: null_mut(),
            pool: 0,
            family,
            outside,
            writable,
            local,
            solid: chosen.solid,
            clear: chosen.clear,
            lost: false,
            #[cfg(test)]
            faked: false,
        };
        let pool = VkCommandPoolCreateInfo { kind: 39, flags: 0x2, family, ..Default::default() };
        // SAFETY: the device's queue 0 of the family it was made with; and a pool for it.
        let made = unsafe {
            (vulkan.on.vkGetDeviceQueue)(device, family, 0, &mut vulkan.queue);
            (vulkan.on.vkCreateCommandPool)(device, &pool, null(), &mut vulkan.pool)
        };
        (made == 0).then_some(vulkan)
    }

    /// A surface's commands and their fence. Nothing where either can't be made.
    fn parts(&mut self) -> Option<(Object, Handle)> {
        let commands = VkCommandBufferAllocateInfo { kind: 40, pool: self.pool, count: 1, ..Default::default() };
        let fence = VkFenceCreateInfo { kind: 8, ..Default::default() };
        let (mut made, mut fenced) = (null_mut(), 0);
        // SAFETY: Vulkan's documented calls on the device, filling in one handle each.
        let results = unsafe { [(self.on.vkAllocateCommandBuffers)(self.device, &commands, &mut made), (self.on.vkCreateFence)(self.device, &fence, null(), &mut fenced)] };
        if results == [0; 2] {
            return Some((made, fenced));
        }
        // SAFETY: what was made is not used after; null handles are let be.
        unsafe { (self.on.vkDestroyFence)(self.device, fenced, null()) };
        if !made.is_null() {
            unsafe { (self.on.vkFreeCommandBuffers)(self.device, self.pool, 1, &made) };
        }
        None
    }

    /// Lets go of a surface's commands and fence.
    fn unpart(&self, shows: &Shows) {
        // SAFETY: neither is in use (every copy is waited for).
        unsafe {
            (self.on.vkDestroyFence)(self.device, shows.fence, null());
            (self.on.vkFreeCommandBuffers)(self.device, self.pool, 1, &shows.commands);
        }
    }

    /// Makes a surface's pixels: a buffer its size, in memory the CPU writes and the GPU reads,
    /// mapped. Says whether it could.
    fn buffer(&mut self, shows: &mut Shows) -> bool {
        let (on, device, [width, height]) = (&self.on, self.device, shows.size);
        let asked = VkBufferCreateInfo { kind: 12, size: width as u64 * height as u64 * 4, usage: USAGE_TRANSFER_SOURCE, ..Default::default() };
        let mut needs = VkMemoryRequirements::default();
        // SAFETY: Vulkan's documented calls on the device.
        unsafe {
            if (on.vkCreateBuffer)(device, &asked, null(), &mut shows.buffer) != 0 {
                return false;
            }
            (on.vkGetBufferMemoryRequirements)(device, shows.buffer, &mut needs);
        }
        let fitting = needs.type_bits & self.writable;
        let asked = VkMemoryAllocateInfo { kind: 5, size: needs.size, type_index: fitting.trailing_zeros(), ..Default::default() };
        let mut pixels = null_mut();
        // SAFETY: as above; the memory is bound whole and mapped whole.
        let made = unsafe {
            fitting != 0
                && (on.vkAllocateMemory)(device, &asked, null(), &mut shows.memory) == 0
                && (on.vkBindBufferMemory)(device, shows.buffer, shows.memory, 0) == 0
                && (on.vkMapMemory)(device, shows.memory, 0, needs.size, 0, &mut pixels) == 0
        };
        shows.pixels = pixels.cast();
        made && !pixels.is_null()
    }

    /// Lets go of a surface's pixels.
    fn unbuffer(&self, shows: &mut Shows) {
        // SAFETY: the GPU is done with them (every copy is waited for); null handles are let be.
        unsafe {
            (self.on.vkDestroyBuffer)(self.device, shows.buffer, null());
            (self.on.vkFreeMemory)(self.device, shows.memory, null());
        }
        (shows.buffer, shows.memory, shows.pixels) = (0, 0, null_mut());
    }

    /// An image `size` big, exported, and the compositor asked to make a buffer of it (through
    /// `dmabuf`). Nothing where any of it can't be had.
    fn image(&self, wire: &Wire, dmabuf: u32, [width, height]: [i32; 2], clear: bool) -> Option<Image> {
        let modifiers = if clear { &self.clear } else { &self.solid };
        let list = VkImageDrmFormatModifierListCreateInfoEXT { kind: 1_000_158_003, count: modifiers.len() as u32, modifiers: modifiers.as_ptr(), ..Default::default() };
        let external = VkExternalMemoryImageCreateInfo { kind: 1_000_072_001, next: (&raw const list).cast(), handles: DMA_BUF };
        let asked = VkImageCreateInfo {
            kind: 14,
            next: (&raw const external).cast(),
            image_type: 1,
            format: B8G8R8A8_UNORM,
            extent: [width as u32, height as u32, 1],
            mip_levels: 1,
            array_layers: 1,
            samples: 1,
            tiling: BY_MODIFIER,
            usage: USAGE_TRANSFER_DESTINATION,
            ..Default::default()
        };
        let mut image = Image {
            image: 0,
            memory: 0,
            params: 0,
            buffer: 0,
            shown: false,
            missing: vec![[0, 0, width, height]],
            #[cfg(test)]
            layout: (0, 0),
        };
        // SAFETY: Vulkan's documented call; everything chained lives until it returns.
        if unsafe { (self.on.vkCreateImage)(self.device, &asked, null(), &mut image.image) } != 0 {
            return None;
        }
        let Some((file, offset, pitch, modifier)) = self.backed(&mut image).then(|| self.exported(&image)).flatten() else {
            self.unmake(&image);
            return None;
        };
        #[cfg(test)]
        {
            image.layout = (offset as u64, pitch as u64);
        }
        let format = if clear { SEE_THROUGH } else { SOLID };
        image.params = wire.make(Interface::DmabufParams);
        wire.send(dmabuf, 1, &[Arg::Uint(image.params)]);
        wire.send(image.params, 1, &[Arg::File(file.as_raw_fd()), Arg::Uint(0), Arg::Uint(offset), Arg::Uint(pitch), Arg::Uint((modifier >> 32) as u32), Arg::Uint(modifier as u32)]);
        wire.send(image.params, 2, &[Arg::Int(width), Arg::Int(height), Arg::Uint(format), Arg::Uint(0)]);
        Some(image)
    }

    /// Gives an image memory of its own, the GPU's where it can, exportable as a dmabuf. Says
    /// whether it could.
    fn backed(&self, image: &mut Image) -> bool {
        let mut needs = VkMemoryRequirements::default();
        // SAFETY: Vulkan's documented call on the device's image.
        unsafe { (self.on.vkGetImageMemoryRequirements)(self.device, image.image, &mut needs) };
        let kinds = if needs.type_bits & self.local != 0 { needs.type_bits & self.local } else { needs.type_bits };
        let own = VkMemoryDedicatedAllocateInfo { kind: 1_000_127_001, image: image.image, ..Default::default() };
        let export = VkExportMemoryAllocateInfo { kind: 1_000_072_002, next: (&raw const own).cast(), handles: DMA_BUF };
        let asked = VkMemoryAllocateInfo { kind: 5, next: (&raw const export).cast(), size: needs.size, type_index: kinds.trailing_zeros() };
        // SAFETY: as above; everything chained lives until it returns.
        kinds != 0 && unsafe { (self.on.vkAllocateMemory)(self.device, &asked, null(), &mut image.memory) == 0 && (self.on.vkBindImageMemory)(self.device, image.image, image.memory, 0) == 0 }
    }

    /// An image's memory as a dmabuf, with its first plane's offset and row pitch, and the
    /// modifier the device chose. Nothing where it can't be had.
    fn exported(&self, image: &Image) -> Option<(OwnedFd, u32, u32, u64)> {
        let mut chosen = VkImageDrmFormatModifierPropertiesEXT { kind: 1_000_158_005, ..Default::default() };
        let (plane, mut layout, mut fd) = (VkImageSubresource { aspect: PLANE_0, level: 0, layer: 0 }, VkSubresourceLayout::default(), -1);
        let asked = VkMemoryGetFdInfoKHR { kind: 1_000_074_002, memory: image.memory, handle: DMA_BUF, ..Default::default() };
        // SAFETY: Vulkan's documented calls on the device's image and its memory.
        let said = unsafe {
            (self.on.vkGetImageSubresourceLayout)(self.device, image.image, &plane, &mut layout);
            [(self.on.vkGetImageDrmFormatModifierPropertiesEXT)(self.device, image.image, &mut chosen), (self.on.vkGetMemoryFdKHR)(self.device, &asked, &mut fd)]
        };
        // SAFETY: an fd Vulkan just gave this program.
        let file = (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })?;
        let (offset, pitch) = (u32::try_from(layout.offset).ok()?, u32::try_from(layout.row_pitch).ok()?);
        (said == [0; 2]).then_some((file, offset, pitch, chosen.modifier))
    }

    /// Destroys an image and its memory.
    fn unmake(&self, image: &Image) {
        // SAFETY: the GPU is done with it (every copy is waited for); null handles are let be.
        unsafe {
            (self.on.vkDestroyImage)(self.device, image.image, null());
            (self.on.vkFreeMemory)(self.device, image.memory, null());
        }
    }

    /// Copies `wanted` places of a surface's pixels into image `at`, taken back from the
    /// compositor and handed to it again, and waits until done. Says whether it was; if not,
    /// the device is lost.
    fn copy(&mut self, shows: &Shows, at: usize, wanted: &[[i32; 4]]) -> bool {
        #[cfg(test)]
        if self.faked {
            self.lost = true;
            return false;
        }
        let (on, commands, image) = (&self.on, shows.commands, &shows.images[at]);
        let regions: Vec<VkBufferImageCopy> = wanted.iter().filter(|place| place[0] < place[2] && place[1] < place[3]).map(|place| region(*place, shows.size[0])).collect();
        let (old, from, to) = if image.shown { (LAYOUT_GENERAL, self.outside, self.family) } else { (LAYOUT_UNDEFINED, !0, !0) };
        let taken = VkImageMemoryBarrier {
            kind: 45,
            destination_access: ACCESS_TRANSFER_WRITE,
            old_layout: old,
            new_layout: LAYOUT_TRANSFER_DESTINATION,
            source_family: from,
            destination_family: to,
            image: image.image,
            range: WHOLE,
            ..Default::default()
        };
        let given = VkImageMemoryBarrier {
            kind: 45,
            source_access: ACCESS_TRANSFER_WRITE,
            old_layout: LAYOUT_TRANSFER_DESTINATION,
            new_layout: LAYOUT_GENERAL,
            source_family: self.family,
            destination_family: self.outside,
            image: image.image,
            range: WHOLE,
            ..Default::default()
        };
        let begin = VkCommandBufferBeginInfo { kind: 42, flags: 0x1, ..Default::default() };
        // SAFETY: Vulkan's documented calls; the commands are not in use (their last run was
        // waited for), the buffer and image are the surface's.
        let recorded = unsafe {
            (on.vkResetFences)(self.device, 1, &shows.fence);
            (on.vkBeginCommandBuffer)(commands, &begin);
            (on.vkCmdPipelineBarrier)(commands, STAGE_TOP, STAGE_TRANSFER, 0, 0, null(), 0, null(), 1, &taken);
            if !regions.is_empty() {
                (on.vkCmdCopyBufferToImage)(commands, shows.buffer, image.image, LAYOUT_TRANSFER_DESTINATION, regions.len() as u32, regions.as_ptr());
            }
            (on.vkCmdPipelineBarrier)(commands, STAGE_TRANSFER, STAGE_BOTTOM, 0, 0, null(), 0, null(), 1, &given);
            (on.vkEndCommandBuffer)(commands)
        };
        let submit = VkSubmitInfo { kind: 4, buffer_count: 1, buffers: &commands, ..Default::default() };
        // SAFETY: as above; the fence is the surface's.
        let done = recorded == 0 && unsafe { (on.vkQueueSubmit)(self.queue, 1, &submit, shows.fence) == 0 && (on.vkWaitForFences)(self.device, 1, &shows.fence, 1, WAITED) == 0 };
        self.lost |= !done;
        done
    }
}

impl Drop for Vulkan {
    /// Lets go of the pool, the device and the instance. (Surfaces went first.)
    fn drop(&mut self) {
        // SAFETY: Vulkan's documented calls; nothing of either is used after.
        unsafe {
            (self.on.vkDeviceWaitIdle)(self.device);
            (self.on.vkDestroyCommandPool)(self.device, self.pool, null());
            (self.on.vkDestroyDevice)(self.device, null());
            (self.calls.vkDestroyInstance)(self.instance, null());
        }
    }
}

/// Destroys an instance or a device nothing came of, through its function `destroy` (if
/// found): nothing.
fn unmade(destroy: *const c_void, made: Object) -> Option<Vulkan> {
    // SAFETY: the function is Vulkan's, as vulkan_core.h declares it; what was made is not used
    // after.
    if !destroy.is_null() {
        unsafe { std::mem::transmute::<*const c_void, unsafe extern "system" fn(Object, *const c_void)>(destroy)(made, null()) };
    }
    None
}

impl Candidate {
    /// Device `physical`, where it can: Vulkan 1.2, the extensions needed, a queue that
    /// copies, and a layout for solid pixels both it and the compositor take (`takes`).
    fn of(calls: &Instance, physical: Object, takes: &[(u32, u64)], main: Option<u64>) -> Option<Candidate> {
        let mut properties = VkPhysicalDeviceProperties::default();
        // SAFETY: Vulkan's documented calls and enumerations, of the instance's device.
        unsafe { (calls.vkGetPhysicalDeviceProperties)(physical, &mut properties) };
        let extensions = |count: *mut u32, list: *mut VkExtensionProperties| unsafe { (calls.vkEnumerateDeviceExtensionProperties)(physical, null(), count, list) };
        let has = |name: &CStr| offered(extensions, name);
        if properties.api_version < VULKAN_1_2 || !NEEDED.iter().all(|name| has(name)) {
            return None;
        }
        let families = listed(VkQueueFamilyProperties::default(), |count, list| {
            unsafe { (calls.vkGetPhysicalDeviceQueueFamilyProperties)(physical, count, list) };
            0
        });
        // (Graphics 0x1, compute 0x2, transfer 0x4: any copies.)
        let family = families.iter().position(|family| family.count > 0 && family.flags & 0x7 != 0)? as u32;
        let ours = layouts(calls, physical);
        let both = |format: u32| takes.iter().filter(|(taken, modifier)| *taken == format && ours.contains(modifier)).map(|(_, modifier)| *modifier).collect::<Vec<u64>>();
        let (solid, clear) = (both(SOLID), both(SEE_THROUGH));
        let serves = has(NUMBERED) && main.is_some_and(|main| drives(calls, physical, main));
        let rank = if serves {
            0
        } else if properties.device_type == 4 {
            2
        } else {
            1
        };
        (!solid.is_empty()).then_some(Candidate { physical, family, solid, clear, foreign: has(TO_OTHERS), rank })
    }
}

/// The modifiers of one plane device `physical` copies 8-bit blue, green, red and alpha into
/// and exports as dmabufs.
fn layouts(calls: &Instance, physical: Object) -> Vec<u64> {
    let mut list = VkDrmFormatModifierPropertiesListEXT { kind: 1_000_158_000, ..Default::default() };
    let mut asked = VkFormatProperties2 { kind: 1_000_059_002, next: (&raw mut list).cast(), ..Default::default() };
    // SAFETY: Vulkan's documented call, asked twice: for the count, then for that many.
    unsafe { (calls.vkGetPhysicalDeviceFormatProperties2)(physical, B8G8R8A8_UNORM, &mut asked) };
    let mut all = vec![VkDrmFormatModifierPropertiesEXT::default(); list.count.min(64) as usize];
    (list.count, list.properties) = (all.len() as u32, all.as_mut_ptr());
    asked.next = (&raw mut list).cast();
    unsafe { (calls.vkGetPhysicalDeviceFormatProperties2)(physical, B8G8R8A8_UNORM, &mut asked) };
    all.truncate(list.count as usize);
    let usable = all.into_iter().filter(|layout| layout.planes == 1 && layout.features & COPIED_INTO != 0);
    usable.map(|layout| layout.modifier).filter(|modifier| exportable(calls, physical, *modifier)).collect()
}

/// Whether device `physical` exports images of 8-bit blue, green, red and alpha laid out by
/// `modifier`, copied into, as dmabufs.
fn exportable(calls: &Instance, physical: Object, modifier: u64) -> bool {
    let layout = VkPhysicalDeviceImageDrmFormatModifierInfoEXT { kind: 1_000_158_002, modifier, ..Default::default() };
    let external = VkPhysicalDeviceExternalImageFormatInfo { kind: 1_000_071_000, next: (&raw const layout).cast(), handle: DMA_BUF };
    let asked = VkPhysicalDeviceImageFormatInfo2 {
        kind: 1_000_059_004,
        next: (&raw const external).cast(),
        format: B8G8R8A8_UNORM,
        image_type: 1,
        tiling: BY_MODIFIER,
        usage: USAGE_TRANSFER_DESTINATION,
        flags: 0,
    };
    let mut outside = VkExternalImageFormatProperties { kind: 1_000_071_001, ..Default::default() };
    let mut said = VkImageFormatProperties2 { kind: 1_000_059_003, next: (&raw mut outside).cast(), ..Default::default() };
    // SAFETY: Vulkan's documented call; everything chained lives until it returns.
    unsafe { (calls.vkGetPhysicalDeviceImageFormatProperties2)(physical, &asked, &mut said) == 0 && outside.features & EXPORTABLE != 0 }
}

/// Whether device `physical` drives device number `main` (its primary or render node).
fn drives(calls: &Instance, physical: Object, main: u64) -> bool {
    let mut numbers = VkPhysicalDeviceDrmPropertiesEXT { kind: 1_000_353_000, ..Default::default() };
    let mut asked = VkPhysicalDeviceProperties2 { kind: 1_000_059_001, next: (&raw mut numbers).cast(), ..Default::default() };
    // SAFETY: Vulkan's documented call; it fills in both.
    unsafe { (calls.vkGetPhysicalDeviceProperties2)(physical, &mut asked) };
    let nodes = [(numbers.has_primary, numbers.primary), (numbers.has_render, numbers.render)];
    nodes.iter().any(|(has, [major, minor])| *has != 0 && device_number(*major as u64, *minor as u64) == main)
}

#[cfg(test)]
mod tests {
    use super::super::readable;
    use super::*;
    use crate::compositors::tests::{Desk, found};
    use crate::geom::Size;
    use crate::platform::{LocalClipboard, Window};
    use crate::surface::Surface;
    use crate::text::BuiltInFont;
    use crate::ui::Ui;
    use crate::x11::{Session, SystemText};
    use std::cell::RefCell;
    use std::os::unix::net::UnixStream;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    unsafe extern "C" {
        fn mmap(at: *mut c_void, length: usize, protection: c_int, flags: c_int, fd: c_int, offset: i64) -> *mut c_void;
        fn munmap(at: *mut c_void, length: usize) -> c_int;
    }

    /// The modifier of no layout said (the buffer's own), which taut never takes; and linear.
    const UNSAID: u64 = 0x00ff_ffff_ffff_ffff;
    const LINEAR: u64 = 0;
    /// The first buffer number a compositor gives.
    const THEIRS: u32 = 0xff00_0000;

    /// A test compositor at the other end of a socket, offering what shows windows, and
    /// `zwp_linux_dmabuf_v1` at a version (if any). With the program's link to it.
    fn offering(dmabuf: Option<u32>) -> (Wire, Link) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let compositor = Wire::over(theirs);
        let mut offers = vec![("wl_compositor", 6), ("wl_shm", 1), ("xdg_wm_base", 6)];
        offers.extend(dmabuf.map(|version| ("zwp_linux_dmabuf_v1", version)));
        // The program's registry is object 2, its first callback 3.
        for (name, (interface, version)) in offers.iter().enumerate() {
            compositor.send(2, 0, &[Arg::Uint(name as u32 + 1), Arg::Text(interface), Arg::Uint(*version)]);
        }
        compositor.send(3, 0, &[Arg::Uint(0)]);
        compositor.flush();
        (compositor, Link::over(Wire::over(ours)).unwrap().0)
    }

    /// What the program said to the compositor since last asked: object, opcode, arguments.
    fn said(compositor: &Wire, link: &Link) -> Vec<(u32, u16, Vec<i32>)> {
        link.wire.flush();
        compositor.read();
        std::iter::from_fn(|| compositor.next()).map(|message| (message.object, message.opcode, message.words())).collect()
    }

    /// The program takes in what the compositor said, as its windows would.
    fn hears(compositor: &Wire, link: &Link) {
        compositor.flush();
        link.wire.read();
        while let Some(message) = link.wire.next() {
            let mut gpu = link.gpu.borrow_mut();
            let gpu = gpu.as_mut().unwrap();
            match message.of {
                Interface::Buffer => _ = gpu.released(&link.wire, message.object),
                Interface::Dmabuf | Interface::DmabufFeedback | Interface::DmabufParams => gpu.heard(&link.wire, message),
                _ => {}
            }
        }
    }

    /// The compositor takes these formats and modifiers (v3).
    fn takes(compositor: &Wire, link: &Link, layouts: &[(u32, u64)]) {
        let dmabuf = link.gpu.borrow().as_ref().unwrap().dmabuf;
        for (format, modifier) in layouts {
            compositor.send(dmabuf, 0, &[Arg::Uint(*format)]);
            compositor.send(dmabuf, 1, &[Arg::Uint(*format), Arg::Uint((modifier >> 32) as u32), Arg::Uint(*modifier as u32)]);
        }
        hears(compositor, link);
    }

    /// Whether a device here exports dmabufs, laid out linearly. Where the machine is to have
    /// one (`TAUT_DMABUF_EXPORTS`, set on the test bed with `/dev/udmabuf`), it must.
    fn exports() -> bool {
        let exports = Vulkan::open(&[(SOLID, LINEAR)], None).is_some();
        assert!(exports || std::env::var_os("TAUT_DMABUF_EXPORTS").is_none(), "a device exporting dmabufs, as this machine is to have");
        exports
    }

    /// A label with a count and a button.
    fn counted(count: &u32, ui: &mut Ui) {
        ui.column().pad(10.0).gap(8.0).open();
        ui.label(&format!("{count} pressed"));
        ui.button("Press");
        ui.close();
    }

    /// An interface with the built-in font, this big, run once with a count.
    fn interface(size: Size, count: u32) -> Ui {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(size, 1.0);
        ui.run(&mut |ui| counted(&count, ui));
        ui
    }

    /// The count changes, and the surface hears what changed.
    fn recount(ui: &mut Ui, sheet: &mut Sheet, count: u32) {
        ui.run(&mut |ui| counted(&count, ui));
        sheet.changed.extend(ui.changes());
        sheet.waiting = 0;
    }

    /// The surface's part on the GPU.
    fn shows(sheet: &Sheet) -> &Shows {
        let Way::Gpu(shows) = &sheet.way else { panic!("not shown through the GPU") };
        shows
    }

    /// The pixels of the image last handed over, read from its dmabuf.
    fn handed(link: &Link, sheet: &Sheet) -> Vec<u32> {
        let shows = shows(sheet);
        let image = shows.images.iter().find(|image| image.shown && image.missing.is_empty()).expect("an image holding all of it");
        let gpu = link.gpu.borrow();
        let vulkan = gpu.as_ref().unwrap().vulkan.as_ref().unwrap();
        let asked = VkMemoryGetFdInfoKHR { kind: 1_000_074_002, memory: image.memory, handle: DMA_BUF, ..Default::default() };
        let mut fd = -1;
        assert_eq!(unsafe { (vulkan.on.vkGetMemoryFdKHR)(vulkan.device, &asked, &mut fd) }, 0);
        let file = unsafe { OwnedFd::from_raw_fd(fd) };
        let ((offset, pitch), [width, height]) = (image.layout, shows.size.map(|side| side as usize));
        let len = offset as usize + pitch as usize * height;
        // (1, 1: readable, shared.)
        let at = unsafe { mmap(null_mut(), len, 1, 1, file.as_raw_fd(), 0) };
        assert_ne!(at as isize, -1, "the dmabuf maps");
        let bytes = unsafe { std::slice::from_raw_parts(at as *const u8, len) };
        let rows = (0..height).map(|row| &bytes[offset as usize + row * pitch as usize..][..width * 4]);
        let pixels = rows.flat_map(|row| row.as_chunks::<4>().0.iter().map(|pixel| u32::from_ne_bytes(*pixel))).collect();
        unsafe { munmap(at, len) };
        pixels
    }

    /// The buffer requests the program made: their objects, each added a plane (fd, offset,
    /// stride, modifier) and asked to be made with `create` (never `create_immed`).
    fn requests(told: &[(u32, u16, Vec<i32>)], dmabuf: u32, size: [i32; 2], format: u32) -> Vec<u32> {
        let params: Vec<u32> = told.iter().filter(|(object, opcode, _)| (*object, *opcode) == (dmabuf, 1)).map(|(_, _, words)| words[0] as u32).collect();
        for params in &params {
            let of = |opcode: u16| told.iter().filter(move |(object, said, _)| (*object, *said) == (*params, opcode));
            let added: Vec<_> = of(1).collect();
            assert_eq!(added.len(), 1, "one plane");
            let [plane, offset, stride, high, low] = added[0].2[..] else { panic!("a plane's numbers") };
            assert_eq!((plane, (high as u64) << 32 | low as u32 as u64), (0, LINEAR), "the first plane, laid out linearly (all both take)");
            assert!(offset >= 0 && stride >= size[0] * 4, "its offset and stride: {offset}, {stride}");
            assert_eq!(of(2).map(|(_, _, words)| words.clone()).collect::<Vec<_>>(), [vec![size[0], size[1], format as i32, 0]]);
            assert_eq!(of(3).count(), 0, "never create_immed");
        }
        params
    }

    /// Each request made: answered with buffers from `first` on.
    fn made(compositor: &Wire, link: &Link, params: &[u32], first: u32) {
        params.iter().enumerate().for_each(|(at, params)| compositor.send(*params, 0, &[Arg::Uint(first + at as u32)]));
        hears(compositor, link);
    }

    /// The surface's damage, in pixels: left, top, right, bottom.
    fn damaged(told: &[(u32, u16, Vec<i32>)], surface: u32) -> Vec<[i32; 4]> {
        let damage = told.iter().filter(|(object, opcode, _)| (*object, *opcode) == (surface, 9));
        damage.map(|(_, _, words)| [words[0], words[1], words[0] + words[2], words[1] + words[3]]).collect()
    }

    /// The buffer handed over (attached).
    fn attached(told: &[(u32, u16, Vec<i32>)], surface: u32) -> Option<u32> {
        told.iter().find(|(object, opcode, _)| (*object, *opcode) == (surface, 1)).map(|(_, _, words)| words[0] as u32)
    }

    #[test]
    fn device_numbers_are_made_as_the_system_makes_them() {
        // (As glibc's makedev gives them. /dev/dri/renderD128 is 226, 128.)
        assert_eq!(device_number(226, 128), 0xe280);
        assert_eq!(device_number(226, 0x1234), 0x120_e234);
        assert_eq!(device_number(0x1_2345, 1), 0x1_2000_0003_4501);
        let entries = [(SOLID, LINEAR), (SEE_THROUGH, 0x0100_0000_0000_0001)];
        let bytes: Vec<u8> = entries.iter().flat_map(|(format, modifier)| [&format.to_ne_bytes()[..], &[0; 4], &modifier.to_ne_bytes()].concat()).collect();
        assert_eq!(table(&bytes), entries);
        assert_eq!(bounded(vec![[1, 2, 3, 4], [0, 0, 9, 9], [1, 2, 3, 4], [10, 0, 12, 2]]), [[0, 0, 9, 9], [10, 0, 12, 2]], "none inside another");
        assert_eq!(bounded((0..20).map(|at| [at, at, at + 2, at + 3]).collect()), [[0, 0, 21, 22]]);
    }

    /// v4: the default feedback is asked for; its format table, read (never mapped), and its
    /// tranches give what the compositor takes; its main device is kept.
    #[test]
    fn the_compositors_feedback_says_what_it_takes() {
        let (compositor, link) = offering(Some(5));
        ask(&link, Renderer::Vulkan);
        let told = said(&compositor, &link);
        let dmabuf = link.gpu.borrow().as_ref().unwrap().dmabuf;
        let bound = told.iter().find(|(object, opcode, words)| (*object, *opcode) == (2, 0) && words[0] == 4).expect("bound");
        assert_eq!(bound.2[bound.2.len() - 2..], [4, dmabuf as i32], "at version 4 at most");
        let feedback = told.iter().find(|(object, opcode, _)| (*object, *opcode) == (dmabuf, 2)).expect("its default feedback asked for").2[0] as u32;
        let entries = [(SOLID, UNSAID), (SOLID, LINEAR), (SEE_THROUGH, LINEAR), (u32::from_le_bytes(*b"NV12"), LINEAR)];
        let bytes: Vec<u8> = entries.iter().flat_map(|(format, modifier)| [&format.to_ne_bytes()[..], &[0; 4], &modifier.to_ne_bytes()].concat()).collect();
        let file = crate::wayland_wire::Shared::new(bytes.len()).unwrap();
        std::os::unix::fs::FileExt::write_all_at(&std::fs::File::from(file.file.try_clone().unwrap()), &bytes, 0).unwrap();
        compositor.send(feedback, 1, &[Arg::File(file.file.as_raw_fd()), Arg::Uint(bytes.len() as u32)]);
        compositor.send(feedback, 2, &[Arg::Bytes(&0xe280u64.to_ne_bytes())]);
        // Two tranches: the first the main device's (with a flag for scanout), the second
        // another's; each names entries of the table.
        let indices = |at: &[u16]| at.iter().flat_map(|index| index.to_ne_bytes()).collect::<Vec<u8>>();
        compositor.send(feedback, 4, &[Arg::Bytes(&0xe280u64.to_ne_bytes())]);
        compositor.send(feedback, 5, &[Arg::Bytes(&indices(&[1, 0]))]);
        compositor.send(feedback, 6, &[Arg::Uint(1)]);
        compositor.send(feedback, 3, &[]);
        compositor.send(feedback, 5, &[Arg::Bytes(&indices(&[2, 3, 9]))]);
        compositor.send(feedback, 3, &[]);
        hears(&compositor, &link);
        assert!(link.gpu.borrow().as_ref().unwrap().offered.is_empty(), "nothing taken before it is done");
        compositor.send(feedback, 0, &[]);
        hears(&compositor, &link);
        let gpu = link.gpu.borrow();
        let gpu = gpu.as_ref().unwrap();
        assert_eq!(gpu.offered, [entries[1], entries[0], entries[2], entries[3]], "the tranches' entries, in order (an index past the table is none)");
        assert_eq!(gpu.main, Some(0xe280));
    }

    /// Only a window asking for a GPU path binds the protocol; `Auto` and `Cpu` never do. A
    /// compositor without it, or without a layout both take, leaves shared memory.
    #[test]
    fn without_the_protocol_or_a_common_layout_shared_memory_shows_the_surface() {
        for renderer in [Renderer::Auto, Renderer::Cpu] {
            let (compositor, link) = offering(Some(3));
            ask(&link, renderer);
            assert!(link.gpu.borrow().is_none() && !said(&compositor, &link).iter().any(|(object, _, words)| *object == 2 && words[0] == 4), "{renderer:?}: not bound");
        }
        let mut ui = interface(Size { w: 120.0, h: 80.0 }, 0);
        for (offered, layouts) in [(None, vec![]), (Some(3), vec![(u32::from_le_bytes(*b"NV12"), LINEAR), (SOLID, UNSAID)])] {
            let (compositor, link) = offering(offered);
            ask(&link, Renderer::Vulkan);
            if offered.is_some() {
                takes(&compositor, &link, &layouts);
            }
            let mut sheet = Sheet::new(&link, false);
            sheet.size(&link, [120, 80], 1.0);
            assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)), "drawn at once");
            assert!(matches!(sheet.way, Way::Shm) && sheet.buffers[0].memory.pixels() == Surface::draw(&mut ui, 64).pixels, "by shared memory");
            let told = said(&compositor, &link);
            assert_eq!(attached(&told, sheet.surface), Some(sheet.buffers[0].id));
            sheet.destroy(&link);
        }
    }

    /// The surface's pixels go into three images, each a dmabuf the compositor is asked to make
    /// a buffer of; each frame is copied into one it does not hold, with what that one lacks:
    /// the compositor gets the CPU's pixels byte for byte, and is told only what changed. Where
    /// no device here exports dmabufs (WSL has no `/dev/udmabuf`), shared memory shows it.
    #[test]
    fn a_surface_is_handed_over_as_dmabufs_and_only_what_changed_is_copied() {
        let (compositor, link) = offering(Some(3));
        ask(&link, Renderer::Vulkan);
        takes(&compositor, &link, &[(SOLID, UNSAID), (SOLID, LINEAR), (SEE_THROUGH, LINEAR)]);
        let (exports, dmabuf, size) = (exports(), link.gpu.borrow().as_ref().unwrap().dmabuf, [200, 120]);
        let mut ui = interface(Size { w: 200.0, h: 120.0 }, 0);
        let mut sheet = Sheet::new(&link, false);
        sheet.size(&link, size, 1.0);
        let first = sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas));
        if !exports {
            assert!(first && matches!(sheet.way, Way::Shm) && sheet.buffers[0].memory.pixels() == Surface::draw(&mut ui, 64).pixels);
            sheet.destroy(&link);
            return eprintln!("no device here exports dmabufs: shared memory shows the surface");
        }
        // Nothing is handed over before the compositor has made a buffer.
        assert!(!first && sheet.buffers.is_empty(), "nothing yet, nor by shared memory");
        let told = said(&compositor, &link);
        let params = requests(&told, dmabuf, size, SOLID);
        assert_eq!(params.len(), IMAGES);
        made(&compositor, &link, &params, THEIRS);
        // (Its number is not used again before: another surface's request would get its answer.)
        assert!(!said(&compositor, &link).iter().any(|(object, opcode, _)| params.contains(object) && *opcode == 0), "a request lives until its image takes the answer");
        assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)));
        let told = said(&compositor, &link);
        assert!(told.iter().filter(|(object, opcode, _)| params.contains(object) && *opcode == 0).count() == IMAGES, "each request let go once its answer is taken");
        assert_eq!(attached(&told, sheet.surface), Some(THEIRS));
        assert_eq!(damaged(&told, sheet.surface), [[0, 0, 200, 120]], "all of it is new");
        assert_eq!(shows(&sheet).copied, [[0, 0, 200, 120]]);
        assert!(handed(&link, &sheet) == Surface::draw(&mut ui, 64).pixels, "the dmabuf holds the interface as drawn afresh");
        // The compositor holds the first: the next frame goes into the second, which lacks
        // everything; the compositor is told only what changed.
        recount(&mut ui, &mut sheet, 5);
        let changed: Vec<[i32; 4]> = sheet.changed.iter().map(|area| on_pixels(*area, 1.0, size)).collect();
        assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)));
        let told = said(&compositor, &link);
        assert_eq!((attached(&told, sheet.surface), damaged(&told, sheet.surface)), (Some(THEIRS + 1), changed.clone()));
        assert_eq!(shows(&sheet).copied, [[0, 0, 200, 120]], "an image not yet shown gets all of it");
        assert!(handed(&link, &sheet) == Surface::draw(&mut ui, 64).pixels);
        // Released, the first is drawn into again: only what it missed and what changed now.
        compositor.send(THEIRS, 0, &[]);
        hears(&compositor, &link);
        recount(&mut ui, &mut sheet, 7);
        let now: Vec<[i32; 4]> = sheet.changed.iter().map(|area| on_pixels(*area, 1.0, size)).collect();
        assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)));
        assert_eq!(attached(&said(&compositor, &link), sheet.surface), Some(THEIRS));
        assert_eq!(shows(&sheet).copied, bounded([changed, now].concat()), "what it missed, and what changed now");
        let copied: i32 = shows(&sheet).copied.iter().map(|[left, top, right, bottom]| (right - left) * (bottom - top)).sum();
        assert!(copied < 200 * 120 / 2, "a part, not all of it: {:?}", shows(&sheet).copied);
        assert!(handed(&link, &sheet) == Surface::draw(&mut ui, 64).pixels);
        // Held: the third. All held: nothing until one is released.
        recount(&mut ui, &mut sheet, 8);
        assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)));
        assert_eq!(attached(&said(&compositor, &link), sheet.surface), Some(THEIRS + 2));
        recount(&mut ui, &mut sheet, 9);
        assert!(!sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)), "every image held");
        assert!(!sheet.changed.is_empty() && attached(&said(&compositor, &link), sheet.surface).is_none());
        compositor.send(THEIRS + 1, 0, &[]);
        hears(&compositor, &link);
        assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)));
        assert_eq!(attached(&said(&compositor, &link), sheet.surface), Some(THEIRS + 1));
        assert!(handed(&link, &sheet) == Surface::draw(&mut ui, 64).pixels);
        sheet.destroy(&link);
        let gone = said(&compositor, &link);
        assert!((0..3).all(|at| gone.iter().any(|(object, opcode, _)| (*object, *opcode) == (THEIRS + at, 0))), "every buffer destroyed with the surface");
    }

    /// A surface of another size gets new pixels and images; the old images go, those the
    /// compositor holds once it releases them.
    #[test]
    fn a_resized_surface_gets_new_images_and_the_old_go_once_released() {
        let (compositor, link) = offering(Some(3));
        ask(&link, Renderer::Vulkan);
        takes(&compositor, &link, &[(SOLID, LINEAR)]);
        if !exports() {
            return eprintln!("no device here exports dmabufs: not tested");
        }
        let dmabuf = link.gpu.borrow().as_ref().unwrap().dmabuf;
        let mut ui = interface(Size { w: 100.0, h: 80.0 }, 0);
        let mut sheet = Sheet::new(&link, false);
        sheet.size(&link, [100, 80], 1.0);
        sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas));
        made(&compositor, &link, &requests(&said(&compositor, &link), dmabuf, [100, 80], SOLID), THEIRS);
        assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)));
        said(&compositor, &link);
        // Larger while the compositor holds the first buffer.
        ui.resize(Size { w: 160.0, h: 90.0 }, 1.0);
        ui.run(&mut |ui| counted(&3, ui));
        sheet.size(&link, [160, 90], 1.0);
        sheet.waiting = 0;
        assert!(!sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)), "new buffers asked for first");
        let told = said(&compositor, &link);
        let unheld: Vec<u32> = [THEIRS + 1, THEIRS + 2].into();
        assert!(unheld.iter().all(|buffer| told.iter().any(|(object, opcode, _)| (*object, *opcode) == (*buffer, 0))), "those not held go at once");
        assert!(!told.iter().any(|(object, opcode, _)| (*object, *opcode) == (THEIRS, 0)), "the held one stays");
        let params = requests(&told, dmabuf, [160, 90], SOLID);
        assert_eq!((params.len(), shows(&sheet).size), (IMAGES, [160, 90]));
        made(&compositor, &link, &params, THEIRS + 10);
        assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)));
        assert_eq!(attached(&said(&compositor, &link), sheet.surface), Some(THEIRS + 10));
        assert!(handed(&link, &sheet) == Surface::draw(&mut ui, 64).pixels, "all of it, at its new size");
        compositor.send(THEIRS, 0, &[]);
        hears(&compositor, &link);
        assert!(said(&compositor, &link).iter().any(|(object, opcode, _)| (*object, *opcode) == (THEIRS, 0)), "released, the old one goes");
        assert!(link.gpu.borrow().as_ref().unwrap().retired.is_empty());
        sheet.destroy(&link);
    }

    /// A buffer the compositor refuses, and a device lost, each put the surface on shared
    /// memory for good, drawn whole there.
    #[test]
    fn a_refused_buffer_or_a_lost_device_puts_the_surface_on_shared_memory() {
        if !exports() {
            return eprintln!("no device here exports dmabufs: not tested");
        }
        for refused in [true, false] {
            let (compositor, link) = offering(Some(3));
            ask(&link, Renderer::Vulkan);
            takes(&compositor, &link, &[(SOLID, LINEAR)]);
            let dmabuf = link.gpu.borrow().as_ref().unwrap().dmabuf;
            let mut ui = interface(Size { w: 100.0, h: 80.0 }, 0);
            let mut sheet = Sheet::new(&link, false);
            sheet.size(&link, [100, 80], 1.0);
            sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas));
            let params = requests(&said(&compositor, &link), dmabuf, [100, 80], SOLID);
            if refused {
                made(&compositor, &link, &params[..2], THEIRS);
                compositor.send(params[2], 1, &[]);
                hears(&compositor, &link);
            } else {
                made(&compositor, &link, &params, THEIRS);
                assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)) && matches!(sheet.way, Way::Gpu(_)));
                said(&compositor, &link);
                link.gpu.borrow_mut().as_mut().unwrap().lose();
                recount(&mut ui, &mut sheet, 1);
            }
            assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)), "drawn");
            assert!(matches!(sheet.way, Way::Shm), "by shared memory");
            let told = said(&compositor, &link);
            assert_eq!(attached(&told, sheet.surface), Some(sheet.buffers[0].id));
            assert_eq!(damaged(&told, sheet.surface), [[0, 0, 100, 80]], "all of it");
            assert!(sheet.buffers[0].memory.pixels() == Surface::draw(&mut ui, 64).pixels);
            // (Lost, the buffer the compositor holds goes once it releases it.)
            let gone = if refused { [THEIRS, THEIRS + 1] } else { [THEIRS + 1, THEIRS + 2] };
            assert!(gone.iter().all(|buffer| told.iter().any(|(object, opcode, _)| (*object, *opcode) == (*buffer, 0))), "the buffers made go");
            recount(&mut ui, &mut sheet, 2);
            assert!(sheet.draw(&link, 1.0, &mut |canvas| ui.paint(canvas)) && matches!(sheet.way, Way::Shm), "for good");
            sheet.destroy(&link);
        }
    }

    /// A test app: a count and a button, in a window asking for Vulkan.
    type Windows = Host<Box<dyn FnMut(&mut Ui)>>;

    /// Opens the app's window on the compositor the environment names. Nothing where none
    /// answers.
    fn opened(count: Rc<RefCell<u32>>, renderer: Renderer, [width, height]: [f32; 2]) -> Option<Windows> {
        let (link, outputs) = Link::new()?;
        let window = Window { title: "taut through dmabufs", width, height, renderer, ..Default::default() };
        let app = Box::new(move |ui: &mut Ui| counted(&count.borrow(), ui)) as Box<dyn FnMut(&mut Ui)>;
        Host::open(link, outputs, SystemText::new()?, Session::on(None), window, app)
    }

    /// Whether a window's surface is drawn as it is now: nothing changed since, and an image
    /// (or buffer) holds all of it.
    fn drawn(host: &Windows) -> bool {
        let sheet = &host.windows[0].sheet;
        let held = match &sheet.way {
            Way::Gpu(shows) => shows.images.iter().any(|image| image.shown && image.missing.is_empty()),
            _ => sheet.buffers.iter().any(|buffer| buffer.missing.is_empty()),
        };
        host.windows[0].configured && sheet.changed.is_empty() && held
    }

    /// Takes turns until something holds, up to five seconds. Says whether it came to hold.
    fn waits(host: &mut Windows, so: impl Fn(&Windows) -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while !so(host) && Instant::now() < until {
            readable(host.link.wire.fd(), 10);
            host.turn();
        }
        so(host)
    }

    /// Where the window's surface is on the screen, if the tests can see it and point.
    fn corner(host: &mut Windows, desk: &mut Desk) -> Option<crate::geom::Point> {
        let surface = host.windows[0].sheet.surface;
        found(desk, &mut |_| {
            readable(host.link.wire.fd(), 10);
            host.turn();
            (host.pointer.over == surface).then_some(host.pointer.at)
        })
    }

    /// Whether the screen shows the window's surface as the interface is drawn afresh, pixel
    /// for pixel (retaken for up to three seconds: a compositor shows a frame at its next
    /// composite). Nothing where the tests can't see the screen.
    fn on_screen(host: &mut Windows, desk: &mut Desk, corner: crate::geom::Point) -> Option<usize> {
        let afresh = Surface::draw(&mut host.ui, 64);
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let (width, pixels) = desk.picture()?;
            let (left, top) = (corner.x.round() as usize, corner.y.round() as usize);
            let inside = (0..afresh.height).flat_map(|row| (0..afresh.width).map(move |column| (row, column)));
            let differ =
                inside.filter(|(row, column)| pixels.get((top + row) * width + left + column).is_none_or(|shown| shown & 0xff_ffff != afresh.pixels[row * afresh.width + column] & 0xff_ffff)).count();
            if differ == 0 || Instant::now() > until {
                return Some(differ);
            }
            readable(host.link.wire.fd(), 100);
            host.turn();
        }
    }

    /// On a real compositor: a window asking for Vulkan is shown through dmabufs where the
    /// compositor takes them and a device exports them, else by shared memory; either way
    /// byte for byte what the CPU draws, on the screen too where the tests see it. A change
    /// copies only what changed. A lost device puts it back on shared memory, still exact.
    #[test]
    #[ignore]
    fn a_window_asking_for_vulkan_is_shown_byte_for_byte_on_the_compositor() {
        let _alone = crate::x11::tests::alone();
        let count = Rc::new(RefCell::new(0));
        let Some(mut host) = opened(count.clone(), Renderer::Vulkan, [320.0, 240.0]) else { return eprintln!("no compositor: not tested") };
        assert!(waits(&mut host, drawn), "the window is drawn");
        let takes_dmabufs = host.link.has.dmabuf.0 != 0;
        let mut desk = Desk::new();
        let corner = desk.as_mut().filter(|desk| desk.has_hands()).and_then(|desk| corner(&mut host, desk));
        // The pointer goes to the screen's corner farthest from the window: the compositor
        // draws it over what it shows.
        if let (Some(desk), Some(at)) = (desk.as_mut(), corner) {
            let [wide, high] = desk.units();
            desk.point(crate::geom::pt(if at.x > wide / 2.0 { 2.0 } else { wide - 3.0 }, if at.y > high / 2.0 { 2.0 } else { high - 3.0 }));
        }
        let afresh = Surface::draw(&mut host.ui, 64);
        if !takes_dmabufs || !exports() {
            assert!(matches!(host.windows[0].sheet.way, Way::Shm), "by shared memory");
            assert_eq!(host.ui.renderer(), Renderer::Cpu);
            let buffer = host.windows[0].sheet.buffers.iter_mut().find(|buffer| buffer.missing.is_empty()).unwrap();
            assert!(buffer.memory.pixels() == afresh.pixels);
            if let (Some(desk), Some(corner)) = (desk.as_mut(), corner) {
                assert_eq!(on_screen(&mut host, desk, corner), Some(0), "the screen shows it");
            }
            return eprintln!("the compositor takes no dmabufs ({takes_dmabufs}) or no device exports them: shared memory shows the window");
        }
        assert!(matches!(host.windows[0].sheet.way, Way::Gpu(_)), "through the GPU");
        assert_eq!(host.ui.renderer(), Renderer::Vulkan, "and the interface is told so");
        assert!(handed(&host.link, &host.windows[0].sheet) == afresh.pixels, "the dmabuf holds what the CPU draws");
        match (desk.as_mut(), corner) {
            (Some(desk), Some(corner)) => assert_eq!(on_screen(&mut host, desk, corner), Some(0), "the screen shows it, pixel for pixel"),
            _ => eprintln!("the tests can't see this compositor's screen: the dmabuf's pixels compared only"),
        }
        // Changes: once an image was shown, only what it lacks is copied into it.
        let [wide, high] = host.windows[0].sheet.pixels;
        let mut small = 0;
        for at in 1..=5 {
            *count.borrow_mut() = at;
            host.rerun = true;
            assert!(waits(&mut host, |host| !host.rerun && drawn(host)), "change {at} drawn");
            let Way::Gpu(shows) = &host.windows[0].sheet.way else { panic!("still through the GPU") };
            small += shows.copied.iter().all(|[left, top, right, bottom]| (right - left) * (bottom - top) < wide * high / 4) as u32;
        }
        assert!(small >= 2, "only what changed was copied, once each image was shown ({small} of 5)");
        assert!(handed(&host.link, &host.windows[0].sheet) == Surface::draw(&mut host.ui, 64).pixels);
        if let (Some(desk), Some(corner)) = (desk.as_mut(), corner) {
            assert_eq!(on_screen(&mut host, desk, corner), Some(0));
        }
        // The device is lost: shared memory shows the window from then on, exactly.
        host.link.gpu.borrow_mut().as_mut().unwrap().lose();
        *count.borrow_mut() = 6;
        host.rerun = true;
        assert!(waits(&mut host, |host| !host.rerun && matches!(host.windows[0].sheet.way, Way::Shm) && drawn(host)), "shared memory shows it");
        let afresh = Surface::draw(&mut host.ui, 64);
        let buffer = host.windows[0].sheet.buffers.iter_mut().find(|buffer| buffer.missing.is_empty()).unwrap();
        assert!(buffer.memory.pixels() == afresh.pixels);
        assert_eq!(host.ui.renderer(), Renderer::Cpu, "the interface is told");
        if let (Some(desk), Some(corner)) = (desk.as_mut(), corner) {
            assert_eq!(on_screen(&mut host, desk, corner), Some(0), "the screen shows it after the fall");
        }
    }

    /// What a window's surface waits for, for a test that failed.
    fn state(host: &Windows) -> String {
        let sheet = &host.windows[0].sheet;
        let gpu = host.link.gpu.borrow();
        let held = gpu.as_ref().map(|gpu| gpu.held.clone()).unwrap_or_default();
        let images: Vec<u32> = match &sheet.way {
            Way::Gpu(shows) => shows.images.iter().map(|image| image.buffer).collect(),
            _ => sheet.buffers.iter().map(|buffer| buffer.id | (buffer.busy as u32) << 31).collect(),
        };
        format!("waiting {}, changed {:?}, images or buffers {images:x?}, held {held:x?}", sheet.waiting, sheet.changed)
    }

    /// Whether a window may draw at once: the compositor showed its last frame, and holds not
    /// every image (or buffer).
    fn ready(host: &Windows) -> bool {
        let sheet = &host.windows[0].sheet;
        let gpu = host.link.gpu.borrow();
        let free = match (&sheet.way, gpu.as_ref()) {
            (Way::Gpu(shows), Some(gpu)) => shows.images.iter().any(|image| image.buffer != 0 && !gpu.held.contains(&image.buffer)),
            _ => sheet.buffers.len() < 2 || sheet.buffers.iter().any(|buffer| !buffer.busy),
        };
        sheet.waiting == 0 && free
    }

    /// On a real compositor: how long drawing a frame takes the program (painting, and through
    /// the GPU the copy and its wait) and how long until the compositor has shown it, through
    /// dmabufs and through shared memory: for a small change (a label) and for all of an 800
    /// by 500 window. Prints the medians of 60 frames.
    #[test]
    #[ignore]
    fn how_long_frames_take_through_dmabufs_and_shared_memory() {
        let _alone = crate::x11::tests::alone();
        for renderer in [Renderer::Vulkan, Renderer::Cpu] {
            let count = Rc::new(RefCell::new(0));
            let Some(mut host) = opened(count.clone(), renderer, [800.0, 500.0]) else { return eprintln!("no compositor: not tested") };
            assert!(waits(&mut host, drawn), "the window is drawn");
            let way = if matches!(host.windows[0].sheet.way, Way::Gpu(_)) { "dmabufs" } else { "shared memory" };
            let pixels = host.windows[0].sheet.pixels;
            for whole in [false, true] {
                let (mut drawing, mut shown) = (Vec::new(), Vec::new());
                for at in 1..=60 {
                    assert!(waits(&mut host, ready), "ready to draw: frame {at}, {}", state(&host));
                    *count.borrow_mut() = at;
                    host.rerun = true;
                    host.describe();
                    if whole {
                        let units = host.windows[0].sheet.units;
                        host.windows[0].sheet.changed = vec![rect(0.0, 0.0, units[0] as f32, units[1] as f32)];
                    }
                    let start = Instant::now();
                    host.draw(0);
                    drawing.push(start.elapsed());
                    assert!(host.windows[0].sheet.changed.is_empty(), "drawn at once");
                    assert!(waits(&mut host, |host| host.windows[0].sheet.waiting == 0), "shown");
                    shown.push(start.elapsed());
                }
                drawing.sort();
                shown.sort();
                let what = if whole { "all of it" } else { "a label" };
                eprintln!(
                    "TIMES {way}, {what}, {}x{} pixels: drawing {:.3} ms, shown after {:.3} ms (medians of 60)",
                    pixels[0],
                    pixels[1],
                    drawing[30].as_secs_f64() * 1e3,
                    shown[30].as_secs_f64() * 1e3
                );
            }
        }
    }
}
