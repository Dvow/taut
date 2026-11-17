//! Dragging out of a window into another program, on Windows: through OLE drag and drop.
//!
//! The payload is a `Carried`, an `IDataObject`: the target program asks it which formats it
//! offers and for itself in one (files as a file list a file manager reads, UTF-16 text, a
//! picture as a PNG file and as a DIB, a link as a URL and as text). Each format is produced on
//! first request and cached. With it goes a `Source`, an `IDropSource`, which Windows asks
//! while dragging whether to continue and how the cursor should look. Both go to `DoDragDrop`,
//! which tracks the mouse until the button is released or Escape pressed, and reports what the
//! target did. Both are hand-written COM objects, as `uia`'s are, each freeing itself on its
//! last release.
//!
//! While dragged over its own window, that window's interface hears the pointer as for an
//! internal drag: so it is an internal drag again there, dropped there as usual.
//!
//! `DoDragDrop` is reached only through `CARRY`, which a test replaces: the real call follows
//! the real mouse and drops on whatever is under it. (The library's own tests never reach it.)
use crate::drag_drop::{DragData, DropEffect, OwnedDragData};
use crate::windows::sys::*;
use crate::windows::{carried_over, in_memory, to_block, to_drop, wide};
use std::cell::{Cell, OnceCell};
use std::ffi::c_void;

#[cfg(not(test))]
#[link(name = "ole32")]
unsafe extern "system" {
    fn DoDragDrop(data: *mut c_void, source: *mut c_void, allowed: u32, effect: *mut u32) -> i32;
}
#[cfg(not(test))]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
}

/// `DROPEFFECT`s the target may perform: copy (1), move (2), link (4). All are allowed; the
/// target chooses.
const COPY: u32 = 1;
const MOVE: u32 = 2;
const LINK: u32 = 4;
/// `DoDragDrop`'s results: dropped (on something that may or may not have accepted it), or
/// cancelled. And the source's request for the system's default cursors.
const DROP: i32 = 0x0004_0100;
const CANCEL: i32 = 0x0004_0101;
const USUAL_POINTERS: i32 = 0x0004_0102;
/// COM results: `DV_E_FORMATETC`, `DV_E_TYMED`, `DV_E_DVASPECT`, `E_NOTIMPL`,
/// `OLE_E_ADVISENOTSUPPORTED`, `E_NOINTERFACE`, and `S_FALSE`.
const NO_FORM: i32 = 0x8004_0064_u32 as i32;
const NO_MEDIUM: i32 = 0x8004_0069_u32 as i32;
const NO_ASPECT: i32 = 0x8004_006B_u32 as i32;
const NOT_DONE: i32 = 0x8000_4001_u32 as i32;
const NOT_TOLD: i32 = 0x8004_0003_u32 as i32;
const NO_INTERFACE: i32 = 0x8000_4002_u32 as i32;
const FEWER: i32 = 1;

/// The standard clipboard formats every program knows: `CF_DIB` (8), `CF_UNICODETEXT` (13),
/// `CF_HDROP` (15). Others are registered by name on first use.
const PIXELS: u16 = 8;
const TEXT: u16 = 13;
const FILES: u16 = 15;
fn form_named(name: &str) -> u16 {
    // SAFETY: the name is NUL-terminated.
    unsafe { RegisterClipboardFormatW(wide(name).as_ptr()) as u16 }
}

/// A COM object of this module: it starts with its vtable pointer and reference count, and is
/// identified by its IID's first number (the rest is shared by all of them and by `IUnknown`,
/// whose number is 0).
trait Com {
    const NAME: u32;
}

/// `QueryInterface` for any of them: this same object, with one more reference, or null.
unsafe extern "system" fn query<T: Com>(this: *mut T, name: *const [u32; 4], out: *mut *mut T) -> i32 {
    // SAFETY: Windows passes the IID and room for the answer.
    unsafe {
        let known = matches!(name.read_unaligned(), [number, 0, 0xC0, 0x4600_0000] if number == 0 || number == T::NAME);
        *out = if known { this } else { std::ptr::null_mut() };
        if !known {
            return NO_INTERFACE;
        }
        hold(this);
    }
    0
}

/// The reference count of an object of this module, which each holds second, after its vtable
/// pointer.
///
/// # Safety
/// `this` is such an object, and alive.
unsafe fn held<'a, T>(this: *mut T) -> &'a Cell<u32> {
    // SAFETY: as the caller promises; each is `#[repr(C)]`, the count after one pointer.
    unsafe { &*this.cast::<u8>().add(size_of::<usize>()).cast::<Cell<u32>>() }
}

