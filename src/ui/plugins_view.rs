//! Drawing plugins: the settings screen's Plugins tab (every plugin, installed or in the catalog, one line each with
//! where it stands, and the selected one's description under them), and the plugin screen it opens (full screen: the catalog and the installed plugins on the left,
//! the selected one's details on the right, and the key hints).

use super::*;
use crate::app::{PlugRow, PlugState};

/// The keys of the plugin screen, for the plugin selected.
pub(super) fn plugin_screen_hints(app: &App) -> Vec<Hint<'_>> {
    let mut out: Vec<Hint> = Vec::new();
    let row = app.pl.selected_row();
    if row.as_ref().is_some_and(|r| editor_unfinished(app, r)) {
        out.push(("enter", "finish downloads", None));
    }
    let reads = row.as_ref().is_some_and(|r| r.manifest.has_instructions());
    match row.map(|r| r.state) {
        Some(PlugState::Available) if reads => out.push(("enter", "read and install", None)),
        Some(PlugState::Available) => out.push(("enter", "install", None)),
        Some(PlugState::Update) => {
            out.push(("enter", "update", None));
            out.push(("d", "remove", None));
        }
        Some(s) if s.installed() => out.push(("d", "remove", None)),
        _ => {}
    }
    // (`1` `2` `3` go straight to a tab from here too; not shown, as on the settings.)
    out.extend([("↑↓", "move", None), ("?", "help", Some(Action::Help)), ("esc", "back to settings", Some(Action::Cancel))]);
    out
}

pub(super) fn draw_plugin_screen(f: &mut Frame, app: &App, area: Rect) {
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
    // Where this is: it was opened from the settings.
    let lb = block(" Settings · Plugins ".into(), true);
    let li = lb.inner(left).inner(Margin::new(1, 0));
    f.render_widget(lb, left);
    draw_plugin_list(f, app, li, &rows);
    let sel = rows.get(app.pl.selected.min(rows.len().saturating_sub(1)));
    let title = sel.map_or(" About plugins ".to_string(), |r| format!(" {} ", r.manifest.name));
    let rb = block(title, false);
    let ri = rb.inner(right).inner(Margin::new(1, 0));
    f.render_widget(rb, right);
    match sel {
        Some(r) => draw_plugin_detail(f, app, ri, r),
        None => {
            let msg = "Plugins add things to platoon that not everyone wants, like its code editor or more color themes. They come with platoon, \
                       in its own catalog, where anyone can send one (see plugins/README.md in platoon's repository).";
            f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }).style(Style::new().fg(t.subtext)), ri);
        }
    }
}

/// The most lines the selected plugin's description has under the settings' list (the rest is cut with "…").
pub(super) const DESCRIPTION_LINES: usize = 3;

/// The description of plugin `r`, wrapped to `w` and cut to `DESCRIPTION_LINES` lines, the last ending in "…" when
/// there was more.
pub(super) fn description_lines(r: &PlugRow, w: usize) -> Vec<String> {
    let mut lines = wrap(&r.manifest.description, w.max(1));
    if lines.len() > DESCRIPTION_LINES {
        lines.truncate(DESCRIPTION_LINES);
        if let Some(last) = lines.last_mut() {
            let keep = w.max(2) - 1;
            let cut: String = last.chars().take(keep).collect();
            *last = format!("{}…", cut.trim_end());
        }
    }
    lines
}

/// The code editor plugin is installed but not everything it downloads is there.
fn editor_unfinished(app: &App, r: &PlugRow) -> bool {
    r.manifest.editor.is_some() && r.state.installed() && !app.pl.editor_complete
}

/// How a row's state is said: the code editor's unfinished downloads too.
fn state_words(app: &App, r: &PlugRow) -> String {
    if editor_unfinished(app, r) {
        return "Installed · downloads not finished".into();
    }
    r.state.words()
}

