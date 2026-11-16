//! Pictures turned upright by their EXIF orientation. A camera stores the picture as it saw it
//! and notes how it was held; JPEG, PNG and WebP carry the note the same way, and each reader
//! asks here how to lay its rows.
//!
//! The notes are TIFF entries; entry 0x0112 holds the orientation, 1 to 8 (1: upright). Other
//! entries are skipped.

/// For each orientation 1 to 8: mirror left to right, mirror top to bottom, then swap rows and
/// columns.
const TURNS: [(bool, bool, bool); 8] =
    [(false, false, false), (true, false, false), (true, true, false), (false, true, false), (false, false, true), (false, true, true), (true, true, true), (true, false, true)];

/// The orientation (1 to 8) in a file's EXIF notes. A JPEG puts "Exif\0\0" before them; PNG and
/// WebP do not (some writers do). Nothing for notes that do not say, or are damaged.
pub(crate) fn turn(notes: &[u8]) -> Option<usize> {
    let notes = notes.strip_prefix(b"Exif\0\0").unwrap_or(notes);
    // The byte order of the numbers.
    let low_first = match notes.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let short = |at: usize| notes.get(at..at + 2).map(|pair| if low_first { u16::from_le_bytes([pair[0], pair[1]]) } else { u16::from_be_bytes([pair[0], pair[1]]) } as usize);
    let list = if low_first { short(4)? | short(6)? << 16 } else { short(4)? << 16 | short(6)? };
    let entries = short(list.min(notes.len()))?;
    let entry = (0..entries).map(|entry| list + 2 + entry * 12).find(|at| short(*at) == Some(0x0112))?;
    short(entry + 8).filter(|turn| (1..=8).contains(turn))
}

/// The width of a `width` by `height` picture once turned `turn` way.
pub(crate) fn width(turn: usize, [width, height]: [usize; 2]) -> usize {
    if TURNS[turn - 1].2 { height } else { width }
}

/// Puts stored row `y` of a picture of `size` where it belongs once turned `turn` way (2 to 8;
/// 1 is as stored), in `pixels`: the shown picture, rows of `width(turn, size)` pixels.
pub(crate) fn lay_row(turn: usize, [width, height]: [usize; 2], y: usize, row: &[u32], pixels: &mut [u32]) {
    let (mirror, upend, swap) = TURNS[turn - 1];
    // A stored row runs along a row of the turned picture, or down a column.
    let (across, step) = if swap { (height, height) } else { (width, 1) };
    let start = if upend { height - 1 - y } else { y } * (across / step);
    let places = (0..width).map(|x| start + if mirror { width - 1 - x } else { x } * step);
    places.zip(row).for_each(|(place, pixel)| pixels[place] = *pixel);
}