/// `AddRef`.
unsafe extern "system" fn hold<T>(this: *mut T) -> u32 {
    // SAFETY: the caller holds the object, so it is alive.
    let held = unsafe { held(this) };
    held.set(held.get() + 1);
    held.get()
}

/// `Release`; frees the object on the last.
unsafe extern "system" fn release<T>(this: *mut T) -> u32 {
    // SAFETY: the caller holds the object, so it is alive. It was made by `Box::into_raw`, and
    // nothing uses it after the last release.
    unsafe {
        let held = held(this);
        held.set(held.get() - 1);
        let left = held.get();
        if left == 0 {
            drop(Box::from_raw(this));
        }
        left
    }
}

/// A drag payload: an `IDataObject`, which other programs ask for its content in a format they
/// know.
#[repr(C)]
pub(crate) struct Carried {
    table: &'static CarriedTable,
    held: Cell<u32>,
    what: OwnedDragData,
    /// Its formats, by clipboard format number, each with its bytes once produced.
    forms: Vec<(u16, OnceCell<Vec<u8>>)>,
    /// What the drop target reports doing, where it does (`SetData`, in two registered
    /// formats): the effect Windows sees, and the one the user sees. A file manager that moves
    /// files itself reports it took nothing, and that it moved them.
    done: Cell<[Option<u32>; 2]>,
}

/// `IDataObject`'s vtable, in the order Windows declares it.
#[repr(C)]
struct CarriedTable {
    query: unsafe extern "system" fn(*mut Carried, *const [u32; 4], *mut *mut Carried) -> i32,
    hold: unsafe extern "system" fn(*mut Carried) -> u32,
    release: unsafe extern "system" fn(*mut Carried) -> u32,
    get_data: unsafe extern "system" fn(*mut Carried, *const FORMATETC, *mut STGMEDIUM) -> i32,
    get_data_here: unsafe extern "system" fn(*mut Carried, *const FORMATETC, *mut STGMEDIUM) -> i32,
    query_get_data: unsafe extern "system" fn(*mut Carried, *const FORMATETC) -> i32,
    canonical: unsafe extern "system" fn(*mut Carried, *const FORMATETC, *mut FORMATETC) -> i32,
    set_data: unsafe extern "system" fn(*mut Carried, *const FORMATETC, *mut STGMEDIUM, i32) -> i32,
    forms: unsafe extern "system" fn(*mut Carried, u32, *mut *mut Forms) -> i32,
    advise: unsafe extern "system" fn(*mut Carried, *const FORMATETC, u32, *mut c_void, *mut u32) -> i32,
    unadvise: unsafe extern "system" fn(*mut Carried, u32) -> i32,
    advices: unsafe extern "system" fn(*mut Carried, *mut *mut c_void) -> i32,
}

static CARRIED: CarriedTable = CarriedTable {
    query: query::<Carried>,
    hold: hold::<Carried>,
    release: release::<Carried>,
    get_data: Carried::get_data,
    get_data_here: Carried::not_done,
    query_get_data: Carried::query_get_data,
    canonical: Carried::canonical,
    set_data: Carried::set_data,
    forms: Carried::forms,
    advise: Carried::advise,
    unadvise: Carried::unadvise,
    advices: Carried::advices,
};

impl Com for Carried {
    const NAME: u32 = 0x10E;
}

impl Carried {
    /// A drag payload owning a copy of everything in it, with one reference: the caller's. Its
    /// formats: files as a file list; text as text; a picture as a PNG file, keeping alpha, and
    /// as a DIB every program reads; a link as a URL (which browsers take as one) and as text.
    pub(crate) fn new(what: DragData) -> *mut Carried {
        let what = OwnedDragData::of(what);
        let forms = match what {
            OwnedDragData::Files(_) => vec![FILES],
            OwnedDragData::Text(_) => vec![TEXT],
            OwnedDragData::Picture(..) => vec![form_named("PNG"), PIXELS],
            OwnedDragData::Link(_) => vec![form_named("UniformResourceLocatorW"), TEXT],
        };
        let forms = forms.into_iter().map(|form| (form, OnceCell::new())).collect();
        Box::into_raw(Box::new(Carried { table: &CARRIED, held: Cell::new(1), what, forms, done: Cell::new([None; 2]) }))
    }

    /// Its bytes in one format.
    fn bytes(&self, form: u16) -> Vec<u8> {
        match &self.what {
            OwnedDragData::Files(files) => to_drop(files),
            OwnedDragData::Text(text) | OwnedDragData::Link(text) => wide(text).into_iter().flat_map(u16::to_le_bytes).collect(),
            OwnedDragData::Picture(width, pixels) if form == PIXELS => to_block(*width, pixels),
            OwnedDragData::Picture(width, pixels) => crate::png::encode_rgba(*width, pixels).unwrap_or_default(),
        }
    }

