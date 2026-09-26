//! A text box that wraps and scrolls: how a piece of text is laid out in rows of a given width, where a cursor
//! (a character index into the text) sits in those rows, and which rows are in view. Pure: the hand-off box
//! (`app/handoff_glue.rs`) keeps the text and the cursor, `ui.rs` draws what this works out.
//!
//! Widths are in terminal columns, not characters: a wide character (most CJK, many emoji) takes two, a
//! combining mark none. A line wraps at a space where it can; a word longer than the width is broken where it
//! has to be. A space that falls just past the width hangs at the end of its row rather than starting the next
//! one, which is why the box keeps one column spare to the right of the text (`content_width`).

use unicode_width::UnicodeWidthChar;

/// How many columns a character takes on screen. Control characters never get into the box (`clean_paste`),
/// but if one did it would take none rather than break the arithmetic.
pub fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// How many columns a run of characters takes.
pub fn width_of(chars: &[char]) -> usize {
    chars.iter().map(|&c| char_width(c)).sum()
}

/// The columns text may take in a box `box_width` wide: one is kept spare so the cursor at the end of a full row,
/// or a space hanging there, is still inside the box.
pub fn content_width(box_width: usize) -> usize {
    box_width.saturating_sub(1).max(1)
}

/// One row on screen: the characters `start..end` of the text. `last` is set on the last row of a line (the text
/// up to a line break); the line break itself is never in a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub start: usize,
    pub end: usize,
    pub last: bool,
}

/// Lay `chars` out in rows at most `width` columns wide (see the module note for the one exception, a hanging
/// space). There is always at least one row, and every line - an empty one too - has at least one.
pub fn layout(chars: &[char], width: usize) -> Vec<Row> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    loop {
        let end = chars[start..].iter().position(|&c| c == '\n').map_or(chars.len(), |p| start + p);
        wrap_line(chars, start, end, width, &mut rows);
        if end >= chars.len() {
            break;
        }
        start = end + 1;
    }
    rows
}

fn wrap_line(chars: &[char], start: usize, end: usize, width: usize, rows: &mut Vec<Row>) {
    let mut row_start = start;
    let mut col = 0;
    // Just after the last space in the current row: where it can be broken without cutting a word.
    let mut brk: Option<usize> = None;
    let mut i = start;
    while i < end {
        let c = chars[i];
        let w = char_width(c);
        // A row always takes at least one character, so even a character wider than the box moves on.
        if col + w > width && i > row_start {
            if c == ' ' {
                // A space just past the edge ends the row where it is, rather than starting the next one.
                rows.push(Row { start: row_start, end: i + 1, last: false });
                row_start = i + 1;
                col = 0;
                brk = None;
                i += 1;
                continue;
            }
            let cut = match brk {
                Some(b) if b > row_start => b,
                _ => i, // one word longer than the row: broken where it meets the edge
            };
            rows.push(Row { start: row_start, end: cut, last: false });
            row_start = cut;
            col = width_of(&chars[cut..i]);
            brk = None;
            continue; // this character is looked at again, on its new row
        }
        col += w;
        if c == ' ' {
            brk = Some(i + 1);
        }
        i += 1;
    }
    // The end of a line that finished with a hanging space gets a row of its own (empty), so the cursor after it
    // has somewhere to be.
    rows.push(Row { start: row_start, end, last: true });
}

/// Where the cursor (a character index) is on screen: its row and its column.
pub fn locate(rows: &[Row], chars: &[char], cursor: usize) -> (usize, usize) {
    for (n, r) in rows.iter().enumerate() {
        if cursor >= r.start && (cursor < r.end || (r.last && cursor == r.end)) {
            return (n, width_of(&chars[r.start..cursor]));
        }
    }
    // Past the end (can't happen for a cursor kept in range): the end of the last row.
    let n = rows.len().saturating_sub(1);
    rows.get(n).map_or((0, 0), |r| (n, width_of(&chars[r.start..r.end.min(chars.len())])))
}

/// The character index at (`row`, `col`): the furthest position on that row that is not right of `col`. On a row
/// that wraps on to the next, the position after its last character is the next row's, so it is never chosen.
pub fn index_at(rows: &[Row], chars: &[char], row: usize, col: usize) -> usize {
    let Some(r) = rows.get(row.min(rows.len().saturating_sub(1))) else { return 0 };
    let last = if r.last { r.end } else { r.end.saturating_sub(1).max(r.start) };
    let mut at = r.start;
    let mut x = 0;
    while at < last {
        let w = char_width(chars[at]);
        if x + w > col {
            break;
        }
        x += w;
        at += 1;
    }
    at
}

