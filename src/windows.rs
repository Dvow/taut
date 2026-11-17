//! Windows: a plain Win32 window, the system's own text drawing (GDI), and the clipboard,
//! called directly.
//!
//! The window is drawn through one small block of pixels (a strip the width of the window),
//! filled and copied to the window as often as it takes to cover what changed. Windows draws
//! the letters into the same block, so they look as everywhere else on the system, in the
//! person's font smoothing.
//!
//! Second windows are of the same kind, each with its own messages, and end with the first. All
//! are drawn through the one strip, as wide as the widest: more windows take no more drawing
//! memory, and one text system writes in all.
//!
//! A frameless window is an ordinary window whose client area is all of it: it says so to
//! `WM_NCCALCSIZE`, and answers `WM_NCHITTEST` from what `frameless` works out, so the system
//! still moves, resizes and snaps it by the strip and edges the app gives. A see-through window
//! is the one kind not drawn through the strip: Windows shows it from a full picture the window
//! keeps and draws into (`SeeThrough`, a layered window).
//!
//! Drag and drop from other programs comes through OLE: the window registers a hand-written COM
//! `DropTarget`; dropped text arrives the same way. Dragging out to other programs also goes
//! through OLE, in `ole_drag`: the window hands it over between two messages and hears what
//! became of it. (Screen reader requests are in `uia`: the window only passes their messages
//! on.)
use crate::canvas::Canvas;
use crate::color::Color;
use crate::event::{Button, Cursor, Event, Key, Mods};
use crate::file_dialog::FileDialogRequest;
use crate::frameless::HitTest;
use crate::geom::{Rect, Size, pt, rect};
use crate::id::Id;
use crate::platform::{Clipboard, PicturesAndFiles, Window, WindowPlacement};
use crate::screens::{OpenAt, Screen, window_origin};
use crate::second_window::{WindowKind, WindowOpening};
use crate::text::{Face, Font, Metrics, TextSystem};
use crate::touch_windows::{self, WM_POINTERCAPTURECHANGED, WM_POINTERUPDATE};
use crate::ui::Ui;
use crate::uia;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

/// The Windows functions this library calls, declared here.
#[allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]
pub(crate) mod sys {
    pub type HANDLE = isize;
    pub type WNDPROC = unsafe extern "system" fn(HANDLE, u32, usize, isize) -> isize;
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct POINT {
        pub x: i32,
        pub y: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct RECT {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }
    /// `RGNDATA` with room for eight rectangles: the header (the third field is the count),
    /// their bounding rectangle, and the rectangles.
    #[repr(C)]
    #[derive(Default)]
    pub struct Stale {
        pub header: [u32; 4],
        pub around: RECT,
        pub parts: [RECT; 8],
    }
    #[repr(C)]
    pub struct WNDCLASSW {
        pub style: u32,
        pub proc: Option<WNDPROC>,
        pub class_extra: i32,
        pub window_extra: i32,
        pub instance: HANDLE,
        pub icon: HANDLE,
        pub cursor: HANDLE,
        pub background: HANDLE,
        pub menu: *const u16,
        pub name: *const u16,
    }
    /// `ICONINFO`: an icon (1), not a cursor; the hot spot; a 1-bit mask; and the colour
    /// bitmap.
    #[repr(C)]
    #[derive(Default)]
    pub struct ICONINFO {
        pub icon: i32,
        pub x: u32,
        pub y: u32,
        pub mask: HANDLE,
        pub color: HANDLE,
    }
    #[repr(C)]
    #[derive(Default)]
    pub struct MSG {
        pub hwnd: HANDLE,
        pub message: u32,
        pub wparam: usize,
        pub lparam: isize,
        pub time: u32,
        pub pt: POINT,
    }
    #[repr(C)]
    pub struct PAINTSTRUCT {
        pub hdc: HANDLE,
        pub erase: i32,
        pub paint: RECT,
        pub restore: i32,
        pub update: i32,
        pub reserved: [u8; 32],
    }
    #[repr(C)]
    #[derive(Default)]
    pub struct BITMAPINFO {
        pub size: u32,
        pub width: i32,
        pub height: i32,
        pub planes: u16,
        pub bits: u16,
        pub compression: u32,
        pub image_size: u32,
        pub x_per_meter: i32,
        pub y_per_meter: i32,
        pub colors_used: u32,
        pub colors_important: u32,
        pub colors: [u32; 1],
    }
    #[repr(C)]
    #[derive(Default)]
    pub struct TEXTMETRICW {
        pub height: i32,
        pub ascent: i32,
        pub descent: i32,
        pub internal_leading: i32,
        pub external_leading: i32,
        pub ave_char_width: i32,
        pub max_char_width: i32,
        pub weight: i32,
        pub overhang: i32,
        pub aspect_x: i32,
        pub aspect_y: i32,
        pub chars: [u16; 4],
        pub flags: [u8; 5],
    }
    /// `GLYPHMETRICS`.
    #[repr(C)]
    #[derive(Default)]
    pub struct GLYPHMETRICS {
        pub wide: u32,
        pub high: u32,
        pub origin: POINT,
        pub advance: [i16; 2],
    }
    #[repr(C)]
    pub struct TRACKMOUSEEVENT {
        pub size: u32,
        pub flags: u32,
        pub hwnd: HANDLE,
        pub hover_time: u32,
    }
    #[repr(C)]
    pub struct MINMAXINFO {
        pub reserved: POINT,
        pub max_size: POINT,
        pub max_position: POINT,
        pub min_track: POINT,
        pub max_track: POINT,
    }
    /// `COMPOSITIONFORM`: where an input method shows the text being composed, in window
    /// pixels.
    #[repr(C)]
    pub struct COMPOSITIONFORM {
        pub style: u32,
        pub at: POINT,
        pub area: RECT,
    }
    /// `CANDIDATEFORM`: where an input method shows its candidate list.
    #[repr(C)]
    pub struct CANDIDATEFORM {
        pub index: u32,
        pub style: u32,
        pub at: POINT,
        pub area: RECT,
    }
    /// `FORMATETC`.
    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct FORMATETC {
        pub format: u16,
        pub device: *const core::ffi::c_void,
        pub aspect: u32,
        pub index: i32,
        pub medium: u32,
    }
    /// `STGMEDIUM`: the medium type, the data (a memory handle here), and who releases it, if
    /// not Windows.
    #[repr(C)]
    pub struct STGMEDIUM {
        pub medium: u32,
        pub memory: HANDLE,
        pub owner: *mut core::ffi::c_void,
    }
    /// `UPDATELAYEREDWINDOWINFO`: its size, the window's screen place and size (unchanged
    /// here), the picture's DC and origin, a colour key (none), the blend, its flags, and the
    /// dirty rectangle.
    #[repr(C)]
    pub struct Layered {
        pub size: u32,
        pub screen: HANDLE,
        pub at: *const POINT,
        pub big: *const POINT,
        pub picture: HANDLE,
        pub from: *const POINT,
        pub key: u32,
        pub blend: *const [u8; 4],
        pub flags: u32,
        pub dirty: *const RECT,
    }
    /// `MONITORINFO`: the whole monitor and its work area (what the task bar leaves).
    #[repr(C)]
    pub struct MONITORINFO {
        pub size: u32,
        pub all: RECT,
        pub work: RECT,
        pub flags: u32,
    }
    /// `IDataObject`: a pointer to its vtable.
    #[repr(C)]
    pub struct IDataObject {
        pub table: *const IDataObjectVtbl,
    }
    /// `IDataObject`'s vtable, as far as this library calls.
    #[repr(C)]
    pub struct IDataObjectVtbl {
        pub unknown: [usize; 3],
        pub get_data: unsafe extern "system" fn(*mut IDataObject, *const FORMATETC, *mut STGMEDIUM) -> i32,
        pub get_data_here: usize,
        pub query_get_data: unsafe extern "system" fn(*mut IDataObject, *const FORMATETC) -> i32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetModuleHandleW(name: *const u16) -> HANDLE;
        pub fn GlobalAlloc(flags: u32, bytes: usize) -> HANDLE;
        pub fn GlobalLock(mem: HANDLE) -> *mut u16;
        pub fn GlobalUnlock(mem: HANDLE) -> i32;
        pub fn GlobalSize(mem: HANDLE) -> usize;
        pub fn GlobalFree(mem: HANDLE) -> HANDLE;
        pub fn MultiByteToWideChar(page: u32, flags: u32, bytes: *const u8, len: i32, units: *mut u16, room: i32) -> i32;
        pub fn GetLocalTime(time: *mut [u16; 8]);
        pub fn GetLogicalDrives() -> u32;
        pub fn SetThreadErrorMode(mode: u32, was: *mut u32) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn RegisterClassW(class: *const WNDCLASSW) -> u16;
        pub fn CreateWindowExW(
            ex: u32,
            class: *const u16,
            title: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: HANDLE,
            menu: HANDLE,
            instance: HANDLE,
            param: *mut core::ffi::c_void,
        ) -> HANDLE;
        pub fn DestroyWindow(hwnd: HANDLE) -> i32;
        pub fn DefWindowProcW(hwnd: HANDLE, msg: u32, w: usize, l: isize) -> isize;
        pub fn GetMessageW(msg: *mut MSG, hwnd: HANDLE, min: u32, max: u32) -> i32;
        pub fn TranslateMessage(msg: *const MSG) -> i32;
        pub fn DispatchMessageW(msg: *const MSG) -> isize;
        pub fn PostQuitMessage(code: i32);
        pub fn PostMessageW(hwnd: HANDLE, msg: u32, w: usize, l: isize) -> i32;
        pub fn SendMessageW(hwnd: HANDLE, msg: u32, w: usize, l: isize) -> isize;
        pub fn SetWindowTextW(hwnd: HANDLE, text: *const u16) -> i32;
        pub fn ShowWindow(hwnd: HANDLE, how: i32) -> i32;
        pub fn IsWindowVisible(hwnd: HANDLE) -> i32;
        pub fn GetForegroundWindow() -> HANDLE;
        pub fn GetFocus() -> HANDLE;
        pub fn GetWindowRect(hwnd: HANDLE, area: *mut RECT) -> i32;
        pub fn MonitorFromWindow(hwnd: HANDLE, otherwise: u32) -> HANDLE;
        pub fn MonitorFromRect(area: *const RECT, otherwise: u32) -> HANDLE;
        pub fn IsZoomed(hwnd: HANDLE) -> i32;
        pub fn IsIconic(hwnd: HANDLE) -> i32;
        pub fn SetForegroundWindow(hwnd: HANDLE) -> i32;
        pub fn GetWindowPlacement(hwnd: HANDLE, place: *mut [i32; 11]) -> i32;
        pub fn SetWindowPlacement(hwnd: HANDLE, place: *const [i32; 11]) -> i32;
        pub fn GetMonitorInfoW(screen: HANDLE, info: *mut MONITORINFO) -> i32;
        pub fn EnumDisplayMonitors(dc: HANDLE, clip: *const RECT, each: unsafe extern "system" fn(HANDLE, HANDLE, *mut RECT, isize) -> i32, with: isize) -> i32;
        pub fn EnableWindow(hwnd: HANDLE, on: i32) -> i32;
        pub fn ShutdownBlockReasonCreate(hwnd: HANDLE, why: *const u16) -> i32;
        pub fn ShutdownBlockReasonDestroy(hwnd: HANDLE) -> i32;
        pub fn SetWindowPos(hwnd: HANDLE, after: HANDLE, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
        pub fn InvalidateRect(hwnd: HANDLE, area: *const RECT, erase: i32) -> i32;
        pub fn UpdateLayeredWindowIndirect(hwnd: HANDLE, how: *const Layered) -> i32;

        pub fn GetUpdateRgn(hwnd: HANDLE, region: HANDLE, erase: i32) -> i32;
        pub fn BeginPaint(hwnd: HANDLE, ps: *mut PAINTSTRUCT) -> HANDLE;
        pub fn EndPaint(hwnd: HANDLE, ps: *const PAINTSTRUCT) -> i32;
        pub fn GetClientRect(hwnd: HANDLE, area: *mut RECT) -> i32;
        pub fn LoadCursorW(instance: HANDLE, name: *const u16) -> HANDLE;
        pub fn LoadIconW(instance: HANDLE, name: *const u16) -> HANDLE;
        pub fn CreateIconIndirect(parts: *const ICONINFO) -> HANDLE;
        #[cfg_attr(target_pointer_width = "32", link_name = "SetClassLongW")]
        pub fn SetClassLongPtrW(hwnd: HANDLE, which: i32, value: isize) -> usize;
        pub fn GetSystemMetrics(which: i32) -> i32;
        pub fn SetCursor(cursor: HANDLE) -> HANDLE;
        pub fn SetCapture(hwnd: HANDLE) -> HANDLE;
        pub fn GetCapture() -> HANDLE;
        pub fn ReleaseCapture() -> i32;
        pub fn TrackMouseEvent(track: *mut TRACKMOUSEEVENT) -> i32;
        pub fn ScreenToClient(hwnd: HANDLE, point: *mut POINT) -> i32;
        pub fn ClientToScreen(hwnd: HANDLE, point: *mut POINT) -> i32;
        pub fn GetCursorPos(point: *mut POINT) -> i32;
        pub fn WindowFromPoint(point: POINT) -> HANDLE;
        pub fn GetKeyState(key: i32) -> i16;
        pub fn SetTimer(hwnd: HANDLE, id: usize, ms: u32, proc: usize) -> usize;
        pub fn KillTimer(hwnd: HANDLE, id: usize) -> i32;
        pub fn GetDpiForWindow(hwnd: HANDLE) -> u32;
        pub fn SetProcessDpiAwarenessContext(context: isize) -> i32;
        pub fn AdjustWindowRectExForDpi(area: *mut RECT, style: u32, menu: i32, ex: u32, dpi: u32) -> i32;
        // On 32-bit Windows these exist only under their older names.
        #[cfg_attr(target_pointer_width = "32", link_name = "SetWindowLongW")]
        pub fn SetWindowLongPtrW(hwnd: HANDLE, index: i32, value: isize) -> isize;
        #[cfg_attr(target_pointer_width = "32", link_name = "GetWindowLongW")]
        pub fn GetWindowLongPtrW(hwnd: HANDLE, index: i32) -> isize;
        pub fn OpenClipboard(owner: HANDLE) -> i32;
        pub fn CloseClipboard() -> i32;
        pub fn EmptyClipboard() -> i32;
        pub fn GetClipboardData(format: u32) -> HANDLE;
        pub fn SetClipboardData(format: u32, mem: HANDLE) -> HANDLE;
        pub fn RegisterClipboardFormatW(name: *const u16) -> u32;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        pub fn CreateCompatibleDC(dc: HANDLE) -> HANDLE;
        pub fn CreateBitmap(width: i32, height: i32, planes: u32, bits: u32, pixels: *const u8) -> HANDLE;
        pub fn CreateDIBSection(dc: HANDLE, info: *const BITMAPINFO, usage: u32, bits: *mut *mut u32, section: HANDLE, offset: u32) -> HANDLE;
        pub fn SelectObject(dc: HANDLE, object: HANDLE) -> HANDLE;
        pub fn GetStockObject(which: i32) -> HANDLE;
        pub fn DeleteObject(object: HANDLE) -> i32;
        pub fn DeleteDC(dc: HANDLE) -> i32;
        pub fn CreateRectRgn(left: i32, top: i32, right: i32, bottom: i32) -> HANDLE;
        #[cfg(test)]
        pub fn CombineRgn(into: HANDLE, one: HANDLE, other: HANDLE, how: i32) -> i32;
        pub fn GetRegionData(region: HANDLE, room: u32, data: *mut Stale) -> u32;
        pub fn BitBlt(dest: HANDLE, x: i32, y: i32, w: i32, h: i32, src: HANDLE, sx: i32, sy: i32, op: u32) -> i32;
        pub fn CreateFontW(
            height: i32,
            width: i32,
            escapement: i32,
            orientation: i32,
            weight: i32,
            italic: u32,
            underline: u32,
            strike: u32,
            charset: u32,
            out: u32,
            clip: u32,
            quality: u32,
            pitch: u32,
            face: *const u16,
        ) -> HANDLE;
        pub fn SetTextColor(dc: HANDLE, color: u32) -> u32;
        pub fn SetBkMode(dc: HANDLE, mode: i32) -> i32;
        pub fn TextOutW(dc: HANDLE, x: i32, y: i32, text: *const u16, len: i32) -> i32;
        pub fn GetTextExtentExPointW(dc: HANDLE, text: *const u16, len: i32, max: i32, fit: *mut i32, each: *mut i32, size: *mut POINT) -> i32;
        pub fn GetTextMetricsW(dc: HANDLE, metrics: *mut TEXTMETRICW) -> i32;
        pub fn GetTextFaceW(dc: HANDLE, room: i32, name: *mut u16) -> i32;
        pub fn SaveDC(dc: HANDLE) -> i32;
        pub fn RestoreDC(dc: HANDLE, saved: i32) -> i32;
        pub fn IntersectClipRect(dc: HANDLE, left: i32, top: i32, right: i32, bottom: i32) -> i32;
        pub fn SetViewportOrgEx(dc: HANDLE, x: i32, y: i32, old: *mut POINT) -> i32;
        pub fn GdiFlush() -> i32;
        pub fn GetGlyphOutlineW(dc: HANDLE, letter: u32, format: u32, metrics: *mut GLYPHMETRICS, size: u32, buffer: *mut u8, turn: *const [u16; 8]) -> u32;
    }
    // A library every windowed program loads already, through shell32.
    #[link(name = "shcore")]
    unsafe extern "system" {
        pub fn GetDpiForMonitor(screen: HANDLE, kind: u32, x: *mut u32, y: *mut u32) -> i32;
    }
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        pub fn DwmSetWindowAttribute(hwnd: HANDLE, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
    }
    #[link(name = "imm32")]
    unsafe extern "system" {
        pub fn ImmGetContext(hwnd: HANDLE) -> HANDLE;
        pub fn ImmReleaseContext(hwnd: HANDLE, context: HANDLE) -> i32;
        pub fn ImmSetCompositionWindow(context: HANDLE, form: *const COMPOSITIONFORM) -> i32;
        pub fn ImmSetCandidateWindow(context: HANDLE, form: *const CANDIDATEFORM) -> i32;
        pub fn ImmAssociateContextEx(hwnd: HANDLE, context: HANDLE, how: u32) -> i32;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        pub fn DragFinish(drop: HANDLE);
        pub fn SHCreateItemFromParsingName(name: *const u16, bind: *mut core::ffi::c_void, interface: *const [u32; 4], out: *mut *mut super::Com) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        pub fn OleInitialize(reserved: *mut core::ffi::c_void) -> i32;
        pub fn CoInitializeEx(reserved: *mut core::ffi::c_void, how: u32) -> i32;
        pub fn CoUninitialize();
        pub fn CoCreateInstance(of: *const [u32; 4], outer: *mut core::ffi::c_void, context: u32, interface: *const [u32; 4], out: *mut *mut super::Com) -> i32;
        pub fn CoTaskMemFree(memory: *mut core::ffi::c_void);
        pub fn OleUninitialize();
        pub fn RegisterDragDrop(hwnd: HANDLE, target: *mut core::ffi::c_void) -> i32;
        pub fn RevokeDragDrop(hwnd: HANDLE) -> i32;
        pub fn ReleaseStgMedium(medium: *mut STGMEDIUM);
    }
}
use sys::*;

const WM_DESTROY: u32 = 0x0002;
const WM_MOVE: u32 = 0x0003;
const WM_SIZE: u32 = 0x0005;
/// A system setting changed; the system colours changed (including high contrast); the window
/// frame colour changed (it follows the accent).
const WM_SETTINGCHANGE: u32 = 0x001A;
const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION: u32 = 0x0016;
const WM_DISPLAYCHANGE: u32 = 0x007E;
const WM_GETDPISCALEDSIZE: u32 = 0x02E4;
const WM_SYSCOLORCHANGE: u32 = 0x0015;
const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETCURSOR: u32 = 0x0020;
const WM_GETMINMAXINFO: u32 = 0x0024;
/// `WM_NCCALCSIZE` and `WM_NCHITTEST`: asked of every window, answered by a frameless one.
const WM_NCCALCSIZE: u32 = 0x0083;
const WM_NCHITTEST: u32 = 0x0084;
/// Asked by accessibility clients for the window's provider.
const WM_GETOBJECT: u32 = 0x003D;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
/// Key up; system key up (with Alt, or Alt itself); a character typed with Alt; and a system
/// menu command, of which `SC_KEYMENU` is what Alt alone, F10 and Alt+letter ask for.
const WM_KEYUP: u32 = 0x0101;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_SYSCHAR: u32 = 0x0106;
const WM_SYSCOMMAND: u32 = 0x0112;
const SC_KEYMENU: usize = 0xF100;
/// An input method starts composing; and the composition changed.
const WM_IME_STARTCOMPOSITION: u32 = 0x010D;
const WM_IME_COMPOSITION: u32 = 0x010F;
const WM_TIMER: u32 = 0x0113;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_MOUSEWHEEL: u32 = 0x020A;
/// The wheel turned with Ctrl held.
const MK_CONTROL: usize = 0x0008;
const WM_MOUSEHWHEEL: u32 = 0x020E;
const WM_DROPFILES: u32 = 0x0233;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_DPICHANGED: u32 = 0x02E0;
/// "Run the interface again": what a [`Waker`] posts from another thread.
const WM_WAKE: u32 = 0x8001;
/// The system file dialog was answered. (The message after this one carries screen reader
/// requests.)
const WM_ANSWERED: u32 = 0x8003;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
/// A frameless, non-resizable window's style: `WS_POPUP` (0x8000_0000), minimizable, with a
/// system menu.
const WS_PLAIN: u32 = 0x800A_0000;

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// The strip the windows are drawn through, and the DC Windows draws letters into it with.
/// Shared by the windows (each fills and shows it in turn) and the text.
struct Strip {
    dc: HANDLE,
    bitmap: Cell<HANDLE>,
    pixels: Cell<*mut u32>,
    width: Cell<i32>,
    rows: Cell<i32>,
}

impl Strip {
    /// Makes the strip `width` pixels wide and as tall as keeps it near a quarter of a
    /// megabyte.
    fn fit(&self, width: i32) {
        if width == self.width.get() || width <= 0 {
            return;
        }
        let rows = (65_536 / width).clamp(16, 256);
        let info = BITMAPINFO { size: 40, width, height: -rows, planes: 1, bits: 32, compression: 0, image_size: 0, x_per_meter: 0, y_per_meter: 0, colors_used: 0, colors_important: 0, colors: [0] };
        let mut pixels = std::ptr::null_mut();
        // SAFETY: `info` describes a top-down 32-bit DIB; Windows returns its memory.
        let bitmap = unsafe { CreateDIBSection(self.dc, &info, 0, &mut pixels, 0, 0) };
        if bitmap == 0 || pixels.is_null() {
            return;
        }
        // SAFETY: the new bitmap replaces the old one in the DC, which is then freed.
        unsafe {
            SelectObject(self.dc, bitmap);
            if self.bitmap.get() != 0 {
                DeleteObject(self.bitmap.get());
            }
        }
        self.bitmap.set(bitmap);
        self.pixels.set(pixels);
        self.width.set(width);
        self.rows.set(rows);
    }
}

impl Drop for Strip {
    /// Frees the DC and the bitmap.
    fn drop(&mut self) {
        // SAFETY: both were made for this strip, which nothing holds any more. The DC goes
        // first, so the bitmap is not selected when deleted.
        unsafe {
            DeleteDC(self.dc);
            DeleteObject(self.bitmap.get());
        }
    }
}

/// A font of this name `px` pixels high, if Windows has it (for a missing face it silently
/// substitutes another). Left selected in `dc`.
fn font_named(dc: HANDLE, name: &str, px: i32, weight: i32, italic: bool) -> Option<HANDLE> {
    let mut got = [0u16; 32];
    // SAFETY: plain calls; the name ends in a zero, and Windows writes the chosen face name
    // into the room given. (5: `CLEARTYPE_QUALITY`, as the system has it.) A font that is not
    // the one asked for is deleted once the DC holds another (13, `SYSTEM_FONT`).
    unsafe {
        let handle = CreateFontW(-px, 0, 0, 0, weight, italic as u32, 0, 0, 1, 0, 0, 5, 0, wide(name).as_ptr());
        SelectObject(dc, handle);
        GetTextFaceW(dc, got.len() as i32, got.as_mut_ptr());
        let chosen = String::from_utf16_lossy(got.split(|unit| *unit == 0).next().unwrap_or_default());
        if chosen.eq_ignore_ascii_case(name) {
            return Some(handle);
        }
        SelectObject(dc, GetStockObject(13));
        DeleteObject(handle);
    }
    None
}

/// How many GDI fonts are kept at once (see `Fonts::font`).
const MADE: usize = 256;
/// How many glyph pictures [`SystemText`] keeps at once.
const GLYPHS: usize = 8192;

/// GDI fonts by face and pixel size, and text widths in them.
struct Fonts {
    dc: HANDLE,
    made: HashMap<(Face, i32), (HANDLE, Metrics)>,
    utf16: Vec<u16>,
    each: Vec<i32>,
    /// A line's pieces by the font each uses (see `system_fonts::pieces`).
    pieces: Vec<(std::ops::Range<usize>, Face)>,
}

impl Fonts {
    fn new(dc: HANDLE) -> Self {
        Fonts { dc, made: HashMap::new(), utf16: Vec::new(), each: Vec::new(), pieces: Vec::new() }
    }

    /// The system font for this face at this pixel size, made on first use. (Used at once,
    /// before another is asked for: past `MADE` fonts all are freed and remade on demand, since
    /// Windows allows some ten thousand GDI objects per process, and a pan-and-zoom view zoomed
    /// through every size would else keep one per size.)
    fn font(&mut self, font: Font, scale: f32) -> (HANDLE, Metrics) {
        let (px, dc) = ((font.size * scale).round() as i32, self.dc);
        if self.made.len() >= MADE && !self.made.contains_key(&(font.face, px)) {
            self.free();
        }
        *self.made.entry((font.face, px)).or_insert_with(|| {
            // The first of the face's names installed (see `system_fonts`).
            let asked = crate::system_fonts::asked(font.face);
            let px = ((px as f32 * asked.scale).round() as i32).max(1);
            // SAFETY: plain calls. With none of the names installed, the font is
            // `DEFAULT_GUI_FONT` (17), always there.
            unsafe {
                let handle = asked.names.iter().find_map(|name| font_named(dc, name, px, asked.weight, asked.italic)).unwrap_or_else(|| GetStockObject(17));
                // Cap height and x-height: how far "H" and "x" reach above the baseline, from
                // `GetGlyphOutlineW` with format 0 (metrics only), identity matrix. Nothing
                // where the font lacks the letter.
                let (mut m, mut letter, upright) = (TEXTMETRICW::default(), GLYPHMETRICS::default(), [0, 1, 0, 0, 0, 0, 0, 1]);
                SelectObject(dc, handle);
                GetTextMetricsW(dc, &mut m);
                let mut reach = |of: char| if GetGlyphOutlineW(dc, of as u32, 0, &mut letter, 0, std::ptr::null_mut(), &upright) == u32::MAX { 0.0 } else { letter.origin.y as f32 / scale };
                (handle, Metrics { ascent: m.ascent as f32 / scale, descent: m.descent as f32 / scale, cap_height: reach('H'), x_height: reach('x') })
            }
        })
    }

    /// How wide `text` is on one line, and with `stops`, where each character ends: letters the
    /// face's font lacks measured in its fallbacks (see `system_fonts`).
    fn measure(&mut self, text: &str, font: Font, scale: f32, mut stops: Option<&mut Vec<f32>>) -> f32 {
        if !crate::system_fonts::pieces(text, font.face, &mut self.pieces) {
            return self.measure_one(text, font, scale, stops);
        }
        let (pieces, mut wide) = (std::mem::take(&mut self.pieces), 0.0);
        for (bytes, face) in &pieces {
            let from = stops.as_ref().map_or(0, |stops| stops.len());
            let piece = self.measure_one(&text[bytes.clone()], Font { face: *face, ..font }, scale, stops.as_deref_mut());
            stops.iter_mut().for_each(|stops| stops[from..].iter_mut().for_each(|stop| *stop += wide));
            wide += piece;
        }
        self.pieces = pieces;
        wide
    }

    /// How wide `text` is in the face's own font, and where each character ends.
    fn measure_one(&mut self, text: &str, font: Font, scale: f32, stops: Option<&mut Vec<f32>>) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let (handle, _) = self.font(font, scale);
        self.utf16.clear();
        self.utf16.extend(text.encode_utf16());
        self.each.resize(self.utf16.len(), 0);
        let mut size = POINT::default();
        // SAFETY: both buffers are as long as the length passed.
        unsafe {
            SelectObject(self.dc, handle);
            GetTextExtentExPointW(self.dc, self.utf16.as_ptr(), self.utf16.len() as i32, 0, std::ptr::null_mut(), self.each.as_mut_ptr(), &mut size);
        }
        // Windows counts UTF-16 units; a character outside the BMP is two.
        let ends = text.chars().scan(0, |unit, c| Some(std::mem::replace(unit, *unit + c.len_utf16()) + c.len_utf16() - 1));
        stops.into_iter().for_each(|stops| stops.extend(ends.clone().map(|end| self.each[end] as f32 / scale)));
        size.x as f32 / scale
    }

    /// Frees the fonts made. None is deleted while selected: the DC selects `SYSTEM_FONT` (13)
    /// first.
    fn free(&mut self) {
        // SAFETY: the DC is alive, which the owner of these fonts ensures; the fonts were made
        // here and nothing else uses them.
        unsafe {
            SelectObject(self.dc, GetStockObject(13));
            self.made.drain().for_each(|(_, (font, _))| _ = DeleteObject(font));
        }
    }
}

/// Text in a window: drawn by GDI straight into the windows' strip, with the person's
/// smoothing. One writes in every window of the program, whichever is being drawn through the
/// strip.
struct GdiText {
    strip: Rc<Strip>,
    fonts: Fonts,
}

impl Drop for GdiText {
    /// Frees the windows' fonts. (The strip whose DC made them goes when its last holder does.)
    fn drop(&mut self) {
        self.fonts.free();
    }
}

impl TextSystem for GdiText {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        self.fonts.font(font, scale).1
    }

    fn measure(&mut self, text: &str, font: Font, scale: f32, stops: Option<&mut Vec<f32>>) -> f32 {
        self.fonts.measure(text, font, scale, stops)
    }

    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        if !crate::system_fonts::pieces(text, font.face, &mut self.fonts.pieces) {
            return self.draw_one(canvas, text, x, y, font, color);
        }
        // Each piece in its own font after the one before, on the baseline of the face's own
        // font.
        let (pieces, scale, mut pen) = (std::mem::take(&mut self.fonts.pieces), canvas.scale(), x);
        let ascent = self.fonts.font(font, scale).1.ascent;
        for (bytes, face) in &pieces {
            let piece = Font { face: *face, ..font };
            let lower = ascent - self.fonts.font(piece, scale).1.ascent;
            self.draw_one(canvas, &text[bytes.clone()], pen, y + lower, piece, color);
            pen += self.fonts.measure_one(&text[bytes.clone()], piece, scale, None);
        }
        self.fonts.pieces = pieces;
    }
}

impl GdiText {
    /// Draws a line of text in one font with its top left at (`x`, `y`).
    fn draw_one(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        let scale = canvas.scale();
        let (handle, _) = self.fonts.font(font, scale);
        self.fonts.utf16.clear();
        self.fonts.utf16.extend(text.encode_utf16());
        let (dc, units) = (self.strip.dc, &self.fonts.utf16);
        let (origin, clip) = (canvas.origin(), canvas.clip_pixels());
        let ink = color.0;
        // SAFETY: the canvas is this strip, so Windows writes into the same pixels; the DC is
        // restored after.
        unsafe {
            let saved = SaveDC(dc);
            SetViewportOrgEx(dc, -origin.0, -origin.1, std::ptr::null_mut());
            IntersectClipRect(dc, clip[0], clip[1], clip[2], clip[3]);
            SelectObject(dc, handle);
            SetBkMode(dc, 1);
            SetTextColor(dc, (ink >> 16 & 0xff) | (ink & 0xff00) | (ink & 0xff) << 16);
            TextOutW(dc, (x * scale).round() as i32, (y * scale).round() as i32, units.as_ptr(), units.len() as i32);
            RestoreDC(dc, saved);
            GdiFlush();
        }
    }
}

/// A glyph rasterized once at one size: its coverage.
struct Glyph {
    width: usize,
    left: i32,
    top: i32,
    advance: i32,
    cover: Vec<u8>,
}

/// Text for a [`Surface`](crate::Surface): Windows' fonts, each glyph rasterized once by
/// Windows in greyscale and then drawn by the library. It works on any pixel buffer and keeps a
/// clear background clear around the letters: what an interface shown by another renderer
/// needs.
pub struct SystemText {
    fonts: Fonts,
    glyphs: HashMap<(Face, i32, char), Glyph>,
}

impl SystemText {
    /// The system's text: Segoe UI, and the first monospaced font installed. Nothing where
    /// Windows gives no DC to measure and draw with (when the program holds all the GDI objects
    /// Windows allows).
    pub fn new() -> Option<Self> {
        // SAFETY: a memory DC of the program's own, for measuring and drawing letters.
        let dc = unsafe { CreateCompatibleDC(0) };
        (dc != 0).then(|| SystemText { fonts: Fonts::new(dc), glyphs: HashMap::new() })
    }

    fn glyph(&mut self, font: Font, scale: f32, letter: char) -> &Glyph {
        let ((handle, _), px, dc) = (self.fonts.font(font, scale), (font.size * scale).round() as i32, self.fonts.dc);
        // Past `GLYPHS` pictures all are dropped and remade on demand.
        if self.glyphs.len() >= GLYPHS && !self.glyphs.contains_key(&(font.face, px, letter)) {
            self.glyphs.clear();
        }
        self.glyphs.entry((font.face, px, letter)).or_insert_with(|| {
            // Upright, unscaled, 65 grey levels (`GGO_GRAY8_BITMAP`, 6), rows padded to four
            // bytes.
            let (mut m, upright) = (GLYPHMETRICS::default(), [0, 1, 0, 0, 0, 0, 0, 1]);
            // SAFETY: Windows is first asked the glyph picture's size, then draws into a buffer
            // that size.
            let shades = unsafe {
                SelectObject(dc, handle);
                let size = GetGlyphOutlineW(dc, letter as u32, 6, &mut m, 0, std::ptr::null_mut(), &upright);
                let mut shades = vec![0u8; if size == u32::MAX { 0 } else { size as usize }];
                GetGlyphOutlineW(dc, letter as u32, 6, &mut m, shades.len() as u32, shades.as_mut_ptr(), &upright);
                shades
            };
            let (width, row) = (m.wide as usize, (m.wide as usize + 3) & !3);
            let cover = shades.chunks(row.max(1)).take(m.high as usize).flat_map(|row| &row[..width]).map(|shade| (*shade as u32 * 255 / 64) as u8).collect();
            // A glyph with no picture still advances the pen.
            Glyph { width, left: m.origin.x, top: m.origin.y, advance: if shades.is_empty() && m.advance[0] == 0 { px / 3 } else { m.advance[0] as i32 }, cover }
        })
    }
}

impl std::fmt::Debug for SystemText {
    /// How many GDI fonts and glyph pictures it holds.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemText").field("fonts", &self.fonts.made.len()).field("letters", &self.glyphs.len()).finish()
    }
}

impl Drop for SystemText {
    /// Frees what the text was measured and drawn with.
    fn drop(&mut self) {
        self.fonts.free();
        // SAFETY: the DC was made for this text alone.
        unsafe { DeleteDC(self.fonts.dc) };
    }
}

impl TextSystem for SystemText {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        self.fonts.font(font, scale).1
    }

    fn measure(&mut self, text: &str, font: Font, scale: f32, stops: Option<&mut Vec<f32>>) -> f32 {
        self.fonts.measure(text, font, scale, stops)
    }

    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        let scale = canvas.scale();
        let ascent = (self.fonts.font(font, scale).1.ascent * scale).round() as i32;
        let (mut pen, line) = ((x * scale).round() as i32, (y * scale).round() as i32 + ascent);
        for letter in text.chars() {
            // A letter the face's font lacks comes from its fallback.
            let glyph = self.glyph(Font { face: crate::system_fonts::face_for(font.face, letter), ..font }, scale, letter);
            canvas.cover(pen + glyph.left, line - glyph.top, glyph.width, &glyph.cover, color);
            pen += glyph.advance;
        }
    }
}

/// Turns the messages Windows sends a window into interface events. A window this library runs
/// does it itself; for one it does not run (a game's, another program's, from inside a DLL),
/// keep one of these per window and hand it every message. (Where a host shows the interface's
/// second windows in its own windows, each has its own, feeding that window's interface.)
#[derive(Debug, Default)]
pub struct Messages {
    /// The first half of a surrogate pair.
    high: u16,
    /// The window the messages are for, if known.
    window: isize,
}

impl Messages {
    /// For the messages of this window (its `HWND`). Knowing the window, a wheel message also
    /// gives the pointer's place: Windows sends screen coordinates, which only the window can
    /// convert.
    pub fn of(window: isize) -> Self {
        Messages { high: 0, window }
    }

    /// What a message means for the interface: `each` gets every event (a button press is a
    /// move there, then the press). `scale` is the window's pixels per unit.
    ///
    /// Dropped files come as `WM_DROPFILES` once the window called `DragAcceptFiles`. The
    /// message is only read here and stays the window's: its owner ends the drop with
    /// `DragFinish`, as for any other.
    ///
    /// # Safety
    /// `w` and `l` are what Windows sent with `msg`. (For `WM_DROPFILES`, `w` is the `HDROP`,
    /// read here: it must be live, or 0.)
    pub unsafe fn events(&mut self, msg: u32, w: usize, l: isize, scale: f32, mut each: impl FnMut(Event)) {
        // Pointer messages of fingers and pens are read in `touch_windows`; the mouse messages
        // Windows also synthesizes from them, where left to it, are skipped.
        if cfg!(feature = "touch") && (WM_POINTERUPDATE..=WM_POINTERCAPTURECHANGED).contains(&msg) {
            return touch_windows::events(msg, w, scale, &mut each);
        }
        if cfg!(feature = "touch") && (WM_MOUSEMOVE..=WM_MBUTTONUP).contains(&msg) && touch_windows::made_up() {
            return;
        }
        let at = Event::Move(pt((l & 0xffff) as i16 as f32 / scale, (l >> 16 & 0xffff) as i16 as f32 / scale));
        let button = match msg {
            WM_LBUTTONDOWN | WM_LBUTTONUP => Button::Left,
            WM_RBUTTONDOWN | WM_RBUTTONUP => Button::Right,
            _ => Button::Middle,
        };
        // One wheel notch is 120 and scrolls three lines.
        let turn = (w >> 16) as i16 as f32 / 120.0 * 48.0;
        let events = match msg {
            WM_MOUSEMOVE => [Some(at), None],
            WM_MOUSELEAVE => [Some(Event::Leave), None],
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => [Some(at), Some(Event::Press(button, mods()))],
            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => [Some(at), Some(Event::Release(button))],
            WM_MOUSEHWHEEL => [self.wheel_at(l, scale), Some(Event::Wheel(turn, 0.0))],
            // With Ctrl it zooms (a touchpad pinch arrives so); with Shift it scrolls sideways.
            WM_MOUSEWHEEL if w & MK_CONTROL != 0 => [self.wheel_at(l, scale), Some(Event::Zoom(crate::touch::wheel_zoom(turn / 48.0)))],
            WM_MOUSEWHEEL if mods().shift => [self.wheel_at(l, scale), Some(Event::Wheel(-turn, 0.0))],
            WM_MOUSEWHEEL => [self.wheel_at(l, scale), Some(Event::Wheel(0.0, -turn))],
            // Shift, Control and Alt are news when pressed, not on each repeat; and when
            // released.
            WM_KEYDOWN | WM_SYSKEYDOWN if (0x10..=0x12).contains(&w) => [(l >> 30 & 1 == 0).then(|| Event::ModifiersChanged(held_with(w, true))), None],
            WM_KEYUP | WM_SYSKEYUP if (0x10..=0x12).contains(&w) => [Some(Event::ModifiersChanged(held_with(w, false))), None],
            // A key pressed with Alt says so itself, in bit 29.
            WM_KEYDOWN | WM_SYSKEYDOWN => [key(w).map(|key| Event::Key(key, Mods { alt: mods().alt || l >> 29 & 1 == 1, ..mods() })), None],
            WM_KEYUP | WM_SYSKEYUP => [key(w).map(Event::KeyUp), None],
            WM_CHAR => [self.typed(w as u16).map(Event::Text), None],
            WM_SETFOCUS => [Some(Event::Focused(true)), None],
            WM_KILLFOCUS => [Some(Event::Focused(false)), None],
            // SAFETY: as the caller promises. (Only where the app asks about files: see
            // `Drops`.)
            WM_DROPFILES => DROPS.get().map_or([None, None], |drops| unsafe { (drops.dropped)(w as HANDLE, scale) }),
            _ => [None, None],
        };
        events.into_iter().flatten().for_each(&mut each);
    }

    /// The pointer's place when the wheel turns, from the wheel message, so the wheel scrolls
    /// what is under the pointer even before the window saw it move. (Nothing when the window
    /// is unknown.)
    fn wheel_at(&self, l: isize, scale: f32) -> Option<Event> {
        pointer_at(self.window, POINT { x: (l & 0xffff) as i16 as i32, y: (l >> 16 & 0xffff) as i16 as i32 }, scale)
    }

    /// One UTF-16 unit of typed text, as a character. A surrogate pair arrives as two: the
    /// first is kept until the second comes.
    fn typed(&mut self, unit: u16) -> Option<String> {
        if (0xD800..0xDC00).contains(&unit) {
            self.high = unit;
            return None;
        }
        let units = [std::mem::take(&mut self.high), unit];
        let text = String::from_utf16_lossy(if units[0] != 0 { &units } else { &units[1..] });
        // Control with a letter is a shortcut, not text. (With Alt too it is AltGr, which types
        // characters on many keyboards.)
        (text.chars().all(|c| !c.is_control()) && (!mods().ctrl || mods().alt)).then_some(text)
    }
}

/// The pointer at a screen place, as `window` gets it: a move there, in units from its top
/// left. (Nothing when the window is unknown.)
fn pointer_at(window: HANDLE, mut screen: POINT, scale: f32) -> Option<Event> {
    // SAFETY: queries a window by handle; a gone one just fails the call.
    let known = window != 0 && unsafe { ScreenToClient(window, &mut screen) } != 0;
    known.then(|| Event::Move(pt(screen.x as f32 / scale, screen.y as f32 / scale)))
}

/// What a `WM_DROPFILES` says, by its `HDROP`: the pointer moved to the drop point, then the
/// files. (Dropped on the window's frame they are not on the interface: nothing.)
///
/// # Safety
/// `drop` is a live `HDROP`, or 0.
unsafe fn dropped(drop: HANDLE, scale: f32) -> [Option<Event>; 2] {
    // SAFETY: as the caller promises.
    let (Some(at), files) = (unsafe { dropped_files(drop) }) else { return [None, None] };
    [Some(Event::Move(pt(at.x as f32 / scale, at.y as f32 / scale))), Some(Event::FilesDropped(files))]
}

/// Reads an `HDROP`: the drop point in the window, and the files.
///
/// # Safety
/// `drop` is a live `HDROP`, or 0.
unsafe fn dropped_files(drop: HANDLE) -> (Option<POINT>, Vec<PathBuf>) {
    // SAFETY: as the caller promises. The block is locked while read, and read no further than
    // its size. (A null handle gives nothing to read.)
    unsafe {
        let start = GlobalLock(drop).cast::<u8>();
        let block = if start.is_null() { &[][..] } else { std::slice::from_raw_parts(start, GlobalSize(drop)) };
        let read = read_drop(block);
        GlobalUnlock(drop);
        read
    }
}

/// What a `DROPFILES` block says: five fields (offset of the names, drop point x and y in
/// pixels, whether on the frame, whether the names are UTF-16 or ANSI), then names each ending
/// in a zero, one more zero ending them. (A block too short for its fields gives nothing.)
fn read_drop(block: &[u8]) -> (Option<POINT>, Vec<PathBuf>) {
    let number = |at: usize| block.get(at..).and_then(|rest| rest.first_chunk()).map_or(0, |four| i32::from_le_bytes(*four));
    let at = (block.len() >= 20 && number(12) == 0).then_some(POINT { x: number(4), y: number(8) });
    let start = number(0) as usize;
    let names = block.get(start..).filter(|_| start >= 20).unwrap_or_default();
    if number(16) == 0 {
        return (at, names.split(|byte| *byte == 0).take_while(|name| !name.is_empty()).map(from_ansi).map(PathBuf::from).collect());
    }
    let units: Vec<u16> = names.as_chunks().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    (at, units.split(|unit| *unit == 0).take_while(|name| !name.is_empty()).map(OsString::from_wide).map(PathBuf::from).collect())
}

/// A name in the system ANSI code page, as text.
fn from_ansi(name: &[u8]) -> OsString {
    let mut units = vec![0u16; name.len()];
    // SAFETY: both buffers are as long as the lengths passed. (0: `CP_ACP`.)
    let len = unsafe { MultiByteToWideChar(0, 0, name.as_ptr(), name.len() as i32, units.as_mut_ptr(), units.len() as i32) };
    OsString::from_wide(&units[..len.max(0) as usize])
}

/// The text cursor in whole window pixels, as the system is told it.
fn cursor_pixels(cursor: Rect, scale: f32) -> RECT {
    let pixel = |unit: f32| (unit * scale).round() as i32;
    RECT { left: pixel(cursor.x), top: pixel(cursor.y), right: pixel(cursor.right()), bottom: pixel(cursor.bottom()) }
}

/// The window class of every window of this library.
const CLASS: &str = "taut";

/// An open window, first or second: what is kept for it alone.
struct Open {
    hwnd: HANDLE,
    /// The interface's id for a second window. (The first is the interface itself.)
    id: Id,
    scale: f32,
    /// Client width in pixels.
    wide: i32,
    /// The least size the person can resize it to, in units.
    min_size: (f32, f32),
    /// Whether `TrackMouseEvent` was asked to report the pointer leaving.
    tracking: bool,
    /// What turns Windows' messages into events.
    messages: Messages,
    /// Where the system was last told the text cursor is.
    ime_spot: RECT,
    /// Its place and size while not maximized.
    normal: WindowPlacement,
    /// The input methods are kept off it: what is typed now must not be seen.
    ime_off: bool,
    /// Whether it accepts dropped files yet.
    files: bool,
    /// Its title bar is dark: for an interface with a dark background.
    dark: bool,
    /// Client height in pixels.
    high: i32,
    /// Frameless and resized by its own edges. (Whether it has a frame at all Windows asks when
    /// only the window is at hand: that is kept on the window, `styles_of`.)
    edges: bool,
    /// Something of it is the window's, not the interface's, as of the last run: a resize edge
    /// or a title bar. (Only then is the interface asked what is under the pointer for
    /// `WM_NCHITTEST`.)
    asks_hit_test: bool,
    /// The picture Windows shows it from, if see-through.
    transparent: Option<SeeThrough>,
    /// What it restores to from fullscreen: the style bits removed for it, and its
    /// `WINDOWPLACEMENT` as Windows keeps it. Nothing while not fullscreen.
    before_fullscreen: Option<(isize, [i32; 11])>,
    /// A dialog opened after it is open: it takes neither keyboard nor pointer.
    blocked: bool,
    /// Its app hid it entirely to live in the tray ([`Ui::hide_window`]), until it comes
    /// forward.
    withdrawn: bool,
    /// What shows it, while a GPU path does; and whether it is hidden until its first paint.
    #[cfg(feature = "gpu")]
    shown: Option<crate::gpu_windows::Shown>,
    #[cfg(feature = "gpu")]
    cloaked: bool,
}

/// The full picture of a see-through (layered) window, which Windows shows it from: not drawn
/// piecewise but handed over whole, with per-pixel alpha. (So it takes four bytes per pixel,
/// where the other windows share one strip.)
struct SeeThrough {
    dc: HANDLE,
    bitmap: HANDLE,
    pixels: *mut u32,
    /// Width and height in pixels.
    size: [i32; 2],
}

impl SeeThrough {
    /// A picture with nothing in it yet.
    fn new() -> SeeThrough {
        // SAFETY: a memory DC of the program's own, to hold the picture.
        SeeThrough { dc: unsafe { CreateCompatibleDC(0) }, bitmap: 0, pixels: std::ptr::null_mut(), size: [0; 2] }
    }

    /// Makes the picture `size` pixels if it is not already. Says whether it was remade: then
    /// it is empty and the whole window must be drawn into it.
    fn fit(&mut self, size: [i32; 2]) -> bool {
        if size == self.size || size[0] <= 0 || size[1] <= 0 {
            return false;
        }
        let (info, mut pixels) = (BITMAPINFO { size: 40, width: size[0], height: -size[1], planes: 1, bits: 32, ..Default::default() }, std::ptr::null_mut());
        // SAFETY: `info` describes a top-down 32-bit DIB; Windows returns its memory. The new
        // bitmap replaces the old one in the DC, and the old one, if any, is freed.
        unsafe {
            let bitmap = CreateDIBSection(self.dc, &info, 0, &mut pixels, 0, 0);
            if bitmap == 0 || pixels.is_null() {
                return false;
            }
            SelectObject(self.dc, bitmap);
            DeleteObject(self.bitmap);
            (self.bitmap, self.pixels, self.size) = (bitmap, pixels, size);
        }
        true
    }

    /// Redraws the part of an interface in `area` into the picture, on a clear background
    /// (cleared first). (Nothing is drawn while there is no picture.)
    fn draw(&mut self, ui: &mut Ui, area: Rect, scale: f32) {
        if self.pixels.is_null() {
            return;
        }
        let [wide, high] = self.size.map(|side| side as usize);
        // SAFETY: the picture holds that many pixels, and nothing else uses them now.
        let block = unsafe { std::slice::from_raw_parts_mut(self.pixels, wide * high) };
        let mut canvas = Canvas::new(block, wide, high, (0, 0), scale);
        canvas.clip(area.snap_to_pixels(scale));
        canvas.erase();
        ui.paint(&mut canvas);
    }

    /// Has Windows show the window from the picture, `dirty` being what changed: blended over
    /// what is behind by per-pixel alpha (`AC_SRC_ALPHA` and `ULW_ALPHA`, 2). Says whether
    /// Windows did.
    fn show(&self, hwnd: HANDLE, dirty: RECT) -> bool {
        let (big, from, blend) = (POINT { x: self.size[0], y: self.size[1] }, POINT::default(), [0, 0, 255, 1]);
        let how = Layered { size: size_of::<Layered>() as u32, screen: 0, at: std::ptr::null(), big: &big, picture: self.dc, from: &from, key: 0, blend: &blend, flags: 2, dirty: &dirty };
        // SAFETY: the window is alive, and everything pointed at outlives the call.
        unsafe { UpdateLayeredWindowIndirect(hwnd, &how) != 0 }
    }
}

impl Drop for SeeThrough {
    /// Frees the DC and the picture.
    fn drop(&mut self) {
        // SAFETY: both were made for this picture, which nothing holds any more. The DC goes
        // first, so the bitmap is not selected when deleted.
        unsafe {
            DeleteDC(self.dc);
            DeleteObject(self.bitmap);
        }
    }
}

/// Shows a window for the first time minimized, without focus; maximized when restored, if
/// `filling`.
///
/// # Safety
/// `hwnd` is a live window.
unsafe fn show_minimized(hwnd: HANDLE, filling: bool) {
    // `WINDOWPLACEMENT`: its length, flags (2: `WPF_RESTORETOMAXIMIZED`), show command (7:
    // `SW_SHOWMINNOACTIVE`), and places left as they are.
    let mut place = [0i32; 11];
    place[0] = 44;
    // SAFETY: as the caller promises; Windows fills in and reads the numbers.
    unsafe {
        GetWindowPlacement(hwnd, &mut place);
        (place[1], place[2]) = (if filling { 2 } else { 0 }, 7);
        SetWindowPlacement(hwnd, &place);
    }
}

/// The outer edges of a window with a `w` by `h` unit client on a `dpi` screen, with the window
/// styles `frame` (`styles`), relative to the client's top left, in pixels. (A frameless window
/// has no styles here: its edges are its client's.)
fn framed(w: f32, h: f32, dpi: u32, frame: Option<[u32; 2]>) -> RECT {
    let scale = dpi as f32 / 96.0;
    let mut area = RECT { left: 0, top: 0, right: (w * scale) as i32, bottom: (h * scale) as i32 };
    if let Some([style, ex]) = frame {
        // SAFETY: Windows adds such a window's frame around the rectangle given.
        unsafe { AdjustWindowRectExForDpi(&mut area, style, 0, ex, dpi) };
    }
    area
}

/// A window kind's styles: its frame (`WS_OVERLAPPEDWINDOW`, or a frameless resizable
/// `WS_PLAIN`) and extended styles. Tool windows and dialogs have no minimize or maximize
/// button (0x0003_0000); a tool window is `WS_EX_TOOLWINDOW` (0x80), a dialog
/// `WS_EX_DLGMODALFRAME` (1), an always-on-top one `WS_EX_TOPMOST` (8). A see-through window is
/// layered (0x0008_0000).
fn styles(kind: WindowKind, edges: bool, transparent: bool) -> [u32; 2] {
    let style = if transparent && !edges { WS_PLAIN } else { WS_OVERLAPPEDWINDOW };
    let (boxes, more) = match kind {
        WindowKind::Plain => (0, 0),
        WindowKind::Tool => (0x0003_0000, 0x80),
        WindowKind::Dialog => (0x0003_0000, 1),
        WindowKind::OnTop => (0, 8),
    };
    [style & !boxes, more | if transparent { 0x0008_0000 } else { 0 }]
}

/// The styles of a framed window as created; none for a frameless one (which records that in
/// its extra bytes).
fn styles_of(hwnd: HANDLE) -> Option<[u32; 2]> {
    // SAFETY: plain queries about a window of this library's class, which has those bytes.
    unsafe { (GetWindowLongPtrW(hwnd, 0) == 0).then(|| [GetWindowLongPtrW(hwnd, -16) as u32, GetWindowLongPtrW(hwnd, -20) as u32]) }
}

/// The person's screens as Windows has them now, main one first.
fn screens() -> Vec<Screen> {
    let mut found = Vec::new();
    // SAFETY: Windows passes each monitor to `each_screen` during the call, with the list.
    unsafe { EnumDisplayMonitors(0, std::ptr::null(), each_screen, (&raw mut found) as isize) };
    found
}

/// Adds a monitor to the list `into` points at, for `screens`: its rectangle, work area, and
/// scale (its DPI over 96). The primary one (flag 1) goes first.
unsafe extern "system" fn each_screen(screen: HANDLE, _: HANDLE, _: *mut RECT, into: isize) -> i32 {
    let (mut info, mut dpi) = (MONITORINFO { size: size_of::<MONITORINFO>() as u32, all: RECT::default(), work: RECT::default(), flags: 0 }, [96u32; 2]);
    let area = |part: RECT| rect(part.left as f32, part.top as f32, (part.right - part.left) as f32, (part.bottom - part.top) as f32);
    // SAFETY: `into` is the list `screens` passes, held no other way while Windows calls this;
    // Windows fills in the rest.
    unsafe {
        let found = &mut *(into as *mut Vec<Screen>);
        if GetMonitorInfoW(screen, &mut info) != 0 {
            GetDpiForMonitor(screen, 0, &mut dpi[0], &mut dpi[1]);
            found.insert(if info.flags & 1 != 0 { 0 } else { found.len() }, Screen { rect: area(info.all), work_area: area(info.work), scale: dpi[0] as f32 / 96.0 });
        }
    }
    1
}

/// The `WM_NCCALCSIZE` answer for a frameless window: all of it is client. `l` points at its
/// proposed screen rectangle, kept as the client. (But a maximized window is placed so its
/// would-be frame lies past the screen's edges: its client is then the work area. Style -16 bit
/// 0x0004_0000: `WS_THICKFRAME`.)
///
/// # Safety
/// `hwnd` is a live window and `l` what Windows sent with the message.
unsafe fn all_inside(hwnd: HANDLE, l: isize) -> isize {
    let (area, mut screen) = (l as *mut RECT, MONITORINFO { size: size_of::<MONITORINFO>() as u32, all: RECT::default(), work: RECT::default(), flags: 0 });
    // SAFETY: as the caller promises: the rectangle is there to read and write.
    unsafe {
        let beyond = IsZoomed(hwnd) != 0 && GetWindowLongPtrW(hwnd, -16) & 0x0004_0000 != 0;
        if beyond && GetMonitorInfoW(MonitorFromRect(area, 2), &mut screen) != 0 {
            *area = screen.work;
        }
    }
    0
}

/// An interface rectangle in whole window pixels: every pixel it touches.
fn pixels_of(part: Rect, scale: f32) -> RECT {
    RECT { left: (part.x * scale).floor() as i32, top: (part.y * scale).floor() as i32, right: (part.right() * scale).ceil() as i32, bottom: (part.bottom() * scale).ceil() as i32 }
}

/// Where a window with a `size` client and frame `looks` (`styles`; none if frameless) goes as
/// `asked` on `screens` (see `screens::window_origin`): a screen DPI, and its frame corner
/// there. Nothing where the system places it.
fn place_on(screens: &[Screen], asked: OpenAt, size: Size, looks: Option<[u32; 2]>) -> Option<(u32, POINT)> {
    let dpi = |scale: f32| (scale * 96.0).round() as u32;
    let big = |scale: f32| {
        let frame = framed(size.w, size.h, dpi(scale), looks);
        [(frame.right - frame.left) as f32, (frame.bottom - frame.top) as f32]
    };
    let (nth, at) = window_origin(screens, asked, big)?;
    Some((dpi(screens[nth].scale), POINT { x: at.x as i32, y: at.y as i32 }))
}

/// A window's top left corner on screen.
fn corner_of(hwnd: HANDLE) -> POINT {
    let mut area = RECT::default();
    // SAFETY: queries a window by handle; a gone one just fails the call.
    unsafe { GetWindowRect(hwnd, &mut area) };
    POINT { x: area.left, y: area.top }
}

impl Stale {
    /// The rectangles of a Windows region. (None when they exceed the room here.)
    fn of(region: HANDLE) -> Stale {
        let mut stale = Stale::default();
        // SAFETY: Windows is told `stale`'s size and writes nothing if it needs more (then
        // returns 0).
        if unsafe { GetRegionData(region, size_of::<Stale>() as u32, &mut stale) } == 0 {
            stale.header[2] = 0;
        }
        stale
    }
}

impl Open {
    /// Creates a hidden window as `opening` says (least size, kind, frame), owned by `owner` (0
    /// for none), see-through if `transparent` (then frameless too). Its client is `size` units
    /// at the scale of the screen it lands on, at `place`: its frame corner at a desktop point,
    /// on a screen of that DPI; or where the system puts it. Nothing if Windows creates none.
    fn make(title: &str, size: Size, opening: WindowOpening, transparent: bool, place: Option<(u32, POINT)>, id: Id, owner: HANDLE) -> Option<Open> {
        let (edges, frameless, min_size) = (opening.frameless, opening.frameless || transparent, (opening.min_size.w, opening.min_size.h));
        // A window the person resizes uses a resizable frame style even if it shows none, so
        // the system resizes and snaps it.
        let [style, more] = styles(opening.kind, edges, transparent);
        let looks = (!frameless).then_some([style, more]);
        // Created where it goes and at its size there, so it is created at that screen's scale.
        // (A place or size of `i32::MIN` is left to the system.)
        let (at, big) = match place {
            Some((dpi, at)) => (at, framed(size.w, size.h, dpi, looks)),
            None => (POINT { x: i32::MIN, y: i32::MIN }, RECT { left: 0, top: 0, right: i32::MIN, bottom: i32::MIN }),
        };
        // SAFETY: the calls creating a window, in the order Windows wants. Every pointer
        // outlives its call.
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let hwnd = CreateWindowExW(more, wide(CLASS).as_ptr(), wide(title).as_ptr(), style, at.x, at.y, big.right - big.left, big.bottom - big.top, owner, 0, instance, std::ptr::null_mut());
            if hwnd == 0 {
                return None;
            }
            // That it is frameless is kept in the window's own extra bytes, which every window
            // of this class has: Windows asks when nothing else of it is at hand.
            SetWindowLongPtrW(hwnd, 0, frameless as isize);
            let (dpi, mut size) = (GetDpiForWindow(hwnd), size);
            let frame = framed(size.w, size.h, dpi, looks);
            // No larger than its screen's work area; and where the system chose the place,
            // wholly on that screen.
            let (mut wide, mut high, mut to, mut screen) =
                (frame.right - frame.left, frame.bottom - frame.top, corner_of(hwnd), MONITORINFO { size: size_of::<MONITORINFO>() as u32, all: RECT::default(), work: RECT::default(), flags: 0 });
            if GetMonitorInfoW(MonitorFromWindow(hwnd, 2), &mut screen) != 0 {
                let (room, scale) = ([screen.work.right - screen.work.left, screen.work.bottom - screen.work.top], dpi as f32 / 96.0);
                size = Size { w: size.w - (wide - room[0]).max(0) as f32 / scale, h: size.h - (high - room[1]).max(0) as f32 / scale };
                (wide, high) = (wide.min(room[0]), high.min(room[1]));
                to = POINT { x: to.x.min(screen.work.right - wide).max(screen.work.left), y: to.y.min(screen.work.bottom - high).max(screen.work.top) };
                // The system places a popup nowhere: it goes to the middle.
                let middle = POINT { x: (screen.work.left + screen.work.right - wide) / 2, y: (screen.work.top + screen.work.bottom - high) / 2 };
                to = if style == WS_PLAIN { middle } else { to };
            }
            // Resized, and moved only if its place was not asked for (`SWP_*` 0x34, or 0x36 to
            // stay put), with a fresh `WM_NCCALCSIZE`. A place on no screen at all that the
            // system gave was asked for by whatever started the program, to show it nowhere: it
            // stays there too.
            let nowhere = MonitorFromWindow(hwnd, 0) == 0;
            SetWindowPos(hwnd, 0, to.x, to.y, wide, high, if place.is_some() || nowhere { 0x0036 } else { 0x0034 });
            let (corner, messages) = (corner_of(hwnd), Messages::of(hwnd));
            let normal = WindowPlacement { position: (corner.x, corner.y), width: size.w, height: size.h, maximized: false };
            let transparent = transparent.then(SeeThrough::new);
            let mut made = Open {
                hwnd,
                id,
                scale: dpi as f32 / 96.0,
                wide: 0,
                min_size,
                tracking: false,
                messages,
                ime_spot: RECT::default(),
                normal,
                ime_off: false,
                files: false,
                dark: false,
                high: 0,
                edges,
                asks_hit_test: false,
                transparent,
                before_fullscreen: None,
                blocked: false,
                withdrawn: false,
                #[cfg(feature = "gpu")]
                shown: None,
                #[cfg(feature = "gpu")]
                cloaked: false,
            };
            made.frame(true); // themes are dark until an app says otherwise
            Some(made)
        }
    }

    /// Shows the window again if hidden (5: `SW_SHOW`), restores it if minimized (9:
    /// `SW_RESTORE`), and asks to bring it to the front. (Windows grants that to the foreground
    /// program and to one it allowed.)
    fn bring_to_front(&mut self) {
        // SAFETY: the window is alive.
        unsafe {
            if std::mem::take(&mut self.withdrawn) {
                ShowWindow(self.hwnd, 5);
            }
            if IsIconic(self.hwnd) != 0 {
                ShowWindow(self.hwnd, 9);
            }
            SetForegroundWindow(self.hwnd);
        }
    }

    /// Makes the window fullscreen without frame, keeping what it restores to: the style bits
    /// giving its frame, and its `WINDOWPLACEMENT` (eleven numbers, the first their length). It
    /// is moved, resized and sent `WM_NCCALCSIZE`, nothing else changing, not the z-order
    /// (0x0234).
    fn enter_fullscreen(&mut self) {
        let (hwnd, mut place, mut screen) = (self.hwnd, [0i32; 11], MONITORINFO { size: size_of::<MONITORINFO>() as u32, all: RECT::default(), work: RECT::default(), flags: 0 });
        place[0] = 44;
        // SAFETY: plain calls about a live window; Windows fills in the numbers. (A minimized
        // one is restored first, 9: a window showing nothing covers nothing.)
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, 9);
            }
            if GetWindowPlacement(hwnd, &mut place) == 0 || GetMonitorInfoW(MonitorFromWindow(hwnd, 2), &mut screen) == 0 {
                return;
            }
            let (style, all, frame) = (GetWindowLongPtrW(hwnd, -16), screen.all, WS_OVERLAPPEDWINDOW as isize);
            SetWindowLongPtrW(hwnd, -16, style & !frame);
            SetWindowPos(hwnd, 0, all.left, all.top, all.right - all.left, all.bottom - all.top, 0x0234);
            self.before_fullscreen = Some((style & frame, place));
        }
    }

    /// Restores the window from fullscreen: its frame back, where it was, maximized if it was.
    /// One minimized meanwhile stays so, and is all that once restored (2 and 7, as for a
    /// window opening minimized).
    fn leave_fullscreen(&mut self) {
        let Some((frame, mut place)) = self.before_fullscreen.take() else { return };
        // SAFETY: plain calls about a live window; the numbers are those Windows filled in. Its
        // frame is redrawn and nothing else changes (0x0237).
        unsafe {
            if IsIconic(self.hwnd) != 0 {
                (place[1], place[2]) = (if place[2] == 3 { 2 } else { 0 }, 7);
            }
            SetWindowLongPtrW(self.hwnd, -16, GetWindowLongPtrW(self.hwnd, -16) | frame);
            SetWindowPlacement(self.hwnd, &place);
            SetWindowPos(self.hwnd, 0, 0, 0, 0, 0, 0x0237);
        }
    }

    /// Does what the app asked of the window's state: fullscreen or back; maximize (3) or
    /// restore (9), not asked of a fullscreen window; minimize (6). Says whether anything was
    /// asked: Windows reported the result while nothing was listening.
    fn apply_state(&mut self, minimize: bool, fill: Option<bool>, whole: Option<bool>) -> bool {
        match whole {
            Some(true) if self.before_fullscreen.is_none() => self.enter_fullscreen(),
            Some(false) => self.leave_fullscreen(),
            _ => {}
        }
        // SAFETY: the window is alive.
        unsafe {
            match fill.filter(|_| self.before_fullscreen.is_none()) {
                Some(true) => _ = ShowWindow(self.hwnd, 3),
                Some(false) if IsZoomed(self.hwnd) != 0 => _ = ShowWindow(self.hwnd, 9),
                _ => {}
            }
            if minimize {
                ShowWindow(self.hwnd, 6);
            }
        }
        minimize || fill.is_some() || whole.is_some()
    }

    /// Handles what a run changed of the window. Each place is invalidated by itself: two small
    /// changes far apart are two small areas, not all between. A see-through window draws them
    /// into its picture at once and has Windows show it; a picture remade for a new size gets
    /// the whole window.
    fn changed(&mut self, ui: &mut Ui) {
        let (all, scale) = (rect(0.0, 0.0, ui.size().w, ui.size().h), self.scale);
        let Some(picture) = &mut self.transparent else {
            return ui.changes().for_each(|place| self.invalidate(place));
        };
        let afresh = picture.fit([self.wide, self.high]);
        let (around, places) = if afresh { (all, 1) } else { (ui.around, ui.places.len()) };
        for place in 0..places {
            let area = if afresh { all } else { ui.places[place] };
            picture.draw(ui, area, scale);
        }
        ui.changes();
        if !around.is_empty() {
            picture.show(self.hwnd, pixels_of(around.intersect(all), scale));
        }
    }

    /// Gives the window the dark or light title bar (`DWMWA_USE_IMMERSIVE_DARK_MODE`, 20) where
    /// it has the other: the one matching its content's background.
    fn frame(&mut self, dark: bool) {
        if std::mem::replace(&mut self.dark, dark) != dark {
            let dark = dark as i32;
            // SAFETY: the window is alive and the number outlives the call. The frame is
            // redrawn without anything else changing (0x37).
            unsafe {
                DwmSetWindowAttribute(self.hwnd, 20, (&raw const dark).cast(), 4);
                SetWindowPos(self.hwnd, 0, 0, 0, 0, 0, 0x0037);
            }
        }
    }

    /// Destroys a second window. (Windows destroys the first when the person closes it.)
    fn destroy(self) {
        #[cfg(feature = "gpu")]
        drop(self.shown);
        // Accessibility clients are told it is gone.
        uia::gone(self.hwnd);
        // SAFETY: Windows releases the window's drop target, if any, which frees it. The window
        // no longer finds the state and is destroyed: Windows sends it messages during that
        // call, while nothing listens, and everything to do about it is done here.
        unsafe {
            if self.files {
                RevokeDragDrop(self.hwnd);
            }
            SetWindowLongPtrW(self.hwnd, -21, 0);
            DestroyWindow(self.hwnd);
        }
    }

    /// The rectangles of the window's update region, asked before `BeginPaint`, which validates
    /// them. (None when more than eight: then their bounding rectangle is drawn, which the
    /// paint gives.)
    fn stale(&self) -> Stale {
        // SAFETY: a region is created, filled with the update region, read, and deleted.
        unsafe {
            let region = CreateRectRgn(0, 0, 0, 0);
            GetUpdateRgn(self.hwnd, region, 0);
            let stale = Stale::of(region);
            DeleteObject(region);
            stale
        }
    }

    /// Asks Windows to repaint this part of the window.
    fn invalidate(&self, part: Rect) {
        if part.is_empty() {
            return;
        }
        let area = pixels_of(part, self.scale);
        // SAFETY: the window is alive; the rectangle is read during the call.
        unsafe { InvalidateRect(self.hwnd, &area, 0) };
    }

    /// Makes the window accept dropped files, from the first time the app asks about files in
    /// it. (Most apps never do and are spared the cost: Windows starts three threads and loads
    /// a few megabytes for a drop-target window.) `ole` says whether OLE is initialized on this
    /// thread.
    fn take_files(&mut self, ole: bool) {
        // SAFETY: Windows takes a drop target for the window and holds it until the window is
        // destroyed. Without OLE (only with it does Windows call the drop target on this
        // thread), the window gets `WS_EX_ACCEPTFILES` (16) instead: files then arrive as a
        // message on drop, with nothing reported during the drag.
        unsafe {
            let target = DropTarget::new(self.hwnd);
            if !ole || RegisterDragDrop(self.hwnd, target.cast()) < 0 {
                SetWindowLongPtrW(self.hwnd, -20, GetWindowLongPtrW(self.hwnd, -20) | 16);
            }
            DropTarget::release(target);
        }
        self.files = true;
    }

    /// Keeps the input methods off the window while what is typed must not be seen, and lets
    /// them back after. (An input method shows typed text in its own window and offers it again
    /// later: not for a password.)
    fn keep_ime_off(&mut self, hidden: bool) {
        if std::mem::replace(&mut self.ime_off, hidden) == hidden {
            return;
        }
        // SAFETY: the window is alive. It is detached from its input context (none, 0) or given
        // back its default one (`IACE_DEFAULT`, 0x10).
        unsafe { ImmAssociateContextEx(self.hwnd, 0, if hidden { 0 } else { 0x10 }) };
    }

    /// Tells the system where the text cursor is, if it moved since last told. The CJK input
    /// method's composition window then opens at the cursor, and its candidate list keeps clear
    /// of the cursor's line, instead of both sitting in the window's corner. (Telling costs
    /// more than typing a letter, so it is done when composing starts or continues, not on
    /// every cursor move.)
    fn follow_cursor(&mut self, cursor: Option<Rect>) {
        let Some(cursor) = cursor else { return };
        let spot = cursor_pixels(cursor, self.scale);
        if std::mem::replace(&mut self.ime_spot, spot) == spot {
            return;
        }
        // Composition starts at the cursor's top left (`CFS_POINT`, 2, which the system may
        // shift to stay on screen); the candidate list avoids an area (`CFS_EXCLUDE`, 0x80).
        let text = COMPOSITIONFORM { style: 2, at: POINT { x: spot.left, y: spot.top }, area: RECT::default() };
        let list = CANDIDATEFORM { index: 0, style: 0x80, at: text.at, area: spot };
        // SAFETY: the window's input context is taken, told the two places (read during the
        // calls), and released. (A window with no context has nothing to tell, and the calls
        // fail.)
        unsafe {
            let context = ImmGetContext(self.hwnd);
            ImmSetCompositionWindow(context, &text);
            ImmSetCandidateWindow(context, &list);
            ImmReleaseContext(self.hwnd, context);
        }
    }

    /// What Windows must be asked around mouse messages: to report the pointer leaving
    /// (`TrackMouseEvent`), and to keep sending moves while a button is held, even outside the
    /// window (capture). (A button-up says in `w` which others are still down: 0x13 is all
    /// three.)
    fn follow_mouse(&mut self, msg: u32, w: usize) {
        let mut ask = TRACKMOUSEEVENT { size: size_of::<TRACKMOUSEEVENT>() as u32, flags: 2, hwnd: self.hwnd, hover_time: 0 };
        // SAFETY: plain calls about this window, which is alive.
        match msg {
            WM_MOUSEMOVE if !std::mem::replace(&mut self.tracking, true) => _ = unsafe { TrackMouseEvent(&mut ask) },
            WM_MOUSELEAVE => self.tracking = false,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => _ = unsafe { SetCapture(self.hwnd) },
            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP if w & 0x13 == 0 => _ = unsafe { ReleaseCapture() },
            _ => {}
        }
    }
}

/// Hands `work` a window's interface: the first one's as is (`at` 0), a second one's while the
/// first lends it. (All work goes through here, so neither it nor the lending is compiled
/// twice, for first and other windows.)
fn lend(ui: &mut Ui, at: usize, id: Id, work: &mut dyn FnMut(&mut Ui)) {
    match at {
        0 => work(ui),
        _ => _ = ui.in_window(id, work),
    }
}

/// What a program's windows share, and the windows.
struct State {
    /// The first window's interface, which holds the second windows' interfaces.
    ui: Ui,
    app: Box<dyn FnMut(&mut Ui)>,
    strip: Rc<Strip>,
    /// The open windows: the first, then the second ones in creation order.
    windows: Vec<Open>,
    /// The clock the interface gets its time from.
    start: Instant,
    /// Whether a timer is set to run the interface again.
    timer: bool,
    /// Whether OLE (for drag and drop) is initialized for the windows; nothing while none has
    /// asked for drops.
    ole: Option<bool>,
    /// The device the GPU shows the windows with, none while the CPU does; and the one being
    /// made, until it comes.
    #[cfg(feature = "gpu")]
    device: Option<crate::gpu_windows::Device>,
    #[cfg(feature = "gpu")]
    coming: Option<std::thread::JoinHandle<Option<crate::gpu_windows::Device>>>,
}

impl State {
    /// Does `work` with window `at`'s interface and its kept state. The first window's
    /// interface is at hand; a second one's is lent by it meanwhile. (Nothing for a second
    /// window the interface no longer has.)
    fn in_window<T>(&mut self, at: usize, work: impl FnOnce(&mut Ui, &mut Open) -> T) -> Option<T> {
        let (ui, window) = (&mut self.ui, &mut self.windows[at]);
        let (id, mut work, mut done) = (window.id, Some(work), None);
        lend(ui, at, id, &mut |ui| done = work.take().map(|work| work(ui, window)));
        done
    }

    /// Runs the app's description of its windows (the first window's to run, whichever window
    /// something happened in), and handles what follows for each.
    fn run(&mut self) {
        self.ui.run(&mut self.app);
        self.sync_windows();
        let mut came = false;
        for at in 0..self.windows.len() {
            came |= self.apply_run(at);
        }
        came |= self.apply_desktop_requests();
        // A window that came forward is no longer minimized, has a size again, and has focus;
        // one minimized, maximized or made fullscreen has another size, or none: Windows said
        // so while this was busy and was not heard. So it is asked now, and what is news is run
        // for after this run.
        if came {
            let news = (0..self.windows.len()).fold(false, |news, at| self.back(at) | (self.fit(at) == Some(true)) | news);
            if self.hear_keyboard() || news {
                // SAFETY: posts to the first window, which is alive.
                unsafe { PostMessageW(self.windows[0].hwnd, WM_WAKE, 0, 0) };
            }
        }
        // No accessibility client is left: the tree kept for them is dropped.
        if self.ui.listening && uia::nobody_left() {
            self.ui.set_screen_reader(false);
        }
        // A blinking cursor or a due tooltip, in any window: the interface asks to run again
        // later.
        let (first, wait) = (self.windows[0].hwnd, self.ui.wake_in());
        // SAFETY: sets, moves or kills the one timer, the first window's.
        match wait {
            Some(wait) => _ = unsafe { SetTimer(first, 1, (wait.as_millis() as u32).max(1), 0) },
            None if self.timer => _ = unsafe { KillTimer(first, 1) },
            None => {}
        }
        self.timer = wait.is_some();
        if self.ui.quitting() {
            // SAFETY: asks the first window to close, as its close button does.
            unsafe { PostMessageW(first, WM_CLOSE, 0, 0) };
        }
    }

    /// Does what a run left window `at`: places to redraw, a title, whether to accept drops
    /// from now on, a drag out. (Each place is invalidated by itself: two small changes far
    /// apart are two small areas, not all between; the bounding rectangle a second window keeps
    /// until asked is not needed then.) Says whether the window's state changed or it came
    /// forward: Windows reported that while nothing was listening.
    fn apply_run(&mut self, at: usize) -> bool {
        let Some((title, takes, top, forward, stood, going)) = self.in_window(at, |ui, window| {
            window.frame(ui.theme.bg.is_dark());
            uia::tell(window.hwnd, ui);
            uia::announce(window.hwnd, ui);
            window.changed(ui);
            ui.changed();
            window.asks_hit_test = window.edges || ui.window_state.title_bar != 0;
            window.keep_ime_off(ui.password_focused());
            // The system file dialog, if the app just asked for it, is shown once this message
            // is handled.
            ui.take_file_dialog_request().into_iter().for_each(|ask| ASKED.with_borrow_mut(|asked| asked.push((window.hwnd, ask))));
            let stood = window.apply_state(ui.take_minimize(), ui.take_maximized(), ui.take_full_screen());
            let takes = ui.takes_files.get() || ui.crossing.takes_text.get();
            let going = ui.take_drag_out().zip(OUTWARD.get()).map(|(what, outward)| ((outward.made)(what), outward));
            (ui.take_title(), takes, ui.take_on_top(), ui.take_bring_to_front(), stood, going)
        }) else {
            return false;
        };
        let window = &mut self.windows[at];
        if let Some(on) = top {
            // SAFETY: the window goes topmost or back (-1, -2), nothing else changing: not its
            // place, size or activation (0x13).
            unsafe { SetWindowPos(window.hwnd, if on { -1 } else { -2 }, 0, 0, 0, 0, 0x0013) };
        }
        if forward {
            window.bring_to_front();
        }
        if let Some(title) = title {
            // SAFETY: the title ends in a zero.
            unsafe { SetWindowTextW(window.hwnd, wide(&title).as_ptr()) };
        }
        // SAFETY: OLE (for dragging in and out) is initialized on this thread the first time a
        // window wants drops or drags something out (unless the program already uses COM in a
        // way that rules it out). It stays for all windows and is released when the first
        // window is destroyed: the window that asked may close mid-drop, which is no time to
        // release OLE.
        let ole = (takes && !window.files) || going.is_some();
        let ole = ole && *self.ole.get_or_insert_with(|| unsafe { OleInitialize(std::ptr::null_mut()) } >= 0);
        if let Some(drops) = DROPS.get().filter(|_| takes && !window.files) {
            (drops.take)(window, ole);
        }
        // A drag out is handed to Windows once this message is handled, which needs OLE:
        // without it nothing goes out.
        if let Some((data, outward)) = going {
            self.queue_drag_out(at, data, outward, ole);
        }
        stood || forward
    }

    /// What the app asked of the desktop goes to Windows once an app on this thread asked for
    /// any (see `windows_shell`): the first window is hidden for the tray, or comes forward for
    /// a later start of the program. Says whether either happened.
    fn apply_desktop_requests(&mut self) -> bool {
        let Some(desktop) = DESKTOP.get() else { return false };
        let (asked, first) = ((desktop.after)(), &mut self.windows[0]);
        let hide = asked.hide && !first.withdrawn;
        if hide {
            // SAFETY: the window is alive; it is hidden (0), its task bar button too.
            unsafe { ShowWindow(first.hwnd, 0) };
            first.withdrawn = true;
        }
        if asked.forward {
            first.bring_to_front();
        }
        hide || asked.forward
    }

    /// Queues what is dragged out of window `at`, with how, to be handed to Windows
    /// (`run_queued_drag_out`); or without OLE, drops it at once, and the interface hears
    /// nothing took it.
    fn queue_drag_out(&mut self, at: usize, data: *mut crate::ole_drag::DataObject, outward: crate::ole_drag::Outward, ole: bool) {
        let hwnd = self.windows[at].hwnd;
        let gone = if ole { OUTGOING.replace(Some((hwnd, data, outward))).map(|(_, was, _)| was) } else { Some(data) };
        if let Some(gone) = gone {
            // SAFETY: the object has one reference, its maker's, which goes to the queue or is
            // released here.
            unsafe { crate::ole_drag::let_go(gone) };
        }
        if !ole {
            self.in_window(at, |ui, _| ui.set_drop_effect(None));
        }
    }

    /// Brings the windows in line with the second windows the interface has after a run: one it
    /// no longer has is destroyed, a new one created.
    fn sync_windows(&mut self) {
        let was = self.windows.len();
        self.destroy_gone_windows();
        let kept = self.windows.len();
        self.create_new_windows();
        if (was, kept) == (kept, self.windows.len()) {
            return;
        }
        // A new dialog blocks the windows before it before it shows.
        self.keep_blocked();
        // Windows reports a window's size and focus during the calls creating, showing and
        // destroying windows, when nothing listens. So each new window's interface is told its
        // size here, and every window where focus went; if that was news to any, the interface
        // runs once more. A new window is shown as the program stands: activated (1) when the
        // person is in one of its windows; without taking focus (4) when the program is in the
        // background; not at all while the first window is hidden.
        let first = self.windows[0].hwnd;
        // SAFETY: plain queries about the first window (alive) and the foreground.
        let (shown, front) = unsafe { (IsWindowVisible(first) != 0, GetForegroundWindow()) };
        let how = if self.windows.iter().any(|window| window.hwnd == front) { 1 } else { 4 };
        let mut news = false;
        for at in kept..self.windows.len() {
            news |= self.fit(at) == Some(true);
            if shown {
                // SAFETY: the new window is alive.
                unsafe { ShowWindow(self.windows[at].hwnd, how) };
            }
        }
        if self.hear_keyboard() || news {
            // SAFETY: posts to the first window, which is alive.
            unsafe { PostMessageW(first, WM_WAKE, 0, 0) };
        }
    }

    /// Destroys each second window the interface no longer has.
    fn destroy_gone_windows(&mut self) {
        for at in (1..self.windows.len()).rev() {
            if self.ui.windows().all(|(id, ..)| id != self.windows[at].id) {
                // A window blocked by a dialog now closing gets keyboard and pointer back
                // first, so Windows returns focus to it.
                self.keep_blocked();
                self.windows.remove(at).destroy();
            }
        }
    }

    /// Creates, hidden, each second window the interface has and Windows does not yet.
    fn create_new_windows(&mut self) {
        let state = self as *mut State as isize;
        for nth in 0..self.ui.windows().count() {
            let new = self.ui.windows().nth(nth).filter(|(id, ..)| self.windows[1..].iter().all(|window| window.id != *id));
            let Some((id, title, size)) = new else { continue };
            // A tool window and a dialog are owned by the first window: Windows keeps them
            // above it, off the task bar, and minimizes them with it.
            let opening = self.ui.window_opening(id).unwrap_or_default();
            let owner = if matches!(opening.kind, WindowKind::Tool | WindowKind::Dialog) { self.windows[0].hwnd } else { 0 };
            let Some(window) = Open::make(title, size, opening, false, self.place_for(size, opening), id, owner) else { continue };
            // SAFETY: the window finds the state by the same pointer the first one does.
            unsafe { SetWindowLongPtrW(window.hwnd, -21, state) };
            self.windows.push(window);
            #[cfg(feature = "gpu")]
            self.give_shown(self.windows.len() - 1, true);
        }
    }

    /// Where a new window's frame corner goes, its client `size`, frame `looks` (none if
    /// frameless), and at what DPI: a step down and right of the window the person is in (or
    /// the last created, when the program is not in front), and a step more while another
    /// window's corner is there, so every title shows. Then moved back just enough to lie on
    /// that window's screen, at its scale. (Nothing when every window is minimized: then the
    /// system places it.)
    fn corner(&self, size: Size, looks: Option<[u32; 2]>) -> Option<(u32, POINT)> {
        let windows = || self.windows.iter().map(|window| window.hwnd);
        // SAFETY: plain queries about live windows and the screen one is on. (Style -16 bit
        // 0x2000_0000: `WS_MINIMIZE`. A window on no screen stays where the steps put it.)
        unsafe {
            let front = GetForegroundWindow();
            let by = windows().filter(|hwnd| *hwnd == front).chain(windows().rev()).find(|hwnd| GetWindowLongPtrW(*hwnd, -16) & 0x2000_0000 == 0)?;
            let dpi = GetDpiForWindow(by);
            let (frame, title) = (framed(size.w, size.h, dpi, looks), framed(0.0, 0.0, dpi, Some([WS_OVERLAPPEDWINDOW, 0])));
            // A step is a title bar's height.
            let (mut at, step) = (corner_of(by), (-title.top).max(1));
            while windows().any(|hwnd| corner_of(hwnd) == at) {
                at = POINT { x: at.x + step, y: at.y + step };
            }
            let mut screen = MONITORINFO { size: size_of::<MONITORINFO>() as u32, all: RECT::default(), work: RECT::default(), flags: 0 };
            if GetMonitorInfoW(MonitorFromWindow(by, 0), &mut screen) != 0 {
                at.x = at.x.min(screen.work.right - (frame.right - frame.left)).max(screen.work.left);
                at.y = at.y.min(screen.work.bottom - (frame.bottom - frame.top)).max(screen.work.top);
            }
            Some((dpi, at))
        }
    }

    /// Where a new second window with client `size` goes, as its app says: at its place or
    /// centred on a screen if given; a dialog centred on the first window; any other a step
    /// from the front window (`corner`). A screen DPI and the frame corner there; nothing where
    /// the system places it.
    fn place_for(&self, size: Size, opening: WindowOpening) -> Option<(u32, POINT)> {
        let looks = (!opening.frameless).then(|| styles(opening.kind, false, false));
        let asked = match (opening.position, opening.screen, opening.kind) {
            (Some((x, y)), ..) => OpenAt::At(x, y),
            (None, Some(nth), _) => OpenAt::Middle(nth),
            (None, None, WindowKind::Dialog) => OpenAt::Over(self.area_of(0)),
            _ => return self.corner(size, looks),
        };
        place_on(self.ui.screens(), asked, size, looks)
    }

    /// Where window `at` lies on the desktop, frame included; or would lie restored, while
    /// minimized.
    fn area_of(&self, at: usize) -> Rect {
        let (window, mut area) = (&self.windows[at], RECT::default());
        // SAFETY: plain queries about a live window.
        if unsafe { IsIconic(window.hwnd) == 0 && GetWindowRect(window.hwnd, &mut area) != 0 } {
            return rect(area.left as f32, area.top as f32, (area.right - area.left) as f32, (area.bottom - area.top) as f32);
        }
        let normal = window.normal;
        rect(normal.position.0 as f32, normal.position.1 as f32, normal.width * window.scale, normal.height * window.scale)
    }

    /// Disables each window while a dialog opened after it is open (Windows withholds keyboard
    /// and pointer from a disabled window), and enables it again once none is. A press on such
    /// a window raises the dialog.
    fn keep_blocked(&mut self) {
        for window in &mut self.windows {
            let blocked = self.ui.window_blocked(window.id);
            if std::mem::replace(&mut window.blocked, blocked) != blocked {
                // SAFETY: the window is alive.
                unsafe { EnableWindow(window.hwnd, !blocked as i32) };
            }
        }
    }

    /// The person wants the first window closed, or the system is logging off: the app hears it
    /// in a run. Says its answer: nothing against it, or why the window stays open.
    fn ask_first(&mut self) -> Result<(), String> {
        self.ui.request_close();
        self.run();
        self.ui.take_closing().unwrap_or(Ok(()))
    }

    /// The screens or a work area changed: the interface is told, and runs if it is news, for
    /// the app to hear.
    fn screens_changed(&mut self) {
        if self.ui.set_screens(screens()) {
            self.run();
        }
    }

    /// Tells each window whether it has focus, where that differs from what it last heard. Says
    /// whether any was told.
    fn hear_keyboard(&mut self) -> bool {
        // SAFETY: asks which of this thread's windows has focus.
        let on = unsafe { GetFocus() };
        let mut told = false;
        for at in 0..self.windows.len() {
            let has = self.windows[at].hwnd == on;
            told |= self.in_window(at, |ui, _| ui.window_focused != has && ui.event(Event::Focused(has))) == Some(true);
        }
        told
    }

    /// Takes a mouse or keyboard message for window `at`, running the interface if needed.
    /// (While a drag leaves the window as soon as the pointer does, the pointer over a window
    /// lying on this one has left this one.)
    fn input(&mut self, at: usize, msg: u32, w: usize, l: isize) {
        let msg = if msg == WM_MOUSEMOVE && self.covered(at, l) { WM_MOUSELEAVE } else { msg };
        let rerun = self.in_window(at, |ui, window| {
            let mut rerun = false;
            // SAFETY: the message's parameters are what Windows sent.
            unsafe { window.messages.events(msg, w, l, window.scale, |event| rerun |= ui.event(event)) };
            rerun
        });
        if rerun == Some(true) {
            self.run();
        }
    }

    /// Whether the pointer at `l` (as Windows packs it) in window `at` is over another window
    /// lying on it, while its drag leaves the window as soon as the pointer does.
    fn covered(&mut self, at: usize, l: isize) -> bool {
        let (hwnd, mut place) = (self.windows[at].hwnd, POINT { x: (l & 0xffff) as i16 as i32, y: (l >> 16 & 0xffff) as i16 as i32 });
        // SAFETY: plain queries about a live window and the screen.
        self.in_window(at, |ui, _| ui.drag_leaves_window()) == Some(true) && unsafe { ClientToScreen(hwnd, &mut place) != 0 && WindowFromPoint(place) != hwnd }
    }

    /// Takes in what a drag over window `at` does, which Windows reports between the window's
    /// messages: files or text from another program, or a drag out of the window brought back.
    /// Runs the interface if needed, and says whether a box of the window took the drop. (A
    /// window its app closed in the run handling the drop counts as having taken it: nobody is
    /// left to ask.)
    fn hear_drag(&mut self, at: usize, events: [Option<Event>; 2]) -> bool {
        self.ui.set_time(self.start.elapsed());
        let hwnd = self.windows[at].hwnd;
        let rerun = self.in_window(at, |ui, _| events.into_iter().flatten().fold(false, |rerun, event| ui.event(event) | rerun));
        if rerun == Some(true) {
            self.run();
        }
        let at = self.windows.iter().position(|window| window.hwnd == hwnd);
        at.is_none_or(|at| self.in_window(at, |ui, _| ui.take_drop_accepted()) == Some(true))
    }

    /// Paints what Windows says needs it in window `at`: each rectangle of the update region by
    /// itself when few, else their bounding rectangle. (The strip is kept as wide as the widest
    /// client, which is ensured here.)
    fn paint(&mut self, at: usize) {
        self.strip.fit(self.windows.iter().map(|window| window.wide).max().unwrap_or(0));
        let (hwnd, stale) = (self.windows[at].hwnd, self.windows[at].stale());
        let mut ps = PAINTSTRUCT { hdc: 0, erase: 0, paint: RECT::default(), restore: 0, update: 0, reserved: [0; 32] };
        // SAFETY: the pair of calls every paint is.
        let dc = unsafe { BeginPaint(hwnd, &mut ps) };
        #[cfg(feature = "gpu")]
        let dc = if self.windows[at].shown.is_some() { 0 } else { dc };
        let parts = stale.parts.get(..stale.header[2] as usize).filter(|parts| !parts.is_empty()).unwrap_or(std::slice::from_ref(&ps.paint));
        parts.iter().for_each(|part| self.draw(at, dc, *part));
        #[cfg(feature = "gpu")]
        self.present(at, parts);
        // SAFETY: ends the paint begun above.
        unsafe { EndPaint(hwnd, &ps) };
        #[cfg(feature = "gpu")]
        self.uncloak(at);
    }

    /// Draws `area` of window `at` a strip at a time, copying each to the window.
    fn draw(&mut self, at: usize, dc: HANDLE, area: RECT) {
        let (s, from, pixels, width, rows) = (self.windows[at].scale, self.strip.dc, self.strip.pixels.get(), self.strip.width.get(), self.strip.rows.get());
        if pixels.is_null() || width <= 0 {
            return;
        }
        let part = rect(area.left as f32 / s, area.top as f32 / s, (area.right - area.left) as f32 / s, (area.bottom - area.top) as f32 / s);
        let mut top = area.top;
        while top < area.bottom {
            let high = rows.min(area.bottom - top);
            // SAFETY: the strip holds `width` by `rows` pixels, and nothing else uses them now.
            let block = unsafe { std::slice::from_raw_parts_mut(pixels, (width * high) as usize) };
            let mut canvas = Canvas::new(block, width as usize, high as usize, (0, top), s);
            canvas.clip(part);
            self.in_window(at, |ui, _| ui.paint(&mut canvas));
            #[cfg(feature = "gpu")]
            if dc == 0 {
                self.upload(at, area, top, high);
                top += high;
                continue;
            }
            // SAFETY: copies the strip to the window (`SRCCOPY`); both are alive.
            unsafe { BitBlt(dc, area.left, top, area.right - area.left, high, from, area.left, 0, 0x00CC_0020) };
            top += high;
        }
    }

    /// Takes in window `at`'s client size and screen place and tells its interface. Says
    /// whether any of it was news. (Nothing for a minimized window: it has no size and keeps
    /// its old one.)
    fn fit(&mut self, at: usize) -> Option<bool> {
        let (hwnd, mut area) = (self.windows[at].hwnd, RECT::default());
        // SAFETY: plain queries about a live window.
        let (_, corner, zoomed) = unsafe { (GetClientRect(hwnd, &mut area), corner_of(hwnd), IsZoomed(hwnd) != 0) };
        if area.right <= 0 || area.bottom <= 0 {
            return None;
        }
        (self.windows[at].wide, self.windows[at].high) = (area.right, area.bottom);
        #[cfg(feature = "gpu")]
        self.fit_shown(at);
        self.in_window(at, |ui, window| {
            let size = Size { w: area.right as f32 / window.scale, h: area.bottom as f32 / window.scale };
            // Its place and size while neither maximized nor fullscreen: what an app keeps to
            // reopen the window another day.
            if !zoomed && window.before_fullscreen.is_none() {
                window.normal = WindowPlacement { position: (corner.x, corner.y), width: size.w, height: size.h, maximized: false };
            }
            let news = (ui.size(), ui.scale()) != (size, window.scale);
            ui.resize(size, window.scale);
            ui.set_window_placement(WindowPlacement { maximized: zoomed, ..window.normal }) | news
        })
    }

    /// What of window `at` lies at screen point `l` (as Windows packs it), as a `WM_NCHITTEST`
    /// code. The system frame comes first, where it has one; then the interface says whether it
    /// is a resize edge (10 to 17: left, right, top, the top corners, bottom, the bottom
    /// corners), the move strip (2, `HTCAPTION`, with all a caption does), or client (1). A
    /// maximized window has no resize edges.
    fn hit_test(&mut self, at: usize, l: isize) -> Option<isize> {
        let (hwnd, scale, edges) = (self.windows[at].hwnd, self.windows[at].scale, self.windows[at].edges);
        // SAFETY: plain queries about a live window; they make Windows send nothing back.
        let (of_frame, sized) = unsafe { (DefWindowProcW(hwnd, WM_NCHITTEST, 0, l), edges && IsZoomed(hwnd) == 0) };
        let place = pointer_at(hwnd, POINT { x: (l & 0xffff) as i16 as i32, y: (l >> 16 & 0xffff) as i16 as i32 }, scale);
        let (Some(Event::Move(place)), 1) = (place, of_frame) else { return Some(of_frame) };
        Some(match self.in_window(at, |ui, _| ui.hit_test(place, sized))? {
            HitTest::Client => 1,
            HitTest::TitleBar => 2,
            HitTest::Edge(across, down) => [13, 12, 14, 10, 1, 11, 16, 15, 17][(down * 3 + across + 4) as usize],
        })
    }

    /// Window `at` was resized, minimized or restored: taken in, and the interface runs.
    fn resized(&mut self, at: usize) {
        let told = self.back(at);
        if self.fit(at).is_some() || told {
            self.run();
        }
    }
    /// Tells window `at`'s interface whether it is minimized or restored, and whether
    /// fullscreen, where either differs from what it last heard. Says whether anything was
    /// told.
    fn back(&mut self, at: usize) -> bool {
        // SAFETY: a plain query about a live window.
        let hidden = unsafe { IsIconic(self.windows[at].hwnd) } != 0 || self.windows[at].withdrawn;
        self.in_window(at, |ui, window| (ui.window_minimized() != hidden && ui.event(Event::Minimized(hidden))) | ui.full_screen_now(window.before_fullscreen.is_some())) == Some(true)
    }
    /// A system setting changed: if it concerns how windows look and move, or a work area
    /// (`what` is `SPI_SETWORKAREA`, 0x2F), the interface is told and runs for the app to hear.
    fn settings_changed(&mut self, what: usize) {
        if self.ui.set_preferences(crate::preferences::of_windows()) | (what == 0x2F && self.ui.set_screens(screens())) {
            self.run();
        }
    }
    /// Window `at` moved: its interface is told and runs, so its app hears.
    fn moved(&mut self, at: usize) {
        if self.fit(at) == Some(true) {
            self.run();
        }
    }
}

/// "The GPU device came": posted by the thread that makes it (`show_by_gpu`).
#[cfg(feature = "gpu")]
const WM_GPU: u32 = 0x8010;

/// The GPU paths (`gpu_windows`): the device, what shows each window, uploads, presents, and a
/// lost device.
#[cfg(feature = "gpu")]
impl State {
    /// Has the GPU show the windows where the user's choice or the app's asks for it and a
    /// device works; never a see-through window. The device is made on a thread of its own (a
    /// GPU's driver takes a few hundred milliseconds to load) while the CPU shows the windows;
    /// `gpu_came` takes it. The first window is cloaked until its first paint if `cloak`.
    fn show_by_gpu(&mut self, asked: crate::platform::Renderer, transparent: bool, cloak: bool) {
        let asked = asked.chosen(std::env::var("TAUT_RENDERER").ok().as_deref());
        if asked == crate::platform::Renderer::Cpu || transparent {
            return;
        }
        if cloak {
            self.cloak(0);
        }
        let hwnd = self.windows[0].hwnd;
        #[cfg(test)]
        let faked = crate::gpu_windows::tests::faked();
        self.coming = Some(std::thread::spawn(move || {
            #[cfg(test)]
            crate::gpu_windows::tests::fake(faked);
            let device = crate::gpu_windows::Device::gpu(asked, asked == crate::platform::Renderer::Auto);
            // SAFETY: posts to the first window by handle; to one gone by then, it fails.
            unsafe { PostMessageW(hwnd, WM_GPU, 0, 0) };
            device
        }));
        // Tests have it at once, so every window test runs on the GPU path; unless one asks.
        #[cfg(test)]
        if !faked.later {
            self.gpu_came();
        }
    }

    /// The device came, or none could be made: every window but a see-through one gets a swap
    /// chain on it and is drawn and presented whole, at once.
    fn gpu_came(&mut self) {
        let Some(coming) = self.coming.take() else { return };
        self.device = coming.join().ok().flatten();
        #[cfg(test)]
        crate::gpu_windows::tests::came();
        for at in 0..self.windows.len() {
            if self.windows[at].transparent.is_none() {
                self.give_shown(at, false);
                self.draw_whole(at);
            }
        }
        if self.windows.iter().all(|window| window.shown.is_none()) {
            self.device = None;
        }
    }

    /// Has the device show window `at`, if there is one and it can; a new window is then cloaked
    /// until its first paint if `cloak`. Its interface hears what shows it.
    fn give_shown(&mut self, at: usize, cloak: bool) {
        let (hwnd, size) = (self.windows[at].hwnd, [self.windows[at].wide, self.windows[at].high]);
        let shown = self.device.as_ref().and_then(|device| crate::gpu_windows::Shown::new(device, hwnd, size));
        let renderer = match (&shown, &self.device) {
            (Some(_), Some(device)) => device.renderer(),
            _ => crate::platform::Renderer::Cpu,
        };
        if cloak && shown.is_some() {
            self.cloak(at);
        }
        self.windows[at].shown = shown;
        self.in_window(at, |ui, _| ui.renderer = renderer);
    }

    /// Draws all of window `at` into its texture and presents it, now.
    fn draw_whole(&mut self, at: usize) {
        let (window, widest) = (&self.windows[at], self.windows.iter().map(|window| window.wide).max().unwrap_or(0));
        let all = RECT { left: 0, top: 0, right: window.wide, bottom: window.high };
        if window.shown.is_some() {
            self.strip.fit(widest);
            self.draw(at, 0, all);
            self.present(at, &[all]);
            self.uncloak(at);
        }
    }

    /// Hides window `at` until its first paint (DWM's cloak, 13): never a frame of nothing. (Not
    /// a window opening minimized: cloaked, it would have no task bar button.)
    fn cloak(&mut self, at: usize) {
        let window = &mut self.windows[at];
        // SAFETY: the window is alive; the number outlives the call.
        window.cloaked = unsafe { DwmSetWindowAttribute(window.hwnd, 13, (&1i32 as *const i32).cast(), 4) } >= 0;
    }

    /// Shows window `at` once painted, if cloaked.
    fn uncloak(&mut self, at: usize) {
        let window = &mut self.windows[at];
        if std::mem::take(&mut window.cloaked) {
            // SAFETY: the window is alive; the number outlives the call.
            unsafe { DwmSetWindowAttribute(window.hwnd, 13, (&0i32 as *const i32).cast(), 4) };
        }
    }

    /// Gives window `at`'s swap chain and texture its new size, and has all of it drawn again. A
    /// failure is a lost device.
    fn fit_shown(&mut self, at: usize) {
        let (Some(device), window) = (&self.device, &mut self.windows[at]) else { return };
        let Some(shown) = &mut window.shown else { return };
        match shown.fit(device, [window.wide, window.high]) {
            // SAFETY: the window is alive.
            Some(true) => _ = unsafe { InvalidateRect(window.hwnd, std::ptr::null(), 0) },
            Some(false) => {}
            None => self.lost(),
        }
    }

    /// Puts `high` rows of the strip, drawn for `area` from row `top`, into what shows window
    /// `at`.
    fn upload(&mut self, at: usize, area: RECT, top: i32, high: i32) {
        let (Some(device), Some(shown)) = (&self.device, &mut self.windows[at].shown) else { return };
        let (pixels, width) = (self.strip.pixels.get(), self.strip.width.get());
        let first = pixels.wrapping_add(area.left.clamp(0, width) as usize);
        // SAFETY: the strip holds `width` by at least `high` pixels, and the area lies within
        // it: it is no wider than its window, which is no wider than the strip.
        unsafe { shown.upload(device, [area.left, top], [area.right - area.left, high], first, width as usize) };
    }

    /// Presents window `at`, `parts` of it drawn anew. A lost device is made again.
    fn present(&mut self, at: usize, parts: &[RECT]) {
        let (Some(device), window) = (&self.device, &mut self.windows[at]) else { return };
        let Some(shown) = &mut window.shown else { return };
        if !shown.present(device, parts) && device.removed() {
            self.lost();
        }
    }

    /// The device is lost. Every window it showed is shown by a new one (a GPU, the same path
    /// first, if the old one was and one works, else WARP) and drawn again, whole; with no
    /// device, GDI is left. Each one's interface hears what shows it now.
    fn lost(&mut self) {
        let Some(old) = self.device.take() else { return };
        let had: Vec<usize> = (0..self.windows.len()).filter(|at| self.windows[*at].shown.take().is_some()).collect();
        self.device = crate::gpu_windows::Device::again(old);
        for at in had {
            self.give_shown(at, false);
            // SAFETY: the window is alive.
            unsafe { InvalidateRect(self.windows[at].hwnd, std::ptr::null(), 0) };
        }
    }
}

fn mods() -> Mods {
    // SAFETY: asks which keys are down (Shift, Control, Alt).
    let down = |key: i32| unsafe { GetKeyState(key) } < 0;
    Mods { shift: down(0x10), ctrl: down(0x11), alt: down(0x12) }
}

/// The modifiers held once modifier key `code` (Shift, Control or Alt) went down or up: the
/// others as the system has them.
fn held_with(code: usize, down: bool) -> Mods {
    let mut held = mods();
    match code {
        0x10 => held.shift = down,
        0x11 => held.ctrl = down,
        _ => held.alt = down,
    }
    held
}

fn key(code: usize) -> Option<Key> {
    Some(match code {
        0x08 => Key::Backspace,
        0x09 => Key::Tab,
        0x0D => Key::Enter,
        0x1B => Key::Escape,
        0x20 => Key::Space,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x23 => Key::End,
        0x24 => Key::Home,
        0x25 => Key::Left,
        0x26 => Key::Up,
        0x27 => Key::Right,
        0x28 => Key::Down,
        0x2E => Key::Delete,
        0x5D => Key::Menu,
        0x30..=0x39 | 0x41..=0x5A => Key::Char((code as u8 as char).to_ascii_lowercase()),
        // The zoom keys: "=" ("+" with Shift) and "-", and the two on the number pad.
        0xBB => Key::Char('='),
        0xBD | 0x6D => Key::Char('-'),
        0x6B => Key::Char('+'),
        0x70..=0x7B => Key::F((code - 0x6F) as u8),
        _ => return None,
    })
}

/// The `FORMATETC` asking a dragged object for its files: `CF_HDROP` (15), all of it (1, -1),
/// as an `HGLOBAL` (1). And for its text: `CF_UNICODETEXT` (13), the same way.
const FILES: FORMATETC = FORMATETC { format: 15, device: std::ptr::null(), aspect: 1, index: -1, medium: 1 };
const TEXT: FORMATETC = FORMATETC { format: 13, ..FILES };
/// The drop effect told to Windows: copy, which leaves the source nothing to do. (Or 0: none.)
const COPY: u32 = 1;

/// What a drag over a window brings that its app takes: files, text, or nothing (also while it
/// is a drag out of this same window, which its source handles: see `ole_drag`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Brings {
    Nothing,
    Files,
    Text,
}

impl Brings {
    /// That it is over the window, or no longer is, as the interface hears it.
    fn over(self, on: bool) -> Event {
        if self == Brings::Text { Event::TextHovered(on) } else { Event::FilesHovered(on) }
    }
    /// What was dropped, as the interface hears it, from its `HGLOBAL`.
    ///
    /// # Safety
    /// `memory` is a live `HGLOBAL`, or 0.
    unsafe fn let_go(self, memory: HANDLE) -> Event {
        // SAFETY: as the caller promises.
        match self {
            Brings::Text => Event::TextDropped(unsafe { dropped_text(memory) }),
            _ => Event::FilesDropped(unsafe { dropped_files(memory) }.1),
        }
    }
}

/// The window's `IDropTarget`, which Windows calls as a drag enters, moves over, leaves or
/// drops on the window. A COM object: its vtable pointer comes first. Reference counted; it
/// frees itself when the last reference is released.
#[repr(C)]
struct DropTarget {
    table: &'static DropTargetTable,
    held: Cell<u32>,
    hwnd: HANDLE,
    /// What the drag over the window now brings that may be dropped.
    brings: Cell<Brings>,
}

/// `IDropTarget`'s vtable, in Windows' order.
#[repr(C)]
struct DropTargetTable {
    query: unsafe extern "system" fn(*mut DropTarget, *const [u32; 4], *mut *mut DropTarget) -> i32,
    hold: unsafe extern "system" fn(*mut DropTarget) -> u32,
    release: unsafe extern "system" fn(*mut DropTarget) -> u32,
    enter: unsafe extern "system" fn(*mut DropTarget, *mut IDataObject, u32, POINT, *mut u32) -> i32,
    over: unsafe extern "system" fn(*mut DropTarget, u32, POINT, *mut u32) -> i32,
    leave: unsafe extern "system" fn(*mut DropTarget) -> i32,
    let_go: unsafe extern "system" fn(*mut DropTarget, *mut IDataObject, u32, POINT, *mut u32) -> i32,
}

static DROP_TARGET: DropTargetTable = DropTargetTable {
    query: DropTarget::query,
    hold: DropTarget::hold,
    release: DropTarget::release,
    enter: DropTarget::enter,
    over: DropTarget::over,
    leave: DropTarget::leave,
    let_go: DropTarget::let_go,
};

impl DropTarget {
    /// A drop target for window `hwnd`, with one reference: the caller's.
    fn new(hwnd: HANDLE) -> *mut DropTarget {
        Box::into_raw(Box::new(DropTarget { table: &DROP_TARGET, held: Cell::new(1), hwnd, brings: Cell::new(Brings::Nothing) }))
    }

    /// `QueryInterface`: it is an `IDropTarget` and, like every COM object, an `IUnknown` (two
    /// GUIDs differing in their first field only).
    unsafe extern "system" fn query(this: *mut DropTarget, name: *const [u32; 4], out: *mut *mut DropTarget) -> i32 {
        // SAFETY: Windows passes the requested IID and room for the answer: this object with
        // one more reference, or null.
        unsafe {
            let known = matches!(name.read_unaligned(), [0 | 0x122, 0, 0xC0, 0x4600_0000]);
            *out = if known { this } else { std::ptr::null_mut() };
            if !known {
                return 0x8000_4002_u32 as i32; // `E_NOINTERFACE`
            }
            Self::hold(this);
        }
        0
    }

    /// `AddRef`.
    unsafe extern "system" fn hold(this: *mut DropTarget) -> u32 {
        // SAFETY: the caller holds a reference, so it is alive.
        let held = unsafe { &(*this).held };
        held.set(held.get() + 1);
        held.get()
    }

    /// `Release`: freed when the count reaches zero.
    unsafe extern "system" fn release(this: *mut DropTarget) -> u32 {
        // SAFETY: the caller holds a reference, so it is alive. It was made with
        // `Box::into_raw` in `new`, and nothing uses it once the last reference is gone.
        unsafe {
            let left = (*this).held.get() - 1;
            (*this).held.set(left);
            if left == 0 {
                drop(Box::from_raw(this));
            }
            left
        }
    }

    /// `DragEnter`. If it brings files or text the window's app takes, and its source allows
    /// copying, it may be dropped here: the interface hears.
    unsafe extern "system" fn enter(this: *mut DropTarget, data: *mut IDataObject, _keys: u32, at: POINT, effect: *mut u32) -> i32 {
        // SAFETY: Windows passes this object, the data object, and the allowed effects, also
        // where the answer goes.
        let (target, effect) = unsafe { (&*this, &mut *effect) };
        let brings = if *effect & COPY == 0 { Brings::Nothing } else { target.brought(data) };
        target.brings.set(brings);
        target.tell(|window| [Some(brings.over(true)), pointer_at(window.hwnd, at, window.scale)]);
        *effect = target.effect();
        0
    }

    /// What a drag brings that the app takes: files where it asks about files and the drag has
    /// them; else text, likewise. (The data object is asked through its own vtable,
    /// `QueryGetData`.)
    fn brought(&self, data: *mut IDataObject) -> Brings {
        // SAFETY: Windows passed the data object, alive during the call.
        let has = |form: &FORMATETC| unsafe { ((*(*data).table).query_get_data)(data, form) == 0 };
        let [files, text] = self.takes();
        match () {
            _ if crate::ole_drag::is_dragging(data.cast(), self.hwnd) => Brings::Nothing,
            _ if files && has(&FILES) => Brings::Files,
            _ if text && has(&TEXT) => Brings::Text,
            _ => Brings::Nothing,
        }
    }

    /// `DragOver`: the drag moves over the window (or rests: Windows asks repeatedly).
    unsafe extern "system" fn over(this: *mut DropTarget, _keys: u32, at: POINT, effect: *mut u32) -> i32 {
        // SAFETY: Windows passes this object and room for the answer.
        let (target, effect) = unsafe { (&*this, &mut *effect) };
        target.tell(|window| [pointer_at(window.hwnd, at, window.scale), None]);
        *effect = target.effect();
        0
    }

    /// `DragLeave`: the drag left or was cancelled over the window; the pointer left with it.
    unsafe extern "system" fn leave(this: *mut DropTarget) -> i32 {
        // SAFETY: Windows passes this object.
        let target = unsafe { &*this };
        let brings = target.brings.get();
        target.tell(|_| [Some(brings.over(false)), Some(Event::Leave)]);
        target.brings.set(Brings::Nothing);
        0
    }

    /// `Drop`: its files or text are read and handed to the interface with the place. The
    /// source is told they were copied if a box of the window took them.
    unsafe extern "system" fn let_go(this: *mut DropTarget, data: *mut IDataObject, _keys: u32, at: POINT, effect: *mut u32) -> i32 {
        let mut handed = STGMEDIUM { medium: 0, memory: 0, owner: std::ptr::null_mut() };
        // SAFETY: Windows passes this object, the data object, and room for the answer. The
        // data object is asked through its own vtable; what it returns is read if it is the
        // `HGLOBAL` asked for, and released as it wants.
        let (target, effect, read) = unsafe {
            let (target, brings) = (&*this, (*this).brings.get());
            let form = if brings == Brings::Text { &TEXT } else { &FILES };
            let got = brings != Brings::Nothing && ((*(*data).table).get_data)(data, form, &mut handed) == 0;
            let read = if got && handed.medium == form.medium { brings.let_go(handed.memory) } else { brings.over(false) };
            if got {
                ReleaseStgMedium(&mut handed);
            }
            (target, &mut *effect, read)
        };
        let taken = target.tell(|window| [pointer_at(window.hwnd, at, window.scale), Some(read)]);
        target.brings.set(Brings::Nothing);
        *effect = if taken { COPY } else { 0 };
        0
    }

    /// The drop effect to tell Windows: copy, or none (0).
    fn effect(&self) -> u32 {
        if self.brings.get() == Brings::Nothing { 0 } else { COPY }
    }

    /// What the window's app takes from drags: files, and text.
    fn takes(&self) -> [bool; 2] {
        // SAFETY: as in `tell`.
        let nested = ANSWERING.replace(true);
        let state = if nested { None } else { unsafe { state_of(self.hwnd) } };
        let takes = state.and_then(|(state, at)| state.in_window(at, |ui, _| [ui.takes_files.get(), ui.crossing.takes_text.get()]));
        ANSWERING.set(nested);
        takes.unwrap_or_default()
    }

    /// Tells the window's interface what the drag over it does, running it if needed. (`events`
    /// builds them from the window's kept state, which knows its place and scale.) While the
    /// drag brings nothing the window takes, nothing is told. Says whether a box of the window
    /// took the drop.
    fn tell(&self, events: impl FnOnce(&Open) -> [Option<Event>; 2]) -> bool {
        // SAFETY: Windows calls a drop target on its window's thread between the windows'
        // messages: nothing else is using the state. (Were it ever called mid-message, nothing
        // is told.)
        let nested = ANSWERING.replace(true);
        let state = if nested || self.brings.get() == Brings::Nothing { None } else { unsafe { state_of(self.hwnd) } };
        let taken = state.is_some_and(|(state, at)| state.hear_drag(at, events(&state.windows[at])));
        ANSWERING.set(nested);
        taken
    }
}

/// The UTF-16 text in an `HGLOBAL`: up to its first zero, at most to the block's end.
///
/// # Safety
/// `memory` is a live `HGLOBAL`, or 0.
unsafe fn dropped_text(memory: HANDLE) -> String {
    // SAFETY: as the caller promises. The block is locked while read, and read no further than
    // its size. (A null handle gives nothing to read.)
    unsafe {
        let start = GlobalLock(memory).cast::<u8>();
        let block = if start.is_null() { &[][..] } else { std::slice::from_raw_parts(start, GlobalSize(memory)) };
        let units: Vec<u16> = block.as_chunks().0.iter().map(|pair| u16::from_le_bytes(*pair)).take_while(|unit| *unit != 0).collect();
        GlobalUnlock(memory);
        String::from_utf16_lossy(&units)
    }
}

/// A drag out of window `hwnd` is over it again at a screen point, or left it (none): its
/// interface hears where the pointer is, as for any drag inside it. (Windows asks the drag
/// source this while dragging, between the windows' messages.)
pub(crate) fn drag_back_over(hwnd: HANDLE, at: Option<POINT>) {
    let nested = ANSWERING.replace(true);
    // SAFETY: as in `DropTarget::tell`.
    if let Some((state, at_window)) = if nested { None } else { unsafe { state_of(hwnd) } } {
        let event = at.and_then(|at| pointer_at(hwnd, at, state.windows[at_window].scale)).unwrap_or(Event::Leave);
        state.hear_drag(at_window, [Some(event), None]);
    }
    ANSWERING.set(nested);
}

/// What came of a drag out of window `hwnd`: its interface hears what the other program did
/// with it, that the button came up (which Windows kept meanwhile), and where the pointer is if
/// it was dropped on the window itself: there it drops as any drag inside the window.
fn drag_out_ended(hwnd: HANDLE, outcome: crate::ole_drag::Outcome) {
    let nested = ANSWERING.replace(true);
    // SAFETY: as in `DropTarget::tell`.
    if let Some((state, at)) = if nested { None } else { unsafe { state_of(hwnd) } } {
        let place = outcome.home.and_then(|home| pointer_at(hwnd, home, state.windows[at].scale)).unwrap_or(Event::Leave);
        // Windows had the mouse meanwhile: `TrackMouseEvent` is asked again. Should the window
        // still hold capture, it releases it: the button is up.
        state.windows[at].tracking = false;
        // SAFETY: plain queries and calls about this thread's mouse.
        if unsafe { GetCapture() } == hwnd {
            unsafe { ReleaseCapture() };
        }
        state.in_window(at, |ui, _| ui.set_drop_effect(outcome.taken));
        state.hear_drag(at, [Some(place), Some(Event::Release(Button::Left))]);
    }
    ANSWERING.set(nested);
}

/// Where the app hands something out of a window ([`Ui::drag_out`]): from then on it goes to
/// Windows through OLE (see `ole_drag::Outward`).
pub(crate) fn enable_drag_out() {
    OUTWARD.set(Some(crate::ole_drag::OUTWARD));
}

/// Hands Windows what the person just dragged out of a window, if anything: between two
/// messages, when no window is in use, since `DoDragDrop` then follows the mouse until the drop
/// (the windows' messages still arriving meanwhile).
fn run_queued_drag_out() {
    while let Some((hwnd, data, outward)) = OUTGOING.take() {
        // SAFETY: the object was made for this and has one reference, the queue's.
        let outcome = unsafe { (outward.run)(hwnd, data) };
        drag_out_ended(hwnd, outcome);
    }
}

/// The state `open` stored on the first window (each second window gets it on creation), alive
/// until the first window is destroyed; and which of its windows this one is.
///
/// # Safety
/// The windows' own thread asks, holding no other reference to the state.
unsafe fn state_of<'a>(hwnd: HANDLE) -> Option<(&'a mut State, usize)> {
    // SAFETY: the pointer is the one `open` stored, or null.
    let state = unsafe { (GetWindowLongPtrW(hwnd, -21) as *mut State).as_mut() }?;
    let at = state.windows.iter().position(|window| window.hwnd == hwnd)?;
    Some((state, at))
}

thread_local! {
    /// Whether this thread is inside the handling of one of the windows' messages.
    static ANSWERING: Cell<bool> = const { Cell::new(false) };
}

unsafe extern "system" fn window_proc(hwnd: HANDLE, msg: u32, w: usize, l: isize) -> isize {
    // A frameless window is all client. Windows sends `WM_NCCALCSIZE` inside every call that
    // resizes a window, so it is answered here first, from the window's own extra bytes.
    // SAFETY: Windows calls this for a window of this library's class, which has those bytes.
    if msg == WM_NCCALCSIZE && unsafe { GetWindowLongPtrW(hwnd, 0) } != 0 {
        return unsafe { all_inside(hwnd, l) };
    }
    // Some calls a handler makes (set the title, move, create or destroy a window) make Windows
    // send more messages before returning, to that window or another of the program's. The
    // shared state is then in use by that handler: such a message goes to `DefWindowProcW`, and
    // the handler deals with whatever it would have changed once its call returns.
    let nested = ANSWERING.replace(true);
    // SAFETY: Windows calls this on the windows' own thread, and nothing else is using their
    // state now.
    let answer = if nested { None } else { unsafe { answer(hwnd, msg, w, l) } };
    ANSWERING.set(nested);
    // SAFETY: what the window does not answer goes to `DefWindowProcW` unchanged. (Done here,
    // with the state released: moving or resizing a window, for one, does not return until the
    // person is done, and the windows are drawn meanwhile.)
    answer.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, w, l) })
}

/// What a window does with a message; nothing for one that is Windows' to answer.
///
/// # Safety
/// The windows' own thread asks, holding no reference to their state.
unsafe fn answer(hwnd: HANDLE, msg: u32, w: usize, l: isize) -> Option<isize> {
    // SAFETY: as the caller promises.
    let (state, at) = unsafe { state_of(hwnd) }?;
    state.ui.set_time(state.start.elapsed());
    match msg {
        // A see-through window is shown from its picture, drawn after each run: nothing to
        // paint, and Windows is left to validate.
        WM_PAINT if state.windows[at].transparent.is_none() => state.paint(at),
        WM_ERASEBKGND => return Some(1),
        #[cfg(feature = "gpu")]
        WM_GPU => state.gpu_came(),
        WM_NCHITTEST if state.windows[at].asks_hit_test => return state.hit_test(at, l),
        WM_SIZE => state.resized(at),
        WM_MOVE => state.moved(at),
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED if at == 0 => state.settings_changed(w),
        WM_DISPLAYCHANGE if at == 0 => state.screens_changed(),
        WM_ANSWERED => {
            state.in_window(at, |ui, _| ui.file_dialog_answered(ANSWER.take()));
            state.run();
        }
        WM_WAKE | WM_TIMER => state.run(),
        // An accessibility client asks for the window's provider. From now on the interface
        // keeps its windows' tree, built each run.
        WM_GETOBJECT => {
            return uia::give(hwnd, w, l, state.ui.listening, || {
                state.ui.set_screen_reader(true);
                state.run();
            });
        }
        // And it asks for something in the window to be done for its user.
        uia::WM_READER => {
            state.in_window(at, |ui, _| uia::work(hwnd, ui));
            state.run();
        }
        // Finger and pen pointer messages are the interface's alone: left to Windows, they
        // would arrive again as mouse messages. (Without the feature `touch`, that is what they
        // are.)
        WM_POINTERUPDATE..=WM_POINTERCAPTURECHANGED if cfg!(feature = "touch") && touch_windows::taken(msg, w) => {
            state.input(at, msg, w, l);
            return Some(0);
        }
        WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            state.windows[at].follow_mouse(msg, w);
            state.input(at, msg, w, l);
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL | WM_KEYDOWN | WM_KEYUP | WM_CHAR | WM_SETFOCUS | WM_KILLFOCUS => state.input(at, msg, w, l),
        WM_SYSKEYDOWN | WM_SYSKEYUP => {
            state.input(at, msg, w, l);
            return None; // Alt+F4 and the like stay the system's
        }
        // While the window has a menu bar, Alt+letter and Alt or F10 alone are its own (they
        // open its menus): the system would beep at the letter or put the keyboard into the
        // window's system menu. (Alt+Space still opens that one.)
        WM_SYSCHAR if w != ' ' as usize && state.in_window(at, |ui, _| ui.takes_alt()) == Some(true) => return Some(0),
        WM_SYSCOMMAND if w & 0xFFF0 == SC_KEYMENU && l == 0 && state.in_window(at, |ui, _| ui.takes_alt()) == Some(true) => return Some(0),
        WM_IME_STARTCOMPOSITION | WM_IME_COMPOSITION => {
            state.in_window(at, |ui, window| window.follow_cursor(ui.ime_area()));
            return None; // Windows shows the composition
        }
        WM_DROPFILES => {
            state.input(at, msg, w, l);
            // SAFETY: the files were read, and the drop is the window's to end, as Windows says
            // a drop is ended.
            unsafe { DragFinish(w as HANDLE) };
        }
        WM_SETCURSOR if (l & 0xffff) == 1 => {
            let shape = system_cursor(state.in_window(at, |ui, _| ui.cursor()).unwrap_or(Cursor::Arrow));
            // SAFETY: the system's own cursors, by number.
            unsafe { SetCursor(LoadCursorW(0, std::ptr::without_provenance(shape))) };
            return Some(1);
        }
        WM_GETMINMAXINFO => {
            let window = &state.windows[at];
            let frame = framed(window.min_size.0, window.min_size.1, (window.scale * 96.0).round() as u32, styles_of(hwnd));
            // SAFETY: `l` points at the `MINMAXINFO` Windows wants filled in.
            unsafe { (*(l as *mut MINMAXINFO)).min_track = POINT { x: frame.right - frame.left, y: frame.bottom - frame.top } };
        }
        // The person is moving the window onto a screen of another scale (`w` dpi): Windows
        // asks its size there, which is the same client size in units.
        WM_GETDPISCALEDSIZE => {
            let window = &state.windows[at];
            let frame = framed(window.wide as f32 / window.scale, window.high as f32 / window.scale, w as u32, styles_of(hwnd));
            // SAFETY: `l` points at the size Windows will give the window, for this to fill in.
            unsafe { *(l as *mut POINT) = POINT { x: frame.right - frame.left, y: frame.bottom - frame.top } };
            return Some(1);
        }
        // It arrived (or its screen's scale changed): Windows gives it the size asked above.
        // (The screens are queried again: one may have another scale.)
        WM_DPICHANGED => {
            state.ui.set_screens(screens());
            state.windows[at].scale = (w & 0xffff) as f32 / 96.0;
            // SAFETY: `l` points at the place Windows suggests on the new screen.
            let to = unsafe { *(l as *const RECT) };
            unsafe { SetWindowPos(hwnd, 0, to.left, to.top, to.right - to.left, to.bottom - to.top, 0x0014) };
            state.resized(at);
        }
        // A second window's close button: the app is told, and closes the window by no longer
        // describing it. (The first window is Windows' to close, which destroys it.)
        WM_CLOSE if at > 0 => {
            state.in_window(at, |ui, _| ui.request_close());
            state.run();
        }
        // The first window's close button (or the task bar's, or Alt+F4): the app is asked, and
        // Windows closes the window (destroying it and ending the program) only if the app does
        // not object. (Where the app itself asked, `quit`, it goes without asking.)
        WM_CLOSE if !state.ui.quitting() => {
            if state.ask_first().is_ok() {
                return None;
            }
        }
        // The system is logging off or shutting down: the app is asked as for the close button,
        // once, through the first window. If it refuses, Windows is told why and shows that
        // beside the program's name for the person to decide. (Windows answers for the other
        // windows, agreeing.)
        WM_QUERYENDSESSION if at == 0 => {
            let answer = state.ask_first();
            let why = wide(answer.as_ref().err().map_or("", String::as_str));
            // SAFETY: the window is alive, and the reason ends in a zero.
            unsafe { if answer.is_ok() { ShutdownBlockReasonDestroy(hwnd) } else { ShutdownBlockReasonCreate(hwnd, why.as_ptr()) } };
            return Some(answer.is_ok() as isize);
        }
        // Done or cancelled: no reason is left with Windows. SAFETY: the window is alive.
        WM_ENDSESSION if at == 0 => _ = unsafe { ShutdownBlockReasonDestroy(hwnd) },
        // A second window destroyed by anything else is closed for the app all the same.
        // (Windows is destroying it already: asking again does nothing.)
        WM_DESTROY if at > 0 => {
            state.in_window(at, |ui, _| ui.request_close());
            state.windows.remove(at).destroy();
            state.run();
        }
        // SAFETY: nothing uses the state after this.
        WM_DESTROY => unsafe { first_destroyed(hwnd, state) },
        _ => return None,
    }
    Some(0)
}

/// The system's own cursor of each shape, by number (`IDC_*`).
fn system_cursor(cursor: Cursor) -> usize {
    match cursor {
        Cursor::Arrow => 32512,
        Cursor::Hand => 32649,
        Cursor::Text => 32513,
        Cursor::ResizeX => 32644,
        Cursor::ResizeY => 32645,
        Cursor::ResizeDown => 32642,
        Cursor::ResizeUp => 32643,
        Cursor::Move => 32646,
        Cursor::No => 32648,
    }
}

/// The first window is gone, and the program's other windows go with it: the state is freed
/// and the message loop ends.
///
/// # Safety
/// `state` is the one `open` stored on `hwnd`, and nothing uses it after this.
unsafe fn first_destroyed(hwnd: HANDLE, state: &mut State) {
    // The program's icons leave the tray with it.
    if let Some(desktop) = DESKTOP.get() {
        (desktop.ended)();
    }
    // Not while a drag out of the window is in progress: OLE still drives it.
    let ole = state.ole == Some(true) && !crate::ole_drag::dragging_out();
    state.windows.drain(1..).for_each(Open::destroy);
    uia::gone(hwnd);
    // A `Waker` wakes no window from now on: the handle may soon be another's.
    _ = WINDOW.compare_exchange(hwnd, 0, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed);
    // SAFETY: Windows releases the window's drop target, if any, which frees it; and OLE is
    // uninitialized if it was initialized for that. The state was made with `Box::into_raw` in
    // `open`; no window finds it any more, and nothing uses it after this, as the caller
    // promises.
    unsafe {
        RevokeDragDrop(hwnd);
        SetWindowLongPtrW(hwnd, -21, 0);
        drop(Box::from_raw(state as *mut State));
        ole.then(|| OleUninitialize());
        PostQuitMessage(0);
    }
}

/// Opens the window and runs the app in it, and in the second windows it describes, until the
/// window is closed.
pub fn run(window: Window, app: impl FnMut(&mut Ui) + 'static) {
    // SAFETY: the message loop every window's messages come through.
    unsafe {
        let (filling, minimized) = (window.maximized, window.minimized);
        let hwnd = open(window, Box::new(app));
        // Showing the window (normal 1, or maximized 3) sizes it, which runs the interface for
        // the first time. One starting minimized runs it too, knowing its size.
        match minimized {
            true if hwnd != 0 => show_minimized(hwnd, filling),
            _ => _ = ShowWindow(hwnd, if filling { 3 } else { 1 }),
        }
        let mut msg = MSG::default();
        while hwnd != 0 && GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if let Some(answer) = DIALOG.get() {
                answer();
            }
            run_queued_drag_out();
        }
    }
}

/// A Windows icon from one of the program's icon pictures: the smallest at least `side` pixels
/// wide, else the widest. (0 where there is none.)
fn icon_of(icons: &[(usize, &[u32])], side: i32) -> HANDLE {
    let fits = icons.iter().filter(|(width, _)| *width as i32 >= side).min_by_key(|(width, _)| *width);
    let Some((width, pixels)) = fits.or(icons.iter().max_by_key(|(width, _)| *width)) else { return 0 };
    icon_made(*width, pixels)
}

/// A Windows icon from a picture `width` pixels wide (pixels as [`Ui::add_picture`] takes
/// them). (0 where Windows makes none.)
#[inline]
pub(crate) fn icon_made(width: usize, pixels: &[u32]) -> HANDLE {
    let (wide, high) = (width as i32, (pixels.len() / width.max(1)) as i32);
    // Visibility comes from the alpha channel: the icon's 1-bit mask hides nothing. Its rows
    // are whole 16-bit words.
    let nothing_hidden = vec![0u8; (wide as usize).div_ceil(16) * 2 * high as usize];
    // SAFETY: Windows copies the pixels (BGRA, as they lie here) and the mask bits into its own
    // bitmaps, makes the icon from both, and both are freed: the icon has its own copies.
    unsafe {
        let (color, mask) = (CreateBitmap(wide, high, 1, 32, pixels.as_ptr().cast()), CreateBitmap(wide, high, 1, 1, nothing_hidden.as_ptr()));
        let icon = CreateIconIndirect(&ICONINFO { icon: 1, x: 0, y: 0, mask, color });
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

/// Creates the first window, hidden, with everything to run the app in it. (0 if Windows
/// creates no window.)
pub(crate) fn open(window: Window, app: Box<dyn FnMut(&mut Ui)>) -> HANDLE {
    // SAFETY: the window class of this library is registered and the first window created.
    // Every pointer outlives its call.
    unsafe {
        SetProcessDpiAwarenessContext(-4); // sharp on every screen, each at its own scale (`DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`)
        let instance = GetModuleHandleW(std::ptr::null());
        RegisterClassW(&WNDCLASSW {
            style: 0,
            proc: Some(window_proc),
            class_extra: 0,
            // One pointer-sized extra value per window: whether it is frameless.
            window_extra: size_of::<isize>() as i32,
            instance,
            icon: LoadIconW(instance, std::ptr::without_provenance(1)),
            cursor: 0,
            background: 0,
            menu: std::ptr::null(),
            name: wide(CLASS).as_ptr(),
        });
        // Where it goes: where its app says (on the screen with that place, else moved onto the
        // nearest), or centred on the screen it names; else where Windows puts it. At that
        // screen's scale.
        let (found, size) = (screens(), Size { w: window.width, h: window.height });
        let opening =
            WindowOpening { min_size: Size { w: window.min_width, h: window.min_height }, position: window.position, screen: window.screen, kind: WindowKind::Plain, frameless: window.frameless };
        let asked = window.position.map(|(x, y)| OpenAt::At(x, y)).or(window.screen.map(OpenAt::Middle)).unwrap_or_default();
        let looks = (!window.frameless && !window.transparent).then(|| styles(WindowKind::Plain, false, false));
        let Some(first) = Open::make(window.title, size, opening, window.transparent, place_on(&found, asked, size, looks), 0, 0) else { return 0 };
        let hwnd = first.hwnd;
        // The program's icons, if any, are every window's: the one fitting the large size (-14,
        // `GCLP_HICON`: the Alt+Tab list), and the small (-34, `GCLP_HICONSM`: window frame and
        // task bar). `SM_CXICON` (11) and `SM_CXSMICON` (49) give their sizes.
        if !window.icons.is_empty() {
            SetClassLongPtrW(hwnd, -14, icon_of(window.icons, GetSystemMetrics(11)));
            SetClassLongPtrW(hwnd, -34, icon_of(window.icons, GetSystemMetrics(49)));
        }
        let strip = Rc::new(Strip { dc: CreateCompatibleDC(0), bitmap: Cell::new(0), pixels: Cell::new(std::ptr::null_mut()), width: Cell::new(0), rows: Cell::new(0) });
        // GDI writes letters straight into the strip with no alpha: a see-through window draws
        // all its text as a surface does, which keeps alpha (where Windows has a DC to give
        // it).
        let clear = if window.transparent { SystemText::new() } else { None };
        let text: Box<dyn TextSystem> = match clear {
            Some(text) => Box::new(text),
            None => Box::new(GdiText { strip: strip.clone(), fonts: Fonts::new(strip.dc) }),
        };
        let mut ui = Ui::new(text, Box::new(SystemClipboard));
        // A see-through window opens on a page of no colour: what is behind shows wherever its
        // app draws nothing.
        if window.transparent {
            ui.theme.bg = Color::NONE;
        }
        // `run`'s loop shows the system file dialog: a window run by it can ask for one.
        ui.set_native_file_dialog(true);
        ui.set_preferences(crate::preferences::of_windows());
        ui.set_screens(found);
        #[rustfmt::skip]
        let mut state = Box::new(State { ui, app, strip, windows: vec![first], start: Instant::now(), timer: false, ole: None, #[cfg(feature = "gpu")] device: None, #[cfg(feature = "gpu")] coming: None });
        // The interface knows its window's size before the window shows: one starting minimized
        // has no size to tell until restored.
        state.fit(0);
        // Shown by the GPU if asked for, once a device works.
        #[cfg(feature = "gpu")]
        state.show_by_gpu(window.renderer, window.transparent, !window.minimized);
        SetWindowLongPtrW(hwnd, -21, Box::into_raw(state) as isize);
        WINDOW.store(hwnd, std::sync::atomic::Ordering::Relaxed);
        hwnd
    }
}

/// The local date and time: year, month, day, hour, minute, second.
pub(crate) fn local_time() -> [u16; 6] {
    let mut time = [0u16; 8];
    // SAFETY: the system fills in the `SYSTEMTIME` given.
    unsafe { GetLocalTime(&mut time) };
    [time[0], time[1], time[3], time[4], time[5], time[6]]
}

/// The computer's drive letters: one bit each, bit 0 for `A:`.
pub(crate) fn drives() -> u32 {
    // SAFETY: a query that takes nothing.
    unsafe { GetLogicalDrives() }
}

/// Does `work` with the system's critical-error box suppressed (as for an empty card reader):
/// errors go to the program alone. For this thread only, and only meanwhile.
pub(crate) fn quietly<T>(work: impl FnOnce() -> T) -> T {
    let mut was = 0;
    // SAFETY: sets this thread's error mode (1: `SEM_FAILCRITICALERRORS`) and restores it.
    unsafe { SetThreadErrorMode(1, &mut was) };
    let done = work();
    unsafe { SetThreadErrorMode(was, std::ptr::null_mut()) };
    done
}

/// A Windows COM object this library uses: a pointer to its vtable pointer.
#[repr(C)]
pub(crate) struct Com {
    table: *const usize,
}

impl Com {
    /// Function `at` of the object's vtable, as type `F`.
    ///
    /// # Safety
    /// The object is alive, and its function at that index has that type.
    pub(crate) unsafe fn function<F: Copy>(this: *mut Com, at: usize) -> F {
        // SAFETY: as the caller promises: the vtable reaches that far, and a function pointer
        // is one pointer, as `F` is.
        unsafe { std::mem::transmute_copy(&*(*this).table.add(at)) }
    }
    /// Releases the object (vtable slot 2 of every COM object).
    pub(crate) unsafe fn release(this: *mut Com) {
        // SAFETY: the caller holds a reference, and no longer after this.
        unsafe { Com::function::<unsafe extern "system" fn(*mut Com) -> u32>(this, 2)(this) };
    }
}

/// A GUID as four numbers.
pub(crate) const fn named(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u32; 4] {
    [a, b as u32 | (c as u32) << 16, u32::from_le_bytes([d[0], d[1], d[2], d[3]]), u32::from_le_bytes([d[4], d[5], d[6], d[7]])]
}

thread_local! {
    /// The windows whose apps asked for the system file dialog, with each request; and the
    /// answer on its way to the window that asked.
    static ASKED: RefCell<Vec<(HANDLE, FileDialogRequest)>> = const { RefCell::new(Vec::new()) };
    static ANSWER: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// What a window just dragged out of itself, until handed to Windows.
    static OUTGOING: Cell<Option<(HANDLE, *mut crate::ole_drag::DataObject, crate::ole_drag::Outward)>> = const { Cell::new(None) };
    /// How a drag out is handed to Windows: set once an app first hands something out
    /// (`enable_drag_out`), so a program that never does is built without it.
    static OUTWARD: Cell<Option<crate::ole_drag::Outward>> = const { Cell::new(None) };
    /// How windows whose apps asked for the system file dialog are shown it: set once an app
    /// first asks (`hand_file_dialog`), so a program that never asks is built without it.
    static DIALOG: Cell<Option<fn()>> = const { Cell::new(None) };
}

/// Hands this thread's windows their system file dialog code: the first time an app asks for
/// it.
pub(crate) fn hand_file_dialog() {
    DIALOG.set(Some(answer_asked));
}

/// Shows the system file dialog for each window whose app asked and tells the window the
/// answer. Done between two messages, when no window is in use: the dialog runs the message
/// loop itself while up, and messages reach the windows as usual.
fn answer_asked() {
    while let Some((hwnd, ask)) = ASKED.with_borrow_mut(|asked| asked.pop()) {
        ANSWER.set(pick_file(hwnd, &ask));
        // SAFETY: a message to a window of this thread; a gone one gets none.
        unsafe { SendMessageW(hwnd, WM_ANSWERED, 0, 0) };
    }
}

/// The system dialog for a file to open, a folder, or a place and name to save under, modal
/// over window `owner`. Gives back the path, or nothing when the person cancelled (or Windows
/// has no such dialog).
///
/// The dialog is a COM object, as is each shell item it is given or returns. Called here by
/// vtable index: `Show` (3), `SetFileTypes` (4), `SetOptions`/`GetOptions` (9, 10), `SetFolder`
/// (12), `SetFileName` (15), `SetTitle` (17), `GetResult` (20), `SetDefaultExtension` (22). On
/// a shell item: `GetDisplayName` (5).
fn pick_file(owner: HANDLE, ask: &FileDialogRequest) -> Option<PathBuf> {
    const TO_OPEN: [u32; 4] = named(0xDC1C_5A9C, 0xE88A, 0x4DDE, [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7]);
    const TO_SAVE: [u32; 4] = named(0xC0B4_E2F3, 0xBA21, 0x4773, [0x8D, 0xBA, 0x33, 0x5E, 0xC9, 0x46, 0xEB, 0x8B]);
    const A_DIALOG: [u32; 4] = named(0x42F8_5136, 0xDB7E, 0x439C, [0x85, 0xF1, 0xE4, 0x07, 0x5D, 0x13, 0x5F, 0xC8]);
    type Says = unsafe extern "system" fn(*mut Com, *const u16) -> i32;
    // The file types as the dialog wants them: "*.png;*.jpg", also how it shows them. And the
    // default extension for names typed without one: the first of them.
    let kinds = wide(&ask.endings.iter().map(|ending| format!("*{ending}")).collect::<Vec<_>>().join(";"));
    let (kind, ending) = ([kinds.as_ptr(), kinds.as_ptr()], wide(ask.endings.first().map_or("", |ending| ending.trim_start_matches('.'))));
    let (title, name, folder, mut dialog, mut options) = (wide(ask.title), wide(&ask.name), wide(&ask.folder.to_string_lossy()), std::ptr::null_mut(), 0u32);
    // SAFETY: COM is initialized on this thread (apartment-threaded, 2) and uninitialized at
    // the end if it was; the dialog is created, configured, shown, asked for its result, and
    // released. Every string ends in a zero and outlives its call.
    unsafe {
        let com = CoInitializeEx(std::ptr::null_mut(), 2);
        if CoCreateInstance(if ask.saving { &TO_SAVE } else { &TO_OPEN }, std::ptr::null_mut(), 1, &A_DIALOG, &mut dialog) >= 0 && !dialog.is_null() {
            // Only file-system items (`FOS_FORCEFILESYSTEM`, 0x40); and where a folder is
            // asked, folders (`FOS_PICKFOLDERS`, 0x20).
            Com::function::<unsafe extern "system" fn(*mut Com, *mut u32) -> i32>(dialog, 10)(dialog, &mut options);
            Com::function::<unsafe extern "system" fn(*mut Com, u32) -> i32>(dialog, 9)(dialog, options | 0x40 | if ask.folders { 0x20 } else { 0 });
            Com::function::<Says>(dialog, 17)(dialog, title.as_ptr());
            Com::function::<Says>(dialog, 15)(dialog, name.as_ptr());
            start_in(dialog, &folder);
            if !ask.endings.is_empty() {
                Com::function::<unsafe extern "system" fn(*mut Com, u32, *const [*const u16; 2]) -> i32>(dialog, 4)(dialog, 1, &kind);
                Com::function::<Says>(dialog, 22)(dialog, ending.as_ptr());
            }
        }
        let picked = if dialog.is_null() { None } else { chosen_path(dialog, owner) };
        if com >= 0 {
            CoUninitialize();
        }
        picked
    }
}

/// Sets a file dialog's start folder, if Windows resolves the path.
///
/// # Safety
/// `dialog` is a live file dialog, and `folder` ends in a zero.
unsafe fn start_in(dialog: *mut Com, folder: &[u16]) {
    const AN_ITEM: [u32; 4] = named(0x4382_6D1E, 0xE718, 0x42EE, [0xBC, 0x55, 0xA1, 0xE2, 0x61, 0xC3, 0x7B, 0xFE]);
    let mut item = std::ptr::null_mut();
    // SAFETY: as the caller promises. Windows creates a shell item for the folder, given to the
    // dialog (`SetFolder`, 12) and released.
    unsafe {
        if SHCreateItemFromParsingName(folder.as_ptr(), std::ptr::null_mut(), &AN_ITEM, &mut item) >= 0 && !item.is_null() {
            Com::function::<unsafe extern "system" fn(*mut Com, *mut Com) -> i32>(dialog, 12)(dialog, item);
            Com::release(item);
        }
    }
}

/// Shows a file dialog over `owner`, releases it, and gives back what was picked: nothing when
/// cancelled.
///
/// # Safety
/// `dialog` is a live file dialog the caller holds one reference to.
unsafe fn chosen_path(dialog: *mut Com, owner: HANDLE) -> Option<PathBuf> {
    let (mut item, mut name): (*mut Com, *mut u16) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: as the caller promises. The result is a shell item, whose file-system path
    // (`SIGDN_FILESYSPATH`, 0x8005_8000) comes in memory this function frees: the name ends at
    // its first zero.
    unsafe {
        let shown = Com::function::<unsafe extern "system" fn(*mut Com, HANDLE) -> i32>(dialog, 3)(dialog, owner);
        if shown == 0 {
            Com::function::<unsafe extern "system" fn(*mut Com, *mut *mut Com) -> i32>(dialog, 20)(dialog, &mut item);
        }
        Com::release(dialog);
        if item.is_null() {
            return None;
        }
        Com::function::<unsafe extern "system" fn(*mut Com, u32, *mut *mut u16) -> i32>(item, 5)(item, 0x8005_8000, &mut name);
        Com::release(item);
        if name.is_null() {
            return None;
        }
        let length = (0..).take_while(|at| *name.add(*at) != 0).count();
        let file = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(name, length)));
        CoTaskMemFree(name.cast());
        Some(file)
    }
}

/// The Windows clipboard: text, pictures and files. Content is there in numbered formats:
/// `CF_UNICODETEXT` (13), `CF_DIB` (8), `CF_HDROP` (15); and pictures also as PNG, a registered
/// format programs agreed on.
#[derive(Debug)]
pub struct SystemClipboard;

/// The registered "PNG" format: it keeps alpha, which `CF_DIB` does not reliably.
fn as_png() -> u32 {
    // SAFETY: the name ends in a zero.
    unsafe { RegisterClipboardFormatW(wide("PNG").as_ptr()) }
}

/// The clipboard's content in one format, as bytes. (Nothing where it holds none, or another
/// program has it open now.)
fn taken(form: u32) -> Option<Vec<u8>> {
    // SAFETY: the clipboard is opened, the memory it returns is locked while copied, as far as
    // Windows says it reaches, and the clipboard is closed.
    unsafe {
        if OpenClipboard(0) == 0 {
            return None;
        }
        let mem = GetClipboardData(form);
        let start = if mem == 0 { std::ptr::null() } else { GlobalLock(mem).cast::<u8>().cast_const() };
        let bytes = (!start.is_null()).then(|| std::slice::from_raw_parts(start, GlobalSize(mem)).to_vec());
        GlobalUnlock(mem);
        CloseClipboard();
        bytes
    }
}

/// Puts blocks of bytes on the clipboard, one per format, replacing everything.
fn put(blocks: &[(u32, Vec<u8>)]) {
    // SAFETY: the clipboard is opened and emptied, and given memory it owns from then on
    // (memory that could not be allocated is no handle and is not given: that would be a
    // delayed-render promise).
    unsafe {
        if OpenClipboard(0) == 0 {
            return;
        }
        EmptyClipboard();
        // Memory the clipboard takes is its own; memory it does not take is still the program's
        // and is freed.
        let blocks = blocks.iter().map(|(form, block)| (*form, in_memory(block))).filter(|(_, mem)| *mem != 0);
        blocks.filter(|(form, mem)| SetClipboardData(*form, *mem) == 0).for_each(|(_, mem)| _ = GlobalFree(mem));
        CloseClipboard();
    }
}

/// A block of bytes in movable global memory (`HGLOBAL`), as Windows passes between programs.
/// (0 where there is no memory for it.)
pub(crate) fn in_memory(block: &[u8]) -> HANDLE {
    // SAFETY: the memory is as long as the copied block, and locked while copying.
    unsafe {
        let mem = GlobalAlloc(2, block.len());
        let to = GlobalLock(mem).cast::<u8>();
        if !to.is_null() {
            std::ptr::copy_nonoverlapping(block.as_ptr(), to, block.len());
        }
        GlobalUnlock(mem);
        mem
    }
}

/// A picture as a `CF_DIB` every Windows program reads: a `BITMAPINFOHEADER` (32-bit BGRA),
/// then rows bottom-up.
pub(crate) fn to_block(width: usize, pixels: &[u32]) -> Vec<u8> {
    let rows = pixels.chunks_exact(width.max(1));
    let header = [40, width as i32, rows.len() as i32, 1 | 32 << 16, 0, (pixels.len() * 4) as i32, 0, 0, 0, 0];
    header.iter().flat_map(|number| number.to_le_bytes()).chain(rows.rev().flatten().flat_map(|pixel| pixel.to_le_bytes())).collect()
}

/// Reads a DIB: width, and pixels top first. It may be 24 or 32 bits per pixel, bottom-up or
/// (negative height) top-down, rows padded to four bytes. A 32-bit DIB with `BI_BITFIELDS`
/// masks must have the usual ones. (Nothing for any other kind, or one cut short.)
fn from_block(block: &[u8]) -> Option<(usize, Vec<u32>)> {
    let number = |at: usize| block.get(at..).and_then(|rest| rest.first_chunk()).map(|four| i32::from_le_bytes(*four));
    let (size, width, height, depth, packing) = (number(0)? as usize, number(4)?, number(8)?, number(12)? >> 16, number(16)?);
    let usual = [0x00ff_0000, 0x0000_ff00, 0x0000_00ff].iter().enumerate().all(|(at, bits)| number(size.saturating_add(at * 4)) == Some(*bits));
    let start = match (depth, packing) {
        (24 | 32, 0) => size,
        (32, 3) if usual => size + 12,
        _ => return None,
    };
    let (wide, step) = (usize::try_from(width).ok().filter(|wide| *wide > 0)?, depth as usize / 8);
    let stride = wide.checked_mul(step)?.div_ceil(4) * 4;
    let mut rows: Vec<&[u8]> = block.get(start..)?.chunks_exact(stride).take(height.unsigned_abs() as usize).collect();
    if rows.len() < height.unsigned_abs() as usize || rows.is_empty() {
        return None;
    }
    if height > 0 {
        rows.reverse();
    }
    let pixel = |bytes: &[u8]| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], if step == 4 { bytes[3] } else { 255 }]);
    let mut pixels: Vec<u32> = rows.iter().flat_map(|row| row.chunks_exact(step).take(wide).map(pixel)).collect();
    // Many programs leave the alpha byte at zero in every pixel, meaning nothing: such a
    // picture is opaque, not invisible.
    if pixels.iter().all(|pixel| pixel >> 24 == 0) {
        pixels.iter_mut().for_each(|pixel| *pixel |= 0xff00_0000);
    }
    Some((wide, pixels))
}

/// Files as the `CF_HDROP` block file managers put on the clipboard and read: a `DROPFILES`
/// (see `read_drop`), each name in UTF-16 with a zero after it, and a zero after them all.
pub(crate) fn to_drop(files: &[PathBuf]) -> Vec<u8> {
    let names = files.iter().flat_map(|file| file.as_os_str().encode_wide().chain([0])).chain([0]);
    [20, 0, 0, 0, 1].iter().flat_map(|number: &i32| number.to_le_bytes()).chain(names.flat_map(u16::to_le_bytes)).collect()
}

impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        // The text ends at its first zero, at most where its memory ends.
        let units: Vec<u16> = taken(13)?.as_chunks().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        Some(String::from_utf16_lossy(units.split(|unit| *unit == 0).next().unwrap_or_default()))
    }
    fn set(&mut self, text: &str) {
        put(&[(13, wide(text).into_iter().flat_map(u16::to_le_bytes).collect())]);
    }
    /// Pictures and files, once an interface asked for one: see `PicturesAndFiles`.
    fn picture(&mut self) -> Option<(usize, Vec<u32>)> {
        (PICTURES_AND_FILES.get()?.picture)(self)
    }
    fn set_picture(&mut self, width: usize, pixels: &[u32]) {
        PICTURES_AND_FILES.get().map_or((), |its| (its.set_picture)(self, width, pixels))
    }
    fn files(&mut self) -> Vec<PathBuf> {
        PICTURES_AND_FILES.get().map_or(Vec::new(), |its| (its.files)(self))
    }
    fn set_files(&mut self, files: &[PathBuf]) {
        PICTURES_AND_FILES.get().map_or((), |its| (its.set_files)(self, files))
    }
}

/// How a window takes drops of files and text: how it starts accepting them, and what a
/// `WM_DROPFILES` says. Set by the first call asking about drops ([`Ui::files_hovered`],
/// [`Ui::files_dropped`], [`Ui::text_hovered`], [`Ui::text_dropped`], through
/// `platform::hand_drops`); reached no other way, so a program that never asks is built without
/// it.
#[derive(Clone, Copy)]
struct Drops {
    take: fn(&mut Open, bool),
    dropped: unsafe fn(HANDLE, f32) -> [Option<Event>; 2],
}

/// Hands the windows their drop handling, on this thread (see `Drops`).
pub(crate) fn hand_drops() {
    DROPS.set(Some(Drops { take: Open::take_files, dropped }));
}

thread_local! {
    /// The windows' drop handling: none until an interface on this thread asks about drops (see
    /// `Drops`).
    static DROPS: Cell<Option<Drops>> = const { Cell::new(None) };
    /// The clipboard's picture and file code: none until an interface on this thread asks for
    /// one (see `PicturesAndFiles`).
    static PICTURES_AND_FILES: Cell<Option<PicturesAndFiles<SystemClipboard>>> = const { Cell::new(None) };
}

/// What the windows do for an app's desktop requests, all in `windows_shell`: after a run, pass
/// on what the app said (and learn what the first window should do); and when the first window
/// goes, remove the program's tray icons. Set by the first call asking for a tray icon, system
/// notification, global hotkey, hidden window or single instance (through
/// `platform::hand_desktop`); reached no other way, so a program that never asks is built
/// without it.
#[derive(Clone, Copy)]
struct Desktop {
    after: fn() -> crate::windows_shell::Asked,
    ended: fn(),
}

thread_local! {
    /// What the windows do for the desktop: nothing until an app on this thread asks (see
    /// `Desktop`).
    static DESKTOP: Cell<Option<Desktop>> = const { Cell::new(None) };
}

/// Hands the windows their desktop integration, on this thread (see `Desktop`).
pub(crate) fn hand_desktop() {
    DESKTOP.set(Some(Desktop { after: crate::windows_shell::after, ended: crate::windows_shell::ended }));
}

/// Hands the clipboard its picture and file code, on this thread (see `PicturesAndFiles`).
pub(crate) fn hand_pictures_and_files() {
    let (picture, set_picture) = (SystemClipboard::picture_on, SystemClipboard::put_picture);
    PICTURES_AND_FILES.set(Some(PicturesAndFiles { picture, set_picture, files: SystemClipboard::files_on, set_files: SystemClipboard::put_files }));
}

impl SystemClipboard {
    /// A picture: as PNG where a program put one; else as a DIB, which Windows synthesizes from
    /// any picture format there.
    fn picture_on(&mut self) -> Option<(usize, Vec<u32>)> {
        let png = taken(as_png()).and_then(|png| crate::png::decode(&png));
        png.or_else(|| taken(8).and_then(|block| from_block(&block)))
    }
    /// Puts a picture on the clipboard as PNG and as a DIB.
    fn put_picture(&mut self, width: usize, pixels: &[u32]) {
        let Some(png) = crate::png::encode_rgba(width, pixels) else { return };
        put(&[(as_png(), png), (8, to_block(width, pixels))]);
    }
    /// The files on the clipboard: what was copied in a file manager.
    fn files_on(&mut self) -> Vec<PathBuf> {
        taken(15).map_or(Vec::new(), |block| read_drop(&block).1)
    }
    /// Puts files on the clipboard, for a file manager to paste.
    fn put_files(&mut self, files: &[PathBuf]) {
        put(&[(15, to_drop(files))]);
    }
}

/// The window, for threads that want it to run its interface again: none before it opens, and
/// none once it is gone, when its handle may become another window's.
static WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Asks the window to run its interface again; can be sent from any thread. (An app learning
/// something in the background, say a changed file, wakes the window with this.)
#[derive(Clone, Copy, Debug, Default)]
pub struct Waker;
impl Waker {
    /// Has the interface run once more, soon, on its own thread, from whatever thread calls
    /// this.
    pub fn wake(&self) {
        // SAFETY: posts to a window by handle. (To a null handle Windows would post to this
        // thread instead: then nothing is posted.)
        let hwnd = WINDOW.load(std::sync::atomic::Ordering::Relaxed);
        if hwnd != 0 {
            unsafe { PostMessageW(hwnd, WM_WAKE, 0, 0) };
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::geom::Point;
    use crate::{DragData, DropEffect, Icon, Length, SecondWindow};
    use std::cell::RefCell;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalFlags(mem: HANDLE) -> u32;
        fn GetCurrentProcess() -> HANDLE;
        fn GetCurrentThreadId() -> u32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetPropW(hwnd: HANDLE, name: *const u16) -> HANDLE;
        fn GetGuiResources(process: HANDLE, which: u32) -> u32;
        fn IsWindow(hwnd: HANDLE) -> i32;
        fn GetWindowTextW(hwnd: HANDLE, text: *mut u16, room: i32) -> i32;
        fn PeekMessageW(msg: *mut MSG, hwnd: HANDLE, min: u32, max: u32, how: u32) -> i32;
        fn SetKeyboardState(keys: *const [u8; 256]) -> i32;
        fn GetUpdateRect(hwnd: HANDLE, area: *mut RECT, erase: i32) -> i32;
        fn PrintWindow(hwnd: HANDLE, dc: HANDLE, how: u32) -> i32;
        fn EnumThreadWindows(thread: u32, each: unsafe extern "system" fn(HANDLE, isize) -> i32, with: isize) -> i32;
    }
    #[link(name = "imm32")]
    unsafe extern "system" {
        fn ImmGetCompositionWindow(context: HANDLE, form: *mut COMPOSITIONFORM) -> i32;
        fn ImmGetCandidateWindow(context: HANDLE, index: u32, form: *mut CANDIDATEFORM) -> i32;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ILCreateFromPathW(path: *const u16) -> *mut core::ffi::c_void;
        fn ILFindLastID(item: *const core::ffi::c_void) -> *mut core::ffi::c_void;
        fn ILFree(item: *mut core::ffi::c_void);
        fn SHCreateDataObject(folder: *mut core::ffi::c_void, count: u32, names: *const *mut core::ffi::c_void, inner: *mut core::ffi::c_void, id: *const [u32; 4], out: *mut *mut IDataObject) -> i32;
    }

    /// The effects Explorer allows for a drag out of it: copy, move, link.
    const ANY: u32 = 7;

    /// What the app in a test window saw on its last run.
    #[derive(Default)]
    struct Seen {
        over: bool,
        pointer: Option<Point>,
        got: Vec<PathBuf>,
        target: Rect,
        typed: Rect,
    }

    /// The state a window finds, shared by all its program's windows.
    fn state<'a>(hwnd: HANDLE) -> &'a mut State {
        unsafe { state_of(hwnd) }.unwrap().0
    }
    /// What is kept for a window alone.
    fn kept<'a>(hwnd: HANDLE) -> &'a mut Open {
        let (state, at) = unsafe { state_of(hwnd) }.unwrap();
        &mut state.windows[at]
    }

    /// A window as `run` creates it, with a drop box and a text box. Never shown: on no screen,
    /// in nobody's way. Also returns whether its drops come through OLE.
    fn window(seen: &Rc<RefCell<Seen>>) -> (HANDLE, bool) {
        let (seen, mut text) = (seen.clone(), String::new());
        let app = move |ui: &mut Ui| {
            let mut seen = seen.borrow_mut();
            ui.column().pad(10.0).gap(10.0).open();
            let target = ui.selectable(false, "Let files go here");
            let typed = ui.text_box(&mut text, "Type here");
            if let Some(files) = ui.files_dropped(&target) {
                seen.got = files;
            }
            (seen.over, seen.pointer, seen.target, seen.typed) = (ui.files_hovered(), ui.pointer(), target.rect, typed.rect);
            ui.close();
        };
        let hwnd = open(Window::default(), Box::new(app));
        assert!(hwnd != 0 && unsafe { IsWindowVisible(hwnd) } == 0);
        // Showing a window sizes it; this one is told its size without that, twice, so every
        // box knows where the first run put it. (The app asks about files in its first run,
        // from when the window accepts them.)
        unsafe { (window_proc(hwnd, WM_SIZE, 0, 0), window_proc(hwnd, WM_SIZE, 0, 0)) };
        (hwnd, state(hwnd).ole == Some(true))
    }
    /// Destroying the window uninitializes OLE, where it was initialized for it.
    fn close((hwnd, _): (HANDLE, bool)) {
        unsafe { DestroyWindow(hwnd) };
    }

    /// A window point on screen, as Windows tells a drop target.
    fn on_screen(hwnd: HANDLE, at: Point) -> POINT {
        let scale = kept(hwnd).scale;
        let mut place = POINT { x: (at.x * scale) as i32, y: (at.y * scale) as i32 };
        unsafe { ClientToScreen(hwnd, &mut place) };
        place
    }
    fn middle(of: Rect) -> Point {
        pt((of.x + of.w / 2.0).floor(), (of.y + of.h / 2.0).floor())
    }

    /// A `DROPFILES` block as Windows writes it: dropped at `at`, in the window or on its
    /// frame, names in UTF-16 or ANSI.
    fn block(at: (i32, i32), on_frame: bool, wide: bool, names: &[&str]) -> Vec<u8> {
        let mut block: Vec<u8> = [20, at.0, at.1, on_frame as i32, wide as i32].iter().flat_map(|number| number.to_le_bytes()).collect();
        for name in names {
            match wide {
                true => block.extend(name.encode_utf16().chain([0]).flat_map(u16::to_le_bytes)),
                false => block.extend(name.bytes().chain([0])),
            }
        }
        block.extend([0, 0]);
        block
    }
    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    /// A 2×3 picture: opaque, half, clear, and nearly each.
    const SIX: [u32; 6] = [0xff10_2030, 0x8040_5060, 0x0070_8090, 0xffa0_b0c0, 0x01d0_e0f0, 0xfe01_0203];

    #[test]
    fn a_picture_goes_into_the_block_of_pixels_windows_knows_and_back() {
        let block = to_block(2, &SIX);
        // A 40-byte header, then rows bottom-up, four bytes a pixel.
        assert_eq!((block.len(), &block[40..44]), (40 + 24, &0x01d0_e0f0_u32.to_le_bytes()[..]));
        assert_eq!(from_block(&block), Some((2, SIX.to_vec())));
        // As other programs write them: 24 bits per pixel, top-down (negative height), rows
        // padded to four bytes.
        let numbers = |numbers: [i32; 10]| numbers.iter().flat_map(|number| number.to_le_bytes()).collect::<Vec<u8>>();
        let three = [numbers([40, 1, -2, 1 | 24 << 16, 0, 0, 0, 0, 0, 0]), vec![0x30, 0x20, 0x10, 0, 0x60, 0x50, 0x40, 0]].concat();
        assert_eq!(from_block(&three), Some((1, vec![0xff10_2030, 0xff40_5060])));
        // 32 bits with alpha zero in every pixel: an opaque picture.
        let unsaid = [numbers([40, 2, 1, 1 | 32 << 16, 0, 0, 0, 0, 0, 0]), vec![0x30, 0x20, 0x10, 0, 0x60, 0x50, 0x40, 0]].concat();
        assert_eq!(from_block(&unsaid), Some((2, vec![0xff10_2030, 0xff40_5060])));
        // With `BI_BITFIELDS` masks: the usual ones are skipped over, others not misread.
        let said = |bits: [i32; 3]| [numbers([40, 1, 1, 1 | 32 << 16, 3, 0, 0, 0, 0, 0]), bits.iter().flat_map(|bits| bits.to_le_bytes()).collect(), vec![0x30, 0x20, 0x10, 0x80]].concat();
        assert_eq!((from_block(&said([0xff_0000, 0xff00, 0xff])), from_block(&said([0xff, 0xff00, 0xff_0000]))), (Some((1, vec![0x8010_2030])), None));
        // A block cut short, of an unread kind, or of no kind is no picture; none is read past
        // its end.
        assert_eq!((from_block(&block[..63]), from_block(&block[..39]), from_block(&[])), (None, None, None));
        assert_eq!(from_block(&numbers([40, 2, 3, 1 | 8 << 16, 0, 0, 0, 0, 0, 0])), None);
        for seed in 0..2000u32 {
            let noise: Vec<u8> = (0..seed % 97).map(|at| ((at + seed).wrapping_mul(2_654_435_761) >> 11) as u8).collect();
            from_block(&noise);
            from_block(&[&numbers([40, (seed % 5) as i32 - 1, (seed % 7) as i32 - 3, 1 | [24, 32][seed as usize % 2] << 16, [0, 3][seed as usize % 2], 0, 0, 0, 0, 0])[..], &noise].concat());
        }
    }

    #[test]
    fn files_go_into_the_block_a_file_manager_reads_and_back() {
        let files = paths(&["C:\\a\\one.txt", "D:\\zwei drei\\für.png"]);
        assert_eq!(read_drop(&to_drop(&files)).1, files);
        assert_eq!(read_drop(&to_drop(&[])).1, paths(&[]));
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn CreateWindowStationW(name: *const u16, flags: u32, access: u32, security: *const std::ffi::c_void) -> HANDLE;
        fn SetProcessWindowStation(station: HANDLE) -> i32;
        fn GetProcessWindowStation() -> HANDLE;
        fn CreateDesktopW(name: *const u16, device: *const u16, mode: *const std::ffi::c_void, flags: u32, access: u32, security: *const std::ffi::c_void) -> HANDLE;
        fn SetThreadDesktop(desktop: HANDLE) -> i32;
        fn GetThreadDesktop(thread: u32) -> HANDLE;
        fn GetUserObjectInformationW(object: HANDLE, what: i32, value: *mut u16, room: u32, needed: *mut u32) -> i32;
    }

    /// Tells a test it runs in a process of its own.
    pub(crate) const ALONE: &str = "TAUT_TEST_BY_ITSELF";

    /// Runs one test marked so in a process of its own (this test binary again, running only
    /// that test) and requires it to pass.
    fn by_itself(test: &str) {
        let ran = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", test, "--ignored", "--nocapture"]).env(ALONE, "1").output().unwrap();
        let said = String::from_utf8_lossy(&ran.stdout);
        assert!(ran.status.success() && said.contains("1 passed"), "{said}\n{}", String::from_utf8_lossy(&ran.stderr));
    }

    /// `STARTUPINFOW`, of which only the desktop is set.
    #[repr(C)]
    struct Start {
        size: u32,
        reserved: *const u16,
        desktop: *const u16,
        title: *const u16,
        numbers: [u32; 8],
        show: [u16; 2],
        more: *const u8,
        handles: [usize; 3],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateProcessW(program: *const u16, line: *mut u16, a: usize, b: usize, inherit: i32, how: u32, env: usize, folder: usize, start: *const Start, made: *mut [usize; 4]) -> i32;
        fn WaitForSingleObject(of: usize, millis: u32) -> u32;
        fn GetExitCodeProcess(process: usize, code: *mut u32) -> i32;
        fn TerminateProcess(process: usize, code: u32) -> i32;
        fn CloseHandle(handle: usize) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn CloseDesktop(desktop: HANDLE) -> i32;
        fn FindWindowW(class: *const u16, title: *const u16) -> HANDLE;
    }

    /// The name of the tests' private desktops: desktops nobody sees or types into. Windows
    /// shown there, and programs driving them, take nothing from whoever is at the computer:
    /// the foreground window and focus are per desktop.
    const DESKTOP: &str = "taut tests";

    /// Runs one test marked so in a process of its own started on such a desktop, made for it
    /// alone, as is every program the test starts in turn; and requires it to pass. (Through
    /// `cmd.exe`, with no console window (`CREATE_NO_WINDOW`, 0x0800_0000), capturing the
    /// test's output.)
    pub(crate) fn on_a_desktop_of_its_own(test: &str) {
        let (program, said) = (std::env::current_exe().unwrap(), std::env::temp_dir().join(format!("taut-{}-{test}.txt", std::process::id()).replace("::", "-")));
        let line = format!("cmd.exe /c \"set {ALONE}=1&& \"{}\" --exact {test} --ignored --nocapture > \"{}\" 2>&1\"", program.display(), said.display());
        // (Numbered in turn: two tests running at once never share one.)
        static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let name = format!("{DESKTOP} {} {}", std::process::id(), MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let (mut line, place, mut made) = (wide(&line), wide(&format!("WinSta0\\{name}")), [0usize; 4]);
        let start = Start {
            size: size_of::<Start>() as u32,
            reserved: std::ptr::null(),
            desktop: place.as_ptr(),
            title: std::ptr::null(),
            numbers: [0; 8],
            show: [0; 2],
            more: std::ptr::null(),
            handles: [0; 3],
        };
        let code = unsafe {
            let desktop = CreateDesktopW(wide(&name).as_ptr(), std::ptr::null(), std::ptr::null(), 0, 0x1000_0000, std::ptr::null());
            assert!(desktop != 0 && CreateProcessW(std::ptr::null(), line.as_mut_ptr(), 0, 0, 0, 0x0800_0000, 0, 0, &start, &mut made) != 0, "the test could not be started on a desktop of its own");
            // Two minutes is far more than it takes; whatever still runs then is ended.
            let (timed_out, mut code) = (WaitForSingleObject(made[0], 120_000) != 0, 1);
            TerminateProcess(made[0], 1);
            GetExitCodeProcess(made[0], &mut code);
            [made[0], made[1]].iter().for_each(|handle| _ = CloseHandle(*handle));
            CloseDesktop(desktop);
            if timed_out { 258 } else { code }
        };
        let said = std::fs::read_to_string(&said).unwrap_or_default();
        assert!(code == 0 && said.contains("1 passed"), "the test ended with {code}:\n{said}");
    }

    /// Whether this thread is on one of the tests' private desktops. (2: `UOI_NAME`.)
    pub(crate) fn is_on_its_own_desktop() -> bool {
        let mut name = [0u16; 64];
        unsafe { GetUserObjectInformationW(GetThreadDesktop(GetCurrentThreadId()), 2, name.as_mut_ptr(), 126, std::ptr::null_mut()) };
        String::from_utf16_lossy(name.split(|unit| *unit == 0).next().unwrap_or_default()).starts_with(DESKTOP)
    }

    #[test]
    fn the_clipboard_takes_text_pictures_and_files_and_gives_them_back() {
        // The clipboard is the user's, no place for a test. But Windows keeps one clipboard per
        // window station, and a process can create its own station: so the test below runs in a
        // process of its own that does that first.
        by_itself("windows::tests::on_a_clipboard_that_is_nobodys");
    }

    #[test]
    fn a_window_opens_filling_its_screen_and_goes_back_to_where_it_was() {
        // Shown for real, which takes focus on its desktop: so also on a private desktop.
        on_a_desktop_of_its_own("windows::tests::on_a_desktop_nobody_sees_a_window_fills_its_screen");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_a_window_fills_its_screen() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(is_on_its_own_desktop(), "this is the desktop the person works on: no window is shown on it");
        // As `run` opens a window whose app asks for it maximized (3).
        let asked = Window { position: Some((120, 90)), width: 400.0, height: 240.0, maximized: true, ..Window::default() };
        let hwnd = open(asked, Box::new(|ui: &mut Ui| _ = ui.label("One")));
        unsafe { ShowWindow(hwnd, 3) };
        pump();
        // It is maximized, and its interface is told so with the place and size it restores to:
        // those its app asked for.
        let (placed, filled) = (state(hwnd).ui.window_placement().expect("the window was told where it is"), state(hwnd).ui.size());
        assert_eq!(placed, WindowPlacement { position: (120, 90), width: 400.0, height: 240.0, maximized: true });
        assert!(filled.w > 400.0 && filled.h > 240.0, "{filled:?} is no more than it asked for");
        // Restored (9), it is where and as big as it should be, and says so.
        unsafe { ShowWindow(hwnd, 9) };
        pump();
        assert_eq!(
            (state(hwnd).ui.window_placement(), state(hwnd).ui.size(), corner_of(hwnd)),
            (Some(WindowPlacement { maximized: false, ..placed }), Size { w: 400.0, h: 240.0 }, POINT { x: 120, y: 90 })
        );
        unsafe { DestroyWindow(hwnd) };
    }

    /// Text measured and drawn at every size a pan-and-zoom view zooms through keeps at most
    /// `MADE` GDI fonts and `GLYPHS` glyph pictures; what is dropped comes back the same.
    #[test]
    fn fonts_and_letters_made_at_every_size_are_held_to_a_bound() {
        let mut text = SystemText::new().unwrap();
        let first = text.measure("Wide", Font::new(14.0), 1.0, None);
        (1..=600).for_each(|px| _ = text.measure("Wide", Font::new(px as f32), 1.0, None));
        assert!(text.fonts.made.len() <= MADE, "{} fonts kept", text.fonts.made.len());
        for (px, letter) in (4..=40).flat_map(|px| (0x41..0x16d).filter_map(char::from_u32).map(move |letter| (px, letter))) {
            text.glyph(Font::new(px as f32), 1.0, letter);
        }
        assert!(text.glyphs.len() <= GLYPHS, "{} pictures of letters kept", text.glyphs.len());
        assert_eq!(text.measure("Wide", Font::new(14.0), 1.0, None), first);
    }

    #[test]
    fn the_face_for_code_has_letters_that_are_all_as_wide() {
        // Whichever monospaced font is installed: a narrow and a wide letter take the same
        // room, unlike in the UI font.
        let mut text = SystemText::new().unwrap();
        let wide = |text: &mut SystemText, letters: &str, font: Font| text.measure(letters, font, 1.0, None);
        let (code, plain) = (Font::new(12.0).mono(), Font::new(12.0));
        assert_eq!(wide(&mut text, "iiiiiiii", code), wide(&mut text, "MMMMMMMM", code));
        assert!(wide(&mut text, "iiiiiiii", plain) < wide(&mut text, "MMMMMMMM", plain));
        // A missing font is not mistaken for an installed one.
        let dc = unsafe { CreateCompatibleDC(0) };
        let (none, there) = (font_named(dc, "No font is called this", 12, 400, false), font_named(dc, "Courier New", 12, 400, false));
        assert!(none.is_none() && there.is_some());
        unsafe { (DeleteDC(dc), DeleteObject(there.unwrap_or_default())) };
    }

    #[test]
    fn a_window_wears_the_frame_of_its_theme_and_knows_what_the_system_was_asked_for() {
        // Never shown: none of this needs it.
        let (light, runs) = (Rc::new(Cell::new(false)), Rc::new(Cell::new(0)));
        let (app, ran) = (light.clone(), runs.clone());
        let describe = move |ui: &mut Ui| {
            ran.set(ran.get() + 1);
            ui.theme = if app.get() { crate::Theme::light() } else { crate::Theme::dark() };
            ui.label("One");
        };
        let hwnd = open(Window::default(), Box::new(describe));
        // Its title bar is dark while its theme's background is.
        let dark = || state(hwnd).windows[0].dark;
        unsafe { window_proc(hwnd, WM_SIZE, 0, 0) };
        assert!(dark() && !state(hwnd).ui.window_minimized());
        light.set(true);
        unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
        assert!(!dark());
        light.set(false);
        unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
        assert!(dark());
        // What the person asked the system for is what the interface says, known before its
        // first run. A setting that changes none of it runs nothing.
        assert_eq!(state(hwnd).ui.preferences.current, crate::preferences::of_windows());
        let before = runs.get();
        for message in [WM_SETTINGCHANGE, WM_SYSCOLORCHANGE, WM_DWMCOLORIZATIONCOLORCHANGED] {
            unsafe { window_proc(hwnd, message, 0, 0) };
        }
        assert_eq!(runs.get(), before);
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn a_window_follows_the_person_when_they_change_how_things_are_to_look() {
        use crate::preferences::tests::PRETENDED;
        use crate::{Color, ContrastColors, Preferences, Theme};
        // Windows is made to report what the test wants, on this thread only: none of the
        // user's settings is read or changed, and the window is never shown.
        let runs = Rc::new(Cell::new(0));
        let ran = runs.clone();
        let describe = move |ui: &mut Ui| {
            ran.set(ran.get() + 1);
            ui.theme = ui.system_theme();
            ui.switch(&mut true, "On");
        };
        PRETENDED.set(Some(Preferences::default()));
        let hwnd = open(Window::default(), Box::new(describe));
        unsafe { window_proc(hwnd, WM_SIZE, 0, 0) };
        assert!(!state(hwnd).windows[0].dark && state(hwnd).ui.theme == Theme::light());
        let before = runs.get();
        // Each of the three messages by which Windows reports a setting change is heard: the
        // interface runs once and the app's theme follows. (Aquatic, a Windows contrast theme,
        // has a dark background: so the frame is dark.)
        let aquatic = ContrastColors {
            background: Color::rgb(0x202020),
            text: Color::WHITE,
            link: Color::rgb(0x75e9fc),
            disabled: Color::rgb(0xa6a6a6),
            selected: Color::rgb(0x8ee3f0),
            selected_text: Color::rgb(0x263b50),
            button: Color::rgb(0x202020),
            button_text: Color::WHITE,
        };
        let changes = [
            (WM_SETTINGCHANGE, Preferences { dark: true, text_scale: 1.5, ..Preferences::default() }),
            (WM_DWMCOLORIZATIONCOLORCHANGED, Preferences { dark: true, accent: Some(Color::rgb(0xe3008c)), ..Preferences::default() }),
            (WM_SYSCOLORCHANGE, Preferences { high_contrast: Some(aquatic), reduced_motion: true, ..Preferences::default() }),
        ];
        for (nth, (message, wishes)) in changes.into_iter().enumerate() {
            PRETENDED.set(Some(wishes));
            unsafe { window_proc(hwnd, message, 0, 0) };
            assert_eq!((runs.get(), state(hwnd).ui.preferences.current, state(hwnd).ui.theme), (before + nth + 1, wishes, Theme::following(&wishes)), "{message:x}");
            assert!(state(hwnd).windows[0].dark, "{message:x}");
        }
        // Nothing moves by itself now: the switch is at its end and asks for no run.
        assert!(state(hwnd).ui.reduced_motion() && state(hwnd).ui.wake_in().is_none());
        // A message saying nothing new runs nothing.
        unsafe { window_proc(hwnd, WM_SETTINGCHANGE, 0, 0) };
        assert_eq!(runs.get(), before + 3);
        PRETENDED.set(None);
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn a_window_opens_minimized_comes_forward_and_stays_over_the_others_when_its_app_asks() {
        // Shown for real and brought to the front: on a private desktop.
        on_a_desktop_of_its_own("windows::tests::on_a_desktop_nobody_sees_a_window_opens_minimized_and_comes_forward");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_a_window_opens_minimized_and_comes_forward() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(is_on_its_own_desktop(), "this is the desktop the person works in: no window is shown on it");
        let (forward, top, runs) = (Rc::new(Cell::new(false)), Rc::new(Cell::new(None::<bool>)), Rc::new(Cell::new(0)));
        let (app, asked, ran) = (forward.clone(), top.clone(), runs.clone());
        let describe = move |ui: &mut Ui| {
            ran.set(ran.get() + 1);
            if app.take() {
                ui.bring_to_front();
            }
            asked.take().into_iter().for_each(|on| ui.set_on_top(on));
            ui.label("One");
        };
        // As `run` opens a window whose app asks for it minimized, maximized when restored.
        let hwnd = open(Window { position: Some((120, 90)), width: 400.0, height: 240.0, maximized: true, minimized: true, ..Window::default() }, Box::new(describe));
        unsafe { show_minimized(hwnd, true) };
        pump();
        // It is minimized, and its interface knows, as it knows the window's size though
        // nothing shows.
        let ui = || &state(hwnd).ui;
        assert!(unsafe { IsIconic(hwnd) } != 0 && ui().window_minimized() && runs.get() > 0);
        assert_eq!(ui().size(), Size { w: 400.0, h: 240.0 });
        // Its app asks it to come forward: it is restored, maximized, in front, and its
        // interface hears all of that.
        forward.set(true);
        unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
        pump();
        // A desktop not on screen has no foreground window: that it has focus is what can be
        // checked here.
        assert!(unsafe { IsIconic(hwnd) } == 0 && !ui().window_minimized() && unsafe { GetFocus() } == hwnd && ui().window_focused());
        assert!(ui().window_placement().is_some_and(|placed| placed.maximized) && ui().size().w > 400.0);
        // Asked to stay above other programs' windows, it is topmost (style -20 bit 8,
        // `WS_EX_TOPMOST`) until asked not to be.
        let over = || unsafe { GetWindowLongPtrW(hwnd, -20) } & 8 != 0;
        assert!(!over());
        top.set(Some(true));
        unsafe { (window_proc(hwnd, WM_WAKE, 0, 0), window_proc(hwnd, WM_WAKE, 0, 0)) };
        assert!(over());
        top.set(Some(false));
        unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
        assert!(!over());
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn the_systems_own_file_dialog_answers_an_app_that_asks_for_it() {
        // The dialog is a real window, which comes to the front and takes focus: so also on a
        // private desktop.
        on_a_desktop_of_its_own("windows::tests::on_a_desktop_nobody_sees_the_systems_file_dialog_is_answered");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_the_systems_file_dialog_is_answered() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(is_on_its_own_desktop(), "this is the desktop the person works on: no dialog is shown on it");
        let folder = std::env::temp_dir().join(format!("taut-files-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        // The dialog answers with long names; a temp folder can have a short one (`RUNNER~1`).
        let folder = PathBuf::from(std::fs::canonicalize(&folder).unwrap().to_string_lossy().trim_start_matches(r"\\?\"));
        std::fs::write(folder.join("report.txt"), "x").unwrap();
        // The user: presses a button of the dialog with this title (1: `IDOK`, 2: `IDCANCEL`)
        // once it is up, and again while it stays up: it takes a moment to initialize and
        // ignores buttons until then. Should no dialog come, or this one never go, the test is
        // ended: it would wait forever.
        let person = |title: &'static str, button: usize| {
            std::thread::spawn(move || {
                let dialog = || unsafe { FindWindowW(wide("#32770").as_ptr(), wide(title).as_ptr()) };
                let mut seen = false;
                for _ in 0..300 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    match (dialog(), seen) {
                        (0, true) => return,
                        (0, false) => {}
                        (up, _) => seen = unsafe { PostMessageW(up, 0x0111, button, 0) } != 0 || seen,
                    }
                }
                std::process::exit(3);
            })
        };
        let ask = |saving: bool, name: &str, title: &'static str| FileDialogRequest { saving, folders: false, folder: folder.clone(), name: name.into(), endings: vec![".txt".into()], title };
        // An existing file is picked; a cancelled dialog answers nothing; and a name to save
        // under gets the app's default extension.
        let pressed = person("Pick one", 1);
        assert_eq!(pick_file(0, &ask(false, "report.txt", "Pick one")), Some(folder.join("report.txt")));
        pressed.join().unwrap();
        let pressed = person("Never mind", 2);
        assert_eq!(pick_file(0, &ask(false, "report.txt", "Never mind")), None);
        pressed.join().unwrap();
        let pressed = person("Keep it", 1);
        assert_eq!(pick_file(0, &ask(true, "new", "Keep it")), Some(folder.join("new.txt")));
        pressed.join().unwrap();
        // Asked for a folder, it answers with the one it shows.
        let pressed = person("In here", 1);
        assert_eq!(pick_file(0, &FileDialogRequest { folders: true, endings: Vec::new(), ..ask(false, "", "In here") }), Some(folder.clone()));
        pressed.join().unwrap();

        // An app asking for the system dialog: its window shows it between two messages, and
        // the app's own file dialog, which never showed, hands it the answer.
        let got = Rc::new(RefCell::new((crate::FileDialog::native(), None::<PathBuf>, 0)));
        let (app, start) = (got.clone(), folder.clone());
        let describe = move |ui: &mut Ui| {
            let mut app = app.borrow_mut();
            let app = &mut *app;
            if std::mem::replace(&mut app.2, 1) == 0 {
                app.0.only(&[".txt"]);
                app.0.pick(&start);
                app.0.name = "report.txt".into();
            }
            ui.label("Under the dialog");
            app.1 = ui.file_dialog(&mut app.0).or(app.1.take());
        };
        // Nothing reaches the dialog's code before an app asks for it: a program that never
        // does is built without it.
        assert!(DIALOG.get().is_none(), "the dialog was handed over before anything asked for it");
        let hwnd = open(Window::default(), Box::new(describe));
        unsafe { window_proc(hwnd, WM_SIZE, 0, 0) };
        assert!(got.borrow().0.shown && got.borrow().1.is_none() && state(hwnd).ui.visible_access_nodes().is_empty());
        let pressed = person("Open a file", 1);
        DIALOG.get().expect("the app's asking handed the dialog over")();
        pressed.join().unwrap();
        assert_eq!((got.borrow().1.clone(), got.borrow().0.shown, got.borrow().0.folder.clone()), (Some(folder.join("report.txt")), false, folder.clone()));
        unsafe { DestroyWindow(hwnd) };
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own: it would put a clipboard of its own under every other test's windows"]
    fn on_a_clipboard_that_is_nobodys() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        // A window station for this process and a desktop in it for this thread. (Windows keeps
        // one such station for all programs of a session that ask: not the interactive
        // `WinSta0`, whose clipboard is the user's.) Where that cannot be had, nothing goes on
        // any clipboard.
        unsafe {
            let (station, mut name) = (CreateWindowStationW(std::ptr::null(), 0, 0x037F, std::ptr::null()), [0u16; 64]);
            assert!(station != 0 && SetProcessWindowStation(station) != 0 && GetProcessWindowStation() == station, "no window station of its own");
            // 2: `UOI_NAME`.
            assert!(GetUserObjectInformationW(station, 2, name.as_mut_ptr(), 126, std::ptr::null_mut()) != 0, "a station without a name");
            let name = String::from_utf16_lossy(name.split(|unit| *unit == 0).next().unwrap_or_default());
            assert!(!name.is_empty() && !name.eq_ignore_ascii_case("WinSta0"), "this is the station the person works in");
            let desktop = CreateDesktopW(wide("taut").as_ptr(), std::ptr::null(), std::ptr::null(), 0, 0x1000_0000, std::ptr::null());
            assert!(desktop != 0 && SetThreadDesktop(desktop) != 0, "no desktop of its own");
        }
        let (mut clipboard, files) = (SystemClipboard, paths(&["C:\\a\\one.txt", "D:\\zwei drei\\für.png"]));
        // Emptied first and again at the end: an earlier run may have left something.
        put(&[]);
        // A clipboard alone holds only text until it gets its picture and file code, as an
        // interface does on its first request for one.
        clipboard.set_picture(2, &SIX);
        clipboard.set_files(&files);
        assert_eq!((clipboard.picture(), clipboard.files(), taken(8), taken(15)), (None, paths(&[]), None, None));
        hand_pictures_and_files();
        assert_eq!((clipboard.get(), clipboard.picture(), clipboard.files()), (None, None, paths(&[])));
        clipboard.set("words, and ünïcode");
        assert_eq!((clipboard.get().as_deref(), clipboard.picture(), clipboard.files()), (Some("words, and ünïcode"), None, paths(&[])));
        // A picture is there both ways: as PNG, with alpha, and as a DIB. It replaces the text.
        clipboard.set_picture(2, &SIX);
        assert_eq!((clipboard.picture(), taken(8).and_then(|block| from_block(&block)), clipboard.get()), (Some((2, SIX.to_vec())), Some((2, SIX.to_vec())), None));
        clipboard.set_files(&files);
        assert_eq!((clipboard.files(), clipboard.picture(), clipboard.get()), (files, None, None));
        put(&[]);
    }

    /// A hand-made data object being dragged: a reference-counted COM object with files, text,
    /// or neither. It wants back what it hands out, to free it itself.
    #[repr(C)]
    struct Dragged {
        object: IDataObject,
        held: Cell<u32>,
        files: HANDLE,
        text: HANDLE,
    }
    impl Dragged {
        fn new(table: &IDataObjectVtbl, files: HANDLE) -> Dragged {
            Dragged { object: IDataObject { table }, held: Cell::new(1), files, text: 0 }
        }
        fn of_text(table: &IDataObjectVtbl, text: &str) -> Dragged {
            let text = in_memory(&wide(text).into_iter().flat_map(u16::to_le_bytes).collect::<Vec<u8>>());
            Dragged { text, ..Dragged::new(table, 0) }
        }
        /// What it has in the format asked, if any: an `HGLOBAL` of all of it.
        fn holds(&self, asked: &FORMATETC) -> HANDLE {
            let held = match asked.format {
                15 => self.files,
                13 => self.text,
                _ => 0,
            };
            if (asked.aspect, asked.index, asked.medium & 1) == (1, -1, 1) { held } else { 0 }
        }
        fn table() -> IDataObjectVtbl {
            IDataObjectVtbl { unknown: [0, Self::hold as *const () as usize, Self::release as *const () as usize], get_data: Self::get, get_data_here: 0, query_get_data: Self::has }
        }
        unsafe extern "system" fn hold(this: *mut Dragged) -> u32 {
            let held = unsafe { &(*this).held };
            held.set(held.get() + 1);
            held.get()
        }
        unsafe extern "system" fn release(this: *mut Dragged) -> u32 {
            let held = unsafe { &(*this).held };
            held.set(held.get() - 1);
            held.get()
        }
        unsafe extern "system" fn has(this: *mut IDataObject, asked: *const FORMATETC) -> i32 {
            let (this, asked) = unsafe { (&*this.cast::<Dragged>(), &*asked) };
            if this.holds(asked) != 0 { 0 } else { 0x8004_0064_u32 as i32 }
        }
        unsafe extern "system" fn get(this: *mut IDataObject, asked: *const FORMATETC, handed: *mut STGMEDIUM) -> i32 {
            if unsafe { Self::has(this, asked) } != 0 {
                return 0x8004_0064_u32 as i32;
            }
            unsafe {
                Self::hold(this.cast());
                *handed = STGMEDIUM { medium: 1, memory: (*this.cast::<Dragged>()).holds(&*asked), owner: this.cast() };
            }
            0
        }
    }

    /// A drop target's vtable, found as Windows finds it: through the object's first pointer.
    fn table(target: *mut DropTarget) -> &'static DropTargetTable {
        unsafe { &**target.cast::<*const DropTargetTable>() }
    }

    #[test]
    fn dropped_files_are_read_from_their_block() {
        let names = ["C:\\one.txt", "D:\\späte Töne\\𝄞 zwei.txt"];
        assert_eq!(read_drop(&block((30, 40), false, true, &names)), (Some(POINT { x: 30, y: 40 }), paths(&names)));
        assert_eq!(read_drop(&block((30, 40), true, true, &names)), (None, paths(&names)), "let go on the frame");
        assert_eq!(read_drop(&block((1, 2), false, false, &["C:\\old\\8.3"])), (Some(POINT { x: 1, y: 2 }), paths(&["C:\\old\\8.3"])), "named in bytes");
        assert_eq!(read_drop(&block((0, 0), false, true, &[])).1, paths(&[]));
        // A block cut short, or lying about where its names start, names what is there and
        // nothing beyond.
        let whole = block((0, 0), false, true, &names);
        assert_eq!(read_drop(&whole[..whole.len() - 5]).1.len(), 2);
        assert_eq!(read_drop(&whole[..30]).1, paths(&["C:\\on"]));
        assert_eq!(read_drop(&whole[..12]), (None, paths(&[])));
        assert_eq!(read_drop(&[]), (None, paths(&[])));
        let mut lying = whole.clone();
        lying[..4].copy_from_slice(&4u32.to_le_bytes());
        assert_eq!(read_drop(&lying).1, paths(&[]), "names that begin inside the five numbers");
        lying[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read_drop(&lying).1, paths(&[]), "names that begin past the end");
    }

    #[test]
    fn alt_going_down_and_coming_up_is_news_and_so_is_the_menu_key() {
        let mut said = Vec::new();
        let mut messages = Messages::default();
        // Alt down, held so it repeats, the menu key, and Alt up.
        for (msg, w, l) in [(WM_SYSKEYDOWN, 0x12, 0), (WM_SYSKEYDOWN, 0x12, 1 << 30), (WM_KEYDOWN, 0x5D, 0), (WM_SYSKEYUP, 0x12, 0)] {
            // SAFETY: key messages carry nothing read as memory.
            unsafe { messages.events(msg, w, l, 1.0, |event| said.push(event)) };
        }
        // Other modifiers held right now are the system's to say: only Alt is checked.
        let alt: Vec<Option<bool>> = said.iter().map(|event| if let Event::ModifiersChanged(held) = event { Some(held.alt) } else { None }).collect();
        assert_eq!(alt, [Some(true), None, Some(false)]);
        assert!(matches!(said[1], Event::Key(Key::Menu, _)));
    }

    #[test]
    fn the_wheel_with_ctrl_zooms_and_keys_that_come_up_say_so() {
        let mut said = Vec::new();
        let mut messages = Messages::default();
        // A notch forward and two back with Ctrl; "=" down and up, and "-" on the number pad.
        let notches = |notches: i16| ((notches * 120) as u16 as usize) << 16 | MK_CONTROL;
        for (msg, w) in [(WM_MOUSEWHEEL, notches(1)), (WM_MOUSEWHEEL, notches(-2)), (WM_KEYUP, 0xBB), (WM_SYSKEYUP, 0x6D), (WM_KEYUP, 0x10)] {
            // SAFETY: wheel and key messages carry nothing read as memory.
            unsafe { messages.events(msg, w, 0, 1.0, |event| said.push(event)) };
        }
        let zooms: Vec<f32> = said.iter().filter_map(|event| if let Event::Zoom(by) = event { Some(*by) } else { None }).collect();
        assert!(zooms.len() == 2 && (zooms[0] - 1.25).abs() < 1e-5 && (zooms[1] - 0.64).abs() < 1e-5, "{zooms:?}");
        // Shift coming up is reported as the modifiers held now, not as a key. Other modifiers
        // are the system's to say.
        let ups: Vec<&Event> = said.iter().filter(|event| matches!(event, Event::KeyUp(_) | Event::ModifiersChanged(_))).collect();
        assert_eq!(ups[..2], [&Event::KeyUp(Key::Char('=')), &Event::KeyUp(Key::Char('-'))]);
        assert!(ups.len() == 3 && matches!(ups[2], Event::ModifiersChanged(Mods { shift: false, .. })), "{ups:?}");
    }

    #[test]
    fn a_message_of_dropped_files_becomes_a_move_and_the_files() {
        let names = ["C:\\one.txt", "C:\\two\\𝄞.txt"];
        let read = |handle: HANDLE| {
            let mut events = Vec::new();
            unsafe { Messages::default().events(WM_DROPFILES, handle as usize, 0, 2.0, |event| events.push(event)) };
            events
        };
        let drop = in_memory(&block((30, 40), false, true, &names));
        // Read once an app asked about files, which hands over the code (see `Drops`): until
        // then the message is nothing.
        assert_eq!(read(drop), []);
        hand_drops();
        let said = [Event::Move(pt(15.0, 20.0)), Event::FilesDropped(paths(&names))];
        assert_eq!(read(drop), said);
        assert_eq!(read(drop), said, "reading a drop does not use it up");
        let on_frame = in_memory(&block((30, 40), true, true, &names));
        assert_eq!(read(on_frame), []);
        // A null block says nothing.
        unsafe { (GlobalFree(drop), GlobalFree(on_frame)) };
        assert_eq!(read(0), []);
    }

    #[test]
    fn files_dragged_over_the_window_reach_the_interface() {
        let seen = Rc::new(RefCell::new(Seen::default()));
        let (names, mut effect) = (["C:\\one.txt", "D:\\späte Töne\\𝄞 zwei.txt"], ANY);
        let (hwnd, ole) = window(&seen);
        let (box_at, beside) = (middle(seen.borrow().target), pt(300.0, 400.0));
        let (data_table, files) = (Dragged::table(), in_memory(&block((0, 0), false, true, &names)));
        let (mut dragged, mut other) = (Dragged::new(&data_table, files), Dragged::new(&data_table, 0));
        let (data, no_files): (*mut IDataObject, *mut IDataObject) = (&raw mut dragged.object, &raw mut other.object);
        let target = DropTarget::new(hwnd);
        let calls = table(target);
        let saw = || (seen.borrow().over, seen.borrow().pointer);
        unsafe {
            // Dragged in beside the box, onto it, and out again.
            assert_eq!(((calls.enter)(target, data, 0, on_screen(hwnd, beside), &mut effect), effect), (0, COPY));
            assert_eq!(saw(), (true, Some(beside)));
            assert_eq!(((calls.over)(target, 0, on_screen(hwnd, box_at), &mut effect), effect), (0, COPY));
            assert_eq!(saw(), (true, Some(box_at)));
            assert_eq!((calls.leave)(target), 0);
            assert_eq!(saw(), (false, None), "the pointer left with the files");
            // Dragged in and dropped beside the box: nothing there asks for files, and the
            // source hears nothing was taken.
            effect = ANY;
            (calls.enter)(target, data, 0, on_screen(hwnd, beside), &mut effect);
            assert_eq!(((calls.let_go)(target, data, 0, on_screen(hwnd, beside), &mut effect), effect), (0, 0));
            assert_eq!((saw(), seen.borrow().got.len()), ((false, Some(beside)), 0));
            // Dragged in and dropped on the box: the app gets them.
            effect = ANY;
            (calls.enter)(target, data, 0, on_screen(hwnd, beside), &mut effect);
            (calls.over)(target, 0, on_screen(hwnd, box_at), &mut effect);
            assert_eq!(((calls.let_go)(target, data, 0, on_screen(hwnd, box_at), &mut effect), effect), (0, COPY));
            assert_eq!((saw(), &seen.borrow().got), ((false, Some(box_at)), &paths(&names)));
            assert_eq!(dragged.held.get(), 1, "what was handed over was given back, each time");

            // A drag with no files is none of the window's business.
            effect = ANY;
            assert_eq!(((calls.enter)(target, no_files, 0, on_screen(hwnd, beside), &mut effect), effect), (0, 0));
            assert_eq!(((calls.over)(target, 0, on_screen(hwnd, beside), &mut effect), effect), (0, 0));
            assert_eq!(((calls.leave)(target), saw()), (0, (false, Some(box_at))), "the interface heard nothing of it");
            // Nor files the source does not allow copying (2 is move only).
            effect = 2;
            assert_eq!(((calls.enter)(target, data, 0, on_screen(hwnd, beside), &mut effect), effect), (0, 0));
            assert_eq!(((calls.leave)(target), saw()), (0, (false, Some(box_at))));

            // It answers `QueryInterface`, and is freed when the last reference is released.
            let (mut same, drop_target, unknown, data_object) = (std::ptr::null_mut(), [0x122, 0, 0xC0, 0x4600_0000], [0, 0, 0xC0, 0x4600_0000], [0x10e, 0, 0xC0, 0x4600_0000]);
            assert_eq!(((calls.query)(target, &drop_target, &mut same), same, (*target).held.get()), (0, target, 2));
            assert_eq!(((calls.query)(target, &unknown, &mut same), same, (*target).held.get()), (0, target, 3));
            assert_eq!(((calls.query)(target, &data_object, &mut same), same.is_null(), (*target).held.get()), (0x8000_4002_u32 as i32, true, 3));
            assert_eq!(((calls.hold)(target), (calls.release)(target), (calls.release)(target), (calls.release)(target), (calls.release)(target)), (4, 3, 2, 1, 0));
            GlobalFree(files);
        }
        close((hwnd, ole));
    }

    #[test]
    fn the_shell_s_own_drag_is_understood() {
        // What Explorer hands over when files are dragged out of it, made by the shell as
        // Explorer makes it (a folder and names in it), for two real files: this program and
        // whatever comes first in its folder.
        let seen = Rc::new(RefCell::new(Seen::default()));
        let program = std::env::current_exe().unwrap();
        let beside = std::fs::read_dir(program.parent().unwrap()).unwrap().map(|entry| entry.unwrap().path()).find(|path| *path != program).unwrap();
        let files = vec![program.clone(), beside];
        let item = |path: &std::path::Path| unsafe { ILCreateFromPathW(wide(path.to_str().unwrap()).as_ptr()) };
        let (mut data, mut effect, data_object) = (std::ptr::null_mut(), ANY, [0x10e, 0, 0xC0, 0x4600_0000]);
        let (hwnd, ole) = window(&seen);
        unsafe {
            let (folder, items) = (item(program.parent().unwrap()), [item(&files[0]), item(&files[1])]);
            let names = [ILFindLastID(items[0]), ILFindLastID(items[1])];
            assert_eq!(SHCreateDataObject(folder, 2, names.as_ptr(), std::ptr::null_mut(), &data_object, &mut data), 0);
            let (target, at) = (DropTarget::new(hwnd), on_screen(hwnd, middle(seen.borrow().target)));
            assert_eq!(((table(target).enter)(target, data, 0, at, &mut effect), effect, seen.borrow().over), (0, COPY, true));
            assert_eq!(((table(target).let_go)(target, data, 0, at, &mut effect), effect, seen.borrow().over), (0, COPY, false));
            assert_eq!(seen.borrow().got, files);
            // The shell's object is released (its third function).
            let release: unsafe extern "system" fn(*mut IDataObject) -> u32 = std::mem::transmute((*(*data).table).unknown[2]);
            assert_eq!((release(data), (table(target).release)(target)), (0, 0));
            [folder, items[0], items[1]].into_iter().for_each(|item| ILFree(item));
        }
        close((hwnd, ole));
    }

    #[test]
    fn windows_holds_the_drop_target_until_the_window_is_destroyed() {
        let seen = Rc::new(RefCell::new(Seen::default()));
        let (hwnd, ole) = window(&seen);
        unsafe {
            assert_eq!((ole, GetWindowLongPtrW(hwnd, -20) & 16), (true, 0), "no need for files as a message");
            // OLE keeps a window's drop target on the window, as this property.
            let target = GetPropW(hwnd, wide("OleDropTargetInterface").as_ptr()) as *mut DropTarget;
            assert!(!target.is_null() && std::ptr::eq(table(target), &DROP_TARGET));
            assert_eq!(((*target).hwnd, (*target).held.get()), (hwnd, 1), "Windows alone holds it");
            // Windows takes no second drop target for the same window
            // (`DRAGDROP_E_ALREADYREGISTERED`, 0x80040101).
            let second = DropTarget::new(hwnd);
            assert_eq!((RegisterDragDrop(hwnd, second.cast()), (*second).held.get()), (0x8004_0101_u32 as i32, 1));
            assert_eq!((table(second).release)(second), 0);
            // With one more reference here, it outlives the window by that one and no more.
            assert_eq!((table(target).hold)(target), 2);
            DestroyWindow(hwnd);
            assert_eq!((table(target).release)(target), 0, "Windows let go when the window went");
        }
    }

    #[test]
    fn a_window_whose_app_never_asks_about_files_is_spared_taking_them() {
        let hwnd = open(Window::default(), Box::new(|ui: &mut Ui| _ = ui.button("Nothing to do with files")));
        unsafe {
            window_proc(hwnd, WM_SIZE, 0, 0);
            window_proc(hwnd, WM_SIZE, 0, 0);
            assert_eq!((state(hwnd).ole, kept(hwnd).files), (None, false));
            assert_eq!((GetPropW(hwnd, wide("OleDropTargetInterface").as_ptr()), GetWindowLongPtrW(hwnd, -20) & 16), (0, 0), "no drop target, and no files as a message");
            DestroyWindow(hwnd);
        }
    }

    #[test]
    fn where_ole_cannot_be_had_dropped_files_come_as_a_message() {
        let seen = Rc::new(RefCell::new(Seen::default()));
        let names = ["C:\\one.txt", "C:\\two.txt"];
        unsafe {
            // A program using the multithreaded COM apartment (0) cannot also have OLE.
            assert_eq!(CoInitializeEx(std::ptr::null_mut(), 0), 0);
            let (hwnd, ole) = window(&seen);
            assert_eq!((ole, GetWindowLongPtrW(hwnd, -20) & 16), (false, 16), "no OLE: the window accepts files as a message");
            assert_eq!(GetPropW(hwnd, wide("OleDropTargetInterface").as_ptr()), 0, "and has no drop target");
            let (at, scale) = (middle(seen.borrow().target), kept(hwnd).scale);
            let drop = in_memory(&block(((at.x * scale) as i32, (at.y * scale) as i32), false, true, &names));
            let mut msg = MSG::default();
            PostMessageW(hwnd, WM_DROPFILES, drop as usize, 0);
            assert!(GetMessageW(&mut msg, hwnd, WM_DROPFILES, WM_DROPFILES) > 0);
            DispatchMessageW(&msg);
            assert_eq!((seen.borrow().pointer, &seen.borrow().got), (Some(at), &paths(&names)));
            assert_eq!(GlobalFlags(drop), 0x8000, "the memory the files came in was freed");
            close((hwnd, ole));
            CoUninitialize();
        }
    }

    #[test]
    fn alt_and_its_letters_are_the_windows_own_while_it_has_a_bar_of_menus() {
        keys_up();
        let opened = Rc::new(std::cell::Cell::new(false));
        let app = opened.clone();
        let describe = move |ui: &mut Ui| {
            ui.menu_bar();
            app.set(ui.menu_button("&File"));
            if app.get() {
                ui.menu_item("&Open");
                ui.close();
            }
            ui.close();
        };
        let hwnd = open(Window::default(), Box::new(describe));
        // Alt+F opens the menu, and the letter is not left to the system (which would beep,
        // having no system-menu entry with that letter).
        unsafe { (window_proc(hwnd, WM_SIZE, 0, 0), window_proc(hwnd, WM_SYSKEYDOWN, 0x12, 1 << 29), window_proc(hwnd, WM_SYSKEYDOWN, 'F' as usize, 1 << 29)) };
        assert!(opened.get());
        assert_eq!(unsafe { answer(hwnd, WM_SYSCHAR, 'f' as usize, 1 << 29) }, Some(0));
        // Nor is Alt alone (or F10), which would put focus in the window's system menu;
        // Alt+Space still opens that menu.
        assert_eq!(unsafe { (answer(hwnd, WM_SYSCOMMAND, SC_KEYMENU, 0), answer(hwnd, WM_SYSCHAR, ' ' as usize, 1 << 29)) }, (Some(0), None));
        assert!(unsafe { answer(hwnd, WM_SYSCOMMAND, SC_KEYMENU, ' ' as isize) }.is_none());
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn input_methods_are_kept_out_of_a_box_for_a_secret() {
        keys_up();
        // A password box and a name box under it. (The window is never shown.)
        let typed = Rc::new(RefCell::new((String::new(), String::new())));
        let app = typed.clone();
        let describe = move |ui: &mut Ui| {
            let mut app = app.borrow_mut();
            ui.password_box(&mut app.0, "Password");
            ui.text_box(&mut app.1, "Name");
        };
        let hwnd = open(Window::default(), Box::new(describe));
        // Whether the window has an input context, which input methods work on.
        let types_through = || unsafe {
            let context = ImmGetContext(hwnd);
            ImmReleaseContext(hwnd, context);
            context != 0
        };
        unsafe { (window_proc(hwnd, WM_SIZE, 0, 0), window_proc(hwnd, WM_SETFOCUS, 0, 0)) };
        assert!(types_through(), "this thread has nothing to type through to begin with");
        // Tab: focus on the password box, and the window has no input context. Typed characters
        // still arrive, one by one.
        unsafe { (window_proc(hwnd, WM_KEYDOWN, 9, 0), window_proc(hwnd, WM_CHAR, 'p' as usize, 0)) };
        assert!(state(hwnd).ui.password_focused() && !types_through() && typed.borrow().0 == "p");
        // Tab again: on to the name, where input methods are welcome again.
        unsafe { (window_proc(hwnd, WM_KEYDOWN, 9, 0), window_proc(hwnd, WM_CHAR, 'n' as usize, 0)) };
        assert!(!state(hwnd).ui.password_focused() && types_through() && *typed.borrow() == ("p".to_string(), "n".to_string()));
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn the_typing_window_follows_the_text_cursor() {
        keys_up();
        assert_eq!(cursor_pixels(rect(10.4, 20.0, 1.0, 17.0), 1.5), RECT { left: 16, top: 30, right: 17, bottom: 56 });
        let seen = Rc::new(RefCell::new(Seen::default()));
        let (hwnd, ole) = window(&seen);
        // What the system was told, read back from the window's input context.
        let nothing = COMPOSITIONFORM { style: 0, at: POINT::default(), area: RECT::default() };
        assert_eq!(unsafe { (ImmSetCompositionWindow(0, &nothing), ImmReleaseContext(hwnd, 0)) }, (0, 1), "with nothing to type through, telling fails and no more");
        let told = || unsafe {
            let (mut text, mut list) = (COMPOSITIONFORM { style: 0, at: POINT::default(), area: RECT::default() }, CANDIDATEFORM { index: 0, style: 0, at: POINT::default(), area: RECT::default() });
            let context = ImmGetContext(hwnd);
            assert!(context != 0, "this thread has no input context to read back from");
            ImmGetCompositionWindow(context, &mut text);
            let list_told = ImmGetCandidateWindow(context, 0, &mut list) != 0;
            ImmReleaseContext(hwnd, context);
            (text.style, (text.at.x, text.at.y), list_told.then_some((list.style, (list.at.x, list.at.y), list.area)))
        };
        let cursor = || state(hwnd).ui.ime_area();
        let scale = kept(hwnd).scale;
        // An input method composing while there is no text cursor: nothing is told.
        unsafe { window_proc(hwnd, WM_IME_STARTCOMPOSITION, 0, 0) };
        assert_eq!((cursor(), told().2), (None, None));
        // Tab twice: focus on the text box, its cursor showing. That alone tells the system
        // nothing yet: it is told when composition starts.
        unsafe { (window_proc(hwnd, WM_KEYDOWN, 9, 0), window_proc(hwnd, WM_KEYDOWN, 9, 0)) };
        let at = cursor().expect("the text box has the keyboard");
        let spot = cursor_pixels(at, scale);
        assert!(seen.borrow().typed.contains(pt(at.x, at.y)) && at.h > 0.0);
        assert_eq!(told().2, None);
        unsafe { window_proc(hwnd, WM_IME_STARTCOMPOSITION, 0, 0) };
        assert_eq!(told(), (2, (spot.left, spot.top), Some((0x80, (spot.left, spot.top), spot))));
        // Typing moves the cursor on; as composition continues, the system is told.
        unsafe { (window_proc(hwnd, WM_CHAR, 'm' as usize, 0), window_proc(hwnd, WM_CHAR, 'm' as usize, 0)) };
        let moved = cursor_pixels(cursor().unwrap(), scale);
        assert!(moved.left > spot.left && moved.top == spot.top);
        assert_eq!(told().1, (spot.left, spot.top));
        unsafe { window_proc(hwnd, WM_IME_COMPOSITION, 0, 0) };
        assert_eq!(told(), (2, (moved.left, moved.top), Some((0x80, (moved.left, moved.top), moved))));
        close((hwnd, ole));
    }

    #[test]
    fn the_pointer_is_kept_until_the_last_button_is_up() {
        keys_up();
        let seen = Rc::new(RefCell::new(Seen::default()));
        let (hwnd, ole) = window(&seen);
        // The left button goes down, then the right one (each message says which are down).
        unsafe { (window_proc(hwnd, WM_LBUTTONDOWN, 1, 0), window_proc(hwnd, WM_RBUTTONDOWN, 3, 0)) };
        assert_eq!(unsafe { GetCapture() }, hwnd);
        unsafe { window_proc(hwnd, WM_RBUTTONUP, 1, 0) };
        assert_eq!(unsafe { GetCapture() }, hwnd, "the left button is still down");
        unsafe { window_proc(hwnd, WM_LBUTTONUP, 0, 0) };
        assert_eq!(unsafe { GetCapture() }, 0);
        close((hwnd, ole));
    }

    #[test]
    fn a_line_of_text_stands_with_its_letters_in_the_middle_of_its_box() {
        // Words in the system fonts at UI sizes, each in a 61-pixel-high box. An all-caps word
        // has its capitals' middle at the box's middle, to half a pixel (rows are whole); a
        // word with lowercase has the point halfway between the middle of its capitals and of
        // its x-height there. Windows' font keeps much more room above its letters than below
        // the baseline: placed by line height alone, words stood up to a pixel and a half low.
        let middle = |font: Font, letter: &str| {
            let mut ui = Ui::new(crate::platform::system_text(), Box::new(crate::platform::LocalClipboard::default()));
            ui.resize(Size { w: 40.0, h: 61.0 }, 1.0);
            (0..2).for_each(|_| _ = ui.run(&mut |ui| _ = ui.el().size(40.0, 61.0).fill(Color::BLACK).text(letter, font, Color::WHITE).show()));
            let drawn = crate::Surface::draw(&mut ui, 64);
            let written: Vec<usize> = drawn.pixels.chunks(drawn.width).enumerate().filter(|(_, row)| row.iter().any(|pixel| pixel & 0xff > 0x20)).map(|(y, _)| y).collect();
            [written[0] as f32, written[written.len() - 1] as f32]
        };
        for font in [10.5, 11.0, 11.5, 12.0, 13.0, 16.0, 24.0].into_iter().flat_map(|size| [Font::new(size), Font::new(size).bold(), Font::new(size).mono()]) {
            let [top, line] = middle(font, "HEIGHT 12");
            assert!(((top + line) / 2.0 - 30.0).abs() <= 0.5, "{font:?}: its capitals stand {} pixels under the middle of their box", (top + line) / 2.0 - 30.0);
            // "Hx": the top of the "H" and the baseline; "x" alone: its top.
            let ([top, line], [small, _]) = (middle(font, "Hx"), middle(font, "x"));
            let off = ((top + line) / 2.0 + (small + line) / 2.0) / 2.0 - 30.0;
            assert!(off.abs() <= 0.5, "{font:?}: its letters stand {off} pixels under the middle of their box");
        }
    }

    #[test]
    fn text_for_a_surface_gives_back_what_it_took() {
        // How many GDI objects this program holds (0: `GR_GDIOBJECTS`).
        let taken = || unsafe { GetGuiResources(GetCurrentProcess(), 0) };
        let before = taken();
        for _ in 0..1000 {
            let mut text = SystemText::new().unwrap();
            assert!(text.measure("Abc", Font::new(14.0), 1.0, None) > 0.0 && text.measure("Abc", Font::new(14.0).bold(), 1.0, None) > 0.0);
        }
        // If kept, each round would leave three behind: 3,000. The count is process-wide, and
        // tests beside this one take some meanwhile: hundreds when they read Indic fonts.
        assert!(taken() < before + 600, "{} more than the {before} before", taken() - before);
    }

    /// The multi-window test app's state, and what it saw on its last run.
    struct Two {
        /// A second window with a check box and a text box; and a smaller one holding a line of
        /// text, closed at first.
        prefs: SecondWindow,
        log: SecondWindow,
        /// How often the first window's button was pressed, and the second window's box
        /// contents.
        pressed: u32,
        dark: bool,
        text: String,
        /// Where the check box and text box are in the second window.
        tick: Rect,
        typed: Rect,
        /// Whether the second window asks about files, and the files it got; and whether the
        /// first window asks about files too.
        asks: bool,
        got: Vec<PathBuf>,
        asks_first: bool,
        /// Whether the app refuses to close the second window; and whether it closes that
        /// window itself once handed files there.
        keep: bool,
        shuts: bool,
        /// A title for the second window to take.
        title: Option<&'static str>,
    }

    impl Two {
        fn new() -> Rc<RefCell<Two>> {
            let (prefs, log) = (SecondWindow::new(200.0, 120.0), SecondWindow { open: false, ..SecondWindow::new(150.0, 90.0) });
            Rc::new(RefCell::new(Two {
                prefs,
                log,
                pressed: 0,
                dark: false,
                text: String::new(),
                tick: Rect::EMPTY,
                typed: Rect::EMPTY,
                asks: false,
                got: Vec::new(),
                asks_first: false,
                keep: false,
                shuts: false,
                title: None,
            }))
        }
    }

    /// A first window as `run` creates it, with a button, whose app describes the open second
    /// windows. None is ever shown. The first one's corner is moved to `corner` before the app
    /// first runs, if given: the others open from there.
    fn two(app: &Rc<RefCell<Two>>, corner: Option<POINT>) -> HANDLE {
        let app = app.clone();
        let describe = move |ui: &mut Ui| {
            let mut app = app.borrow_mut();
            let app = &mut *app;
            app.pressed += ui.button("In the first window").clicked as u32;
            _ = app.asks_first && ui.files_hovered();
            if ui.window(&mut app.prefs, "Preferences") {
                app.tick = ui.checkbox(&mut app.dark, "Dark").rect;
                let typed = ui.text_box(&mut app.text, "Type here");
                if app.asks {
                    app.got.extend(ui.files_dropped(&typed).into_iter().flatten());
                }
                app.typed = typed.rect;
                app.title.take().into_iter().for_each(|title| ui.set_title(title));
                ui.close();
            }
            app.prefs.open |= app.keep;
            app.prefs.open &= !(app.shuts && !app.got.is_empty());
            if ui.window(&mut app.log, "Log") {
                ui.label("Nothing yet");
                ui.close();
            }
        };
        let hwnd = open(Window::default(), Box::new(describe));
        unsafe {
            // Moved, nothing else: not resized, raised or activated.
            corner.into_iter().for_each(|at| _ = SetWindowPos(hwnd, 0, at.x, at.y, 0, 0, 0x0015));
            window_proc(hwnd, WM_SIZE, 0, 0);
            window_proc(hwnd, WM_SIZE, 0, 0);
        }
        hwnd
    }
    /// A place far off every screen: windows there are seen by nobody, even when shown.
    const FAR: POINT = POINT { x: -30000, y: -30000 };

    /// The windows of the program whose first window this is, first first.
    fn windows(first: HANDLE) -> Vec<HANDLE> {
        state(first).windows.iter().map(|window| window.hwnd).collect()
    }
    fn title(hwnd: HANDLE) -> String {
        let mut text = [0u16; 64];
        let len = unsafe { GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32) };
        String::from_utf16_lossy(&text[..len as usize])
    }
    /// A window's client size, in pixels.
    fn inside(hwnd: HANDLE) -> (i32, i32) {
        let mut area = RECT::default();
        unsafe { GetClientRect(hwnd, &mut area) };
        (area.right, area.bottom)
    }
    /// A window's screen rectangle, frame included.
    fn outside(hwnd: HANDLE) -> RECT {
        let mut area = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut area) };
        area
    }
    /// A DIB Windows draws into, the size of a window's client area.
    struct Sheet {
        dc: HANDLE,
        bitmap: HANDLE,
        pixels: *mut u32,
        wide: i32,
        high: i32,
    }

    impl Sheet {
        fn new(hwnd: HANDLE) -> Sheet {
            let ((wide, high), mut pixels) = (inside(hwnd), std::ptr::null_mut());
            let info = BITMAPINFO { size: 40, width: wide, height: -high, planes: 1, bits: 32, ..Default::default() };
            unsafe {
                let dc = CreateCompatibleDC(0);
                let bitmap = CreateDIBSection(dc, &info, 0, &mut pixels, 0, 0);
                SelectObject(dc, bitmap);
                assert!(!pixels.is_null());
                Sheet { dc, bitmap, pixels, wide, high }
            }
        }
        /// Has the window paint these parts onto the sheet as its `WM_PAINT` paints them on
        /// screen: through the strip, part by part.
        fn draw(&self, hwnd: HANDLE, parts: &[RECT]) {
            let state = state(hwnd);
            state.strip.fit(state.windows.iter().map(|window| window.wide).max().unwrap_or(0));
            parts.iter().for_each(|part| state.draw(0, self.dc, *part));
        }
        fn all(&self) -> RECT {
            RECT { left: 0, top: 0, right: self.wide, bottom: self.high }
        }
        /// The pixels, rows from the top.
        fn pixels(&self) -> Vec<u32> {
            unsafe {
                GdiFlush();
                std::slice::from_raw_parts(self.pixels, (self.wide * self.high) as usize).to_vec()
            }
        }
    }

    impl Drop for Sheet {
        fn drop(&mut self) {
            unsafe {
                DeleteDC(self.dc);
                DeleteObject(self.bitmap);
            }
        }
    }

    /// A window's client area as Windows has it draw all of itself for a picture (`PrintWindow`
    /// 3: client only, with everything it draws), though it is on no screen: its pixels, rows
    /// from the top, and the row length. (A picture can come out empty though the window drew
    /// all of itself: the first one of a just-shown window, and any one under load. So pictures
    /// are taken until two in a row agree, ten at most.)
    pub(crate) fn picture(hwnd: HANDLE) -> (Vec<u32>, usize) {
        let mut last: Option<(Vec<u32>, usize)> = None;
        for _ in 0..10 {
            let sheet = Sheet::new(hwnd);
            assert!(unsafe { PrintWindow(hwnd, sheet.dc, 3) != 0 });
            let now = (sheet.pixels(), sheet.wide as usize);
            if last.as_ref() == Some(&now) {
                return now;
            }
            last = Some(now);
        }
        panic!("ten pictures in a row of the window differed")
    }
    /// A window's update region, as Windows keeps it.
    fn stale(hwnd: HANDLE) -> RECT {
        let mut area = RECT::default();
        unsafe { GetUpdateRect(hwnd, &mut area, 0) };
        area
    }
    /// Dispatches every waiting message to these windows and has them paint, until nothing is
    /// left to dispatch or paint: as an idle window does. (The system can send a window
    /// something at any time, such as a setting changed meanwhile, and it repaints for it. What
    /// it asked to be woken for comes in its time: waited for, a second at most.)
    fn settled(hwnds: &[HANDLE]) {
        for _ in 0..100 {
            pump();
            hwnds.iter().for_each(|hwnd| _ = picture(*hwnd));
            if !waiting() && hwnds.iter().all(|hwnd| stale(*hwnd) == RECT::default()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the windows never came to rest");
    }
    /// Whether a message waits for this thread's windows.
    fn waiting() -> bool {
        let mut msg = MSG::default();
        unsafe { PeekMessageW(&mut msg, 0, 0, 0, 0) != 0 }
    }
    /// A window point given in units, as a mouse message packs it.
    fn place(hwnd: HANDLE, at: Point) -> isize {
        let scale = kept(hwnd).scale;
        ((at.y * scale) as isize) << 16 | (at.x * scale) as isize
    }
    /// Dispatches every waiting message to its window, as `run`'s loop does. Says whether the
    /// loop was told to end (`WM_QUIT`, 18). Makes this thread see every key as up, whatever
    /// the user holds meanwhile: a test pressing Tab means Tab, not Shift+Tab. (A thread has
    /// its own keyboard state, which is all this changes.)
    fn keys_up() {
        // SAFETY: the keyboard state is 256 bytes, one per key.
        unsafe { SetKeyboardState(&[0; 256]) };
    }

    /// Whether this test program has the GPU path: the name of the function that makes
    /// Direct3D devices is in its file. (The name is made here at run time, so the test is not
    /// what compiles it in.)
    pub(crate) fn has_device_creation() -> bool {
        let mut name = *b"D3D11CreateDevicf";
        name[16] = std::hint::black_box(name[16]) - 1;
        let program = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        program.windows(name.len()).any(|bytes| bytes == name)
    }

    #[test]
    #[cfg(not(feature = "gpu"))]
    fn without_the_feature_the_gpu_path_is_not_compiled_in() {
        assert!(!has_device_creation());
    }

    pub(crate) fn pump() -> bool {
        let (mut msg, mut end) = (MSG::default(), false);
        while unsafe { PeekMessageW(&mut msg, 0, 0, 0, 1) } != 0 {
            end |= msg.message == 18;
            unsafe { DispatchMessageW(&msg) };
        }
        end
    }

    #[test]
    fn a_second_window_is_a_window_of_its_own() {
        keys_up();
        let app = Two::new();
        let first = two(&app, None);
        let [_, second] = windows(first)[..] else { panic!("{} windows", windows(first).len()) };
        // It has the app's title and the client size the app asked, at its screen's scale. It
        // has no owner (-8), and like the first, is not shown.
        let scale = kept(second).scale;
        assert_eq!((title(second), inside(second)), ("Preferences".into(), ((200.0 * scale) as i32, (120.0 * scale) as i32)));
        assert_eq!(unsafe { (GetWindowLongPtrW(second, -8), IsWindowVisible(second)) }, (0, 0));
        // A click posted to it checks its box, and the first window's button, at the same place
        // in its own window, is not pressed. And the reverse.
        let click = |hwnd: HANDLE| {
            [(WM_MOUSEMOVE, 0), (WM_LBUTTONDOWN, 1), (WM_LBUTTONUP, 0)].into_iter().for_each(|(msg, w)| _ = unsafe { PostMessageW(hwnd, msg, w, place(hwnd, pt(12.0, 14.0))) });
            pump();
        };
        click(second);
        assert_eq!((app.borrow().dark, app.borrow().pressed), (true, 0));
        click(first);
        assert_eq!((app.borrow().dark, app.borrow().pressed), (true, 1));
        // A title its app gives it is its own.
        app.borrow_mut().title = Some("Renamed");
        unsafe { window_proc(first, WM_WAKE, 0, 0) };
        assert_eq!((title(second), title(first)), ("Renamed".into(), String::new()));
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn a_second_window_goes_when_the_app_no_longer_describes_it() {
        let app = Two::new();
        let first = two(&app, None);
        let second = windows(first)[1];
        // Its close button asks the app, nothing more: where the app refuses and reopens it at
        // once, the window stays as is.
        app.borrow_mut().keep = true;
        unsafe { window_proc(second, WM_CLOSE, 0, 0) };
        assert_eq!((app.borrow().prefs.open, unsafe { IsWindow(second) }, windows(first)), (true, 1, vec![first, second]));
        // Otherwise the app hears it and stops describing the window, which is destroyed.
        app.borrow_mut().keep = false;
        unsafe { window_proc(second, WM_CLOSE, 0, 0) };
        assert_eq!((app.borrow().prefs.open, unsafe { IsWindow(second) }, windows(first)), (false, 0, vec![first]));
        // The app reopens it, creating a new window, and closes it itself.
        app.borrow_mut().prefs.open = true;
        unsafe { window_proc(first, WM_WAKE, 0, 0) };
        let again = windows(first)[1];
        assert_eq!((title(again), unsafe { IsWindow(again) }), ("Preferences".into(), 1));
        app.borrow_mut().prefs.open = false;
        unsafe { window_proc(first, WM_WAKE, 0, 0) };
        assert_eq!((unsafe { IsWindow(again) }, windows(first)), (0, vec![first]));
        // One destroyed by anything else is closed for the app all the same.
        app.borrow_mut().log.open = true;
        unsafe { window_proc(first, WM_WAKE, 0, 0) };
        let log = windows(first)[1];
        assert_eq!((title(log), unsafe { DestroyWindow(log) }), ("Log".into(), 1));
        assert_eq!((app.borrow().log.open, unsafe { IsWindow(log) }, windows(first)), (false, 0, vec![first]));
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn closing_the_first_window_ends_the_others_and_the_program() {
        let app = Two::new();
        app.borrow_mut().log.open = true;
        let first = two(&app, None);
        let all = windows(first);
        // The app is held by this test and by what the first window runs.
        assert_eq!((all.len(), Rc::strong_count(&app)), (3, 2));
        // The first window's close button is Windows' to answer, which destroys the window.
        unsafe { window_proc(first, WM_CLOSE, 0, 0) };
        assert_eq!(all.iter().map(|hwnd| unsafe { IsWindow(*hwnd) }).sum::<i32>(), 0, "every window is gone");
        assert_eq!((Rc::strong_count(&app), pump()), (1, true), "what the windows ran is freed, and the loop is told to end");
    }

    #[test]
    fn windows_that_go_leave_nothing_behind() {
        unsafe extern "system" fn count(_: HANDLE, into: isize) -> i32 {
            unsafe { *(into as *mut u32) += 1 };
            1
        }
        // How many windows this thread has (the system keeps a few for a thread that had one),
        // and how many GDI (0) and USER (1) objects the program holds.
        let here = || {
            let mut windows = 0u32;
            unsafe { EnumThreadWindows(GetCurrentThreadId(), count, &raw mut windows as isize) };
            windows
        };
        let taken = |which: u32| unsafe { GetGuiResources(GetCurrentProcess(), which) };
        // On the CPU: no device is made meanwhile (`gpu_windows` has the GPU's own test).
        #[cfg(feature = "gpu")]
        crate::gpu_windows::tests::fake(crate::gpu_windows::tests::Faked { no_gpu: true, ..Default::default() });
        // Three windows, each asked to paint, which creates the strip (nothing is drawn: they
        // are not shown). The app closes one; the other goes with the first.
        let round = || {
            let app = Two::new();
            app.borrow_mut().log.open = true;
            let first = two(&app, None);
            windows(first).into_iter().for_each(|hwnd| _ = unsafe { window_proc(hwnd, WM_PAINT, 0, 0) });
            assert!(state(first).strip.bitmap.get() != 0 && windows(first).len() == 3);
            app.borrow_mut().prefs.open = false;
            unsafe { (window_proc(first, WM_WAKE, 0, 0), DestroyWindow(first)) };
            assert!(pump() && Rc::strong_count(&app) == 1);
        };
        round();
        let before = (here(), taken(0), taken(1));
        (0..200).for_each(|_| round());
        // If kept, each round would leave three windows, a DC, a bitmap and its fonts; the
        // other tests take a few of each meanwhile.
        assert_eq!(here(), before.0, "windows of this thread");
        assert!(taken(0) < before.1 + 100, "{} more to draw with than the {} before", taken(0) - before.1, before.1);
        assert!(taken(1) < before.2 + 100, "{} more windows and the like than the {} before", taken(1) - before.2, before.2);
    }

    #[test]
    fn what_changes_in_a_second_window_is_drawn_again_there() {
        keys_up();
        // Windows tracks invalidation only for shown windows. So these two are shown: far off
        // every screen, the second a step from the first, not on the task bar
        // (`WS_EX_TOOLWINDOW`, 0x80), never taking focus (`WS_EX_NOACTIVATE`, 0x0800_0000), not
        // activated (`SW_SHOWNOACTIVATE`, 4). Nothing there is drawn until a picture is taken,
        // for which each window draws all of itself.
        let app = Two::new();
        let first = two(&app, Some(FAR));
        let second = windows(first)[1];
        for hwnd in [first, second] {
            unsafe { (SetWindowLongPtrW(hwnd, -20, GetWindowLongPtrW(hwnd, -20) | 0x80 | 0x0800_0000), ShowWindow(hwnd, 4)) };
        }
        let click = |hwnd: HANDLE| [(WM_MOUSEMOVE, 0), (WM_LBUTTONDOWN, 1), (WM_LBUTTONUP, 0)].map(|(msg, w)| unsafe { window_proc(hwnd, msg, w, place(hwnd, pt(12.0, 14.0))) });
        assert!(unsafe { IsWindowVisible(first) != 0 && IsWindowVisible(second) != 0 && MonitorFromWindow(second, 0) == 0 && GetForegroundWindow() != second });
        // Each step starts with both windows idle: whatever the system sent them meanwhile
        // dispatched and painted (see `settled`).
        settled(&[first, second]);
        // Both are drawn through the one strip, as wide as the wider; the narrow one to its
        // last pixel, which has the background colour.
        let (ground, wider) = (state(first).ui.theme.bg.0, inside(first).0.max(inside(second).0));
        assert_eq!((picture(first).0.last(), picture(second).0.last(), state(first).strip.width.get()), (Some(&ground), Some(&ground), wider));
        // The check box's row in second-window pixels; and one pixel of that window inside the
        // box, near its left edge, as a picture has it.
        let (row, scale) = (cursor_pixels(app.borrow().tick, kept(second).scale), kept(second).scale);
        let (x, y) = (row.left + (3.0 * scale) as i32, (row.top + row.bottom) / 2);
        let in_box = || {
            let (pixels, wide) = picture(second);
            pixels[y as usize * wide + x as usize]
        };
        let unticked = in_box();
        assert_eq!((stale(first), stale(second)), (RECT::default(), RECT::default()), "nothing is left to draw");
        // A click in the second window checks its box: the box is invalidated in that window,
        // nothing in the first.
        click(second);
        let changed = stale(second);
        assert!(app.borrow().dark && stale(first) == RECT::default());
        assert!(changed.left >= row.left && changed.top >= row.top && changed.right <= row.right && changed.bottom <= row.bottom, "{changed:?} is not in {row:?}");
        assert!(changed.left <= x && x < changed.right && changed.top <= y && y < changed.bottom, "{changed:?} leaves out the box");
        assert_ne!(in_box(), unticked, "the tick shows in what the window draws");
        // And the reverse.
        settled(&[first, second]);
        click(first);
        assert!(app.borrow().pressed == 1 && stale(first) != RECT::default() && stale(second) == RECT::default());
        unsafe { DestroyWindow(first) };
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetClassLongPtrW(hwnd: HANDLE, which: i32) -> usize;
        fn GetIconInfo(icon: HANDLE, parts: *mut ICONINFO) -> i32;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn GetObjectW(object: HANDLE, room: i32, into: *mut i32) -> i32;
    }

    /// The width of this library's window class icon, large (-14) or small (-34). (`BITMAP`
    /// starts with four ints, the second its width.)
    fn icon_shown(hwnd: HANDLE, which: i32) -> i32 {
        let (mut parts, mut picture) = (ICONINFO::default(), [0i32; 8]);
        // SAFETY: Windows fills in what it is given: the icon's two bitmaps, freed by this
        // test, and 32 bytes about one of them.
        unsafe {
            GetIconInfo(GetClassLongPtrW(hwnd, which) as HANDLE, &mut parts);
            GetObjectW(parts.color, 32, picture.as_mut_ptr());
            DeleteObject(parts.color);
            DeleteObject(parts.mask);
        }
        picture[1]
    }

    #[test]
    fn its_windows_have_the_programs_own_pictures_of_itself() {
        // Pictures in four sizes: Windows uses the smallest big enough, for both sizes.
        let pictures = [16usize, 32, 48, 64].map(|side| (side, vec![0xff40_5060u32; side * side]));
        let icons = pictures.each_ref().map(|(side, pixels)| (*side, pixels.as_slice()));
        let hwnd = open(Window { position: Some((300, 200)), width: 400.0, height: 240.0, icons: &icons, ..Window::default() }, Box::new(|ui: &mut Ui| _ = ui.label("One")));
        let fitting = |which: i32| [16, 32, 48, 64].into_iter().find(|side| *side >= unsafe { GetSystemMetrics(which) }).unwrap_or(64);
        assert_eq!([icon_shown(hwnd, -14), icon_shown(hwnd, -34)], [fitting(11), fitting(49)]);
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn a_window_opens_where_its_app_says_and_its_app_is_told_where_it_is() {
        // A window has a screen place whether or not it shows.
        let first = open(Window { position: Some((300, 200)), width: 400.0, height: 240.0, ..Window::default() }, Box::new(|ui: &mut Ui| _ = ui.label("One")));
        unsafe { window_proc(first, WM_SIZE, 0, 0) };
        let placed = || state(first).ui.window_placement().expect("the window was told where it is");
        assert_eq!((corner_of(first), placed()), (POINT { x: 300, y: 200 }, WindowPlacement { position: (300, 200), width: 400.0, height: 240.0, maximized: false }));
        // Moved, as the user would, nothing else changing; its interface is told.
        unsafe { SetWindowPos(first, 0, 520, 360, 0, 0, 0x0015) };
        assert_eq!(placed().position, (520, 360));
        // A maximized window (only set in its style here: it is not shown) is told so, with its
        // restore place and size.
        unsafe { (SetWindowLongPtrW(first, -16, GetWindowLongPtrW(first, -16) | 0x0100_0000), window_proc(first, WM_MOVE, 0, 0)) };
        assert_eq!(placed(), WindowPlacement { position: (520, 360), width: 400.0, height: 240.0, maximized: true });
        unsafe { DestroyWindow(first) };
        // A place on no screen is left to the system.
        let lost = open(Window { position: Some((-30000, -30000)), ..Window::default() }, Box::new(|ui: &mut Ui| _ = ui.label("One")));
        assert!(corner_of(lost).x > -30000 && corner_of(lost).y > -30000);
        unsafe { DestroyWindow(lost) };

        // A second window: where its app says, no smaller than it says. Its app hears where it
        // is and where it moves.
        let app = Two::new();
        app.borrow_mut().prefs = SecondWindow::new(200.0, 120.0).position(640, 480).min_size(120.0, 60.0);
        let first = two(&app, None);
        let second = windows(first)[1];
        assert_eq!((corner_of(second), kept(second).min_size, app.borrow().prefs.position), (POINT { x: 640, y: 480 }, (120.0, 60.0), Some((640, 480))));
        unsafe { SetWindowPos(second, 0, 700, 500, 0, 0, 0x0015) };
        assert_eq!((corner_of(second), app.borrow().prefs.position), (POINT { x: 700, y: 500 }, Some((700, 500))));
        // One with no place given opens a step from the first, and its app hears where.
        let log = windows(first).len();
        app.borrow_mut().log.open = true;
        unsafe { window_proc(first, WM_WAKE, 0, 0) };
        // The run telling the app is one the first window posts for itself.
        pump();
        let third = windows(first)[log];
        assert_eq!(app.borrow().log.position.map(|(x, y)| POINT { x, y }), Some(corner_of(third)));
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn a_new_window_opens_a_step_from_the_one_before_it() {
        // Two windows opening together: the first a step down and right of the first window,
        // the next a step further. A step is a title bar's height.
        let app = Two::new();
        app.borrow_mut().log.open = true;
        let first = two(&app, Some(FAR));
        let [_, prefs, log] = windows(first)[..] else { panic!("{} windows", windows(first).len()) };
        let step = -framed(0.0, 0.0, unsafe { GetDpiForWindow(first) }, Some([WS_OVERLAPPEDWINDOW, 0])).top;
        assert!(step > 0);
        let steps = |count: i32| POINT { x: FAR.x + count * step, y: FAR.y + count * step };
        assert_eq!((corner_of(prefs), corner_of(log)), (steps(1), steps(2)));
        unsafe { DestroyWindow(first) };

        // Where the next would open, a step from the last created, another window already has
        // its corner (the first, moved there): the next opens a step further.
        let app = Two::new();
        let first = two(&app, Some(FAR));
        unsafe { SetWindowPos(first, 0, steps(2).x, steps(2).y, 0, 0, 0x0015) };
        app.borrow_mut().log.open = true;
        unsafe { window_proc(first, WM_WAKE, 0, 0) };
        assert_eq!(windows(first).iter().map(|hwnd| corner_of(*hwnd)).collect::<Vec<_>>(), [steps(2), steps(1), steps(3)]);
        unsafe { DestroyWindow(first) };

        // A first window on a screen (never shown, like the others), its corner at the far
        // corner of the screen's work area: a step from there would be off it. The new window
        // is kept on its first window's screen.
        let app = Two::new();
        app.borrow_mut().prefs.open = false;
        let first = two(&app, None);
        let mut screen = MONITORINFO { size: size_of::<MONITORINFO>() as u32, all: RECT::default(), work: RECT::default(), flags: 0 };
        unsafe {
            assert!(GetMonitorInfoW(MonitorFromWindow(first, 0), &mut screen) != 0, "the system put the first window on no screen");
            SetWindowPos(first, 0, screen.work.right - 10, screen.work.bottom - 10, 0, 0, 0x0015);
            assert!(GetMonitorInfoW(MonitorFromWindow(first, 0), &mut screen) != 0);
        }
        app.borrow_mut().prefs.open = true;
        unsafe { window_proc(first, WM_WAKE, 0, 0) };
        let (second, work) = (outside(windows(first)[1]), screen.work);
        assert!(work.left <= second.left && second.right <= work.right && work.top <= second.top && second.bottom <= work.bottom, "{second:?} is not on {work:?}");
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn each_window_has_its_own_scale_and_its_own_smallest_size() {
        let app = Two::new();
        app.borrow_mut().log.open = true;
        let first = two(&app, Some(FAR));
        let [_, prefs, log] = windows(first)[..] else { panic!("{} windows", windows(first).len()) };
        let was = kept(first).scale;
        // Windows says the second window is on a 144-dpi screen from now on (1.5 pixels per
        // unit), and where to go there.
        let to = RECT { left: FAR.x, top: FAR.y + 1000, right: FAR.x + 480, bottom: FAR.y + 1360 };
        unsafe { window_proc(prefs, WM_DPICHANGED, 144 | 144 << 16, &raw const to as isize) };
        let (wide, high) = inside(prefs);
        assert_eq!((outside(prefs), kept(prefs).scale, kept(first).scale, kept(log).scale), (to, 1.5, was, was));
        // Its interface is that big in units now, and the app is told.
        let units = Size { w: wide as f32 / 1.5, h: high as f32 / 1.5 };
        assert_eq!(state(first).in_window(1, |ui, _| (ui.size(), ui.scale())), Some((units, 1.5)));
        assert_eq!((app.borrow().prefs.width, app.borrow().prefs.height), (units.w, units.h));
        // The user can shrink it to a first window's default least size, 200 by 120 units,
        // which is more pixels at its scale. A window that opened smaller can shrink to its
        // opening size.
        let smallest = |hwnd: HANDLE| {
            let none = POINT::default();
            let mut limits = MINMAXINFO { reserved: none, max_size: none, max_position: none, min_track: none, max_track: none };
            unsafe { window_proc(hwnd, WM_GETMINMAXINFO, 0, &raw mut limits as isize) };
            limits.min_track
        };
        let around = |w: f32, h: f32, dpi: u32| {
            let frame = framed(w, h, dpi, Some([WS_OVERLAPPEDWINDOW, 0]));
            POINT { x: frame.right - frame.left, y: frame.bottom - frame.top }
        };
        assert_eq!((smallest(prefs), smallest(log)), (around(200.0, 120.0, 144), around(150.0, 90.0, (was * 96.0) as u32)));
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn a_second_window_is_typed_into_by_itself() {
        keys_up();
        let app = Two::new();
        let first = two(&app, None);
        let second = windows(first)[1];
        // Neither window has focus, as Windows says while creating the second: both heard it
        // all the same.
        assert_eq!((state(first).ui.window_focused, state(first).in_window(1, |ui, _| ui.window_focused)), (false, Some(false)));
        // The second window is told it has focus, as when clicked. Tab twice puts focus on its
        // text box, and a letter goes there.
        unsafe {
            window_proc(second, WM_SETFOCUS, 0, 0);
            window_proc(second, WM_KEYDOWN, 9, 0);
            window_proc(second, WM_KEYDOWN, 9, 0);
            window_proc(second, WM_CHAR, 'm' as usize, 0);
        }
        assert_eq!((app.borrow().text.as_str(), state(first).ui.window_focused), ("m", false));
        assert!(state(first).timer, "its cursor blinks by the one timer, which is the first window's");
        // An input method composing there is told the cursor's place in that window; the first
        // window tells no place.
        let cursor = state(first).in_window(1, |ui, _| ui.ime_area()).unwrap().expect("the text box has the keyboard");
        let spot = cursor_pixels(cursor, kept(second).scale);
        assert!(app.borrow().typed.contains(pt(cursor.x, cursor.y)));
        let mut told = COMPOSITIONFORM { style: 0, at: POINT::default(), area: RECT::default() };
        unsafe {
            window_proc(second, WM_IME_STARTCOMPOSITION, 0, 0);
            let context = ImmGetContext(second);
            assert!(context != 0, "this thread has no input context to read back from");
            ImmGetCompositionWindow(context, &mut told);
            ImmReleaseContext(second, context);
        }
        assert_eq!((told.style, told.at, kept(second).ime_spot, kept(first).ime_spot), (2, POINT { x: spot.left, y: spot.top }, spot, RECT::default()));
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn files_are_let_go_on_the_window_whose_app_asks_for_them() {
        let app = Two::new();
        app.borrow_mut().asks = true;
        let first = two(&app, None);
        let second = windows(first)[1];
        let (names, kept_as, mut effect) = (["C:\\one.txt", "C:\\two\\𝄞.txt"], wide("OleDropTargetInterface"), ANY);
        let (data_table, files) = (Dragged::table(), in_memory(&block((0, 0), false, true, &names)));
        let mut dragged = Dragged::new(&data_table, files);
        unsafe {
            // Only the second window's app part asks about files: that window has a drop
            // target, the first none.
            assert_eq!((kept(first).files, kept(second).files, GetPropW(first, kept_as.as_ptr())), (false, true, 0));
            let target = GetPropW(second, kept_as.as_ptr()) as *mut DropTarget;
            assert!(!target.is_null() && (*target).hwnd == second);
            // Files dragged over its text box and dropped there reach the app.
            let (calls, at) = (table(target), on_screen(second, middle(app.borrow().typed)));
            assert_eq!(((calls.enter)(target, &raw mut dragged.object, 0, at, &mut effect), effect), (0, COPY));
            assert_eq!(((calls.let_go)(target, &raw mut dragged.object, 0, at, &mut effect), effect), (0, COPY));
            assert_eq!((&app.borrow().got, dragged.held.get()), (&paths(&names), 1));
            GlobalFree(files);
            // The app closes the second window. With one more reference here, its drop target
            // outlives it by that one: Windows released it. OLE stays for the other windows.
            // (Meanwhile the thread cannot use the COM apartment OLE rules out, 0:
            // `RPC_E_CHANGED_MODE`, 0x80010106.)
            assert_eq!((calls.hold)(target), 2);
            app.borrow_mut().prefs.open = false;
            window_proc(first, WM_WAKE, 0, 0);
            assert_eq!((IsWindow(second), (calls.release)(target), CoInitializeEx(std::ptr::null_mut(), 0)), (0, 0, 0x8001_0106_u32 as i32));
            // Now the app asks about files in the first window, which accepts them from then
            // on, through the existing OLE.
            app.borrow_mut().asks_first = true;
            window_proc(first, WM_WAKE, 0, 0);
            assert_eq!((kept(first).files, GetPropW(first, kept_as.as_ptr()) != 0), (true, true));
            // With the first window gone, OLE was uninitialized as often as initialized: once.
            DestroyWindow(first);
            assert_eq!(CoInitializeEx(std::ptr::null_mut(), 0), 0);
            CoUninitialize();
        }
    }

    #[test]
    fn files_may_close_the_window_they_are_let_go_on() {
        let app = Two::new();
        (app.borrow_mut().asks, app.borrow_mut().shuts) = (true, true);
        let first = two(&app, None);
        let second = windows(first)[1];
        let (names, kept_as, mut effect) = (["C:\\one.txt"], wide("OleDropTargetInterface"), ANY);
        let (data_table, files) = (Dragged::table(), in_memory(&block((0, 0), false, true, &names)));
        let mut dragged = Dragged::new(&data_table, files);
        unsafe {
            let target = GetPropW(second, kept_as.as_ptr()) as *mut DropTarget;
            let (calls, at) = (table(target), on_screen(second, middle(app.borrow().typed)));
            // A drop target's caller holds a reference meanwhile, as Windows does.
            assert_eq!(((calls.hold)(target), (calls.enter)(target, &raw mut dragged.object, 0, at, &mut effect)), (2, 0));
            // The app is handed the files and closes the window for them, mid-drop: the window
            // is gone, and the drop is answered all the same.
            assert_eq!(((calls.let_go)(target, &raw mut dragged.object, 0, at, &mut effect), effect), (0, COPY));
            assert_eq!((&app.borrow().got, IsWindow(second), windows(first), dragged.held.get()), (&paths(&names), 0, vec![first], 1));
            // Windows released the drop target when the window went; OLE is uninitialized with
            // the first window, not before.
            assert_eq!(((calls.release)(target), CoInitializeEx(std::ptr::null_mut(), 0)), (0, 0x8001_0106_u32 as i32));
            GlobalFree(files);
            DestroyWindow(first);
            assert_eq!(CoInitializeEx(std::ptr::null_mut(), 0), 0);
            CoUninitialize();
        }
    }

    /// A region made of these rectangles: left, top, right, bottom.
    fn region(of: &[[i32; 4]]) -> HANDLE {
        unsafe {
            let all = CreateRectRgn(0, 0, 0, 0);
            for [left, top, right, bottom] in of {
                let one = CreateRectRgn(*left, *top, *right, *bottom);
                // 2: `RGN_OR`.
                CombineRgn(all, all, one, 2);
                DeleteObject(one);
            }
            all
        }
    }

    #[test]
    fn what_windows_wants_drawn_is_read_as_the_rectangles_it_is_made_of() {
        // A window off screen has no update region, so this uses regions combined here: what a
        // window's would be after such changes.
        let rect = |[left, top, right, bottom]: [i32; 4]| RECT { left, top, right, bottom };
        // Two buttons far apart: two rectangles; their bounding one is far larger.
        let two = region(&[[10, 10, 55, 38], [10, 262, 82, 290]]);
        let stale = Stale::of(two);
        assert_eq!((stale.header[2], stale.parts[0], stale.parts[1]), (2, rect([10, 10, 55, 38]), rect([10, 262, 82, 290])));
        assert_eq!(stale.around, rect([10, 10, 82, 290]));
        // Nothing to draw: no rectangles. And more than there is room for, down a diagonal:
        // none are given, and the paint uses the bounding rectangle.
        let (none, many) = (region(&[]), region(&[0, 1, 2, 3, 4, 5, 6, 7, 8].map(|step| [step * 20, step * 20, step * 20 + 10, step * 20 + 10])));
        assert_eq!((Stale::of(none).header[2], Stale::of(many).header[2]), (0, 0));
        unsafe { (DeleteObject(two), DeleteObject(none), DeleteObject(many)) };
    }

    #[test]
    fn two_changes_far_apart_are_two_places_for_windows_to_draw() {
        // A button at the top of a window and one at its bottom. (Shown far off every screen,
        // listed nowhere and never active, as above: Windows tracks invalidation only for shown
        // windows.)
        let at = Rc::new(Cell::new([Rect::default(); 2]));
        let seen = at.clone();
        let describe = move |ui: &mut Ui| {
            ui.column().grow().pad(10.0).align(crate::layout::Align::Start, crate::layout::Align::Start).open();
            let top = ui.button("Top").rect;
            ui.spacer();
            seen.set([top, ui.button("Bottom").rect]);
            ui.close();
        };
        let hwnd = open(Window::default(), Box::new(describe));
        unsafe {
            SetWindowPos(hwnd, 0, FAR.x, FAR.y, 0, 0, 0x0015);
            window_proc(hwnd, WM_SIZE, 0, 0);
            window_proc(hwnd, WM_SIZE, 0, 0);
            SetWindowLongPtrW(hwnd, -20, GetWindowLongPtrW(hwnd, -20) | 0x80 | 0x0800_0000);
            ShowWindow(hwnd, 4);
        }
        pump();
        let over = |which: usize| {
            let button = at.get()[which];
            unsafe { window_proc(hwnd, WM_MOUSEMOVE, 0, place(hwnd, pt(button.x + button.w / 2.0, button.y + button.h / 2.0))) };
        };
        let pixels = |which: usize| cursor_pixels(at.get()[which], kept(hwnd).scale);
        // Once it drew all of itself (for a picture), nothing is left to draw.
        picture(hwnd);
        assert_eq!(kept(hwnd).stale().header[2], 0);
        // The pointer enters the top button: it is invalidated.
        over(0);
        let stale = kept(hwnd).stale();
        assert_eq!((stale.header[2], stale.parts[0]), (1, pixels(0)));
        // Then straight to the bottom one, before anything was drawn: both buttons, each by
        // itself, nothing between.
        over(1);
        let stale = kept(hwnd).stale();
        assert_eq!((stale.header[2], stale.parts[0], stale.parts[1]), (2, pixels(0), pixels(1)));
        // The paint draws those two and leaves nothing to draw.
        unsafe { window_proc(hwnd, WM_PAINT, 0, 0) };
        assert_eq!(kept(hwnd).stale().header[2], 0);
        unsafe { DestroyWindow(hwnd) };
    }

    /// A window's update rectangles, as its paint finds them.
    fn wanted(hwnd: HANDLE) -> Vec<RECT> {
        let stale = kept(hwnd).stale();
        let mut around = RECT::default();
        unsafe { GetUpdateRect(hwnd, &mut around, 0) };
        let parts = stale.parts.get(..stale.header[2] as usize).filter(|parts| !parts.is_empty()).unwrap_or(std::slice::from_ref(&around));
        parts.iter().filter(|part| part.right > part.left && part.bottom > part.top).copied().collect()
    }

    #[test]
    fn what_is_drawn_part_by_part_is_what_drawing_it_all_gives() {
        const MARK: Icon = Icon::line("M1.8 4.3a1 1 0 0 1 1-1h3l1.4 1.5h6a1 1 0 0 1 1 1v6.4a1 1 0 0 1-1 1H2.8a1 1 0 0 1-1-1z");
        const PLUS: Icon = Icon::line("M8 3.5v9M3.5 8h9");
        // A bar of bordered buttons, and rows that highlight under the pointer and then show
        // two more buttons, each with a tooltip.
        let at = Rc::new(RefCell::new(Vec::new()));
        let seen = at.clone();
        let describe = move |ui: &mut Ui| {
            let (t, mut seen) = (ui.theme, seen.borrow_mut());
            seen.clear();
            ui.column().grow().open();
            ui.row().w(Length::Grow(1.0)).h(Length::Px(44.0)).pad((8.0, 0.0)).gap(6.0).fill(t.panel).open();
            for name in ["All notes", "Open all", "Save all", "Send all"] {
                let state = ui.press(ui.next_id());
                let edge = if state.hovered { t.text_faint } else { t.line_strong };
                ui.row().h(Length::Px(28.0)).pad((10.0, 0.0)).gap(6.0).fill(t.raised).border(edge, 1.0).radius(6.0).sense().focusable().open();
                ui.el().icon(MARK, 16.0, t.text_dim).show();
                ui.el().text(name, t.font, t.text).show();
                ui.close();
                ui.tooltip(&state, "Saves every note that has changed, one after the other, and says how each went");
                seen.push(state.rect);
            }
            ui.close();
            for row in 0..14 {
                let state = ui.press(ui.next_id());
                let fill = if state.hovered { t.hover } else { Color::NONE };
                ui.row().w(Length::Grow(1.0)).h(Length::Px(30.0)).pad((8.0, 0.0)).gap(8.0).fill(fill).sense().focusable().open();
                ui.el().icon(MARK, 16.0, t.text_dim).show();
                ui.el().text(&format!("Note number {row}"), t.font.bold(), t.text).show();
                ui.el().text("C:\\Notes\\with a long path to it", t.small_font, t.text_faint).show();
                ui.spacer();
                if state.hovered {
                    ui.el().icon(PLUS, 13.0, t.text_dim).show();
                    ui.el().icon(PLUS, 13.0, t.text).show();
                }
                ui.el().text("M", t.font.bold(), t.amber).show();
                ui.close();
                ui.tooltip(&state, "D:\\Code\\with a long path to it\\and what else there is to say about this row of the list");
                seen.push(state.rect);
            }
            ui.close();
        };
        let hwnd = open(Window { width: 520.0, height: 480.0, ..Default::default() }, Box::new(describe));
        unsafe {
            SetWindowPos(hwnd, 0, FAR.x, FAR.y, 0, 0, 0x0015);
            window_proc(hwnd, WM_SIZE, 0, 0);
            window_proc(hwnd, WM_SIZE, 0, 0);
            SetWindowLongPtrW(hwnd, -20, GetWindowLongPtrW(hwnd, -20) | 0x80 | 0x0800_0000);
            ShowWindow(hwnd, 4);
        }
        pump();
        // The pointer is not really there: `TrackMouseEvent` is not asked.
        kept(hwnd).tracking = true;
        let (shown, fresh) = (Sheet::new(hwnd), Sheet::new(hwnd));
        picture(hwnd);
        shown.draw(hwnd, &[shown.all()]);
        let step = |x: f32, y: f32, rest: u64| {
            unsafe { window_proc(hwnd, WM_MOUSEMOVE, 0, place(hwnd, pt(x, y))) };
            if rest > 0 {
                std::thread::sleep(std::time::Duration::from_millis(rest));
                unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
            }
            shown.draw(hwnd, &wanted(hwnd));
            unsafe { window_proc(hwnd, WM_PAINT, 0, 0) };
        };
        // How many shown pixels differ from a full redraw.
        let differ = || {
            fresh.draw(hwnd, &[fresh.all()]);
            shown.pixels().iter().zip(&fresh.pixels()).filter(|(have, want)| have != want).count()
        };
        let places: Vec<Rect> = at.borrow().clone();
        assert_eq!(differ(), 0);
        // Over everything once in a jumping order; then slowly down the rows a few pixels at a
        // time; then resting until the tooltips show, and off again.
        let mut wrong = Vec::new();
        for turn in 0..3 {
            for which in (0..places.len()).map(|i| (i * 7 + turn * 3) % places.len()) {
                let r = places[which];
                step(r.x + r.w / 2.0, r.y + r.h / 2.0, 0);
            }
            wrong.push(("jumps", differ()));
        }
        for y in (40..470).step_by(3) {
            step(120.0 + (y % 7) as f32, y as f32, 0);
        }
        wrong.push(("down", differ()));
        for which in [1, 6, 2, 11, 17, 0] {
            let r = places[which];
            step(r.x + r.w / 2.0, r.y + r.h / 2.0, 0);
            step(r.x + r.w / 2.0 + 1.0, r.y + r.h / 2.0, 560);
            wrong.push(("rested", differ()));
            step(r.x + r.w / 2.0 + 2.0, r.y + r.h / 2.0 + 1.0, 0);
        }
        step(-10.0, -10.0, 0);
        unsafe { window_proc(hwnd, WM_MOUSELEAVE, 0, 0) };
        shown.draw(hwnd, &wanted(hwnd));
        wrong.push(("left", differ()));
        unsafe { DestroyWindow(hwnd) };
        assert!(wrong.iter().all(|(_, wrong)| *wrong == 0), "pixels that are not what a fresh drawing gives: {wrong:?}");
    }

    /// The accessibility test app's state.
    #[derive(Default)]
    struct Form {
        loud: bool,
        level: f32,
        name: String,
        saved: u32,
        more: bool,
        choice: u8,
    }

    /// A window as `run` creates it, with a form. Never shown.
    fn form(app: &Rc<RefCell<Form>>) -> HANDLE {
        let app = app.clone();
        let describe = move |ui: &mut Ui| {
            let mut app = app.borrow_mut();
            let app = &mut *app;
            ui.label("Sound");
            ui.checkbox(&mut app.loud, "Loud");
            ui.slider(&mut app.level, 0.0..=10.0);
            ui.text_box(&mut app.name, "Your name");
            if ui.button("Save").clicked {
                app.saved += 1;
                ui.announce("Saved");
            }
            if ui.fold(&mut app.more, "More") {
                ui.label("Under the fold");
            }
            ui.radio(&mut app.choice, 0, "One");
            ui.radio(&mut app.choice, 1, "Two");
        };
        let hwnd = open(Window::default(), Box::new(describe));
        unsafe { (window_proc(hwnd, WM_SIZE, 0, 0), window_proc(hwnd, WM_SIZE, 0, 0)) };
        hwnd
    }
    /// `UiaRootObjectId` (-25), with which a window is asked for its UI Automation provider;
    /// and the pattern ids asked for: invoke, value, range value, expand/collapse, selection
    /// item, toggle.
    const ROOT: isize = -25;
    const TO_PRESS: i32 = 10000;
    const TO_TYPE: i32 = 10002;
    const TO_SLIDE: i32 = 10003;
    const TO_FOLD: i32 = 10005;
    const TO_PICK: i32 = 10010;
    const TO_TICK: i32 = 10015;
    /// The names of the things a window's interface reports.
    fn told(hwnd: HANDLE) -> Vec<String> {
        let (state, at) = unsafe { state_of(hwnd) }.unwrap();
        state.in_window(at, |ui, _| ui.visible_access_nodes().iter().map(|said| said.name.clone()).collect()).unwrap()
    }

    #[test]
    fn a_window_that_nobody_asks_keeps_no_list_for_screen_readers() {
        let app = Rc::new(RefCell::new(Form::default()));
        let hwnd = form(&app);
        unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
        assert!(!state(hwnd).ui.listening && told(hwnd).is_empty());
        // Nor one asked something else by the same message: Windows asks windows so for more
        // than screen readers want (here, `OBJID_CLIENT`'s class).
        unsafe { window_proc(hwnd, WM_GETOBJECT, 0, -12) };
        assert!(!state(hwnd).ui.listening && told(hwnd).is_empty());
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn a_window_that_is_asked_what_is_in_it_keeps_the_list_from_then_on() {
        let app = Two::new();
        let first = two(&app, None);
        let second = windows(first)[1];
        unsafe {
            // The second window is asked: Windows gets an answer, and from then on the
            // interface keeps the tree, for every window (the app describes them in one go).
            assert!(window_proc(second, WM_GETOBJECT, 0, ROOT) != 0 && state(first).ui.listening);
            assert_eq!((told(first), told(second)), (vec!["In the first window".to_string()], vec!["Dark".to_string(), "Type here".to_string()]));
            // COM is now apartment-threaded on this thread: it cannot become multithreaded (0),
            // `RPC_E_CHANGED_MODE` (0x80010106).
            assert_eq!(CoInitializeEx(std::ptr::null_mut(), 0), 0x8001_0106_u32 as i32);
            // Asked again, and the first window too: answered, with nothing more to set up.
            assert!(window_proc(second, WM_GETOBJECT, 0, ROOT) != 0 && window_proc(first, WM_GETOBJECT, 0, ROOT) != 0);
            // The app closes the window asked first; the other is still read.
            app.borrow_mut().prefs.open = false;
            window_proc(first, WM_WAKE, 0, 0);
            assert_eq!((IsWindow(second), CoInitializeEx(std::ptr::null_mut(), 0)), (0, 0x8001_0106_u32 as i32));
            // With the last window that was asked gone, COM is uninitialized.
            DestroyWindow(first);
            assert_eq!(CoInitializeEx(std::ptr::null_mut(), 0), 0);
            CoUninitialize();
        }
    }

    #[test]
    fn where_windows_could_not_keep_to_the_thread_it_is_handed_no_provider() {
        unsafe {
            // A program using the multithreaded apartment (0): Windows would call the providers
            // from other threads. The window then provides nothing itself, and Windows reports
            // it as any window.
            assert_eq!(CoInitializeEx(std::ptr::null_mut(), 0), 0);
            let app = Rc::new(RefCell::new(Form::default()));
            let hwnd = form(&app);
            assert_eq!((window_proc(hwnd, WM_GETOBJECT, 0, ROOT), state(hwnd).ui.listening), (0, false));
            DestroyWindow(hwnd);
            CoUninitialize();
        }
    }

    #[test]
    fn a_screen_reader_brings_what_is_out_of_view_into_view() {
        // Forty buttons in a box 100 high: those below are reported too, as offscreen; asked
        // through its provider, a button is scrolled into view.
        let describe = |ui: &mut Ui| {
            ui.column().h(crate::Length::Px(100.0)).scroll().open();
            for row in 0..40 {
                ui.button(&format!("Row {row}"));
            }
            ui.close();
        };
        let hwnd = open(Window::default(), Box::new(describe));
        unsafe { (window_proc(hwnd, WM_SIZE, 0, 0), window_proc(hwnd, WM_SIZE, 0, 0), window_proc(hwnd, WM_GETOBJECT, 0, ROOT)) };
        let shown = |name: &str| state(hwnd).ui.access_nodes().iter().find(|said| said.name == name).map(|said| said.visible);
        assert_eq!((shown("Row 0"), shown("Row 30")), (Some(true), Some(false)));
        assert_eq!(uia::tests::act(hwnd, "Row 30", 10017, None, None), 0);
        assert_eq!((shown("Row 0"), shown("Row 30")), (Some(false), Some(true)));
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn a_screen_reader_selects_part_of_what_is_typed_and_the_box_has_it() {
        let app = Rc::new(RefCell::new(Form::default()));
        let hwnd = form(&app);
        unsafe { window_proc(hwnd, WM_GETOBJECT, 0, ROOT) };
        assert_eq!(uia::tests::act(hwnd, "Your name", TO_TYPE, Some("Ada Lovelace"), None), 0);
        let caret = || state(hwnd).ui.visible_access_nodes().iter().find(|said| said.name == "Your name").and_then(|said| said.caret);
        assert_eq!((app.borrow().name.clone(), caret()), ("Ada Lovelace".to_string(), Some([12, 12])));
        // Fifth to ninth letter: the box has it before the call returns, and the next typing
        // replaces it.
        assert_eq!((uia::tests::select(hwnd, "Your name", [4, 8]), caret()), (0, Some([4, 8])));
        unsafe { window_proc(hwnd, WM_CHAR, 'x' as usize, 0) };
        assert_eq!((app.borrow().name.clone(), caret()), ("Ada xlace".to_string(), Some([5, 5])));
        // The same place twice puts the cursor there; one past the end, at the end.
        assert_eq!((uia::tests::select(hwnd, "Your name", [1, 1]), caret()), (0, Some([1, 1])));
        assert_eq!((uia::tests::select(hwnd, "Your name", [3, 99]), caret()), (0, Some([3, 9])));
        // With focus elsewhere, focus moves to the box, and the box has what was asked.
        assert_eq!(uia::tests::act(hwnd, "Loud", TO_TICK, None, None), 0);
        assert_eq!(caret(), None);
        assert_eq!((uia::tests::select(hwnd, "Your name", [0, 3]), caret()), (0, Some([0, 3])));
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn what_a_screen_reader_asks_of_a_thing_is_done_in_its_window() {
        let app = Rc::new(RefCell::new(Form { level: 4.0, ..Form::default() }));
        let hwnd = form(&app);
        unsafe { window_proc(hwnd, WM_GETOBJECT, 0, ROOT) };
        let act = |name: &str, pattern: i32| uia::tests::act(hwnd, name, pattern, None, None);
        let stands = || {
            let app = app.borrow();
            (app.loud, app.level, app.name.clone(), app.more, app.choice, app.saved)
        };
        // It checks the box, types a name, moves the unnamed slider, expands what is collapsed,
        // picks the other choice and presses the button: each done in the window before the
        // call returns.
        assert_eq!((act("Loud", TO_TICK), stands()), (0, (true, 4.0, String::new(), false, 0, 0)));
        assert_eq!((uia::tests::act(hwnd, "Your name", TO_TYPE, Some("Ada"), None), stands()), (0, (true, 4.0, "Ada".into(), false, 0, 0)));
        assert_eq!((uia::tests::act(hwnd, "", TO_SLIDE, None, Some(6.0)), stands()), (0, (true, 6.0, "Ada".into(), false, 0, 0)));
        assert_eq!((act("More", TO_FOLD), stands()), (0, (true, 6.0, "Ada".into(), true, 0, 0)));
        assert_eq!((act("Two", TO_PICK), act("Save", TO_PRESS), stands()), (0, 0, (true, 6.0, "Ada".into(), true, 1, 1)));
        // What the app announced about that ("Saved") is available after the run it was said
        // in, and Windows takes it; the next run has nothing to announce.
        assert_eq!((state(hwnd).ui.announced(), uia::announce(hwnd, &state(hwnd).ui)), (&["Saved".to_string()][..], 0));
        unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
        assert!(state(hwnd).ui.announced().is_empty());
        // What already is as asked stays so: expanded, picked. A slider takes the value asked,
        // also between its keyboard steps (halves here); and stays put when asked for one it
        // cannot take.
        assert_eq!((act("More", TO_FOLD), act("Two", TO_PICK), uia::tests::act(hwnd, "", TO_SLIDE, None, Some(7.2)), stands()), (0, 0, 0, (true, 7.2, "Ada".into(), true, 1, 1)));
        assert_eq!((uia::tests::act(hwnd, "", TO_SLIDE, None, Some(10.5)), app.borrow().level), (0x8007_0057_u32 as i32, 7.2));
        assert_eq!((uia::tests::act(hwnd, "", TO_SLIDE, None, Some(0.0)), app.borrow().level), (0, 0.0));
        // The interface reports the result: what was collapsed is there now, and focus is on
        // what was used last.
        assert_eq!(told(hwnd), ["Sound", "Loud", "", "Your name", "Save", "More", "Under the fold", "One", "Two"]);
        assert!(state(hwnd).ui.visible_access_nodes()[2].focused);
        // Mid-message, the window is not asked anything: "not now", and nothing is done.
        ANSWERING.set(true);
        assert_eq!((act("Save", TO_PRESS), app.borrow().saved), (0x8004_0200_u32 as i32, 1));
        ANSWERING.set(false);
        assert_eq!((act("Save", TO_PRESS), app.borrow().saved), (0, 2));
        unsafe { DestroyWindow(hwnd) };

        // In a program with two windows, each thing is used in its own.
        let app = Two::new();
        let first = two(&app, None);
        let second = windows(first)[1];
        unsafe { (window_proc(first, WM_GETOBJECT, 0, ROOT), window_proc(second, WM_GETOBJECT, 0, ROOT)) };
        assert_eq!((uia::tests::act(second, "Dark", TO_TICK, None, None), app.borrow().dark, app.borrow().pressed), (0, true, 0));
        assert_eq!((uia::tests::act(first, "In the first window", TO_PRESS, None, None), app.borrow().dark, app.borrow().pressed), (0, true, 1));
        assert_eq!((uia::tests::act(second, "Type here", TO_TYPE, Some("m"), None), app.borrow().text.as_str()), (0, "m"));
        unsafe { DestroyWindow(first) };
    }

    /// The start of a script for an accessibility client, for the window whose handle replaces
    /// WINDOW: Windows PowerShell's UI Automation client (`System.Windows.Automation`), which
    /// goes through Windows as Narrator does. `All` reports everything in the window: each
    /// element's type, name and state.
    const READER: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$uia = 'System.Windows.Automation.'
$element = [type]($uia + 'AutomationElement')
$walker = ([type]($uia + 'TreeWalker'))::RawViewWalker
$window = $element::FromHandle([IntPtr]WINDOW)
function Way($thing, $kind) { $way = $null; if ($thing.TryGetCurrentPattern(([type]($uia + $kind + 'Pattern'))::Pattern, [ref]$way)) { $way } }
function Tell($thing, $indent) {
  $is = $thing.Current
  $line = $indent + $is.ControlType.ProgrammaticName.Substring(12) + ' [' + $is.Name + ']'
  $way = Way $thing 'Toggle'; if ($way) { $line += ' ' + $way.Current.ToggleState }
  $way = Way $thing 'SelectionItem'; if ($way) { $line += ' picked: ' + $way.Current.IsSelected }
  $way = Way $thing 'ExpandCollapse'; if ($way) { $line += ' ' + $way.Current.ExpandCollapseState }
  $way = Way $thing 'Value'; if ($way) { $line += ' holds [' + $way.Current.Value + ']' }
  $way = Way $thing 'RangeValue'; if ($way) { $line += ' at ' + $way.Current.Value + ' of ' + $way.Current.Minimum + ' to ' + $way.Current.Maximum }
  if (Way $thing 'Invoke') { $line += ' to press' }
  if ($is.HasKeyboardFocus) { $line += ' <- keyboard' }
  $line
  $inside = $walker.GetFirstChild($thing)
  while ($inside) { Tell $inside ($indent + '  '); $inside = $walker.GetNextSibling($inside) }
}
function All { $inside = $walker.GetFirstChild($window); while ($inside) { Tell $inside ''; $inside = $walker.GetNextSibling($inside) } }
function Find($property, $is) { $window.FindFirst('Descendants', (New-Object ($uia + 'PropertyCondition') $property, $is)) }
function Named($name) { Find $element::NameProperty $name }
"#;

    /// Starts Windows PowerShell with a script for window `hwnd`. Its output comes through the
    /// channel line by line; it reads from the stdin handed back.
    fn reader(hwnd: HANDLE, script: &str) -> (std::process::ChildStdin, std::sync::mpsc::Receiver<String>) {
        use std::io::BufRead;
        use std::process::Stdio;
        let script = format!("{READER}{script}").replace("WINDOW", &hwnd.to_string());
        let mut reader = std::process::Command::new("powershell.exe");
        reader.args(["-NoProfile", "-NonInteractive", "-Command", &script]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        let mut reader = reader.spawn().expect("Windows PowerShell, to read the window with");
        let (written, lines) = (reader.stdout.take().unwrap(), std::sync::mpsc::channel());
        let to_reader = reader.stdin.take().unwrap();
        std::thread::spawn(move || {
            std::io::BufReader::new(written).lines().map_while(Result::ok).for_each(|line| _ = lines.0.send(line));
            _ = reader.wait();
        });
        (to_reader, lines.1)
    }
    /// The client's lines up to one starting with `last` (or its end). Its queries reach the
    /// window between the window's messages: they are dispatched as they come, as `run`'s loop
    /// does.
    fn read(lines: &std::sync::mpsc::Receiver<String>, last: &str) -> String {
        let (mut read, started) = (Vec::new(), Instant::now());
        loop {
            pump();
            match lines.try_recv() {
                Ok(line) if line.starts_with(last) => return read.join("\n"),
                Ok(line) => read.push(line),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return read.join("\n"),
                Err(_) => assert!(started.elapsed().as_secs() < 120, "the reader does not come to an end"),
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    /// What the client reports of the form at first.
    const AT_FIRST: &str = "Text [Sound]
CheckBox [Loud] Off
Slider [] at 4 of 0 to 10
Edit [Your name] holds []
Button [Save] to press
Button [More] Collapsed
RadioButton [One] picked: True
RadioButton [Two] picked: False";

    #[test]
    fn a_program_that_reads_windows_out_is_told_what_is_in_the_window() {
        use std::io::Write;
        let app = Rc::new(RefCell::new(Form { level: 4.0, ..Form::default() }));
        let hwnd = form(&app);
        // It reports everything in the window, keeps the button, and waits for a line.
        let script = "All; $kept = Named 'Save'; 'kept [' + $kept.Current.Name + '] to press: ' + [bool](Way $kept 'Invoke'); 'waiting'; $null = [Console]::In.ReadLine()
try { $name = $kept.Current.Name; $press = [bool](Way $kept 'Invoke') } catch { $name = ''; $press = $false }
'kept [' + $name + '] to press: ' + $press";
        let (mut to_reader, lines) = reader(hwnd, script);
        assert_eq!(read(&lines, "waiting"), format!("{AT_FIRST}\nkept [Save] to press: True"));
        assert!(state(hwnd).ui.listening);
        // The window is destroyed while the client still holds an element: asked about it, the
        // client learns it is gone.
        unsafe { DestroyWindow(hwnd) };
        writeln!(to_reader).unwrap();
        assert_eq!(read(&lines, "the end"), "kept [] to press: False");
    }

    #[test]
    fn a_program_that_reads_windows_out_works_the_window() {
        // A client acting on a window first asks the element to take focus, and Windows then
        // brings that window to the front, on its desktop. So the test below runs on a private
        // desktop in a process of its own, and the user at the computer keeps their keyboard.
        on_a_desktop_of_its_own("windows::tests::on_a_desktop_nobody_sees_a_reader_works_the_window");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_a_reader_works_the_window() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(is_on_its_own_desktop(), "this is the desktop the person works on: nothing is worked on it");
        let app = Rc::new(RefCell::new(Form { level: 4.0, ..Form::default() }));
        let hwnd = form(&app);
        // It checks, types, slides, expands, picks and presses, then reports everything again.
        let script = "
(Way (Named 'Loud') 'Toggle').Toggle()
(Way (Named 'Your name') 'Value').SetValue('Ada')
(Way (Find $element::ControlTypeProperty ([type]($uia + 'ControlType'))::Slider) 'RangeValue').SetValue(6)
(Way (Named 'More') 'ExpandCollapse').Expand()
(Way (Named 'Two') 'SelectionItem').Select()
(Way (Named 'Save') 'Invoke').Invoke()
All";
        let (_to_reader, lines) = reader(hwnd, script);
        let after = "Text [Sound]
CheckBox [Loud] On
Slider [] at 6 of 0 to 10
Edit [Your name] holds [Ada]
Button [Save] to press <- keyboard
Button [More] Expanded
Text [Under the fold]
RadioButton [One] picked: False
RadioButton [Two] picked: True";
        assert_eq!(read(&lines, "the end"), after);
        let stands = (app.borrow().loud, app.borrow().level, app.borrow().name.clone(), app.borrow().more, app.borrow().choice, app.borrow().saved);
        assert_eq!(stands, (true, 6.0, "Ada".to_string(), true, 1, 1));
        unsafe { DestroyWindow(hwnd) };
    }

    /// The window-kind tests' app state: where its move strip and the strip's button are; what
    /// to ask of its window next run, once each; and what the window reported last run.
    #[derive(Default)]
    struct Kinds {
        strip: Rect,
        button: Rect,
        /// The app names no strip.
        no_strip: bool,
        /// To ask: minimize, come forward, maximize or restore, fullscreen or not.
        minimize: bool,
        forward: bool,
        fill: Option<bool>,
        over_all: Option<bool>,
        /// It is maximized; it is fullscreen.
        fills: bool,
        whole: bool,
    }

    /// A window as `run` creates it, of the kind `asked` says, whose app draws its own title
    /// strip across the top with a button at its end. Not shown by this.
    fn of_a_kind(asked: Window, app: &Rc<RefCell<Kinds>>) -> HANDLE {
        let app = app.clone();
        let describe = move |ui: &mut Ui| {
            let mut app = app.borrow_mut();
            let strip = ui.row().h(Length::Px(30.0)).sense().open();
            ui.label("A title");
            ui.spacer();
            app.button = ui.button("Close").rect;
            ui.close();
            if !app.no_strip {
                ui.set_title_bar(&strip);
            }
            if std::mem::take(&mut app.minimize) {
                ui.minimize();
            }
            if std::mem::take(&mut app.forward) {
                ui.bring_to_front();
            }
            app.fill.take().into_iter().for_each(|on| ui.set_maximized(on));
            app.over_all.take().into_iter().for_each(|on| ui.set_full_screen(on));
            (app.strip, app.fills, app.whole) = (strip.rect, ui.maximized(), ui.full_screen());
            ui.label("The page");
        };
        let hwnd = open(asked, Box::new(describe));
        unsafe { (window_proc(hwnd, WM_SIZE, 0, 0), window_proc(hwnd, WM_SIZE, 0, 0)) };
        hwnd
    }

    /// What a window says lies at a point given in units from its client corner, asked as
    /// Windows asks: with the screen position.
    fn lies_at(hwnd: HANDLE, at: Point) -> isize {
        let place = on_screen(hwnd, at);
        unsafe { window_proc(hwnd, WM_NCHITTEST, 0, (place.y as u16 as isize) << 16 | place.x as u16 as isize) }
    }
    /// A window's client area on screen.
    fn inside_on_screen(hwnd: HANDLE) -> RECT {
        let ((wide, high), mut corner) = (inside(hwnd), POINT::default());
        unsafe { ClientToScreen(hwnd, &mut corner) };
        RECT { left: corner.x, top: corner.y, right: corner.x + wide, bottom: corner.y + high }
    }
    /// The style bits giving a window's frame.
    fn frame_of(hwnd: HANDLE) -> u32 {
        unsafe { GetWindowLongPtrW(hwnd, -16) as u32 & WS_OVERLAPPEDWINDOW }
    }

    #[test]
    fn a_window_with_no_frame_is_all_inside_and_says_what_lies_at_each_place() {
        // Never shown: none of this needs it.
        let app = Rc::new(RefCell::new(Kinds::default()));
        let hwnd = of_a_kind(Window { width: 300.0, height: 200.0, frameless: true, ..Window::default() }, &app);
        // All of it is client, as big as its app asked, at its screen's scale. Its style is
        // still a resizable framed one, so Windows resizes and snaps it.
        let (scale, all) = (kept(hwnd).scale, outside(hwnd));
        assert_eq!((inside(hwnd), inside_on_screen(hwnd)), (((300.0 * scale) as i32, (200.0 * scale) as i32), all));
        assert_eq!(frame_of(hwnd), WS_OVERLAPPEDWINDOW);
        // Its strip is a caption to Windows (2), so Windows moves, snaps and
        // double-click-maximizes the window by it. The strip's button and the page under it are
        // client (1).
        let (button, on_strip) = (app.borrow().button, pt(150.0, 15.0));
        assert_eq!([lies_at(hwnd, on_strip), lies_at(hwnd, middle(button)), lies_at(hwnd, pt(150.0, 100.0))], [2, 1, 1]);
        // Its edges and corners are Windows' resize handles: left and right, top and its two
        // corners, bottom and its two.
        let edges = [(0.0, 100.0, 10), (299.0, 100.0, 11), (150.0, 0.0, 12), (0.0, 0.0, 13), (299.0, 0.0, 14), (150.0, 199.0, 15), (0.0, 199.0, 16), (299.0, 199.0, 17)];
        for (x, y, part) in edges {
            assert_eq!(lies_at(hwnd, pt(x, y)), part, "at {x}, {y}");
        }
        // Its least size is its client's least: it has no frame to add.
        let mut limits = MINMAXINFO { reserved: POINT::default(), max_size: POINT::default(), max_position: POINT::default(), min_track: POINT::default(), max_track: POINT::default() };
        unsafe { window_proc(hwnd, WM_GETMINMAXINFO, 0, &raw mut limits as isize) };
        assert_eq!(limits.min_track, POINT { x: (200.0 * scale) as i32, y: (120.0 * scale) as i32 });
        // An app that stops naming a strip has a window moved by nothing; its edges are still
        // its edges.
        app.borrow_mut().no_strip = true;
        unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
        assert_eq!([lies_at(hwnd, on_strip), lies_at(hwnd, pt(0.0, 100.0))], [1, 10]);
        unsafe { DestroyWindow(hwnd) };

        // A framed window is moved by the box its app names too, and by its title bar as ever;
        // its edges are the frame's, not its own.
        let app = Rc::new(RefCell::new(Kinds::default()));
        let hwnd = of_a_kind(Window { width: 300.0, height: 200.0, ..Window::default() }, &app);
        let (within, all) = (inside_on_screen(hwnd), outside(hwnd));
        assert!(within.top > all.top && within.left > all.left && frame_of(hwnd) == WS_OVERLAPPEDWINDOW, "{within:?} in {all:?}");
        assert_eq!([lies_at(hwnd, on_strip), lies_at(hwnd, pt(0.0, 100.0)), lies_at(hwnd, pt(150.0, 100.0)), lies_at(hwnd, pt(150.0, -4.0))], [2, 1, 1, 2]);
        // And one whose app names no strip is asked nothing of the kind: hit testing is
        // Windows' own, as always.
        app.borrow_mut().no_strip = true;
        unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
        assert!(!kept(hwnd).asks_hit_test && lies_at(hwnd, on_strip) == 1);
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn a_window_is_minimized_fills_its_screen_and_lies_over_all_of_it_when_its_app_asks() {
        // Shown for real, maximized and made fullscreen: on a private desktop nobody sees.
        on_a_desktop_of_its_own("windows::tests::on_a_desktop_nobody_sees_a_window_stands_as_its_app_asks");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_a_window_stands_as_its_app_asks() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(is_on_its_own_desktop(), "this is the desktop the person works on: no window is shown on it");
        // Frameless, and framed.
        [true, false].into_iter().for_each(stands_as_asked);
    }

    /// Has a shown window do everything an app can ask of its state, checking window, Windows
    /// and interface against each other at each step.
    fn stands_as_asked(bare: bool) {
        let app = Rc::new(RefCell::new(Kinds::default()));
        let hwnd = of_a_kind(Window { position: Some((120, 90)), width: 400.0, height: 240.0, frameless: bare, ..Window::default() }, &app);
        unsafe { ShowWindow(hwnd, 1) };
        pump();
        let ui = || &state(hwnd).ui;
        // The app asks in its next run, which the window is woken for; the result is heard and
        // run for.
        let asks = |ask: &dyn Fn(&mut Kinds)| {
            ask(&mut app.borrow_mut());
            unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
            pump();
        };
        let (zoomed, iconic) = (|| unsafe { IsZoomed(hwnd) } != 0, || unsafe { IsIconic(hwnd) } != 0);
        let stands = || (app.borrow().fills, app.borrow().whole, ui().maximized(), ui().full_screen());
        let mut screen = MONITORINFO { size: size_of::<MONITORINFO>() as u32, all: RECT::default(), work: RECT::default(), flags: 0 };
        assert!(unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, 2), &mut screen) } != 0);
        let (there, small, within, on_strip) = (WindowPlacement { position: (120, 90), width: 400.0, height: 240.0, maximized: false }, outside(hwnd), inside_on_screen(hwnd), pt(200.0, 15.0));
        assert_eq!((ui().window_placement(), stands(), bare, lies_at(hwnd, on_strip)), (Some(there), (false, false, false, false), within == small, 2));

        // Minimized by its app, it is, and its interface knows; it comes back as it was.
        asks(&|app| app.minimize = true);
        assert!(iconic() && ui().window_minimized());
        asks(&|app| app.forward = true);
        assert!(!iconic() && !ui().window_minimized() && outside(hwnd) == small);

        // Asked to maximize, it does, and says so with its restore place. A frameless window is
        // then placed so its missing frame lies past the screen's edges: its client is the work
        // area, with no resize edges.
        asks(&|app| app.fill = Some(true));
        assert_eq!((zoomed(), stands(), ui().window_placement()), (true, (true, false, true, false), Some(WindowPlacement { maximized: true, ..there })));
        let filled = (outside(hwnd), inside_on_screen(hwnd));
        assert!(!bare || (filled.1 == screen.work && filled.0.left < screen.work.left && filled.0.bottom > screen.work.bottom), "{filled:?} on {:?}", screen.work);
        assert_eq!([lies_at(hwnd, pt(1.0, 1.0)), lies_at(hwnd, on_strip)], [2, 2]);
        // A double click on the strip, as Windows passes one on a caption to the window
        // (`WM_NCLBUTTONDBLCLK` 0xA3, with 2), restores it, and another maximizes it again:
        // what the system title bar does.
        unsafe { window_proc(hwnd, 0x00A3, 2, 0) };
        pump();
        assert_eq!((zoomed(), stands(), ui().window_placement(), outside(hwnd)), (false, (false, false, false, false), Some(there), small));
        unsafe { window_proc(hwnd, 0x00A3, 2, 0) };
        pump();
        assert_eq!((zoomed(), stands(), outside(hwnd)), (true, (true, false, true, false), filled.0));
        asks(&|app| app.fill = Some(false));
        assert_eq!((zoomed(), stands(), ui().window_placement(), outside(hwnd)), (false, (false, false, false, false), Some(there), small));

        // Asked to go fullscreen, it does, frameless, its client the screen. Its place still
        // says where it restores to; nothing of it can be grabbed; asked to maximize meanwhile,
        // it stays as is.
        asks(&|app| app.over_all = Some(true));
        assert_eq!((outside(hwnd), inside_on_screen(hwnd), frame_of(hwnd)), (screen.all, screen.all, 0));
        assert_eq!((stands(), ui().window_placement(), [lies_at(hwnd, on_strip), lies_at(hwnd, pt(0.0, 0.0))]), ((false, true, false, true), Some(there), [1, 1]));
        asks(&|app| app.fill = Some(true));
        assert_eq!((zoomed(), outside(hwnd)), (false, screen.all));
        // And back: where it was, as big, framed or not as before.
        asks(&|app| app.over_all = Some(false));
        assert_eq!((outside(hwnd), inside_on_screen(hwnd), frame_of(hwnd)), (small, within, WS_OVERLAPPEDWINDOW));
        assert_eq!((stands(), ui().window_placement(), lies_at(hwnd, on_strip)), ((false, false, false, false), Some(there), 2));

        // From maximized it goes fullscreen, says it restores to maximized, and does.
        asks(&|app| app.fill = Some(true));
        asks(&|app| app.over_all = Some(true));
        assert_eq!((outside(hwnd), inside_on_screen(hwnd), stands(), ui().window_placement()), (screen.all, screen.all, (true, true, true, true), Some(WindowPlacement { maximized: true, ..there })));
        asks(&|app| app.over_all = Some(false));
        assert_eq!((zoomed(), outside(hwnd), inside_on_screen(hwnd), stands()), (true, filled.0, filled.1, (true, false, true, false)));
        // Minimized while fullscreen and asked to leave fullscreen, it stays minimized, and
        // comes back as before: maximized.
        asks(&|app| app.over_all = Some(true));
        asks(&|app| app.minimize = true);
        assert!(iconic() && ui().window_minimized() && ui().full_screen());
        asks(&|app| app.over_all = Some(false));
        assert!(iconic() && !ui().full_screen());
        asks(&|app| app.forward = true);
        assert_eq!((iconic(), zoomed(), outside(hwnd), frame_of(hwnd)), (false, true, filled.0, WS_OVERLAPPEDWINDOW));
        asks(&|app| app.fill = Some(false));
        assert_eq!((outside(hwnd), ui().window_placement()), (small, Some(there)));
        unsafe { DestroyWindow(hwnd) };
    }

    /// A see-through window's picture, rows from the top.
    fn pictured(hwnd: HANDLE) -> (Vec<u32>, usize) {
        let clear = kept(hwnd).transparent.as_ref().expect("a see-through window");
        let [wide, high] = clear.size.map(|side| side as usize);
        (unsafe { std::slice::from_raw_parts(clear.pixels, wide * high) }.to_vec(), wide)
    }
    /// A full redraw of a window's interface, on a clear background.
    fn afresh(hwnd: HANDLE) -> Vec<u32> {
        state(hwnd).in_window(0, |ui, _| crate::Surface::draw(ui, 64)).unwrap().pixels
    }

    /// The see-through test app draws an opaque square, a half-transparent one, a line of text,
    /// and a check box the test checks.
    fn see_through(asked: Window, ticked: &Rc<Cell<bool>>) -> HANDLE {
        let ticked = ticked.clone();
        let describe = move |ui: &mut Ui| {
            let mut on = ticked.get();
            ui.el().size(40.0, 40.0).fill(Color::rgb(0xff0000)).show();
            ui.el().size(40.0, 40.0).fill(Color::rgb(0x0000ff).with_alpha(0.5)).show();
            ui.label("Written");
            ui.checkbox(&mut on, "Ticked");
        };
        let hwnd = open(Window { width: 200.0, height: 160.0, transparent: true, ..asked }, Box::new(describe));
        unsafe { (window_proc(hwnd, WM_SIZE, 0, 0), window_proc(hwnd, WM_SIZE, 0, 0)) };
        hwnd
    }

    #[test]
    fn a_see_through_window_is_shown_from_a_picture_that_is_clear_where_nothing_is_drawn() {
        // Never shown: the picture is what Windows gets, and it is read here.
        let ticked = Rc::new(Cell::new(false));
        let hwnd = see_through(Window::default(), &ticked);
        // It is layered (style -20 bit 0x0008_0000), frameless, and not resizable by the user:
        // all client.
        let (scale, all) = (kept(hwnd).scale, outside(hwnd));
        assert!(unsafe { GetWindowLongPtrW(hwnd, -20) } & 0x0008_0000 != 0 && unsafe { GetWindowLongPtrW(hwnd, -16) } as u32 & (WS_PLAIN | WS_OVERLAPPEDWINDOW) == WS_PLAIN);
        assert_eq!((inside(hwnd), inside_on_screen(hwnd), lies_at(hwnd, pt(0.0, 80.0))), (((200.0 * scale) as i32, (160.0 * scale) as i32), all, 1));
        // The picture is the window's size. Where the app drew opaque it is opaque,
        // half-transparent it is half (premultiplied), and where it drew nothing there is
        // nothing.
        let ((pixels, wide), px) = (pictured(hwnd), |unit: f32| (unit * scale) as usize);
        let at = |x: f32, y: f32| pixels[px(y) * wide + px(x)];
        assert_eq!((pixels.len(), wide), ((all.right - all.left) as usize * (all.bottom - all.top) as usize, (all.right - all.left) as usize));
        assert_eq!((at(20.0, 20.0), at(150.0, 20.0), at(150.0, 150.0)), (0xffff_0000, 0, 0));
        let half = at(20.0, 60.0);
        assert!((0x7f..=0x80).contains(&(half >> 24)) && half & 0xff == half >> 24 && half & 0x00ff_ff00 == 0, "{half:08x}");
        // Letters have per-pixel alpha like everything else: some of the picture is neither
        // clear nor opaque, and no pixel has more colour than alpha.
        let solid = |pixel: &u32| pixel >> 24;
        assert!(pixels.iter().all(|pixel| [16, 8, 0].iter().all(|shift| pixel >> shift & 0xff <= solid(pixel))), "a colour not multiplied by how solid it is");
        let row = px(90.0) * wide..px(100.0) * wide;
        assert!(pixels[row].iter().any(|pixel| (1..255).contains(&solid(pixel))), "no letters in the picture");
        // It equals a full redraw; and after a change, redrawing only that part, it still does.
        assert!(pixels == afresh(hwnd));
        ticked.set(true);
        unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
        let (ticked_pixels, _) = pictured(hwnd);
        assert!(ticked_pixels != pixels && ticked_pixels == afresh(hwnd));
        // Windows takes the picture when handed it.
        let whole = RECT { left: 0, top: 0, right: all.right - all.left, bottom: all.bottom - all.top };
        assert!(kept(hwnd).transparent.as_ref().is_some_and(|clear| clear.show(hwnd, whole)));
        // A window of another size has a picture of that size, drawn afresh.
        unsafe { SetWindowPos(hwnd, 0, 0, 0, whole.right + 30, whole.bottom + 20, 0x0016) };
        pump();
        let (larger, wider) = pictured(hwnd);
        assert_eq!((wider, larger.len()), (wide + 30, (wide + 30) * (whole.bottom as usize + 20)));
        assert!(larger == afresh(hwnd));
        unsafe { DestroyWindow(hwnd) };

        // One also bare is resized by its edges, as a frameless window is.
        let hwnd = see_through(Window { frameless: true, ..Window::default() }, &ticked);
        assert_eq!((frame_of(hwnd), lies_at(hwnd, pt(0.0, 80.0)), inside_on_screen(hwnd)), (WS_OVERLAPPEDWINDOW, 10, outside(hwnd)));
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn a_second_window_has_no_frame_where_its_app_asks_for_that() {
        // Never shown, like the other second windows of these tests.
        let kept_by_app = Rc::new(RefCell::new((SecondWindow::new(200.0, 120.0), false)));
        let app = kept_by_app.clone();
        let describe = move |ui: &mut Ui| {
            let (tools, close) = &mut *app.borrow_mut();
            ui.label("The first window");
            if ui.frameless_window(tools, "Tools") {
                let strip = ui.row().h(Length::Px(24.0)).sense().open();
                ui.label("Tools");
                ui.close();
                ui.set_title_bar(&strip);
                if *close {
                    ui.quit();
                }
                ui.close();
            }
        };
        let first = open(Window::default(), Box::new(describe));
        unsafe { (window_proc(first, WM_SIZE, 0, 0), window_proc(first, WM_SIZE, 0, 0)) };
        let [_, second] = windows(first)[..] else { panic!("{} windows", windows(first).len()) };
        // The second window is all client, as big as its app asked; it moves by the strip its
        // app names and resizes by its edges. The first keeps its frame.
        let scale = kept(second).scale;
        assert_eq!((inside(second), inside_on_screen(second)), (((200.0 * scale) as i32, (120.0 * scale) as i32), outside(second)));
        assert_eq!([lies_at(second, pt(100.0, 12.0)), lies_at(second, pt(100.0, 60.0)), lies_at(second, pt(199.0, 60.0)), lies_at(second, pt(199.0, 119.0))], [2, 1, 11, 17]);
        assert!(inside_on_screen(first) != outside(first) && !kept(first).asks_hit_test);
        // Its app closes it from inside, as its close button would.
        kept_by_app.borrow_mut().1 = true;
        unsafe { window_proc(first, WM_WAKE, 0, 0) };
        pump();
        assert_eq!((kept_by_app.borrow().0.open, windows(first), unsafe { IsWindow(second) }), (false, vec![first], 0));
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn what_is_clear_of_a_see_through_window_lets_the_pointer_through() {
        // Shown for real: on a private desktop nobody sees.
        on_a_desktop_of_its_own("windows::tests::on_a_desktop_nobody_sees_a_see_through_window_lets_the_pointer_through");
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_a_see_through_window_lets_the_pointer_through() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(is_on_its_own_desktop(), "this is the desktop the person works on: no window is shown on it");
        // As `run` shows a see-through window, here over a plain window of another kind.
        let under = of_a_kind(Window { position: Some((100, 100)), width: 500.0, height: 400.0, ..Window::default() }, &Rc::new(RefCell::new(Kinds::default())));
        let hwnd = see_through(Window { position: Some((200, 200)), ..Window::default() }, &Rc::new(Cell::new(false)));
        unsafe { (ShowWindow(under, 1), ShowWindow(hwnd, 1)) };
        pump();
        // It opened where its app said and is shown: Windows has its picture.
        let scale = kept(hwnd).scale;
        assert!(unsafe { IsWindowVisible(hwnd) } != 0 && corner_of(hwnd) == POINT { x: 200, y: 200 });
        // Which window the pointer would be over at a point of it: the see-through one where
        // opaque or half transparent, the one behind where clear.
        let over = |x: f32, y: f32| unsafe { WindowFromPoint(POINT { x: 200 + (x * scale) as i32, y: 200 + (y * scale) as i32 }) };
        assert_eq!([over(20.0, 20.0), over(20.0, 60.0), over(150.0, 20.0), over(150.0, 150.0)], [hwnd, hwnd, under, under]);
        unsafe { (DestroyWindow(hwnd), DestroyWindow(under)) };
    }
    // ---- screens, second-window kinds, and closing ----

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindow(hwnd: HANDLE, which: u32) -> HANDLE;
        fn IsWindowEnabled(hwnd: HANDLE) -> i32;
        fn MonitorFromPoint(at: POINT, otherwise: u32) -> HANDLE;
        fn ShutdownBlockReasonQuery(hwnd: HANDLE, why: *mut u16, room: *mut u32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn K32GetProcessMemoryInfo(process: HANDLE, counters: *mut [usize; 10], size: u32) -> i32;
    }

    /// These tests' app state: its second windows, whether it has unsaved changes, and what it
    /// heard.
    #[derive(Default)]
    struct Owned {
        windows: Vec<(SecondWindow, &'static str)>,
        unsaved: bool,
        /// The windows that heard the user ask them to close, and how many runs there were.
        asked: Vec<&'static str>,
        runs: u32,
    }

    /// A first window as `run` creates it, as `asked`, never shown, whose app describes its
    /// second windows; with unsaved changes each window refuses to close.
    fn owned(asked: Window, app: &Rc<RefCell<Owned>>) -> HANDLE {
        let app = app.clone();
        let describe = move |ui: &mut Ui| {
            let mut app = app.borrow_mut();
            let app = &mut *app;
            app.runs += 1;
            let mut hear = |ui: &mut Ui, name: &'static str| {
                if ui.close_requested() && !app.asked.contains(&name) {
                    app.asked.push(name);
                }
                if app.unsaved {
                    ui.cancel_close("There are unsaved changes");
                }
            };
            hear(ui, "first");
            ui.label("The first window");
            for (state, title) in app.windows.iter_mut() {
                if ui.window(state, title) {
                    hear(ui, title);
                    ui.label(title);
                    ui.close();
                }
            }
        };
        let hwnd = open(asked, Box::new(describe));
        unsafe { (window_proc(hwnd, WM_SIZE, 0, 0), window_proc(hwnd, WM_SIZE, 0, 0)) };
        hwnd
    }

    /// The screen Windows has at a desktop point, as `screens` reports it.
    fn screen_at(found: &[Screen], x: f32, y: f32) -> Screen {
        let monitor = unsafe { MonitorFromPoint(POINT { x: x as i32, y: y as i32 }, 2) };
        let mut info = MONITORINFO { size: size_of::<MONITORINFO>() as u32, all: RECT::default(), work: RECT::default(), flags: 0 };
        unsafe { GetMonitorInfoW(monitor, &mut info) };
        *found.iter().find(|screen| screen.rect.x as i32 == info.all.left && screen.rect.y as i32 == info.all.top).expect("the screen is in the list")
    }

    #[test]
    fn the_screens_are_the_ones_windows_has_and_a_window_hears_when_they_change() {
        let found = screens();
        eprintln!("{found:#?}");
        // As many as Windows counts (`SM_CMONITORS`, 80), each with its work area inside it, at
        // its own scale; the main one, at the origin, first.
        assert_eq!((found.len(), found[0].rect.x, found[0].rect.y), (unsafe { GetSystemMetrics(80) } as usize, 0.0, 0.0));
        for screen in &found {
            let (monitor, mut dpi) = (unsafe { MonitorFromPoint(POINT { x: (screen.rect.x + 1.0) as i32, y: (screen.rect.y + 1.0) as i32 }, 0) }, [0u32; 2]);
            unsafe { GetDpiForMonitor(monitor, 0, &mut dpi[0], &mut dpi[1]) };
            assert!(screen.rect.intersect(screen.work_area) == screen.work_area && !screen.work_area.is_empty(), "{screen:?}");
            assert_eq!(screen.scale, dpi[0] as f32 / 96.0);
        }
        // A window has them from the start and hears them again when Windows says the screens
        // changed (here they had not: the app does not run for it), or a work area
        // (`SPI_SETWORKAREA`, 0x2F).
        let app = Rc::new(RefCell::new(Owned::default()));
        let first = owned(Window::default(), &app);
        assert_eq!(state(first).ui.screens(), &found[..]);
        let runs = app.borrow().runs;
        unsafe { window_proc(first, WM_DISPLAYCHANGE, 32, 0) };
        assert_eq!(app.borrow().runs, runs);
        for (message, what) in [(WM_DISPLAYCHANGE, 32), (WM_SETTINGCHANGE, 0x2F)] {
            state(first).ui.set_screens(Vec::new());
            unsafe { window_proc(first, message, what, 0) };
            assert!(state(first).ui.screens() == &found[..] && app.borrow().runs > runs, "{message:x}");
        }
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn a_window_opens_in_the_middle_of_the_screen_its_app_names_at_that_screens_scale() {
        // On each screen, read back from Windows; none of the windows is shown.
        for (nth, screen) in screens().into_iter().enumerate() {
            let app = Rc::new(RefCell::new(Owned::default()));
            let first = owned(Window { screen: Some(nth), width: 400.0, height: 240.0, ..Window::default() }, &app);
            let (area, work) = (outside(first), screen.work_area);
            let off = |from: i32, to: i32, start: f32, long: f32| ((from + to) as f32 / 2.0 - (start + long / 2.0)).abs();
            assert!(off(area.left, area.right, work.x, work.w) <= 1.0 && off(area.top, area.bottom, work.y, work.h) <= 1.0, "{area:?} is not in the middle of {work:?}");
            assert_eq!((kept(first).scale, state(first).ui.screen(), state(first).ui.size()), (screen.scale, Some(nth), Size { w: 400.0, h: 240.0 }));
            unsafe { DestroyWindow(first) };
        }
        // A missing screen means the main one.
        let first = owned(Window { screen: Some(99), ..Window::default() }, &Rc::new(RefCell::new(Owned::default())));
        assert_eq!(state(first).ui.screen(), Some(0));
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn a_window_opens_where_it_was_on_the_screen_it_was_on_or_else_on_the_nearest() {
        let found = screens();
        // On each screen near its top left: just there, at that screen's scale.
        for (nth, screen) in found.iter().enumerate() {
            let at = ((screen.work_area.x + 40.0) as i32, (screen.work_area.y + 30.0) as i32);
            let first = owned(Window { position: Some(at), width: 400.0, height: 240.0, ..Window::default() }, &Rc::new(RefCell::new(Owned::default())));
            assert_eq!((corner_of(first), kept(first).scale, state(first).ui.screen()), (POINT { x: at.0, y: at.1 }, screen.scale, Some(nth)));
            unsafe { DestroyWindow(first) };
        }
        // Far right of and below every screen (a screen was removed): moved onto the nearest,
        // wholly in its work area.
        let right = found.iter().map(|screen| screen.rect.right()).fold(f32::MIN, f32::max);
        let bottom = found.iter().map(|screen| screen.rect.bottom()).fold(f32::MIN, f32::max);
        for at in [(right as i32 + 5000, 200), (300, bottom as i32 + 5000)] {
            let first = owned(Window { position: Some(at), ..Window::default() }, &Rc::new(RefCell::new(Owned::default())));
            let (area, nth) = (outside(first), crate::screens::nearest(&found, rect(at.0 as f32, at.1 as f32, 160.0, 30.0)).unwrap());
            let work = found[nth].work_area;
            assert!(area.left >= work.x as i32 && area.top >= work.y as i32 && area.right <= work.right() as i32 && area.bottom <= work.bottom() as i32, "{area:?} is not in {work:?}");
            assert_eq!((state(first).ui.screen(), kept(first).scale), (Some(nth), found[nth].scale));
            assert_eq!(screen_at(&found, (area.left + 50) as f32, (area.top + 50) as f32), found[nth]);
            unsafe { DestroyWindow(first) };
        }
    }

    #[test]
    fn second_windows_of_each_kind_stand_to_the_first_as_people_expect() {
        let app = Rc::new(RefCell::new(Owned::default()));
        app.borrow_mut().windows = vec![
            (SecondWindow::new(300.0, 200.0), "Plain"),
            (SecondWindow::new(160.0, 300.0).tool(), "Tools"),
            (SecondWindow::new(200.0, 100.0).on_top(), "Clock"),
            (SecondWindow { open: false, ..SecondWindow::new(320.0, 140.0).dialog() }, "Save?"),
        ];
        let first = owned(Window { position: Some((200, 150)), width: 700.0, height: 500.0, ..Window::default() }, &app);
        let [_, plain, tools, clock] = windows(first)[..] else { panic!("{} windows", windows(first).len()) };
        let owner = |hwnd: HANDLE| unsafe { GetWindow(hwnd, 4) };
        let (style, more) = (|hwnd: HANDLE| unsafe { GetWindowLongPtrW(hwnd, -16) } as u32, |hwnd: HANDLE| unsafe { GetWindowLongPtrW(hwnd, -20) } as u32);
        // A plain one has no owner and is on the task bar like the first.
        assert_eq!((owner(plain), more(plain) & 0x89), (0, 0));
        // A tool window is owned by the first (kept above it, minimized with it), off the task
        // bar, with no minimize or maximize button, in its own frame style. (Windows 11 gives
        // it a title as tall as a plain one's.)
        assert_eq!((owner(tools), more(tools) & 0x80, style(tools) & 0x0003_0000), (first, 0x80, 0));
        let (area, (wide, high)) = (outside(tools), inside(tools));
        let frame = framed(wide as f32 / kept(tools).scale, high as f32 / kept(tools).scale, unsafe { GetDpiForWindow(tools) }, styles_of(tools));
        assert_eq!((area.right - area.left, area.bottom - area.top), (frame.right - frame.left, frame.bottom - frame.top));
        // An always-on-top one is topmost from the start, with no owner.
        assert_eq!((owner(clock), more(clock) & 8), (0, 8));
        let enabled = || windows(first).iter().map(|hwnd| unsafe { IsWindowEnabled(*hwnd) } != 0).collect::<Vec<_>>();
        assert_eq!(enabled(), [true; 4]);
        // A dialog opens: owned by the first, centred on it; and the windows opened before it
        // get neither keyboard nor pointer until it goes.
        app.borrow_mut().windows[3].0.open = true;
        unsafe { window_proc(first, WM_WAKE, 0, 0) };
        let dialog = windows(first)[4];
        assert_eq!((owner(dialog), more(dialog) & 1, style(dialog) & 0x0003_0000), (first, 1, 0));
        let (over, under) = (outside(dialog), outside(first));
        assert!((over.left + over.right - under.left - under.right).abs() <= 1 && (over.top + over.bottom - under.top - under.bottom).abs() <= 1, "{over:?} is not in the middle of {under:?}");
        assert_eq!(enabled(), [false, false, false, false, true]);
        // It is answered and goes: the others get both again.
        app.borrow_mut().windows[3].0.open = false;
        unsafe { window_proc(first, WM_WAKE, 0, 0) };
        assert_eq!((windows(first).len(), enabled()), (4, vec![true; 4]));
        unsafe { DestroyWindow(first) };
    }

    /// The reason Windows was given for blocking shutdown, if any.
    fn reason(hwnd: HANDLE) -> Option<String> {
        let (mut text, mut room) = ([0u16; 256], 256u32);
        let said = unsafe { ShutdownBlockReasonQuery(hwnd, text.as_mut_ptr(), &mut room) } != 0;
        said.then(|| String::from_utf16_lossy(&text[..room.min(256) as usize]).trim_end_matches('\0').into())
    }

    #[test]
    fn the_app_is_asked_before_a_window_closes_and_before_the_system_logs_off() {
        let app = Rc::new(RefCell::new(Owned::default()));
        app.borrow_mut().windows = vec![(SecondWindow::new(300.0, 200.0), "Notes")];
        let first = owned(Window::default(), &app);
        let notes = windows(first)[1];
        // With unsaved changes the app refuses, to the second window's close button and the
        // first's. Both stay.
        app.borrow_mut().unsaved = true;
        unsafe { (window_proc(notes, WM_CLOSE, 0, 0), window_proc(first, WM_CLOSE, 0, 0)) };
        assert_eq!((app.borrow().asked.clone(), unsafe { (IsWindow(first), IsWindow(notes)) }), (vec!["Notes", "first"], (1, 1)));
        // The system is logging off: the app is asked, refuses, and Windows is told why.
        app.borrow_mut().asked.clear();
        assert_eq!(unsafe { window_proc(first, WM_QUERYENDSESSION, 0, 0) }, 0);
        assert_eq!((app.borrow().asked.clone(), reason(first)), (vec!["first"], Some("There are unsaved changes".into())));
        // Without them it agrees, and no reason is left with Windows.
        app.borrow_mut().unsaved = false;
        assert_eq!((unsafe { window_proc(first, WM_QUERYENDSESSION, 0, 0) }, reason(first)), (1, None));
        unsafe { window_proc(first, WM_ENDSESSION, 0, 0) };
        // The second window's close button: it goes. The first's: it goes, and the message loop
        // is told to end.
        unsafe { window_proc(notes, WM_CLOSE, 0, 0) };
        assert_eq!((unsafe { IsWindow(notes) }, app.borrow().windows[0].0.open), (0, false));
        unsafe { window_proc(first, WM_CLOSE, 0, 0) };
        assert_eq!((unsafe { IsWindow(first) }, pump()), (0, true));
    }

    #[test]
    fn a_window_moved_onto_a_screen_of_another_scale_keeps_its_size_in_units() {
        let app = Rc::new(RefCell::new(Owned::default()));
        app.borrow_mut().windows = vec![(SecondWindow::new(300.0, 200.0).tool(), "Tools")];
        let first = owned(Window { width: 400.0, height: 240.0, frameless: true, ..Window::default() }, &app);
        for (at, hwnd) in windows(first).into_iter().enumerate() {
            for dpi in [144usize, 192, 120, 96] {
                let units = state(first).in_window(at, |ui, _| ui.size()).unwrap();
                // Windows asks its size at that scale: the same client size in units, in its
                // frame.
                let mut big = POINT::default();
                assert_eq!(unsafe { window_proc(hwnd, WM_GETDPISCALEDSIZE, dpi, &raw mut big as isize) }, 1);
                let frame = framed(units.w, units.h, dpi as u32, styles_of(hwnd));
                assert_eq!(big, POINT { x: frame.right - frame.left, y: frame.bottom - frame.top });
                // And puts it there at that size (far off every screen), at the new scale. A
                // frameless window keeps its size in units. (A framed one keeps the frame
                // Windows really draws, its actual screen's: here every screen has one scale,
                // so this message cannot show it keeping its size. Its interface is the size of
                // the client inside that frame.)
                let to = RECT { left: FAR.x, top: FAR.y, right: FAR.x + big.x, bottom: FAR.y + big.y };
                unsafe { window_proc(hwnd, WM_DPICHANGED, dpi | dpi << 16, &raw const to as isize) };
                let (scale, (wide, high)) = (dpi as f32 / 96.0, inside(hwnd));
                let size = if styles_of(hwnd).is_none() { units } else { Size { w: wide as f32 / scale, h: high as f32 / scale } };
                assert_eq!((outside(hwnd), state(first).in_window(at, |ui, _| (ui.size(), ui.scale()))), (to, Some((size, scale))), "{dpi}");
            }
        }
        unsafe { DestroyWindow(first) };
    }

    #[test]
    #[ignore = "a measurement: run it by itself, so that no other test's windows are counted"]
    fn how_long_ten_second_windows_take_and_what_they_hold() {
        // The program's private bytes (the tenth field of `PROCESS_MEMORY_COUNTERS_EX`), and
        // its USER (1) and GDI (0) object counts.
        let held = || {
            let mut counters = [0usize; 10];
            counters[0] = size_of::<[usize; 10]>();
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size_of::<[usize; 10]>() as u32) };
            let taken = |which: u32| unsafe { GetGuiResources(GetCurrentProcess(), which) } as usize;
            [counters[9], taken(1), taken(0)]
        };
        let titles = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];
        let app = Rc::new(RefCell::new(Owned::default()));
        app.borrow_mut().windows = titles.iter().map(|title| (SecondWindow { open: false, ..SecondWindow::new(400.0, 300.0) }, *title)).collect();
        let first = owned(Window::default(), &app);
        // A run of the app: whatever is open is described.
        let timed = |rounds: u32| {
            let started = Instant::now();
            (0..rounds).for_each(|_| _ = unsafe { window_proc(first, WM_WAKE, 0, 0) });
            started.elapsed() / rounds
        };
        let open = |on: bool| {
            app.borrow_mut().windows.iter_mut().for_each(|(state, _)| state.open = on);
            unsafe { window_proc(first, WM_WAKE, 0, 0) };
        };
        // Counted before any window ever opened: memory a closed one freed stays with the
        // program for the next.
        let none = held();
        open(true);
        let ten = held();
        open(false);
        let alone = timed(500);
        open(true);
        let busy = timed(500);
        open(false);
        let (after, closed) = (held(), timed(500));
        let each = |was: [usize; 3], now: [usize; 3]| now.map(|n| n as f64).iter().zip(was).map(|(now, was)| (now - was as f64) / 10.0).collect::<Vec<_>>();
        eprintln!("a run with no second window: {alone:?}; with ten: {busy:?}; with the ten closed again: {closed:?}");
        eprintln!("held with none: {none:?}; with ten: {ten:?}; after: {after:?}; an interface is {} bytes", size_of::<Ui>());
        eprintln!("each window (bytes of memory, windows and the like, things to draw with): {:?}", each(none, ten));
        assert_eq!(windows(first).len(), 1);
        assert!(after[1] <= none[1] && after[2] <= none[2], "closed windows left something behind: {none:?} {after:?}");
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn text_dragged_over_a_window_whose_app_takes_text_reaches_it() {
        // An app asking about text, not files.
        let seen = Rc::new(RefCell::new((false, None::<String>, Rect::EMPTY)));
        let described = seen.clone();
        let app = move |ui: &mut Ui| {
            let mut seen = described.borrow_mut();
            let on = ui.button("Let text go here");
            seen.0 = ui.text_hovered();
            seen.1 = seen.1.take().or(ui.text_dropped(&on));
            seen.2 = on.rect;
        };
        let hwnd = open(Window::default(), Box::new(app));
        unsafe { (window_proc(hwnd, WM_SIZE, 0, 0), window_proc(hwnd, WM_SIZE, 0, 0)) };
        assert!(state(hwnd).ole == Some(true) && kept(hwnd).files, "a window that takes text takes it the way it takes files");
        let (data_table, files) = (Dragged::table(), in_memory(&block((0, 0), false, true, &["C:\\one.txt"])));
        let (mut text, mut file) = (Dragged::of_text(&data_table, "words, ünïcode 𝄞"), Dragged::new(&data_table, files));
        let (text, file): (*mut IDataObject, *mut IDataObject) = (&raw mut text.object, &raw mut file.object);
        let (target, at) = (DropTarget::new(hwnd), on_screen(hwnd, middle(seen.borrow().2)));
        let (calls, mut effect) = (table(target), ANY);
        unsafe {
            // Files are none of its business.
            assert_eq!(((calls.enter)(target, file, 0, at, &mut effect), effect, seen.borrow().0), (0, 0, false));
            (calls.leave)(target);
            // Text is, dropped on its box.
            effect = ANY;
            assert_eq!(((calls.enter)(target, text, 0, at, &mut effect), effect, seen.borrow().0), (0, COPY, true));
            assert_eq!(((calls.let_go)(target, text, 0, at, &mut effect), effect), (0, COPY));
            assert_eq!((seen.borrow().0, seen.borrow().1.as_deref()), (false, Some("words, ünïcode 𝄞")));
            // Dropped beside the box, it is not taken, and its source hears so.
            (effect, seen.borrow_mut().1) = (ANY, None);
            (calls.enter)(target, text, 0, at, &mut effect);
            assert_eq!(((calls.let_go)(target, text, 0, on_screen(hwnd, pt(500.0, 400.0)), &mut effect), effect, seen.borrow().1.clone()), (0, 0, None));
            assert_eq!((calls.release)(target), 0);
            GlobalFree(files);
            DestroyWindow(hwnd);
        }
    }

    #[test]
    fn text_handed_over_ends_at_its_first_zero_or_its_block() {
        let block = |units: &[u16]| in_memory(&units.iter().flat_map(|unit| unit.to_le_bytes()).collect::<Vec<u8>>());
        let (whole, cut, odd) = (block(&[0x77, 0xfc, 0, 0x78]), block(&[0x77, 0xd834]), in_memory(&[0x77, 0, 0x78]));
        assert_eq!(unsafe { [dropped_text(whole), dropped_text(cut), dropped_text(odd), dropped_text(0)] }, ["wü", "w\u{fffd}", "w", ""]);
        unsafe { [whole, cut, odd].into_iter().for_each(|block| _ = GlobalFree(block)) };
    }

    /// The drag-out tests' app state. Its first window has a drag source box (the number 7
    /// inside the window, `what` outside it) and a drop box; its second window, while open, a
    /// box taking files and text.
    #[derive(Default)]
    struct Out {
        what: Option<DragData<'static>>,
        from: Rect,
        on: Rect,
        /// What became of what was dragged out; what was dropped on the box, as a number.
        taken: Vec<DropEffect>,
        dropped: Option<u64>,
        /// The second window, its box, and the files and text dropped on it.
        takes: SecondWindow,
        took: Rect,
        files: Vec<PathBuf>,
        text: Option<String>,
    }

    /// The first window of such an app as `run` creates it, its corner at `corner` if given;
    /// not shown.
    fn drag_source_window(app: &Rc<RefCell<Out>>, corner: Option<(i32, i32)>) -> HANDLE {
        let app = app.clone();
        let describe = move |ui: &mut Ui| {
            let mut app = app.borrow_mut();
            let app = &mut *app;
            ui.column().pad(10.0).gap(10.0).open();
            let from = ui.button("RunDrag me");
            if ui.drag_source(&from, 7) {
                ui.label("seven");
                ui.close();
            }
            app.taken.extend(app.what.and_then(|what| ui.drag_out(&from, what)));
            let on = ui.button("Let go here");
            app.dropped = app.dropped.or(ui.dropped(&on));
            // So the first window has a drop target of its own too.
            _ = ui.files_hovered();
            (app.from, app.on) = (from.rect, on.rect);
            ui.close();
            if ui.window(&mut app.takes, "Takes") {
                let took = ui.button("Let go in here");
                app.files.extend(ui.files_dropped(&took).into_iter().flatten());
                app.text = app.text.take().or(ui.text_dropped(&took));
                app.took = took.rect;
                ui.close();
            }
        };
        let hwnd = open(Window { position: corner, width: 300.0, height: 200.0, ..Window::default() }, Box::new(describe));
        unsafe { (window_proc(hwnd, WM_SIZE, 0, 0), window_proc(hwnd, WM_SIZE, 0, 0)) };
        hwnd
    }

    /// The user grabs the first box of such a window and drags it a step, then on to a point
    /// given in window units: by the messages Windows sends a window.
    fn take_out(hwnd: HANDLE, app: &Rc<RefCell<Out>>, to: Point) {
        let at = middle(app.borrow().from);
        unsafe {
            window_proc(hwnd, WM_MOUSEMOVE, 0, place(hwnd, at));
            window_proc(hwnd, WM_LBUTTONDOWN, 1, place(hwnd, at));
            window_proc(hwnd, WM_MOUSEMOVE, 1, place(hwnd, pt(at.x + 10.0, at.y)));
            window_proc(hwnd, WM_MOUSEMOVE, 1, place(hwnd, to));
        }
    }

    thread_local! {
        /// What a stand-in for Windows' `DoDragDrop` saw on its way.
        static SAW: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        /// The window a stand-in drags over, and where on screen.
        static OVER: Cell<(HANDLE, POINT)> = const { Cell::new((0, POINT { x: 0, y: 0 })) };
    }
    /// `DRAGDROP_S_DROP` and `DRAGDROP_S_CANCEL`.
    const LET_GO: i32 = 0x0004_0100;
    const GIVEN_UP: i32 = 0x0004_0101;

    /// Function `at` of the dragged data object, as Windows calls it: through its vtable
    /// pointer.
    unsafe fn function<F: Copy>(data: *mut IDataObject, at: usize) -> F {
        unsafe { std::mem::transmute_copy(&*(*data.cast::<*const usize>()).add(at)) }
    }

    /// `DoDragDrop` to a file manager that reads the files, moves them itself, and reports so
    /// as Explorer does: no effect as Windows sees it, a move as the user sees it.
    unsafe fn moved_by_a_file_manager(data: *mut crate::ole_drag::DataObject, source: *mut crate::ole_drag::Source, allowed: u32, effect: *mut u32) -> i32 {
        let (data, mut handed) = (data.cast::<IDataObject>(), STGMEDIUM { medium: 0, memory: 0, owner: std::ptr::null_mut() });
        unsafe {
            assert_eq!((((*(*data).table).query_get_data)(data, &FILES), ((*(*data).table).get_data)(data, &FILES, &mut handed)), (0, 0));
            SAW.with_borrow_mut(|saw| saw.extend(dropped_files(handed.memory).1.iter().map(|file| file.display().to_string())));
            ReleaseStgMedium(&mut handed);
            // The user drags it along, and drops.
            assert_eq!((crate::ole_drag::go_on(source, false, 1), crate::ole_drag::go_on(source, false, 0), allowed), (0, LET_GO, 7));
            let set_data: unsafe extern "system" fn(*mut IDataObject, *const FORMATETC, *mut STGMEDIUM, i32) -> i32 = function(data, 7);
            let said = FORMATETC { format: RegisterClipboardFormatW(wide("Logical Performed DropEffect").as_ptr()) as u16, ..FILES };
            let mut moved = STGMEDIUM { medium: 1, memory: in_memory(&2u32.to_le_bytes()), owner: std::ptr::null_mut() };
            assert_eq!(set_data(data, &said, &mut moved, 1), 0);
            *effect = 0;
        }
        LET_GO
    }

    /// `DoDragDrop` until the user cancels with Escape.
    unsafe fn given_up(_: *mut crate::ole_drag::DataObject, source: *mut crate::ole_drag::Source, _: u32, _: *mut u32) -> i32 {
        assert_eq!(crate::ole_drag::go_on(source, true, 1), GIVEN_UP);
        GIVEN_UP
    }

    /// `DoDragDrop` over a window of this program (`OVER`), whose drop target hears it as
    /// Windows reports, then dropping there. (It sees what the drop target says it may do.)
    unsafe fn let_go_over_a_window(data: *mut crate::ole_drag::DataObject, source: *mut crate::ole_drag::Source, allowed: u32, effect: *mut u32) -> i32 {
        let ((hwnd, at), data) = (OVER.get(), data.cast::<IDataObject>());
        unsafe {
            let target = GetPropW(hwnd, wide("OleDropTargetInterface").as_ptr()) as *mut DropTarget;
            let (calls, mut may) = (table(target), allowed);
            (calls.enter)(target, data, 1, at, &mut may);
            (calls.over)(target, 1, at, &mut may);
            SAW.with_borrow_mut(|saw| saw.push(format!("may: {may}")));
            assert_eq!(crate::ole_drag::go_on(source, false, 0), LET_GO);
            // Where nothing may be done, Windows only says it left.
            *effect = 0;
            if may == 0 {
                (calls.leave)(target);
                return LET_GO;
            }
            may = allowed;
            (calls.let_go)(target, data, 0, at, &mut may);
            *effect = may;
        }
        LET_GO
    }

    #[test]
    fn files_dragged_out_of_a_window_go_to_windows_between_messages_and_its_app_hears_they_were_moved() {
        let paths: &'static [PathBuf] = Box::leak(Box::new([PathBuf::from("C:\\a\\one.txt"), PathBuf::from("D:\\zwei drei\\für.png")]));
        let app = Rc::new(RefCell::new(Out { what: Some(DragData::Files(paths)), ..Out::default() }));
        let hwnd = drag_source_window(&app, None);
        // Dragged out of the window, it waits until the message that took it there is handled.
        // (This window is never shown: whatever the pointer is over, it is not this one.)
        take_out(hwnd, &app, pt(320.0, 50.0));
        assert_eq!(OUTGOING.get().map(|(from, ..)| from), Some(hwnd));
        assert!(!state(hwnd).ui.drag_leaves_window(), "it is out already");
        let was = crate::ole_drag::RUN_DRAG.replace(moved_by_a_file_manager);
        run_queued_drag_out();
        crate::ole_drag::RUN_DRAG.set(was);
        assert!(OUTGOING.get().is_none());
        assert_eq!(SAW.take(), ["C:\\a\\one.txt", "D:\\zwei drei\\für.png"]);
        // The app heard once that they were moved; nothing was dropped in the window; the
        // button is up for it; and the window releases capture.
        assert_eq!((app.borrow().taken.clone(), app.borrow().dropped), (vec![DropEffect::Moved], None));
        unsafe { window_proc(hwnd, WM_WAKE, 0, 0) };
        assert_eq!(app.borrow().taken, [DropEffect::Moved]);
        assert!(state(hwnd).ui.drag_payload().is_none() && !state(hwnd).ui.wants_pointer() && unsafe { GetCapture() } != hwnd);
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn what_is_given_up_with_escape_is_let_go_nowhere() {
        let app = Rc::new(RefCell::new(Out { what: Some(DragData::Text("words")), ..Out::default() }));
        let hwnd = drag_source_window(&app, None);
        take_out(hwnd, &app, pt(150.0, 250.0));
        let was = crate::ole_drag::RUN_DRAG.replace(given_up);
        run_queued_drag_out();
        crate::ole_drag::RUN_DRAG.set(was);
        assert_eq!((app.borrow().taken.clone(), app.borrow().dropped), (vec![], None));
        assert!(state(hwnd).ui.drag_payload().is_none() && unsafe { GetCapture() } != hwnd);
        // In the library's own tests Windows drags nothing: a drag out is dropped at once, as
        // cancelled.
        take_out(hwnd, &app, pt(150.0, 250.0));
        run_queued_drag_out();
        assert_eq!((app.borrow().taken.clone(), app.borrow().dropped, state(hwnd).ui.drag_payload()), (vec![], None, None));
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn what_one_window_drags_out_another_window_of_the_program_takes() {
        let app = Rc::new(RefCell::new(Out { what: Some(DragData::Text("über alles")), takes: SecondWindow::new(200.0, 120.0), ..Out::default() }));
        let first = drag_source_window(&app, None);
        let [_, second] = windows(first)[..] else { panic!("{} windows", windows(first).len()) };
        assert!(state(first).ole == Some(true) && kept(second).files, "the second window takes files and text");
        take_out(first, &app, pt(320.0, 50.0));
        OVER.set((second, on_screen(second, middle(app.borrow().took))));
        let was = crate::ole_drag::RUN_DRAG.replace(let_go_over_a_window);
        run_queued_drag_out();
        assert_eq!((SAW.take(), app.borrow().text.clone(), app.borrow().taken.clone()), (vec!["may: 1".to_string()], Some("über alles".into()), vec![DropEffect::Copied]));
        // Over its source window, its drop target leaves it alone: there it is that window's
        // own (see the private-desktop test), not text from elsewhere.
        app.borrow_mut().taken.clear();
        take_out(first, &app, pt(320.0, 50.0));
        OVER.set((first, on_screen(first, middle(app.borrow().on))));
        run_queued_drag_out();
        crate::ole_drag::RUN_DRAG.set(was);
        assert_eq!((SAW.take(), app.borrow().taken.clone(), app.borrow().dropped), (vec!["may: 0".to_string()], vec![], None));
        // Files from the second window to the first, the same way.
        let paths: &'static [PathBuf] = Box::leak(Box::new([PathBuf::from("C:\\a\\one.txt")]));
        app.borrow_mut().what = Some(DragData::Files(paths));
        take_out(first, &app, pt(320.0, 50.0));
        OVER.set((second, on_screen(second, middle(app.borrow().took))));
        let was = crate::ole_drag::RUN_DRAG.replace(let_go_over_a_window);
        run_queued_drag_out();
        crate::ole_drag::RUN_DRAG.set(was);
        assert_eq!((SAW.take(), app.borrow().files.clone(), app.borrow().taken.clone()), (vec!["may: 1".to_string()], paths.to_vec(), vec![DropEffect::Copied]));
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn what_is_dragged_out_comes_back_over_its_window_and_goes_out_over_a_window_lying_on_it() {
        // Shown for real: on a private desktop nobody sees.
        on_a_desktop_of_its_own("windows::tests::on_a_desktop_nobody_sees_what_is_dragged_out_come_back_and_go_out_over_a_window_on_it");
    }

    /// `DoDragDrop` out of window `OVER` and back over it at the screen point, dropping there.
    /// (It sees the cursor, and where the window's interface heard the pointer was.)
    unsafe fn home_again(_: *mut crate::ole_drag::DataObject, source: *mut crate::ole_drag::Source, _: u32, effect: *mut u32) -> i32 {
        let (hwnd, at) = OVER.get();
        let off = POINT { x: at.x, y: at.y + 2000 };
        unsafe {
            for place in [off, at] {
                let looks = (*source).looks_at(place);
                SAW.with_borrow_mut(|saw| saw.push(format!("{looks:x} {:?} {:?}", state(hwnd).ui.pointer(), state(hwnd).ui.drag_payload())));
            }
            assert_eq!(crate::ole_drag::go_on(source, false, 0), LET_GO);
            *effect = 0;
        }
        LET_GO
    }

    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_what_is_dragged_out_come_back_and_go_out_over_a_window_on_it() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        assert!(is_on_its_own_desktop(), "this is the desktop the person works on: no window is shown on it");
        let app = Rc::new(RefCell::new(Out { what: Some(DragData::Text("words")), takes: SecondWindow::new(200.0, 120.0).position(300, 150), ..Out::default() }));
        let first = drag_source_window(&app, Some((100, 100)));
        let [_, second] = windows(first)[..] else { panic!("{} windows", windows(first).len()) };
        unsafe { (ShowWindow(first, 1), ShowWindow(second, 1)) };
        pump();
        let scale = kept(first).scale;
        // A point of the first window's client where the second window lies over it, and one
        // where nothing does, in first-window units.
        let (mut corner, beside) = (POINT::default(), pt(20.0, 150.0));
        unsafe { ClientToScreen(first, &mut corner) };
        let covered = pt((320 - corner.x) as f32 / scale, (200 - corner.y) as f32 / scale);
        assert_eq!(unsafe { (WindowFromPoint(POINT { x: 320, y: 200 }), WindowFromPoint(on_screen(first, beside))) }, (second, first));
        // Dragged inside the window, it is the window's; nothing goes out.
        take_out(first, &app, beside);
        assert!(OUTGOING.get().is_none() && state(first).ui.drag_payload() == Some(7) && state(first).ui.drag_leaves_window());
        // Over the window lying on it, it goes out.
        unsafe { window_proc(first, WM_MOUSEMOVE, 1, place(first, covered)) };
        assert_eq!(OUTGOING.get().map(|(from, ..)| from), Some(first));
        // Windows drags it off the window and back over its box, and drops there: the window's
        // interface hears where the pointer is, and has the number dropped there.
        let on = on_screen(first, middle(app.borrow().on));
        OVER.set((first, on));
        let was = crate::ole_drag::RUN_DRAG.replace(home_again);
        run_queued_drag_out();
        crate::ole_drag::RUN_DRAG.set(was);
        let at = middle(app.borrow().on);
        assert_eq!(SAW.take(), ["40102 None Some(7)".to_string(), format!("0 Some({at:?}) Some(7)")]);
        assert_eq!((app.borrow().dropped, app.borrow().taken.clone()), (Some(7), vec![]));
        assert!(state(first).ui.drag_payload().is_none() && unsafe { GetCapture() } != first);
        unsafe { DestroyWindow(first) };
    }

    #[test]
    fn a_see_through_window_with_no_picture_draws_nothing() {
        // A picture Windows could not make, or has not made yet, has no pixels to draw into.
        let mut ui = Ui::new(crate::platform::system_text(), Box::new(crate::platform::LocalClipboard::default()));
        let mut clear = SeeThrough::new();
        clear.draw(&mut ui, rect(0.0, 0.0, 10.0, 10.0), 1.0);
        assert!(clear.pixels.is_null());
        // A window of no size has none either, and runs as usual.
        let app = |ui: &mut Ui| _ = ui.el().size(10.0, 10.0).fill(Color::BLACK).show();
        let hwnd = open(Window { transparent: true, width: 0.0, height: 0.0, ..Default::default() }, Box::new(app));
        assert!(hwnd != 0 && unsafe { IsWindowVisible(hwnd) } == 0);
        unsafe { (window_proc(hwnd, WM_WAKE, 0, 0), window_proc(hwnd, WM_WAKE, 0, 0)) };
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn a_waker_wakes_no_window_once_the_window_is_gone() {
        let hwnd = open(Window::default(), Box::new(|ui: &mut Ui| _ = ui.label("Woken")));
        // Other tests open windows meanwhile: what is checked is that this one is not woken
        // once gone, its handle maybe soon another's.
        unsafe { DestroyWindow(hwnd) };
        assert_ne!(WINDOW.load(std::sync::atomic::Ordering::Relaxed), hwnd);
        Waker.wake();
    }
}
