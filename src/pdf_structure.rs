//! A paper's structure, as screen readers are told a window's: what each part of the pages is
//! (a heading, a paragraph, a list, a table and its headings, a picture and what it shows), in
//! reading order, in what language. A PDF with it is "tagged": a screen reader reads it in
//! order, a viewer reflows it, and an outline (bookmarks) lists its headings.
//!
//! An app opens and closes parts as it opens and closes boxes ([`Paper::begin`],
//! [`Paper::end`]); [`Paper::markdown`] does it for every block. What is written while a part
//! is open belongs to it: words always; shapes and pictures only in a picture part (elsewhere
//! they are decoration, which readers skip); a link with the link words around it. Words
//! written with no part open are a paragraph of their own.
//!
//! In the file, each part is a structure element (PDF's standard types: `P`, `H1`–`H6`, `L`,
//! `LI`, `Table`, `TH`, `Figure`, …), and what a page draws for it is marked in the page's
//! content by number (`BDC` … `EMC`); a parent tree leads back from those numbers, and from
//! link annotations, to their elements. Everything else on a page is an artifact.
use crate::accessibility::Role;
use crate::paper::{Mark, Paper};
use crate::pdf::{Annotations, File, num, text};

/// What a part of the pages is: the structure types of PDF's standard set taut writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tag {
    Document,
    Paragraph,
    Heading(u8),
    List,
    Item,
    Label,
    Body,
    Code,
    Quote,
    Table,
    Row,
    /// A column heading.
    Head,
    Cell,
    Figure,
    Link,
    Division,
    /// A form field (`pdf_forms.rs`).
    Form,
    /// Decoration: page numbers, running heads. Readers skip it, as anything in it.
    Decoration,
}

impl Tag {
    /// Its standard name in a file.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Tag::Document => "Document",
            Tag::Paragraph => "P",
            Tag::Heading(level) => ["H1", "H2", "H3", "H4", "H5", "H6"][(level.clamp(1, 6) - 1) as usize],
            Tag::List => "L",
            Tag::Item => "LI",
            Tag::Label => "Lbl",
            Tag::Body => "LBody",
            Tag::Code => "Code",
            Tag::Quote => "BlockQuote",
            Tag::Table => "Table",
            Tag::Row => "TR",
            Tag::Head => "TH",
            Tag::Cell => "TD",
            Tag::Figure => "Figure",
            Tag::Link => "Link",
            Tag::Division => "Div",
            Tag::Form => "Form",
            Tag::Decoration => "Artifact",
        }
    }

    /// The part a role is on paper.
    fn of(role: Role) -> Tag {
        match role {
            Role::Text | Role::Paragraph => Tag::Paragraph,
            Role::Heading => Tag::Heading(1),
            Role::List | Role::Menu | Role::MenuBar | Role::Tabs | Role::Tree => Tag::List,
            Role::ListItem | Role::MenuItem | Role::MenuItemRadio | Role::Submenu | Role::Tab | Role::TreeItem | Role::Row => Tag::Item,
            Role::Code => Tag::Code,
            Role::Quote => Tag::Quote,
            Role::Table => Tag::Table,
            Role::TableRow => Tag::Row,
            Role::ColumnHeader => Tag::Head,
            Role::Cell => Tag::Cell,
            Role::Picture | Role::Progress => Tag::Figure,
            Role::Link => Tag::Link,
            Role::Button | Role::CheckBox | Role::Switch | Role::RadioButton | Role::Slider | Role::SpinButton | Role::TextBox | Role::PasswordBox | Role::ComboBox | Role::Fold => Tag::Form,
            _ => Tag::Division,
        }
    }
}

/// A part of the pages: what it is, the part it is in, what it shows where it is a picture
/// (or says where it is a form field), its language (BCP 47), and what it holds in reading
/// order.
#[derive(Debug)]
pub(crate) struct Part {
    pub tag: Tag,
    pub up: u32,
    pub alt: String,
    pub language: String,
    pub held: Vec<Held>,
}

