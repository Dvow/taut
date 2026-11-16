//! Animated GIFs in a headless window; and picture formats told apart.
mod common;
use common::*;
use std::time::Duration;
use taut::{Color, Rect, Ui, gif, png};

/// Four frames of sixteen by twelve that show for 70, 100, 300 and 50 thousandths of a
/// second, three times over: written by hand, built by giflib.
const HAND: &[u8] = include_bytes!("pictures/hand.gif");
/// Five frames of 24 by 16, a fifth of a second each, for ever: ffmpeg's.
const MOVING: &[u8] = include_bytes!("pictures/moving.gif");

/// Frame `frame` of a film as other readers show it: the pixels of a picture that holds all
/// the frames one under the other, `high` rows to each.
fn frame(all: &[u8], high: usize, frame: usize) -> (usize, Vec<u32>) {
    let (width, pixels) = png::decode(all).unwrap();
    (width, pixels[frame * high * width..(frame + 1) * high * width].to_vec())
}

/// The part of the window's picture where a film of `wide` by `high` shows that was put at
/// the top left, as pixels with nothing behind them.
fn shown(ui: &Window, wide: usize, high: usize) -> Vec<u32> {
    (0..high).flat_map(|y| ui.screen.pixels[y * ui.screen.width..][..wide].to_vec()).collect()
}

/// A film's frame as it comes out on the window's page: what is clear shows the page.
fn on_page(frame: &[u32], page: u32) -> Vec<u32> {
    frame.iter().map(|pixel| if pixel >> 24 == 0 { page } else { *pixel }).collect()
}

#[test]
fn a_film_shows_the_frame_that_is_due_and_asks_to_be_run_for_the_next() {
    let mut ui = window(1.0);
    ui.set_time(Duration::from_secs(5));
    let film = ui.add_film(HAND).unwrap();
    let (page, at) = (ui.theme.bg.0, std::cell::Cell::new(Rect::EMPTY));
    let mut build = |ui: &mut Ui| {
        at.set(ui.el().film(film).sense().show().rect);
        ui.el().size(20.0, 20.0).fill(Color::rgb(0x336699)).show();
    };
    // It plays from when it was taken in: its first frame, and a run asked for when that
    // has shown for its 70 thousandths.
    settle(&mut ui, &mut build);
    assert_eq!((at.get().w, at.get().h, ui.wake_in()), (16.0, 12.0, Some(Duration::from_millis(70))));
    assert_eq!(shown(&ui, 16, 12), on_page(&frame(include_bytes!("pictures/hand.frames.png"), 12, 0).1, page));
    // Through its four frames, three times: each shows when it is due, the run for the next
    // is asked for, and nothing but the film's own box is drawn again. (The window checks
    // after every run that what was drawn again is all that changed.)
    let mut time = 5000;
    for (round, (frame_shown, shows)) in (0..3).flat_map(|round| [70u64, 100, 300, 50].into_iter().enumerate().map(move |frame| (round, frame))) {
        ui.set_time(Duration::from_millis(time));
        let changed = ui.run(&mut build);
        assert_eq!(shown(&ui, 16, 12), on_page(&frame(include_bytes!("pictures/hand.frames.png"), 12, frame_shown).1, page), "round {round}, frame {frame_shown}");
        assert!(changed.is_empty() || (changed.w <= 16.0 && changed.h <= 12.0 && changed.x == 0.0 && changed.y == 0.0), "{changed:?}");
        assert_eq!(ui.wake_in(), Some(Duration::from_millis(shows)), "round {round}, frame {frame_shown}");
        // (Run again before the frame is over, nothing changes, and less time is left.)
        ui.set_time(Duration::from_millis(time + shows - 1));
        assert!(ui.run(&mut build).is_empty() && ui.wake_in() == Some(Duration::from_millis(1)));
        time += shows;
    }
    // After the third time it stays at its last frame, and asks for nothing more.
    ui.set_time(Duration::from_millis(time));
    ui.run(&mut build);
    assert_eq!((ui.wake_in(), shown(&ui, 16, 12)), (None, on_page(&frame(include_bytes!("pictures/hand.frames.png"), 12, 3).1, page)));
    ui.set_time(Duration::from_secs(1000));
    assert!(ui.run(&mut build).is_empty() && ui.wake_in().is_none());
}

