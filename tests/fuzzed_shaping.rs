//! Fuzzer finds in OpenType shaping (`GSUB`, `GPOS`), each minimized. Regression tests: each
//! failed before its fix.
use std::time::{Duration, Instant};
use taut::text::{Font, TextSystem};
use taut::{FontFile, Ordered, OwnFonts};

/// A table of a font: its name and its bytes.
type Table = ([u8; 4], Vec<u8>);

fn be16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn be32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

/// A font file of these tables, each where its record says.
fn font_of(mut tables: Vec<Table>) -> Vec<u8> {
    tables.sort_by_key(|table| table.0);
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    for value in [tables.len() as u16, 0, 0, 0] {
        be16(&mut out, value);
    }
    let mut at = 12 + 16 * tables.len();
    for (name, body) in &tables {
        out.extend_from_slice(name);
        be32(&mut out, 0);
        be32(&mut out, at as u32);
        be32(&mut out, body.len() as u32);
        at += body.len().next_multiple_of(4);
    }
    for (_, body) in &tables {
        out.extend_from_slice(body);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    out
}

/// A font of 28 empty glyphs, with the Hebrew letters (U+05D0 to U+05EA) as glyphs 1 to 27,
/// and `gsub` as its table of glyphs put in the place of others.
fn hebrew_font(gsub: Vec<u8>) -> Vec<u8> {
    let glyphs = 28u16;
    let mut head = vec![0u8; 54];
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    let mut hhea = vec![0u8; 36];
    hhea[4..6].copy_from_slice(&800u16.to_be_bytes());
    hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes());
    hhea[34..36].copy_from_slice(&glyphs.to_be_bytes());
    let mut maxp = vec![0, 0, 0x50, 0];
    be16(&mut maxp, glyphs);
    let mut hmtx = Vec::new();
    for _ in 0..glyphs {
        be16(&mut hmtx, 500);
        be16(&mut hmtx, 0);
    }
    // A map of characters to glyphs of the twelfth format (groups), for Unicode.
    let mut cmap = Vec::new();
    for value in [0u16, 1, 3, 10] {
        be16(&mut cmap, value);
    }
    be32(&mut cmap, 12);
    for value in [12u16, 0] {
        be16(&mut cmap, value);
    }
    for value in [28u32, 0, 1, 0x05D0, 0x05EA, 1] {
        be32(&mut cmap, value);
    }
    let loca = vec![0u8; 2 * (glyphs as usize + 1)];
    font_of(vec![(*b"head", head), (*b"hhea", hhea), (*b"maxp", maxp), (*b"hmtx", hmtx), (*b"cmap", cmap), (*b"loca", loca), (*b"glyf", Vec::new()), (*b"GSUB", gsub)])
}

/// A `GSUB` whose script `hebr` has one feature, `ccmp`, of `lookups` lookups, each of which
/// says it has 65,535 lists of rules (and has none: the places of its lists lie past the
/// end of the table, as they do where a font is damaged).
fn gsub_of_lookups_with_lists_that_are_not_there(lookups: u16) -> Vec<u8> {
    let mut out = Vec::new();
    // The head: version 1.0, then where the scripts, the features and the lookups are.
    for value in [1u16, 0, 10, 30, 0] {
        be16(&mut out, value);
    }
    // Scripts (at 10): one, `hebr`, whose own way (at 8 from it) names feature 0.
    be16(&mut out, 1);
    out.extend_from_slice(b"hebr");
    be16(&mut out, 8);
    for value in [4u16, 0, 0, 0xFFFF, 1, 0] {
        be16(&mut out, value);
    }
    // Features (at 30): one, `ccmp`, of every lookup.
    assert_eq!(out.len(), 30);
    be16(&mut out, 1);
    out.extend_from_slice(b"ccmp");
    be16(&mut out, 8);
    be16(&mut out, 0);
    be16(&mut out, lookups);
    for n in 0..lookups {
        be16(&mut out, n);
    }
    // Lookups: where each is, then each of them, of the first kind, with no flags.
    let listed = out.len();
    out[8..10].copy_from_slice(&(listed as u16).to_be_bytes());
    be16(&mut out, lookups);
    let first = 2 + 2 * lookups as usize;
    for n in 0..lookups as usize {
        be16(&mut out, (first + 6 * n) as u16);
    }
    for _ in 0..lookups {
        for value in [1u16, 0, 0xFFFF] {
            be16(&mut out, value);
        }
    }
    out
}

/// A damaged font whose lookups each say they have 65,535 lists: every glyph of a line is
/// tried against every list of every lookup, though none of them is there. With twenty such
/// lookups, a line of a hundred Hebrew letters is 131 million tries: seconds for one
/// line, every time it is measured or drawn (found with a font whose `GSUB` and `GPOS` had
/// places changed: 2.6 s for one word of 14 letters with its points). HarfBuzz bounds the
/// work a buffer may take for the same reason; here a run of text may try so many lists in
/// all, and then is left as it is.
#[test]
fn a_font_whose_lookups_say_they_have_thousands_of_lists_does_not_take_seconds_a_line() {
    let file = hebrew_font(gsub_of_lookups_with_lists_that_are_not_there(20));
    let mut text = Ordered::around(Box::new(OwnFonts::new(FontFile::read(file).expect("the font is read"))));
    let line = "שלום עולם ".repeat(10);
    let clock = Instant::now();
    let wide = text.measure(&line, Font::new(16.0), 1.0, None);
    let took = clock.elapsed();
    assert!(wide > 0.0);
    assert!(took < Duration::from_millis(1500), "a line of {} letters took {took:?} to measure", line.chars().count());
}