/// What a part holds: a part within it, or a mark on a page (page, and index among its marks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Held {
    Part(u32),
    Mark(u32, u32),
}

/// An outline entry: a heading's level and words, its page, and how far down it is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bookmark {
    pub level: u32,
    pub title: String,
    pub page: usize,
    pub y: f32,
}

impl Paper {
    /// Opens a part of the pages' structure: what is written until [`end`](Self::end) is in
    /// it, as a box's content is told to screen readers as in the box. A role says what it is:
    /// [`Role::Paragraph`], [`Role::List`] and [`Role::ListItem`], [`Role::Table`],
    /// [`Role::TableRow`], [`Role::ColumnHeader`] and [`Role::Cell`], [`Role::Picture`] (then
    /// [`describe`](Self::describe) it), [`Role::Link`], [`Role::Quote`], [`Role::Code`];
    /// others are a group. Words written with none open are a paragraph each.
    ///
    /// ```
    /// use taut::{Color, Font, Paper, Path, Role};
    /// let mut paper = Paper::new();
    /// paper.begin_heading(1);
    /// paper.text("Sales", 72.0, 72.0, Font::new(24.0), Color::BLACK);
    /// paper.end();
    /// paper.begin(Role::Picture);
    /// paper.describe("Sales rose every month");
    /// let mut line = Path::new();
    /// line.move_to(72.0, 300.0).line_to(400.0, 150.0);
    /// paper.outline(&line, 3.0, Color::BLACK);
    /// paper.end();
    /// assert!(String::from_utf8_lossy(&paper.to_pdf()).contains("/S /Figure"));
    /// ```
    pub fn begin(&mut self, role: Role) {
        self.begin_tag(Tag::of(role));
    }
    /// Opens a heading of `level` (1 the main one, at most 6): its words are a bookmark too, in
    /// the viewer's outline.
    pub fn begin_heading(&mut self, level: u32) {
        self.begin_tag(Tag::Heading(level.clamp(1, 6) as u8));
    }
    /// Opens decoration: page numbers, running heads, lines. Readers skip it.
    pub fn begin_decoration(&mut self) {
        self.begin_tag(Tag::Decoration);
    }
    /// Says what the part opened last shows, for those who can't see it: a picture's
    /// description (its alternative text).
    pub fn describe(&mut self, words: &str) {
        if let Some(part) = self.open.last().copied() {
            self.parts[part as usize].alt = words.to_string();
        }
    }
    /// Closes the part opened last. A heading's words become a bookmark.
    pub fn end(&mut self) {
        let Some(part) = self.open.pop() else { return };
        let Tag::Heading(level) = self.parts[part as usize].tag else { return };
        let (mut title, mut first) = (String::new(), None);
        for held in &self.parts[part as usize].held {
            let Held::Mark(page, at) = *held else { continue };
            let Mark::Words(words) = &self.pages[page as usize].marks[at as usize] else { continue };
            title = if title.is_empty() { words.text.clone() } else { format!("{title} {}", words.text) };
            let top = words.glyphs.first().map_or(0.0, |glyph| glyph.at.y - words.size);
            first = first.or(Some((page as usize, top)));
        }
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if let Some((page, y)) = first.filter(|_| !title.is_empty()) {
            self.bookmarks.push(Bookmark { level: level as u32, title, page, y: y.max(0.0) });
        }
    }

    /// Opens a part of `tag` in the open one (or the document): inside decoration, decoration.
    /// Gives back its index.
    pub(crate) fn begin_tag(&mut self, tag: Tag) -> u32 {
        let up = self.innermost();
        let tag = if self.parts[up as usize].tag == Tag::Decoration { Tag::Decoration } else { tag };
        self.parts.push(Part { tag, up, alt: String::new(), language: self.said.clone(), held: Vec::new() });
        let part = self.parts.len() as u32 - 1;
        self.parts[up as usize].held.push(Held::Part(part));
        self.open.push(part);
        part
    }

