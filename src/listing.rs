//! Folder contents for the file dialog: read once, shown while loading.
//!
//! The first few hundred items are read at once, so a folder of that size shows whole when the
//! dialog first draws. The rest is read on a thread, handed over in batches: each dialog run
//! takes what came, merges it into place, and shows it. Sorting, searching and hidden files
//! never reread anything: they only change the list of visible indices. So 100,000 files cost a
//! dialog run what a screenful does; beyond that, one read, and one sort per change of order.
//!
//! Names are stored back to back in one string, each item holding its range: 100,000 names are
//! one allocation.
//!
//! Thumbnails are read on a thread too, one or two at a time, only for files in view: scrolling
//! through 10,000 photos requests only those that stop in view.
use std::cmp::Ordering;
use std::fs::{DirEntry, Metadata, ReadDir};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Items read before the dialog first draws: a folder this size shows whole at once. The rest
/// is read on a thread.
const AT_ONCE: usize = 512;
/// And at most this long, so a slow disk doesn't delay the first view.
const AT_MOST: Duration = Duration::from_millis(15);
/// Items the reading thread hands over at a time.
const BATCH: usize = 4096;

/// `Entry::flags` bits: folder; hidden; selected; an image whose thumbnail was requested.
const FOLDER: u8 = 1;
const HIDDEN: u8 = 2;
const PICKED: u8 = 4;
const ASKED: u8 = 8;

/// Extensions of files with thumbnails.
const PICTURES: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"];

/// Decodes an image file's bytes: width, and pixels.
pub(crate) type Reader = fn(&[u8]) -> Option<(usize, Vec<u32>)>;
/// A returned thumbnail: its item, and the image (`None` if the file could not be decoded).
type Answer = (u32, Option<(usize, Vec<u32>)>);
/// What a thread hands over, and whether it is done: shared with the dialog, taking turns. (A
/// thread holding the only reference knows nobody wants its work any more.)
type Handed<T> = Arc<Mutex<(Vec<T>, bool)>>;

/// What a list column sorts by.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum By {
    /// Name, in natural order (see `natural`).
    #[default]
    Name,
    /// File size.
    Size,
    /// File type: its extension.
    Kind,
    /// Modification time.
    Date,
}

/// One folder item.
#[derive(Clone, Copy)]
pub(crate) struct Entry {
    /// Its name's range among the listing's names.
    name: [u32; 2],
    /// Size in bytes.
    pub size: u64,
    /// Modification time, in seconds since 1970 UTC.
    pub time: i64,
    /// What it is (see `FOLDER` and the rest).
    flags: u8,
}

impl Entry {
    /// A folder (or a drive's root).
    pub fn folder(&self) -> bool {
        self.flags & FOLDER != 0
    }
    /// Selected by the user.
    pub fn picked(&self) -> bool {
        self.flags & PICKED != 0
    }
}

/// Items read on the reading thread, as handed over: their names back to back, and each item
/// (its name range counted from the start of these names).
#[derive(Default)]
struct Batch {
    names: String,
    entries: Vec<Entry>,
}

/// Which items are wanted: folders always, and files ending in one of `endings` (all, if
/// empty), unless only folders are.
#[derive(Clone, Default)]
pub(crate) struct Wanted {
    pub folders_only: bool,
    pub endings: Vec<String>,
}

impl Wanted {
    /// Whether a file of this name is wanted.
    fn file(&self, name: &str) -> bool {
        !self.folders_only && (self.endings.is_empty() || self.endings.iter().any(|ending| ends_in(name, ending)))
    }
}

/// One folder's contents, as far as read.
#[derive(Default)]
pub(crate) struct Listing {
    /// The folder. (Empty: above the drives, which are its items.)
    pub folder: PathBuf,
    /// All names, back to back.
    names: String,
    /// Items in read order: an item keeps its index while the listing lives.
    entries: Vec<Entry>,
    /// All indices, in the requested order.
    order: Vec<u32>,
    /// The visible ones, in that order: hidden items only if shown, and only those matching the
    /// search.
    pub found: Vec<u32>,
    /// The requested order: key, and whether descending.
    by: By,
    down: bool,
    /// The search, lower-cased; and whether hidden items show.
    seek: String,
    hidden: bool,
    /// What is still to come from the reading thread, while it reads.
    coming: Option<Handed<Batch>>,
    /// The error reading the folder, as the system words it.
    pub trouble: String,
    /// The last selected item: where a Shift range starts.
    pub anchor: u32,
}

