use crate::app::{Action, AdoptKind, App, Confirm, Hit, Mode, SEARCH_KINDS, TABS};
use crate::dirs::tilde;
use crate::client::Client;
use crate::select::{Selection, Span as Cells};
use crate::model::{Session, Status};
use crate::settings::{Kind, Step, ITEMS};
use crate::usage::Usage;
use crate::theme::{contrast, Theme};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BAR_WIDTH: usize = 10;
/// The session's own text color is kept on a selected cell as long as it reads this well against the highlight.
const SELECTION_MIN_FG_CONTRAST: f64 = 3.0;

type Hint<'a> = (&'a str, &'a str, Option<Action>);

pub fn draw(f: &mut Frame, app: &App) {
    let t = app.effective_theme();
    app.hits.borrow_mut().clear();
    f.render_widget(Block::default().style(Style::new().bg(t.bg)), f.area());

    let frame = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.border))
        .style(Style::new().bg(t.bg));
    let inner = frame.inner(f.area());
    f.render_widget(frame, f.area());

    if let (Mode::Attached(pid), Some(client)) = (&app.mode, &app.client) {
        let dir = app.hosts.iter().find(|h| h.claude_pid == *pid).map(|h| h.dir.as_str()).unwrap_or("");
        draw_attached(f, app, client, dir, inner);
        return;
    }

    let [header, rule, cols, list, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner.inner(Margin::new(2, 0)));

    let tabs = draw_header(f, app, header);
    draw_tab_rule(f, t, rule, &tabs);
    draw_columns(f, t, cols);
    let visible = app.visible();
    if visible.is_empty() {
        let tab = &TABS[app.tab.min(TABS.len() - 1)];
        let msg = if app.counts[app.tab.min(TABS.len() - 1)] > 0 {
            "No sessions match the filter"
        } else if app.indexing().is_some() {
            "Reading your conversations…"
        } else {
            tab.empty
        };
        let p = Paragraph::new(msg).style(Style::new().fg(t.dim)).alignment(Alignment::Center);
        f.render_widget(p, Rect::new(list.x, list.y + list.height / 3, list.width, 1));
    } else {
        draw_list(f, app, visible, list);
    }
    let tab_keys = format!("1-{}", TABS.len());
    let archive_verb = if app.selected_session().is_some_and(|s| s.archived) { "restore" } else { "archive" };
    let hints: Vec<Hint> = match app.mode {
        Mode::Search => vec![("enter", "apply", Some(Action::Confirm)), ("esc", "clear", Some(Action::Cancel))],
        _ => vec![
            ("enter", "open", Some(Action::Jump)),
            ("n", "new", Some(Action::New)),
            ("a", archive_verb, Some(Action::Archive)),
            ("/", "search", Some(Action::Search)),
            ("r", "rename", Some(Action::Rename)),
            (&tab_keys, "tabs", None),
            ("↑↓", "move", None),
            ("s", "settings", Some(Action::Settings)),
            ("q", "quit", Some(Action::Quit)),
        ],
    };
    // The mouse and the obvious keys need no reminder: when the line is short of room (a narrow terminal, or a
    // notice sharing it) those hints go first.
    let notice = app.active_notice();
    let room = (footer.width as usize).saturating_sub(notice.map_or(0, |n| n.chars().count() + 6));
    let hints = fit_hints(hints, room, &["move", "tabs"]);
    let hints = hints.as_slice();
    match notice {
        // A message that can't share the line with the key hints gets the whole line rather than being cut off.
        Some(n) if hints_width(hints) > room => {
            // It may use the footer's side margin too: a notice that says something happened shouldn't be cut off for want of a column.
            let wide = Rect::new(footer.x.saturating_sub(1), footer.y, footer.width + 2, 1);
            f.render_widget(Paragraph::new(Span::styled(fit_end(n, wide.width as usize), Style::new().fg(t.warn))), wide);
        }
        notice => draw_footer(f, app, footer, hints, notice.map(|n| (n.to_string(), t.warn))),
    }

    match &app.mode {
        Mode::Picker => draw_picker(f, app, inner),
        Mode::Confirm(c) => draw_confirm(f, app, c, inner),
        Mode::Settings => draw_settings_modal(f, app, inner),
        Mode::Search => draw_search(f, app, inner),
        Mode::Rename => draw_rename(f, app, inner),
        // No fade here: the point is to see the real screen in the colors being tried.
        Mode::Themes => draw_theme_list(f, app, inner),
        _ => {}
    }
}

/// Where a tab was drawn: (x, width, is the current tab). The rule under the header follows these.
type TabSpan = (u16, u16, bool);

/// The line under the header: a quiet rule, with the current tab underlined in the accent color.
fn draw_tab_rule(f: &mut Frame, t: &Theme, area: Rect, tabs: &[TabSpan]) {
    f.render_widget(Paragraph::new(Span::styled("─".repeat(area.width as usize), Style::new().fg(t.border))), area);
    // A heavy accent segment over it, under the current tab.
    for &(x, w, current) in tabs {
        if current {
            let seg = Rect::new(x, area.y, w, 1).intersection(area);
            if !seg.is_empty() {
                f.render_widget(Paragraph::new(Span::styled("━".repeat(seg.width as usize), Style::new().fg(t.accent))), seg);
            }
        }
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) -> Vec<TabSpan> {
    let t = app.effective_theme();
    let unread = app.unread_count();
    let mut left = vec![Span::styled("platoon", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), Span::raw("   ")];
    let mut x = area.x + 10;
    let mut spans = Vec::new();
    for (i, tab) in TABS.iter().enumerate() {
        if i > 0 {
            left.push(Span::raw(" "));
            x += 1;
        }
        // A chip: ` 1 Active ` with the count in a badge after a little air. Round caps where the terminal has the
        // glyphs, otherwise a padded square. Either way the text reads "1 Active 3".
        let current = i == app.tab;
        let (key, label) = (format!("{}", i + 1), format!(" {} ", tab.label));
        let w = chip_width(&key, &label, app.counts[i]);
        let rect = Rect::new(x, area.y, w, 1).intersection(area);
        if !rect.is_empty() {
            app.hits.borrow_mut().push((rect, Hit::Tab(i)));
        }
        spans.push((x, w, current));
        x += w;
        let hover = !current && rect.contains(app.mouse.into());
        left.extend(tab_chip(t, app.rounded, &key, &label, app.counts[i], current, hover));
    }
    if unread > 0 {
        left.push(Span::styled(format!("   ● {unread} unread"), Style::new().fg(t.ok)));
    }
    if !app.filter.is_empty() {
        left.push(Span::styled(format!("   filter: {}", app.filter), Style::new().fg(t.accent)));
        left.push(Span::styled("  (esc clears)", Style::new().fg(t.dim)));
    }
    if let Some(n) = app.indexing() {
        let spin = SPINNER[(app.tick % 10) as usize];
        let text = if n > 0 { format!("   {spin} indexing {n}…") } else { format!("   {spin} indexing…") };
        left.push(Span::styled(text, Style::new().fg(t.dim)));
    }

    let mut right = Vec::new();
    let dim = Style::new().fg(t.dim);
    match &app.usage {
        Usage::Ready(u) => {
            for (i, (label, w)) in [("5h", u.five_hour), ("7d", u.seven_day)].into_iter().enumerate() {
                right.push(Span::styled(if i == 0 { format!("{label} ") } else { format!("   {label} ") }, dim));
                match w {
                    Some(w) => right.extend(bar(t, w.pct)),
                    None => right.push(Span::styled("n/a", dim)),
                }
            }
        }
        Usage::Waiting => right.push(Span::styled("plan usage: waiting for a session to report", dim)),
        Usage::NotSetUp => right.push(Span::styled("plan usage: off · run `platoon setup`", dim)),
    }

    let width = |spans: &[Span]| spans.iter().map(|s| s.content.chars().count()).sum::<usize>();
    // The tabs matter most. When the usage readout doesn't fit beside them it shrinks to bare percentages, then goes.
    let room = (area.width as usize).saturating_sub(width(&left) + 2);
    if width(&right) > room {
        right = match &app.usage {
            Usage::Ready(u) => {
                let mut compact = Vec::new();
                for (label, w) in [("5h", u.five_hour), ("7d", u.seven_day)] {
                    let pct = w.map_or("n/a".to_string(), |w| format!("{}%", w.pct));
                    compact.push(Span::styled(format!("{}{label} ", if compact.is_empty() { "" } else { "  " }), dim));
                    compact.push(Span::styled(pct, Style::new().fg(t.subtext)));
                }
                compact
            }
            _ => Vec::new(),
        };
        if width(&right) > room {
            right.clear();
        }
    }
    let right_w = width(&right) as u16;
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(right_w)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(left)), l);
    f.render_widget(Paragraph::new(Line::from(right)), r);
    spans
}

