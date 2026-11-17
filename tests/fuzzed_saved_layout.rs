//! Fuzzer finds in app state kept as text (`saved_layout.rs`): a value written with
//! `to_string()` must read back unchanged with `parse()`. Regression tests: each failed before
//! its fix.
use taut::Table;

/// The whole numbers of a table's line (the columns it is sorted by, their order, the hidden
/// ones, and how many stay in view) are read as numbers with a point, of 32 bits, which hold
/// whole numbers as they are only up to 16,777,216: one more is read back as one less, so a
/// table with more columns or rows than that does not come back as it was left. (The
/// fuzzer's own check of these lines never ran until tonight: its seeds overflowed.)
#[test]
fn whole_numbers_of_a_table_come_back_as_they_were_written() {
    let table = Table { sort: vec![(16_777_217, true)], order: vec![20_000_001, 0], hidden: vec![16_777_219], frozen: 3_974_091_461, ..Table::default() };
    let written = table.to_string();
    assert_eq!(written.parse::<Table>().ok(), Some(table), "{written:?} was read back otherwise");
}