    /// Which of its formats is asked for, if it supports the request: the full content
    /// (`DVASPECT_CONTENT`), in global memory (`TYMED_HGLOBAL`, among the acceptable media).
    fn form(&self, asked: &FORMATETC) -> Result<usize, i32> {
        let at = self.forms.iter().position(|(form, _)| *form == asked.format).ok_or(NO_FORM)?;
        match (asked.aspect, asked.medium & 1) {
            (1, 1) => Ok(at),
            (1, _) => Err(NO_MEDIUM),
            _ => Err(NO_ASPECT),
        }
    }

    /// `GetData`: itself in one format, in global memory the caller frees (no release callback:
    /// no owner).
    unsafe extern "system" fn get_data(this: *mut Carried, asked: *const FORMATETC, out: *mut STGMEDIUM) -> i32 {
        // SAFETY: Windows passes this object, the request, and room for the answer.
        let (carried, asked) = unsafe { (&*this, &*asked) };
        let at = match carried.form(asked) {
            Ok(at) => at,
            Err(why) => return why,
        };
        let (form, bytes) = &carried.forms[at];
        let memory = in_memory(bytes.get_or_init(|| carried.bytes(*form)));
        if memory == 0 {
            return 0x8007_000E_u32 as i32; // E_OUTOFMEMORY
        }
        // SAFETY: the room is Windows' own, for this answer.
        unsafe { *out = STGMEDIUM { medium: 1, memory, owner: std::ptr::null_mut() } };
        0
    }

    /// `QueryGetData`.
    unsafe extern "system" fn query_get_data(this: *mut Carried, asked: *const FORMATETC) -> i32 {
        // SAFETY: Windows passes this object and the request.
        let (carried, asked) = unsafe { (&*this, &*asked) };
        carried.form(asked).map_or_else(|why| why, |_| 0)
    }

    /// `GetCanonicalFormatEtc`: none here; every format suits every device.
    unsafe extern "system" fn canonical(_: *mut Carried, _: *const FORMATETC, out: *mut FORMATETC) -> i32 {
        // SAFETY: Windows passes room for the answer, of which only the device is set.
        if !out.is_null() {
            unsafe { (*out).device = std::ptr::null() };
        }
        NOT_DONE
    }

    /// `SetData`, from the drop target. Of everything it may say, two are heard: the effect it
    /// performed, as Windows sees it and as the user does, each a DWORD in global memory. (The
    /// medium is released here when it is ours to release.)
    unsafe extern "system" fn set_data(this: *mut Carried, said: *const FORMATETC, medium: *mut STGMEDIUM, ours: i32) -> i32 {
        // SAFETY: Windows passes this object, the format and the medium; the memory is locked
        // while its DWORD is read, no further than its size.
        unsafe {
            let (carried, said) = (&*this, &*said);
            let which = [form_named("Performed DropEffect"), form_named("Logical Performed DropEffect")].iter().position(|form| *form == said.format);
            let Some(which) = which.filter(|_| (*medium).medium == 1) else { return NOT_DONE };
            let start = GlobalLock((*medium).memory).cast::<u8>();
            if !start.is_null() && GlobalSize((*medium).memory) >= 4 {
                let mut done = carried.done.get();
                done[which] = Some(u32::from_le_bytes(*start.cast::<[u8; 4]>()));
                carried.done.set(done);
            }
            GlobalUnlock((*medium).memory);
            if ours != 0 {
                ReleaseStgMedium(medium);
            }
        }
        0
    }

    /// `EnumFormatEtc`: an enumerator of its formats.
    unsafe extern "system" fn forms(this: *mut Carried, direction: u32, out: *mut *mut Forms) -> i32 {
        // SAFETY: Windows passes this object and room for the answer. (`DATADIR_GET`: the
        // formats it offers; none can be set.)
        let carried = unsafe { &*this };
        if direction != 1 {
            return NOT_DONE;
        }
        let list = carried.forms.iter().map(|(format, _)| FORMATETC { format: *format, device: std::ptr::null(), aspect: 1, index: -1, medium: 1 }).collect();
        unsafe { *out = Box::into_raw(Box::new(Forms { table: &FORMS, held: Cell::new(1), list, at: Cell::new(0) })) };
        0
    }

