//! Ruby (furigana): a reading set small beside its base text, as CSS Ruby Layout and JLREQ
//! place it and Chrome does: over horizontal text, on the right of vertical text.
//!
//! The reading is half its base's size, its em box against the base's. The shorter of the two
//! is spread over the other's length (`ruby-align: space-around`): between characters of CJK
//! text (and after spaces) by as much as twice its ends get, a reading by no more than twice
//! its size at the ends; text with no such places is centred. A reading longer than its base
//! reaches over the text either side by up to half its size and half that text's length, where
//! that text is no larger than the base.
//!
//! Readers are told the base with its reading after it, in brackets.

/// How extra length is spread over a stretch of text: before its first character, and at each
/// place between characters where text may be spread.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Spread {
    pub lead: f32,
    pub gap: f32,
}

/// How `space` more length is spread over text with `places` where it may be (see
/// `can_spread`): one share at each place and half a share at each end. For a reading, of
/// `size`, the ends get at most its size. Without places, half at each end.
pub(crate) fn spread(space: f32, places: usize, reading: Option<f32>) -> Spread {
    if space <= 0.0 {
        return Spread::default();
    }
    if places == 0 {
        return Spread { lead: space / 2.0, gap: 0.0 };
    }
    let ends = reading.map_or(space / (places + 1) as f32, |size| (space / (places + 1) as f32).min(2.0 * size));
    Spread { lead: ends / 2.0, gap: (space - ends) / places as f32 }
}

/// How far a reading `space` longer than its base (with `places` to spread at) may reach over
/// the text either side: its base's lead, at most half the reading's `size`.
pub(crate) fn reach(space: f32, places: usize, size: f32) -> f32 {
    if space <= 0.0 { 0.0 } else { spread(space, places, None).lead.min(size / 2.0) }
}

/// Whether text may be spread between two characters: where either is CJK (ideographs, kana,
/// Hangul, their punctuation), or after a space.
pub(crate) fn can_spread(before: char, after: char) -> bool {
    use crate::line_breaks::{BreakClass, break_class};
    let wide = |letter: char| letter as u32 >= 0x2e80 && !matches!(break_class(letter), BreakClass::Letter | BreakClass::Number | BreakClass::Space | BreakClass::Quote);
    before == ' ' || wide(before) || wide(after)
}

/// How many places a text's characters (first of each cluster) may be spread at.
pub(crate) fn places(letters: &[char]) -> usize {
    letters.windows(2).filter(|pair| can_spread(pair[0], pair[1])).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shorter_text_is_spread_with_half_shares_at_its_ends() {
        // Three kana under two ideographs 40 long: 10 to spare, two places: a third at the
        // ends (a sixth each), the rest at the places.
        let spread = spread(10.0, 2, Some(10.0));
        assert!((spread.lead - 10.0 / 6.0).abs() < 1e-5 && (spread.gap - 10.0 / 3.0).abs() < 1e-5, "{spread:?}");
        // Nowhere to spread: centred.
        assert_eq!(super::spread(10.0, 0, None), Spread { lead: 5.0, gap: 0.0 });
        // A reading's ends get no more than twice its size.
        assert_eq!(super::spread(100.0, 1, Some(5.0)).lead, 5.0);
        // Nothing to spare: nothing spread.
        assert_eq!(super::spread(-3.0, 4, None), Spread::default());
    }

    #[test]
    fn a_long_reading_reaches_over_its_neighbours_by_at_most_half_its_size() {
        // A one-character base 20 long under a reading 30 long: 5 each side, under 7.5.
        assert_eq!(reach(10.0, 0, 15.0), 5.0);
        assert_eq!(reach(40.0, 0, 10.0), 5.0);
        assert_eq!(reach(-1.0, 0, 10.0), 0.0);
    }

    #[test]
    fn cjk_text_spreads_between_characters_and_latin_only_after_spaces() {
        assert_eq!(places(&['か', 'ん', 'じ']), 2);
        assert_eq!(places(&['k', 'a', 'n']), 0);
        assert_eq!(places(&['a', ' ', 'b']), 1);
        assert_eq!(places(&['漢', 'A']), 1);
    }
}