impl Listing {
    /// Reads a folder, keeping only the `wanted` items: what can be read at once, the rest on a
    /// thread. In the requested order, with hidden items or without.
    pub fn of(folder: PathBuf, wanted: Wanted, (by, down): (By, bool), hidden: bool) -> Listing {
        let mut listing = Listing { folder, by, down, hidden, ..Default::default() };
        // On Windows, above a drive's root are the drives.
        if listing.folder.as_os_str().is_empty() {
            let mut batch = Batch::default();
            crate::places::drive_roots().for_each(|drive| batch.add(&drive.to_string_lossy(), 0, 0, FOLDER));
            listing.take_in(vec![batch]);
            return listing;
        }
        let (inside, started) = (quietly(|| std::fs::read_dir(&listing.folder)), Instant::now());
        let mut inside = match inside {
            Ok(inside) => inside,
            Err(error) => return Listing { trouble: error.to_string(), ..listing },
        };
        let mut batch = Batch::default();
        let ended = read_some(&mut inside, &mut batch, &wanted, |count| count >= AT_ONCE || started.elapsed() > AT_MOST);
        listing.take_in(vec![batch]);
        if !ended {
            let coming = Handed::default();
            listing.coming = Some(coming.clone());
            std::thread::spawn(move || quietly(move || read_rest(inside, &wanted, &coming)));
        }
        listing
    }

    /// Whether the folder is still being read.
    pub fn reading(&self) -> bool {
        self.coming.is_some()
    }
    /// Takes what the reading thread handed over since last time. Returns whether anything
    /// came, or the reading ended.
    pub fn take_coming(&mut self) -> bool {
        let Some(coming) = &self.coming else { return false };
        let Ok(mut handed) = coming.lock() else { return false };
        let (batches, ended) = (std::mem::take(&mut handed.0), handed.1);
        drop(handed);
        if ended {
            self.coming = None;
        }
        let came = !batches.is_empty();
        if came {
            self.take_in(batches);
        }
        came || ended
    }

    /// Merges read items into place in the requested order, and recomputes which are visible.
    fn take_in(&mut self, batches: Vec<Batch>) {
        let first = self.entries.len();
        for batch in batches {
            let start = self.names.len() as u32;
            self.names.push_str(&batch.names);
            self.entries.extend(batch.entries.into_iter().map(|entry| Entry { name: entry.name.map(|end| end + start), ..entry }));
        }
        // The new ones sorted alone, then both together: a sort that finds two sorted runs only
        // merges them, in one pass.
        let new = self.order.len();
        self.order.extend(first as u32..self.entries.len() as u32);
        let mut order = std::mem::take(&mut self.order);
        self.put_in_order(&mut order[new..]);
        self.put_in_order(&mut order);
        self.order = order;
        self.narrow();
    }
    /// Sorts indices in the requested order. (One sort for every use: each separate sort would
    /// add its own code to the program.)
    fn put_in_order(&self, order: &mut [u32]) {
        order.sort_by(|a, b| self.compare(*a, *b));
    }

    /// Re-sorts everything: by what, and whether descending.
    pub fn sort(&mut self, by: By, down: bool) {
        (self.by, self.down) = (by, down);
        let mut order = std::mem::take(&mut self.order);
        self.put_in_order(&mut order);
        self.order = order;
        self.narrow();
    }
    /// Shows only names containing `seek` (all, if empty), with hidden items or without.
    pub fn show(&mut self, seek: &str, hidden: bool) {
        let seek = seek.to_lowercase();
        if (seek.as_str(), hidden) != (self.seek.as_str(), self.hidden) {
            (self.seek, self.hidden) = (seek, hidden);
            self.narrow();
        }
    }
    /// Recomputes the visible items.
    fn narrow(&mut self) {
        let Listing { names, entries, order, found, seek, hidden, .. } = self;
        let shows = |entry: &Entry| (*hidden || entry.flags & HIDDEN == 0) && (seek.is_empty() || has(&names[entry.name[0] as usize..entry.name[1] as usize], seek));
        found.clear();
        found.extend(order.iter().copied().filter(|at| shows(&entries[*at as usize])));
    }

