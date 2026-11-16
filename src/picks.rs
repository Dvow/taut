//! Picking many rows of a list: a click picks one row, Control+click adds or removes one,
//! Shift+click picks the range from the row clicked last.
//!
//! Picked rows are kept as ranges, so picking a million rows costs what picking one does.
use crate::event::Mods;
use std::ops::Range;

/// Which rows of a list are picked. The app keeps one per list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Picks {
    /// The picked ranges, in order, none touching the next.
    spans: Vec<Range<usize>>,
    /// The row clicked last: where a Shift range starts.
    last: usize,
}

impl Picks {
    /// Whether this row is picked.
    pub fn has(&self, row: usize) -> bool {
        self.spans.get(self.spans.partition_point(|span| span.end <= row)).is_some_and(|span| span.start <= row)
    }
    /// How many rows are picked.
    pub fn len(&self) -> usize {
        self.spans.iter().map(|span| span.len()).sum()
    }
    /// Whether nothing is picked.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
    /// The rows that are picked, lowest first.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.spans.iter().flat_map(|span| span.clone())
    }
    /// Picks nothing.
    pub fn clear(&mut self) {
        self.spans.clear();
    }

    /// Picks these rows as well.
    pub fn add(&mut self, rows: Range<usize>) {
        if rows.is_empty() {
            return;
        }
        // Ranges this one touches or overlaps merge with it.
        let first = self.spans.partition_point(|span| span.end < rows.start);
        let after = self.spans.partition_point(|span| span.start <= rows.end);
        let start = self.spans[first..after].first().map_or(rows.start, |span| span.start.min(rows.start));
        let end = self.spans[first..after].last().map_or(rows.end, |span| span.end.max(rows.end));
        self.spans.splice(first..after, std::iter::once(start..end));
    }
    /// Unpicks this row.
    pub fn remove(&mut self, row: usize) {
        let at = self.spans.partition_point(|span| span.end <= row);
        let Some(span) = self.spans.get(at).filter(|span| span.start <= row).cloned() else { return };
        // What is left of its range: the rows before it and after it.
        let left = [span.start..row, row + 1..span.end];
        self.spans.splice(at..at + 1, left.into_iter().filter(|part| !part.is_empty()));
    }

    /// Takes a click on `row` with these modifiers ([`ui.mods()`](crate::Ui::mods)), as any
    /// list does: alone it picks that row, with Control it adds or removes it, with Shift it
    /// picks the range from the row clicked last.
    pub fn click(&mut self, row: usize, mods: Mods) {
        // With Shift, the range from the row clicked last; else this row, which becomes the one
        // clicked last.
        let rows = if mods.shift { self.last.min(row)..self.last.max(row) + 1 } else { row..row + 1 };
        if !mods.shift {
            self.last = row;
        }
        if !mods.ctrl {
            self.spans.clear();
        }
        match mods.ctrl && !mods.shift && self.has(row) {
            true => self.remove(row),
            false => self.add(rows),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_pick_one_row_add_one_and_pick_a_stretch() {
        let (mut picks, ctrl, shift) = (Picks::default(), Mods::CTRL, Mods::SHIFT);
        picks.click(4, Mods::NONE);
        assert_eq!(picks.iter().collect::<Vec<_>>(), [4]);
        picks.click(8, ctrl);
        picks.click(6, ctrl);
        assert_eq!(picks.iter().collect::<Vec<_>>(), [4, 6, 8]);
        picks.click(6, ctrl);
        assert!(!picks.has(6) && picks.has(8) && picks.len() == 2, "Control and a click on a picked row takes it away");
        // Shift picks from the row clicked last (6) to this one, replacing what was picked.
        picks.click(2, shift);
        assert_eq!(picks.iter().collect::<Vec<_>>(), [2, 3, 4, 5, 6]);
        picks.click(9, Mods { shift: true, ctrl: true, alt: false });
        assert_eq!(picks.len(), 8, "with Control as well, the stretch is added: {picks:?}");
        picks.click(1, Mods::NONE);
        assert_eq!(picks.iter().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn stretches_that_touch_become_one_and_a_row_taken_out_splits_one() {
        let mut picks = Picks::default();
        picks.add(10..20);
        picks.add(30..40);
        picks.add(20..30);
        assert_eq!(picks.spans.first(), Some(&(10..40)));
        picks.add(0..1_000_000);
        assert_eq!((picks.spans.len(), picks.len()), (1, 1_000_000), "a million rows are one stretch");
        picks.remove(500);
        assert_eq!(picks.spans, [0..500, 501..1_000_000]);
        picks.remove(0);
        picks.remove(999_999);
        assert_eq!(picks.spans, [1..500, 501..999_999]);
        assert!(picks.has(1) && !picks.has(500) && !picks.has(0) && !picks.has(999_999));
    }
}
