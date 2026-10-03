//! Markdown drawn as a terminal would show it: a project's notes (and later the hand-off box) stay plain Markdown
//! on disk, and this turns them into styled, wrapped lines for a pane of a given width. Pure: text, a width, the
//! theme and the glyph choice go in, lines come out, so everything here is tested without a terminal.
//!
//! What is drawn, and how it still reads without color:
//! - `#` section headings (bold, underlined), `##` subheadings (bold), `###` and deeper (bold italic).
//! - Bullets as `•` / `◦` / `▪` by depth (`-` / `*` / `+` with plain glyphs), numbered lists as `1.`, both with a
//!   hanging indent: a wrapped line starts under the text, not under the bullet.
//! - **bold**, *italic*, `inline code` (its own colors), links as their text (underlined; the address is left out:
//!   it is in the file), block quotes behind a `│` bar, fenced and indented code on a band of its own, not
//!   word-wrapped but cut at a space where it can be, and a horizontal rule across the pane.
//! - A line break inside a paragraph is kept: notes are written a fact a line, and joining those lines into one
//!   paragraph, as strict Markdown would, loses exactly what they were written to show.
//!
//! Whatever the text, nothing here panics, and no control character or escape sequence reaches the screen.

use crate::theme::Theme;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The bullets of a list by how deeply it is nested, when the terminal has the glyphs.
const BULLETS: [&str; 3] = ["•", "◦", "▪"];
/// The same, for the Linux console and `PLATOON_GLYPHS=plain`.
const PLAIN_BULLETS: [&str; 3] = ["-", "*", "+"];
/// How many spaces a tab in a code block stands for.
const TAB: usize = 4;

/// `text` as lines of at most `width` columns (a prefix wider than a tiny pane may still overflow it; the pane
/// clips). `plain`: bullets from `PLAIN_BULLETS`.
pub fn render(text: &str, width: usize, theme: &Theme, plain: bool) -> Vec<Line<'static>> {
    let mut v = View::new(width.max(1), theme, plain);
    for event in Parser::new_ext(text, Options::empty()) {
        v.event(event);
    }
    v.finish()
}

/// One piece of inline text: words in one style, or a line break.
enum Piece {
    Text(String, Style),
    Break,
}

/// A block the current text sits inside, which puts something at the start of each of its lines.
enum Container {
    Quote,
    /// A list item: its bullet or number on its first line, spaces as wide on every line after.
    Item {
        first: String,
        used: bool,
    },
}

struct View<'t> {
    width: usize,
    t: &'t Theme,
    plain: bool,
    lines: Vec<Line<'static>>,
    pieces: Vec<Piece>,
    containers: Vec<Container>,
    /// Open lists, innermost last: the next number of a numbered one, None for bullets.
    lists: Vec<Option<u64>>,
    heading: Option<HeadingLevel>,
    strong: usize,
    emphasis: usize,
    strike: usize,
    link: usize,
    /// The text of a code block while it is being read.
    code: Option<String>,
    /// Inside raw HTML, which is shown as it is written, dimmed.
    html: bool,
    /// A blank line is owed before the next block.
    gap: bool,
}

impl<'t> View<'t> {
    fn new(width: usize, t: &'t Theme, plain: bool) -> Self {
        View {
            width,
            t,
            plain,
            lines: Vec::new(),
            pieces: Vec::new(),
            containers: Vec::new(),
            lists: Vec::new(),
            heading: None,
            strong: 0,
            emphasis: 0,
            strike: 0,
            link: 0,
            code: None,
            html: false,
            gap: false,
        }
    }

    fn heading_style(&self, level: HeadingLevel) -> Style {
        match level {
            HeadingLevel::H1 => Style::new().fg(self.t.accent).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            HeadingLevel::H2 => Style::new().fg(self.t.accent).add_modifier(Modifier::BOLD),
            _ => Style::new().fg(self.t.text).add_modifier(Modifier::BOLD | Modifier::ITALIC),
        }
    }

    /// The style of ordinary text here: a heading's, or the body's, with bold, italic and a link on top.
    fn text_style(&self) -> Style {
        let mut s = match self.heading {
            Some(level) => self.heading_style(level),
            None if self.html => Style::new().fg(self.t.dim),
            None => Style::new().fg(self.t.text),
        };
        if self.strong > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.emphasis > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.link > 0 {
            s = s.fg(self.t.accent).add_modifier(Modifier::UNDERLINED);
        }
        s
    }