    /// The order of two items: folders before files, then as requested, ties by name.
    fn compare(&self, a: u32, b: u32) -> Ordering {
        let (one, two) = (&self.entries[a as usize], &self.entries[b as usize]);
        let way = match self.by {
            By::Name => natural(self.name(a), self.name(b)),
            By::Size => one.size.cmp(&two.size),
            By::Kind => natural(ending(self.name(a)), ending(self.name(b))),
            By::Date => one.time.cmp(&two.time),
        };
        // Descending reverses the requested key, not the names among ties.
        let way = if self.down { way.reverse() } else { way };
        (two.flags & FOLDER).cmp(&(one.flags & FOLDER)).then(way).then_with(|| natural(self.name(a), self.name(b)))
    }

    /// The item at this index.
    pub fn entry(&self, at: u32) -> &Entry {
        &self.entries[at as usize]
    }
    /// Its name.
    pub fn name(&self, at: u32) -> &str {
        let [from, to] = self.entries[at as usize].name;
        &self.names[from as usize..to as usize]
    }
    /// The first visible row whose name starts with `start` (case-insensitive), searching from
    /// `from` and wrapping.
    pub fn starting(&self, start: &str, from: usize) -> Option<usize> {
        let count = self.found.len();
        let starts = |row: &usize| has_at_start(self.name(self.found[*row]), start);
        (from..count).chain(0..from.min(count)).find(starts)
    }

    /// Selects only this item, or nothing.
    pub fn pick_only(&mut self, at: Option<u32>) {
        self.entries.iter_mut().for_each(|entry| entry.flags &= !PICKED);
        if let Some(entry) = at.and_then(|at| self.entries.get_mut(at as usize)) {
            entry.flags |= PICKED;
        }
        self.anchor = at.unwrap_or(self.anchor);
    }
    /// A click on visible row `row` with modifiers, as in any list: alone it selects only that;
    /// with Ctrl it toggles; with Shift it selects from the anchor to it.
    pub fn click(&mut self, row: usize, ctrl: bool, shift: bool) {
        let Some(&at) = self.found.get(row) else { return };
        let from = self.found.iter().position(|other| *other == self.anchor).unwrap_or(row);
        if !ctrl {
            self.entries.iter_mut().for_each(|entry| entry.flags &= !PICKED);
        }
        let rows = if shift { from.min(row)..from.max(row) + 1 } else { row..row + 1 };
        let on = shift || self.entries[at as usize].flags & PICKED == 0;
        for row in rows {
            let entry = &mut self.entries[self.found[row] as usize];
            entry.flags = if on { entry.flags | PICKED } else { entry.flags & !PICKED };
        }
        if !shift {
            self.anchor = at;
        }
    }
    /// Selects everything visible.
    pub fn pick_all(&mut self) {
        let Listing { entries, found, .. } = self;
        found.iter().for_each(|at| entries[*at as usize].flags |= PICKED);
    }
    /// The selected items, in visible order. (Selected items hidden by the search are left
    /// out.)
    pub fn picked(&self) -> impl Iterator<Item = u32> + '_ {
        self.found.iter().copied().filter(|at| self.entries[*at as usize].flags & PICKED != 0)
    }

    /// Whether this item's thumbnail can be read and was not yet requested: if so, it now is
    /// (returns `true` once).
    pub fn ask_picture(&mut self, at: u32) -> bool {
        let fresh = self.is_picture(at) && self.entries[at as usize].flags & ASKED == 0;
        self.entries[at as usize].flags |= if fresh { ASKED } else { 0 };
        fresh
    }
    /// Whether this item is an image file, by extension.
    pub fn is_picture(&self, at: u32) -> bool {
        !self.entries[at as usize].folder() && PICTURES.iter().any(|kind| ending(self.name(at)).eq_ignore_ascii_case(kind))
    }
    /// Marks this item's thumbnail as dropped, so it is requested again when visible again.
    pub fn forget_picture(&mut self, at: u32) {
        self.entries[at as usize].flags &= !ASKED;
    }
}

