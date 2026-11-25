//! System notifications and one running copy on the web, handed to the page the first time an
//! app asks for either (`platform::hand_desktop`): a program that never does carries none of it.
//!
//! - **Notifications:** the browser's (`Notification`), shown once the person allows them: the
//!   first one asks, as on the desktops that ask. A click brings the page's tab forward and is
//!   heard in the next run. Pictures go as PNG.
//! - **One running copy:** the first tab of a program holds a lock named for it (Web Locks); a
//!   later tab of the same site finds it held, hands its address to the first over a
//!   `BroadcastChannel`, and is told it is not the first. The first hears the later start
//!   ([`Ui::relaunches`]) with that address as its one argument. (Two tabs opened at the same
//!   moment may both start as the first: the lock goes to one, and the other takes it over when
//!   that one closes.)
use crate::notifications::NOTICES;
use crate::single_instance::{Relaunch, Tokens, came};
use crate::ui::Ui;
use crate::web_host::{Heard, Part, Then, add_part, parts};

#[link(wasm_import_module = "taut")]
unsafe extern "C" {
    fn notify(number: u32, title: *const u8, title_length: usize, text: *const u8, text_length: usize, png: *const u8, png_length: usize);
    fn first_copy(name: *const u8, length: usize) -> u32;
}

/// What the loader hears for notifications and later starts.
mod heard {
    /// A notification was clicked: `number` is its number.
    pub const CLICKED: u32 = 0;
    /// A later tab started: the bytes are its address.
    pub const STARTED: u32 = 1;
}

/// Hands the page its notifications and its way to hear later starts.
pub(crate) fn hand_desktop() {
    add_part(parts::DESKTOP, Part { after: show_notifications, heard });
}

/// Makes this tab the one running copy of program `name`: whether it is the first (see the
/// module).
pub(crate) fn first(name: &str) -> bool {
    // SAFETY: the loader reads `length` bytes.
    unsafe { first_copy(name.as_ptr(), name.len()) != 0 }
}

/// After a run: shows the notifications it asked for.
fn show_notifications(_: &mut Ui) {
    let out = NOTICES.with_borrow_mut(|notices| std::mem::take(&mut notices.out));
    for notice in out {
        let png = notice.picture.and_then(|(width, pixels)| crate::png::encode_rgba(width, &pixels)).unwrap_or_default();
        let (title, text) = (notice.title.as_bytes(), notice.text.as_bytes());
        // SAFETY: the loader reads each at once.
        unsafe { notify(notice.number, title.as_ptr(), title.len(), text.as_ptr(), text.len(), png.as_ptr(), png.len()) };
    }
}

/// A notification clicked, or a later tab's start.
fn heard(_: &mut Ui, heard: Heard) -> (Then, u32) {
    match heard.what {
        heard::CLICKED => NOTICES.with_borrow_mut(|notices| notices.clicked.push(heard.number as u32)),
        heard::STARTED => {
            let address = String::from_utf8_lossy(&heard.bytes).into_owned();
            came(Tokens::default(), Relaunch { args: vec![address.into()], working_dir: Default::default() });
        }
        _ => return (Then::Wait, 0),
    }
    (Then::Run, 0)
}