    fn code_style(&self) -> Style {
        Style::new().fg(self.t.ok).bg(self.t.surface)
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(s) => match self.code.as_mut() {
                Some(code) => code.push_str(&s),
                None => self.text(&s, self.text_style()),
            },
            Event::Code(s) => self.pieces.push(Piece::Text(clean(&s), self.code_style())),
            Event::Html(s) | Event::InlineHtml(s) => self.text(&s, Style::new().fg(self.t.dim)),
            Event::SoftBreak | Event::HardBreak => self.pieces.push(Piece::Break),
            Event::Rule => {
                self.start_block();
                let bar = if self.plain { "-" } else { "─" };
                let n = self.width.saturating_sub(self.prefix_width()).max(1);
                let prefix = self.prefix(true);
                self.push_line(prefix, vec![Span::styled(bar.repeat(n), Style::new().fg(self.t.border))]);
                self.gap = true;
            }
            Event::FootnoteReference(s) => self.text(&format!("[{s}]"), self.text_style()),
            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }, self.text_style()),
            Event::InlineMath(s) | Event::DisplayMath(s) => self.pieces.push(Piece::Text(clean(&s), self.code_style())),
        }
    }

    /// Text of a paragraph or heading; a line break in it (raw HTML keeps its own) becomes a break.
    fn text(&mut self, s: &str, style: Style) {
        for (k, part) in s.split('\n').enumerate() {
            if k > 0 {
                self.pieces.push(Piece::Break);
            }
            let part = clean(part);
            if !part.is_empty() {
                self.pieces.push(Piece::Text(part, style));
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.start_block(),
            Tag::Heading { level, .. } => {
                self.start_block();
                self.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.start_block();
                self.containers.push(Container::Quote);
            }
            Tag::CodeBlock(_) => {
                self.start_block();
                self.code = Some(String::new());
            }
            Tag::HtmlBlock => {
                self.start_block();
                self.html = true;
            }
            Tag::List(start) => {
                self.flush();
                if self.lists.is_empty() {
                    self.start_block();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len().saturating_sub(1);
                let first = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let mark = format!("{n}. ");
                        *n += 1;
                        mark
                    }
                    _ => {
                        let set = if self.plain { PLAIN_BULLETS } else { BULLETS };
                        format!("{} ", set[depth % set.len()])
                    }
                };
                self.containers.push(Container::Item { first, used: false });
            }
            Tag::Emphasis => self.emphasis += 1,
            Tag::Strong => self.strong += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { .. } => self.link += 1,
            // An image is its description, like a link is its text.
            Tag::Image { .. } => self.emphasis += 1,
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.gap = true;
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.heading = None;
                self.gap = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.containers.pop();
                self.gap = true;
            }
            TagEnd::CodeBlock => {
                let code = self.code.take().unwrap_or_default();
                self.code_block(&code);
                self.gap = true;
            }
            TagEnd::HtmlBlock => {
                self.flush();
                self.html = false;
                self.gap = true;
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.gap = true;
                }
            }
            TagEnd::Item => {
                self.flush();
                // An item with nothing in it still shows its bullet.
                if matches!(self.containers.last(), Some(Container::Item { used: false, .. })) {
                    let prefix = self.prefix(true);
                    self.push_line(prefix, Vec::new());
                }
                self.containers.pop();
            }
            TagEnd::Emphasis => self.emphasis = self.emphasis.saturating_sub(1),
            TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => self.link = self.link.saturating_sub(1),
            TagEnd::Image => self.emphasis = self.emphasis.saturating_sub(1),
            _ => {}
        }
    }

    /// A new block begins: what came before is drawn, and the blank line owed goes in.
    fn start_block(&mut self) {
        self.flush();
        if self.gap && !self.lines.is_empty() {
            let bars = self.prefix(false);
            let keep = bars.iter().any(|s| !s.content.trim().is_empty());
            let prefix =
                if keep { bars.into_iter().map(|s| Span::styled(s.content.trim_end().to_string(), s.style)).collect() } else { Vec::new() };
            self.lines.push(Line::from(prefix));
        }
        self.gap = false;
    }

    /// What goes in front of a line: a bar for each quote it is in and a bullet (or its width in spaces) for each
    /// list item. `first`: this is the first line drawn in the innermost items, so their bullets show (once).
    fn prefix(&mut self, first: bool) -> Vec<Span<'static>> {
        let (bar, bullet) = (Style::new().fg(self.t.border), Style::new().fg(self.t.accent));
        let mut out = Vec::new();
        for c in self.containers.iter_mut() {
            match c {
                Container::Quote => out.push(Span::styled("│ ", bar)),
                Container::Item { first: mark, used } => {
                    if first && !*used {
                        *used = true;
                        out.push(Span::styled(mark.clone(), bullet));
                    } else {
                        out.push(Span::raw(" ".repeat(mark.width())));
                    }
                }
            }
        }
        out
    }

    fn prefix_width(&self) -> usize {
        self.containers
            .iter()
            .map(|c| match c {
                Container::Quote => 2,
                Container::Item { first, .. } => first.width(),
            })
            .sum()
    }

    fn push_line(&mut self, mut prefix: Vec<Span<'static>>, content: Vec<Span<'static>>) {
        prefix.extend(content);
        self.lines.push(Line::from(prefix));
    }

    /// Draw the inline text gathered so far, wrapped at word boundaries with the hanging indent of its containers.
    fn flush(&mut self) {
        if self.pieces.is_empty() {
            return;
        }
        let pieces = std::mem::take(&mut self.pieces);
        let space = self.text_style_for_spaces();
        let avail = self.width.saturating_sub(self.prefix_width()).max(1);
        let mut first = true;
        let mut line: Vec<Span<'static>> = Vec::new();
        let mut used = 0usize;
        for tok in words(&pieces) {
            match tok {
                Tok::Break => {
                    let prefix = self.prefix(first);
                    first = false;
                    self.push_line(prefix, std::mem::take(&mut line));
                    used = 0;
                }
                Tok::Word { segs, space_before } => {
                    let w: usize = segs.iter().map(|(s, _)| s.width()).sum();
                    let gap = usize::from(used > 0 && space_before);
                    if used > 0 && used + gap + w > avail {
                        let prefix = self.prefix(first);
                        first = false;
                        self.push_line(prefix, std::mem::take(&mut line));
                        used = 0;
                    } else if gap == 1 {
                        push_char(&mut line, ' ', space);
                        used += 1;
                    }
                    // A word wider than the whole line is cut into pieces that fit.
                    for (text, style) in segs {
                        for ch in text.chars() {
                            let cw = ch.width().unwrap_or(0);
                            if used > 0 && used + cw > avail {
                                let prefix = self.prefix(first);
                                first = false;
                                self.push_line(prefix, std::mem::take(&mut line));
                                used = 0;
                            }
                            push_char(&mut line, ch, style);
                            used += cw;
                        }
                    }
                }
            }
        }
        if !line.is_empty() || first {
            let prefix = self.prefix(first);
            self.push_line(prefix, line);
        }
    }

    /// The style of the space between two words: a heading's, so its underline runs on; plain text otherwise.
    fn text_style_for_spaces(&self) -> Style {
        match self.heading {
            Some(level) => self.heading_style(level),
            None => Style::new().fg(self.t.text),
        }
    }

    /// A code block: its own band, indented, each line cut to fit (at a space when there is one near the end).
    fn code_block(&mut self, code: &str) {
        let style = Style::new().fg(self.t.subtext).bg(self.t.surface);
        let avail = self.width.saturating_sub(self.prefix_width() + 2).max(1);
        let body = code.strip_suffix('\n').unwrap_or(code);
        let mut first = true;
        for raw in body.split('\n') {
            let text = clean(&raw.replace('\t', &" ".repeat(TAB)));
            for chunk in cut_code(&text, avail) {
                let pad = avail.saturating_sub(chunk.width());
                let prefix = self.prefix(first);
                first = false;
                self.push_line(prefix, vec![Span::styled(format!("  {chunk}{}", " ".repeat(pad)), style)]);
            }
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush();
        while self.lines.last().is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty())) {
            self.lines.pop();
        }
        self.lines
    }
}

