//! Text selection in the attached view. platoon owns the mouse there (the wheel, the back hint), so the
//! terminal's own selection is not available without holding Shift; this is the same thing done inside
//! platoon: which cells are selected, what word or line a double or triple click means, and the text in them.
//!
//! Everything works on the emulator's *visible* screen, so it follows the scrollback offset by itself:
//! `Screen::cell` and `Screen::row_wrapped` both look at the rows currently on display.

use std::time::{Duration, Instant};

/// Clicks closer together than this, on the same or a neighbouring cell, are one multi-click.
pub const MULTI_CLICK: Duration = Duration::from_millis(400);

/// A cell of the session's screen (0,0 is its top left corner, whatever the scrollback offset).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Point {
    pub row: u16,
    pub col: u16,
}

impl Point {
    pub fn new(row: u16, col: u16) -> Self {
        Self { row, col }
    }
}

/// What a drag selects by: single characters, or whole words (after a double click) or lines (triple click).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Char,
    Word,
    Line,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    pub anchor: Point,
    pub head: Point,
    pub unit: Unit,
}

/// The cells a selection covers, both ends included, first to last in reading order: the rest of the first
/// row, every cell of the rows between, and the start of the last row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: Point,
    pub end: Point,
}

impl Span {
    pub fn contains(&self, row: u16, col: u16) -> bool {
        let p = Point::new(row, col);
        self.start <= p && p <= self.end
    }
}

/// The last click, for telling a double click from two single ones.
#[derive(Clone, Copy, Debug)]
pub struct Click {
    pub at: Instant,
    pub point: Point,
    pub count: u8,
}

/// Which click this is: 1, 2 or 3. A click soon enough after the last one, on the same or a neighbouring
/// cell, continues it; a fourth starts over at one (like most terminals).
pub fn click_count(prev: Option<Click>, now: Instant, point: Point) -> u8 {
    let Some(p) = prev else { return 1 };
    let near = p.point.row.abs_diff(point.row) <= 1 && p.point.col.abs_diff(point.col) <= 1;
    if near && now.saturating_duration_since(p.at) <= MULTI_CLICK && p.count < 3 { p.count + 1 } else { 1 }
}

impl Selection {
    pub fn new(at: Point, unit: Unit) -> Self {
        Self { anchor: at, head: at, unit }
    }

    /// The cells selected now, or None when nothing is: a press that has not moved off its cell.
    pub fn span(&self, screen: &vt100::Screen) -> Option<Span> {
        let (a, h) = (self.anchor, self.head);
        let (lo, hi) = match self.unit {
            Unit::Char => {
                if lead(screen, a) == lead(screen, h) {
                    return None;
                }
                let (s, e) = if a <= h { (a, h) } else { (h, a) };
                (lead(screen, s), trail(screen, e))
            }
            Unit::Word => {
                let (a0, a1) = word_bounds(screen, a);
                let (h0, h1) = word_bounds(screen, h);
                (a0.min(h0), a1.max(h1))
            }
            Unit::Line => {
                let (a0, a1) = line_bounds(screen, a);
                let (h0, h1) = line_bounds(screen, h);
                (a0.min(h0), a1.max(h1))
            }
        };
        Some(Span { start: lo, end: hi })
    }
}

/// The first cell of the character at `p`: a wide character fills two cells, and a selection never splits it.
fn lead(screen: &vt100::Screen, p: Point) -> Point {
    match screen.cell(p.row, p.col) {
        Some(c) if c.is_wide_continuation() && p.col > 0 => Point::new(p.row, p.col - 1),
        _ => p,
    }
}

/// The last cell of the character at `p`.
fn trail(screen: &vt100::Screen, p: Point) -> Point {
    let (_, cols) = screen.size();
    match screen.cell(p.row, p.col) {
        Some(c) if c.is_wide() && p.col + 1 < cols => Point::new(p.row, p.col + 1),
        _ => p,
    }
}