/// Width of `tab_chip`: a space, the key, the label, and the count (in a padded square, or between round caps).
fn chip_width(key: &str, label: &str, count: usize) -> u16 {
    // Round caps or padding: two cells either way.
    (1 + key.chars().count() + label.chars().count() + count.to_string().chars().count() + 2) as u16
}

/// One tab of the header, ` 1 Active 3 `: the current tab is lit, a hovered one half lit, the rest quiet.
fn tab_chip(t: &Theme, rounded: bool, key: &str, label: &str, count: usize, current: bool, hover: bool) -> Vec<Span<'static>> {
    let count = if rounded { format!("{count}") } else { format!(" {count} ") };
    let chip = if current || hover { t.surface } else { t.bg };
    let (k, l, badge) = if current {
        (Style::new().fg(t.accent), Style::new().fg(t.text).add_modifier(Modifier::BOLD), Style::new().fg(t.on_accent).bg(t.accent).add_modifier(Modifier::BOLD))
    } else if hover {
        (Style::new().fg(t.accent), Style::new().fg(t.text), Style::new().fg(t.text).bg(t.border))
    } else {
        (Style::new().fg(t.dim), Style::new().fg(t.dim), Style::new().fg(t.dim).bg(t.surface))
    };
    let mut out = vec![Span::styled(" ", Style::new().bg(chip)), Span::styled(key.to_string(), k.bg(chip)), Span::styled(label.to_string(), l.bg(chip))];
    if rounded {
        // The caps are drawn in the badge color on the color beside them, so the block reads as a pill.
        let pill = badge.bg.unwrap_or(t.surface);
        out.push(Span::styled("\u{e0b6}", Style::new().fg(pill).bg(chip)));
        out.push(Span::styled(count, badge));
        out.push(Span::styled("\u{e0b4}", Style::new().fg(pill).bg(t.bg)));
    } else {
        out.push(Span::styled(count, badge));
    }
    out
}

struct Cols {
    name: usize,
    dir: usize,
    branch: usize,
    model: usize,
    status: usize,
}

/// Width of the "last active" column, right-aligned, at the end of the row.
const LAST_W: usize = 6;

fn cols(width: u16) -> Cols {
    // marker(3) + name + dir + branch + model + status + context bar(BAR_WIDTH + 5) + last active
    let fixed = 3 + BAR_WIDTH + 5 + LAST_W;
    let w = width as usize;
    let branch = if w >= 90 { 16 } else { 0 };
    let model = if w >= 100 { 12 } else { 0 };
    let status = 11; // room for "Background" and a space
    let flex = w.saturating_sub(fixed + branch + model + status);
    Cols { name: flex * 2 / 5, dir: flex - flex * 2 / 5, branch, model, status }
}

fn draw_columns(f: &mut Frame, t: &Theme, area: Rect) {
    let c = cols(area.width);
    let s = Style::new().fg(t.dim);
    let line = Line::from(vec![
        Span::raw("   "),
        Span::styled(fit("NAME", c.name), s),
        Span::styled(fit("DIR", c.dir), s),
        Span::styled(fit(if c.branch > 0 { "BRANCH" } else { "" }, c.branch), s),
        Span::styled(fit(if c.model > 0 { "MODEL" } else { "" }, c.model), s),
        Span::styled(fit("STATUS", c.status), s),
        Span::styled(fit("CONTEXT", BAR_WIDTH + 5), s),
        Span::styled(format!("{:>w$}", "LAST", w = LAST_W - 1), s),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_list(f: &mut Frame, app: &App, visible: &[usize], area: Rect) {
    const ROW_H: u16 = 3; // two text lines + one spacer
    let rows = (area.height / ROW_H) as usize;
    let now = crate::usage::now() * 1000;
    // Keep the selection in view.
    let start = app.selected.saturating_sub(rows.saturating_sub(1));
    for (i, &idx) in visible.iter().enumerate().skip(start).take(rows) {
        let y = area.y + (i - start) as u16 * ROW_H;
        let row = Rect::new(area.x, y, area.width, ROW_H.min(area.bottom() - y));
        app.hits.borrow_mut().push((row, Hit::Row(i)));
        let hover = row.contains(app.mouse.into());
        draw_row(f, app, &app.sessions[idx], i == app.selected, hover, now, row);
    }
}

/// `3m`, `5h`, `2d`, `3mo`, `1y`: how long ago, at a glance. `-` when unknown.
pub fn ago(now_ms: u64, then_ms: u64) -> String {
    if then_ms == 0 {
        return "-".into();
    }
    let s = now_ms.saturating_sub(then_ms) / 1000;
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        86_400..=5_183_999 => format!("{}d", s / 86_400),
        5_184_000..=31_535_999 => format!("{}mo", s / 2_592_000),
        _ => format!("{}y", s / 31_536_000),
    }
}

/// How a row shows what its conversation is doing: the marker before the name and the status word.
struct Look {
    marker: &'static str,
    marker_color: Color,
    status: &'static str,
    status_color: Color,
}

fn row_look(t: &Theme, status: Status, unread: bool, tick: u64) -> Look {
    // Unread pulses between two shades about once a second.
    let pulse_on = (tick / 5) % 2 == 0;
    match (status, unread) {
        (Status::Busy, _) => Look { marker: SPINNER[(tick % 10) as usize], marker_color: t.busy, status: "working", status_color: t.busy },
        (Status::Idle, true) => Look { marker: "●", marker_color: if pulse_on { t.ok } else { t.dim }, status: "done", status_color: t.ok },
        (Status::Idle, false) => Look { marker: "○", marker_color: t.dim, status: "idle", status_color: t.dim },
        // The accent: every theme keeps it apart from the working, done and warning colors (`MIN_DELTA_E`) and
        // readable on the background (`MIN_ROLE`), so a running-in-the-background row can't be mistaken for those.
        (Status::Background, _) => Look { marker: "◆", marker_color: t.accent, status: "Background", status_color: t.accent },
    }
}

fn draw_row(f: &mut Frame, app: &App, s: &Session, selected: bool, hover: bool, now: u64, area: Rect) {
    let t = app.effective_theme();
    let c = cols(area.width);
    let bg = if selected || hover { t.surface } else { t.bg };
    let base = Style::new().bg(bg);
    // Archived rows are dimmed so the tab reads as "put away" at a glance.
    let text = if s.archived { t.dim } else { t.text };
    let subtext = if s.archived { t.dim } else { t.subtext };

    let Look { marker, marker_color, status: status_text, status_color } = row_look(t, s.status, s.unread, app.tick);
    let name_style = if s.unread {
        base.fg(t.ok).add_modifier(Modifier::BOLD)
    } else {
        base.fg(text)
    };

    let lead = if selected { Span::styled("▎", base.fg(t.accent)) } else { Span::styled(" ", base) };
    let mut l1 = vec![
        lead.clone(),
        Span::styled(format!("{marker} "), base.fg(marker_color)),
        Span::styled(fit(&s.name, c.name), name_style),
        Span::styled(fit(&s.dir, c.dir), base.fg(subtext)),
        Span::styled(fit(s.branch.as_deref().unwrap_or("-"), c.branch), base.fg(t.dim)),
        Span::styled(fit(s.model.as_deref().unwrap_or("-"), c.model), base.fg(subtext)),
        Span::styled(fit(status_text, c.status), base.fg(status_color)),
    ];
    l1.extend(bar_bg(t, s.context_pct, bg, s.context_exact, s.archived));
    l1.push(Span::styled(format!("{:>w$}", ago(now, s.last_active), w = LAST_W - 1), base.fg(t.dim)));

    let desc_w = (area.width as usize).saturating_sub(3);
    let l2 = vec![
        lead,
        Span::styled("  ", base),
        Span::styled(fit(&s.description, desc_w.saturating_sub(1)), base.fg(t.dim)),
    ];

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)]).split(area);
    f.render_widget(Paragraph::new(Line::from(l1)).style(base), rows[0]);
    f.render_widget(Paragraph::new(Line::from(l2)).style(base), rows[1]);
    if rows[2].height > 0 {
        f.render_widget(Block::default().style(Style::new().bg(t.bg)), rows[2]);
    }
}

