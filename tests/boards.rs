//! Boards: panned and zoomed, their content scaled as shown. Every step goes through `common`'s
//! window, which compares incremental redraws with full ones.
mod common;
use common::*;
use taut::geom::{pt, rect};
use taut::{Board, Button, Event, Key, Mods, Rect, Response, Ui};

/// A board that fills the window, with three things on it: two labelled boxes in sight and
/// one far off. Gives back the board and where each thing is that was described.
fn plan(ui: &mut Ui, board: &mut Board, seen: &mut Vec<(usize, Rect)>) -> Response {
    let t = ui.theme;
    seen.clear();
    let ground = ui.board(board, "Plan", 0.05..=8.0);
    let things = [rect(20.0, 20.0, 100.0, 40.0), rect(160.0, 120.0, 100.0, 40.0), rect(2000.0, 1500.0, 100.0, 40.0)];
    for (n, at) in things.into_iter().enumerate() {
        if ui.in_sight(at) {
            let box_ = ui.el().key(n).at(at.x, at.y).size(at.w, at.h).fill(t.panel).border(t.line, 1.0).radius(4.0).text("Thing", t.font, t.text).sense().show();
            seen.push((n, box_.rect));
        }
    }
    ui.close();
    ground
}

#[test]
fn a_board_shows_only_what_is_in_sight_and_its_bars_say_where_the_rest_is() {
    let mut ui = window(1.0);
    let (mut board, mut seen) = (Board::default(), Vec::new());
    let mut ground = Response::default();
    settle(&mut ui, |ui| ground = plan(ui, &mut board, &mut seen));
    ui.run(&mut |ui| ground = plan(ui, &mut board, &mut seen));
    // The thing far off is not described; the two in sight are where the board puts them.
    assert_eq!(seen.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(seen[0].1, rect(20.0, 20.0, 100.0, 40.0));
    // The board leaves room for its bars: it is narrower and lower by them.
    assert_eq!((ground.rect.w, ground.rect.h), (420.0, 300.0));
}

#[test]
fn the_wheel_moves_a_board_and_control_with_it_shows_it_nearer_about_the_pointer() {
    let mut ui = window(1.0);
    let (mut board, mut seen) = (Board::default(), Vec::new());
    settle(&mut ui, |ui| _ = plan(ui, &mut board, &mut seen));
    does(&mut ui, Event::Move(pt(70.0, 40.0)), |ui| _ = plan(ui, &mut board, &mut seen));
    does(&mut ui, Event::Wheel(0.0, 10.0), |ui| _ = plan(ui, &mut board, &mut seen));
    assert_eq!((board.x, board.y), (0.0, 10.0));
    // (What a box gives back is where it was as of the last layout: one run on, it is where it is now.)
    ui.run(&mut |ui| _ = plan(ui, &mut board, &mut seen));
    assert_eq!(seen[0].1, rect(20.0, 10.0, 100.0, 40.0));
    // Twice as near, about the pointer: the point under it stays under it.
    let under = board.point(pt(70.0, 40.0), rect(0.0, 0.0, 420.0, 300.0));
    does(&mut ui, Event::Zoom(2.0), |ui| _ = plan(ui, &mut board, &mut seen));
    assert_eq!(board.zoom, 2.0);
    ui.run(&mut |ui| _ = plan(ui, &mut board, &mut seen));
    let after = board.point(pt(70.0, 40.0), rect(0.0, 0.0, 420.0, 300.0));
    assert!((under.x - after.x).abs() < 1e-3 && (under.y - after.y).abs() < 1e-3, "{under:?} {after:?}");
    // What is on it is twice as large, and still where the board puts it (and what of it
    // can be seen is what is on the board).
    let spot = board.spot(pt(20.0, 20.0), rect(0.0, 0.0, 420.0, 300.0));
    assert_eq!(seen[0].1, rect(spot.x, spot.y, 200.0, 80.0).intersect(rect(0.0, 0.0, 420.0, 300.0)));
}

#[test]
fn a_board_is_dragged_by_the_middle_button_and_by_its_ground() {
    let mut ui = window(1.0);
    let (mut board, mut seen) = (Board::default(), Vec::new());
    settle(&mut ui, |ui| _ = plan(ui, &mut board, &mut seen));
    for event in [Event::Move(pt(60.0, 40.0)), Event::Press(Button::Middle, Mods::NONE), Event::Move(pt(90.0, 70.0)), Event::Release(Button::Middle)] {
        does(&mut ui, event, |ui| _ = plan(ui, &mut board, &mut seen));
    }
    assert_eq!((board.x, board.y), (-30.0, -30.0));
    // Its ground, plainly: where nothing is on it.
    drag(&mut ui, pt(300.0, 250.0), pt(280.0, 240.0), |ui| _ = plan(ui, &mut board, &mut seen));
    assert_eq!((board.x, board.y), (-10.0, -20.0));
}

#[test]
fn keys_show_a_board_nearer_further_at_its_own_size_and_all_of_it() {
    let mut ui = window(1.0);
    let (mut board, mut seen) = (Board::default(), Vec::new());
    let mut ground = Response::default();
    settle(&mut ui, |ui| ground = plan(ui, &mut board, &mut seen));
    click(&mut ui, pt(400.0, 280.0), |ui| ground = plan(ui, &mut board, &mut seen));
    assert!(ground.focused);
    key(&mut ui, Key::Char('='), Mods::CTRL, |ui| _ = plan(ui, &mut board, &mut seen));
    assert_eq!(board.zoom, 1.25);
    key(&mut ui, Key::Char('-'), Mods::CTRL, |ui| _ = plan(ui, &mut board, &mut seen));
    key(&mut ui, Key::Char('-'), Mods::CTRL, |ui| _ = plan(ui, &mut board, &mut seen));
    assert_eq!(board.zoom, 0.8);
    key(&mut ui, Key::Char('0'), Mods::CTRL, |ui| _ = plan(ui, &mut board, &mut seen));
    assert_eq!(board.zoom, 1.0);
    // All of it: from the first thing to the one far off, in the middle of the window.
    key(&mut ui, Key::Char('1'), Mods::SHIFT, |ui| _ = plan(ui, &mut board, &mut seen));
    assert!(board.zoom < 0.25, "{board:?}");
    assert_eq!(seen.len(), 3, "everything is in sight");
}

// (Fingers are the feature `touch`.)
#[cfg(feature = "touch")]
#[test]
fn two_fingers_on_a_board_move_it_and_pinch_it_about_their_middle() {
    use taut::{Phase, Touch};
    let mut ui = window(1.0);
    let (mut board, mut seen) = (Board::default(), Vec::new());
    settle(&mut ui, |ui| _ = plan(ui, &mut board, &mut seen));
    let finger = |finger: u32, x: f32, y: f32, phase: Phase| Event::Touch(Touch { finger, at: pt(x, y), phase });
    // Twice as far apart, and both 10 lower: what was under their middle is under it still.
    let steps = [finger(1, 200.0, 150.0, Phase::Down), finger(2, 240.0, 150.0, Phase::Down), finger(1, 180.0, 160.0, Phase::Move), finger(2, 260.0, 160.0, Phase::Move)];
    for event in steps {
        does(&mut ui, event, |ui| _ = plan(ui, &mut board, &mut seen));
    }
    let under = board.point(pt(220.0, 160.0), rect(0.0, 0.0, 420.0, 300.0));
    assert!((board.zoom - 2.0).abs() < 1e-4 && (under.x - 220.0).abs() < 1e-3 && (under.y - 150.0).abs() < 1e-3, "{board:?} {under:?}");
    for event in [finger(1, 180.0, 160.0, Phase::Up), finger(2, 260.0, 160.0, Phase::Up)] {
        does(&mut ui, event, |ui| _ = plan(ui, &mut board, &mut seen));
    }
    assert!((board.zoom - 2.0).abs() < 1e-4, "lifting them changes nothing: {board:?}");
}

#[test]
fn space_held_drags_a_board_by_what_is_on_it_until_it_comes_up() {
    let mut ui = window(1.0);
    let (mut board, mut seen) = (Board::default(), Vec::new());
    settle(&mut ui, |ui| _ = plan(ui, &mut board, &mut seen));
    // On the first thing, with Space held: the board moves, and the thing is not pressed.
    key(&mut ui, Key::Space, Mods::NONE, |ui| _ = plan(ui, &mut board, &mut seen));
    drag(&mut ui, pt(60.0, 40.0), pt(90.0, 70.0), |ui| _ = plan(ui, &mut board, &mut seen));
    assert_eq!((board.x, board.y), (-30.0, -30.0));
    // Space comes up: that is news, as it was asked; and the same drag moves nothing now.
    assert!(ui.event(Event::KeyUp(Key::Space)));
    ui.run(&mut |ui| _ = plan(ui, &mut board, &mut seen));
    let thing = middle(seen[0].1);
    drag(&mut ui, thing, pt(thing.x + 30.0, thing.y + 30.0), |ui| _ = plan(ui, &mut board, &mut seen));
    assert_eq!((board.x, board.y), (-30.0, -30.0));
}

#[test]
fn a_board_is_written_as_three_numbers_and_read_back() {
    let board = Board { x: -12.5, y: 300.0, zoom: 0.75 };
    assert_eq!(board.to_string(), "-12.5 300 0.75");
    assert_eq!("-12.5 300 0.75".parse::<Board>(), Ok(board));
    assert!("1 2".parse::<Board>().is_err() && "1 2 0".parse::<Board>().is_err() && "1 2 NaN".parse::<Board>().is_err());
}