    /// `GetDataHere`, `DAdvise`: not supported; the payload never changes.
    unsafe extern "system" fn not_done(_: *mut Carried, _: *const FORMATETC, _: *mut STGMEDIUM) -> i32 {
        NOT_DONE
    }
    unsafe extern "system" fn advise(_: *mut Carried, _: *const FORMATETC, _: u32, _: *mut c_void, _: *mut u32) -> i32 {
        NOT_TOLD
    }
    unsafe extern "system" fn unadvise(_: *mut Carried, _: u32) -> i32 {
        NOT_TOLD
    }
    unsafe extern "system" fn advices(_: *mut Carried, _: *mut *mut c_void) -> i32 {
        NOT_TOLD
    }

    /// What the drop target did: what it reported, where it did, else what Windows says
    /// (`effect`). A link counts as a copy to the app: what it handed out stays as is.
    fn taken(&self, effect: u32) -> Option<DropEffect> {
        let [done, seen] = self.done.get();
        match seen.or(done).filter(|done| *done != 0).unwrap_or(effect) {
            0 => None,
            done if done & MOVE != 0 => Some(DropEffect::Moved),
            _ => Some(DropEffect::Copied),
        }
    }
}

/// An enumerator of a payload's formats: an `IEnumFORMATETC`.
#[repr(C)]
struct Forms {
    table: &'static FormsTable,
    held: Cell<u32>,
    list: Vec<FORMATETC>,
    /// Its position in the list.
    at: Cell<usize>,
}

/// `IEnumFORMATETC`'s vtable, in the order Windows declares it.
#[repr(C)]
struct FormsTable {
    query: unsafe extern "system" fn(*mut Forms, *const [u32; 4], *mut *mut Forms) -> i32,
    hold: unsafe extern "system" fn(*mut Forms) -> u32,
    release: unsafe extern "system" fn(*mut Forms) -> u32,
    next: unsafe extern "system" fn(*mut Forms, u32, *mut FORMATETC, *mut u32) -> i32,
    skip: unsafe extern "system" fn(*mut Forms, u32) -> i32,
    reset: unsafe extern "system" fn(*mut Forms) -> i32,
    clone: unsafe extern "system" fn(*mut Forms, *mut *mut Forms) -> i32,
}

static FORMS: FormsTable = FormsTable { query: query::<Forms>, hold: hold::<Forms>, release: release::<Forms>, next: Forms::next, skip: Forms::skip, reset: Forms::reset, clone: Forms::clone };

impl Com for Forms {
    const NAME: u32 = 0x103;
}

impl Forms {
    /// `Next`: the next `count` formats, or as many as remain, into the given room, with how
    /// many where asked.
    unsafe extern "system" fn next(this: *mut Forms, count: u32, out: *mut FORMATETC, got: *mut u32) -> i32 {
        // SAFETY: Windows passes this object, and room for `count` formats and a count.
        let forms = unsafe { &*this };
        let left = forms.list.get(forms.at.get()..).unwrap_or_default();
        let given = left.len().min(count as usize);
        unsafe {
            std::ptr::copy_nonoverlapping(left.as_ptr(), out, given);
            if !got.is_null() {
                *got = given as u32;
            }
        }
        forms.at.set(forms.at.get() + given);
        if given == count as usize { 0 } else { FEWER }
    }
    /// `Skip`.
    unsafe extern "system" fn skip(this: *mut Forms, count: u32) -> i32 {
        // SAFETY: Windows passes this object.
        let forms = unsafe { &*this };
        let to = forms.at.get().saturating_add(count as usize);
        forms.at.set(to.min(forms.list.len()));
        if to <= forms.list.len() { 0 } else { FEWER }
    }
    /// `Reset`.
    unsafe extern "system" fn reset(this: *mut Forms) -> i32 {
        // SAFETY: Windows passes this object.
        unsafe { (*this).at.set(0) };
        0
    }
    /// `Clone`: a second enumerator at the same position.
    unsafe extern "system" fn clone(this: *mut Forms, out: *mut *mut Forms) -> i32 {
        // SAFETY: Windows passes this object and room for the answer.
        unsafe {
            let forms = &*this;
            *out = Box::into_raw(Box::new(Forms { table: &FORMS, held: Cell::new(1), list: forms.list.clone(), at: Cell::new(forms.at.get()) }));
        }
        0
    }
}

/// A drag's source, as Windows queries it while dragging: an `IDropSource`.
#[repr(C)]
pub(crate) struct Source {
    table: &'static SourceTable,
    held: Cell<u32>,
    /// The window dragged from; the pointer's screen position over that window when Windows
    /// last asked about the cursor (none when not over it); and whether the user cancelled.
    hwnd: HANDLE,
    home: Cell<Option<POINT>>,
    given_up: Cell<bool>,
}

