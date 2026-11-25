//! The person's fonts on the web, as the browser lists them (`queryLocalFonts`, which Chrome and
//! Edge have, after asking the person): what `system_fonts::families` and `styles` give. Asked
//! the first time an app lists fonts, so a program that never does carries none of it.
use crate::system_fonts::Style;
use crate::ui::Ui;
use crate::web_host::{Heard, Part, Then, add_part, parts};
use std::cell::RefCell;

#[link(wasm_import_module = "taut")]
unsafe extern "C" {
    fn ask_fonts();
}

/// What the loader hears: the list, or that the browser refused to give it.
const LISTED: u32 = 0;

thread_local! {
    /// The person's fonts as the browser listed them, each family and style; none until asked.
    static LOCAL_FONTS: RefCell<Option<Vec<(String, Style)>>> = const { RefCell::new(None) };
}

/// The person's fonts, each family and style: all, or one family's. Empty until the browser
/// answers (the window runs again then), and for good where it lists none.
pub(crate) fn listed(family: Option<&str>) -> Vec<(String, Style)> {
    let Some(fonts) = LOCAL_FONTS.with_borrow(Clone::clone) else {
        LOCAL_FONTS.set(Some(Vec::new()));
        add_part(parts::FONTS, Part { after: |_| {}, heard });
        // SAFETY: a plain request, answered through `heard`.
        unsafe { ask_fonts() };
        return Vec::new();
    };
    fonts.into_iter().filter(|(of, _)| family.is_none_or(|family| of.eq_ignore_ascii_case(family))).collect()
}

/// The browser's list: a line a font, its family and style name apart by a tab. Or it refused:
/// it is asked again at the next call.
fn heard(_: &mut Ui, heard: Heard) -> (Then, u32) {
    if heard.what != LISTED {
        LOCAL_FONTS.set(None);
        return (Then::Wait, 0);
    }
    let lines = String::from_utf8_lossy(&heard.bytes).into_owned();
    let fonts = lines.lines().filter_map(|line| line.split_once('\t')).map(|(family, style)| (family.to_string(), crate::system_fonts::style_named(style))).collect();
    LOCAL_FONTS.set(Some(fonts));
    (Then::Run, 0)
}