fn hints_width(hints: &[Hint<'_>]) -> usize {
    let words: usize = hints.iter().map(|(k, d, _)| k.chars().count() + 1 + d.chars().count()).sum();
    words + 3 * hints.len().saturating_sub(1)
}

/// Drop the hints described by `droppable`, in that order, until the rest fit in `width`.
fn fit_hints<'a>(mut hints: Vec<Hint<'a>>, width: usize, droppable: &[&str]) -> Vec<Hint<'a>> {
    for d in droppable {
        if hints_width(&hints) <= width {
            break;
        }
        hints.retain(|h| h.1 != *d);
    }
    hints
}

/// Key hints along the bottom. Hints with an action are clickable and highlight on hover.
fn draw_footer(f: &mut Frame, app: &App, area: Rect, hints: &[Hint<'_>], right: Option<(String, Color)>) {
    let t = app.effective_theme();
    let mut spans = Vec::new();
    let mut x = area.x;
    for (i, (k, d, action)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
            x += 3;
        }
        let w = (k.chars().count() + 1 + d.chars().count()) as u16;
        let rect = Rect::new(x, area.y, w, 1);
        let hover = action.is_some() && rect.contains(app.mouse.into());
        if let Some(a) = action {
            app.hits.borrow_mut().push((rect, Hit::Action(*a)));
        }
        let bg = if hover { t.surface } else { t.bg };
        let label = if hover { t.text } else { t.dim };
        spans.push(Span::styled(*k, Style::new().fg(t.accent).bg(bg)));
        spans.push(Span::styled(format!(" {d}"), Style::new().fg(label).bg(bg)));
        x += w;
    }
    let used = x - area.x;
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    if let Some((msg, color)) = right {
        let avail = area.width.saturating_sub(used + 3) as usize;
        if avail > 8 {
            let msg = fit_end(&msg, avail);
            let w = msg.chars().count() as u16;
            let r = Rect::new(area.right() - w, area.y, w, 1);
            f.render_widget(Paragraph::new(Span::styled(msg, Style::new().fg(color))), r);
        }
    }
}

/// Break `text` into lines of at most `width` chars at word boundaries (a word longer than a line is split).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word;
        while word.chars().count() > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let cut = word.char_indices().nth(width).map_or(word.len(), |(i, _)| i);
            lines.push(word[..cut].to_string());
            word = &word[cut..];
        }
        let len = line.chars().count();
        if len > 0 && len + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Keep the end of `s` (the file name of a path), with an ellipsis in front, when it is longer than `n`.
fn fit_start(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len <= n {
        return s.to_string();
    }
    let tail: String = s.chars().skip(len - n.saturating_sub(1)).collect();
    format!("…{tail}")
}

/// The settings screen: one entry per `ITEMS`, each with its explanation and a value that steps with `‹` `›`.
fn draw_settings(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let w = area.width as usize;
    let store = app.settings();
    let current = store.get();

    // What goes under the entries, worked out first because its height decides how much room they get.
    let mut notes = settings_notes(app, w);
    let label = "Settings file: ";
    let file = tilde(&store.path().to_string_lossy(), &app.home);
    let hints = fit_hints(
        vec![("↑↓", "move", None), ("←→", "change", None), ("enter", "next choice", None), ("esc", "back", Some(Action::Cancel))],
        w,
        &["move"],
    );

    // The file and the key hints sit at the bottom, always. The entries get what is left after the notes.
    let hints_y = area.bottom().saturating_sub(1).max(area.y);
    let path_y = hints_y.saturating_sub(1).max(area.y);
    let top = area.y;
    // The reassurance line is the first thing to go when that lets the selected entry explain itself.
    // A warning or an error is never dropped.
    if !store.is_corrupt() && app.active_notice().is_none() {
        let text_w = (area.width as usize).saturating_sub(4);
        let explained: Vec<Vec<String>> = ITEMS.iter().map(|it| wrap(it.explain, text_w)).collect();
        let room = |notes: usize| path_y.saturating_sub(notes as u16 + 1).max(top + ITEMS.len() as u16).min(path_y.max(top)) - top;
        if explain_level(&explained, app.setting, room(notes.len()) as usize) == 2 && explain_level(&explained, app.setting, room(0) as usize) < 2 {
            notes.clear();
        }
    }
    // The entries come before the notes: on a very short terminal the notes give way.
    let rows_bottom = path_y.saturating_sub(notes.len() as u16 + 1).max(top + ITEMS.len() as u16).min(path_y.max(top));
    let rows_area = Rect::new(area.x, top, area.width, rows_bottom - top);

    // The notes follow the entries directly.
    let mut y = draw_setting_rows(f, app, &current, rows_area) + 1;
    for (line, color) in &notes {
        if y < path_y {
            f.render_widget(Paragraph::new(Span::styled(line.clone(), Style::new().fg(*color))), Rect::new(area.x, y, area.width, 1));
        }
        y += 1;
    }
    let path = Line::from(vec![
        Span::styled(label, Style::new().fg(t.dim)),
        Span::styled(fit_start(&file, w.saturating_sub(label.len())), Style::new().fg(t.subtext)),
    ]);
    f.render_widget(Paragraph::new(path), Rect::new(area.x, path_y, area.width, 1));
    draw_footer(f, app, Rect::new(area.x, hints_y, area.width, 1), &hints, None);
}

/// What goes under the settings entries: a warning if the file was unreadable, then the latest notice or
/// the reassurance that changes apply at once.
fn settings_notes(app: &App, w: usize) -> Vec<(String, Color)> {
    let t = app.effective_theme();
    let mut notes: Vec<(String, Color)> = Vec::new();
    if app.settings().is_corrupt() {
        let msg = "The settings file couldn't be read, so the standard values are shown. Your first change saves a fresh file and keeps the old one as settings.json.bak.";
        notes.extend(wrap(msg, w).into_iter().map(|l| (l, t.warn)));
    }
    match app.active_notice() {
        Some(n) => notes.extend(wrap(n, w).into_iter().map(|l| (l, t.warn))),
        None => notes.push(("Changes are saved and apply immediately.".into(), t.dim)),
    }
    notes
}

/// Rows the popup's contents need when nothing has to be left out: every entry explained, the notes, the file
/// path and the key hints.
fn settings_content_height(app: &App, w: usize) -> u16 {
    let text_w = w.saturating_sub(4);
    let entries: usize = ITEMS.iter().map(|it| 1 + wrap(it.explain, text_w).len()).sum::<usize>() + ITEMS.len().saturating_sub(1);
    (entries + 1 + settings_notes(app, w).len() + 1 + 2) as u16
}

/// Settings float over the list, like the other dialogs: the list fades behind, the popup is sized to its contents,
/// and a click outside it closes it.
fn draw_settings_modal(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    // Everything behind reads as inactive: same layout, muted colors.
    f.buffer_mut().set_style(area, Style::new().fg(t.border).remove_modifier(Modifier::BOLD | Modifier::UNDERLINED));
    app.hits.borrow_mut().clear();

    // Floating with room around it on ordinary terminals; nearly the full width on a narrow one, where every column counts.
    let w = if area.width < 70 { area.width.saturating_sub(2) } else { (area.width - 4).min(76) };
    // A margin of 2 each side inside the borders; 1 when the terminal is narrow, so entry names aren't clipped.
    let hpad = if w < 64 { 1 } else { 2 };
    let inner_w = w.saturating_sub(2 + 2 * hpad) as usize;
    let need = settings_content_height(app, inner_w) + 2; // plus the borders
    // A row of air above and below when the terminal can spare it; on a short one the popup uses every row it can.
    let vpad = u16::from(need + 2 <= area.height);
    let r = centered(area, w, (need + 2 * vpad).min(area.height).max(1));
    f.render_widget(Clear, r);
    let block = modal_block(t, "Settings");
    let inner = block.inner(r).inner(Margin::new(hpad, vpad));
    f.render_widget(block, r);
    draw_settings(f, app, inner);
    // Last, so it only catches clicks the popup didn't: outside it, a click closes settings.
    app.hits.borrow_mut().push((area, Hit::Action(Action::Cancel)));
}