/// `IDropSource`'s vtable, in the order Windows declares it.
#[repr(C)]
struct SourceTable {
    query: unsafe extern "system" fn(*mut Source, *const [u32; 4], *mut *mut Source) -> i32,
    hold: unsafe extern "system" fn(*mut Source) -> u32,
    release: unsafe extern "system" fn(*mut Source) -> u32,
    go_on: unsafe extern "system" fn(*mut Source, i32, u32) -> i32,
    looks: unsafe extern "system" fn(*mut Source, u32) -> i32,
}

static SOURCE: SourceTable = SourceTable { query: query::<Source>, hold: hold::<Source>, release: release::<Source>, go_on: Source::go_on, looks: Source::looks };

impl Com for Source {
    const NAME: u32 = 0x121;
}

impl Source {
    /// The source of a drag out of window `hwnd`, with one reference.
    pub(crate) fn new(hwnd: HANDLE) -> *mut Source {
        Box::into_raw(Box::new(Source { table: &SOURCE, held: Cell::new(1), hwnd, home: Cell::new(None), given_up: Cell::new(false) }))
    }

    /// `QueryContinueDrag`, on every key or button change: Escape cancels, and the left button
    /// released (no longer among the held buttons, 1) drops.
    unsafe extern "system" fn go_on(this: *mut Source, escape: i32, held: u32) -> i32 {
        // SAFETY: Windows passes this object.
        let source = unsafe { &*this };
        match (escape != 0, held & 1 != 0) {
            (true, _) => {
                source.given_up.set(true);
                CANCEL
            }
            (false, false) => DROP,
            (false, true) => 0,
        }
    }

    /// `GiveFeedback`, on every move, with the effect the current target would perform: shown
    /// with the system cursors, over another program's window.
    unsafe extern "system" fn looks(this: *mut Source, _effect: u32) -> i32 {
        let mut at = POINT::default();
        // SAFETY: Windows passes this object, and the pointer position is filled in.
        unsafe {
            GetCursorPos(&mut at);
            (*this).looks_at(at)
        }
    }

    /// The same, with the pointer at a screen point. Over its own window the payload is that
    /// window's again: its interface hears the pointer, and the cursor is the in-window drag
    /// cursor (32646, `IDC_SIZEALL`). Leaving it again, the interface hears the pointer left.
    pub(crate) fn looks_at(&self, at: POINT) -> i32 {
        let home = over(self.hwnd, at).then_some(at);
        if home.is_some() || self.home.get().is_some() {
            carried_over(self.hwnd, home);
        }
        self.home.set(home);
        if home.is_none() {
            return USUAL_POINTERS;
        }
        // SAFETY: a system cursor, by its id.
        unsafe { SetCursor(LoadCursorW(0, std::ptr::without_provenance(32646))) };
        0
    }
}

/// Whether a screen point is on window `hwnd`, with nothing over it.
fn over(hwnd: HANDLE, at: POINT) -> bool {
    // SAFETY: a plain query about the screen.
    unsafe { WindowFromPoint(at) == hwnd }
}

/// How a drag out of a window runs: Windows' `DoDragDrop`, or a test's replacement. It gets the
/// payload, its source, the allowed effects, and room for the performed effect.
pub(crate) type Carry = unsafe fn(*mut Carried, *mut Source, u32, *mut u32) -> i32;

thread_local! {
    /// How drags out of this thread's windows run.
    pub(crate) static CARRY: Cell<Carry> = Cell::new(BY);
    /// The payload being dragged out now, and its window: for that window's own drop target,
    /// which defers to the source (see `Source::looks_at`).
    static GOING: Cell<(*mut Carried, HANDLE)> = const { Cell::new((std::ptr::null_mut(), 0)) };
}

/// Whether a drag out of a window of this thread is running: meanwhile the program's OLE use
/// continues, even if the first window is gone.
pub(crate) fn carrying() -> bool {
    !GOING.get().0.is_null()
}

/// Windows runs the drag, if its mouse button is really held: physically, not just as a window
/// was told. (`DoDragDrop` drops at once with no button held, wherever the real pointer is.)
/// With swapped mouse buttons (`SM_SWAPBUTTON`, 23), the dragging button is physically the
/// right one.
#[cfg(not(test))]
const BY: Carry = by_windows;
#[cfg(not(test))]
unsafe fn by_windows(data: *mut Carried, source: *mut Source, allowed: u32, effect: *mut u32) -> i32 {
    // SAFETY: plain queries; and the objects are alive, held by the caller.
    unsafe {
        let button = if GetSystemMetrics(23) != 0 { 2 } else { 1 };
        if GetAsyncKeyState(button) >= 0 {
            (*source).given_up.set(true);
            return CANCEL;
        }
        DoDragDrop(data.cast(), source.cast(), allowed, effect)
    }
}
/// In the library's own tests Windows never runs a drag: a test dragging out installs its own
/// replacement.
#[cfg(test)]
const BY: Carry = by_nobody;
#[cfg(test)]
unsafe fn by_nobody(_: *mut Carried, _: *mut Source, _: u32, _: *mut u32) -> i32 {
    CANCEL
}

