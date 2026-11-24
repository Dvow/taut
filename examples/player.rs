//! A video player: the file or address named on the command line, with its controls.
//!
//! `cargo run --example player --features video -- clip.mp4`
use taut::{Video, Window};

fn main() {
    let Some(source) = std::env::args().nth(1) else {
        return eprintln!("Which video? cargo run --example player --features video -- clip.mp4");
    };
    let mut video = match Video::open(source.as_str()) {
        Ok(video) => video,
        Err(why) => return eprintln!("{why}"),
    };
    video.play();
    let window = Window { title: "Player", width: 640.0, height: 440.0, ..Default::default() };
    taut::run(window, move |ui| {
        ui.column().pad(8.0).gap(8.0).open();
        ui.video(&mut video).grow().show();
        if let Some(why) = video.error() {
            ui.label(why);
        }
        ui.video_controls(&mut video);
        ui.close();
    });
}