    /// The part open innermost: the document where none is (made at first need).
    fn innermost(&mut self) -> u32 {
        if self.parts.is_empty() {
            self.parts.push(Part { tag: Tag::Document, up: 0, alt: String::new(), language: self.said.clone(), held: Vec::new() });
        }
        self.open.last().copied().unwrap_or(0)
    }

    /// Gives the mark just put its part: words go in the open part (or a paragraph of their
    /// own); shapes and pictures only in a picture; a link in a link part (one of its own if
    /// none is open). Decoration takes nothing.
    pub(crate) fn own_mark(&mut self) {
        let (page, at) = (self.on, self.pages[self.on].marks.len() - 1);
        let held = Held::Mark(page as u32, at as u32);
        let inner = self.innermost();
        let tag = self.parts[inner as usize].tag;
        let words = matches!(self.pages[page].marks[at], Mark::Words(_));
        let (into, alone) = match &self.pages[page].marks[at] {
            Mark::Clip(_) | Mark::Unclip => return,
            _ if tag == Tag::Decoration => return,
            _ if words && inner == 0 => (Tag::Paragraph, true),
            Mark::Field(_) if tag != Tag::Form => (Tag::Form, true),
            Mark::Link(..) if tag != Tag::Link => (Tag::Link, true),
            Mark::Shape(_) | Mark::Picture(..) if tag != Tag::Figure => return,
            _ => (tag, false),
        };
        let part = if alone { self.begin_tag(into) } else { inner };
        self.parts[part as usize].held.push(held);
        if alone {
            self.open.pop();
        }
    }
}

/// Which part each mark on each page belongs to, if any, and the number its content is marked
/// with: consecutive marks of one part share one, unless a clip comes between. Links and clips
/// have none.
pub(crate) struct Marked {
    pub owner: Vec<Vec<Option<u32>>>,
    pub number: Vec<Vec<Option<u32>>>,
    /// Per page, each number's part.
    pub parts: Vec<Vec<u32>>,
    /// Whether a part (or one it is in) is decoration.
    pub skipped: Vec<bool>,
}

/// Works out each mark's part and number.
pub(crate) fn marked(paper: &Paper) -> Marked {
    let mut owner: Vec<Vec<Option<u32>>> = paper.pages.iter().map(|sheet| vec![None; sheet.marks.len()]).collect();
    let mut skipped = vec![false; paper.parts.len()];
    for (at, part) in paper.parts.iter().enumerate() {
        // (A part comes after the one it is in.)
        skipped[at] = part.tag == Tag::Decoration || (at > 0 && skipped[part.up as usize]);
        for held in part.held.iter().filter(|_| !skipped[at]) {
            if let Held::Mark(page, mark) = *held {
                owner[page as usize][mark as usize] = Some(at as u32);
            }
        }
    }
    let (mut number, mut parts) = (Vec::new(), Vec::new());
    for (page, sheet) in paper.pages.iter().enumerate() {
        let (mut numbers, mut by, mut last) = (vec![None; sheet.marks.len()], Vec::new(), None);
        for (at, mark) in sheet.marks.iter().enumerate() {
            let part = match (mark, owner[page][at]) {
                (Mark::Link(..) | Mark::Field(_), _) => continue,
                (Mark::Clip(_) | Mark::Unclip, _) | (_, None) => None,
                (_, part) => part,
            };
            if part.is_some() && part != last {
                by.extend(part);
            }
            (numbers[at], last) = (part.map(|_| by.len() as u32 - 1), part);
        }
        number.push(numbers);
        parts.push(by);
    }
    Marked { owner, number, parts, skipped }
}

