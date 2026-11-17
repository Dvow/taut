//! Selecting many rows of a list: a click selects one row, Control+click adds or removes one,
//! Shift+click selects the range from the row clicked last.
//!
//! Selected rows are kept as ranges, so selecting a million rows costs what selecting one does.
use crate::event::Mods;
use std::ops::Range;

/// Which rows of a list are selected. The app keeps one per list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    /// The selected ranges, in order, none touching the next.
    spans: Vec<Range<usize>>,
    /// The row clicked last: where a Shift range starts.
    last: usize,
}

impl Selection {
    /// Whether this row is selected.
    pub fn contains(&self, row: usize) -> bool {
        self.spans.get(self.spans.partition_point(|span| span.end <= row)).is_some_and(|span| span.start <= row)
    }
    /// How many rows are selected.
    pub fn len(&self) -> usize {
        self.spans.iter().map(|span| span.len()).sum()
    }
    /// Whether nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
    /// The rows that are selected, lowest first.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.spans.iter().flat_map(|span| span.clone())
    }
    /// Selects nothing.
    pub fn clear(&mut self) {
        self.spans.clear();
    }

    /// Selects these rows as well.
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
    /// Deselects this row.
    pub fn remove(&mut self, row: usize) {
        let at = self.spans.partition_point(|span| span.end <= row);
        let Some(span) = self.spans.get(at).filter(|span| span.start <= row).cloned() else { return };
        // What is left of its range: the rows before it and after it.
        let left = [span.start..row, row + 1..span.end];
        self.spans.splice(at..at + 1, left.into_iter().filter(|part| !part.is_empty()));
    }

    /// Takes a click on `row` with these modifiers ([`ui.mods()`](crate::Ui::mods)), as any
    /// list does: alone it selects that row, with Control it adds or removes it, with Shift it
    /// selects the range from the row clicked last.
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
        match mods.ctrl && !mods.shift && self.contains(row) {
            true => self.remove(row),
            false => self.add(rows),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_select_one_row_add_one_and_select_a_stretch() {
        let (mut selection, ctrl, shift) = (Selection::default(), Mods::CTRL, Mods::SHIFT);
        selection.click(4, Mods::NONE);
        assert_eq!(selection.iter().collect::<Vec<_>>(), [4]);
        selection.click(8, ctrl);
        selection.click(6, ctrl);
        assert_eq!(selection.iter().collect::<Vec<_>>(), [4, 6, 8]);
        selection.click(6, ctrl);
        assert!(!selection.contains(6) && selection.contains(8) && selection.len() == 2, "Control and a click on a selected row takes it away");
        // Shift selects from the row clicked last (6) to this one, replacing what was selected.
        selection.click(2, shift);
        assert_eq!(selection.iter().collect::<Vec<_>>(), [2, 3, 4, 5, 6]);
        selection.click(9, Mods { shift: true, ctrl: true, alt: false });
        assert_eq!(selection.len(), 8, "with Control as well, the stretch is added: {selection:?}");
        selection.click(1, Mods::NONE);
        assert_eq!(selection.iter().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn stretches_that_touch_become_one_and_a_row_taken_out_splits_one() {
        let mut selection = Selection::default();
        selection.add(10..20);
        selection.add(30..40);
        selection.add(20..30);
        assert_eq!(selection.spans.first(), Some(&(10..40)));
        selection.add(0..1_000_000);
        assert_eq!((selection.spans.len(), selection.len()), (1, 1_000_000), "a million rows are one stretch");
        selection.remove(500);
        assert_eq!(selection.spans, [0..500, 501..1_000_000]);
        selection.remove(0);
        selection.remove(999_999);
        assert_eq!(selection.spans, [1..500, 501..999_999]);
        assert!(selection.contains(1) && !selection.contains(500) && !selection.contains(0) && !selection.contains(999_999));
    }
}