impl Batch {
    /// Adds an item.
    fn add(&mut self, name: &str, size: u64, time: i64, flags: u8) {
        let from = self.names.len() as u32;
        self.names.push_str(name);
        self.entries.push(Entry { name: [from, self.names.len() as u32], size, time, flags });
    }
}

/// Reads items into a batch until `enough` says so or the folder ends. Returns whether it
/// ended.
fn read_some(inside: &mut ReadDir, batch: &mut Batch, wanted: &Wanted, enough: impl Fn(usize) -> bool) -> bool {
    while !enough(batch.entries.len()) {
        let Some(found) = inside.next() else { return true };
        if let Some((name, kind)) = found.ok().and_then(|found| looked_at(&found)) {
            let (folder, hidden) = (kind.is_dir(), is_hidden(&name, &kind));
            let time = kind.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |since| since.as_secs() as i64);
            if folder || wanted.file(&name) {
                batch.add(&name, kind.len(), time, (FOLDER * folder as u8) | (HIDDEN * hidden as u8));
            }
        }
    }
    false
}

/// An item's name and metadata: for a link, its target's (a link to a folder is a folder), or
/// the link's own where the target is missing.
fn looked_at(found: &DirEntry) -> Option<(String, Metadata)> {
    let kind = found.metadata().ok()?;
    let kind = if kind.file_type().is_symlink() { std::fs::metadata(found.path()).unwrap_or(kind) } else { kind };
    Some((found.file_name().to_string_lossy().into_owned(), kind))
}

/// Reads the rest of a folder on a thread, handing it over in batches. Stops at the end, or
/// when nobody wants it: its listing is gone.
fn read_rest(mut inside: ReadDir, wanted: &Wanted, handed: &Handed<Batch>) {
    let mut ended = false;
    while !ended && Arc::strong_count(handed) > 1 {
        let mut batch = Batch::default();
        ended = read_some(&mut inside, &mut batch, wanted, |count| count >= BATCH);
        if let Ok(mut handed) = handed.lock() {
            handed.0.push(batch);
            handed.1 = ended;
        }
    }
}

/// Whether an item is hidden: by Windows' attribute; elsewhere by a leading dot.
#[cfg(windows)]
fn is_hidden(_: &str, kind: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    kind.file_attributes() & 2 != 0
}
#[cfg(not(windows))]
fn is_hidden(name: &str, _: &Metadata) -> bool {
    name.starts_with('.')
}

/// Runs `work` without the system's "no disk in drive" message box: Windows may show one;
/// elsewhere there is nothing to suppress.
pub(crate) fn quietly<T>(work: impl FnOnce() -> T) -> T {
    #[cfg(windows)]
    return crate::windows::quietly(work);
    #[cfg(not(windows))]
    work()
}

/// Natural order: case-insensitive, with digit runs compared as numbers, so "file 2" sorts
/// before "file 10" and "file 9" before both. Names equal that way fall back to their
/// characters, so no two names are ever equal.
pub(crate) fn natural(a: &str, b: &str) -> Ordering {
    let (mut one, mut two) = (a, b);
    while let (Some(x), Some(y)) = (one.chars().next(), two.chars().next()) {
        let (way, [past_one, past_two]) = match x.is_ascii_digit() && y.is_ascii_digit() {
            true => number(one, two),
            false => (small(x).cmp(&small(y)), [x.len_utf8(), y.len_utf8()]),
        };
        if way != Ordering::Equal {
            return way;
        }
        (one, two) = (&one[past_one..], &two[past_two..]);
    }
    one.len().cmp(&two.len()).then_with(|| a.cmp(b))
}

