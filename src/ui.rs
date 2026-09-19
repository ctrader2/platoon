use crate::app::{Action, App, Confirm, Hit, Mode};
use crate::dirs::tilde;
use crate::client::Client;
use crate::model::{Session, Status};
use crate::usage::Usage;
use crate::theme::Theme;
use ratatui::{
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BAR_WIDTH: usize = 10;

type Hint = (&'static str, &'static str, Option<Action>);

pub fn draw(f: &mut Frame, app: &App) {
    let t = &app.theme;
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

    let [header, _, cols, list, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner.inner(Margin::new(2, 0)));

    draw_header(f, app, header);
    draw_columns(f, t, cols);
    let visible = app.visible();
    if visible.is_empty() {
        let msg = if app.sessions.is_empty() { "No active Claude Code sessions. Press n to start one." } else { "No sessions match the filter" };
        let p = Paragraph::new(msg).style(Style::new().fg(t.dim)).alignment(Alignment::Center);
        f.render_widget(p, Rect::new(list.x, list.y + list.height / 3, list.width, 1));
    } else {
        draw_list(f, app, &visible, list);
    }
    let hints: &[Hint] = match app.mode {
        Mode::Filter => &[("enter", "apply", Some(Action::Confirm)), ("esc", "clear", Some(Action::Cancel))],
        _ => &[
            ("enter", "jump", Some(Action::Jump)),
            ("n", "new", Some(Action::New)),
            ("x", "kill", Some(Action::Kill)),
            ("/", "filter", Some(Action::Filter)),
            ("j/k ↑↓", "move", None),
            ("q", "quit", Some(Action::Quit)),
        ],
    };
    draw_footer(f, app, footer, hints, app.active_notice().map(|n| (n.to_string(), t.warn)));

    match &app.mode {
        Mode::Picker => draw_picker(f, app, inner),
        Mode::Confirm(c) => draw_confirm(f, app, c, inner),
        _ => {}
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let unread = app.unread_count();
    let shown = app.visible().len();
    let total = app.sessions.len();
    let count = if shown == total { format!("  {total} session{}", if total == 1 { "" } else { "s" }) } else { format!("  {shown}/{total} sessions") };
    let mut left = vec![
        Span::styled("platoon", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(count, Style::new().fg(t.dim)),
    ];
    if unread > 0 {
        left.push(Span::styled(format!("  ● {unread} unread"), Style::new().fg(t.ok)));
    }
    match app.mode {
        Mode::Filter => {
            left.push(Span::styled("   / ", Style::new().fg(t.accent)));
            left.push(Span::styled(app.filter.clone(), Style::new().fg(t.text)));
            left.push(Span::styled("▏", Style::new().fg(t.accent)));
        }
        _ if !app.filter.is_empty() => {
            left.push(Span::styled(format!("   filter: {}", app.filter), Style::new().fg(t.accent)));
            left.push(Span::styled("  (esc clears)", Style::new().fg(t.dim)));
        }
        _ => {}
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

    let right_w = right.iter().map(|s| s.content.chars().count()).sum::<usize>() as u16;
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(right_w)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(left)), l);
    f.render_widget(Paragraph::new(Line::from(right)), r);
}

struct Cols {
    name: usize,
    dir: usize,
    branch: usize,
    status: usize,
}

fn cols(width: u16) -> Cols {
    // marker(3) + name + dir + branch + status + context bar(BAR_WIDTH + 5)
    let fixed = 3 + BAR_WIDTH + 5;
    let w = width as usize;
    let branch = if w >= 90 { 16 } else { 0 };
    let status = 9;
    let flex = w.saturating_sub(fixed + branch + status);
    Cols { name: flex * 2 / 5, dir: flex - flex * 2 / 5, branch, status }
}

fn draw_columns(f: &mut Frame, t: &Theme, area: Rect) {
    let c = cols(area.width);
    let s = Style::new().fg(t.dim);
    let line = Line::from(vec![
        Span::raw("   "),
        Span::styled(fit("NAME", c.name), s),
        Span::styled(fit("DIR", c.dir), s),
        Span::styled(fit(if c.branch > 0 { "BRANCH" } else { "" }, c.branch), s),
        Span::styled(fit("STATUS", c.status), s),
        Span::styled("CONTEXT", s),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_list(f: &mut Frame, app: &App, visible: &[usize], area: Rect) {
    const ROW_H: u16 = 3; // two text lines + one spacer
    let rows = (area.height / ROW_H) as usize;
    // Keep the selection in view.
    let start = app.selected.saturating_sub(rows.saturating_sub(1));
    for (i, &idx) in visible.iter().enumerate().skip(start).take(rows) {
        let y = area.y + (i - start) as u16 * ROW_H;
        let row = Rect::new(area.x, y, area.width, ROW_H.min(area.bottom() - y));
        app.hits.borrow_mut().push((row, Hit::Row(i)));
        let hover = row.contains(app.mouse.into());
        draw_row(f, app, &app.sessions[idx], i == app.selected, hover, row);
    }
}

fn draw_row(f: &mut Frame, app: &App, s: &Session, selected: bool, hover: bool, area: Rect) {
    let t = &app.theme;
    let c = cols(area.width);
    let bg = if selected || hover { t.surface } else { t.bg };
    let base = Style::new().bg(bg);

    // Unread pulses between two shades about once a second.
    let pulse_on = (app.tick / 5) % 2 == 0;
    let (marker, marker_color) = match (s.status, s.unread) {
        (Status::Busy, _) => (SPINNER[(app.tick % 10) as usize], t.busy),
        (Status::Idle, true) => ("●", if pulse_on { t.ok } else { t.dim }),
        (Status::Idle, false) => ("○", t.dim),
    };
    let name_style = if s.unread {
        base.fg(t.ok).add_modifier(Modifier::BOLD)
    } else if s.external {
        base.fg(t.subtext)
    } else {
        base.fg(t.text)
    };
    let (status_text, status_color) = match (s.status, s.unread) {
        (Status::Busy, _) => ("working", t.busy),
        (Status::Idle, true) => ("done", t.ok),
        (Status::Idle, false) => ("idle", t.dim),
    };
    // Only tag external sessions once some sessions are hosted by platoon.
    let mixed = app.sessions.iter().any(|o| !o.external);
    let name = if s.external && mixed { format!("{} ext", s.name) } else { s.name.clone() };

    let lead = if selected { Span::styled("▎", base.fg(t.accent)) } else { Span::styled(" ", base) };
    let mut l1 = vec![
        lead.clone(),
        Span::styled(format!("{marker} "), base.fg(marker_color)),
        Span::styled(fit(&name, c.name), name_style),
        Span::styled(fit(&s.dir, c.dir), base.fg(t.subtext)),
        Span::styled(fit(s.branch.as_deref().unwrap_or("-"), c.branch), base.fg(t.dim)),
        Span::styled(fit(status_text, c.status), base.fg(status_color)),
    ];
    l1.extend(bar_bg(t, s.context_pct, bg));

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

/// Key hints along the bottom. Hints with an action are clickable and highlight on hover.
fn draw_footer(f: &mut Frame, app: &App, area: Rect, hints: &[Hint], right: Option<(String, Color)>) {
    let t = &app.theme;
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

fn draw_picker(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
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
    let t = &app.theme;
    let (title, l1, l2, yes) = match c {
        Confirm::Kill { name, dir, .. } => (
            "kill session",
            format!("Kill \"{name}\"?"),
            format!("This makes it inactive; nothing is deleted. Resume it later from {dir} with `claude --resume`, or from platoon."),
            "kill",
        ),
        Confirm::Adopt { pid, name, busy, .. } => (
            "take over session",
            format!("Take over \"{name}\"?"),
            format!(
                "Ends the claude in its terminal (pid {pid}) and resumes it here.{}",
                if *busy { " It is working now: this interrupts it." } else { "" }
            ),
            "take over",
        ),
    };
    let w = (l2.chars().count() as u16 + 6).clamp(30, 64.min(area.width));
    let r = centered(area, w, 9);
    app.hits.borrow_mut().clear();
    f.render_widget(Clear, r);
    let block = modal_block(t, title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [a, b, _, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(3), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(Paragraph::new(l1).style(Style::new().fg(t.text).add_modifier(Modifier::BOLD)), a);
    f.render_widget(Paragraph::new(l2).wrap(Wrap { trim: true }).style(Style::new().fg(t.dim)), b);
    let hints: [Hint; 2] = [("y", yes, Some(Action::Confirm)), ("n", "cancel", Some(Action::Cancel))];
    draw_footer(f, app, foot, &hints, None);
}

fn draw_attached(f: &mut Frame, app: &App, client: &Client, dir: &str, area: Rect) {
    let t = &app.theme;
    let [term, bar] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    draw_terminal(f, t, client, term);
    let scrolled = client.parser.lock().map(|p| p.screen().scrollback()).unwrap_or(0);
    let right = if scrolled > 0 {
        format!("↑ scrolled {scrolled} lines · type to return")
    } else {
        tilde(dir, &app.home)
    };
    let hints: [Hint; 1] = [("ctrl+space", "back to list", Some(Action::Back))];
    draw_footer(f, app, bar.inner(Margin::new(1, 0)), &hints, Some((right, t.dim)));
}

fn vt_color(c: vt100::Color, default: Color) -> Color {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Paint the emulator's screen into the buffer, cell by cell.
fn draw_terminal(f: &mut Frame, t: &Theme, client: &Client, area: Rect) {
    let parser = client.parser.lock().unwrap_or_else(|x| x.into_inner());
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    let buf = f.buffer_mut();
    for r in 0..rows.min(area.height) {
        for c in 0..cols.min(area.width) {
            let Some(cell) = screen.cell(r, c) else { continue };
            if cell.is_wide_continuation() {
                continue;
            }
            let mut fg = vt_color(cell.fgcolor(), t.text);
            let mut bg = vt_color(cell.bgcolor(), t.bg);
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            let mut m = Modifier::empty();
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
    bar_bg(t, pct, t.bg)
}

/// `━━━━━━────  62%`, colored by how full it is.
fn bar_bg(t: &Theme, pct: u8, bg: Color) -> Vec<Span<'static>> {
    let filled = (pct as usize * BAR_WIDTH + 50) / 100;
    vec![
        Span::styled("━".repeat(filled), Style::new().fg(pct_color(t, pct)).bg(bg)),
        Span::styled("━".repeat(BAR_WIDTH - filled), Style::new().fg(t.border).bg(bg)),
        Span::styled(format!(" {pct:>3}%"), Style::new().fg(t.subtext).bg(bg)),
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