/// As much explanation as fits in `height` rows: every entry explained with air between them (0), else only
/// the selected one (1), else none (2, a terminal a handful of lines tall).
fn explain_level(explained: &[Vec<String>], selected: usize, height: usize) -> usize {
    (0..2)
        .find(|&level| {
            let gap = usize::from(level == 0);
            let total: usize = (0..explained.len()).map(|i| 1 + if level == 0 || i == selected { explained[i].len() } else { 0 }).sum();
            total + gap * (explained.len() - 1) <= height
        })
        .unwrap_or(2)
}

/// Draws the entries and returns the row below the last one.
fn draw_setting_rows(f: &mut Frame, app: &App, current: &crate::settings::Settings, area: Rect) -> u16 {
    let t = app.effective_theme();
    if area.is_empty() {
        return area.y;
    }
    let text_w = (area.width as usize).saturating_sub(4);
    let explained: Vec<Vec<String>> = ITEMS.iter().map(|it| wrap(it.explain, text_w)).collect();

    let level = explain_level(&explained, app.setting, area.height as usize);
    let show = |i: usize| level == 0 || (level == 1 && i == app.setting);
    let height = |i: usize| 1 + if show(i) { explained[i].len() } else { 0 };
    let gap = u16::from(level == 0);

    // Scroll only if even that doesn't fit: keep the selected entry in view.
    let mut start = 0;
    while start < app.setting && (start..=app.setting).map(|i| height(i) + gap as usize).sum::<usize>() > area.height as usize + gap as usize {
        start += 1;
    }

    let mut y = area.y;
    for (i, item) in ITEMS.iter().enumerate().skip(start) {
        let h = height(i) as u16;
        if y + h > area.bottom() {
            break;
        }
        let row = Rect::new(area.x, y, area.width, h);
        let selected = i == app.setting;
        let hover = row.contains(app.mouse.into());
        let bg = if selected || hover { t.surface } else { t.bg };
        f.render_widget(Block::default().style(Style::new().bg(bg)), row);

        // The control is `‹ value ›` at the right end, the value centered in a width that fits every choice so
        // the buttons stay put as it changes. Worked out here: the text of the value, that width, whether it can
        // go no further either way, and whether it means "switched off".
        let (label, vw, at_start, at_end, off) = match item.kind {
            Kind::Steps => {
                let value = (item.get)(current);
                let label = (item.label)(value);
                let vw = item.choices.iter().map(|&c| (item.label)(c).chars().count()).chain([label.chars().count()]).max().unwrap_or(0);
                (label, vw, item.step(value, Step::Prev) == value, item.step(value, Step::Next) == value, item.off == Some(value))
            }
            Kind::Theme => {
                let all = app.themes().all();
                let at = all.iter().position(|t| t.id == app.theme.id).unwrap_or(0);
                let vw = all.iter().map(|t| t.name.chars().count()).max().unwrap_or(0).min(24);
                (fit_end(&app.theme.name, vw), vw, at == 0, at + 1 >= all.len(), false)
            }
        };
        let ctrl_w = 2 + vw + 2;
        let cx = (row.right() as usize).saturating_sub(ctrl_w + 1).max(row.x as usize + 2) as u16;
        let button = |x: u16| Rect::new(x, row.y, 2, 1);
        let (prev, val, next) = (button(cx), Rect::new(cx + 2, row.y, vw as u16, 1), button(cx + 2 + vw as u16));
        let mut hits = app.hits.borrow_mut();
        // Buttons first: the first region under the pointer wins, and the row covers them.
        hits.push((prev, Hit::SettingStep(i, Step::Prev)));
        hits.push((next, Hit::SettingStep(i, Step::Next)));
        hits.push((val, Hit::SettingStep(i, Step::Cycle)));
        hits.push((row, Hit::Setting(i)));
        drop(hits);

        let arrow = |symbol: &'static str, rect: Rect, disabled: bool| {
            let hot = !disabled && rect.contains(app.mouse.into());
            let (fg, cell_bg) = if disabled { (t.border, bg) } else if hot { (t.text, t.border) } else { (t.accent, bg) };
            Span::styled(symbol, Style::new().fg(fg).bg(cell_bg))
        };
        // "Off" reads as switched off: muted, where an active value is bold and colored.
        let value_style = if off { Style::new().fg(t.dim) } else { Style::new().fg(t.accent).add_modifier(Modifier::BOLD) };
        let hot_value = if val.contains(app.mouse.into()) { Style::new().bg(t.border) } else { Style::new().bg(bg) };
        let control = Line::from(vec![
            arrow("‹ ", prev, at_start),
            Span::styled(format!("{label:^vw$}"), value_style.patch(hot_value)),
            arrow(" ›", next, at_end),
        ]);
        f.render_widget(Paragraph::new(control), Rect::new(cx, row.y, ctrl_w as u16, 1));

        let name_w = (cx as usize).saturating_sub(row.x as usize + 3);
        let lead = Span::styled(if selected { "▎ " } else { "  " }, Style::new().fg(t.accent).bg(bg));
        let name = Span::styled(fit_end(item.name, name_w), Style::new().fg(t.text).bg(bg).add_modifier(Modifier::BOLD));
        f.render_widget(Paragraph::new(Line::from(vec![lead, name])), Rect::new(row.x, row.y, (cx - row.x).max(1), 1));
        if show(i) {
            for (n, line) in explained[i].iter().enumerate() {
                let r = Rect::new(row.x + 2, row.y + 1 + n as u16, row.width.saturating_sub(4), 1);
                f.render_widget(Paragraph::new(Span::styled(line.clone(), Style::new().fg(t.subtext).bg(bg))), r);
            }
        }
        y += h + gap;
    }
    y.saturating_sub(gap)
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

fn modal_block<'a>(t: &Theme, title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.accent))
        .title(Span::styled(format!(" {title} "), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))
        .style(Style::new().bg(t.bg))
}

/// The theme list: a panel docked to the right of the screen, so the header, the tabs and the left of the list
/// stay in view in the colors being tried, and never a box across the middle. Nothing behind it is faded. It
/// holds the entries (name and a strip of accent / busy / ok / warn / err, a check on the saved one), a note
/// about theme files that were skipped, a small built-in sample that shows every color role whatever the list
/// behind holds (even nothing at all), and the keys.
fn draw_theme_list(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let pick = &app.tpick;
    // The panel owns the clicks it covers; a click anywhere else (on the real screen) puts the old theme back.
    app.hits.borrow_mut().clear();

    let pw = (area.width * 47 / 100).clamp(34, 42).min(area.width);
    let gap = u16::from(area.width > pw + 12);
    let panel = Rect::new(area.right() - pw - gap, area.y, pw, area.height);
    f.render_widget(Clear, panel);
    let block = modal_block(t, "Theme");
    let inner = block.inner(panel).inner(Margin::new(1, 0));
    f.render_widget(block, panel);
    let w = inner.width as usize;

    // Heights, in order of importance: the keys, three entries, the sample (a rule and up to seven lines), the note
    // about skipped files, and then everything left over goes to the entries.
    let hints_h = u16::from(inner.height >= 2);
    let mut left = inner.height.saturating_sub(hints_h);
    let list_min = (pick.list.len() as u16).min(3).min(left);
    left -= list_min;
    let sample_lines = left.saturating_sub(1).min(SAMPLE_LINES as u16);
    let sample_h = if sample_lines > 0 { sample_lines + 1 } else { 0 };
    left -= sample_h;
    let scan = app.themes().skipped();
    let note = if scan.skipped_count() == 0 {
        Vec::new()
    } else {
        let n = scan.skipped_count();
        let text = format!("{n} theme file{} skipped. {}", if n == 1 { "" } else { "s" }, scan.reasons().join("; "));
        let mut lines = wrap(&text, w);
        if lines.len() > 3 {
            lines.truncate(3);
            let last = lines.pop().unwrap_or_default();
            lines.push(fit_end(&format!("{last}…"), w));
        }
        lines
    };
    let note_h = (note.len() as u16).min(left);
    let list_h = inner.height - hints_h - sample_h - note_h;

    let list = Rect::new(inner.x, inner.y, inner.width, list_h);
    let note_area = Rect::new(inner.x, list.bottom(), inner.width, note_h);
    let sample = Rect::new(inner.x, note_area.bottom(), inner.width, sample_h);
    let foot = Rect::new(inner.x, inner.bottom() - hints_h, inner.width, hints_h);

    // The entries, scrolled only as far as it takes to keep the highlighted one in view.
    let rows = list_h as usize;
    let mut top = pick.top.get().min(pick.list.len().saturating_sub(rows));
    if pick.selected < top {
        top = pick.selected;
    } else if rows > 0 && pick.selected >= top + rows {
        top = pick.selected + 1 - rows;
    }
    pick.top.set(top);
    for (k, th) in pick.list.iter().enumerate().skip(top).take(rows) {
        let rect = Rect::new(list.x, list.y + (k - top) as u16, list.width, 1);
        app.hits.borrow_mut().push((rect, Hit::ThemeRow(k)));
        let selected = k == pick.selected;
        let bg = if selected || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        // marker(2) name space swatch(5) check(2)
        let name_w = w.saturating_sub(10);
        let tag = if th.is_user() && !th.name.ends_with("(yours)") && name_w >= 14 { " yours" } else { "" };
        let name_style = if selected { Style::new().fg(t.text).add_modifier(Modifier::BOLD) } else { Style::new().fg(t.subtext) };
        let mut spans = vec![
            Span::styled(if selected { "▎ " } else { "  " }, Style::new().fg(t.accent).bg(bg)),
            Span::styled(fit(&th.name, name_w - tag.len().min(name_w)), name_style.bg(bg)),
            Span::styled(tag, Style::new().fg(t.dim).bg(bg)),
            Span::styled(" ", Style::new().bg(bg)),
        ];
        // Each entry's own colors, whatever theme is being shown.
        for c in [th.accent, th.busy, th.ok, th.warn, th.err] {
            spans.push(Span::styled("█", Style::new().fg(c).bg(bg)));
        }
        spans.push(Span::styled(if th.id == app.theme.id { " ✓" } else { "  " }, Style::new().fg(t.accent).bg(bg)));
        f.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(bg)), rect);
    }
    for (n, line) in note.iter().enumerate() {
        f.render_widget(Paragraph::new(Span::styled(line.clone(), Style::new().fg(t.dim))), Rect::new(note_area.x, note_area.y + n as u16, note_area.width, 1));
    }
    if !sample.is_empty() {
        draw_theme_sample(f, app, sample);
    }
    if !foot.is_empty() {
        let hints = fit_hints(
            vec![("↑↓", "preview", None), ("enter", "use", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel))],
            w,
            &["preview"],
        );
        draw_footer(f, app, foot, &hints, None);
    }
    let mut hits = app.hits.borrow_mut();
    hits.push((panel, Hit::Inert));
    hits.push((area, Hit::Action(Action::Cancel)));
}

