//! Drawing the Plugins tab (`4`): the catalog and the installed plugins on the left, the selected one's details on
//! the right, and the key hints; and the settings screen's Plugins tab (the installed plugins, each with remove).

use super::*;
use crate::app::{PlugRow, PlugState};

/// The keys of the Plugins tab, for the plugin selected.
pub(super) fn plugins_tab_hints(app: &App) -> Vec<Hint<'static>> {
    let mut out: Vec<Hint<'static>> = Vec::new();
    match app.pl.selected_row().map(|r| r.state) {
        Some(PlugState::Available) => out.push(("enter", "install", None)),
        Some(PlugState::Update) => {
            out.push(("enter", "update", None));
            out.push(("d", "remove", None));
        }
        Some(s) if s.installed() => out.push(("d", "remove", None)),
        _ => {}
    }
    out.extend([("f", "check the catalog", None), ("↑↓", "move", None), ("1-4", "tabs", None), ("s", "settings", Some(Action::Settings)), ("q", "quit", Some(Action::Quit))]);
    out
}

pub(super) fn draw_plugins_tab(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let left_w = (area.width * 34 / 100).clamp(26, 42).min(area.width.saturating_sub(20));
    let [left, right] = Layout::horizontal([Constraint::Length(left_w), Constraint::Min(1)]).areas(area);
    let block = |title: String, focused: bool| {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(if focused { t.accent } else { t.border }))
            .title(Span::styled(title, Style::new().fg(if focused { t.accent } else { t.dim }).add_modifier(Modifier::BOLD)))
            .style(Style::new().bg(t.bg))
    };
    let rows = app.pl.rows();
    let lb = block(" Plugins ".into(), true);
    let li = lb.inner(left).inner(Margin::new(1, 0));
    f.render_widget(lb, left);
    // The last line of the list says where the catalog stands.
    let status_h = u16::from(li.height > 3);
    let list = Rect::new(li.x, li.y, li.width, li.height - status_h);
    draw_plugin_list(f, app, list, &rows);
    if status_h > 0 {
        let (text, color) = catalog_status(app);
        f.render_widget(Paragraph::new(fit_end(&text, li.width as usize)).style(Style::new().fg(color)), Rect::new(li.x, li.bottom() - 1, li.width, 1));
    }
    let sel = rows.get(app.pl.selected.min(rows.len().saturating_sub(1)));
    let title = sel.map_or(" About plugins ".to_string(), |r| format!(" {} ", r.manifest.name));
    let rb = block(title, false);
    let ri = rb.inner(right).inner(Margin::new(1, 0));
    f.render_widget(rb, right);
    match sel {
        Some(r) => draw_plugin_detail(f, app, ri, r),
        None => {
            let msg = "Plugins add things to platoon that not everyone wants, like more color themes. They come from platoon's \
                       own catalog, where anyone can send one (see plugins/README.md in platoon's repository).";
            f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }).style(Style::new().fg(t.subtext)), ri);
        }
    }
}

/// "⠋ Checking the catalog…", "Catalog: commit 1a2b3c4", or that it couldn't be reached.
fn catalog_status(app: &App) -> (String, Color) {
    let t = app.effective_theme();
    if app.pl.fetching() {
        let spin = SPINNER[(app.tick % 10) as usize];
        return (format!("{spin} Checking the catalog…"), t.dim);
    }
    if app.pl.fetch_error.is_some() {
        let old = if app.pl.catalog.is_some() { ": showing the last copy" } else { "" };
        return (format!("Couldn't reach the catalog{old}"), t.warn);
    }
    match &app.pl.catalog {
        Some(c) => (format!("Catalog: commit {}", crate::plugins::short(&c.commit)), t.dim),
        None => ("Catalog: not fetched yet".into(), t.dim),
    }
}

fn draw_plugin_list(f: &mut Frame, app: &App, li: Rect, rows: &[PlugRow]) {
    let t = app.effective_theme();
    if rows.is_empty() {
        let msg = if app.pl.fetching() {
            "Fetching the catalog…".to_string()
        } else if let Some(e) = &app.pl.fetch_error {
            format!("{e}. f tries again.")
        } else {
            "No plugins yet. f checks the catalog again.".to_string()
        };
        let color = if app.pl.fetch_error.is_some() && !app.pl.fetching() { t.warn } else { t.dim };
        f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }).style(Style::new().fg(color)), li);
        return;
    }
    let per = 3u16;
    let room = (li.height / per).max(1) as usize;
    let selected = app.pl.selected.min(rows.len() - 1);
    let start = selected.saturating_sub(room.saturating_sub(1));
    let mut y = li.y;
    for (i, r) in rows.iter().enumerate().skip(start) {
        if y + 2 > li.bottom() {
            break;
        }
        let rect = Rect::new(li.x, y, li.width, 2);
        app.hits.borrow_mut().push((rect, Hit::PluginRow(i)));
        let sel = i == selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let w = li.width.saturating_sub(2) as usize;
        let version = r.manifest.version.clone();
        let (name_w, version) = if r.manifest.name.chars().count() + 2 + version.chars().count() <= w { (w - version.chars().count() - 1, version) } else { (w, String::new()) };
        let name = fit_end(&r.manifest.name, name_w);
        let gap = w.saturating_sub(name.chars().count() + version.chars().count());
        let state_color = match r.state {
            PlugState::Update | PlugState::Unavailable(_) => t.warn,
            ref s if s.installed() => t.ok,
            _ => t.dim,
        };
        let lines = vec![
            Line::from(vec![
                Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)),
                Span::styled(name, base.fg(t.text).add_modifier(Modifier::BOLD)),
                Span::styled(" ".repeat(gap), base),
                Span::styled(version, base.fg(t.dim)),
            ]),
            Line::from(vec![Span::styled("  ", base), Span::styled(fit_end(&r.state.words(), w), base.fg(state_color))]),
        ];
        f.render_widget(Paragraph::new(lines).style(base), rect);
        y += per;
    }
}

