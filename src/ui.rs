use crate::app::App;
use crate::model::{Session, Status};
use crate::theme::Theme;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BAR_WIDTH: usize = 10;

pub fn draw(f: &mut Frame, app: &App) {
    let t = &app.theme;
    f.render_widget(Block::default().style(Style::new().bg(t.bg)), f.area());

    let frame = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.border))
        .style(Style::new().bg(t.bg));
    let inner = frame.inner(f.area());
    f.render_widget(frame, f.area());

    let [header, _, cols, list, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner.inner(ratatui::layout::Margin::new(2, 0)));

    draw_header(f, app, header);
    draw_columns(f, t, cols);
    draw_list(f, app, list);
    draw_footer(f, t, footer);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let unread = app.unread_count();
    let mut left = vec![
        Span::styled("platoon", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {} sessions", app.sessions.len()), Style::new().fg(t.dim)),
    ];
    if unread > 0 {
        left.push(Span::styled(format!("  ● {unread} unread"), Style::new().fg(t.ok)));
    }
    let mut right = vec![Span::styled("5h ", Style::new().fg(t.dim))];
    right.extend(bar(t, app.usage.five_hour_pct));
    right.push(Span::styled("   7d ", Style::new().fg(t.dim)));
    right.extend(bar(t, app.usage.seven_day_pct));

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

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    const ROW_H: u16 = 3; // two text lines + one spacer
    let visible = (area.height / ROW_H) as usize;
    // Keep the selection in view.
    let start = app.selected.saturating_sub(visible.saturating_sub(1));
    for (i, s) in app.sessions.iter().enumerate().skip(start).take(visible) {
        let y = area.y + (i - start) as u16 * ROW_H;
        let row = Rect::new(area.x, y, area.width, ROW_H.min(area.bottom() - y));
        draw_row(f, app, s, i == app.selected, row);
    }
}

fn draw_row(f: &mut Frame, app: &App, s: &Session, selected: bool, area: Rect) {
    let t = &app.theme;
    let c = cols(area.width);
    let bg = if selected { t.surface } else { t.bg };
    let base = Style::new().bg(bg);

    // Unread pulses between the accent and a softer shade about once a second.
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
    let name = if s.external { format!("{} ext", s.name) } else { s.name.clone() };
    let bar_spans = bar_bg(t, s.context_pct, bg);

    let lead = if selected { Span::styled("▎", base.fg(t.accent)) } else { Span::styled(" ", base) };
    let mut l1 = vec![
        lead.clone(),
        Span::styled(format!("{marker} "), base.fg(marker_color)),
        Span::styled(fit(&name, c.name), name_style),
        Span::styled(fit(&s.dir, c.dir), base.fg(t.subtext)),
        Span::styled(fit(s.branch.as_deref().unwrap_or("-"), c.branch), base.fg(t.dim)),
        Span::styled(fit(status_text, c.status), base.fg(status_color)),
    ];
    l1.extend(bar_spans);

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

fn draw_footer(f: &mut Frame, t: &Theme, area: Rect) {
    let hints = [("enter", "jump"), ("n", "new"), ("x", "kill"), ("/", "filter"), ("j/k ↑↓", "move"), ("q", "quit")];
    let mut spans = Vec::new();
    for (i, (k, d)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", Style::new().fg(t.dim)));
        }
        spans.push(Span::styled(*k, Style::new().fg(t.accent)));
        spans.push(Span::styled(format!(" {d}"), Style::new().fg(t.dim)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn pct_color(t: &Theme, pct: u8) -> ratatui::style::Color {
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
fn bar_bg(t: &Theme, pct: u8, bg: ratatui::style::Color) -> Vec<Span<'static>> {
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
