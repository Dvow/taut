//! A chat window: messages in a growing list, newest at the bottom, and a text box. While the
//! user is at the end, the list follows new messages; once they scroll back, it stays put,
//! shows how many new ones arrived, and jumps to the end on a press. Enter sends, Shift+Enter
//! starts a new line. A simulated friend replies to each message a moment later.
//!   cargo run -p taut --example chat
#![windows_subsystem = "windows"]
use std::time::{Duration, Instant};
use taut::{Align, Flow, Key, Length, Mods, Ui, Window};

/// What the friend says, in turn.
const ANSWERS: [&str; 5] = [
    "Hello! Good to hear from you.",
    "I was just about to write to you, in fact. How did the move go in the end, and is the new place as quiet as you hoped?",
    "Ha, that sounds about right.",
    "Shall we meet on Saturday?",
    "Good. See you then!",
];
/// How long the friend takes to answer.
const THINKING: Duration = Duration::from_millis(1200);

/// One message, and who sent it.
pub struct Message {
    mine: bool,
    text: String,
}

/// Everything the chat keeps.
#[derive(Default)]
pub struct Chat {
    messages: Vec<Message>,
    typed: String,
    /// The list showed its end as it was last described; how many messages came since it
    /// did not; and whether it is to go back to its end.
    at_end: bool,
    unread: usize,
    to_end: bool,
    /// What time it is, and when the friend will have an answer ready.
    now: Duration,
    answer_at: Option<Duration>,
    answered: usize,
}

impl Chat {
    fn add(&mut self, mine: bool, text: String) {
        self.messages.push(Message { mine, text });
        self.unread += !self.at_end as usize;
    }
}

pub fn describe(app: &mut Chat, ui: &mut Ui) {
    let t = ui.theme;
    // The friend's answer, once it is ready; until then, the interface is woken for it.
    match app.answer_at {
        Some(at) if at <= app.now => {
            app.add(false, ANSWERS[app.answered % ANSWERS.len()].into());
            (app.answer_at, app.answered) = (None, app.answered + 1);
        }
        Some(at) => ui.wake_after(at - app.now),
        None => {}
    }

    // The list, and what lies over it (the friend writing, the way back to the newest).
    ui.column().grow().open();
    ui.el().grow().flow(Flow::Layers).open();
    // (A few stand at the bottom, as the newest always does.)
    let list = ui.column().key("list").grow().scroll().align(Align::End, Align::Spread).open();
    ui.stay_at_end(&list);
    if std::mem::take(&mut app.to_end) {
        ui.scroll_to(&list, [f32::NAN, f32::INFINITY]);
    }
    // Each message as high as its text comes out: only the ones in view are described.
    let wide = ui.size().w;
    for at in ui.uneven_rows(app.messages.len(), 40.0) {
        bubble(&app.messages[at], wide, ui);
    }
    ui.close();
    ui.close();
    app.at_end = ui.scrolled(&list).at_end();
    app.unread = if app.at_end { 0 } else { app.unread };
    if app.messages.is_empty() {
        ui.row().place(Align::Center).open();
        ui.text("No messages yet. Say hello!", t.font, t.text_faint);
        ui.close();
    }
    if app.answer_at.is_some() {
        ui.row().place((Align::Start, Align::End)).pad((14.0, 4.0)).open();
        ui.note("Sam is writing\u{2026}");
        ui.close();
    }
    if !app.at_end {
        ui.row().place((Align::Center, Align::End)).pad(6.0).gap(8.0).fill(t.raise).radius(t.radius).align(Align::Center, Align::Center).open();
        if app.unread > 0 {
            ui.note(&format!("{} new", app.unread));
        }
        app.to_end |= ui.button("Back to the newest").clicked;
        ui.close();
    }
    ui.close();

    // The box to type in: Enter sends what is in it, as the button does; Shift and Enter
    // start a new line.
    ui.row().pad(10.0).gap(8.0).fill(t.panel).open();
    ui.leave_enter();
    let typing = ui.text_area(&mut app.typed, "Write a message", (1, 4));
    let send = ui.primary_button("Send").clicked || typing.focused && ui.key(Key::Enter, Mods::NONE);
    ui.close();
    ui.close();
    if send && !app.typed.trim().is_empty() {
        let text = std::mem::take(&mut app.typed);
        app.add(true, text.trim().into());
        // (What one sends, one sees.)
        (app.to_end, app.answer_at) = (true, Some(app.now + THINKING));
    }
}