/// The cell after `p` in reading order. A row that continues on the next one (the emulator wrapped it, the
/// program didn't press enter) goes on to it; a row that really ended does not.
fn next(screen: &vt100::Screen, p: Point) -> Option<Point> {
    let (rows, cols) = screen.size();
    if p.col + 1 < cols {
        Some(Point::new(p.row, p.col + 1))
    } else if screen.row_wrapped(p.row) && p.row + 1 < rows {
        Some(Point::new(p.row + 1, 0))
    } else {
        None
    }
}

fn prev(screen: &vt100::Screen, p: Point) -> Option<Point> {
    let (_, cols) = screen.size();
    if p.col > 0 {
        Some(Point::new(p.row, p.col - 1))
    } else if p.row > 0 && screen.row_wrapped(p.row - 1) {
        Some(Point::new(p.row - 1, cols - 1))
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Space,
    /// Part of a word: letters, digits, and the characters that live inside paths, URLs and flags.
    Word,
    /// Punctuation that ends a word. A double click on one selects the run of that same character.
    Stop(char),
}

/// Punctuation that stays inside a word, so that a path, URL or `--flag=value` selects as one.
const INSIDE_WORDS: &str = "_-./:~@%+=#?&";
/// Punctuation that ends a word. Box drawing characters do too: Claude's own interface is full of them.
const ENDS_WORDS: &str = ",;()[]{}<>\"'`|\\!*^$“”‘’«»";

fn class_of(contents: &str) -> Class {
    let Some(c) = contents.chars().next() else { return Class::Space };
    if c.is_whitespace() {
        Class::Space
    } else if INSIDE_WORDS.contains(c) || c.is_alphanumeric() {
        Class::Word
    } else if ENDS_WORDS.contains(c) || ('\u{2500}'..='\u{259f}').contains(&c) {
        Class::Stop(c)
    } else {
        // Anything else (symbols, emoji, other scripts' marks) is text like any other.
        Class::Word
    }
}

fn class_at(screen: &vt100::Screen, p: Point) -> Class {
    screen.cell(p.row, p.col).map_or(Class::Space, |c| class_of(c.contents()))
}

/// The word under `p`: a run of cells of one class, following soft wraps. Trailing `.`, `:` and `?` are left
/// off ("run cargo build." selects `build`), unless the click was on them.
fn word_bounds(screen: &vt100::Screen, p: Point) -> (Point, Point) {
    let p = lead(screen, p);
    let class = class_at(screen, p);
    let same = |q: Point| class_at(screen, lead(screen, q)) == class;
    let mut start = p;
    while let Some(q) = prev(screen, start).filter(|&q| same(q)) {
        start = lead(screen, q);
    }
    let mut end = p;
    while let Some(q) = next(screen, end).filter(|&q| same(q)) {
        end = q;
    }
    if class == Class::Word {
        let mut trimmed = end;
        while trimmed > start
            && screen.cell(trimmed.row, trimmed.col).is_some_and(|c| matches!(c.contents(), "." | ":" | "?"))
        {
            match prev(screen, trimmed) {
                Some(q) => trimmed = q,
                None => break,
            }
        }
        // A word made of nothing else ("..") is left whole rather than trimmed to nothing.
        let all_trim = trimmed == start && screen.cell(start.row, start.col).is_some_and(|c| matches!(c.contents(), "." | ":" | "?"));
        if trimmed >= p && !all_trim {
            end = trimmed;
        }
    }
    (start, trail(screen, end))
}

/// The whole logical line at `p`: every row it wraps over, from its first cell to its last.
fn line_bounds(screen: &vt100::Screen, p: Point) -> (Point, Point) {
    let (rows, cols) = screen.size();
    let mut first = p.row;
    while first > 0 && screen.row_wrapped(first - 1) {
        first -= 1;
    }
    let mut last = p.row;
    while last + 1 < rows && screen.row_wrapped(last) {
        last += 1;
    }
    (Point::new(first, 0), Point::new(last, cols.saturating_sub(1)))
}

/// The text in a span, as it would be pasted: rows the emulator wrapped are joined without a break, every
/// other row end is a newline, trailing spaces of each line are dropped, and so are blank lines at either end.
pub fn extract(screen: &vt100::Screen, span: Span) -> String {
    let (rows, cols) = screen.size();
    let mut out = String::new();
    for row in span.start.row..=span.end.row.min(rows.saturating_sub(1)) {
        let from = if row == span.start.row { span.start.col } else { 0 };
        let to = if row == span.end.row { span.end.col } else { cols };
        // Blank cells only count once something follows them, so the empty tail of a row adds nothing.
        let mut blanks = 0;
        let mut line = String::new();
        for col in from..=to.min(cols.saturating_sub(1)) {
            let Some(cell) = screen.cell(row, col) else { break };
            if cell.is_wide_continuation() {
                continue;
            }
            if cell.has_contents() {
                line.extend(std::iter::repeat_n(' ', blanks));
                blanks = 0;
                line.push_str(cell.contents());
            } else {
                blanks += 1;
            }
        }
        if row < span.end.row && screen.row_wrapped(row) {
            // The line goes on below: a space at the end of this row belongs to the text.
            out.push_str(&line);
        } else {
            out.push_str(line.trim_end());
            if row < span.end.row {
                out.push('\n');
            }
        }
    }
    let mut text = out.trim_end();
    while let Some((first, rest)) = text.split_once('\n') {
        if !first.trim().is_empty() {
            break;
        }
        text = rest;
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen of `rows` x `cols` after the emulator was fed `text` (`\n` is written as a carriage return
    /// and line feed, like a terminal's line discipline).
    fn screen(rows: u16, cols: u16, text: &str) -> vt100::Parser {
        let mut p = vt100::Parser::new(rows, cols, 100);
        p.process(text.replace('\n', "\r\n").as_bytes());
        p
    }

    fn pt(row: u16, col: u16) -> Point {
        Point::new(row, col)
    }

    /// The text between two cells dragged with the mouse.
    fn drag(p: &vt100::Parser, from: Point, to: Point) -> String {
        let sel = Selection { anchor: from, head: to, unit: Unit::Char };
        sel.span(p.screen()).map(|s| extract(p.screen(), s)).unwrap_or_default()
    }

    fn word(p: &vt100::Parser, at: Point) -> String {
        let sel = Selection::new(at, Unit::Word);
        extract(p.screen(), sel.span(p.screen()).unwrap())
    }

    fn line(p: &vt100::Parser, at: Point) -> String {
        let sel = Selection::new(at, Unit::Line);
        extract(p.screen(), sel.span(p.screen()).unwrap())
    }

    // --- what the emulator does with rows that wrap, which the extraction relies on ------------------

    #[test]
    fn the_emulator_marks_a_row_that_wrapped_but_not_one_that_ended_with_a_line_break() {
        let p = screen(4, 10, "0123456789ab\nnext");
        let s = p.screen();
        assert!(s.row_wrapped(0), "the first row ran into the second");
        assert!(!s.row_wrapped(1), "a line break ended it");
        assert!(!s.row_wrapped(2));
        assert_eq!(s.cell(1, 0).unwrap().contents(), "a");
    }

    #[test]
    fn contents_between_ends_before_its_last_column_and_keeps_explicit_spaces_which_is_why_platoon_extracts_itself() {
        let p = screen(3, 10, "abc  \nxyz");
        let s = p.screen();
        // The end column is exclusive, so a selection that includes its end cell needs one more.
        assert_eq!(s.contents_between(0, 0, 0, 2), "ab");
        // Spaces the program printed are cells with contents, and stay at the end of a line.
        assert_eq!(s.contents_between(0, 0, 1, 3), "abc  \nxyz");
        assert_eq!(extract(s, Span { start: pt(0, 0), end: pt(1, 2) }), "abc\nxyz");
    }

    // --- extraction --------------------------------------------------------------------------------

    #[test]
    fn a_selection_inside_one_row_includes_both_end_cells() {
        let p = screen(3, 20, "hello brave world");
        assert_eq!(drag(&p, pt(0, 6), pt(0, 10)), "brave");
        assert_eq!(drag(&p, pt(0, 0), pt(0, 4)), "hello");
    }

    #[test]
    fn dragging_backwards_or_upwards_selects_the_same_cells() {
        let p = screen(4, 20, "first line here\nsecond line\nthird");
        let forward = drag(&p, pt(0, 6), pt(2, 2));
        assert_eq!(forward, "line here\nsecond line\nthi");
        assert_eq!(drag(&p, pt(2, 2), pt(0, 6)), forward, "up and to the left");
        assert_eq!(drag(&p, pt(0, 10), pt(0, 6)), "line", "to the left on one row");
    }

    #[test]
    fn several_rows_take_the_rest_of_the_first_all_the_middle_and_the_start_of_the_last() {
        let p = screen(5, 12, "alpha beta\ngamma\n\ndelta epsilon");
        assert_eq!(drag(&p, pt(0, 6), pt(3, 4)), "beta\ngamma\n\ndelta");
    }

    #[test]
    fn rows_the_emulator_wrapped_join_without_a_break_and_real_line_breaks_stay() {
        // Width 10: "abcdefghij" fills row 0 and wraps; the line break after "klm" is real.
        let p = screen(4, 10, "abcdefghijklm\nnext line");
        assert_eq!(drag(&p, pt(0, 0), pt(2, 3)), "abcdefghijklm\nnext");
        // A wrapped row keeps a space at its end: it separates two words of one line.
        let p = screen(3, 10, "aaaa bbbb cccc");
        assert!(p.screen().row_wrapped(0));
        assert_eq!(drag(&p, pt(0, 0), pt(1, 8)), "aaaa bbbb cccc");
    }

    #[test]
    fn trailing_spaces_are_trimmed_from_every_line_and_blank_lines_at_the_ends_go() {
        let p = screen(6, 20, "one   \n\ntwo   \n   \nthree");
        assert_eq!(drag(&p, pt(0, 0), pt(4, 19)), "one\n\ntwo\n\nthree");
        // Blank lines before and after the text are not part of what was meant.
        let p = screen(6, 20, "\n\nbody\n\n\n");
        assert_eq!(drag(&p, pt(0, 0), pt(5, 19)), "body");
        // Leading spaces on the first line are position, and stay.
        let p = screen(2, 20, "    indented");
        assert_eq!(drag(&p, pt(0, 0), pt(0, 11)), "    indented");
    }

    #[test]
    fn a_selection_of_only_blanks_is_empty() {
        let p = screen(4, 20, "text\n\n\n");
        assert_eq!(drag(&p, pt(1, 0), pt(3, 19)), "");
        assert_eq!(drag(&p, pt(0, 10), pt(0, 15)), "", "past the end of the text");
    }

    #[test]
    fn a_press_that_has_not_left_its_cell_selects_nothing() {
        let p = screen(2, 10, "abc");
        assert!(Selection::new(pt(0, 1), Unit::Char).span(p.screen()).is_none());
        let s = Selection { anchor: pt(0, 1), head: pt(0, 2), unit: Unit::Char };
        assert_eq!(s.span(p.screen()), Some(Span { start: pt(0, 1), end: pt(0, 2) }));
    }

    #[test]
    fn a_wide_character_is_never_split_by_either_end() {
        let p = screen(2, 12, "a世界b");
        // cells: a=0, 世=1..2, 界=3..4, b=5
        assert_eq!(drag(&p, pt(0, 0), pt(0, 1)), "a世", "ending on the first half takes the whole character");
        assert_eq!(drag(&p, pt(0, 0), pt(0, 2)), "a世", "ending on the second half too");
        assert_eq!(drag(&p, pt(0, 2), pt(0, 5)), "世界b", "starting on the second half takes the whole character");
        assert_eq!(drag(&p, pt(0, 1), pt(0, 2)), "", "one character is a press that hasn't moved");
        let s = Selection { anchor: pt(0, 4), head: pt(0, 1), unit: Unit::Char };
        assert_eq!(s.span(p.screen()), Some(Span { start: pt(0, 1), end: pt(0, 4) }));
        assert_eq!(extract(p.screen(), s.span(p.screen()).unwrap()), "世界");
    }

    #[test]
    fn a_wide_character_that_wraps_whole_does_not_leave_a_gap_in_the_text() {
        // Width 5: four narrow cells, then a wide character that does not fit and moves to the next row.
        let p = screen(3, 5, "abcd世x");
        let text = drag(&p, pt(0, 0), pt(1, 4));
        assert!(text == "abcd世x" || text == "abcd\n世x", "{text:?}");
        assert!(!text.contains("  "), "{text:?}");
    }

    #[test]
    fn extraction_follows_the_scrollback_offset() {
        let mut p = vt100::Parser::new(3, 10, 100);
        for i in 0..8 {
            p.process(format!("line {i}\r\n").as_bytes());
        }
        // Not scrolled: the last lines are on screen (the cursor sits on a blank row at the bottom).
        assert_eq!(drag(&p, pt(0, 0), pt(1, 9)), "line 6\nline 7");
        p.screen_mut().set_scrollback(2);
        assert_eq!(p.screen().cell(0, 5).unwrap().contents(), "4", "two rows further back");
        assert_eq!(drag(&p, pt(0, 0), pt(1, 9)), "line 4\nline 5");
        p.screen_mut().set_scrollback(100);
        assert_eq!(drag(&p, pt(0, 0), pt(2, 9)), "line 0\nline 1\nline 2");
    }

    #[test]
    fn a_span_reaching_past_the_screen_is_clamped_not_a_panic() {
        let p = screen(2, 6, "abc");
        let s = Span { start: pt(0, 0), end: pt(5, 50) };
        assert_eq!(extract(p.screen(), s), "abc");
    }

    // --- words and lines ---------------------------------------------------------------------------

    #[test]
    fn a_word_is_a_run_of_letters_digits_and_the_punctuation_that_lives_in_paths_and_urls() {
        let p = screen(4, 90, "see https://ex.com/a_b-c.html?x=1&y=2#top and ~/repos/platoon/src/app.rs:120 now\nrun --flag=value (in parens), \"quoted\"");
        assert_eq!(word(&p, pt(0, 10)), "https://ex.com/a_b-c.html?x=1&y=2#top");
        assert_eq!(word(&p, pt(0, 50)), "~/repos/platoon/src/app.rs:120");
        assert_eq!(word(&p, pt(1, 6)), "--flag=value");
        assert_eq!(word(&p, pt(0, 0)), "see");
        assert_eq!(word(&p, pt(1, 19)), "in", "an opening bracket ends a word");
        assert_eq!(word(&p, pt(1, 24)), "parens");
        assert_eq!(word(&p, pt(1, 34)), "quoted", "so do quotes and commas");
    }

    #[test]
    fn a_sentence_ending_dot_or_colon_is_not_part_of_the_word_unless_it_was_clicked() {
        let p = screen(2, 30, "run cargo build. Then: check");
        assert_eq!(word(&p, pt(0, 10)), "build");
        assert_eq!(word(&p, pt(0, 18)), "Then");
        assert_eq!(word(&p, pt(0, 15)), "build.", "the click was on the dot");
        let p = screen(1, 20, "..");
        assert_eq!(word(&p, pt(0, 0)), "..", "nothing but dots is not trimmed to nothing");
    }

    #[test]
    fn a_double_click_on_blanks_or_a_box_drawing_run_selects_that_run() {
        let p = screen(2, 20, "ab    cd\n│ x ───── y │");
        let sel = Selection::new(pt(0, 3), Unit::Word);
        assert_eq!(sel.span(p.screen()), Some(Span { start: pt(0, 2), end: pt(0, 5) }));
        assert_eq!(word(&p, pt(0, 3)), "", "blanks copy nothing");
        let sel = Selection::new(pt(1, 6), Unit::Word);
        assert_eq!(sel.span(p.screen()), Some(Span { start: pt(1, 4), end: pt(1, 8) }));
        assert_eq!(word(&p, pt(1, 0)), "│", "a border is a word of its own");
        assert_eq!(word(&p, pt(1, 2)), "x");
    }

    #[test]
    fn a_word_that_wraps_over_two_rows_is_one_word() {
        let p = screen(4, 10, "go to https://example.org/long/path now");
        assert!(p.screen().row_wrapped(0) && p.screen().row_wrapped(1));
        let url = "https://example.org/long/path";
        assert_eq!(word(&p, pt(0, 8)), url, "clicked in the first part");
        assert_eq!(word(&p, pt(2, 3)), url, "clicked in the last part");
    }

    #[test]
    fn a_word_does_not_continue_past_a_real_line_break() {
        // The first row is exactly as wide as the screen, but the program ended it itself.
        let p = screen(3, 6, "abcdef\nghi");
        assert!(!p.screen().row_wrapped(0));
        assert_eq!(word(&p, pt(0, 2)), "abcdef");
        assert_eq!(word(&p, pt(1, 1)), "ghi");
    }

    #[test]
    fn a_wide_character_is_part_of_the_word_it_is_in() {
        let p = screen(1, 20, "日本語 text");
        assert_eq!(word(&p, pt(0, 1)), "日本語", "the second half of the first character");
        assert_eq!(word(&p, pt(0, 8)), "text");
    }

    #[test]
    fn a_triple_click_takes_the_logical_line_following_soft_wraps() {
        let p = screen(6, 10, "first\naaaa bbbb cccc\nlast");
        // "aaaa bbbb cccc" wraps over rows 1 and 2.
        assert!(p.screen().row_wrapped(1));
        let whole = "aaaa bbbb cccc";
        assert_eq!(line(&p, pt(1, 3)), whole);
        assert_eq!(line(&p, pt(2, 3)), whole);
        assert_eq!(line(&p, pt(0, 2)), "first");
        assert_eq!(line(&p, pt(3, 1)), "last");
        let sel = Selection::new(pt(2, 3), Unit::Line);
        assert_eq!(sel.span(p.screen()), Some(Span { start: pt(1, 0), end: pt(2, 9) }));
    }

    #[test]
    fn dragging_after_a_double_or_triple_click_grows_the_selection_by_words_or_lines() {
        let p = screen(4, 30, "one two three\nfour five six");
        let mut sel = Selection::new(pt(0, 5), Unit::Word); // "two"
        sel.head = pt(1, 6); // into "five"
        assert_eq!(extract(p.screen(), sel.span(p.screen()).unwrap()), "two three\nfour five");
        sel.head = pt(0, 0);
        assert_eq!(extract(p.screen(), sel.span(p.screen()).unwrap()), "one two");
        let mut sel = Selection::new(pt(0, 5), Unit::Line);
        sel.head = pt(1, 2);
        assert_eq!(extract(p.screen(), sel.span(p.screen()).unwrap()), "one two three\nfour five six");
    }

    // --- clicks --------------------------------------------------------------------------------------

    #[test]
    fn clicks_in_quick_succession_on_the_same_or_a_neighbouring_cell_count_up_to_three_then_start_over() {
        let t0 = Instant::now();
        let mut prev = None;
        let mut counts = Vec::new();
        for (ms, at) in [(0, pt(3, 3)), (100, pt(3, 3)), (200, pt(3, 4)), (300, pt(3, 4)), (400, pt(3, 4))] {
            let now = t0 + Duration::from_millis(ms);
            let count = click_count(prev, now, at);
            counts.push(count);
            prev = Some(Click { at: now, point: at, count });
        }
        assert_eq!(counts, [1, 2, 3, 1, 2]);
    }

    #[test]
    fn a_slow_click_or_one_somewhere_else_is_a_first_click() {
        let t0 = Instant::now();
        let first = Click { at: t0, point: pt(5, 5), count: 1 };
        assert_eq!(click_count(Some(first), t0 + Duration::from_millis(400), pt(5, 5)), 2, "exactly at the limit counts");
        assert_eq!(click_count(Some(first), t0 + Duration::from_millis(401), pt(5, 5)), 1, "too slow");
        assert_eq!(click_count(Some(first), t0 + Duration::from_millis(50), pt(5, 7)), 1, "two cells away");
        assert_eq!(click_count(Some(first), t0 + Duration::from_millis(50), pt(7, 5)), 1, "two rows away");
        assert_eq!(click_count(Some(first), t0 + Duration::from_millis(50), pt(6, 6)), 2, "diagonally adjacent");
        assert_eq!(click_count(None, t0, pt(0, 0)), 1);
    }
}