/// One more character on a line being built, joined to the span before it when the style is the same.
fn push_char(line: &mut Vec<Span<'static>>, ch: char, style: Style) {
    match line.last_mut() {
        Some(last) if last.style == style => last.content.to_mut().push(ch),
        _ => line.push(Span::styled(ch.to_string(), style)),
    }
}

enum Tok {
    Break,
    /// Characters with no space between them, which may change style part way ("**bold**,").
    Word {
        segs: Vec<(String, Style)>,
        space_before: bool,
    },
}

fn words(pieces: &[Piece]) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut cur: Vec<(String, Style)> = Vec::new();
    let mut space_before = false;
    let mut pending_space = false;
    let end = |cur: &mut Vec<(String, Style)>, out: &mut Vec<Tok>, space_before: bool| {
        if !cur.is_empty() {
            out.push(Tok::Word { segs: std::mem::take(cur), space_before });
        }
    };
    for p in pieces {
        match p {
            Piece::Break => {
                end(&mut cur, &mut out, space_before);
                out.push(Tok::Break);
                pending_space = false;
            }
            Piece::Text(text, style) => {
                for ch in text.chars() {
                    if ch.is_whitespace() {
                        if !cur.is_empty() {
                            end(&mut cur, &mut out, space_before);
                        }
                        pending_space = true;
                        continue;
                    }
                    if cur.is_empty() {
                        space_before = pending_space;
                        pending_space = false;
                    }
                    match cur.last_mut() {
                        Some((s, st)) if st == style => s.push(ch),
                        _ => cur.push((ch.to_string(), *style)),
                    }
                }
            }
        }
    }
    end(&mut cur, &mut out, space_before);
    out
}