/// One message, a row of the list: the person's own on the right in the colour that stands
/// out, the friend's on the left; as wide as its text, up to three quarters of the window.
fn bubble(message: &Message, wide: f32, ui: &mut Ui) {
    let t = ui.theme;
    let (side, fill, ink) = if message.mine { (Align::End, t.accent, t.on_accent) } else { (Align::Start, t.raise, t.text) };
    ui.row().w(Length::Grow(1.0)).pad((12.0, 3.0)).align(side, Align::Start).open();
    ui.column().max((wide * 0.75).max(120.0), f32::INFINITY).pad((10.0, 6.0)).fill(fill).radius(10.0).open();
    ui.el().text(&message.text, t.font, ink).wrap().show();
    ui.close();
    ui.close();
}

fn main() {
    let (mut app, started) = (Chat::default(), Instant::now());
    let window = Window { title: "Chat", width: 420.0, height: 560.0, min_width: 280.0, min_height: 240.0, ..Default::default() };
    taut::run(window, move |ui| {
        app.now = started.elapsed();
        describe(&mut app, ui);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use taut::Role;
    use taut::testing::Trial;

    fn send(trial: &mut Trial<Chat>, text: &str) {
        trial.fill("Write a message", text);
        trial.key("Enter");
    }

    #[test]
    fn the_list_stays_at_its_end_while_the_person_is_there() {
        let mut trial = Trial::new(Chat::default(), describe);
        trial.tell_time(|chat, now| chat.now = now);
        trial.resize(360.0, 400.0, 1.0);
        assert!(trial.has(Role::Text, "No messages yet. Say hello!"));

        // A message is sent with Enter; Shift and Enter start a new line in it first. The
        // friend writes, and answers a moment later.
        trial.fill("Write a message", "Hi Sam!");
        trial.key("Shift+Enter");
        trial.type_text("How are you?");
        trial.key("Enter");
        assert!(trial.has(Role::Text, "Hi Sam!\nHow are you?") && trial.has(Role::Text, "Sam is writing\u{2026}"));
        assert_eq!(trial.find(Role::TextBox, "Write a message").value, "", "the box is empty at once");
        trial.wait(Duration::from_secs(2));
        assert!(trial.has(Role::Text, ANSWERS[0]) && !trial.has(Role::Text, "Sam is writing\u{2026}"));
        trial.looks_like("tests/looks/chat.png");

        // More than fit: the newest are in view, at the bottom, the first are gone off the top.
        for number in 1..=12 {
            send(&mut trial, &format!("Message {number}"));
        }
        let (last, typing) = (trial.find(Role::Text, "Message 12").rect, trial.find(Role::TextBox, "Write a message").rect);
        assert!(last.bottom() <= typing.y && last.bottom() > typing.y - 60.0, "the newest is just over the box to type in: {last:?}");
        assert!(!trial.has(Role::Text, "Hi Sam!\nHow are you?"));
        trial.wait(Duration::from_secs(2));
        assert!(trial.has(Role::Text, ANSWERS[1]), "and the answer that came is in view, at the end:\n{}", trial.listed());

        // Back to read what came before: the list stays there as more comes, and says so.
        let middle = trial.spot(Role::Text, "Message 12");
        trial.wheel(middle, 0.0, -10_000.0);
        assert!(trial.has(Role::Text, "Hi Sam!\nHow are you?") && trial.has(Role::Button, "Back to the newest"));
        trial.app.answer_at = Some(trial.app.now + THINKING); // (as if the friend wrote first)
        trial.run();
        trial.wait(Duration::from_secs(2));
        assert!(trial.has(Role::Text, "Hi Sam!\nHow are you?") && trial.has(Role::Text, "1 new"));

        // And back to the end at a press, or by sending.
        trial.press(Role::Button, "Back to the newest");
        assert!(trial.has(Role::Text, ANSWERS[2]) && !trial.has(Role::Text, "Hi Sam!\nHow are you?"));
        assert!(!trial.has(Role::Button, "Back to the newest"));
        let middle = trial.spot(Role::Text, ANSWERS[2]);
        trial.wheel(middle, 0.0, -10_000.0);
        send(&mut trial, "Bye!");
        assert!(trial.has(Role::Text, "Bye!") && trial.app.at_end);
    }
}