fn draw_plugin_detail(f: &mut Frame, app: &App, area: Rect, r: &PlugRow) {
    let t = app.effective_theme();
    let w = area.width as usize;
    let m = &r.manifest;
    let mut lines: Vec<Line<'static>> = wrap(&m.description, w).into_iter().map(|l| Line::styled(l, Style::new().fg(t.text))).collect();
    lines.push(Line::raw(""));
    const LABEL: usize = 10;
    let mut field = |label: &str, value: String, color: Color| {
        let room = w.saturating_sub(LABEL + 1).max(8);
        for (k, part) in wrap(&value, room).into_iter().enumerate() {
            let lead = if k == 0 { format!("{label:<LABEL$}") } else { " ".repeat(LABEL) };
            lines.push(Line::from(vec![Span::styled(lead, Style::new().fg(t.dim)), Span::raw(" "), Span::styled(part, Style::new().fg(color))]));
        }
    };
    field("By", m.author.clone(), t.subtext);
    let version = match &r.installed {
        Some(p) if p.manifest.version != m.version => format!("{} (installed: {})", m.version, p.manifest.version),
        _ => m.version.clone(),
    };
    field("Version", version, t.subtext);
    let names = m.theme_names();
    let adds = if names.is_empty() { m.adds_words() } else { format!("{}: {}", m.adds_words(), names.join(", ")) };
    field("Adds", adds, t.subtext);
    if let Some(l) = &m.license {
        field("License", l.clone(), t.subtext);
    }
    let from = match &r.installed {
        Some(p) => p.origin.words(&app.home),
        None => "the catalog".to_string(),
    };
    field("From", from, t.subtext);
    field("Needs", format!("platoon {} or newer", m.platoon), t.subtext);
    lines.push(Line::raw(""));
    let (what, color) = match &r.state {
        PlugState::Available => ("Not installed. Enter installs it.".to_string(), t.dim),
        PlugState::Installed => ("Installed. Its themes are in the settings (s, then Theme). d removes it.".to_string(), t.ok),
        PlugState::Update => ("An update is ready: enter installs it. d removes the plugin.".to_string(), t.warn),
        PlugState::FromFolder => ("Installed from a folder on this computer. d removes it.".to_string(), t.ok),
        PlugState::NotInCatalog => ("Installed, but the catalog doesn't have it any more. It keeps working; d removes it.".to_string(), t.subtext),
        PlugState::Unavailable(why) => (format!("It can't be installed here: {why}"), t.warn),
    };
    for l in wrap(&what, w) {
        lines.push(Line::styled(l, Style::new().fg(color)));
    }
    for (k, line) in lines.into_iter().take(area.height as usize).enumerate() {
        f.render_widget(Paragraph::new(line), Rect::new(area.x, area.y + k as u16, area.width, 1));
    }
}

/// Rows the settings' Plugins tab needs: two per installed plugin, a blank between, or one line saying there are none.
pub(super) fn plugin_settings_height(app: &App) -> usize {
    let n = app.pl.installed.len();
    if n == 0 { 1 } else { 3 * n - 1 }
}

/// The settings' Plugins tab: each installed plugin (its name, version and what it adds; under it where it came
/// from). Returns the row below the last.
pub(super) fn draw_plugin_settings(f: &mut Frame, app: &App, area: Rect) -> u16 {
    let t = app.effective_theme();
    if area.is_empty() {
        return area.y;
    }
    let installed = &app.pl.installed;
    if installed.is_empty() {
        f.render_widget(Paragraph::new(Span::styled("No plugins installed.", Style::new().fg(t.dim))), Rect::new(area.x, area.y, area.width, 1));
        return area.y + 1;
    }
    let per = 3usize;
    let room = ((area.height as usize + 1) / per).max(1);
    let selected = app.setting.min(installed.len() - 1);
    let start = selected.saturating_sub(room - 1);
    let mut y = area.y;
    for (i, p) in installed.iter().enumerate().skip(start) {
        if y + 2 > area.bottom() {
            break;
        }
        let row = Rect::new(area.x, y, area.width, 2);
        app.hits.borrow_mut().push((row, Hit::Setting(i)));
        let sel = i == selected;
        let bg = if sel || row.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let w = (area.width as usize).saturating_sub(2);
        let right = format!("{} · {}", p.manifest.version, p.manifest.adds_words());
        let name_w = w.saturating_sub(right.chars().count() + 1);
        let name = fit_end(&p.manifest.name, name_w);
        let gap = w.saturating_sub(name.chars().count() + right.chars().count());
        let lines = vec![
            Line::from(vec![
                Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)),
                Span::styled(name, base.fg(t.text).add_modifier(Modifier::BOLD)),
                Span::styled(" ".repeat(gap), base),
                Span::styled(right, base.fg(t.accent)),
            ]),
            Line::from(vec![Span::styled("  ", base), Span::styled(fit_end(&format!("From {}", p.origin.words(&app.home)), w), base.fg(t.subtext))]),
        ];
        f.render_widget(Paragraph::new(lines).style(base), row);
        y += per as u16;
    }
    y.saturating_sub(1)
}