/// Lines of the sample, without its rule.
const SAMPLE_LINES: usize = 7;

/// A small stand-in for the main screen, drawn with the same pieces (tab chip, row markers and status words,
/// context bar, usage bar, key hints) so that every color role shows however few or many conversations the real
/// list has: text, subtext, dim, accent, busy, ok, warn and err on the background, the surface of a selected row
/// and the border of a bar's track.
fn draw_theme_sample(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let w = area.width as usize;
    let rule = " sample ";
    let dashes = w.saturating_sub(1 + rule.len());
    let head = Line::from(vec![
        Span::styled("─", Style::new().fg(t.border)),
        Span::styled(rule, Style::new().fg(t.dim)),
        Span::styled("─".repeat(dashes), Style::new().fg(t.border)),
    ]);
    f.render_widget(Paragraph::new(head), Rect::new(area.x, area.y, area.width, 1));

    let row = |selected: bool, status: Status, unread: bool, name: &str, pct: u8| -> Line<'static> {
        let bg = if selected { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let look = row_look(t, status, unread, app.tick);
        // The status word goes first when the panel is narrow.
        let status_w = if w >= 36 { 8 } else { 0 };
        let name_w = w.saturating_sub(3 + status_w + BAR_WIDTH + 5);
        let name_style = if unread { base.fg(t.ok).add_modifier(Modifier::BOLD) } else { base.fg(t.text) };
        let mut spans = vec![
            if selected { Span::styled("▎", base.fg(t.accent)) } else { Span::styled(" ", base) },
            Span::styled(format!("{} ", look.marker), base.fg(look.marker_color)),
            Span::styled(fit(name, name_w), name_style),
        ];
        if status_w > 0 {
            spans.push(Span::styled(fit(look.status, status_w), base.fg(look.status_color)));
        }
        spans.extend(bar_bg(t, pct, bg, true, false));
        Line::from(spans)
    };
    let chips = {
        let mut spans = tab_chip(t, app.rounded, "1", " Active ", 3, true, false);
        let second = tab_chip(t, app.rounded, "2", " Archived ", 12, false, false);
        let width = |spans: &[Span]| spans.iter().map(|s| s.content.chars().count()).sum::<usize>();
        if width(&spans) + 1 + width(&second) <= w {
            spans.push(Span::raw(" "));
            spans.extend(second);
        }
        Line::from(spans)
    };
    let usage = {
        let dim = Style::new().fg(t.dim);
        let mut spans = vec![Span::styled("5h ", dim)];
        spans.extend(bar(t, 47));
        if w >= 3 + BAR_WIDTH + 5 + 9 {
            spans.push(Span::styled("   7d ", dim));
            spans.push(Span::styled("21%", Style::new().fg(t.subtext)));
        }
        Line::from(spans)
    };
    let hints: Vec<Hint> = fit_hints(vec![("enter", "open", None), ("n", "new", None), ("s", "settings", None)], w, &["settings", "new"]);
    let notice = Line::from(Span::styled(fit_end("Archived \"docs-site\"", w), Style::new().fg(t.warn)));

    // Every line the sample can have, in order. When there isn't room for all of them the least telling go first:
    // the usage bars, the hints, the notice, the tab chips, and then the rows from the bottom up.
    let mut lines: Vec<Option<Line>> = vec![
        Some(chips),
        Some(row(true, Status::Busy, false, "api-server", 40)),
        Some(row(false, Status::Idle, true, "docs-site", 70)),
        Some(row(false, Status::Idle, false, "notes", 92)),
        Some(usage),
        None, // the hints, drawn by `draw_footer`
        Some(notice),
    ];
    let mut keep = [true; SAMPLE_LINES];
    let room = area.height.saturating_sub(1) as usize;
    for drop in [4, 5, 6, 0, 3, 2, 1] {
        if keep.iter().filter(|k| **k).count() <= room {
            break;
        }
        keep[drop] = false;
    }
    let mut y = area.y + 1;
    for (i, line) in lines.iter_mut().enumerate() {
        if !keep[i] {
            continue;
        }
        let rect = Rect::new(area.x, y, area.width, 1);
        match line.take() {
            Some(l) => f.render_widget(Paragraph::new(l), rect),
            None => draw_footer(f, app, rect, &hints, None),
        }
        y += 1;
    }
}

/// The search dialog: a chip per kind of search, the words being typed, and what they find. It floats like the
/// settings, but the list behind is left as it is (not faded): the quick filter narrows it as you type.
fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let s = &app.search;
    let kind = &SEARCH_KINDS[s.kind.min(SEARCH_KINDS.len() - 1)];
    app.hits.borrow_mut().clear();
    let w = if area.width < 70 { area.width.saturating_sub(2) } else { (area.width - 4).min(76) };
    let r = centered(area, w, area.height.saturating_sub(2).clamp(10, 22));
    f.render_widget(Clear, r);
    let block = modal_block(t, "Search");
    let hpad = if w < 64 { 1 } else { 2 };
    let inner = block.inner(r).inner(Margin::new(hpad, 0));
    f.render_widget(block, r);
    let [chips, explain, input, sep, list, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    // One chip per kind: the showing one is lit, a hovered one half lit.
    let mut x = chips.x;
    let mut spans = Vec::new();
    for (i, k) in SEARCH_KINDS.iter().enumerate() {
        let label = format!(" {} ", k.label);
        let cw = label.chars().count() as u16;
        let rect = Rect::new(x, chips.y, cw, 1).intersection(chips);
        if !rect.is_empty() {
            app.hits.borrow_mut().push((rect, Hit::SearchKind(i)));
        }
        let style = if i == s.kind {
            Style::new().fg(t.on_accent).bg(t.accent).add_modifier(Modifier::BOLD)
        } else if rect.contains(app.mouse.into()) {
            Style::new().fg(t.text).bg(t.border)
        } else {
            Style::new().fg(t.dim).bg(t.surface)
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        x += cw + 1;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), chips);
    f.render_widget(Paragraph::new(Span::styled(fit_end(kind.explain, explain.width as usize), Style::new().fg(t.dim))), explain);
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled("❯ ", Style::new().fg(t.accent)), Span::styled(s.text.clone(), Style::new().fg(t.text))])),
        input,
    );
    f.set_cursor_position((input.x + 2 + (s.text.chars().count() as u16).min(input.width.saturating_sub(3)), input.y));
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);

    let count = if kind.narrows_list {
        draw_quick_rows(f, app, list);
        format!("{} shown", app.visible().len())
    } else {
        draw_result_rows(f, app, list);
        match (s.results.len(), s.total) {
            (0, _) => String::new(),
            (n, total) if n < total => format!("first {n} of {total}"),
            (n, _) => format!("{n} found"),
        }
    };
    let enter = if kind.narrows_list { "keep filter" } else { "open" };
    let esc = if kind.narrows_list { "clear" } else { "close" };
    let hints: [Hint; 3] = [("enter", enter, Some(Action::Confirm)), ("tab", "switch", None), ("esc", esc, Some(Action::Cancel))];
    draw_footer(f, app, foot, &hints, Some((count, t.dim)));
    // Last, so it only catches clicks the dialog didn't: outside it, a click closes it like esc.
    app.hits.borrow_mut().push((area, Hit::Action(Action::Cancel)));
}