fn draw_plugin_list(f: &mut Frame, app: &App, li: Rect, rows: &[PlugRow]) {
    let t = app.effective_theme();
    if rows.is_empty() {
        f.render_widget(Paragraph::new("No plugins to show.").wrap(Wrap { trim: true }).style(Style::new().fg(t.dim)), li);
        return;
    }
    let per = 3u16;
    let room = (li.height / per).max(1) as usize;
    app.pages.plugins.set(room);
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
        let (name_w, version) = if r.manifest.name.chars().count() + 2 + version.chars().count() <= w {
            (w - version.chars().count() - 1, version)
        } else {
            (w, String::new())
        };
        let name = fit_end(&r.manifest.name, name_w);
        let gap = w.saturating_sub(name.chars().count() + version.chars().count());
        let state_color = match r.state {
            _ if editor_unfinished(app, r) => t.warn,
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
            Line::from(vec![Span::styled("  ", base), Span::styled(fit_end(&state_words(app, r), w), base.fg(state_color))]),
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
            lines.push(Line::from(vec![
                Span::styled(lead, Style::new().fg(t.dim)),
                Span::raw(" "),
                Span::styled(part, Style::new().fg(color)),
            ]));
        }
    };
    field("By", m.author.clone(), t.subtext);
    let version = match &r.installed {
        Some(p) if p.manifest.version != m.version => format!("{} (installed: {})", m.version, p.manifest.version),
        _ => m.version.clone(),
    };
    field("Version", version, t.subtext);
    let names: Vec<String> = m
        .theme_names()
        .into_iter()
        .chain(m.files(crate::skillplugin::KIND).iter().map(|f| format!("/{}", f.trim_end_matches('/').rsplit('/').next().unwrap_or(f))))
        .chain(
            [crate::squadplugin::SQUADS, crate::squadplugin::AGENTS]
                .iter()
                .flat_map(|k| m.files(k).iter().map(|f| f.rsplit('/').next().unwrap_or(f).trim_end_matches(".json").to_string())),
        )
        .collect();
    let adds = if m.editor.is_some() {
        let size = r.download_bytes.map_or(String::new(), |b| format!("; it downloads {}", crate::editorplugin::size_words(b)));
        format!("{} (Neovim, its tools and plugins, syntax highlighting, a font with icons){size}", m.adds_words())
    } else if names.is_empty() {
        m.adds_words()
    } else {
        format!("{}: {}", m.adds_words(), names.join(", "))
    };
    field("Adds", adds, t.subtext);
    if let Some(l) = &m.license {
        field("License", l.clone(), t.subtext);
    }
    field("Needs", format!("platoon {} or newer", m.platoon), t.subtext);
    lines.push(Line::raw(""));
    let editor = m.editor.is_some();
    let (what, color) = match &r.state {
        _ if editor_unfinished(app, r) => (
            "Installed, but not everything it downloads is there yet: enter finishes it (only what is missing is fetched). d removes it."
                .to_string(),
            t.warn,
        ),
        PlugState::Available if editor => {
            ("Not installed. Enter installs it and its downloads; then v on a session or a ticket opens it.".to_string(), t.dim)
        }
        PlugState::Installed | PlugState::FromFolder | PlugState::NotInCatalog if editor => {
            ("Installed. v on a session or a ticket opens it. d removes it with everything it downloaded.".to_string(), t.ok)
        }
        PlugState::Available if m.has_instructions() => (
            "Not installed. Enter shows each skill, squad and agent it adds, with what it may do, to read before you install it."
                .to_string(),
            t.dim,
        ),
        PlugState::Available => ("Not installed. Enter installs it.".to_string(), t.dim),
        PlugState::Installed if m.themes().is_empty() => (format!("Installed. {} d removes it.", crate::plugins::next_words(m)), t.ok),
        PlugState::Installed => ("Installed. Its themes are in the settings (s, then Theme). d removes it.".to_string(), t.ok),
        PlugState::Update => ("An update is ready: enter installs it. d removes the plugin.".to_string(), t.warn),
        PlugState::FromFolder => ("Installed from a folder on this computer. d removes it.".to_string(), t.ok),
        PlugState::NotInCatalog => {
            ("Installed, but the catalog doesn't have it any more. It keeps working; d removes it.".to_string(), t.subtext)
        }
        PlugState::Unavailable(why) => (format!("It can't be installed here: {why}"), t.warn),
    };
    for l in wrap(&what, w) {
        lines.push(Line::styled(l, Style::new().fg(color)));
    }
    for (k, line) in lines.into_iter().take(area.height as usize).enumerate() {
        f.render_widget(Paragraph::new(line), Rect::new(area.x, area.y + k as u16, area.width, 1));
    }
}

/// Rows the settings' Plugins tab needs: one per plugin, or one line saying there are none.
pub(super) fn plugin_settings_height(app: &App) -> usize {
    app.pl.rows().len().max(1)
}

/// How a plugin's state is said on its one line in the settings: "Installed", "Update ready", "Available", ...
pub(super) fn settings_state_words(app: &App, r: &PlugRow) -> &'static str {
    if editor_unfinished(app, r) {
        return "Installed, downloads not finished";
    }
    r.state.short_words()
}