/// Whether `data` is the payload being dragged out of window `hwnd` now.
pub(crate) fn is_going(data: *const c_void, hwnd: HANDLE) -> bool {
    GOING.get() == (data.cast_mut().cast(), hwnd)
}

/// The source's answer to `QueryContinueDrag` (see `Source::go_on`), for the window tests.
#[cfg(test)]
pub(crate) fn go_on(source: *mut Source, escape: bool, held: u32) -> i32 {
    unsafe { Source::go_on(source, escape as i32, held) }
}

/// Releases a payload that will not be dragged after all.
///
/// # Safety
/// `data` is alive, held once by the caller, which no longer holds it after this.
pub(crate) unsafe fn let_go(data: *mut Carried) {
    // SAFETY: as the caller promises.
    unsafe { release(data) };
}

/// The result of a drag out of a window: what another program did with it; or where on screen
/// it was dropped, if on its own window, where it drops as an internal drag does. (Neither when
/// cancelled.)
#[derive(Debug, PartialEq)]
pub(crate) struct Outcome {
    pub(crate) taken: Option<DropEffect>,
    pub(crate) home: Option<POINT>,
}

/// How a drag out of a window is handed to Windows: the `Carried` constructor, and the drag
/// runner (`carry_out`). Handed over where the app first drags something out (`Ui::drag_out`,
/// through `windows::hands_out`), and nothing else reaches it: a program that never drags out
/// carries none of this file but `is_going` and `let_go`.
#[derive(Clone, Copy)]
pub(crate) struct Outward {
    pub made: fn(DragData) -> *mut Carried,
    pub carried: unsafe fn(HANDLE, *mut Carried) -> Outcome,
}

/// The only implementation (see `Outward`).
pub(crate) const OUTWARD: Outward = Outward { made: Carried::new, carried: carry_out };

