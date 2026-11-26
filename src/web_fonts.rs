//! The person's fonts on the web, as the browser lists them (`queryLocalFonts`, which Chrome and
//! Edge have, after asking the person): what `system_fonts::families` and `styles` give. Asked
//! the first time an app lists fonts, so a program that never does carries none of it.
use crate::system_fonts::Style;
use crate::ui::Ui;
use crate::web_host::{Heard, Part, Then, add_part, parts};
use std::cell::{Cell, RefCell};

#[link(wasm_import_module = "taut")]
unsafe extern "C" {
    fn ask_fonts(again: u32) -> u32;
}

/// What the loader hears: the list, or that the browser refused to give it.
const LISTED: u32 = 0;
const REFUSED_TO_LIST: u32 = 1;

thread_local! {
    /// The person's fonts as the browser listed them, each family and style; none until asked.
    static LOCAL_FONTS: RefCell<Option<Vec<(String, Style)>>> = const { RefCell::new(None) };
    /// The browser refused the last time it was asked.
    static REFUSED: Cell<bool> = const { Cell::new(false) };
}

/// The person's fonts, each family and style: all, or one family's. Empty until the browser
/// answers (the window runs again then), and for good where it lists none.
pub(crate) fn listed(family: Option<&str>) -> Vec<(String, Style)> {
    let wanted = |(of, _): &&(String, Style)| family.is_none_or(|family| of.eq_ignore_ascii_case(family));
    let found = LOCAL_FONTS.with_borrow(|fonts| fonts.as_ref().map(|fonts| fonts.iter().filter(wanted).cloned().collect()));
    found.unwrap_or_else(|| {
        add_part(parts::FONTS, Part { after: |_| {}, heard });
        // SAFETY: a plain request, answered through `heard`; it says whether it was made (after
        // a refusal, only from a press or a key: else it is asked again at the next call).
        if unsafe { ask_fonts(REFUSED.get() as u32) } != 0 {
            LOCAL_FONTS.set(Some(Vec::new()));
        }
        Vec::new()
    })
}

/// The browser's list: a line a font, its family and style name apart by a tab. Or it refused
/// (Chrome and Edge list none until the person allows it): it is asked again at the next call
/// (from a press or a key).
fn heard(_: &mut Ui, heard: Heard) -> (Then, u32) {
    let refused = match heard.what {
        LISTED => heard.bytes.is_empty(),
        REFUSED_TO_LIST => true,
        _ => return (Then::Wait, 0),
    };
    REFUSED.set(refused);
    if refused {
        LOCAL_FONTS.set(None);
        return (Then::Wait, 0);
    }
    let lines = String::from_utf8_lossy(&heard.bytes).into_owned();
    let fonts = lines.lines().filter_map(|line| line.split_once('\t')).map(|(family, style)| (family.to_string(), crate::system_fonts::style_named(style))).collect();
    LOCAL_FONTS.set(Some(fonts));
    (Then::Run, 0)
}
