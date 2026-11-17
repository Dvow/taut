//! What an input method is composing, on macOS (its "marked text"). AppKit leaves showing it to
//! the program: it is shown at the text cursor, over the text box, in a small panel of its own
//! that goes with the window (a child window). It is underlined; the part the input method is
//! working on (its selected clause) is underlined thicker. The panel never takes the keyboard or
//! the pointer, and goes once the text is committed or dropped.
use crate::geom::Rect;
use crate::macos_sys::*;
use std::cell::RefCell;
use std::ffi::c_void;

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    static NSFontAttributeName: Obj;
    static NSForegroundColorAttributeName: Obj;
    static NSUnderlineStyleAttributeName: Obj;
}

/// Window styles: no frame, and a panel that never makes its program active.
const BORDERLESS: usize = 0;
const NONACTIVATING: usize = 1 << 7;
/// Underline styles: single, thick.
const SINGLE: isize = 1;
const THICK: isize = 2;

thread_local! {
    /// Each view's panel, once one was shown (kept, retained, until the view goes).
    static PANELS: RefCell<Vec<(Obj, Obj)>> = const { RefCell::new(Vec::new()) };
}

/// The panel and the label in it, made the first time.
fn panel_of(view: Obj) -> Obj {
    if let Some(panel) = PANELS.with_borrow(|panels| panels.iter().find(|(of, _)| *of == view).map(|(_, panel)| *panel)) {
        return panel;
    }
    // SAFETY: a panel and a label made and set up on the main thread; the panel keeps the
    // label, and is kept here (+1) until `forget`.
    let panel = unsafe {
        let panel: Obj = send(class(c"NSPanel"), c"alloc", ());
        let panel: Obj = send(panel, c"initWithContentRect:styleMask:backing:defer:", (NSRect::new(0.0, 0.0, 1.0, 1.0), BORDERLESS | NONACTIVATING, 2usize, 0 as Bool));
        send::<()>(panel, c"setReleasedWhenClosed:", (0 as Bool,));
        send::<()>(panel, c"setIgnoresMouseEvents:", (1 as Bool,));
        send::<()>(panel, c"setBecomesKeyOnlyIfNeeded:", (1 as Bool,));
        send::<()>(panel, c"setHasShadow:", (1 as Bool,));
        let label: Obj = send(class(c"NSTextField"), c"labelWithString:", (ns_string(""),));
        send::<()>(label, c"setDrawsBackground:", (1 as Bool,));
        send::<()>(label, c"setBackgroundColor:", (send::<Obj>(class(c"NSColor"), c"textBackgroundColor", ()),));
        send::<()>(panel, c"setContentView:", (label,));
        panel
    };
    PANELS.with_borrow_mut(|panels| panels.push((view, panel)));
    panel
}

/// The text as shown: in the system font at `size` points, underlined, its clause `picked`
/// (UTF-16 units) thicker. Autoreleased.
fn dressed(text: &str, picked: NSRange, size: f64) -> Obj {
    let number = |style: isize| -> Obj { unsafe { send(class(c"NSNumber"), c"numberWithInteger:", (style,)) } };
    // SAFETY: plain objects made from strings, a font, a colour and numbers; the keys are
    // AppKit's attribute names.
    unsafe {
        let font: Obj = send(class(c"NSFont"), c"systemFontOfSize:", (size,));
        let colour: Obj = send(class(c"NSColor"), c"textColor", ());
        let keys = [NSFontAttributeName, NSForegroundColorAttributeName, NSUnderlineStyleAttributeName];
        let values = [font, colour, number(SINGLE)];
        let looks: Obj = send(class(c"NSDictionary"), c"dictionaryWithObjects:forKeys:count:", (values.as_ptr(), keys.as_ptr(), keys.len()));
        let shown: Obj = send(class(c"NSMutableAttributedString"), c"alloc", ());
        let shown: Obj = send(shown, c"initWithString:attributes:", (ns_string(text), looks));
        let length = text.encode_utf16().count();
        if picked.length > 0 && picked.location + picked.length <= length {
            send::<()>(shown, c"addAttribute:value:range:", (NSUnderlineStyleAttributeName, number(THICK), picked));
        }
        send(shown, c"autorelease", ())
    }
}

/// Shows what is composed in a view: `text`, its clause `picked`, at the text cursor `at` (in
/// the view's units); or hides it, with no text.
pub(crate) fn show(view: Obj, text: &str, picked: NSRange, at: Option<Rect>) {
    let Some(at) = at.filter(|_| !text.is_empty()) else { return hide(view) };
    let panel = panel_of(view);
    let size = f64::from(at.h * 0.75).clamp(9.0, 72.0);
    // SAFETY: the panel and its label are the library's; the rectangle is turned from the view's
    // into its window's and the screen's.
    unsafe {
        let label: Obj = send(panel, c"contentView", ());
        send::<()>(label, c"setAttributedStringValue:", (dressed(text, picked, size),));
        send::<()>(label, c"sizeToFit", ());
        let fits = send_rect(label, c"frame", ()).size;
        let window: Obj = send(view, c"window", ());
        let cursor = NSRect::new(f64::from(at.x), f64::from(at.y), 1.0, f64::from(at.h));
        let on_screen = send_rect(window, c"convertRectToScreen:", (send_rect(view, c"convertRect:toView:", (cursor, std::ptr::null_mut::<c_void>())),));
        let height = fits.height.max(on_screen.size.height);
        let frame = NSRect::new(on_screen.origin.x, on_screen.origin.y + on_screen.size.height - height, fits.width + 2.0, height);
        send::<()>(panel, c"setFrame:display:", (frame, 1 as Bool));
        send::<()>(label, c"setFrame:", (NSRect::new(1.0, (height - fits.height) / 2.0, fits.width, fits.height),));
        if send::<Obj>(panel, c"parentWindow", ()).is_null() {
            send::<()>(window, c"addChildWindow:ordered:", (panel, 1isize));
        }
        send::<()>(panel, c"orderFront:", (std::ptr::null_mut::<c_void>(),));
    }
}

/// Hides what is composed in a view, if shown.
pub(crate) fn hide(view: Obj) {
    let Some(panel) = PANELS.with_borrow(|panels| panels.iter().find(|(of, _)| *of == view).map(|(_, panel)| *panel)) else { return };
    // SAFETY: the panel is the library's; taken off its window, then out of sight.
    unsafe {
        let parent: Obj = send(panel, c"parentWindow", ());
        if !parent.is_null() {
            send::<()>(parent, c"removeChildWindow:", (panel,));
        }
        send::<()>(panel, c"orderOut:", (std::ptr::null_mut::<c_void>(),));
    }
}

/// A view goes: its panel too.
pub(crate) fn forget(view: Obj) {
    hide(view);
    let gone: Vec<Obj> = PANELS.with_borrow_mut(|panels| panels.extract_if(.., |(of, _)| *of == view).map(|(_, panel)| panel).collect());
    // SAFETY: the panel was kept here with a count of one, given up here.
    gone.into_iter().for_each(|panel| unsafe { send::<()>(panel, c"release", ()) });
}