/// Compares two digit runs at the start of two names by value ("007" equals "7"); and each
/// run's length in bytes.
fn number(one: &str, two: &str) -> (Ordering, [usize; 2]) {
    let long = |name: &str| name.bytes().take_while(u8::is_ascii_digit).count();
    let (a, b) = (long(one), long(two));
    let (x, y) = (one[..a].trim_start_matches('0'), two[..b].trim_start_matches('0'));
    (x.len().cmp(&y.len()).then_with(|| x.cmp(y)), [a, b])
}

/// A character lower-cased: the first character of its lower case.
fn small(character: char) -> char {
    match character.is_ascii() {
        true => character.to_ascii_lowercase(),
        false => character.to_lowercase().next().unwrap_or(character),
    }
}

/// A name's extension, after its last dot: empty with none, or with only a leading dot
/// (".profile" has none).
pub(crate) fn ending(name: &str) -> &str {
    name.rfind('.').filter(|at| *at > 0).map_or("", |at| &name[at + 1..])
}

/// Whether a name ends in `ending` (lower case): ".png" for "Photo.PNG".
pub(crate) fn ends_in(name: &str, ending: &str) -> bool {
    let start = name.len().checked_sub(ending.len()).filter(|start| name.is_char_boundary(*start));
    start.is_some_and(|start| name[start..].to_lowercase() == ending)
}

/// Whether a name contains `seek` (lower case).
fn has(name: &str, seek: &str) -> bool {
    match name.is_ascii() && seek.is_ascii() {
        true => name.as_bytes().windows(seek.len()).any(|part| part.eq_ignore_ascii_case(seek.as_bytes())),
        false => name.to_lowercase().contains(seek),
    }
}

/// Whether a name starts with `start`, case-insensitive.
fn has_at_start(name: &str, start: &str) -> bool {
    let mut name = name.chars().map(small);
    start.chars().map(small).all(|character| name.next() == Some(character))
}

/// Image thumbnails, each read on its own thread: each returns at most about twice the asked
/// size each way.
#[derive(Default)]
pub(crate) struct Small {
    back: Handed<Answer>,
    /// Requested and not yet back.
    pub out: usize,
}

impl Small {
    /// Requests a thumbnail of `file`, item `at`, decoded by `read`.
    pub fn ask(&mut self, at: u32, file: PathBuf, read: Reader, most: usize) {
        let back = self.back.clone();
        std::thread::spawn(move || read_small(at, &file, read, most, &back));
        self.out += 1;
    }
    /// A returned thumbnail, if any: its item, and the image (`None` if not decodable).
    pub fn answer(&mut self) -> Option<Answer> {
        let answer = self.back.lock().ok()?.0.pop();
        self.out -= answer.is_some() as usize;
        answer
    }
}

/// Reads a thumbnail on its own thread and hands it back (unless nobody wants it any more).
fn read_small(at: u32, file: &Path, read: Reader, most: usize, back: &Handed<Answer>) {
    if Arc::strong_count(back) == 1 {
        return;
    }
    let small = std::fs::read(file).ok().and_then(|bytes| read(&bytes)).map(|(width, pixels)| shrunk(width, pixels, most));
    if let Ok(mut handed) = back.lock() {
        handed.0.push((at, small));
    }
}