#[test]
fn a_film_is_shown_at_any_size_and_let_go_like_a_picture() {
    let mut ui = window(1.0);
    let (moving, hand) = (ui.add_film(MOVING).unwrap(), ui.add_film(HAND).unwrap());
    // Twice as large: each pixel of the frame is four of the window's, at its middle.
    settle(&mut ui, |ui| _ = ui.el().film(moving).size(48.0, 32.0).show());
    let first = frame(include_bytes!("pictures/moving.frames.png"), 16, 0).1;
    assert_eq!(ui.screen.pixels[20 * ui.screen.width + 21], first[10 * 24 + 10]);
    // Two films, each by its own times: the earlier of the two runs is the one asked for.
    let mut both = |ui: &mut Ui| {
        ui.el().film(moving).show();
        ui.el().film(hand).show();
    };
    settle(&mut ui, &mut both);
    assert_eq!(ui.wake_in(), Some(Duration::from_millis(70)));
    ui.set_time(Duration::from_millis(180));
    ui.run(&mut both);
    assert_eq!(ui.wake_in(), Some(Duration::from_millis(20)));
    // Let go, a film takes no room and shows nothing; and asks for no run.
    ui.drop_film(moving);
    ui.drop_film(hand);
    let mut after = Rect::EMPTY;
    settle(&mut ui, |ui| {
        both(ui);
        after = ui.el().size(4.0, 4.0).fill(Color::rgb(0x00ff00)).sense().show().rect;
    });
    assert_eq!((after.y, ui.wake_in()), (0.0, None));
    // What is no film is none. A film of one frame stands still.
    assert!(ui.add_film(b"GIF89a").is_none() && ui.add_film(b"no picture at all").is_none());
    let still = ui.add_film(include_bytes!("pictures/still.gif")).unwrap();
    settle(&mut ui, |ui| _ = ui.el().film(still).show());
    assert_eq!(ui.wake_in(), None);
}

#[test]
fn the_kinds_of_picture_files_are_told_apart_by_how_they_begin() {
    let mut ui = window(1.0);
    // A GIF as a picture is its first frame.
    let (gif, mut at) = (ui.add_picture_file(MOVING).unwrap(), Rect::EMPTY);
    settle(&mut ui, |ui| at = ui.el().picture(gif).sense().show().rect);
    let (page, first) = (ui.theme.bg.0, gif::decode(MOVING).unwrap());
    assert_eq!((at.w, at.h, first.0), (24.0, 16.0, 24));
    assert_eq!(shown(&ui, 24, 16), on_page(&first.1, page));
    // A WebP, stored with loss or without: the pixels another reader makes of the file.
    for (webp, read) in
        [(&include_bytes!("pictures/lossy.webp")[..], &include_bytes!("pictures/lossy.webp.png")[..]), (include_bytes!("pictures/exact.webp"), include_bytes!("pictures/exact.webp.png"))]
    {
        let picture = ui.add_picture_file(webp).unwrap();
        settle(&mut ui, |ui| at = ui.el().picture(picture).sense().show().rect);
        assert_eq!((at.w, at.h, shown(&ui, 61, 45)), (61.0, 45.0, png::decode(read).unwrap().1));
    }
    // A BMP likewise. And of an ICO the largest picture in it: which is solid at its
    // right edge, and less so to the left.
    let bmp = ui.add_picture_file(include_bytes!("pictures/bmp-24.bmp")).unwrap();
    settle(&mut ui, |ui| at = ui.el().picture(bmp).sense().show().rect);
    assert!((at.w, at.h) == (23.0, 17.0) && shown(&ui, 23, 17) == png::decode(include_bytes!("pictures/bmp-24.bmp.png")).unwrap().1);
    let (icon, wanted) = (ui.add_picture_file(include_bytes!("pictures/icon-png.ico")).unwrap(), png::decode(include_bytes!("pictures/icon-png.ico.png")).unwrap().1);
    settle(&mut ui, |ui| at = ui.el().picture(icon).sense().show().rect);
    assert!((at.w, at.h) == (23.0, 17.0) && (0..17).all(|y| ui.screen.pixels[y * ui.screen.width + 22] == wanted[y * 23 + 22]));
    // And the others as before: a PNG, a JPEG; and what is none of them is none.
    assert!(ui.add_picture_file(include_bytes!("pictures/plain.png")).is_some() && ui.add_picture_file(include_bytes!("pictures/plain.jpg")).is_some());
    let none = [&b"GIF89a, but no more"[..], b"RIFF\x04\0\0\0WEBP", b"BM, but no more", b"\0\0\x01\0\x01\0", b""];
    assert!(none.iter().all(|file| ui.add_picture_file(file).is_none()));
}