/// The settings' Plugins tab: every plugin, installed or in the catalog, one line each (its name, its version dim,
/// and where it stands on the right). Returns the row below the last.
pub(super) fn draw_plugin_settings(f: &mut Frame, app: &App, area: Rect) -> u16 {
    let t = app.effective_theme();
    if area.is_empty() {
        return area.y;
    }
    let rows = app.pl.rows();
    if rows.is_empty() {
        let msg = "No plugins installed, and none from the catalog to show.";
        f.render_widget(
            Paragraph::new(Span::styled(fit_end(msg, area.width as usize), Style::new().fg(t.dim))),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return area.y + 1;
    }
    let room = (area.height as usize).max(1);
    app.pages.settings.set(room);
    let selected = app.setting.min(rows.len() - 1);
    let start = selected.saturating_sub(room - 1);
    let mut y = area.y;
    for (i, r) in rows.iter().enumerate().skip(start) {
        if y >= area.bottom() {
            break;
        }
        let row = Rect::new(area.x, y, area.width, 1);
        app.hits.borrow_mut().push((row, Hit::Setting(i)));
        let sel = i == selected;
        let bg = if sel || row.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let w = (area.width as usize).saturating_sub(2);
        let state = settings_state_words(app, r);
        let state_color = match r.state {
            _ if editor_unfinished(app, r) => t.warn,
            PlugState::Update | PlugState::Unavailable(_) => t.warn,
            ref s if s.installed() => t.ok,
            _ => t.dim,
        };
        // The name first, then the version when there is room for it, the state always.
        let name_room = w.saturating_sub(state.chars().count() + 2);
        let version = format!("  {}", r.manifest.version);
        let name = fit_end(&r.manifest.name, name_room);
        let version = if name.chars().count() + version.chars().count() <= name_room { version } else { String::new() };
        let gap = w.saturating_sub(name.chars().count() + version.chars().count() + state.chars().count());
        let line = Line::from(vec![
            Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)),
            Span::styled(name, base.fg(t.text).add_modifier(Modifier::BOLD)),
            Span::styled(version, base.fg(t.dim)),
            Span::styled(" ".repeat(gap), base),
            Span::styled(state, base.fg(state_color)),
        ]);
        f.render_widget(Paragraph::new(line).style(base), row);
        y += 1;
    }
    y
}