/// Writes the structure tree: an element per part (decoration left out), each holding its
/// parts and marked content in reading order, and the parent tree leading back to them from
/// each page's numbers (key: the page's index) and each annotation (its key, by page and
/// mark, from the page count on). Gives back the tree's root.
pub(crate) fn write_structure(file: &mut File, paper: &Paper, marked: &Marked, kids: &[u32], annotations: &Annotations) -> Option<u32> {
    if paper.parts.is_empty() {
        return None;
    }
    let (root, tree) = (file.number(), file.number());
    let numbers: Vec<u32> = marked.skipped.iter().map(|skipped| if *skipped { 0 } else { file.number() }).collect();
    for (at, part) in paper.parts.iter().enumerate().filter(|(at, _)| !marked.skipped[*at]) {
        let held = held_list(part, marked, &numbers, kids, annotations);
        let up = if at == 0 { root } else { numbers[part.up as usize] };
        let mut said = format!("<< /Type /StructElem /S /{} /P {up} 0 R /K [{held}]", part.tag.name());
        if !part.alt.is_empty() {
            said.push_str(&format!(" /Alt {}", text(&part.alt)));
        }
        let above = if at == 0 { "" } else { paper.parts[part.up as usize].language.as_str() };
        if !part.language.is_empty() && part.language != above && at > 0 {
            said.push_str(&format!(" /Lang {}", text(&part.language)));
        }
        if part.tag == Tag::Head {
            said.push_str(" /A << /O /Table /Scope /Column >>");
        }
        said.push_str(" >>");
        file.object(numbers[at], &said);
    }
    let mut nums = String::new();
    for (page, by) in marked.parts.iter().enumerate() {
        let refs: String = by.iter().map(|part| format!("{} 0 R ", numbers[*part as usize])).collect();
        nums.push_str(&format!("{page} [{refs}] "));
    }
    // (Each annotation's element, by its key: keys go up in the order annotations were written.)
    let mut keyed = vec![None; annotations.count as usize];
    for (page, marks) in annotations.by_mark.iter().enumerate() {
        for (mark, (_, key)) in marks.iter().enumerate().filter_map(|(mark, written)| Some((mark, (*written)?))) {
            keyed[(key - paper.pages.len() as u32) as usize] = marked.owner[page][mark];
        }
    }
    for (at, part) in keyed.iter().enumerate().filter_map(|(at, part)| Some((at, (*part)?))) {
        nums.push_str(&format!("{} {} 0 R ", paper.pages.len() + at, numbers[part as usize]));
    }
    let next = annotations.next_key();
    file.object(tree, &format!("<< /Nums [{nums}] >>"));
    file.object(root, &format!("<< /Type /StructTreeRoot /K [{} 0 R] /ParentTree {tree} 0 R /ParentTreeNextKey {next} >>", numbers[0]));
    Some(root)
}

/// What a part's element holds, in order: its parts' elements, its marked content (each
/// number once), its annotations.
fn held_list(part: &Part, marked: &Marked, numbers: &[u32], kids: &[u32], annotations: &Annotations) -> String {
    let (mut held, mut last) = (String::new(), None);
    for one in &part.held {
        let (page, mark) = match *one {
            Held::Part(inner) if !marked.skipped[inner as usize] => {
                held.push_str(&format!("{} 0 R ", numbers[inner as usize]));
                continue;
            }
            Held::Part(_) => continue,
            Held::Mark(page, mark) => (page, mark),
        };
        if let Some((annotation, _)) = annotations.get(page, mark) {
            held.push_str(&format!("<< /Type /OBJR /Pg {} 0 R /Obj {annotation} 0 R >> ", kids[page as usize]));
            continue;
        }
        let Some(number) = marked.number[page as usize][mark as usize] else { continue };
        if last != Some((page, number)) {
            held.push_str(&format!("<< /Type /MCR /Pg {} 0 R /MCID {number} >> ", kids[page as usize]));
        }
        last = Some((page, number));
    }
    held
}

/// The words of a part's own marks, joined: what a link or a field says.
pub(crate) fn words_of(paper: &Paper, part: u32) -> String {
    let mut said = Vec::new();
    for held in &paper.parts[part as usize].held {
        if let Held::Mark(page, at) = *held
            && let Mark::Words(words) = &paper.pages[page as usize].marks[at as usize]
        {
            said.push(words.text.trim().to_string());
        }
    }
    said.join(" ")
}

