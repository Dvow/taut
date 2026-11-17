//! System notifications: a title, a line of text and a picture, shown where the system shows
//! every program's. The run after a click says which one was clicked. (Text shown in the window
//! itself is [`Ui::notice`].)
//!
//! A notification is shown after the run that asks for it. On Windows it comes from a tray icon
//! (`Shell_NotifyIcon`): the app's own, or one that exists only while the notification shows.
//! On Linux it goes to `org.freedesktop.Notifications`, or, in a Flatpak sandbox, through the
//! desktop portal.
use crate::ui::Ui;
use std::cell::RefCell;

/// A system notification the program showed; tells which one was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Notification(pub(crate) u32);

/// A notification still to be shown: its number, title, text and picture (width in pixels, and
/// pixels).
pub(crate) struct Out {
    pub number: u32,
    pub title: String,
    pub text: String,
    pub picture: Option<(usize, Vec<u32>)>,
}

/// The program's notifications on this thread: how many were shown, those still to show, and
/// those clicked until a run takes them.
#[derive(Default)]
pub(crate) struct Notices {
    pub count: u32,
    pub out: Vec<Out>,
    pub clicked: Vec<u32>,
}

thread_local! {
    pub(crate) static NOTICES: RefCell<Notices> = RefCell::new(Notices::default());
}

/// System notifications.
impl Ui {
    /// Shows a system notification after this run: a title, a line of text, and a picture if
    /// given (width in pixels and pixels, as [`add_picture`](Self::add_picture) takes them;
    /// without one the system shows the program's icon). Gives back what tells it apart when
    /// clicked ([`notification_clicked`](Self::notification_clicked)). Only in a window run by
    /// `taut::run`.
    pub fn system_notification(&mut self, title: &str, text: &str, picture: Option<(usize, &[u32])>) -> Notification {
        crate::platform::hand_desktop();
        NOTICES.with_borrow_mut(|notices| {
            notices.count += 1;
            let picture = picture.map(|(width, pixels)| (width, pixels.to_vec()));
            notices.out.push(Out { number: notices.count, title: title.into(), text: text.into(), picture });
            Notification(notices.count)
        })
    }

    /// The system notification the person just clicked, once, in the run after. The system
    /// removes it; the app usually comes forward ([`bring_to_front`](Self::bring_to_front)) and
    /// shows what it was about.
    pub fn notification_clicked(&mut self) -> Option<Notification> {
        let clicked = NOTICES.with_borrow_mut(|notices| (!notices.clicked.is_empty()).then(|| notices.clicked.remove(0)));
        self.again |= clicked.is_some();
        clicked.map(Notification)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Trial;

    #[test]
    fn a_notification_is_kept_until_it_is_shown_and_a_click_comes_back_once() {
        NOTICES.set(Notices::default());
        let mut trial = Trial::new((false, Vec::new()), |(send, heard): &mut (bool, Vec<Notification>), ui| {
            if std::mem::take(send) {
                heard.push(ui.system_notification("Saved", "notes.txt is on the disk", Some((1, &[0xff00_ff00]))));
            }
            heard.extend(ui.notification_clicked());
            ui.label("The window");
        });
        trial.app.0 = true;
        trial.run();
        let sent = trial.app.1[0];
        let out = NOTICES.with_borrow_mut(|notices| std::mem::take(&mut notices.out));
        assert_eq!(out.len(), 1, "asked for once, in one run of several goes");
        assert_eq!((out[0].number, out[0].title.as_str(), out[0].text.as_str(), out[0].picture.clone()), (sent.0, "Saved", "notes.txt is on the disk", Some((1, vec![0xff00_ff00]))));
        // The person clicks it: the next run hears which, once.
        NOTICES.with_borrow_mut(|notices| notices.clicked.push(sent.0));
        trial.run();
        trial.run();
        assert_eq!(trial.app.1, [sent, sent]);
        // A second one is told apart from the first.
        trial.app.0 = true;
        trial.run();
        assert_ne!(trial.app.1[2], sent);
    }
}