/// Drags `data` out of window `hwnd` until dropped or cancelled, and reports the result. The
/// caller's reference to `data` is released here.
///
/// # Safety
/// `data` is alive, and referenced once by the caller. No reference to the windows' state is
/// held meanwhile: the windows keep being drawn and notified, as ever.
pub(crate) unsafe fn carry_out(hwnd: HANDLE, data: *mut Carried) -> Outcome {
    let (source, mut effect) = (Source::new(hwnd), 0);
    GOING.set((data, hwnd));
    // SAFETY: both objects are alive, referenced here once each until the end.
    unsafe {
        let said = CARRY.get()(data, source, COPY | MOVE | LINK, &mut effect);
        GOING.set((std::ptr::null_mut(), 0));
        let taken = if said == DROP { (*data).taken(effect) } else { None };
        // (The drop point is where the pointer was when Windows last asked about the cursor: it
        // asks after every move.)
        let home = (*source).home.get().filter(|_| taken.is_none() && !(*source).given_up.get());
        release(source);
        release(data);
        Outcome { taken, home }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request for one format: in global memory, full content.
    fn asked(format: u16) -> FORMATETC {
        FORMATETC { format, device: std::ptr::null(), aspect: 1, index: -1, medium: 1 }
    }

    /// What a program gets of `data` in one format, as bytes; or the error it gets.
    fn got(data: *mut Carried, format: u16) -> Result<Vec<u8>, i32> {
        let mut handed = STGMEDIUM { medium: 0, memory: 0, owner: std::ptr::null_mut() };
        unsafe {
            let said = ((*data).table.get_data)(data, &asked(format), &mut handed);
            if said != 0 {
                return Err(said);
            }
            assert_eq!((handed.medium, handed.owner), (1, std::ptr::null_mut()), "a block of memory that is the asker's");
            let start = GlobalLock(handed.memory).cast::<u8>();
            let bytes = std::slice::from_raw_parts(start, GlobalSize(handed.memory)).to_vec();
            GlobalUnlock(handed.memory);
            ReleaseStgMedium(&mut handed);
            Ok(bytes)
        }
    }

    /// The formats a program finds in `data`, enumerating one at a time.
    fn listed(data: *mut Carried) -> Vec<u16> {
        let mut forms = std::ptr::null_mut();
        unsafe {
            assert_eq!((CARRIED.forms)(data, 1, &mut forms), 0);
            assert_eq!((CARRIED.forms)(data, 2, &mut std::ptr::null_mut()), NOT_DONE, "nothing can be set in it");
            let (mut found, mut one, mut count) = (Vec::new(), asked(0), 0);
            while (FORMS.next)(forms, 1, &mut one, &mut count) == 0 {
                assert_eq!((count, one.aspect, one.index, one.medium), (1, 1, -1, 1));
                found.push(one.format);
            }
            assert_eq!((FORMS.release)(forms), 0);
            found
        }
    }

    fn utf16(text: &str) -> Vec<u8> {
        wide(text).into_iter().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn files_are_carried_out_as_the_list_a_file_manager_reads() {
        let files = [std::path::PathBuf::from("C:\\a\\one.txt"), std::path::PathBuf::from("D:\\zwei drei\\für.png")];
        let data = Carried::new(DragData::Files(&files));
        assert_eq!(listed(data), [FILES]);
        assert_eq!(got(data, FILES), Ok(to_drop(&files)));
        // (A `DROPFILES` header: the names' offset, the drop point, and two flags, the last
        // saying they are UTF-16.)
        assert_eq!(&got(data, FILES).unwrap()[..20], &[20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(got(data, TEXT), Err(NO_FORM), "files are not text");
        unsafe {
            assert_eq!((CARRIED.query_get_data)(data, &asked(FILES)), 0);
            assert_eq!((CARRIED.query_get_data)(data, &asked(PIXELS)), NO_FORM);
            assert_eq!((CARRIED.query_get_data)(data, &FORMATETC { medium: 4, ..asked(FILES) }), NO_MEDIUM, "only in memory");
            assert_eq!((CARRIED.query_get_data)(data, &FORMATETC { aspect: 2, ..asked(FILES) }), NO_ASPECT, "only all of it");
            assert_eq!((CARRIED.release)(data), 0);
        }
    }

    #[test]
    fn text_and_links_are_carried_out_as_text_and_a_link_as_an_address_too() {
        let (text, link) = (Carried::new(DragData::Text("words, ünïcode 𝄞")), Carried::new(DragData::Link("https://example.org/ä")));
        let address = form_named("UniformResourceLocatorW");
        assert_eq!((listed(text), listed(link)), (vec![TEXT], vec![address, TEXT]));
        assert_eq!(got(text, TEXT), Ok(utf16("words, ünïcode 𝄞")));
        assert_eq!((got(link, address), got(link, TEXT)), (Ok(utf16("https://example.org/ä")), Ok(utf16("https://example.org/ä"))));
        assert_eq!(got(text, address), Err(NO_FORM));
        unsafe { assert_eq!(((CARRIED.release)(text), (CARRIED.release)(link)), (0, 0)) };
    }

    #[test]
    fn a_picture_is_carried_out_as_a_png_file_and_as_the_block_of_pixels_made_once() {
        let pixels = [0xff10_2030u32, 0x8040_5060, 0x0070_8090, 0xffa0_b0c0, 0x01d0_e0f0, 0xfe01_0203];
        let data = Carried::new(DragData::Picture(2, &pixels));
        let png = form_named("PNG");
        assert_eq!(listed(data), [png, PIXELS]);
        assert_eq!(got(data, png).ok().and_then(|png| crate::png::decode(&png)), Some((2, pixels.to_vec())));
        assert_eq!(got(data, PIXELS), Ok(to_block(2, &pixels)));
        // Each format is produced on first request, and cached.
        unsafe {
            let forms = &(*data).forms;
            let made = forms[0].1.get().map(|bytes| bytes.as_ptr());
            assert!(made.is_some());
            got(data, png).unwrap();
            assert_eq!(forms[0].1.get().map(|bytes| bytes.as_ptr()), made);
            assert_eq!((CARRIED.release)(data), 0);
        }
    }

    #[test]
    fn the_list_of_forms_is_gone_through_skipped_reset_and_copied() {
        let data = Carried::new(DragData::Picture(1, &[0xff00_0000]));
        let (mut forms, mut two, mut count) = (std::ptr::null_mut(), [asked(0), asked(0)], 0);
        unsafe {
            (CARRIED.forms)(data, 1, &mut forms);
            assert_eq!(((FORMS.next)(forms, 2, two.as_mut_ptr(), &mut count), count, two.map(|form| form.format)), (0, 2, [form_named("PNG"), PIXELS]));
            assert_eq!(((FORMS.next)(forms, 2, two.as_mut_ptr(), &mut count), count), (FEWER, 0), "none are left");
            assert_eq!(((FORMS.reset)(forms), (FORMS.skip)(forms, 1)), (0, 0));
            let mut copy = std::ptr::null_mut();
            assert_eq!((FORMS.clone)(forms, &mut copy), 0);
            assert_eq!(((FORMS.next)(copy, 2, two.as_mut_ptr(), std::ptr::null_mut()), two[0].format), (FEWER, PIXELS), "a copy is as far in");
            assert_eq!((FORMS.skip)(forms, 5), FEWER);
            let (mut same, mut none) = (std::ptr::null_mut(), std::ptr::null_mut());
            assert_eq!(((FORMS.query)(forms, &[0x103, 0, 0xC0, 0x4600_0000], &mut same), same), (0, forms));
            assert_eq!(((FORMS.query)(forms, &[0x10E, 0, 0xC0, 0x4600_0000], &mut none), none.is_null()), (NO_INTERFACE, true));
            assert_eq!(((FORMS.release)(forms), (FORMS.release)(forms), (FORMS.release)(copy), (CARRIED.release)(data)), (1, 0, 0, 0));
        }
    }

    #[test]
    fn what_the_program_it_went_to_says_it_did_is_heard() {
        let data = Carried::new(DragData::Text("t"));
        let say = |form: &str, what: u32| {
            let mut medium = STGMEDIUM { medium: 1, memory: in_memory(&what.to_le_bytes()), owner: std::ptr::null_mut() };
            unsafe { (CARRIED.set_data)(data, &asked(form_named(form)), &mut medium, 1) }
        };
        unsafe {
            // Windows' own answer, when nothing was reported.
            assert_eq!(
                [(*data).taken(0), (*data).taken(COPY), (*data).taken(MOVE), (*data).taken(LINK), (*data).taken(COPY | MOVE)],
                [None, Some(DropEffect::Copied), Some(DropEffect::Moved), Some(DropEffect::Copied), Some(DropEffect::Moved)]
            );
            // A file manager that moved the files itself reports it took nothing, and moved
            // them.
            assert_eq!((say("Performed DropEffect", 0), say("Logical Performed DropEffect", MOVE)), (0, 0));
            assert_eq!((*data).taken(0), Some(DropEffect::Moved));
            assert_eq!(say("DropDescription", 1), NOT_DONE, "the rest is not heard");
            assert_eq!((CARRIED.release)(data), 0);
        }
        let data = Carried::new(DragData::Text("t"));
        let said = |what: u32| {
            let mut medium = STGMEDIUM { medium: 1, memory: in_memory(&what.to_le_bytes()), owner: std::ptr::null_mut() };
            unsafe { (CARRIED.set_data)(data, &asked(form_named("Performed DropEffect")), &mut medium, 1) }
        };
        said(COPY);
        unsafe {
            assert_eq!((*data).taken(MOVE), Some(DropEffect::Copied), "what it says it did counts");
            assert_eq!((CARRIED.release)(data), 0);
        }
    }

    #[test]
    fn the_source_goes_on_while_the_button_is_held_and_escape_gives_it_up() {
        let source = Source::new(0);
        unsafe {
            assert_eq!([(SOURCE.go_on)(source, 0, 1), (SOURCE.go_on)(source, 0, 1 | 4), (SOURCE.go_on)(source, 0, 0), (SOURCE.go_on)(source, 0, 2)], [0, 0, DROP, DROP]);
            assert!(!(*source).given_up.get());
            assert_eq!((SOURCE.go_on)(source, 1, 1), CANCEL);
            assert!((*source).given_up.get());
            // Over none of its own windows (the window is null), the system cursors show.
            assert_eq!(((SOURCE.looks)(source, COPY), (*source).home.get()), (USUAL_POINTERS, None));
            let (mut same, mut none) = (std::ptr::null_mut(), std::ptr::null_mut());
            assert_eq!(((SOURCE.query)(source, &[0x121, 0, 0xC0, 0x4600_0000], &mut same), same), (0, source));
            assert_eq!(((SOURCE.query)(source, &[0, 0, 0xC0, 0x4600_0000], &mut same), same), (0, source));
            assert_eq!(((SOURCE.query)(source, &[0x122, 0, 0xC0, 0x4600_0000], &mut none), none.is_null()), (NO_INTERFACE, true));
            assert_eq!(((SOURCE.release)(source), (SOURCE.release)(source), (SOURCE.release)(source)), (2, 1, 0));
        }
    }

    #[test]
    fn nothing_in_these_tests_is_carried_by_windows() {
        let data = Carried::new(DragData::Text("t"));
        assert_eq!(CARRY.get() as usize, by_nobody as Carry as usize);
        assert_eq!(unsafe { carry_out(0, data) }, Outcome { taken: None, home: None });
    }
}