/// A line of code cut into pieces at most `avail` columns wide, at the last space of a piece when that space is
/// in its second half (so a word is not cut in two when it need not be), else wherever the width runs out.
fn cut_code(text: &str, avail: usize) -> Vec<String> {
    if text.width() <= avail {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while !rest.is_empty() {
        let mut w = 0;
        let mut end = 0;
        while end < rest.len() {
            let cw = rest[end].width().unwrap_or(0);
            if w + cw > avail && end > 0 {
                break;
            }
            w += cw;
            end += 1;
        }
        if end < rest.len()
            && let Some(sp) = rest[..end].iter().rposition(|c| *c == ' ')
            && sp >= end / 2
            && sp > 0
        {
            end = sp + 1;
        }
        out.push(rest[..end].iter().collect::<String>().trim_end().to_string());
        rest.drain(..end);
    }
    out
}

/// No control character or escape sequence reaches the screen: they are dropped, and a tab is a space.
fn clean(s: &str) -> String {
    s.chars()
        .filter_map(|c| {
            if c == '\t' {
                Some(' ')
            } else if c.is_control() {
                None
            } else {
                Some(c)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn texts(md: &str, width: usize) -> Vec<String> {
        render(md, width, &theme(), false).iter().map(text_of).collect()
    }

    /// The style of the span holding `needle` somewhere in the output.
    fn style_of(lines: &[Line], needle: &str) -> Style {
        for l in lines {
            for s in &l.spans {
                if s.content.contains(needle) {
                    return s.style;
                }
            }
        }
        panic!("{needle:?} is not in {:?}", lines.iter().map(text_of).collect::<Vec<_>>());
    }

    #[test]
    fn headings_are_drawn_without_their_marks_and_in_three_distinct_styles() {
        let t = theme();
        let lines = render("# Summary\n\nA shop.\n\n## Pricing\n\n### Later\n", 40, &t, false);
        let shown: Vec<String> = lines.iter().map(text_of).collect();
        assert_eq!(shown, ["Summary", "", "A shop.", "", "Pricing", "", "Later"]);
        let (h1, h2, h3) = (style_of(&lines, "Summary"), style_of(&lines, "Pricing"), style_of(&lines, "Later"));
        assert!(h1.add_modifier.contains(Modifier::BOLD | Modifier::UNDERLINED) && h1.fg == Some(t.accent));
        assert!(h2.add_modifier.contains(Modifier::BOLD) && !h2.add_modifier.contains(Modifier::UNDERLINED));
        assert!(h3.add_modifier.contains(Modifier::BOLD | Modifier::ITALIC));
        assert!(h1 != h2 && h2 != h3 && h1 != h3, "three different looks, even without color");
        assert_eq!(style_of(&lines, "A shop.").fg, Some(t.text));
    }

    #[test]
    fn bold_italic_code_and_links_are_styled_and_their_marks_are_gone() {
        let t = theme();
        let lines = render("Use **Postgres**, *not* `sqlite` - see [the docs](https://example.com/x).", 80, &t, false);
        assert_eq!(lines.iter().map(text_of).collect::<Vec<_>>(), ["Use Postgres, not sqlite - see the docs."]);
        assert!(style_of(&lines, "Postgres").add_modifier.contains(Modifier::BOLD));
        assert!(!style_of(&lines, ",").add_modifier.contains(Modifier::BOLD), "the comma after it is plain");
        assert!(style_of(&lines, "not").add_modifier.contains(Modifier::ITALIC));
        assert_eq!(style_of(&lines, "sqlite"), Style::new().fg(t.ok).bg(t.surface));
        let link = style_of(&lines, "docs");
        assert!(link.add_modifier.contains(Modifier::UNDERLINED) && link.fg == Some(t.accent));
        assert!(!lines.iter().map(text_of).any(|l| l.contains("example.com")), "the address stays in the file");
    }

    #[test]
    fn bullets_wrap_with_a_hanging_indent_at_any_width() {
        let md = "- The basket page adds up the totals with VAT included for every country we sell to\n- Short one\n";
        for width in [20, 33, 50, 200] {
            let shown = texts(md, width);
            assert!(shown[0].starts_with("• The basket"), "{width}: {shown:?}");
            let cont: Vec<&String> = shown.iter().skip(1).take_while(|l| !l.starts_with('•')).collect();
            for l in &cont {
                assert!(l.starts_with("  ") && !l.starts_with("   "), "continuation lines sit under the text at {width}: {shown:?}");
            }
            assert!(shown.iter().all(|l| l.width() <= width), "{width}: {shown:?}");
            assert_eq!(shown.last().unwrap(), "• Short one");
            // Nothing is lost in the wrapping.
            let words: Vec<String> = shown.join(" ").replace('•', " ").split_whitespace().map(str::to_string).collect();
            assert_eq!(words.join(" "), "The basket page adds up the totals with VAT included for every country we sell to Short one");
        }
        assert_eq!(texts(md, 20)[..3], ["• The basket page", "  adds up the totals", "  with VAT included"]);
    }

    #[test]
    fn nested_and_numbered_lists_indent_by_depth_and_use_plain_bullets_when_asked() {
        let md =
            "- Pricing\n  - VAT in the total?\n    - per country\n- Shipping\n\n1. first\n2. second\n\nThen:\n\n10. tenth\n    goes on\n";
        assert_eq!(
            texts(md, 60),
            [
                "• Pricing",
                "  ◦ VAT in the total?",
                "    ▪ per country",
                "• Shipping",
                "",
                "1. first",
                "2. second",
                "",
                "Then:",
                "",
                "10. tenth",
                "    goes on"
            ]
        );
        let plain: Vec<String> = render(md, 60, &theme(), true).iter().map(text_of).collect();
        assert_eq!(plain[..4], ["- Pricing", "  * VAT in the total?", "    + per country", "- Shipping"]);
        assert!(plain.iter().all(|l| l.is_ascii()), "{plain:?}");
        // A wrapped numbered item hangs under its text too.
        let shown = texts("12. one two three four five six\n", 12);
        assert_eq!(shown, ["12. one two", "    three", "    four", "    five six"]);
    }

    #[test]
    fn a_line_break_inside_a_paragraph_is_kept_because_notes_are_a_fact_a_line() {
        assert_eq!(texts("Sell socks online.\nThe user's own line.\n", 80), ["Sell socks online.", "The user's own line."]);
        assert_eq!(texts("- one\n  more of one\n- two\n", 80), ["• one", "  more of one", "• two"]);
    }

    #[test]
    fn code_blocks_keep_their_lines_have_a_band_of_their_own_and_are_cut_at_a_space() {
        let t = theme();
        let md = "Run it:\n\n```sh\ncargo test --release\n\tindented\n\nlast one is long enough to be cut somewhere\n```\n\nAfter.";
        let lines = render(md, 30, &t, false);
        let shown: Vec<String> = lines.iter().map(text_of).collect();
        assert_eq!(shown[0], "Run it:");
        assert_eq!(shown[1], "");
        assert_eq!(shown[2].trim_end(), "  cargo test --release");
        assert_eq!(shown[3].trim_end(), "      indented", "a tab is four spaces");
        assert_eq!(shown[4].trim(), "", "a blank line in the code stays");
        assert_eq!(shown[5].trim_end(), "  last one is long enough to");
        assert_eq!(shown[6].trim_end(), "  be cut somewhere");
        assert_eq!(shown[8], "After.");
        for l in &lines[2..7] {
            assert_eq!(text_of(l).width(), 30, "the band runs the width of the pane: {:?}", text_of(l));
            assert!(l.spans.iter().all(|s| s.style.bg == Some(t.surface)));
        }
        // No space to cut at: cut where the width runs out.
        assert_eq!(cut_code("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        // Indented code is code too.
        assert_eq!(texts("text\n\n    let x = 1;\n", 40)[2].trim_end(), "  let x = 1;");
    }

    #[test]
    fn quotes_and_rules() {
        let shown = texts("> said the user\n> and more\n\n---\n\nafter", 20);
        assert_eq!(shown, ["│ said the user", "│ and more", "", "─".repeat(20).as_str(), "", "after"]);
        let plain: Vec<String> = render("---", 5, &theme(), true).iter().map(text_of).collect();
        assert_eq!(plain, ["-----"]);
        // A quote that wraps keeps its bar on every line.
        let shown = texts("> one two three four five", 10);
        assert!(shown.len() > 1 && shown.iter().all(|l| l.starts_with("│ ")), "{shown:?}");
    }

    #[test]
    fn wide_characters_are_measured_by_the_columns_they_take() {
        let shown = texts("- 日本語のテキストはとても長いです and more", 12);
        for l in &shown {
            assert!(l.width() <= 12, "{l:?} is {} columns: {shown:?}", l.width());
        }
        assert!(shown[0].starts_with("• 日本"), "{shown:?}");
        assert!(shown[1].starts_with("  "), "the hanging indent holds for a cut word too: {shown:?}");
        assert!(texts("🙂🙂🙂🙂", 3).iter().all(|l| l.width() <= 3));
    }

    #[test]
    fn garbage_and_edge_cases_never_panic_and_no_control_character_gets_through() {
        let nasty = [
            "",
            "\n\n\n",
            "#",
            "# ",
            "-",
            "- \n-\n*",
            "```",
            "```\nnever closed",
            "> > > deep\n> > > quote",
            "\u{1b}[31mred\u{1b}[0m and \u{7}bell\u{0}",
            "<div>raw <b>html</b></div>\n\ntext <span>inline</span>",
            "![a picture](x.png) and [](empty) and <https://x.y>",
            "| a | b |\n|---|---|\n| 1 | 2 |",
            "1) one\n2) two",
            "Setext\n===\n\nAnd\n---",
            "[^1]\n\n[^1]: a note",
            "- [ ] a box",
            &"x".repeat(5000),
            &"- ".repeat(300),
            &"> ".repeat(300),
        ];
        for md in nasty {
            for width in [0, 1, 2, 5, 40] {
                for plain in [false, true] {
                    let lines = render(md, width, &theme(), plain);
                    for l in &lines {
                        let s = text_of(l);
                        assert!(!s.chars().any(|c| c.is_control()), "{md:?} at {width}: {s:?}");
                    }
                }
            }
        }
        assert_eq!(texts("\u{1b}[31mred\u{1b}[0m", 40), ["[31mred[0m"], "the escape character itself is what matters");
        assert_eq!(texts("", 40), Vec::<String>::new());
        assert_eq!(texts("- \n", 40), ["• "], "an empty item still shows its bullet");
    }

    #[test]
    fn the_notes_a_project_really_has_read_as_they_should() {
        let md = "platoon keeps these notes up to date.\n\n# Summary\n\nA sock shop.\n\n# Open questions\n\n## Pricing\n\n- Does VAT go in the total?\n  - only for the EU?\n\n# Decisions\n\n- 2026-09-25: Use **Postgres**.\n";
        let shown = texts(md, 40);
        assert_eq!(
            shown,
            [
                "platoon keeps these notes up to date.",
                "",
                "Summary",
                "",
                "A sock shop.",
                "",
                "Open questions",
                "",
                "Pricing",
                "",
                "• Does VAT go in the total?",
                "  ◦ only for the EU?",
                "",
                "Decisions",
                "",
                "• 2026-09-25: Use Postgres.",
            ]
        );
    }
}