/// The quick filter's rows: the conversations the list behind is narrowed to, one line each.
fn draw_quick_rows(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let visible = app.visible();
    if visible.is_empty() {
        let msg = if app.search.text.trim().is_empty() { SEARCH_KINDS[0].prompt } else { "No sessions match the filter" };
        f.render_widget(Paragraph::new(msg).style(Style::new().fg(t.dim)), area);
        return;
    }
    let rows = area.height as usize;
    let start = app.selected.saturating_sub(rows.saturating_sub(1));
    let now = crate::usage::now() * 1000;
    for (i, &idx) in visible.iter().enumerate().skip(start).take(rows) {
        let s = &app.sessions[idx];
        let rect = Rect::new(area.x, area.y + (i - start) as u16, area.width, 1);
        app.hits.borrow_mut().push((rect, Hit::SearchRow(i)));
        let sel = i == app.selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let name_w = (area.width as usize * 2 / 5).max(8);
        let dir_w = (area.width as usize).saturating_sub(name_w + 2 + LAST_W);
        let line = Line::from(vec![
            Span::styled(if sel { "▎ " } else { "  " }, Style::new().fg(t.accent).bg(bg)),
            Span::styled(fit(&s.name, name_w.saturating_sub(2)), Style::new().fg(if sel { t.text } else { t.subtext }).bg(bg)),
            Span::styled(fit(&s.dir, dir_w), Style::new().fg(t.dim).bg(bg)),
            Span::styled(format!("{:>w$}", ago(now, s.last_active), w = LAST_W), Style::new().fg(t.dim).bg(bg)),
        ]);
        f.render_widget(Paragraph::new(line).style(Style::new().bg(bg)), rect);
    }
}

/// The word search's results: the conversation's name and when it was used, and under it the matching message with the
/// words in the accent color.
fn draw_result_rows(f: &mut Frame, app: &App, area: Rect) {
    const H: u16 = 2;
    let t = app.effective_theme();
    let s = &app.search;
    if s.results.is_empty() {
        let msg = if s.text.trim().is_empty() {
            SEARCH_KINDS[s.kind].prompt
        } else if app.search_catching_up() {
            "Still reading your conversations…"
        } else {
            "Nothing found"
        };
        f.render_widget(Paragraph::new(msg).style(Style::new().fg(t.dim)), area);
        return;
    }
    let rows = ((area.height / H) as usize).max(1);
    // Scroll only when the highlight would leave the screen.
    let mut top = s.top.get().min(s.results.len().saturating_sub(1));
    if s.selected < top {
        top = s.selected;
    } else if s.selected >= top + rows {
        top = s.selected + 1 - rows;
    }
    s.top.set(top);
    let now = crate::usage::now() * 1000;
    let snippet_max = app.settings().get().search_snippet_chars as usize;
    for (n, r) in s.results.iter().enumerate().skip(top).take(rows) {
        let rect = Rect::new(area.x, area.y + (n - top) as u16 * H, area.width, H.min(area.bottom() - (area.y + (n - top) as u16 * H)));
        app.hits.borrow_mut().push((rect, Hit::SearchRow(n)));
        let sel = n == s.selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let lead = Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent));
        let (name_w, tail) = (area.width as usize * 2 / 5, if r.archived { "archived" } else { "" });
        let l1 = Line::from(vec![
            lead.clone(),
            Span::styled(fit(&r.name, name_w.saturating_sub(2)), base.fg(t.text).add_modifier(Modifier::BOLD)),
            Span::styled(fit(&r.dir, (area.width as usize).saturating_sub(name_w + LAST_W + 10)), base.fg(t.dim)),
            Span::styled(format!("{tail:>9}"), base.fg(t.warn)),
            Span::styled(format!("{:>w$}", ago(now, r.last_active), w = LAST_W), base.fg(t.dim)),
        ]);
        f.render_widget(Paragraph::new(l1).style(base), Rect::new(rect.x, rect.y, rect.width, 1));
        if rect.height > 1 {
            let width = (area.width as usize).saturating_sub(2).min(snippet_max);
            let (text, marks) = crate::prompts::snippet(&r.detail, &r.marks, width);
            let mut spans = vec![lead.clone().content("  ")];
            let chars: Vec<char> = text.chars().collect();
            let mut at = 0;
            for m in marks.iter().filter(|m| m.start < chars.len()) {
                let end = m.end.min(chars.len());
                if m.start > at {
                    spans.push(Span::styled(chars[at..m.start].iter().collect::<String>(), base.fg(t.subtext)));
                }
                spans.push(Span::styled(chars[m.start.max(at)..end].iter().collect::<String>(), base.fg(t.accent).add_modifier(Modifier::BOLD)));
                at = end;
            }
            if at < chars.len() {
                spans.push(Span::styled(chars[at..].iter().collect::<String>(), base.fg(t.subtext)));
            }
            f.render_widget(Paragraph::new(Line::from(spans)).style(base), Rect::new(rect.x, rect.y + 1, rect.width, 1));
        }
    }
}

/// The small box for naming a conversation.
fn draw_rename(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let rn = &app.rename;
    app.hits.borrow_mut().clear();
    let r = centered(area, area.width.saturating_sub(8).clamp(30, 64), 7);
    f.render_widget(Clear, r);
    let block = modal_block(t, "rename conversation");
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [input, sep, note, _, foot] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled("❯ ", Style::new().fg(t.accent)), Span::styled(rn.input.clone(), Style::new().fg(t.text))])),
        input,
    );
    f.set_cursor_position((input.x + 2 + (rn.input.chars().count() as u16).min(input.width.saturating_sub(3)), input.y));
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);
    f.render_widget(Paragraph::new(fit_end("Leave it empty to go back to the automatic name.", note.width as usize)).style(Style::new().fg(t.dim)), note);
    let hints: [Hint; 2] = [("enter", "save", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel))];
    draw_footer(f, app, foot, &hints, None);
}

fn draw_picker(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let p = &app.picker;
    let r = centered(area, area.width.saturating_sub(8).clamp(24, 84), area.height.saturating_sub(4).clamp(8, 20));
    // The modal owns all clicks while it's open.
    app.hits.borrow_mut().clear();
    f.render_widget(Clear, r);
    let block = modal_block(t, "new session: pick a directory");
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);

    let [input, sep, list, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("❯ ", Style::new().fg(t.accent)),
            Span::styled(p.input.clone(), Style::new().fg(t.text)),
        ])),
        input,
    );
    f.set_cursor_position((input.x + 2 + p.input.chars().count() as u16, input.y));
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);

    if p.results.is_empty() {
        f.render_widget(Paragraph::new("No matching directories").style(Style::new().fg(t.dim)), list);
    }
    let rows = list.height as usize;
    let start = p.selected.saturating_sub(rows.saturating_sub(1));
    for (i, path) in p.results.iter().enumerate().skip(start).take(rows) {
        let rect = Rect::new(list.x, list.y + (i - start) as u16, list.width, 1);
        app.hits.borrow_mut().push((rect, Hit::PickerRow(i)));
        let sel = i == p.selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let line = Line::from(vec![
            Span::styled(if sel { "▎ " } else { "  " }, Style::new().fg(t.accent).bg(bg)),
            Span::styled(fit(&tilde(path, &app.home), (rect.width as usize).saturating_sub(2)), Style::new().fg(if sel { t.text } else { t.subtext }).bg(bg)),
        ]);
        f.render_widget(Paragraph::new(line).style(Style::new().bg(bg)), rect);
    }
    let hints: [Hint; 3] = [("enter", "start", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel)), ("↑↓", "choose", None)];
    draw_footer(f, app, foot, &hints, Some((format!("{} dirs", p.results.len()), t.dim)));
}