/// A whole picture, `width` pixels a row, turned `turn` way: its new width and pixels. Upright
/// or unknown turns come back as they are.
pub(crate) fn turned(turn: Option<usize>, width: usize, pixels: Vec<u32>) -> (usize, Vec<u32>) {
    let Some(turn) = turn.filter(|turn| *turn > 1 && width > 0) else { return (width, pixels) };
    let size = [width, pixels.len() / width];
    let mut upright = vec![0; pixels.len()];
    for (y, row) in pixels.chunks_exact(width).enumerate() {
        lay_row(turn, size, y, row, &mut upright);
    }
    (self::width(turn, size), upright)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// EXIF notes giving an orientation, with numbers low byte first or high byte first, as a
    /// JPEG keeps them.
    pub(crate) fn notes(turn: u16, low_first: bool) -> Vec<u8> {
        let short = |number: u16| if low_first { number.to_le_bytes() } else { number.to_be_bytes() };
        let long = |number: u32| if low_first { number.to_le_bytes() } else { number.to_be_bytes() };
        let mut notes = if low_first { b"Exif\0\0II".to_vec() } else { b"Exif\0\0MM".to_vec() };
        // The TIFF mark (42), where the entry list starts, and its count: one other entry, then
        // the orientation.
        notes.extend(short(42));
        notes.extend(long(8));
        notes.extend(short(2));
        // An entry: its number, type SHORT, count, value.
        for (number, value) in [(0x0110, 7), (0x0112, turn)] {
            notes.extend(short(number));
            notes.extend(short(3));
            notes.extend(long(1));
            notes.extend(short(value));
            notes.extend(short(0));
        }
        notes
    }

    /// Where a pixel of the shown picture lies in the stored one (`width` by `height`): the
    /// eight turns as the standard draws them.
    pub(crate) fn from(turn: usize, [width, height]: [usize; 2], x: usize, y: usize) -> (usize, usize) {
        match turn {
            1 => (x, y),
            2 => (width - 1 - x, y),
            3 => (width - 1 - x, height - 1 - y),
            4 => (x, height - 1 - y),
            5 => (y, x),
            6 => (y, height - 1 - x),
            7 => (width - 1 - y, height - 1 - x),
            _ => (width - 1 - y, x),
        }
    }

    #[test]
    fn the_notes_say_how_to_turn_whichever_end_comes_first() {
        for (way, low_first) in (1..=8).flat_map(|way| [(way, true), (way, false)]) {
            assert_eq!(turn(&notes(way as u16, low_first)), Some(way as usize));
            // Without the prefix, as PNG and WebP keep them.
            assert_eq!(turn(&notes(way as u16, low_first)[6..]), Some(way as usize));
        }
        // An orientation out of range; notes cut short; no notes.
        assert_eq!([turn(&notes(9, true)), turn(&notes(0, false)), turn(&notes(6, true)[..30]), turn(b"Exif\0\0"), turn(b"")], [None; 5]);
    }

    #[test]
    fn a_picture_is_turned_each_of_eight_ways() {
        // A 5 by 3 picture whose every pixel differs.
        let (width, height, stored) = (5, 3, (0..15u32).collect::<Vec<u32>>());
        for turn in 1..=8 {
            let (across, shown) = turned(Some(turn), width, stored.clone());
            assert_eq!(across, if turn < 5 { width } else { height }, "turn {turn}: its width");
            let wanted = (0..15).map(|at| from(turn, [width, height], at % across, at / across)).map(|(x, y)| stored[y * width + x]);
            assert!(shown.iter().copied().eq(wanted), "turn {turn}");
        }
        assert_eq!(turned(None, width, stored.clone()), (width, stored));
    }

    /// A reader of one kind of picture file.
    type Reader = fn(&[u8]) -> Option<(usize, Vec<u32>)>;

    /// Holds JPEG, PNG and WebP files with each of the eight orientations against vips
    /// (`autorot`): the same pixels. (`cargo test --lib upright -- --ignored`, with vips
    /// installed.)
    #[test]
    #[ignore = "needs vips"]
    fn vips_turns_them_as_this_does() {
        // Without vips there is nothing to compare with: said, and skipped.
        if std::process::Command::new("vips").arg("--version").output().is_err() {
            return eprintln!("vips is not on this system: the turns are not held against it");
        }
        let work = std::env::temp_dir().join(format!("taut-upright-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let chunk_of = |name: &[u8], holds: &[u8]| [name, &(holds.len() as u32).to_le_bytes(), holds, &[0][..holds.len() % 2]].concat();
        let jpeg = include_bytes!("../tests/pictures/plain.jpg");
        let png = crate::png::encode(5, &(0..15u32).map(|at| at * 0x0a_0b0c).collect::<Vec<u32>>()).unwrap();
        let still = include_bytes!("../tests/pictures/exact-2.webp");
        let first = still.windows(4).position(|name| name == b"VP8L").unwrap();
        let long = u32::from_le_bytes([still[first + 4], still[first + 5], still[first + 6], still[first + 7]]) as usize;
        for (turn, low_first) in (1..=8).flat_map(|turn| [(turn, true), (turn, false)]) {
            let notes = notes(turn, low_first);
            let rows = png.windows(4).position(|name| name == b"IDAT").unwrap() - 4;
            let webp = [chunk_of(b"VP8X", &[0x08, 0, 0, 0, 32, 0, 0, 20, 0, 0]), still[first..first + 8 + long + long % 2].to_vec(), chunk_of(b"EXIF", &notes[6..])].concat();
            let files: [(&str, Vec<u8>, Reader); 3] = [
                ("jpg", [&jpeg[..2], &[0xFF, 0xE1, 0, notes.len() as u8 + 2], &notes, &jpeg[2..]].concat(), crate::jpeg::decode),
                ("png", [&png[..rows], &crate::png::chunk(b"eXIf", &notes[6..]), &png[rows..]].concat(), crate::png::decode),
                ("webp", [b"RIFF", &(webp.len() as u32 + 4).to_le_bytes()[..], b"WEBP", &webp].concat(), crate::webp::decode),
            ];
            for (kind, file, read) in files {
                let (from, to) = (work.join(format!("in.{kind}")), work.join("out.png"));
                std::fs::write(&from, &file).unwrap();
                let done = std::process::Command::new("vips").arg("autorot").arg(&from).arg(&to).status();
                assert!(done.is_ok_and(|status| status.success()), "vips autorot {kind}");
                let (vips, this) = (crate::png::decode(&std::fs::read(&to).unwrap()).unwrap(), read(&file).unwrap());
                let solid = |(width, pixels): (usize, Vec<u32>)| (width, pixels.iter().map(|pixel| pixel | 0xff00_0000).collect::<Vec<u32>>());
                assert!(solid(vips) == solid(this), "{kind}, turn {turn}, low end first: {low_first}");
            }
        }
        std::fs::remove_dir_all(work).unwrap();
    }
}