/// An image halved until under twice `most` pixels each way.
fn shrunk(mut width: usize, mut pixels: Vec<u32>, most: usize) -> (usize, Vec<u32>) {
    while width.max(pixels.len() / width.max(1)) >= 2 * most && width > 1 {
        (width, pixels) = crate::canvas::halve(&pixels, width);
    }
    (width, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_in_the_order_people_expect() {
        let mut names = ["file 10.txt", "File 2.txt", "file 9.txt", "a", "B", "file 02.txt", "file 1.txt", "Ångström", "z", "10", "9", "file 2.txt"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["9", "10", "a", "B", "file 1.txt", "File 2.txt", "file 02.txt", "file 2.txt", "file 9.txt", "file 10.txt", "z", "Ångström"]);
        // Long digit runs compare too, past any integer type.
        assert_eq!(natural("x 99999999999999999999999", "x 100000000000000000000000"), Ordering::Less);
        assert_eq!((ending("photo.PNG"), ending(".profile"), ending("notes"), ending("a.tar.gz")), ("PNG", "", "", "gz"));
        assert!(ends_in("Photo.PNG", ".png") && !ends_in("png", ".png") && !ends_in("x.pngx", ".png"));
    }

    #[test]
    fn a_long_folder_is_read_in_the_background_and_shown_as_it_comes() {
        // More than is read at once, so the rest comes from the reading thread.
        let folder = std::env::temp_dir().join(format!("taut-listing-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        for number in 0..3000 {
            std::fs::write(folder.join(format!("file {number}.txt")), vec![b'x'; number % 7]).unwrap();
        }
        std::fs::write(folder.join(".hidden"), "").unwrap();
        std::fs::write(folder.join("picture.PNG"), "").unwrap();
        let wanted = Wanted { folders_only: false, endings: vec![".txt".into(), ".png".into()] };
        let asked = Instant::now();
        let mut listing = Listing::of(folder.clone(), wanted, (By::Name, false), false);
        // As much as is read at once, unless that took longer than the first read may: on a
        // busy machine the rest then comes from the thread too.
        let took = asked.elapsed();
        assert!(listing.found.len() >= AT_ONCE.min(3000) || !listing.reading() || took > AT_MOST, "the first part shows at once");
        let started = Instant::now();
        while listing.reading() && started.elapsed() < Duration::from_secs(20) {
            listing.take_coming();
            std::thread::sleep(Duration::from_millis(1));
        }
        // Everything wanted, the folder first, then by natural name order; the hidden file is
        // not wanted (no wanted extension), nor are the others.
        let names: Vec<&str> = listing.found.iter().map(|at| listing.name(*at)).collect();
        assert_eq!(names.len(), 3002);
        assert_eq!(names[..4], ["sub", "file 0.txt", "file 1.txt", "file 2.txt"]);
        assert_eq!(names[3001], "picture.PNG");
        // By size, descending: folders still first, and files of equal size by name.
        listing.sort(By::Size, true);
        let first: Vec<&str> = listing.found[..3].iter().map(|at| listing.name(*at)).collect();
        assert_eq!(first, ["sub", "file 6.txt", "file 13.txt"]);
        // Searched by part of the name, case-insensitive.
        listing.show("E 299", false);
        let names: Vec<&str> = listing.found.iter().map(|at| listing.name(*at)).collect();
        assert_eq!(names.len(), 11, "{names:?}");
        // Selected with Ctrl and Shift.
        listing.show("", false);
        listing.sort(By::Name, false);
        listing.click(1, false, false);
        listing.click(3, false, true);
        listing.click(10, true, false);
        listing.click(2, true, false);
        let picked: Vec<&str> = listing.picked().map(|at| listing.name(at)).collect();
        assert_eq!(picked, ["file 0.txt", "file 2.txt", "file 9.txt"]);
        assert_eq!(listing.starting("FILE 1", 0), Some(2));
        assert_eq!(listing.starting("file 1", 3), Some(11), "file 10, after file 9");
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn hidden_files_show_when_asked() {
        let folder = std::env::temp_dir().join(format!("taut-hidden-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("seen.txt"), "").unwrap();
        std::fs::write(folder.join(".unseen"), "").unwrap();
        #[cfg(windows)]
        std::process::Command::new("attrib").arg("+h").arg(folder.join(".unseen")).status().unwrap();
        let mut listing = Listing::of(folder.clone(), Wanted::default(), (By::Name, false), false);
        let names = |listing: &Listing| listing.found.iter().map(|at| listing.name(*at).to_string()).collect::<Vec<_>>();
        assert_eq!(names(&listing), ["seen.txt"]);
        listing.show("", true);
        assert_eq!(names(&listing), [".unseen", "seen.txt"]);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