fn draw_confirm(f: &mut Frame, app: &App, c: &Confirm, area: Rect) {
    let t = app.effective_theme();
    let (title, l1, l2, yes) = match c {
        Confirm::Archive { name, busy, outside, .. } => {
            let what = match (outside, busy) {
                (true, true) => "It is running in a terminal outside platoon and is working right now. Archiving ends that copy, which interrupts what it is doing.",
                (true, false) => "It is running in a terminal outside platoon. Archiving ends that copy.",
                _ => "It is working right now. Archiving stops it, which interrupts what it is doing.",
            };
            (
                "archive conversation",
                format!("Archive \"{name}\"?"),
                format!("{what} The conversation is kept: find it under Archived and press enter to pick it up again."),
                "archive",
            )
        }
        Confirm::Adopt { pids, name, busy, kind, .. } => {
            let interrupt = if *busy { " It is working now, so this interrupts it." } else { "" };
            match kind {
                AdoptKind::TakeOver => (
                    "take over session",
                    format!("Take over \"{name}\"?"),
                    format!("Ends the claude in its terminal (pid {}) and resumes it here.{interrupt}", pids[0]),
                    "take over",
                ),
                AdoptKind::Reload => (
                    "reload session",
                    format!("Reload \"{name}\" from its saved history?"),
                    format!(
                        "It is open in {} place{}, or has fallen out of sync. platoon ends every copy and reopens it here, \
                         so nothing is overwritten or out of sync.{interrupt}",
                        pids.len(),
                        if pids.len() == 1 { "" } else { "s" }
                    ),
                    "reload",
                ),
            }
        }
    };
    let w = (l2.chars().count() as u16 + 6).min(72).max(30).min(area.width);
    // Size the box to its wrapped text (+1 line of slack for word wrapping) so nothing is ever cut off.
    let inner_w = w.saturating_sub(4).max(1) as usize;
    let body_lines = (l2.chars().count().div_ceil(inner_w) + 1) as u16;
    let r = centered(area, w, body_lines + 5);
    app.hits.borrow_mut().clear();
    f.render_widget(Clear, r);
    let block = modal_block(t, title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [a, b, _, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(body_lines), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(Paragraph::new(l1).style(Style::new().fg(t.text).add_modifier(Modifier::BOLD)), a);
    f.render_widget(Paragraph::new(l2).wrap(Wrap { trim: true }).style(Style::new().fg(t.dim)), b);
    let hints: [Hint; 2] = [("y", yes, Some(Action::Confirm)), ("n", "cancel", Some(Action::Cancel))];
    draw_footer(f, app, foot, &hints, None);
}

fn draw_attached(f: &mut Frame, app: &App, client: &Client, dir: &str, area: Rect) {
    let t = app.effective_theme();
    let [term, bar] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let paused = app.attached_paused();
    draw_terminal(f, t, client, term, if paused { None } else { app.selection });
    if paused {
        draw_paused_banner(f, app, term);
    }
    let scrolled = client.parser.lock().map(|p| p.screen().scrollback()).unwrap_or(0);
    let right = if let Some(notice) = app.active_notice() {
        // "Copied 3 lines": what just happened wins over the standing text.
        notice.to_string()
    } else if scrolled > 0 {
        format!("↑ scrolled {scrolled} lines · type to return")
    } else {
        tilde(dir, &app.home)
    };
    let hints: [Hint; 1] = [("ctrl+space", "back to list", Some(Action::Back))];
    let color = if app.active_notice().is_some() { t.warn } else { t.dim };
    draw_footer(f, app, bar.inner(Margin::new(1, 0)), &hints, Some((right, color)));
}

/// Covers the session when this copy is no longer safe to write to. Keys aren't forwarded while it shows.
pub fn draw_paused_banner(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let r = centered(area, area.width.saturating_sub(6).clamp(20, 72), 9.min(area.height));
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.warn))
        .title(Span::styled(" session paused ", Style::new().fg(t.warn).add_modifier(Modifier::BOLD)))
        .style(Style::new().bg(t.bg));
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let lines = vec![
        Line::styled("This session was opened somewhere else too.", Style::new().fg(t.text).add_modifier(Modifier::BOLD)),
        Line::styled("To keep the conversation in sync and nothing overwritten, this copy is paused: what you type is not sent.", Style::new().fg(t.subtext)),
        Line::raw(""),
        Line::from(vec![
            Span::styled("ctrl+space", Style::new().fg(t.accent)),
            Span::styled(" go back, then press ", Style::new().fg(t.dim)),
            Span::styled("enter", Style::new().fg(t.accent)),
            Span::styled(" on it to reload", Style::new().fg(t.dim)),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Paint the attached session into the buffer.
fn draw_terminal(f: &mut Frame, t: &Theme, client: &Client, area: Rect, selection: Option<Selection>) {
    let parser = client.parser.lock().unwrap_or_else(|x| x.into_inner());
    let screen = parser.screen();
    draw_screen(f, t, screen, area, selection.and_then(|s| s.span(screen)));
}

/// The emulator's colors as the theme shows them: the 16 ANSI colors become the theme's palette (when it has
/// one), the default foreground and background become its text and background, and the 256-color cube and
/// truecolor values are the program's own and pass through.
fn vt_color(c: vt100::Color, default: Color, ansi: Option<&[Color; 16]>) -> Color {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => match ansi {
            Some(palette) if i < 16 => palette[i as usize],
            _ => Color::Indexed(i),
        },
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Paint an emulator screen into the buffer, cell by cell. Cells in `selected` are drawn as selected text on
/// top of the session's own colors: on the theme's selection color (see `Theme::selection_bg`), keeping the
/// text's color unless that would not read on it, or reversed where the theme has no colors of its own.
fn draw_screen(f: &mut Frame, t: &Theme, screen: &vt100::Screen, area: Rect, selected: Option<Cells>) {
    let (rows, cols) = screen.size();
    let selection_bg = t.selection_bg();
    let buf = f.buffer_mut();
    for r in 0..rows.min(area.height) {
        for c in 0..cols.min(area.width) {
            let Some(cell) = screen.cell(r, c) else { continue };
            if cell.is_wide_continuation() {
                continue;
            }
            let mut fg = vt_color(cell.fgcolor(), t.text, t.ansi.as_ref());
            let mut bg = vt_color(cell.bgcolor(), t.bg, t.ansi.as_ref());
            let mut m = Modifier::empty();
            if cell.inverse() {
                // Swapping is exact for colors we know. A default that is the terminal's own (Terminal colors) has
                // no value to swap, so the terminal is asked to reverse the cell instead.
                if fg == Color::Reset || bg == Color::Reset {
                    m |= Modifier::REVERSED;
                } else {
                    std::mem::swap(&mut fg, &mut bg);
                }
            }
            if selected.is_some_and(|s| s.contains(r, c)) {
                match selection_bg {
                    Some(sel) => {
                        bg = sel;
                        // A dim gray or a color close to the highlight would vanish on it. (Only colors with
                        // numbers can be measured; the session's 256-color ones are left as they are.)
                        if matches!(fg, Color::Rgb(..)) && contrast(fg, sel) < SELECTION_MIN_FG_CONTRAST {
                            fg = t.text;
                        }
                    }
                    // Toggled, so a cell that was already reversed (inverse video) changes too.
                    None => m ^= Modifier::REVERSED,
                }
            }
            if cell.bold() {
                m |= Modifier::BOLD;
            }
            if cell.italic() {
                m |= Modifier::ITALIC;
            }
            if cell.underline() {
                m |= Modifier::UNDERLINED;
            }
            let sym = cell.contents();
            let sym = if sym.is_empty() { " " } else { sym };
            buf[(area.x + c, area.y + r)].set_symbol(sym).set_style(Style::new().fg(fg).bg(bg).add_modifier(m));
        }
    }
    if !screen.hide_cursor() && screen.scrollback() == 0 {
        let (cr, cc) = screen.cursor_position();
        if cr < area.height && cc < area.width {
            f.set_cursor_position((area.x + cc, area.y + cr));
        }
    }
}

fn pct_color(t: &Theme, pct: u8) -> Color {
    match pct {
        0..=59 => t.ok,
        60..=84 => t.warn,
        _ => t.err,
    }
}

fn bar(t: &Theme, pct: u8) -> Vec<Span<'static>> {
    bar_bg(t, pct, t.bg, true, false)
}

/// `━━━━━━────  62%`, colored by how full it is. A `~` marks an estimate rather than Claude's own figure.
fn bar_bg(t: &Theme, pct: u8, bg: Color, exact: bool, dimmed: bool) -> Vec<Span<'static>> {
    let filled = (pct as usize * BAR_WIDTH + 50) / 100;
    vec![
        Span::styled("━".repeat(filled), Style::new().fg(if dimmed { t.dim } else { pct_color(t, pct) }).bg(bg)),
        Span::styled("━".repeat(BAR_WIDTH - filled), Style::new().fg(t.border).bg(bg)),
        Span::styled(format!("{}{pct:>3}%", if exact { " " } else { "~" }), Style::new().fg(t.subtext).bg(bg)),
    ]
}

/// Pad or truncate (with an ellipsis) to exactly `n` chars. Counts chars, not display width:
/// wide characters (CJK, emoji) will need `unicode-width` before real data is shown.
fn fit(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len > n {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        if n > 0 {
            out.push('…');
        }
        out
    } else {
        format!("{s:<n$}")
    }
}

/// Truncate to at most `n` chars (with an ellipsis), no padding.
fn fit_end(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

    /// What a session that printed `bytes` looks like under `theme` (one row per line, 12 columns).
    fn shown(theme: &Theme, bytes: &[u8]) -> Buffer {
        let mut parser = vt100::Parser::new(4, 12, 0);
        parser.process(bytes);
        let mut term = Terminal::new(TestBackend::new(12, 4)).unwrap();
        term.draw(|f| draw_screen(f, theme, parser.screen(), f.area(), None)).unwrap();
        term.backend().buffer().clone()
    }

    fn fixed() -> Theme {
        Theme::by_id("tokyo-night").unwrap()
    }

    #[test]
    fn a_fixed_theme_recolors_the_sixteen_ansi_colors_with_its_palette() {
        let t = fixed();
        let p = t.ansi.unwrap();
        let b = shown(&t, b"\x1b[31mR\x1b[92mG\x1b[0m\x1b[44m B\x1b[0m\x1b[1;33mY");
        assert_eq!((b[(0, 0)].symbol(), b[(0, 0)].fg), ("R", p[1]), "SGR 31 is the theme's red");
        assert_eq!((b[(1, 0)].symbol(), b[(1, 0)].fg), ("G", p[10]), "SGR 92 is the theme's bright green");
        assert_eq!(b[(3, 0)].bg, p[4], "SGR 44 is the theme's blue background");
        assert_eq!(b[(3, 0)].fg, t.text, "with the default foreground on it");
        assert_eq!((b[(4, 0)].fg, b[(4, 0)].modifier.contains(Modifier::BOLD)), (p[3], true), "bold yellow keeps its weight");
        // Black and white are palette entries too, not the emulator's.
        let b = shown(&t, b"\x1b[30;47mK");
        assert_eq!((b[(0, 0)].fg, b[(0, 0)].bg), (p[0], p[7]));
    }

    #[test]
    fn defaults_become_the_themes_text_and_background_and_program_colors_pass_through() {
        let t = fixed();
        let b = shown(&t, b"plain\x1b[38;5;200mA\x1b[38;5;16mB\x1b[38;5;15mC\x1b[38;2;10;20;30mD\x1b[48;2;1;2;3mE");
        assert_eq!((b[(0, 0)].fg, b[(0, 0)].bg), (t.text, t.bg));
        assert_eq!(b[(5, 0)].fg, Color::Indexed(200), "a 256-color index is the program's own");
        assert_eq!(b[(6, 0)].fg, Color::Indexed(16), "the first cube color is not remapped");
        assert_eq!(b[(7, 0)].fg, t.ansi.unwrap()[15], "index 15 is still one of the sixteen");
        assert_eq!(b[(8, 0)].fg, Color::Rgb(10, 20, 30), "truecolor is the program's own");
        assert_eq!(b[(9, 0)].bg, Color::Rgb(1, 2, 3));
        // Empty cells are painted with the theme's background too.
        assert_eq!((b[(11, 3)].fg, b[(11, 3)].bg), (t.text, t.bg));
    }

    #[test]
    fn reverse_video_swaps_the_recolored_colors() {
        let t = fixed();
        let p = t.ansi.unwrap();
        let b = shown(&t, b"\x1b[7mX\x1b[0m\x1b[31;7mY\x1b[0m\x1b[32;44;7mZ");
        assert_eq!((b[(0, 0)].fg, b[(0, 0)].bg), (t.bg, t.text), "the defaults swap");
        assert!(!b[(0, 0)].modifier.contains(Modifier::REVERSED), "the colors are swapped here, not by the terminal");
        assert_eq!((b[(1, 0)].fg, b[(1, 0)].bg), (t.bg, p[1]), "red text reversed is red behind the background color");
        assert_eq!((b[(2, 0)].fg, b[(2, 0)].bg), (p[4], p[2]));
    }

    #[test]
    fn terminal_colors_pass_the_emulators_colors_through_untouched() {
        let t = Theme::by_id("terminal").unwrap();
        let b = shown(&t, b"\x1b[31mR\x1b[92mG\x1b[0m\x1b[44m B\x1b[0m\x1b[38;5;200mA\x1b[38;2;10;20;30mD\x1b[0m plain");
        assert_eq!(b[(0, 0)].fg, Color::Indexed(1), "the terminal's own red");
        assert_eq!(b[(1, 0)].fg, Color::Indexed(10));
        assert_eq!(b[(3, 0)].bg, Color::Indexed(4));
        assert_eq!(b[(4, 0)].fg, Color::Indexed(200));
        assert_eq!(b[(5, 0)].fg, Color::Rgb(10, 20, 30));
        assert_eq!((b[(7, 0)].fg, b[(7, 0)].bg), (Color::Reset, Color::Reset), "defaults are the terminal's defaults");
    }

    #[test]
    fn reverse_video_still_shows_under_terminal_colors() {
        let t = Theme::by_id("terminal").unwrap();
        // With both colors the terminal's own there is nothing to swap by hand, so the terminal is told to.
        let b = shown(&t, b"\x1b[7mX\x1b[0m\x1b[31;7mY");
        assert!(b[(0, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!((b[(0, 0)].fg, b[(0, 0)].bg), (Color::Reset, Color::Reset));
        assert!(b[(1, 0)].modifier.contains(Modifier::REVERSED), "red text on the default background, reversed by the terminal");
        assert_eq!((b[(1, 0)].fg, b[(1, 0)].bg), (Color::Indexed(1), Color::Reset));
        // Both explicit: swapped by hand, same as with a fixed theme.
        let b = shown(&t, b"\x1b[32;44;7mZ");
        assert_eq!((b[(0, 0)].fg, b[(0, 0)].bg), (Color::Indexed(4), Color::Indexed(2)));
        assert!(!b[(0, 0)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn every_fixed_theme_paints_a_session_only_in_fixed_colors() {
        for t in Theme::all().into_iter().filter(|t| t.ansi.is_some()) {
            let b = shown(&t, b"\x1b[31mred\x1b[0m \x1b[7mrev\x1b[0m \x1b[42mgreen");
            for cell in &b.content {
                assert!(matches!(cell.fg, Color::Rgb(..)) && matches!(cell.bg, Color::Rgb(..)), "{}: {:?} {:?}", t.id, cell.fg, cell.bg);
            }
        }
    }

    #[test]
    fn relative_times_read_at_a_glance() {
        let now = 100_000_000_000u64;
        let back = |secs: u64| ago(now, now - secs * 1000);
        assert_eq!(back(5), "now");
        assert_eq!(back(60), "1m");
        assert_eq!(back(3 * 60 + 20), "3m");
        assert_eq!(back(59 * 60), "59m");
        assert_eq!(back(3600), "1h");
        assert_eq!(back(23 * 3600), "23h");
        assert_eq!(back(2 * 86_400 + 100), "2d");
        assert_eq!(back(45 * 86_400), "45d");
        assert_eq!(back(90 * 86_400), "3mo");
        assert_eq!(back(400 * 86_400), "1y");
        assert_eq!(ago(now, 0), "-");
        assert_eq!(ago(now, now + 5000), "now", "a clock a little ahead never goes negative");
    }
}