/// What a plugin with instructions adds, before it is installed: each skill, squad and agent with what it may do, and
/// the keys to read one in full, install it, or not. Reading one takes the whole screen.
pub(super) fn draw_plugin_review(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let Some(r) = app.pl.review.as_ref() else { return };
    // The review owns every click while it is open.
    app.hits.borrow_mut().clear();
    if let Some(i) = r.reading {
        return draw_review_item(f, app, r, i, area);
    }
    let rect = centered(area, area.width.saturating_sub(4).clamp(30, 100), area.height.saturating_sub(2).max(8));
    f.render_widget(Clear, rect);
    let block = modal_block(t, if r.update.is_some() { "update plugin" } else { "install plugin" });
    let inner = block.inner(rect).inner(Margin::new(1, 0));
    f.render_widget(block, rect);
    let w = inner.width as usize;
    let mut head: Vec<Line<'static>> = Vec::new();
    let question = match &r.update {
        Some((from, to)) => format!("Update {} from {from} to {to}?", r.name),
        None => format!("Install {}?", r.name),
    };
    head.push(Line::styled(fit_end(&question, w), Style::new().fg(t.text).add_modifier(Modifier::BOLD)));
    let says = if r.update.is_some() {
        "Its instructions changed. Read the ones marked new or changed before you update: enter reads the one selected."
    } else {
        "It adds instructions Claude follows with your permissions. Read each one before you install it: enter reads the one selected. It holds files only, and d on it (in the settings: s, then Plugins) takes it off again."
    };
    head.extend(wrap(says, w).into_iter().map(|l| Line::styled(l, Style::new().fg(t.dim))));
    head.push(Line::raw(""));
    let head_h = (head.len() as u16).min(inner.height.saturating_sub(2));
    f.render_widget(Paragraph::new(head), Rect::new(inner.x, inner.y, inner.width, head_h));
    let list = Rect::new(inner.x, inner.y + head_h, inner.width, inner.height.saturating_sub(head_h + 1));
    // Every item's lines, and where each starts.
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans_of: Vec<(usize, usize)> = Vec::new();
    for (i, it) in r.items.iter().enumerate() {
        let start = lines.len();
        let sel = i == r.selected;
        let base = Style::new().bg(if sel { t.surface } else { t.bg });
        let mark = match (r.update.is_some(), it.change) {
            (true, crate::pluginreview::Change::New) => " · new",
            (true, crate::pluginreview::Change::Changed) => " · changed",
            _ => "",
        };
        let kind = format!("{:<6} ", it.kind.word());
        let name_w = w.saturating_sub(2 + kind.chars().count() + mark.chars().count());
        let name = fit_end(&it.name, name_w);
        let about_w = name_w.saturating_sub(name.chars().count() + 3);
        let about = if about_w > 8 { format!("  {}", fit_end(&it.about, about_w)) } else { String::new() };
        let used = 2 + kind.chars().count() + name.chars().count() + mark.chars().count() + about.chars().count();
        lines.push(Line::from(vec![
            Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)),
            Span::styled(kind, base.fg(t.accent)),
            Span::styled(name, base.fg(t.text).add_modifier(Modifier::BOLD)),
            Span::styled(mark, base.fg(t.warn)),
            Span::styled(about, base.fg(t.dim)),
            Span::styled(" ".repeat(w.saturating_sub(used)), base),
        ]));
        for p in &it.permissions {
            for (k, part) in wrap(p, w.saturating_sub(6)).into_iter().enumerate() {
                let lead = if k == 0 { "    – " } else { "      " };
                lines.push(Line::from(vec![Span::styled(lead, Style::new().fg(t.dim)), Span::styled(part, Style::new().fg(t.subtext))]));
            }
        }
        spans_of.push((start, lines.len()));
        lines.push(Line::raw(""));
    }
    for g in &r.gone {
        for l in wrap(&format!("No longer in it: {g}"), w) {
            lines.push(Line::styled(l, Style::new().fg(t.warn)));
        }
    }
    let rows = list.height as usize;
    let mut top = r.top.get();
    if let Some(&(start, end)) = spans_of.get(r.selected) {
        if start < top {
            top = start;
        }
        if end > top + rows {
            top = end.saturating_sub(rows).min(start);
        }
    }
    top = top.min(lines.len().saturating_sub(rows));
    r.top.set(top);
    for (k, l) in lines.into_iter().skip(top).take(rows).enumerate() {
        f.render_widget(Paragraph::new(l), Rect::new(list.x, list.y + k as u16, list.width, 1));
    }
    let verb = if r.update.is_some() { "update" } else { "install" };
    let hints: [Hint; 4] =
        [("enter", "read it", None), ("y", verb, Some(Action::Confirm)), ("n", "not now", Some(Action::Cancel)), ("↑↓", "choose", None)];
    let foot = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
    draw_footer(f, app, foot, &hints, None);
}

/// One item of the review, read in full: its whole text as Markdown, scrolled, on the whole screen.
fn draw_review_item(f: &mut Frame, app: &App, r: &crate::app::Review, i: usize, area: Rect) {
    let t = app.effective_theme();
    let Some(it) = r.items.get(i) else { return };
    f.render_widget(Clear, area);
    let title = format!("{}: {} · from {}", it.kind.word(), it.name, r.name);
    let block = modal_block(t, &title);
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
    if inner.height < 3 {
        return;
    }
    let w = inner.width as usize;
    let lead = match (r.update.is_some(), it.change) {
        (true, crate::pluginreview::Change::Changed) => "Changed in this version. This is what Claude will follow:",
        (true, crate::pluginreview::Change::New) => "New in this version. This is what Claude will follow:",
        _ => "This is what Claude will follow, with your permissions:",
    };
    f.render_widget(Paragraph::new(Span::styled(fit_end(lead, w), Style::new().fg(t.dim))), Rect::new(inner.x, inner.y, inner.width, 1));
    let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(3));
    let lines = crate::mdview::render(&it.text, w, t, app.plain_marker);
    let rows = body.height as usize;
    let top = r.read_top.get().min(lines.len().saturating_sub(rows));
    r.read_top.set(top);
    r.rows.set(rows);
    let total = lines.len();
    for (k, l) in lines.into_iter().skip(top).take(rows).enumerate() {
        f.render_widget(Paragraph::new(l), Rect::new(body.x, body.y + k as u16, body.width, 1));
    }
    let shown =
        if total <= rows { "all of it shown".to_string() } else { format!("lines {}-{} of {total}", top + 1, (top + rows).min(total)) };
    let hints: [Hint; 2] = [("↑↓ pgup pgdn", "scroll", None), ("esc", "back to the list", None)];
    draw_footer(f, app, Rect::new(inner.x, inner.bottom() - 1, inner.width, 1), &hints, Some((shown, t.dim)));
}