/// The first row in view: as close to `top` as it can stay while the cursor's row is in view and no empty space
/// is shown below the text that could be filled.
pub fn clamp_top(top: usize, cursor_row: usize, visible: usize, total: usize) -> usize {
    let mut top = top.min(total.saturating_sub(visible));
    if cursor_row < top {
        top = cursor_row;
    }
    if visible > 0 && cursor_row >= top + visible {
        top = cursor_row + 1 - visible;
    }
    top
}

/// The first row in view when there is no cursor (a message rather than something being edited).
pub fn clamp_view(top: usize, visible: usize, total: usize) -> usize {
    top.min(total.saturating_sub(visible))
}

/// What is said at the edge of the box when there is more text than shows: plain words, readable without color.
pub fn more_label(n: usize, above: bool) -> Option<String> {
    let lines = if n == 1 { "line" } else { "lines" };
    (n > 0).then(|| if above { format!("↑ {n} more {lines} above") } else { format!("↓ {n} more {lines} below") })
}

/// Text typed or pasted into the box, made safe to keep: line endings become `\n`, a tab four spaces, and every
/// other control character (an escape sequence's ESC among them) is dropped.
pub fn clean_paste(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    /// The rows as text, to read the tests by.
    fn shown(s: &str, width: usize) -> Vec<String> {
        let c = chars(s);
        layout(&c, width).iter().map(|r| c[r.start..r.end].iter().collect()).collect()
    }

    #[test]
    fn a_long_line_wraps_at_spaces_and_no_row_is_wider_than_the_box() {
        assert_eq!(shown("the quick brown fox jumps", 10), ["the quick ", "brown fox ", "jumps"]);
        let text = "Every word of this sentence has to be seen whole, not cut off at the edge of the box it sits in.";
        let c = chars(text);
        for width in 8..40 {
            let rows = layout(&c, width);
            // Nothing is lost or repeated: the rows put back together are the text.
            let back: String = rows.iter().flat_map(|r| c[r.start..r.end].iter()).collect();
            assert_eq!(back, text, "width {width}");
            for r in &rows {
                let row: String = c[r.start..r.end].iter().collect();
                assert!(width_of(&chars(row.trim_end_matches(' '))) <= width, "{row:?} at {width}");
                // A row that isn't the end of its line ends between two words.
                if !r.last {
                    assert!(row.ends_with(' '), "{row:?} was cut inside a word at {width}");
                }
            }
        }
    }

    #[test]
    fn a_word_longer_than_the_box_is_broken_where_it_meets_the_edge() {
        assert_eq!(shown("abcdefghijklmnop", 5), ["abcde", "fghij", "klmno", "p"]);
        assert_eq!(shown("to abcdefghij ok", 5), ["to ", "abcde", "fghij ", "ok"]);
    }

    #[test]
    fn a_line_exactly_the_width_fits_and_a_space_past_the_edge_hangs() {
        assert_eq!(shown("abcde", 5), ["abcde"]);
        assert_eq!(shown("abcde fgh", 5), ["abcde ", "fgh"]);
        // A trailing space past the edge ends the row, and the line's end gets an empty row for the cursor.
        assert_eq!(shown("abcde ", 5), ["abcde ", ""]);
        let c = chars("abcde ");
        let rows = layout(&c, 5);
        assert_eq!(locate(&rows, &c, 6), (1, 0));
        assert_eq!(locate(&rows, &c, 5), (0, 5), "the cursor on the hanging space is in the spare column");
    }

    #[test]
    fn empty_text_and_empty_lines_each_get_a_row() {
        assert_eq!(shown("", 10), [""]);
        assert_eq!(shown("a\n\nb\n", 10), ["a", "", "b", ""]);
        let c = chars("a\n\nb\n");
        let rows = layout(&c, 10);
        assert_eq!(locate(&rows, &c, 2), (1, 0), "on the empty line");
        assert_eq!(locate(&rows, &c, 5), (3, 0), "after the last line break");
        assert_eq!(locate(&rows, &c, 1), (0, 1), "at the end of the first line, before its break");
    }

    #[test]
    fn wide_characters_take_two_columns_and_are_never_split_across_the_edge() {
        // Five wide characters are ten columns: four fit in nine.
        assert_eq!(shown("日本語の文", 9), ["日本語の", "文"]);
        assert_eq!(shown("日本語の文", 10), ["日本語の文"]);
        let c = chars("ab 日本語");
        let rows = layout(&c, 6);
        assert_eq!(shown("ab 日本語", 6), ["ab ", "日本語"]);
        assert_eq!(locate(&rows, &c, 4), (1, 2), "after one wide character the cursor is two columns in");
        assert_eq!(index_at(&rows, &c, 1, 3), 4, "a column inside a wide character lands before it");
        assert_eq!(index_at(&rows, &c, 1, 6), 6);
        // A combining mark takes no room of its own.
        assert_eq!(width_of(&chars("e\u{301}")), 1);
    }

    #[test]
    fn the_cursor_at_a_wrap_belongs_to_the_next_row_and_up_down_keep_the_column() {
        let c = chars("the quick brown fox");
        let rows = layout(&c, 10);
        assert_eq!(rows.len(), 2);
        // Index 10 is both "after the space" on row 0 and "before brown" on row 1: it is shown on row 1.
        assert_eq!(locate(&rows, &c, 10), (1, 0));
        assert_eq!(locate(&rows, &c, 9), (0, 9));
        // Going up from far right of a short row ends at that row's last character, never on the next row.
        assert_eq!(index_at(&rows, &c, 0, 50), 9);
        assert_eq!(index_at(&rows, &c, 1, 50), c.len(), "the last row of a line reaches its end");
        assert_eq!(index_at(&rows, &c, 1, 3), 13);
        // Every index round-trips through its row and column.
        for i in 0..=c.len() {
            let (r, col) = locate(&rows, &c, i);
            assert_eq!(index_at(&rows, &c, r, col), i, "index {i}");
        }
    }

    #[test]
    fn every_position_round_trips_with_wide_characters_and_breaks_too() {
        let text = "Übersicht: 日本語のテキスト and a verylongwordthatmustbreak\n\nend 🙂 ok ";
        let c = chars(text);
        for width in 3..20 {
            let rows = layout(&c, width);
            for i in 0..=c.len() {
                let (r, col) = locate(&rows, &c, i);
                assert!(col <= width, "col {col} past {width} at {i}");
                assert_eq!(index_at(&rows, &c, r, col), i, "index {i} at width {width}");
            }
        }
    }

    #[test]
    fn the_view_follows_the_cursor_and_scrolls_as_little_as_it_can() {
        // 30 rows, 10 in view.
        assert_eq!(clamp_top(0, 0, 10, 30), 0);
        assert_eq!(clamp_top(0, 9, 10, 30), 0, "the bottom row is still in view");
        assert_eq!(clamp_top(0, 10, 10, 30), 1, "one row past it scrolls by one");
        assert_eq!(clamp_top(15, 29, 10, 30), 20);
        assert_eq!(clamp_top(15, 3, 10, 30), 3, "the cursor above the view brings it up");
        assert_eq!(clamp_top(15, 18, 10, 30), 15, "a cursor in view moves nothing");
        assert_eq!(clamp_top(25, 25, 10, 30), 20, "never more empty space below the text than it needs");
        assert_eq!(clamp_top(5, 2, 10, 4), 0, "text shorter than the box starts at the top");
        assert_eq!(clamp_view(50, 10, 30), 20);
        assert_eq!(clamp_view(3, 10, 5), 0);
    }

    #[test]
    fn more_above_and_below_is_said_in_words() {
        assert_eq!(more_label(0, true), None);
        assert_eq!(more_label(1, true).as_deref(), Some("↑ 1 more line above"));
        assert_eq!(more_label(12, false).as_deref(), Some("↓ 12 more lines below"));
    }

    #[test]
    fn pasted_text_loses_its_control_characters_and_keeps_its_lines() {
        assert_eq!(clean_paste("a\r\nb\rc\td\u{1b}[2Je\u{7}"), "a\nb\nc    d[2Je");
        assert_eq!(clean_paste("日本\n🙂"), "日本\n🙂");
    }
}