/// Writes the outline: the bookmarks, each under the last before it of a higher level, all
/// open; each leads to its heading. Nothing without bookmarks.
pub(crate) fn write_outline(file: &mut File, paper: &Paper, kids: &[u32]) -> Option<u32> {
    let marks = &paper.bookmarks;
    if marks.is_empty() {
        return None;
    }
    let root = file.number();
    let numbers: Vec<u32> = marks.iter().map(|_| file.number()).collect();
    // Each entry's parent (by index; none for the root), from a stack of open entries.
    let (mut up, mut stack): (Vec<Option<usize>>, Vec<usize>) = (Vec::new(), Vec::new());
    for (at, mark) in marks.iter().enumerate() {
        while stack.last().is_some_and(|open| marks[*open].level >= mark.level) {
            stack.pop();
        }
        up.push(stack.last().copied());
        stack.push(at);
    }
    // How many entries lie below each: counted from the last, each adding itself and its own
    // to its parent.
    let mut below = vec![0u32; marks.len()];
    for at in (0..marks.len()).rev() {
        if let Some(parent) = up[at] {
            below[parent] += below[at] + 1;
        }
    }
    let children = |of: Option<usize>| -> Vec<usize> { (0..marks.len()).filter(|at| up[*at] == of).collect() };
    let link = |at: usize| numbers[at];
    for (at, mark) in marks.iter().enumerate() {
        let own = children(Some(at));
        let siblings = children(up[at]);
        let place = siblings.iter().position(|one| *one == at).unwrap_or(0);
        let parent = up[at].map_or(root, link);
        let mut said = format!("<< /Title {} /Parent {parent} 0 R", text(&mark.title));
        if place > 0 {
            said.push_str(&format!(" /Prev {} 0 R", link(siblings[place - 1])));
        }
        if let Some(next) = siblings.get(place + 1) {
            said.push_str(&format!(" /Next {} 0 R", link(*next)));
        }
        if let (Some(first), Some(last)) = (own.first(), own.last()) {
            said.push_str(&format!(" /First {} 0 R /Last {} 0 R /Count {}", link(*first), link(*last), below[at]));
        }
        let page = mark.page.min(kids.len() - 1);
        let top = (paper.pages[page].size.h - mark.y) * 0.75;
        said.push_str(&format!(" /Dest [{} 0 R /XYZ null {} null] >>", kids[page], num(top)));
        file.object(numbers[at], &said);
    }
    let tops = children(None);
    let count = tops.len() as u32 + tops.iter().map(|at| below[*at]).sum::<u32>();
    file.object(root, &format!("<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {count} >>", link(tops[0]), link(tops[tops.len() - 1])));
    Some(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::geom::{Size, rect};
    use crate::markdown::Markdown;
    use crate::pdf::tests::{latin, objects, streamed};
    use crate::text::{Face, Font};

    /// A paper whose regular text uses an installed font.
    fn paper() -> Paper {
        let mut paper = Paper::new();
        if let Some(font) = latin() {
            paper.font(Face::Regular, font);
        }
        paper
    }

    /// A document of every kind of part, with a picture the app supplies.
    fn document() -> Paper {
        let mut paper = paper();
        paper.set_picture("chart", 4, &[0xff00_00ff; 16]);
        let text = "# Report\n\nWords with [a link](https://example.com).\n\n## Figures\n\n![Sales rose every month](chart)\n\n1. first\n2. second\n\n- a bullet\n\n> quoted\n\n```\ncode\n```\n\n### Table\n\n| Name | Size |\n| --- | --- |\n| a | 1 |\n\n---\n";
        paper.markdown(&Markdown::parse(text), Paper::A4, 72.0);
        paper
    }

    /// The structure elements of a file: each one's type and the rest of its dictionary.
    fn elements(file: &[u8]) -> Vec<(String, String)> {
        let found = objects(file).into_iter().map(|(_, object)| String::from_utf8_lossy(&object).into_owned()).filter(|object| object.contains("/Type /StructElem"));
        found.map(|object| (object.split("/S /").nth(1).unwrap_or("").split(' ').next().unwrap_or("").to_string(), object)).collect()
    }

    #[test]
    fn a_document_is_tagged_as_a_window_tells_it() {
        let paper = document();
        let file = paper.to_pdf();
        let told = elements(&file);
        let kinds: Vec<&str> = told.iter().map(|(kind, _)| kind.as_str()).collect();
        for kind in ["Document", "H1", "H2", "H3", "P", "Link", "Figure", "L", "LI", "Lbl", "LBody", "BlockQuote", "Code", "Table", "TR", "TH", "TD"] {
            assert!(kinds.contains(&kind), "{kind} in {kinds:?}");
        }
        let (_, figure) = told.iter().find(|(kind, _)| kind == "Figure").unwrap();
        assert!(figure.contains(&format!("/Alt {}", crate::pdf::text("Sales rose every month"))), "{figure}");
        assert!(told.iter().filter(|(kind, _)| kind == "TH").all(|(_, head)| head.contains("/Scope /Column")));
        assert_eq!(told.iter().filter(|(kind, _)| kind == "Lbl").count(), 2, "a numbered item's number is its label");
        let (_, link) = told.iter().find(|(kind, _)| kind == "Link").unwrap();
        assert!(link.contains("/Type /OBJR") && link.contains("/Type /MCR"), "a link holds its annotation and its words: {link}");
        let text = String::from_utf8_lossy(&file);
        assert!(text.contains("/MarkInfo << /Marked true >>") && text.contains("/StructTreeRoot") && text.contains("/ParentTree") && text.contains("/Tabs /S"));
        assert!(text.contains("/DisplayDocTitle true") && text.contains("<pdfuaid:part>1</pdfuaid:part>"));
        // Every word is in a part; no shape is, but the picture.
        let marked = marked(&paper);
        for (page, sheet) in paper.pages.iter().enumerate() {
            for (at, mark) in sheet.marks.iter().enumerate() {
                let owner = marked.owner[page][at].map(|part| paper.parts[part as usize].tag);
                match mark {
                    Mark::Words(words) => assert!(owner.is_some(), "{:?} is told", words.text),
                    Mark::Shape(_) => assert_eq!(owner, None, "a shape is decoration"),
                    Mark::Picture(..) => assert_eq!(owner, Some(Tag::Figure)),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn marked_content_opens_and_closes_in_turn() {
        let file = document().to_pdf();
        for (_, object) in objects(&file) {
            let Some(bytes) = streamed(&object).filter(|_| !String::from_utf8_lossy(&object).contains("/Type")) else { continue };
            let said = String::from_utf8_lossy(&bytes);
            let mut depth = 0;
            for word in said.split_whitespace() {
                depth += match word {
                    "BDC" | "BMC" => 1,
                    "EMC" => -1,
                    _ => 0,
                };
                assert!((0..=1).contains(&depth), "marked content nests no deeper than one, and closes what opened");
            }
            assert_eq!(depth, 0);
        }
    }

    #[test]
    fn headings_make_the_outline() {
        let paper = document();
        let marks: Vec<(u32, &str)> = paper.bookmarks.iter().map(|mark| (mark.level, mark.title.as_str())).collect();
        assert_eq!(marks, [(1, "Report"), (2, "Figures"), (3, "Table")]);
        let file = String::from_utf8_lossy(&paper.to_pdf()).into_owned();
        assert!(file.contains("/Outlines ") && file.contains("/PageMode /UseOutlines"));
        assert!(file.contains("<< /Type /Outlines") && file.contains(" /Count 3 >>"), "three entries, all open");
        // "Report" holds "Figures", which holds "Table".
        let entries: Vec<&str> = file.split("obj\n").filter(|object| object.starts_with("<< /Title ") && object.contains("/Parent ")).collect();
        assert_eq!(entries.len(), 3);
        assert!(entries[0].contains("/Count 2") && entries[1].contains("/Count 1") && entries[2].contains("/Dest ["));
    }

    #[test]
    fn a_tables_headings_come_again_on_each_page_it_goes_on_to() {
        let mut text = String::from("| Name | Size |\n| --- | --- |\n");
        for n in 0..60 {
            text.push_str(&format!("| row {n} | {n} |\n"));
        }
        let mut paper = paper();
        let pages = paper.markdown(&Markdown::parse(&text), Size { w: 400.0, h: 400.0 }, 40.0);
        assert!(pages >= 3, "{pages} pages");
        for (page, sheet) in paper.pages.iter().enumerate() {
            let words: Vec<&str> = sheet.marks.iter().filter_map(|mark| if let Mark::Words(words) = mark { Some(words.text.as_str()) } else { None }).collect();
            assert_eq!(words.first().copied(), Some("Name"), "page {page} starts with the headings");
        }
        // Told once: again, they are decoration.
        let heads = elements(&paper.to_pdf()).iter().filter(|(kind, _)| kind == "TH").count();
        assert_eq!(heads, 2);
    }

    #[test]
    fn an_apps_own_parts_hold_what_is_written_in_them() {
        let mut paper = paper();
        paper.page(Paper::A4);
        paper.set_language("de");
        paper.begin(Role::Picture);
        paper.describe("Ein Kreis");
        paper.rounded(rect(10.0, 10.0, 40.0, 40.0), 20.0, Color::BLACK);
        paper.end();
        paper.rect(rect(60.0, 10.0, 40.0, 40.0), Color::BLACK);
        paper.begin_decoration();
        paper.text("1 of 1", 10.0, 100.0, Font::new(10.0), Color::BLACK);
        paper.end();
        paper.text("Alone", 10.0, 130.0, Font::new(10.0), Color::BLACK);
        let kinds: Vec<Tag> = paper.parts.iter().map(|part| part.tag).collect();
        assert_eq!(kinds, [Tag::Document, Tag::Figure, Tag::Decoration, Tag::Paragraph]);
        let file = paper.to_pdf();
        let told = elements(&file);
        assert_eq!(told.iter().map(|(kind, _)| kind.as_str()).collect::<Vec<_>>(), ["Document", "Figure", "P"], "decoration is not told");
        assert!(String::from_utf8_lossy(&file).contains(&format!("/Lang {}", crate::pdf::text("de"))));
    }

    #[test]
    fn a_window_on_a_page_is_described_by_what_it_tells_readers() {
        let mut paper = Paper::new();
        paper.interface(rect(0.0, 0.0, 200.0, 80.0), &mut |ui| {
            ui.label("Shown in a window");
            ui.button("Press");
        });
        let (_, figure) = elements(&paper.to_pdf()).into_iter().find(|(kind, _)| kind == "Figure").expect("a figure");
        assert!(figure.contains(&crate::pdf::text("Shown in a window. Press, button.")), "{figure}");
    }

    #[test]
    fn a_reader_finds_the_structure() {
        let file = document().to_pdf();
        let path = std::env::temp_dir().join(format!("taut-struct-{}.pdf", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        let ran = std::process::Command::new("pdfinfo").arg("-struct-text").arg(&path).output();
        let _ = std::fs::remove_file(&path);
        let Some(ran) = ran.ok().filter(|ran| ran.status.success()) else { return eprintln!("no Poppler pdfinfo here: not read back") };
        let said = String::from_utf8_lossy(&ran.stdout);
        for kind in ["Document", "H1 (block)", "Figure", "TH", "/Scope /Column", "Link", "\"Report\"", "\"Words with \""] {
            assert!(said.contains(kind), "{kind} in\n{said}");
        }
    }
}
