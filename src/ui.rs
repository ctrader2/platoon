use crate::app::{split_areas, Action, AdoptKind, App, AssignOpt, Confirm, FindState, HandoffState, Hit, Mode, NamePurpose, PStat, Pane, PickKind, PickOpt, TRow, FIND_KIND, PTABS, SEARCH_KINDS, SPLIT_MIN_WIDTH, TABS, TOPS};
use crate::tickets::{self, State, Ticket};
use crate::dirs::tilde;
use crate::client::Client;
use crate::select::{Selection, Span as Cells};
use crate::model::{NameSource, Session, Status};
use crate::settings::{Kind, Step, TABS as SETTINGS_TABS};
use crate::usage::Usage;
use crate::theme::{contrast, Theme};
mod squads_view;

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

/// The summary strip needs this many rows inside the frame (header, rule, column titles, footer, the strip and three
/// rows of the list); on a shorter terminal it is left out.
const STRIP_MIN_HEIGHT: u16 = 15;

/// The selected conversation's summary in up to two lines, dim, with the generated-name marker in front of the first.
fn draw_summary_strip(f: &mut Frame, app: &App, text: &str, area: Rect) {
    let t = app.effective_theme();
    let marker = app.marker();
    let width = (area.width as usize).saturating_sub(3);
    let mut lines = wrap(text, width);
    if lines.len() > 2 {
        lines.truncate(2);
        let last = lines.pop().unwrap_or_default();
        lines.push(fit_end(&format!("{last}…"), width));
    }
    for (i, line) in lines.iter().enumerate() {
        let lead = if i == 0 { Span::styled(format!("{marker} "), Style::new().fg(t.dim)) } else { Span::raw("  ") };
        f.render_widget(Paragraph::new(Line::from(vec![lead, Span::styled(line.clone(), Style::new().fg(t.subtext))])), Rect::new(area.x, area.y + i as u16, area.width, 1));
    }
}

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
        if let Some(n) = app.split_showing() {
            draw_split(f, app, client, n);
            return;
        }
        let dir = app.hosts.iter().find(|h| h.claude_pid == *pid).map(|h| h.dir.as_str()).unwrap_or("");
        draw_attached(f, app, client, dir, inner);
        return;
    }
    if let (Mode::EditNotes, Some(edit)) = (&app.mode, &app.editing) {
        draw_editing(f, app, edit, inner);
        return;
    }

    // The selected conversation's summary, two lines just above the footer, when there is one and the terminal is
    // tall enough to keep at least three rows of the list beside it. (Not on the Projects tab, whose rows are projects.)
    let summary = app.selected_session().map(|s| s.summary.as_str()).filter(|s| !s.is_empty() && app.open_project.is_none() && !app.squads_tab_showing());
    let strip_h = if summary.is_some() && inner.height >= STRIP_MIN_HEIGHT { 2 } else { 0 };
    let term_w = f.area().width;
    let [header, rule, shelf, titles, list, strip, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(strip_h),
        Constraint::Length(1),
    ])
    .areas(inner.inner(Margin::new(2, 0)));
    if let (Some(text), true) = (summary, strip_h > 0) {
        draw_summary_strip(f, app, text, strip);
    }

    let tabs = draw_header(f, app, header);
    draw_tab_rule(f, t, rule, &tabs);
    let hints: Vec<Hint> = if let (Some(_), Some(n)) = (&app.open_project, app.tk.open) {
        // A ticket's own screen takes the whole body; one with a squad has its session's place beside it.
        let body = Rect::new(shelf.x, shelf.y, shelf.width, list.bottom() - shelf.y);
        // In the line for a free squad, its squad's place beside it is not there yet (an earlier squad's isn't its).
        if app.ticket(n).is_some_and(|t| t.leader().is_some() && t.queued.is_none()) {
            draw_ticket_with_squad(f, app, n, body, term_w);
        } else {
            draw_ticket_screen(f, app, n, body);
        }
        ticket_screen_hints(app, n)
    } else if app.open_project.is_some() {
        draw_project_screen(f, app, shelf, Rect::new(titles.x, titles.y, titles.width, list.bottom() - titles.y));
        project_screen_hints(app)
    } else if app.squads_tab_showing() {
        squads_view::draw_squads_tab(f, app, Rect::new(shelf.x, shelf.y, shelf.width, list.bottom() - shelf.y));
        squads_view::squads_tab_hints(app)
    } else if app.projects_list_showing() {
        let labels: Vec<&str> = PTABS.iter().map(|(l, _)| *l).collect();
        draw_shelf(f, app, shelf, &labels, &app.pcounts, app.ptab);
        draw_project_columns(f, t, app, titles);
        if app.project_rows().is_empty() {
            let msg = if app.pcounts[app.ptab.min(1)] > 0 { "No projects match the filter" } else { PTABS[app.ptab.min(1)].1 };
            let p = Paragraph::new(msg).style(Style::new().fg(t.dim)).alignment(Alignment::Center);
            f.render_widget(p, Rect::new(list.x, list.y + list.height / 3, list.width, 1));
        } else {
            draw_project_list(f, app, list);
        }
        projects_hints(app)
    } else {
        let labels: Vec<&str> = TABS.iter().map(|t| t.label).collect();
        draw_shelf(f, app, shelf, &labels, &app.counts, app.tab);
        let visible = app.visible();
        // The PROJECT column is sized from every row on the tab (so it doesn't jump while the filter is
        // typed); NAME and DIR from the rows actually listed.
        let project_w = project_width(app, app.tab_rows(), term_w);
        let c = cols(titles.width, project_w, content_widths(app, visible));
        draw_columns(f, t, titles, &c);
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
            draw_list(f, app, visible, &c, term_w, list);
        }
        sessions_hints(app)
    };
    let hints = match app.mode {
        Mode::Search => vec![("enter", "apply", Some(Action::Confirm)), ("esc", "clear", Some(Action::Cancel))],
        _ => hints,
    };
    // The mouse and the obvious keys need no reminder: when the line is short of room (a narrow terminal, or a
    // notice sharing it) those hints go first.
    let notice = app.active_notice();
    let room = (footer.width as usize).saturating_sub(notice.map_or(0, |n| n.chars().count() + 6));
    let hints = fit_hints(hints, room, &["tabs", "active/archived", "move", "hand-off", "project", "panes", "rename", "settings", "scroll", "new session", "search", "folders", "waits for", "edit", "back to the list", "details", "copy", "remove", "member"]);
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
        Mode::Confirm(c) => {
            if matches!(c, Confirm::ResetInstructions { .. }) {
                draw_settings_modal(f, app, inner);
            }
            draw_confirm(f, app, c, inner)
        }
        Mode::Settings => draw_settings_modal(f, app, inner),
        Mode::Search => draw_search(f, app, inner),
        Mode::Rename => draw_rename(f, app, inner),
        Mode::ProjectName => draw_name_box(f, app, inner),
        Mode::Assign => draw_assign(f, app, inner),
        Mode::Look => draw_look(f, app, inner),
        Mode::PickEditor => {
            if app.epick.instructions.is_some() {
                draw_settings_modal(f, app, inner);
            }
            draw_editor_pick(f, app, inner)
        }
        Mode::Changes => draw_changes(f, app, inner),
        Mode::Handoff => draw_handoff(f, app, inner),
        Mode::TicketPick => draw_ticket_pick(f, app, inner),
        Mode::SquadPick => draw_squad_pick(f, app, inner),
        Mode::SquadChoice => squads_view::draw_squad_choice(f, app, inner),
        Mode::SquadText => squads_view::draw_squad_text(f, app, inner),
        // No fade here: the point is to see the real screen in the colors being tried.
        Mode::Themes => draw_theme_list(f, app, inner),
        _ => {}
    }
}

fn sessions_hints(app: &App) -> Vec<Hint<'static>> {
    let archive_verb = if app.selected_session().is_some_and(|s| s.archived) { "restore" } else { "archive" };
    vec![
        ("enter", "open", Some(Action::Jump)),
        ("n", "new", Some(Action::New)),
        ("a", archive_verb, Some(Action::Archive)),
        ("m", "project", Some(Action::Assign)),
        ("h", "hand-off", Some(Action::HandOff)),
        ("/", "search", Some(Action::Search)),
        ("r", "rename", Some(Action::Rename)),
        ("←→", "active/archived", None),
        ("1-3", "tabs", None),
        ("↑↓", "move", None),
        ("s", "settings", Some(Action::Settings)),
        ("q", "quit", Some(Action::Quit)),
    ]
}

fn projects_hints(app: &App) -> Vec<Hint<'static>> {
    let archive_verb = if app.selected_project().is_some_and(|p| p.archived) { "restore" } else { "archive" };
    vec![
        ("enter", "open", Some(Action::Jump)),
        ("n", "new", Some(Action::New)),
        ("a", archive_verb, Some(Action::Archive)),
        ("r", "rename", Some(Action::Rename)),
        ("c", "color", Some(Action::Look)),
        ("d", "delete", Some(Action::Delete)),
        ("/", "search", Some(Action::Search)),
        ("←→", "active/archived", None),
        ("1-3", "tabs", None),
        ("↑↓", "move", None),
        ("s", "settings", Some(Action::Settings)),
        ("q", "quit", Some(Action::Quit)),
    ]
}

fn project_screen_hints(app: &App) -> Vec<Hint<'static>> {
    let archive_verb = if app.selected_session().is_some_and(|s| s.archived) { "restore" } else { "archive" };
    let mut out = vec![("enter", "open", Some(Action::Jump)), ("n", "new session", Some(Action::New))];
    if app.pane == Pane::Sessions {
        out.push(("a", archive_verb, Some(Action::Archive)));
        out.push(("m", "project", Some(Action::Assign)));
        out.push(("h", "hand-off", Some(Action::HandOff)));
    } else if app.pane == Pane::Tickets {
        let on_divider = matches!(app.ticket_rows().get(app.ticket_selected()), Some(TRow::Divider(_)));
        out = vec![("enter", if on_divider { "show/hide" } else { "open" }, None), ("n", "new ticket", None)];
        if let Some(t) = app.selected_ticket() {
            if t.state == State::Ready {
                out.push(("p", "let it publish", None));
            } else if handable(app, t) {
                out.push(("a", "hand to a squad", None));
            }
            out.extend([("s", "state", None), ("e", "edit", None)]);
        }
    } else {
        out = vec![
            ("e", "edit notes", Some(Action::Edit)),
            ("c", "changes", Some(Action::ShowChanges)),
            ("u", "undo", Some(Action::Undo)),
            ("↑↓", "scroll", None),
            ("n", "new session", Some(Action::New)),
        ];
    }
    out.extend([
        ("/", "search", Some(Action::Search)),
        ("tab shift+arrows", "panes", None),
        ("esc", "back", Some(Action::Leave)),
        ("q", "quit", Some(Action::Quit)),
    ]);
    out
}

/// A ticket `a` would hand to a squad now (the hint is only offered then; `a` says why not otherwise).
fn handable(app: &App, t: &Ticket) -> bool {
    t.state == State::New && tickets::is_ready(t, app.tickets())
}

/// The keys of a ticket's own screen.
fn ticket_screen_hints(app: &App, n: u32) -> Vec<Hint<'static>> {
    let t = app.ticket(n);
    let mut out: Vec<Hint<'static>> = Vec::new();
    if t.is_some_and(|t| t.leader().is_some() && t.queued.is_none()) {
        out.push(("enter", if app.leader_running(n) { "the squad's session" } else { "continue the squad" }, None));
    }
    if let Some(t) = t.filter(|t| t.from.is_some()) {
        out.push((app.ticket_source_key(t), "where it came from", None));
    }
    if t.is_some_and(|t| handable(app, t)) {
        out.push(("a", "hand to a squad", None));
    }
    if t.is_some_and(|t| t.state == State::Ready) {
        out.push(("p", "let it publish", None));
    }
    out.extend([
        ("e", "edit", None),
        ("r", "rename", None),
        ("s", "state", None),
        ("f", "folders", None),
        ("b", "waits for", None),
        ("↑↓", "scroll", None),
        ("esc", "back", Some(Action::Leave)),
        ("q", "quit", Some(Action::Quit)),
    ]);
    out
}

/// The line under the rule: Active and Archived of the tab showing, with their counts; the arrows switch them.
fn draw_shelf(f: &mut Frame, app: &App, area: Rect, labels: &[&str], counts: &[usize], current: usize) {
    let t = app.effective_theme();
    let mut spans = Vec::new();
    let mut x = area.x;
    for (i, label) in labels.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
            x += 1;
        }
        let label = format!("{label} ");
        let count = counts.get(i).copied().unwrap_or(0);
        let w = chip_width("", &label, count);
        let rect = Rect::new(x, area.y, w, 1).intersection(area);
        if !rect.is_empty() {
            app.hits.borrow_mut().push((rect, Hit::Tab(i)));
        }
        let hover = i != current && rect.contains(app.mouse.into());
        spans.extend(tab_chip(t, app.rounded, "", &label, count, i == current, hover));
        x += w;
    }
    spans.push(Span::styled("  ←→", Style::new().fg(t.dim)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A project's badge: its letters on its color.
fn badge_span(t: &Theme, p: &crate::projects::Project) -> Span<'static> {
    Span::styled(format!(" {} ", p.badge), Style::new().fg(crate::projects::badge_text(t)).bg(crate::projects::slot_color(t, &p.color)).add_modifier(Modifier::BOLD))
}

fn badge_width(p: &crate::projects::Project) -> usize {
    p.badge.chars().count() + 2
}

/// `3 active · 1 archived`
fn session_counts(st: &PStat) -> String {
    match (st.active, st.archived) {
        (0, 0) => "no sessions".into(),
        (a, 0) => format!("{a} active"),
        (0, b) => format!("{b} archived"),
        (a, b) => format!("{a} active · {b} archived"),
    }
}

struct PCols {
    name: usize,
    sessions: usize,
    notes: usize,
    /// The spare width of a wide terminal, shared between these columns (same rule as the Sessions table).
    pad: PPad,
}

#[derive(Default, Clone, Copy)]
struct PPad {
    name: usize,
    sessions: usize,
    notes: usize,
    last: usize,
}

/// How much room the PROJECT column of the Projects list would like: the longest badge-and-name on show
/// plus a gap, never less than its own title, never more than the cap.
fn project_name_width(app: &App) -> usize {
    let all = app.projects.all();
    let longest = app
        .project_rows()
        .iter()
        .map(|&i| {
            let p = &all[i];
            badge_width(p) + 1 + p.name.chars().count()
        })
        .max()
        .unwrap_or(0);
    (longest.max("PROJECT".chars().count()) + COL_GAP).min(NAME_MAX)
}

/// The Projects list's columns. `want` is what the names on show need; the column is that wide unless the
/// terminal is too narrow, in which case it takes whatever room is left, exactly as it always did. What a
/// wide terminal has over is shared equally between the columns, as on the Sessions table.
fn project_cols(width: u16, want: usize) -> PCols {
    let w = width as usize;
    let fixed = 3 + LAST_W;
    let notes = if w >= 70 { 14 } else { 0 };
    let sessions = if w >= 50 { 24 } else { 0 };
    let name = want.min(w.saturating_sub(fixed + notes + sessions));
    let spare = w.saturating_sub(fixed + notes + sessions + name);
    let s = spread(spare, [true, sessions > 0, notes > 0, true]);
    PCols { name, sessions, notes, pad: PPad { name: s[0], sessions: s[1], notes: s[2], last: s[3] } }
}

fn draw_project_columns(f: &mut Frame, t: &Theme, app: &App, area: Rect) {
    let c = project_cols(area.width, project_name_width(app));
    let s = Style::new().fg(t.dim);
    let line = Line::from(vec![
        Span::raw("   "),
        Span::styled(fit("PROJECT", c.name + c.pad.name), s),
        Span::styled(fit(if c.sessions > 0 { "SESSIONS" } else { "" }, c.sessions + c.pad.sessions), s),
        Span::styled(fit(if c.notes > 0 { "NOTES" } else { "" }, c.notes + c.pad.notes), s),
        Span::styled(format!("{:>w$}", "LAST", w = LAST_W - 1 + c.pad.last), s),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// A project row's description, wrapped at word boundaries into the room beside its indent: as many lines
/// as it needs, and one empty line when there is none yet, so a row is never shorter than it always was.
fn project_desc_lines(p: &crate::projects::Project, width: u16) -> Vec<String> {
    let room = (width as usize).saturating_sub(project_desc_indent(p) + 1);
    let lines = if room == 0 { Vec::new() } else { wrap(&p.description, room) };
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

/// Where a project row's description starts: under the name, past the badge.
fn project_desc_indent(p: &crate::projects::Project) -> usize {
    2 + badge_width(p) + 1
}

/// The first row to draw so that the selected one is fully on screen, given each row's height. As before,
/// the list only scrolls when the selection would otherwise fall off the bottom, and then the selection is
/// the last row shown. A row's spacer line may be left off the bottom; its text never is.
fn project_list_start(heights: &[u16], selected: usize, room: u16) -> usize {
    let Some(&h) = heights.get(selected) else { return 0 };
    let mut used = h.saturating_sub(1);
    let mut start = selected;
    while start > 0 && used + heights[start - 1] <= room {
        used += heights[start - 1];
        start -= 1;
    }
    start
}

fn draw_project_list(f: &mut Frame, app: &App, area: Rect) {
    // A row is the name line, the description (one line, or more when a long one wraps; blank until there is
    // one), then a spacer. Rows differ in height, so where each one goes is worked out from the descriptions.
    let t = app.effective_theme();
    let now = crate::usage::now() * 1000;
    let c = project_cols(area.width, project_name_width(app));
    let all = app.projects.all();
    let shown = app.project_rows();
    let descs: Vec<Vec<String>> = shown.iter().map(|&idx| project_desc_lines(&all[idx], area.width)).collect();
    let heights: Vec<u16> = descs.iter().map(|d| d.len() as u16 + 2).collect();
    let start = project_list_start(&heights, app.pselected, area.height);
    let mut y = area.y;
    for (i, &idx) in shown.iter().enumerate().skip(start) {
        let p = &all[idx];
        let h = heights[i];
        // A row whose text would not fit is left for scrolling to (the first one is drawn whatever happens).
        if y >= area.bottom() || (i > start && y + h - 1 > area.bottom()) {
            break;
        }
        let row = Rect::new(area.x, y, area.width, h.min(area.bottom() - y));
        y += h;
        app.hits.borrow_mut().push((row, Hit::ProjectRow(i)));
        let selected = i == app.pselected;
        let bg = if selected || row.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let text = if p.archived { t.dim } else { t.text };
        let st = app.pstats.get(&p.id).cloned().unwrap_or_default();
        let lead = if selected { Span::styled("▎", base.fg(t.accent)) } else { Span::styled(" ", base) };
        let bw = badge_width(p) + 1;
        let notes = match st.notes_at {
            Some(at) => format!("notes {}", ago(now, at)),
            None => "no notes".into(),
        };
        let l1 = Line::from(vec![
            lead.clone(),
            Span::styled(" ", base),
            badge_span(t, p),
            Span::styled(" ", base),
            Span::styled(fit(&p.name, c.name.saturating_sub(bw)), base.fg(text).add_modifier(Modifier::BOLD)),
            gap(c.pad.name, base),
            Span::styled(fit(&session_counts(&st), c.sessions), base.fg(t.subtext)),
            gap(c.pad.sessions, base),
            Span::styled(fit(if c.notes > 0 { &notes } else { "" }, c.notes), base.fg(t.dim)),
            gap(c.pad.notes, base),
            // A row starts one cell to the left of the titles (the badge carries its own leading space), so
            // LAST gets that cell back here and the times line up under the title, at the row's right edge.
            Span::styled(format!("{:>w$}", ago(now, st.last_active), w = LAST_W + c.pad.last), base.fg(t.dim)),
        ]);
        f.render_widget(Paragraph::new(l1).style(base), Rect::new(row.x, row.y, row.width, 1));
        // Under the name: what platoon wrote the project is, in full, each line starting under the name.
        let indent = project_desc_indent(p);
        for (n, d) in descs[i].iter().enumerate() {
            let ly = row.y + 1 + n as u16;
            if ly >= row.bottom() {
                break;
            }
            let line = Line::from(vec![lead.clone(), gap(indent - 1, base), Span::styled(d.clone(), base.fg(t.dim))]);
            f.render_widget(Paragraph::new(line).style(base), Rect::new(row.x, ly, row.width, 1));
        }
        if row.height == h {
            f.render_widget(Block::default().style(Style::new().bg(t.bg)), Rect::new(row.x, row.bottom() - 1, row.width, 1));
        }
    }
}

/// A project's screen: its name and description at the top, its sessions on the left and its notes on the right.
fn draw_project_screen(f: &mut Frame, app: &App, title: Rect, body: Rect) {
    let t = app.effective_theme();
    let Some(p) = app.open_project() else { return };
    let st = app.pstats.get(&p.id).cloned().unwrap_or_default();
    let mut spans = vec![badge_span(t, p), Span::raw(" "), Span::styled(p.name.clone(), Style::new().fg(t.text).add_modifier(Modifier::BOLD))];
    if p.archived {
        spans.push(Span::styled("  archived", Style::new().fg(t.warn)));
    }
    spans.push(Span::styled(format!("   {}", session_counts(&st)), Style::new().fg(t.dim)));
    if !app.filter.is_empty() {
        spans.push(Span::styled(format!("   filter: {}", app.filter), Style::new().fg(t.accent)));
        spans.push(Span::styled("  (esc clears)", Style::new().fg(t.dim)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), title);
    // The description across the top, in full: as many lines as it needs, the panes below it. Until platoon has
    // written one for this project, the line stays empty.
    let lines: Vec<Line> = wrap(&p.description, body.width as usize).into_iter().map(|l| Line::styled(l, Style::new().fg(t.subtext))).collect();
    let [desc, panes] = Layout::vertical([Constraint::Length(lines.len().max(1) as u16), Constraint::Min(1)]).areas(body);
    if !lines.is_empty() {
        f.render_widget(Paragraph::new(lines), desc);
    }
    // Below 100 columns the left column (sessions and tickets) gets half, so a ticket's row keeps its title and
    // what it waits for; wider, the notes get a little more.
    let left_w = if panes.width >= 100 { panes.width * 45 / 100 } else { panes.width / 2 };
    let [left_col, right] = Layout::horizontal([Constraint::Length(left_w), Constraint::Min(1)]).areas(panes);
    // The left column: Sessions over Tickets, half each (Sessions gets the odd line).
    let [left, lower] = Layout::vertical([Constraint::Length(left_col.height - left_col.height / 2), Constraint::Min(0)]).areas(left_col);
    let pane_block = |title: &'static str, focused: bool| {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(if focused { t.accent } else { t.border }))
            .title(Span::styled(format!(" {title} "), Style::new().fg(if focused { t.accent } else { t.dim }).add_modifier(Modifier::BOLD)))
            .style(Style::new().bg(t.bg))
    };
    let lb = pane_block("Sessions", app.pane == Pane::Sessions);
    let li = lb.inner(left).inner(Margin::new(1, 0));
    f.render_widget(lb, left);
    draw_project_sessions(f, app, li);
    let tb = pane_block("Tickets", app.pane == Pane::Tickets);
    let ti = tb.inner(lower).inner(Margin::new(1, 0));
    f.render_widget(tb, lower);
    draw_project_tickets(f, app, ti);
    let rb = pane_block("Notes", app.pane == Pane::Notes);
    let ri = rb.inner(right).inner(Margin::new(1, 0));
    f.render_widget(rb, right);
    draw_notes(f, app, ri);
    let mut hits = app.hits.borrow_mut();
    // After the rows (drawn above), so a click on a row finds the row first.
    hits.push((left, Hit::Pane(Pane::Sessions)));
    hits.push((lower, Hit::Pane(Pane::Tickets)));
    hits.push((right, Hit::Pane(Pane::Notes)));
}

/// A state's color: what is waiting on the user stands out, what is finished steps back.
pub fn state_color(t: &Theme, s: &State) -> Color {
    match s {
        State::New => t.text,
        State::Working => t.busy,
        State::Waiting => t.warn,
        State::Ready => t.accent,
        State::Done => t.ok,
        State::Cancelled => t.dim,
        State::Other(_) => t.subtext,
    }
}

/// The widths of the Tickets pane's columns. Zero means the column is left out.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) struct TCols {
    pub num: usize,
    pub title: usize,
    pub state: usize,
    pub tail: usize,
}

/// The title never gets fewer cells than this while another column is still shown.
const TICKET_TITLE_MIN: usize = 12;

/// How the Tickets pane's `width` is shared: the number, then the title (whatever is left), then the state and
/// "waits for / ready", each as wide as its longest value (`want`). When the title would get fewer than
/// `TICKET_TITLE_MIN` cells, the state goes first (its words then move into the last column on the rows that have
/// nothing else to say there, see `ticket_tail`), then the last column.
pub(crate) fn ticket_cols(width: usize, num: usize, want_state: usize, want_tail: usize) -> TCols {
    let fixed = 1 + num + 1;
    let (mut state, mut tail) = (want_state.min(21), want_tail.min(24));
    let used = |s: usize, t: usize| fixed + [s, t].iter().filter(|w| **w > 0).map(|w| w + 2).sum::<usize>();
    if width < used(state, tail) + TICKET_TITLE_MIN {
        state = 0;
    }
    if width < used(state, tail) + TICKET_TITLE_MIN {
        // The last column is cut (with `…`) before it goes: "waits for #2, …" still says the most.
        let room = width.saturating_sub(fixed + 2 + TICKET_TITLE_MIN);
        tail = if room >= 8 { tail.min(room) } else { 0 };
    }
    let title = width.saturating_sub(used(state, tail));
    TCols { num, title, state, tail }
}

/// What a ticket's row says after its state: the tickets it still waits for, or a quiet "ready". `with_state`: the
/// state has no column of its own (a narrow pane), so a row with nothing else to say there says its state, unless
/// that is plain New.
fn ticket_tail(t: &Ticket, all: &[Ticket], with_state: bool) -> (String, bool) {
    // Being handed to a squad: what is happening says the most.
    if let Some(busy) = &t.busy {
        return (busy.clone(), false);
    }
    if t.queued.is_some() {
        return (tickets::QUEUED_WORDS.to_string(), false);
    }
    if t.state.closed() {
        return (if with_state { t.state.label().to_string() } else { String::new() }, false);
    }
    let wait = tickets::waiting_for(t, all);
    if !wait.is_empty() {
        return (format!("waits for {}", wait.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", ")), false);
    }
    if tickets::is_ready(t, all) {
        return ("ready".into(), true);
    }
    if with_state && t.state != State::New {
        return (t.state.label().to_string(), false);
    }
    (String::new(), false)
}

/// The Tickets pane: the open tickets, then "Done and cancelled · N" (folded until enter opens it).
fn draw_project_tickets(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    if area.height == 0 || area.width == 0 {
        return;
    }
    if app.tickets_store().is_some_and(|s| s.is_corrupt()) {
        f.render_widget(Paragraph::new(tickets::CORRUPT).wrap(Wrap { trim: true }).style(Style::new().fg(t.warn)), area);
        return;
    }
    let all = app.tickets();
    let rows = app.ticket_rows();
    if rows.is_empty() {
        f.render_widget(Paragraph::new("No tickets yet · n to make one").wrap(Wrap { trim: true }).style(Style::new().fg(t.dim)), area);
        return;
    }
    let shown: Vec<&Ticket> = rows.iter().filter_map(|r| if let TRow::Ticket(n) = r { all.iter().find(|x| x.number == *n) } else { None }).collect();
    let num = shown.iter().map(|x| format!("#{}", x.number).len()).max().unwrap_or(2);
    let want_state = shown.iter().map(|x| x.state.label().chars().count()).max().unwrap_or(0);
    // What each row's squads spent, short ("1.2M tokens"), on the rows that have it.
    let tokens: std::collections::HashMap<u32, String> = shown.iter().filter_map(|x| Some((x.number, crate::squadwork::pane_total(&app.ticket_tokens(x)?)))).collect();
    let tail_of = |x: &Ticket, with_state: bool| -> (String, bool) {
        let (tail, ready) = ticket_tail(x, all, with_state);
        match tokens.get(&x.number) {
            Some(n) if tail.is_empty() => (n.clone(), ready),
            Some(n) => (format!("{tail} · {n}"), ready),
            None => (tail, ready),
        }
    };
    let want_tail = |with_state: bool| shown.iter().map(|x| tail_of(x, with_state).0.chars().count()).max().unwrap_or(0);
    let mut c = ticket_cols(area.width as usize, num, want_state, want_tail(false));
    // No room for the state's own column: the rows that have nothing else to say at the end say their state there.
    let merged = c.state == 0 && want_state > 0;
    if merged {
        c = ticket_cols(area.width as usize, num, 0, want_tail(true));
    }
    let selected = app.ticket_selected();
    let height = area.height as usize;
    let top = selected.saturating_sub(height.saturating_sub(1)).min(rows.len().saturating_sub(height));
    let plain = app.plain_marker;
    for (i, row) in rows.iter().enumerate().skip(top).take(height) {
        let rect = Rect::new(area.x, area.y + (i - top) as u16, area.width, 1);
        app.hits.borrow_mut().push((rect, Hit::TicketRow(i)));
        let sel = i == selected;
        let focused = app.pane == Pane::Tickets;
        let bg = if (sel && focused) || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let lead = Span::styled(if sel { "▎" } else { " " }, base.fg(t.accent));
        let line = match row {
            TRow::Divider(n) => {
                let mark = match (app.tk.unfolded, plain) {
                    (true, false) => "▾",
                    (false, false) => "▸",
                    (true, true) => "-",
                    (false, true) => "+",
                };
                Line::from(vec![lead, Span::styled(fit_end(&format!("{mark} Done and cancelled · {n}"), rect.width as usize - 1), base.fg(t.subtext).add_modifier(Modifier::BOLD))])
            }
            TRow::Ticket(n) => {
                let Some(x) = all.iter().find(|x| x.number == *n) else { continue };
                let closed = x.state.closed();
                let mut spans = vec![
                    lead,
                    Span::styled(format!("{:<w$} ", format!("#{}", x.number), w = c.num), base.fg(t.dim)),
                    Span::styled(fit(&x.title, c.title), base.fg(if closed { t.dim } else { t.text })),
                ];
                if c.state > 0 {
                    spans.push(Span::styled(format!("  {}", fit(x.state.label(), c.state)), base.fg(state_color(t, &x.state))));
                }
                if c.tail > 0 {
                    let (tail, ready) = tail_of(x, merged);
                    let color = if ready {
                        t.ok
                    } else if tail.starts_with("waits for") {
                        t.warn
                    } else if tokens.get(&x.number) == Some(&tail) {
                        t.subtext
                    } else {
                        state_color(t, &x.state)
                    };
                    spans.push(Span::styled(format!("  {}", fit(&tail, c.tail)), base.fg(color)));
                }
                Line::from(spans)
            }
        };
        f.render_widget(Paragraph::new(line).style(base), rect);
    }
}

/// A ticket's own screen, in `area`: its number and title, its state, its squad, its folders and what it
/// waits for, then its body drawn from the Markdown, scrolled with the arrows. It takes any `area`, so a split view
/// can put a session beside it.
fn draw_ticket_screen(f: &mut Frame, app: &App, number: u32, area: Rect) {
    let t = app.effective_theme();
    let Some(x) = app.ticket(number) else { return };
    let all = app.tickets();
    let w = area.width as usize;
    let mut head: Vec<Line> = Vec::new();
    let title = format!("#{} {}", x.number, x.title);
    for (i, l) in wrap(&title, w).into_iter().enumerate() {
        let style = Style::new().fg(t.text).add_modifier(Modifier::BOLD);
        if i == 0 {
            let rest = l.strip_prefix(&format!("#{}", x.number)).unwrap_or(&l).to_string();
            head.push(Line::from(vec![Span::styled(format!("#{}", x.number), style.fg(t.accent)), Span::styled(rest, style)]));
        } else {
            head.push(Line::styled(l, style));
        }
    }
    if let Some(p) = app.open_project() {
        head.push(Line::from(vec![Span::styled("in ", Style::new().fg(t.dim)), badge_span(t, p), Span::styled(format!(" {}", p.name), Style::new().fg(t.subtext))]));
    }
    head.push(Line::raw(""));
    const LABEL: usize = 12;
    let label = |s: &str| Span::styled(format!("{s:<LABEL$}"), Style::new().fg(t.dim));
    let (tail, ready) = ticket_tail(x, all, false);
    let mut state = vec![label("State"), Span::styled(x.state.label().to_string(), Style::new().fg(state_color(t, &x.state)).add_modifier(Modifier::BOLD))];
    if !tail.is_empty() {
        state.push(Span::styled(format!(" · {tail}"), Style::new().fg(if ready { t.ok } else { t.warn })));
    }
    head.push(Line::from(state));
    // The session it was made in (`/to-tickets`): its name, and the key that opens it.
    if let Some(from) = &x.from {
        let hint = format!("  {} opens it", app.ticket_source_key(x));
        let room = w.saturating_sub(LABEL + hint.chars().count());
        head.push(Line::from(vec![
            label("From"),
            Span::styled(fit_end(&app.ticket_source_name(from), room.max(8)), Style::new().fg(t.text)),
            Span::styled(hint, Style::new().fg(t.dim)),
        ]));
    }
    // What the squad said when it stopped: its question, why it is stuck, what it wants to publish.
    if !x.reason.is_empty() {
        for (i, l) in wrap(&x.reason, w.saturating_sub(LABEL)).into_iter().enumerate() {
            head.push(Line::from(vec![label(if i == 0 { "Squad says" } else { "" }), Span::styled(l, Style::new().fg(t.warn))]));
        }
    }
    // Why the last hand-over to a squad didn't happen: the commander's word, or what went wrong after it.
    if let Some(h) = x.handover.as_ref().filter(|_| x.busy.is_none()) {
        let who = if h.kind == tickets::HandoverKind::NotStarted { "Not started" } else { "Commander" };
        for (i, l) in wrap(&h.says, w.saturating_sub(LABEL)).into_iter().enumerate() {
            head.push(Line::from(vec![label(if i == 0 { who } else { "" }), Span::styled(l, Style::new().fg(t.warn))]));
        }
    }
    // Which squad has it: "Coding squad" (whether its session runs shows beside the ticket, or as the enter hint).
    if let Some(l) = x.leader() {
        let name = app.squad_name(&l.squad);
        let name = name.as_str();
        let mut spans = vec![Span::styled(fit_end(&format!("{name} squad"), w), Style::new().fg(t.text).add_modifier(Modifier::BOLD))];
        if x.go_ahead {
            spans.push(Span::styled(fit_end(" · you let it publish", w.saturating_sub(name.chars().count() + 6)), Style::new().fg(t.warn)));
        }
        head.push(Line::from(spans));
    }
    for (i, wt) in x.worktrees.iter().enumerate() {
        let shown = fit_start(&format!("{} ({})", tilde(&wt.path, &app.home), wt.branch), w.saturating_sub(LABEL));
        head.push(Line::from(vec![label(if i == 0 { "Worktree" } else { "" }), Span::styled(shown, Style::new().fg(t.text))]));
    }
    if x.folders.is_empty() {
        head.push(Line::from(vec![label("Folders"), Span::styled("No folder", Style::new().fg(t.subtext))]));
    }
    for (i, folder) in x.folders.iter().enumerate() {
        let shown = fit_start(&tilde(folder, &app.home), w.saturating_sub(LABEL));
        head.push(Line::from(vec![label(if i == 0 { "Folders" } else { "" }), Span::styled(shown, Style::new().fg(t.text))]));
    }
    // The private work folder platoon keeps for the ticket, once a squad has had it.
    if x.leader().is_some() {
        if let Some(store) = app.tickets_store() {
            let work = tickets::work_dir(&app.config, store.project(), x.number);
            let shown = fit_start(&tilde(&work.to_string_lossy(), &app.home), w.saturating_sub(LABEL));
            head.push(Line::from(vec![label("Work folder"), Span::styled(shown, Style::new().fg(t.text))]));
        }
    }
    if x.blocked_by.is_empty() {
        head.push(Line::from(vec![label("Waits for"), Span::styled("nothing", Style::new().fg(t.subtext))]));
    }
    for (i, b) in x.blocked_by.iter().enumerate() {
        let what = match all.iter().find(|o| o.number == *b) {
            Some(o) => format!("#{b} {} · {}", o.title, o.state.label()),
            None => format!("#{b} (not in this project)"),
        };
        let done = all.iter().any(|o| o.number == *b && o.state == State::Done);
        head.push(Line::from(vec![label(if i == 0 { "Waits for" } else { "" }), Span::styled(fit_end(&what, w.saturating_sub(LABEL)), Style::new().fg(if done { t.dim } else { t.text }))]));
    }
    // What its squads spent and what they did, from their leaders' transcripts.
    ticket_work_lines(app, x, w, &mut head);
    if let Some(name) = app.ticket_window_open(number) {
        head.push(Line::styled(format!("Open in {name}, in a window of its own. platoon reads it again when you are done."), Style::new().fg(t.accent)));
    }
    head.push(Line::styled("─".repeat(w), Style::new().fg(t.border)));
    let used = (head.len() as u16).min(area.height);
    f.render_widget(Paragraph::new(head), Rect::new(area.x, area.y, area.width, used));
    let body = Rect::new(area.x, area.y + used, area.width, area.height.saturating_sub(used));
    if body.height == 0 {
        return;
    }
    let mut text = app.tickets_store().map(|s| s.body(x)).unwrap_or_default();
    // The squad's result comes first: on a Done ticket it is what the user opened it for.
    if let Some(result) = app.tickets_store().and_then(|s| s.result(x.number)) {
        text = format!("## Result\n\n{}\n\n{text}", result.trim_end());
    }
    if text.trim().is_empty() {
        f.render_widget(Paragraph::new("Nothing written yet. Press e to write it.").style(Style::new().fg(t.subtext)), body);
        return;
    }
    let lines = crate::mdview::render(&text, w, t, app.plain_marker);
    let rows = body.height as usize;
    let top = app.tk.body_top.get().min(lines.len().saturating_sub(rows));
    app.tk.body_top.set(top);
    for (n, l) in lines.into_iter().skip(top).take(rows).enumerate() {
        f.render_widget(Paragraph::new(l), Rect::new(body.x, body.y + n as u16, body.width, 1));
    }
}

/// The ticket screen's two parts about its squads' work: the tokens they used (in all, what that counts, and a line per
/// agent), and what the squad has done (each member's run, in order, and who is at work now). Nothing until a
/// leader's transcript has been read.
fn ticket_work_lines(app: &App, x: &Ticket, w: usize, head: &mut Vec<Line<'static>>) {
    let t = app.effective_theme();
    let heading = |s: &str| Line::styled(s.to_string(), Style::new().fg(t.subtext).add_modifier(Modifier::BOLD));
    if let Some(tk) = app.ticket_tokens(x) {
        head.push(Line::raw(""));
        // "Tokens used  84K sent and written, plus 1.9M read again from Claude's cache", wrapped under itself.
        const LEAD: &str = "Tokens used  ";
        let lead_w = LEAD.chars().count();
        for (i, part) in wrap(&crate::squadwork::screen_total(&tk), w.saturating_sub(lead_w).max(10)).into_iter().enumerate() {
            let lead = if i == 0 { LEAD.to_string() } else { " ".repeat(lead_w) };
            head.push(Line::from(vec![
                Span::styled(lead, Style::new().fg(t.subtext).add_modifier(Modifier::BOLD)),
                Span::styled(part, Style::new().fg(t.text)),
            ]));
        }
        for l in wrap(&crate::squadwork::explained(&tk), w.saturating_sub(2)) {
            head.push(Line::styled(format!("  {l}"), Style::new().fg(t.dim)));
        }
        // Per agent: sent and written, then what it read again from the cache (cut first when narrow).
        let name_w = tk.agents.iter().map(|(n, _)| n.chars().count()).max().unwrap_or(0).min(w.saturating_sub(10));
        for (name, s) in &tk.agents {
            let mut spans = vec![
                Span::styled(format!("  {:<name_w$}", fit(name, name_w)), Style::new().fg(t.text)),
                Span::styled(format!("  {:>5}", crate::squadwork::short(crate::squadwork::sent_and_written(s))), Style::new().fg(t.subtext)),
            ];
            if s.cache_read > 0 {
                spans.push(Span::styled(format!("  + {} from cache", crate::squadwork::short(s.cache_read)), Style::new().fg(t.dim)));
            }
            head.push(Line::from(spans));
        }
    }
    // The members' runs, per leader (with its squad's name when the ticket has had more than one): the last
    // `RUNS_SHOWN` of them in all, with a line for how many came before.
    let current = x.leader().map(|l| l.session.clone());
    let several = x.leaders.len() > 1;
    let logs: Vec<_> = x.leaders.iter().filter_map(|l| app.leader_log(&l.session).filter(|log| !log.runs.is_empty()).map(|log| (l, log))).collect();
    let (mut skip, earlier) = crate::squadwork::earlier(logs.iter().map(|(_, log)| log.runs.len()).sum());
    if !logs.is_empty() {
        head.push(Line::raw(""));
        head.push(heading("What the squad has done"));
    }
    if let Some(e) = earlier {
        head.push(Line::styled(format!("  {e}"), Style::new().fg(t.dim)));
    }
    for (l, log) in logs {
        // Only the current leader, while it runs, has members at work; an earlier one's are over.
        let running = current.as_deref() == Some(l.session.as_str()) && x.queued.is_none() && app.leader_session_running(&l.session);
        let hidden = skip.min(log.runs.len());
        skip -= hidden;
        if hidden < log.runs.len() && several {
            head.push(Line::styled(format!("  {} squad", app.squad_name(&l.squad)), Style::new().fg(t.subtext)));
        }
        for r in &log.runs[hidden..] {
            let (st, line) = crate::squadwork::run_line(r, running);
            let color = match st {
                crate::squadwork::RunState::Finished => t.text,
                crate::squadwork::RunState::Running => t.busy,
                crate::squadwork::RunState::Failed | crate::squadwork::RunState::Unfinished => t.warn,
            };
            // Wrapped under its own text: the description is the leader's words, and says the most at its end too.
            for (i, part) in wrap(&line, w.saturating_sub(6).max(10)).into_iter().enumerate() {
                head.push(Line::styled(format!("{}{part}", if i == 0 { "  " } else { "      " }), Style::new().fg(color)));
            }
        }
        if let Some(now) = crate::squadwork::now_line(&log.runs, running) {
            head.push(Line::styled(fit_end(&format!("  {now}"), w), Style::new().fg(t.busy).add_modifier(Modifier::BOLD)));
        }
    }
}

/// A list over a ticket: its state (New, Done, Cancelled), the tickets it waits for, or its folders.
fn draw_ticket_pick(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let pick = &app.tk.pick;
    app.hits.borrow_mut().clear();
    let opts = app.ticket_pick_options();
    let Some(x) = app.ticket(pick.number) else { return };
    let (title, head, hints): (String, String, Vec<Hint>) = match pick.kind {
        PickKind::State => (format!("state of #{}", x.number), x.title.clone(), vec![("enter", "choose", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel)), ("↑↓", "move", None)]),
        PickKind::BlockedBy => (
            format!("#{} waits for", x.number),
            format!("#{} is ready once these are Done:", x.number),
            vec![("enter", "tick/untick", Some(Action::Confirm)), ("esc", "done", Some(Action::Cancel)), ("↑↓", "move", None)],
        ),
        PickKind::Folders => (
            format!("folders of #{}", x.number),
            if x.folders.is_empty() { "No folder yet.".into() } else { format!("#{} works in:", x.number) },
            vec![("enter", "add", Some(Action::Confirm)), ("d", "remove", None), ("esc", "done", Some(Action::Cancel)), ("↑↓", "move", None)],
        ),
    };
    let h = (opts.len() as u16 + 6).min(area.height.saturating_sub(2)).max(7);
    let r = centered(area, area.width.saturating_sub(8).clamp(30, 72), h);
    f.render_widget(Clear, r);
    let block = modal_block(t, &title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [head_a, sep, list, _, foot] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(Paragraph::new(fit_end(&head, head_a.width as usize)).style(Style::new().fg(t.text)), head_a);
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);
    let rows = list.height as usize;
    let start = pick.selected.saturating_sub(rows.saturating_sub(1));
    for (i, (opt, on)) in opts.iter().enumerate().skip(start).take(rows) {
        let rect = Rect::new(list.x, list.y + (i - start) as u16, list.width, 1);
        app.hits.borrow_mut().push((rect, Hit::TicketPickRow(i)));
        let sel = i == pick.selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let check = if pick.kind == PickKind::BlockedBy {
            if *on { "[x] " } else { "[ ] " }
        } else if *on {
            "✓ "
        } else {
            "  "
        };
        let room = (rect.width as usize).saturating_sub(2 + check.chars().count());
        let (text, color) = match opt {
            PickOpt::State(State::Working) if x.leader().is_some() => ("Working · continue with the squad".to_string(), state_color(t, &State::Working)),
            PickOpt::State(s) => (s.label().to_string(), state_color(t, s)),
            PickOpt::Blocker(n) => match app.ticket(*n) {
                Some(o) => (format!("#{n} {} · {}", o.title, o.state.label()), t.text),
                None => (format!("#{n}"), t.text),
            },
            PickOpt::Folder(path) => (fit_start(&tilde(path, &app.home), room), t.text),
            PickOpt::AddFolder => ("Add a folder…".to_string(), t.accent),
        };
        let spans = vec![
            Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)),
            Span::styled(check, base.fg(t.accent)),
            Span::styled(fit_end(&text, room), base.fg(color)),
        ];
        f.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
    }
    draw_footer(f, app, foot, &hints, None);
}

fn draw_project_sessions(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let visible = app.visible();
    if visible.is_empty() {
        let msg = if app.filter.is_empty() {
            "No sessions in this project yet. Press n to start one, or m on a session in the Sessions tab."
        } else {
            "No sessions match the filter"
        };
        f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }).style(Style::new().fg(t.dim)), area);
        return;
    }
    let active = visible.len() - app.archived_rows;
    // Screen lines: the active rows, then (if any are archived) a heading and the archived rows.
    let line_of = |i: usize| if i < active { i } else { i + 1 };
    let total = line_of(visible.len() - 1) + 1;
    let rows = area.height as usize;
    let want = line_of(app.selected);
    let top = if rows == 0 { 0 } else { want.saturating_sub(rows.saturating_sub(1)).min(total.saturating_sub(rows)) };
    let now = crate::usage::now() * 1000;
    let w = area.width as usize;
    let name_w = (w.saturating_sub(4 + LAST_W) * 3 / 5).max(6);
    let dir_w = w.saturating_sub(4 + LAST_W + name_w);
    for (i, &idx) in visible.iter().enumerate() {
        let line = line_of(i);
        if i == active && active <= visible.len() && line > 0 && line - 1 >= top && line - 1 < top + rows {
            let y = area.y + (line - 1 - top) as u16;
            f.render_widget(Paragraph::new(Span::styled(format!("Archived ({})", app.archived_rows), Style::new().fg(t.dim).add_modifier(Modifier::BOLD))), Rect::new(area.x, y, area.width, 1));
        }
        if line < top || line >= top + rows {
            continue;
        }
        let s = &app.sessions[idx];
        let rect = Rect::new(area.x, area.y + (line - top) as u16, area.width, 1);
        app.hits.borrow_mut().push((rect, Hit::Row(i)));
        let sel = i == app.selected;
        let bg = if (sel && app.pane == Pane::Sessions) || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let Look { marker, marker_color, .. } = row_look(t, s.status, s.unread, app.tick);
        let text = if s.archived { t.dim } else { t.text };
        let spans = vec![
            Span::styled(if sel { "▎" } else { " " }, base.fg(t.accent)),
            Span::styled(format!("{marker} "), base.fg(if s.archived { t.dim } else { marker_color })),
            Span::styled(fit(&s.name, name_w), base.fg(text)),
            Span::styled(fit_tail(&s.dir, dir_w), base.fg(t.dim)),
            Span::styled(format!("{:>w$}", ago(now, s.last_active), w = LAST_W), base.fg(t.dim)),
        ];
        f.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
    }
}

fn draw_notes(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    // Above the notes, one quiet line: whether platoon is updating them or when it last did, and where they go.
    let mut top_lines: Vec<Line> = Vec::new();
    let (first, first_color) = if app.notes_updating() {
        (Some("platoon is currently updating the project notes.".to_string()), t.warn)
    } else {
        let when = app.notes_updated_at().map(|at| ago(crate::usage::now() * 1000, at * 1000));
        let when = when.map(|w| if w == "now" { "just now".to_string() } else { format!("{w} ago") });
        (when.map(|w| format!("platoon last updated these {w}.")), t.dim)
    };
    let reach = "notes are added as context to each session in a project";
    let width = area.width as usize;
    match first {
        Some(first) if first.chars().count() + 1 + reach.chars().count() <= width => {
            top_lines.push(Line::from(vec![Span::styled(first, Style::new().fg(first_color)), Span::styled(format!(" {reach}"), Style::new().fg(t.dim))]));
        }
        first => {
            for line in first.iter().flat_map(|f| wrap(f, width)) {
                top_lines.push(Line::styled(line, Style::new().fg(first_color)));
            }
            for line in wrap(reach, width) {
                top_lines.push(Line::styled(line, Style::new().fg(t.dim)));
            }
        }
    }
    top_lines.push(Line::raw(""));
    if let Some(name) = app.notes_editor_open() {
        top_lines.push(Line::styled(format!("Open in {name}, in a window of its own."), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)));
        top_lines.push(Line::styled("platoon reads them again when you are done.", Style::new().fg(t.subtext)));
        top_lines.push(Line::raw(""));
    }
    // Why they are not being kept up to date, when that is so: plain words, and where to put it right.
    if let Some(why) = app.notes_stopped() {
        for line in wrap(&format!("These notes are not updating right now: {why}"), area.width as usize) {
            top_lines.push(Line::styled(line, Style::new().fg(t.warn)));
        }
        top_lines.push(Line::raw(""));
    }
    let used = (top_lines.len() as u16).min(area.height);
    if used > 0 {
        f.render_widget(Paragraph::new(top_lines), Rect::new(area.x, area.y, area.width, used));
    }
    let area = Rect::new(area.x, area.y + used, area.width, area.height.saturating_sub(used));
    if area.height == 0 {
        return;
    }
    let Some(text) = app.open_notes().filter(|n| !n.trim().is_empty()) else {
        let mut lines = vec![Line::styled("No notes yet", Style::new().fg(t.subtext))];
        lines.push(Line::raw(""));
        lines.push(Line::styled("Press e to start them from a short template.", Style::new().fg(t.text)));
        lines.push(Line::raw(""));
        lines.push(Line::styled(format!("They live in {}", app.open_notes_path()), Style::new().fg(t.dim)));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
        return;
    };
    // The file stays plain Markdown; the pane shows it the way a Markdown viewer would, scrolled in drawn lines.
    // A line platoon once wrote at the top saying who writes what is left out: the line above says it now.
    let lines = crate::mdview::render(crate::notes::without_preamble(&text), area.width as usize, t, app.plain_marker);
    let rows = area.height as usize;
    let top = app.notes_top.get().min(lines.len().saturating_sub(rows));
    app.notes_top.set(top);
    for (n, l) in lines.into_iter().skip(top).take(rows).enumerate() {
        f.render_widget(Paragraph::new(l), Rect::new(area.x, area.y + n as u16, area.width, 1));
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
    // The top-level tabs count what is active: sessions, and projects.
    let top_counts = [app.counts.first().copied().unwrap_or(0), app.pcounts[0], app.squad_list().len()];
    for (i, tab) in TOPS.iter().enumerate() {
        if i > 0 {
            left.push(Span::raw(" "));
            x += 1;
        }
        // A chip: ` 1 Sessions ` with the count in a badge after a little air. Round caps where the terminal has the
        // glyphs, otherwise a padded square. Either way the text reads "1 Sessions 3".
        let current = i == app.top;
        let (key, label) = (format!("{}", i + 1), format!(" {tab} "));
        let count = top_counts.get(i).copied().unwrap_or(0);
        let w = chip_width(&key, &label, count);
        let rect = Rect::new(x, area.y, w, 1).intersection(area);
        if !rect.is_empty() {
            app.hits.borrow_mut().push((rect, Hit::Top(i)));
        }
        spans.push((x, w, current));
        x += w;
        let hover = !current && rect.contains(app.mouse.into());
        left.extend(tab_chip(t, app.rounded, &key, &label, count, current, hover));
    }
    if unread > 0 {
        left.push(Span::styled(format!("   ● {unread} unread"), Style::new().fg(t.ok)));
    }
    if !app.filter.is_empty() && app.open_project.is_none() {
        left.push(Span::styled(format!("   filter: {}", app.filter), Style::new().fg(t.accent)));
        left.push(Span::styled("  (esc clears)", Style::new().fg(t.dim)));
    }
    if let Some(n) = app.indexing() {
        let spin = SPINNER[(app.tick % 10) as usize];
        let text = if n > 0 { format!("   {spin} indexing {n}…") } else { format!("   {spin} indexing…") };
        left.push(Span::styled(text, Style::new().fg(t.dim)));
    }
    // The smart names being made: how far along, while there is work.
    if let Some((done, total, _)) = app.smart_progress() {
        let paused = if app.smart_idle().is_some() { " (paused)" } else { "" };
        left.push(Span::styled(format!("   {} Indexing {done} of {total}{paused}", app.marker()), Style::new().fg(t.dim)));
    }
    // Nothing that spends tokens can run right now: say so quietly, and why. It goes as soon as the reason does.
    // The header is where it belongs: it shows on every tab, so notes that stopped updating can't go unnoticed.
    let so_far = left.iter().map(|s| s.content.chars().count()).sum::<usize>();
    if let Some(note) = app.ai_header_note((area.width as usize).saturating_sub(so_far + 6)) {
        left.push(Span::styled(format!("   {} {note}", app.marker()), Style::new().fg(t.dim)));
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
    let chip = if current || hover { t.surface } else { t.bg };
    let (k, l, badge) = if current {
        (Style::new().fg(t.accent), Style::new().fg(t.text).add_modifier(Modifier::BOLD), Style::new().fg(t.on_accent).bg(t.accent).add_modifier(Modifier::BOLD))
    } else if hover {
        (Style::new().fg(t.accent), Style::new().fg(t.text), Style::new().fg(t.text).bg(t.border))
    } else {
        (Style::new().fg(t.dim), Style::new().fg(t.dim), Style::new().fg(t.dim).bg(t.surface))
    };
    let mut out = vec![Span::styled(" ", Style::new().bg(chip)), Span::styled(key.to_string(), k.bg(chip)), Span::styled(label.to_string(), l.bg(chip))];
    out.extend(pill(t, rounded, &count.to_string(), badge, chip, t.bg));
    out
}

/// Text in a pill: round caps where the terminal has the glyphs, otherwise a padded square. Two cells more than the
/// text either way. `style` carries the pill's color as its background; the caps sit on `left_bg` and `right_bg`.
fn pill(t: &Theme, rounded: bool, text: &str, style: Style, left_bg: Color, right_bg: Color) -> Vec<Span<'static>> {
    if rounded {
        // The caps are drawn in the pill's color on the color beside them, so the block reads as a pill.
        let color = style.bg.unwrap_or(t.surface);
        vec![
            Span::styled("\u{e0b6}", Style::new().fg(color).bg(left_bg)),
            Span::styled(text.to_string(), style),
            Span::styled("\u{e0b4}", Style::new().fg(color).bg(right_bg)),
        ]
    } else {
        vec![Span::styled(format!(" {text} "), style)]
    }
}

struct Cols {
    name: usize,
    /// The PROJECT column (pill and the gap after it); 0 when no row on the tab is in a project, or no room.
    project: usize,
    dir: usize,
    branch: usize,
    model: usize,
    status: usize,
    /// The spare width of a wide terminal, shared out between the columns (see `Pad`).
    pad: Pad,
}

/// What a wide terminal has left over once every column is as wide as its content needs, shared equally
/// between the columns. A column keeps its own width for text; its share is clear air after it, so the row
/// reaches the right edge and a short value simply leaves a wider gap than a long one. LAST's share goes
/// before it, because LAST is read at the right edge.
#[derive(Default, Clone, Copy)]
struct Pad {
    name: usize,
    project: usize,
    dir: usize,
    branch: usize,
    model: usize,
    status: usize,
    context: usize,
    last: usize,
}

impl Pad {
    #[cfg(test)]
    fn sum(&self) -> usize {
        self.name + self.project + self.dir + self.branch + self.model + self.status + self.context + self.last
    }
}

/// Share `spare` cells equally between the columns marked `true`, left to right. What does not divide evenly
/// goes one cell each to the leftmost of them, so the shares are always within one cell of each other and
/// add up to exactly `spare`: the row fills the width and never comes out a cell too wide.
fn spread<const N: usize>(spare: usize, present: [bool; N]) -> [usize; N] {
    let mut out = [0usize; N];
    let n = present.iter().filter(|p| **p).count();
    if n == 0 || spare == 0 {
        return out;
    }
    let (each, mut rem) = (spare / n, spare % n);
    for (i, p) in present.iter().enumerate() {
        if *p {
            let bonus = usize::from(rem > 0);
            rem -= bonus;
            out[i] = each + bonus;
        }
    }
    out
}

/// `n` cells of clear air in a row's own background.
fn gap(n: usize, style: Style) -> Span<'static> {
    Span::styled(" ".repeat(n), style)
}

/// Width of the "last active" column, right-aligned, at the end of the row.
const LAST_W: usize = 6;

/// The longest project name a row's pill shows in full; a longer one is cut with `…`.
const PROJECT_NAME_MAX: usize = 40;
/// The widest the PROJECT column gets: that name plus the pill's two caps and the gap after it. A wide screen
/// has room for it; on a narrower one the column still gives way to NAME and DIR (see `cols`).
const PROJECT_MAX: usize = PROJECT_NAME_MAX + 3;
/// The PROJECT column is never narrower than its title and a space.
const PROJECT_MIN: usize = 8;
/// Below this many columns a row's project pill shows the project's short badge letters instead of its name.
const PROJECT_NAME_MIN_WIDTH: u16 = 90;
/// NAME and DIR together keep at least this much room; the PROJECT column shrinks, then goes, before they do.
const NAME_DIR_MIN: usize = 20;

/// The widest NAME and DIR get when they are sized to what the rows hold; a longer value is cut.
const NAME_MAX: usize = 60;
const DIR_MAX: usize = 60;
/// Clear air a content-sized column keeps after its longest value, so the columns don't touch.
const COL_GAP: usize = 2;

/// What a row's project pill says at this terminal width: the name, or on a narrow terminal the badge letters.
fn project_label(p: &crate::projects::Project, term_w: u16) -> &str {
    if term_w >= PROJECT_NAME_MIN_WIDTH {
        &p.name
    } else {
        &p.badge
    }
}

/// How wide the PROJECT column wants to be for these rows: enough for the longest pill plus a gap, at most
/// `PROJECT_MAX`; 0 (no column) when none of them is in a project. `term_w` is the real terminal width,
/// which decides whether the pills hold names or badge letters.
fn project_width(app: &App, rows: &[usize], term_w: u16) -> usize {
    let longest = rows.iter().filter_map(|&i| app.projects.of(&app.sessions[i].id)).map(|p| project_label(p, term_w).chars().count()).max();
    longest.map_or(0, |n| (n + 3).clamp(PROJECT_MIN, PROJECT_MAX))
}

/// How much room NAME and DIR would like for these rows: the longest value plus a gap, never less than the
/// column's own title, never more than the cap. This is what keeps the table beside its content on a wide
/// screen instead of stretched across it.
fn content_widths(app: &App, rows: &[usize]) -> (usize, usize) {
    let (mut name, mut dir) = ("NAME".chars().count(), "DIR".chars().count());
    for &i in rows {
        let s = &app.sessions[i];
        // A name platoon made up carries ` ✦ ` after it, inside the same column.
        let n = s.name.chars().count() + if s.name_source == NameSource::Generated { 3 } else { 0 };
        name = name.max(n);
        dir = dir.max(s.dir.chars().count());
        if name + COL_GAP >= NAME_MAX && dir + COL_GAP >= DIR_MAX {
            break; // both are already past their cap; nothing further can widen them
        }
    }
    ((name + COL_GAP).min(NAME_MAX), (dir + COL_GAP).min(DIR_MAX))
}

/// The columns of the Sessions table at `width`, with a PROJECT column that wants `project` cells (0 for
/// none) and NAME/DIR that want `want` (see `content_widths`).
fn cols(width: u16, project: usize, want: (usize, usize)) -> Cols {
    // marker(3) + name + project + dir + branch + model + status + context bar(BAR_WIDTH + 5) + last active
    let fixed = 3 + BAR_WIDTH + 5 + LAST_W;
    let w = width as usize;
    let branch = if w >= 90 { 16 } else { 0 };
    let model = if w >= 100 { 12 } else { 0 };
    let status = 11; // room for "Background" and a space
    let flex = w.saturating_sub(fixed + branch + model + status);
    // The project column comes out of the room NAME and DIR share, and gives way to them on a narrow terminal:
    // it shrinks (the name cut with `…`), and goes when not even its title fits.
    let room = flex.saturating_sub(NAME_DIR_MIN);
    let project = if project > 0 && room >= PROJECT_MIN { project.min(room) } else { 0 };
    let flex = flex - project;
    // NAME and DIR are as wide as their content needs. When the content wants more than there is, the two
    // fall back to sharing what there is, as they always have, so nothing overflows and the row still lines
    // up with the titles.
    let (wn, wd) = (want.0.min(NAME_MAX), want.1.min(DIR_MAX));
    let share = flex * 2 / 5;
    let (name, dir) = if wn + wd <= flex {
        (wn, wd)
    } else if wn <= share {
        (wn, flex - wn)
    } else if wd <= flex - share {
        (flex - wd, wd)
    } else {
        (share, flex - share)
    };
    // Whatever a wide screen has over is then shared equally between the columns, so the row fills the
    // terminal and LAST sits at the right edge. On a narrow terminal there is nothing over and nothing moves.
    let spare = flex.saturating_sub(name + dir);
    let s = spread(spare, [true, project > 0, true, branch > 0, model > 0, true, true, true]);
    let pad = Pad { name: s[0], project: s[1], dir: s[2], branch: s[3], model: s[4], status: s[5], context: s[6], last: s[7] };
    Cols { name, project, dir, branch, model, status, pad }
}

fn draw_columns(f: &mut Frame, t: &Theme, area: Rect, c: &Cols) {
    let s = Style::new().fg(t.dim);
    let p = &c.pad;
    let line = Line::from(vec![
        Span::raw("   "),
        Span::styled(fit("NAME", c.name + p.name), s),
        Span::styled(fit(if c.project > 0 { "PROJECT" } else { "" }, c.project + p.project), s),
        Span::styled(fit("DIR", c.dir + p.dir), s),
        Span::styled(fit(if c.branch > 0 { "BRANCH" } else { "" }, c.branch + p.branch), s),
        Span::styled(fit(if c.model > 0 { "MODEL" } else { "" }, c.model + p.model), s),
        Span::styled(fit("STATUS", c.status + p.status), s),
        Span::styled(fit("CONTEXT", BAR_WIDTH + 5 + p.context), s),
        Span::styled(format!("{:>w$}", "LAST", w = LAST_W - 1 + p.last), s),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_list(f: &mut Frame, app: &App, visible: &[usize], c: &Cols, term_w: u16, area: Rect) {
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
        draw_row(f, app, &app.sessions[idx], i == app.selected, hover, now, c, term_w, row);
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

#[allow(clippy::too_many_arguments)]
fn draw_row(f: &mut Frame, app: &App, s: &Session, selected: bool, hover: bool, now: u64, c: &Cols, term_w: u16, area: Rect) {
    let t = app.effective_theme();
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
    let mut l1 = vec![lead.clone(), Span::styled(format!("{marker} "), base.fg(marker_color))];
    let name_w = c.name;
    // A name platoon made up carries a quiet mark at the end of its column, so it can be told from one you chose.
    if s.name_source == NameSource::Generated && name_w >= 8 {
        l1.push(Span::styled(fit(&s.name, name_w - 3), name_style));
        l1.push(Span::styled(format!(" {} ", app.marker()), base.fg(t.dim)));
    } else {
        l1.push(Span::styled(fit(&s.name, name_w), name_style));
    }
    // The column's share of a wide screen's spare width sits after the text (see `Pad`).
    l1.push(gap(c.pad.name, base));
    // The last parts of a path are what tell folders apart, so a path too long for the column is cut at the
    // front (`…/repos/platoon`) and its end kept. (`s.dir` is already written with `~` for the home folder.)
    let dir = fit_tail(&s.dir, c.dir);
    // The project, as a pill in its color (the same pill as the counts beside Active and Archived), then a gap.
    if c.project > 0 {
        match app.projects.of(&s.id) {
            Some(p) => {
                let text = fit_end(project_label(p, term_w), c.project - 3);
                let style = Style::new().fg(crate::projects::badge_text(t)).bg(crate::projects::slot_color(t, &p.color)).add_modifier(Modifier::BOLD);
                let spans = pill(t, app.rounded, &text, style, bg, bg);
                let used = text.chars().count() + 2;
                l1.extend(spans);
                l1.push(gap(c.project - used + c.pad.project, base));
            }
            None => l1.push(gap(c.project + c.pad.project, base)),
        }
    }
    l1.extend([
        Span::styled(dir, base.fg(subtext)),
        gap(c.pad.dir, base),
        Span::styled(fit(s.branch.as_deref().unwrap_or("-"), c.branch), base.fg(t.dim)),
        gap(c.pad.branch, base),
        Span::styled(fit(s.model.as_deref().unwrap_or("-"), c.model), base.fg(subtext)),
        gap(c.pad.model, base),
        Span::styled(fit(status_text, c.status), base.fg(status_color)),
        gap(c.pad.status, base),
    ]);
    // The bar keeps its own size, whatever the screen: how full it looks is what it means. Its column's
    // share of the spare width sits after the percentage.
    l1.extend(bar_bg(t, s.context_pct, bg, s.context_exact, s.archived));
    l1.push(gap(c.pad.context, base));
    l1.push(Span::styled(format!("{:>w$}", ago(now, s.last_active), w = LAST_W - 1 + c.pad.last), base.fg(t.dim)));

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
    if n == 0 {
        return String::new(); // a column with no room at all shows nothing, not even the ellipsis
    }
    let tail: String = s.chars().skip(len - n.saturating_sub(1)).collect();
    format!("…{tail}")
}

/// Rows the settings tabs take at the top of the popup: the tab titles, and a rule under them.
const SETTINGS_TAB_ROWS: u16 = 2;

/// The titles of the settings tabs as they are drawn: in full where they fit, shortened on a narrow terminal.
fn settings_tab_labels(width: usize) -> Vec<String> {
    let full: Vec<String> = SETTINGS_TABS.iter().map(|tab| format!(" {} ", tab.title)).collect();
    let need = full.iter().map(|l| l.chars().count() + 1).sum::<usize>();
    if need <= width + 1 {
        return full;
    }
    // Only the first word of each, so "Smart features" becomes "Smart".
    SETTINGS_TABS.iter().map(|tab| format!(" {} ", tab.title.split_whitespace().next().unwrap_or(tab.title))).collect()
}

/// The tabs of the settings screen, lit like the header's: the showing one on the raised color in bold, a hovered one
/// raised, the rest quiet. A rule under them separates them from the entries. A click on a title shows that tab.
fn draw_settings_tabs(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let mut spans = Vec::new();
    let mut x = area.x;
    for (i, label) in settings_tab_labels(area.width as usize).into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
            x += 1;
        }
        let w = label.chars().count() as u16;
        let rect = Rect::new(x, area.y, w, 1).intersection(area);
        if !rect.is_empty() {
            app.hits.borrow_mut().push((rect, Hit::SettingsTab(i)));
        }
        let current = i == app.settings_tab;
        let hover = !current && rect.contains(app.mouse.into());
        let style = if current {
            Style::new().fg(t.text).bg(t.surface).add_modifier(Modifier::BOLD)
        } else if hover {
            Style::new().fg(t.text).bg(t.surface)
        } else {
            Style::new().fg(t.dim).bg(t.bg)
        };
        spans.push(Span::styled(label, style));
        x += w;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), Rect::new(area.x, area.y, area.width, 1));
    if area.height > 1 {
        let rule = "─".repeat(area.width as usize);
        f.render_widget(Paragraph::new(Span::styled(rule, Style::new().fg(t.border))), Rect::new(area.x, area.y + 1, area.width, 1));
    }
}

/// The settings screen: the tabs, then one entry per item of the tab showing, each with its explanation and a value
/// that steps with `‹` `›`.
fn draw_settings(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let items = app.items();
    let w = area.width as usize;
    let store = app.settings();
    let current = store.get();
    // On a very short terminal the rule under the tabs goes first, so an entry still shows.
    let tabs_h = if area.height >= 10 { SETTINGS_TAB_ROWS } else { 1 }.min(area.height);
    draw_settings_tabs(f, app, Rect::new(area.x, area.y, area.width, tabs_h));
    let area = Rect::new(area.x, area.y + tabs_h, area.width, area.height - tabs_h);

    // What goes under the entries, worked out first because its height decides how much room they get.
    let mut notes = settings_notes(app, app.settings_tab, w);
    let label = "Settings file: ";
    let file = tilde(&store.path().to_string_lossy(), &app.home);
    // On a system prompt entry the arrows have nothing to change: its own keys take their place, and `r` (restore
    // platoon's default) only once the user has changed that prompt.
    let on_instructions = match items.get(app.setting).map(|i| i.kind) {
        Some(Kind::Instructions(id)) => Some(app.instructions_status(id).restorable()),
        _ => None,
    };
    let hints = if let Some(restorable) = on_instructions {
        let mut hints: Vec<Hint> = vec![("tab", "tabs", None), ("↑↓", "move", None), ("enter", "edit", None)];
        if restorable {
            hints.push(("r", "restore", None));
        }
        hints.push(("esc", "back", Some(Action::Cancel)));
        fit_hints(hints, w, &["move", "tabs"])
    } else {
        fit_hints(
            vec![("tab", "tabs", None), ("↑↓", "move", None), ("←→", "change", None), ("enter", "next choice", None), ("esc", "back", Some(Action::Cancel))],
            w,
            &["move", "next choice"],
        )
    };

    // The file and the key hints sit at the bottom, always. The entries get what is left after the notes.
    let hints_y = area.bottom().saturating_sub(1).max(area.y);
    let path_y = hints_y.saturating_sub(1).max(area.y);
    let top = area.y;
    // The reassurance line is the first thing to go when that lets the selected entry explain itself.
    // A warning or an error is never dropped.
    if !store.is_corrupt() && app.active_notice().is_none() {
        let text_w = (area.width as usize).saturating_sub(4);
        let explained: Vec<Vec<String>> = items.iter().map(|it| wrap(it.explain, text_w)).collect();
        let room = |notes: usize| path_y.saturating_sub(notes as u16 + 1).max(top + items.len().min(SETTINGS_MIN_ROWS) as u16).min(path_y.max(top)) - top;
        // (On the smart features tab the first note is the day's token count: that one stays.)
        let keep = usize::from(app.settings_tab == crate::settings::TAB_SMART);
        if explain_level(&explained, app.setting, room(notes.len()) as usize) == 2 && explain_level(&explained, app.setting, room(keep) as usize) < 2 {
            notes.truncate(keep);
        }
    }
    // The entries come before the notes: on a very short terminal the notes give way.
    let rows_bottom = path_y.saturating_sub(notes.len() as u16 + 1).max(top + items.len().min(SETTINGS_MIN_ROWS) as u16).min(path_y.max(top));
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

/// What goes under the settings entries: on the smart features tab how many tokens they used today and why naming
/// is waiting, if it is; on the sessions tab what naming the archived conversations would take; a warning if the file
/// was unreadable, then the latest notice or the reassurance that changes apply at once.
fn settings_notes(app: &App, tab: usize, w: usize) -> Vec<(String, Color)> {
    let t = app.effective_theme();
    let mut notes: Vec<(String, Color)> = Vec::new();
    if tab == crate::settings::TAB_SMART {
        let (used, cap) = (app.tokens_today, app.settings().get().ai_daily_tokens);
        let line = format!("Used today: {} of {} tokens", crate::settings::thousands(used), crate::settings::thousands(cap));
        notes.push((line, if used >= cap { t.warn } else { t.subtext }));
        if let Some(why) = app.smart_idle() {
            notes.extend(wrap(&format!("Naming is waiting: {why}"), w).into_iter().map(|l| (l, t.warn)));
        }
    }
    if tab == crate::settings::TAB_SESSIONS {
        let e = app.archived_estimate();
        let archived = if e.conversations == 0 {
            "Archived: none waiting to be named".to_string()
        } else {
            format!(
                "Archived: {} conversation{}, about {} tokens to index (an estimate)",
                e.conversations,
                if e.conversations == 1 { "" } else { "s" },
                crate::settings::thousands(e.tokens)
            )
        };
        notes.extend(wrap(&archived, w).into_iter().map(|l| (l, t.subtext)));
    }
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

/// Rows the popup's contents need when nothing has to be left out: the tabs, every entry explained, the notes, the
/// file path and the key hints. It is the most any tab needs, so the popup keeps its size and place as the tabs change.
fn settings_content_height(app: &App, w: usize) -> u16 {
    let text_w = w.saturating_sub(4);
    let need = |tab: usize| {
        let items = &SETTINGS_TABS[tab].items;
        let entries: usize = items.iter().map(|it| 1 + wrap(it.explain, text_w).len()).sum::<usize>() + items.len().saturating_sub(1);
        entries + 1 + settings_notes(app, tab, w).len()
    };
    let most = (0..SETTINGS_TABS.len()).map(need).max().unwrap_or(0);
    (SETTINGS_TAB_ROWS as usize + most + 1 + 2) as u16
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

/// The entries get at least this many rows before the notes under them give way (a screen with more entries than
/// that scrolls, so they needn't all be in view).
const SETTINGS_MIN_ROWS: usize = 9;

/// The fewest entries the settings list still shows, together with the selected entry's explanation, before the
/// explanations are given up (a terminal a handful of lines tall).
const SETTINGS_MIN_WINDOW: usize = 6;

/// As much explanation as fits in `height` rows: every entry explained with air between them (0), else the selected
/// one only (1) - with the list scrolling if it is too long to be shown whole beside it - else none (2).
fn explain_level(explained: &[Vec<String>], selected: usize, height: usize) -> usize {
    let whole = (0..2).find(|&level| {
        let gap = usize::from(level == 0);
        let total: usize = (0..explained.len()).map(|i| 1 + if level == 0 || i == selected { explained[i].len() } else { 0 }).sum();
        total + gap * (explained.len() - 1) <= height
    });
    match whole {
        Some(level) => level,
        None if explained.get(selected).is_some_and(|e| e.len() + SETTINGS_MIN_WINDOW.min(explained.len()) <= height) => 1,
        None => 2,
    }
}

/// Draws the entries and returns the row below the last one.
fn draw_setting_rows(f: &mut Frame, app: &App, current: &crate::settings::Settings, area: Rect) -> u16 {
    let t = app.effective_theme();
    if area.is_empty() {
        return area.y;
    }
    let text_w = (area.width as usize).saturating_sub(4);
    let items = app.items();
    let explained: Vec<Vec<String>> = items.iter().map(|it| wrap(it.explain, text_w)).collect();

    let level = explain_level(&explained, app.setting, area.height as usize);
    let show = |i: usize| level == 0 || (level == 1 && i == app.setting);
    let height = |i: usize| 1 + if show(i) { explained[i].len() } else { 0 };
    let gap = u16::from(level == 0);

    // Scroll only if even that doesn't fit, and only as far as it takes to keep the selected entry (with its
    // explanation) in view.
    let mut start = app.setting_top.get().min(app.setting);
    while start < app.setting && (start..=app.setting).map(|i| height(i) + gap as usize).sum::<usize>() > area.height as usize + gap as usize {
        start += 1;
    }
    app.setting_top.set(start);

    let mut y = area.y;
    for (i, item) in items.iter().enumerate().skip(start) {
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
            Kind::Steps | Kind::Feature(_) => {
                let value = item.value(current);
                let label = (item.label)(value);
                let vw = item.choices.iter().map(|&c| (item.label)(c).chars().count()).chain([label.chars().count()]).max().unwrap_or(0);
                (label, vw, item.step(value, Step::Prev) == value, item.step(value, Step::Next) == value, item.off == Some(value))
            }
            // "Enter to change" or "User Modified", with no arrows: enter (or a click on it) opens the editor list.
            // No buttons to keep in place, so it is only as wide as what it says: the longest ("User Modified ·
            // default updated") would cut every system prompt's name at 80 columns.
            Kind::Instructions(id) => {
                let status = app.instructions_status(id);
                let label = status.label().to_string();
                let vw = label.chars().count();
                (label, vw, true, true, status == crate::instructions::Status::BuiltIn)
            }
            Kind::Theme => {
                let all = app.themes().all();
                let at = all.iter().position(|t| t.id == app.theme.id).unwrap_or(0);
                let vw = all.iter().map(|t| t.name.chars().count()).max().unwrap_or(0).min(24);
                (fit_end(&app.theme.name, vw), vw, at == 0, at + 1 >= all.len(), false)
            }
        };
        let plain = matches!(item.kind, Kind::Instructions(_));
        let ctrl_w = if plain { 1 + vw + 1 } else { 2 + vw + 2 };
        // (A value with no arrows ends in a blank of its own, which is the margin, so its last letter lines up with the `›`s.)
        let cx = (row.right() as usize).saturating_sub(ctrl_w + usize::from(!plain)).max(row.x as usize + 2) as u16;
        let button = |x: u16| Rect::new(x, row.y, 2, 1);
        let (prev, val, next) = if plain {
            // No buttons: the value alone, clickable, in the same place the others' values are.
            (Rect::new(cx, row.y, 0, 0), Rect::new(cx + 1, row.y, vw as u16, 1), Rect::new(cx, row.y, 0, 0))
        } else {
            (button(cx), Rect::new(cx + 2, row.y, vw as u16, 1), button(cx + 2 + vw as u16))
        };
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
        // platoon's default updated under the user's own: said in the warning color, so it is noticed.
        let value_style = match item.kind {
            Kind::Instructions(id) if app.instructions_status(id) == crate::instructions::Status::BuiltInUpdated => Style::new().fg(t.warn).add_modifier(Modifier::BOLD),
            _ => value_style,
        };
        let control = if plain {
            Line::from(vec![Span::styled(" ", Style::new().bg(bg)), Span::styled(format!("{label:>vw$}"), value_style.patch(hot_value)), Span::styled(" ", Style::new().bg(bg))])
        } else {
            Line::from(vec![
                arrow("‹ ", prev, at_start),
                Span::styled(format!("{label:^vw$}"), value_style.patch(hot_value)),
                arrow(" ›", next, at_end),
            ])
        };
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
    for i in app.search_kinds() {
        let k = &SEARCH_KINDS[i];
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
        let n = if app.projects_list_showing() { app.project_rows().len() } else { app.visible().len() };
        if app.search_rows_hidden() { String::new() } else { format!("{n} shown") }
    } else {
        draw_result_rows(f, app, list);
        match (s.results.len(), s.total) {
            (0, _) => String::new(),
            _ if s.kind == FIND_KIND && !matches!(app.find_state(), FindState::Done) => String::new(),
            (n, total) if n < total => format!("first {n} of {total}"),
            (n, _) => format!("{n} found"),
        }
    };
    // Whatever the results area holds, a click on its empty part is not a click outside the dialog.
    app.hits.borrow_mut().push((list, Hit::Inert));
    let (enter, esc) = if kind.narrows_list {
        ("keep filter", "clear")
    } else if s.kind == FIND_KIND {
        match app.find_state() {
            FindState::Idle | FindState::Failed(_) => ("ask Claude", "close"),
            FindState::Done => ("open", "close"),
            FindState::Confirm { .. } => ("go ahead", "back"),
            FindState::Waiting | FindState::Asking => ("", "cancel"),
        }
    } else {
        ("open", "close")
    };
    let mut hints: Vec<Hint> = Vec::new();
    if !enter.is_empty() {
        hints.push(("enter", enter, Some(Action::Confirm)));
    }
    if s.kind == FIND_KIND && matches!(app.find_state(), FindState::Idle | FindState::Done | FindState::Failed(_)) && !s.text.trim().is_empty() {
        hints.push(("ctrl+w", "search wider", None));
    }
    if app.search_kinds().len() > 1 {
        hints.push(("tab", "switch", None));
    }
    hints.push(("esc", esc, Some(Action::Cancel)));
    let hints = fit_hints(hints, foot.width as usize, &["switch", "search wider"]);
    draw_footer(f, app, foot, &hints, Some((count, t.dim)));
    // Last, so it only catches clicks the dialog didn't: outside it, a click closes it like esc.
    app.hits.borrow_mut().push((area, Hit::Action(Action::Cancel)));
}

/// The quick filter's rows: the conversations the list behind is narrowed to, one line each.
fn draw_quick_rows(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    if app.projects_list_showing() {
        return draw_quick_project_rows(f, app, area);
    }
    let visible = app.visible();
    // Nothing is listed until something is typed: the box's own hint stands in the empty area.
    if app.search_rows_hidden() || visible.is_empty() {
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

/// The quick filter on the Projects tab: the projects it narrows the list to, one line each.
fn draw_quick_project_rows(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let rows_ = app.project_rows();
    if app.search_rows_hidden() || rows_.is_empty() {
        let msg = if app.search.text.trim().is_empty() { "Type to narrow the list of projects." } else { "No projects match the filter" };
        f.render_widget(Paragraph::new(msg).style(Style::new().fg(t.dim)), area);
        return;
    }
    let rows = area.height as usize;
    let start = app.pselected.saturating_sub(rows.saturating_sub(1));
    let all = app.projects.all();
    for (i, &idx) in rows_.iter().enumerate().skip(start).take(rows) {
        let p = &all[idx];
        let rect = Rect::new(area.x, area.y + (i - start) as u16, area.width, 1);
        app.hits.borrow_mut().push((rect, Hit::SearchRow(i)));
        let sel = i == app.pselected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let st = app.pstats.get(&p.id).cloned().unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(if sel { "▎ " } else { "  " }, Style::new().fg(t.accent).bg(bg)),
            badge_span(t, p),
            Span::styled(" ", Style::new().bg(bg)),
            Span::styled(fit(&p.name, (area.width as usize / 2).max(8)), Style::new().fg(if sel { t.text } else { t.subtext }).bg(bg)),
            Span::styled(session_counts(&st), Style::new().fg(t.dim).bg(bg)),
        ]);
        f.render_widget(Paragraph::new(line).style(Style::new().bg(bg)), rect);
    }
}

/// What "Describe it" says in place of a list: what to do, that it is waiting for Claude, the question about a
/// wider search, or why it didn't work. False when there are results to draw instead.
fn draw_find_message(f: &mut Frame, app: &App, area: Rect) -> bool {
    let t = app.effective_theme();
    let s = &app.search;
    let mut lines: Vec<(String, Color)> = Vec::new();
    if !app.search_kinds().contains(&FIND_KIND) {
        let text = format!(
            "Describe it is switched off. Press s in the list, go to the Smart features tab, and turn on \"{}\" and \"Find a session by description\".",
            crate::ai::MASTER_LABEL
        );
        lines.push((text, t.warn));
    } else {
        match app.find_state() {
            FindState::Idle if s.text.trim().is_empty() => lines.push((SEARCH_KINDS[FIND_KIND].prompt.to_string(), t.dim)),
            FindState::Idle => lines.push(("Press enter to ask Claude. It uses some of your plan, and nothing is sent until you do.".to_string(), t.dim)),
            FindState::Waiting => {
                lines.push(("Asking Claude...".to_string(), t.subtext));
                lines.push(("Waiting for the names being made to finish first. Press esc to cancel.".to_string(), t.dim));
            }
            FindState::Asking => {
                lines.push(("Asking Claude...".to_string(), t.subtext));
                lines.push(("Press esc to cancel.".to_string(), t.dim));
            }
            FindState::Confirm { estimate, count } => {
                lines.push((
                    format!(
                        "Search wider? This sends {count} conversations to Claude, about {} tokens (an estimate). Press enter to go ahead or esc to go back.",
                        crate::settings::thousands(estimate.tokens)
                    ),
                    t.text,
                ));
            }
            FindState::Done if s.results.is_empty() => {
                lines.push(("Claude found no likely match. Try other words, or press ctrl+w to search wider.".to_string(), t.dim));
            }
            FindState::Done => return false,
            FindState::Failed(msg) => lines.push((msg.clone(), t.warn)),
        }
    }
    let mut y = area.y;
    for (text, color) in lines {
        for line in wrap(&text, area.width as usize) {
            if y < area.bottom() {
                f.render_widget(Paragraph::new(Span::styled(line, Style::new().fg(color))), Rect::new(area.x, y, area.width, 1));
                y += 1;
            }
        }
    }
    true
}

/// The word search's results: the conversation's name and when it was used, and under it the matching message with the
/// words in the accent color.
fn draw_result_rows(f: &mut Frame, app: &App, area: Rect) {
    const H: u16 = 2;
    let t = app.effective_theme();
    let s = &app.search;
    if s.kind == FIND_KIND {
        if draw_find_message(f, app, area) {
            return;
        }
    } else if s.results.is_empty() {
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

/// The box for a project's name: a new one, or a new name.
fn draw_name_box(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let nb = &app.name_box;
    app.hits.borrow_mut().clear();
    let wide = matches!(nb.purpose, NamePurpose::NewTicket | NamePurpose::TicketTitle(_));
    let r = centered(area, area.width.saturating_sub(8).clamp(30, if wide { 84 } else { 64 }), 7);
    f.render_widget(Clear, r);
    let (title, note) = match nb.purpose {
        NamePurpose::Rename(_) => ("rename project", "The project's notes and sessions stay as they are."),
        NamePurpose::NewFor(_) => ("new project", "The session goes into it. Its color and badge can be changed later (c)."),
        NamePurpose::New => ("new project", "Give it a name. Its color and badge can be changed later (c)."),
        NamePurpose::NewTicket => ("new ticket", "Its title. The body opens in your editor next."),
        NamePurpose::TicketTitle(_) => ("rename ticket", "The body and its file stay as they are."),
        NamePurpose::Squad(ref p) => match p {
            crate::app::SquadName::Name => ("squad name", "What the squad is called. Its agents keep their names."),
            crate::app::SquadName::Copy => ("copy squad", "The copy is yours to change; the squad you copied stays as it is."),
            crate::app::SquadName::AgentName => ("agent name", "What the agent is called. Every squad that uses it keeps it."),
            crate::app::SquadName::AgentCopy => ("copy agent", "The copy is yours to change; no squad uses it until you put it on a step."),
            crate::app::SquadName::NewAgent(_) => ("new agent", "Its name, for the job it does (say, test writer). You write its system prompt next."),
        },
    };
    let block = modal_block(t, title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [input, sep, note_area, _, foot] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    // A long line shows its end, where the typing is.
    let room = (input.width as usize).saturating_sub(3);
    let len = nb.input.chars().count();
    let shown: String = nb.input.chars().skip(len.saturating_sub(room)).collect();
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled("❯ ", Style::new().fg(t.accent)), Span::styled(shown, Style::new().fg(t.text))])),
        input,
    );
    f.set_cursor_position((input.x + 2 + (len as u16).min(input.width.saturating_sub(3)), input.y));
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);
    f.render_widget(Paragraph::new(fit_end(note, note_area.width as usize)).style(Style::new().fg(t.dim)), note_area);
    let hints: [Hint; 2] = [("enter", "save", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel))];
    draw_footer(f, app, foot, &hints, None);
}

/// The project picker of `m`: the active projects, "New project…" and "No project".
fn draw_assign(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let a = &app.assign;
    app.hits.borrow_mut().clear();
    let h = (a.options.len() as u16 + 5).min(area.height.saturating_sub(2)).max(6);
    let r = centered(area, area.width.saturating_sub(8).clamp(30, 60), h);
    f.render_widget(Clear, r);
    let block = modal_block(t, "project");
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [head, sep, list, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(Paragraph::new(fit_end(&format!("Put \"{}\" in:", a.session_name), head.width as usize)).style(Style::new().fg(t.text)), head);
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);
    let current = app.projects.project_id_of(&a.session);
    let rows = list.height as usize;
    let start = a.selected.saturating_sub(rows.saturating_sub(1));
    for (i, o) in a.options.iter().enumerate().skip(start).take(rows) {
        let rect = Rect::new(list.x, list.y + (i - start) as u16, list.width, 1);
        app.hits.borrow_mut().push((rect, Hit::AssignRow(i)));
        let sel = i == a.selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let mut spans = vec![Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent))];
        match o {
            AssignOpt::Project(id) => {
                if let Some(p) = app.projects.get(id) {
                    spans.push(badge_span(t, p));
                    spans.push(Span::styled(" ", base));
                    spans.push(Span::styled(fit(&p.name, (rect.width as usize).saturating_sub(badge_width(p) + 6)), base.fg(t.text)));
                    if current == Some(id.as_str()) {
                        spans.push(Span::styled(" ✓", base.fg(t.accent)));
                    }
                }
            }
            AssignOpt::New => spans.push(Span::styled("New project…", base.fg(t.accent))),
            AssignOpt::None => spans.push(Span::styled("No project", base.fg(t.subtext))),
        }
        f.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
    }
    let hints: [Hint; 3] = [("enter", "choose", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel)), ("↑↓", "move", None)];
    draw_footer(f, app, foot, &hints, None);
}

/// The editors this computer has, to open a project's notes with. `e` always shows it; the one used last is
/// starred, first and already picked, so enter opens it again.
fn draw_editor_pick(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let pick = &app.epick;
    app.hits.borrow_mut().clear();
    let h = (pick.options.len() as u16 + 6).min(area.height.saturating_sub(2)).max(7);
    let r = centered(area, area.width.saturating_sub(8).clamp(30, 62), h);
    f.render_widget(Clear, r);
    let what = match (pick.instructions, pick.ticket) {
        (Some(_), _) => "edit the system prompt with".to_string(),
        (None, Some(n)) => format!("edit ticket #{n} with"),
        (None, None) => "edit the notes with".to_string(),
    };
    let block = modal_block(t, &what);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [head, sep, list, note, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    f.render_widget(Paragraph::new(fit_end("Editors found on this computer:", head.width as usize)).style(Style::new().fg(t.text)), head);
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);
    let rows = list.height as usize;
    let start = pick.selected.saturating_sub(rows.saturating_sub(1));
    for (i, e) in pick.options.iter().enumerate().skip(start).take(rows) {
        let rect = Rect::new(list.x, list.y + (i - start) as u16, list.width, 1);
        app.hits.borrow_mut().push((rect, Hit::EditorRow(i)));
        let sel = i == pick.selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let where_it_runs = if e.terminal { "here in platoon" } else { "in its own window" };
        // A star on the one used last, which is also the first and the one already picked.
        let star = if e.id == pick.starred { app.star() } else { " " };
        let spans = vec![
            Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)),
            Span::styled(format!("{star} "), base.fg(t.accent)),
            Span::styled(fit(&e.name, (rect.width as usize).saturating_sub(26)), base.fg(t.text)),
            Span::styled(where_it_runs, base.fg(t.dim)),
        ];
        f.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
    }
    let foot_note = if pick.starred.is_empty() {
        "The one you use is starred here next time.".to_string()
    } else {
        format!("{} the one you used last: enter opens it again.", app.star())
    };
    f.render_widget(Paragraph::new(fit_end(&foot_note, note.width as usize)).style(Style::new().fg(t.dim)), note);
    let hints: [Hint; 3] = [("enter", "open", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel)), ("↑↓", "move", None)];
    draw_footer(f, app, foot, &hints, None);
}

/// The last change to the notes, line by line: `-` for what went, `+` for what came. The marks carry the meaning,
/// so it reads on a screen with no colors at all.
fn draw_changes(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let ch = &app.changes;
    app.hits.borrow_mut().clear();
    let r = centered(area, area.width.saturating_sub(6).max(30), area.height.saturating_sub(2).max(8));
    f.render_widget(Clear, r);
    let block = modal_block(t, "what changed last");
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [head, sep, body, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let legend = Line::from(vec![
        Span::styled("- ", Style::new().fg(t.err).add_modifier(Modifier::BOLD)),
        Span::styled("was there before    ", Style::new().fg(t.subtext)),
        Span::styled("+ ", Style::new().fg(t.ok).add_modifier(Modifier::BOLD)),
        Span::styled("is there now", Style::new().fg(t.subtext)),
    ]);
    f.render_widget(Paragraph::new(legend), head);
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);
    let rows = body.height as usize;
    let top = ch.top.get().min(ch.lines.len().saturating_sub(rows));
    ch.top.set(top);
    let w = body.width as usize;
    for (n, (mark, line)) in ch.lines.iter().skip(top).take(rows).enumerate() {
        let color = match mark {
            crate::notes::Mark::Removed => t.err,
            crate::notes::Mark::Added => t.ok,
            crate::notes::Mark::Gap => t.dim,
            crate::notes::Mark::Same => t.subtext,
        };
        let text = format!("{}{line}", mark.sign());
        f.render_widget(
            Paragraph::new(Span::styled(fit_end(&text, w), Style::new().fg(color))),
            Rect::new(body.x, body.y + n as u16, body.width, 1),
        );
    }
    let hints: [Hint; 3] = [("esc", "close", Some(Action::Cancel)), ("↑↓", "scroll", None), ("u", "undo it", Some(Action::Undo))];
    draw_footer(f, app, foot, &hints, None);
}

/// The hand-off box: the brief Claude wrote, which the user may edit before it becomes the first message of a
/// new session in the same folder and project.
fn draw_handoff(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let h = &app.handoff;
    app.hits.borrow_mut().clear();
    let r = centered(area, area.width.saturating_sub(6).clamp(40, 100), area.height.saturating_sub(2).clamp(10, 40));
    f.render_widget(Clear, r);
    let block = modal_block(t, "hand this work on");
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [head, sep, body, sep2, note, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let title = format!("A new session in {} · project {}", fit_start(&h.cwd, 40), h.project_name);
    f.render_widget(Paragraph::new(Span::styled(fit_end(&title, head.width as usize), Style::new().fg(t.subtext))), head);
    // The text wraps at the box's width (one column kept spare for the cursor) and scrolls; the rulers above and
    // below it say, in words, when there is more than shows.
    let (width, visible) = (crate::textbox::content_width(body.width as usize), body.height as usize);
    h.view.set((width, visible));
    let (shown, style, cursor) = match &h.state {
        HandoffState::Asking => (h.shown(), Style::new().fg(t.dim), None),
        HandoffState::Failed(_) => (h.shown(), Style::new().fg(t.warn), None),
        HandoffState::Ready => (h.shown(), Style::new().fg(t.text), Some(h.cursor)),
    };
    let chars: Vec<char> = shown.chars().collect();
    let rows = crate::textbox::layout(&chars, width);
    let at = cursor.map(|c| crate::textbox::locate(&rows, &chars, c));
    let top = match at {
        Some((row, _)) => crate::textbox::clamp_top(h.top.get(), row, visible, rows.len()),
        None => crate::textbox::clamp_view(h.top.get(), visible, rows.len()),
    };
    h.top.set(top);
    for (n, r) in rows.iter().skip(top).take(visible).enumerate() {
        let line: String = chars[r.start..r.end].iter().collect();
        f.render_widget(Paragraph::new(Span::styled(line, style)), Rect::new(body.x, body.y + n as u16, body.width, 1));
    }
    if let Some((row, col)) = at {
        if row >= top && row < top + visible {
            f.set_cursor_position((body.x + (col as u16).min(body.width.saturating_sub(1)), body.y + (row - top) as u16));
        }
    }
    let below = rows.len().saturating_sub(top + visible);
    handoff_ruler(f, t, sep, crate::textbox::more_label(top, true));
    handoff_ruler(f, t, sep2, crate::textbox::more_label(below, false));
    let note_text = match &h.state {
        HandoffState::Ready => "Edit it if you like. The old session is left exactly as it is. The project's notes go to the new session by themselves.",
        _ => "The old session is left exactly as it is.",
    };
    f.render_widget(Paragraph::new(fit_end(note_text, note.width as usize)).style(Style::new().fg(t.dim)), note);
    let hints: Vec<Hint> = match h.state {
        HandoffState::Ready => vec![
            ("ctrl+s", "start the new session", Some(Action::Confirm)),
            ("enter", "new line", None),
            ("esc", "cancel", Some(Action::Cancel)),
            ("pgup/pgdn", "scroll", None),
        ],
        _ => vec![("esc", "cancel", Some(Action::Cancel))],
    };
    draw_footer(f, app, foot, &hints, None);
}

/// A ruler across the hand-off box, with "↑ 3 more lines above" (or below) near its right end when there is more.
fn handoff_ruler(f: &mut Frame, t: &Theme, area: Rect, label: Option<String>) {
    let w = area.width as usize;
    let line = match label {
        Some(l) if l.chars().count() + 6 <= w => {
            let l = format!(" {l} ");
            let left = w - l.chars().count() - 3;
            Line::from(vec![
                Span::styled("─".repeat(left), Style::new().fg(t.border)),
                Span::styled(l, Style::new().fg(t.subtext)),
                Span::styled("─".repeat(3), Style::new().fg(t.border)),
            ])
        }
        _ => Line::from(Span::styled("─".repeat(w), Style::new().fg(t.border))),
    };
    f.render_widget(Paragraph::new(line), area);
}

/// A terminal editor running inside platoon's window on a project's notes: its screen, and one line saying how to
/// get out if it ever wedges.
fn draw_editing(f: &mut Frame, app: &App, edit: &crate::app::NotesEdit, area: Rect) {
    let t = app.effective_theme();
    let [screen, bar] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    {
        let parser = edit.run.parser().lock().unwrap_or_else(|x| x.into_inner());
        draw_screen(f, t, parser.screen(), screen, None);
    }
    let right = match app.active_notice() {
        Some(n) => n.to_string(),
        None => match (edit.instructions, edit.ticket) {
            (Some(id), _) => format!("{} · the system prompt for {}", edit.name, crate::instructions::action(id).what),
            (None, Some(n)) => format!("{} · ticket #{n}", edit.name),
            (None, None) if edit.squad.is_some() => format!("{} · a squad's system prompt", edit.name),
            (None, None) => format!("{} · the notes of {}", edit.name, app.open_project().map(|p| p.name.clone()).unwrap_or_default()),
        },
    };
    let hints: [Hint; 1] = [("ctrl+space", "stop the editor", Some(Action::Back))];
    let color = if app.active_notice().is_some() { t.warn } else { t.dim };
    draw_footer(f, app, bar.inner(Margin::new(1, 0)), &hints, Some((right, color)));
}

/// A project's color (from the theme) and badge.
fn draw_look(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let lk = &app.look;
    app.hits.borrow_mut().clear();
    let r = centered(area, area.width.saturating_sub(8).clamp(30, 64), 10);
    f.render_widget(Clear, r);
    let block = modal_block(t, "color and badge");
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [swatches, names, _, badge, preview, _, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    let mut spans = Vec::new();
    let mut x = swatches.x;
    for (i, (slot, _)) in crate::projects::SLOTS.iter().enumerate() {
        let rect = Rect::new(x, swatches.y, 4, 1).intersection(swatches);
        if !rect.is_empty() {
            app.hits.borrow_mut().push((rect, Hit::Slot(i)));
        }
        let c = crate::projects::slot_color(t, slot);
        let mark = if i == lk.slot { "▐██▌" } else { " ██ " };
        spans.push(Span::styled(mark, Style::new().fg(c).bg(t.bg)));
        x += 4;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), swatches);
    let label = crate::projects::SLOTS.get(lk.slot).map_or("", |s| s.1);
    f.render_widget(Paragraph::new(Span::styled(format!("Color: {label}   (←→ to change)"), Style::new().fg(t.dim))), names);
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled("Badge ❯ ", Style::new().fg(t.accent)), Span::styled(lk.badge.clone(), Style::new().fg(t.text))])),
        badge,
    );
    f.set_cursor_position((badge.x + 8 + lk.badge.chars().count() as u16, badge.y));
    if let Some(p) = app.projects.get(&lk.project) {
        let mut shown = p.clone();
        shown.color = crate::projects::SLOTS.get(lk.slot).map_or("accent", |s| s.0).to_string();
        shown.badge = if lk.badge.is_empty() { crate::projects::default_badge(&p.name) } else { lk.badge.clone() };
        f.render_widget(Paragraph::new(Line::from(vec![badge_span(t, &shown), Span::raw(" "), Span::styled(p.name.clone(), Style::new().fg(t.text))])), preview);
    }
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
    let title = match (p.ticket, p.project.as_deref().and_then(|id| app.projects.get(id))) {
        (Some(n), _) => format!("a folder for ticket #{n}"),
        (None, Some(pr)) => format!("new session in {}: pick a directory", pr.name),
        (None, None) => "new session: pick a directory".to_string(),
    };
    let block = modal_block(t, &title);
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
    let hints: [Hint; 3] = [("enter", if p.ticket.is_some() { "add" } else { "start" }, Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel)), ("↑↓", "choose", None)];
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
        Confirm::ArchiveProject { name, running, busy, outside, .. } => {
            let what = match (outside, busy) {
                (true, true) => "Some of them run in a terminal outside platoon, and some are working right now: archiving ends them, which interrupts what they are doing.",
                (true, false) => "Some of them run in a terminal outside platoon: archiving ends those copies.",
                _ => "Some of them are working right now: archiving stops them, which interrupts what they are doing.",
            };
            (
                "archive project",
                format!("Archive the project \"{name}\" and its active sessions?"),
                format!(
                    "{running} of its sessions {} running. {what} Nothing is lost: restoring the project brings its sessions back.",
                    if *running == 1 { "is" } else { "are" }
                ),
                "archive",
            )
        }
        Confirm::DeleteProject { name, sessions, .. } => (
            "delete project",
            format!("Delete the project \"{name}\"?"),
            format!(
                "Its notes are deleted too (a backup copy is kept). Its {sessions} session{} stay in the Sessions tab, in no project; no conversation is deleted.",
                if *sessions == 1 { "" } else { "s" }
            ),
            "delete",
        ),
        Confirm::DeleteSquad { id, name } => (
            "delete squad",
            format!("Delete the squad \"{name}\"?"),
            format!("Tickets it already has keep it, as it was when they started. Its file is kept as {id}.json.bak in the squads folder."),
            "delete",
        ),
        Confirm::RestoreSquad { id, name } => (
            "restore squad",
            format!("Put {name} back as platoon has it?"),
            format!("Your changes stop being used. They are kept as {id}.json.bak in the squads folder, until the next time. Tickets it already has keep what they started with."),
            "restore",
        ),
        Confirm::DiscardDraft { name } => (
            "discard draft",
            format!("Throw away the draft \"{name}\"?"),
            "It was never saved, so nothing of it is kept.".to_string(),
            "discard",
        ),
        Confirm::DeleteAgent { id, name } => (
            "delete agent",
            format!("Delete the agent \"{name}\"?"),
            format!("No squad uses it. Tickets already started keep what they started with. Its file is kept as {id}.json.bak in the agents folder."),
            "delete",
        ),
        Confirm::RestoreAgent { id, name } => (
            "restore agent",
            format!("Put {name} back as platoon has it?"),
            format!("Every squad that uses it gets platoon's version. Your changes are kept as {id}.json.bak in the agents folder, until the next time. Tickets already started keep what they started with."),
            "restore",
        ),
        Confirm::DiscardAgentDraft { name } => (
            "discard draft",
            format!("Throw away the draft agent \"{name}\"?"),
            "It was never saved, so nothing of it is kept.".to_string(),
            "discard",
        ),
        Confirm::HandAnyway { number, title, says, .. } => (
            "hand it anyway",
            "The commander said this squad doesn't fit. Hand it anyway?".to_string(),
            format!("#{number} {title}. The commander said: {says}"),
            "hand it over",
        ),
        Confirm::StopSquad { number, title, .. } => (
            "stop the squad",
            "Stop the squad's session too?".to_string(),
            format!("#{number} {title} is Cancelled. Its squad's session is still running. Stopping it keeps its conversation: choosing Working later continues it."),
            "stop it",
        ),
        Confirm::ResetInstructions { id } => {
            let a = crate::instructions::action(id);
            (
                "restore default",
                format!("Restore platoon's default system prompt for {}?", a.what),
                format!(
                    "Your version stops being used. It is kept as {}.bak in the prompts folder, until the next time you restore.",
                    crate::instructions::path(std::path::Path::new(""), id).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                ),
                "restore",
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
    let no = if matches!(c, Confirm::StopSquad { .. }) { "leave it running" } else { "cancel" };
    let hints: [Hint; 2] = [("y", yes, Some(Action::Confirm)), ("n", no, Some(Action::Cancel))];
    draw_footer(f, app, foot, &hints, None);
}

/// The split view: the ticket on the left, its squad's live session on the right. Too narrow for that, the session
/// gets the width and the ticket one line above it.
fn draw_split(f: &mut Frame, app: &App, client: &Client, number: u32) {
    let t = app.effective_theme();
    let size = f.area();
    let a = split_areas(size.width, size.height);
    if a.wide {
        draw_ticket_screen(f, app, number, a.ticket);
        for y in a.term.y..a.bar.bottom() {
            f.render_widget(Paragraph::new("│").style(Style::new().fg(t.border)), Rect::new(a.rule_x, y, 1, 1));
        }
    } else if let Some(x) = app.ticket(number) {
        let mut line = format!("#{} {} · {}", x.number, x.title, x.state.label());
        if !x.reason.is_empty() {
            line.push_str(&format!(" · {}", x.reason));
        }
        f.render_widget(Paragraph::new(fit_end(&line, a.ticket.width as usize)).style(Style::new().fg(state_color(t, &x.state)).add_modifier(Modifier::BOLD)), a.ticket);
    }
    let paused = app.attached_paused();
    draw_terminal(f, t, client, a.term, if paused { None } else { app.selection });
    let scrolled = client.parser.lock().map(|p| p.screen().scrollback()).unwrap_or(0);
    let right = if let Some(notice) = app.active_notice() {
        notice.to_string()
    } else if scrolled > 0 {
        format!("↑ scrolled {scrolled} lines · type to return")
    } else {
        app.ticket(number).map(|x| format!("#{} · {}", x.number, x.state.label())).unwrap_or_default()
    };
    let hints: [Hint; 1] = [("ctrl+space", "back to the project", Some(Action::Back))];
    let color = if app.active_notice().is_some() { t.warn } else { t.dim };
    draw_footer(f, app, a.bar.inner(Margin::new(1, 0)), &hints, Some((right, color)));
}

/// A ticket with a squad, while its session isn't showing: the ticket, and beside it (or above it, when narrow) what
/// the session is doing and how to open or continue it.
fn draw_ticket_with_squad(f: &mut Frame, app: &App, number: u32, area: Rect, term_w: u16) {
    let t = app.effective_theme();
    let running = app.leader_running(number);
    let msg = if running { "The squad's session is running. Enter to open it." } else { "The squad's session isn't running. Enter to continue it." };
    if term_w >= SPLIT_MIN_WIDTH {
        let left = area.width * 2 / 5;
        draw_ticket_screen(f, app, number, Rect::new(area.x, area.y, left.saturating_sub(1), area.height));
        let right = Rect::new(area.x + left + 1, area.y, area.width.saturating_sub(left + 1), area.height);
        for y in area.y..area.bottom() {
            f.render_widget(Paragraph::new("│").style(Style::new().fg(t.border)), Rect::new(area.x + left, y, 1, 1));
        }
        let at = Rect::new(right.x + 2, right.y + right.height / 3, right.width.saturating_sub(4), 2);
        f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }).alignment(Alignment::Center).style(Style::new().fg(if running { t.subtext } else { t.warn })), at);
    } else {
        f.render_widget(Paragraph::new(fit_end(msg, area.width as usize)).style(Style::new().fg(if running { t.subtext } else { t.warn })), Rect::new(area.x, area.y, area.width, 1));
        draw_ticket_screen(f, app, number, Rect::new(area.x, area.y + 2, area.width, area.height.saturating_sub(2)));
    }
}

/// The squads a ticket can be handed to, each with what it can do.
fn draw_squad_pick(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    app.hits.borrow_mut().clear();
    let Some(x) = app.ticket(app.sq.pick_ticket) else { return };
    let w = area.width.saturating_sub(8).clamp(30, 76);
    let inner_w = w.saturating_sub(6) as usize;
    let agents = app.agent_library();
    let rows: Vec<Vec<String>> = app
        .sq
        .pick
        .iter()
        .map(|s| {
            let text = if !crate::squadfile::problems(s, &agents).is_empty() {
                format!("Needs fixing on the Squads tab first. {}", s.can_do)
            } else if s.needs_folder && x.folders.is_empty() {
                format!("Needs a folder, and #{} has none. {}", x.number, s.can_do)
            } else {
                s.can_do.clone()
            };
            wrap(&text, inner_w.saturating_sub(2))
        })
        .collect();
    let list_h: u16 = rows.iter().map(|r| r.len() as u16 + 2).sum();
    let r = centered(area, w, (list_h + 6).min(area.height.saturating_sub(2)).max(8));
    f.render_widget(Clear, r);
    let block = modal_block(t, "hand to a squad");
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [head, sep, list, _, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(Paragraph::new(fit_end(&format!("#{} {}", x.number, x.title), head.width as usize)).style(Style::new().fg(t.text)), head);
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);
    // Scrolled so the picked squad shows: from the first one that leaves room for it.
    let picked = app.sq.pick_selected.min(rows.len().saturating_sub(1));
    let fits = |from: usize| rows.get(from..=picked).map_or(true, |r| r.iter().map(|r| r.len() as u16 + 2).sum::<u16>() <= list.height + 1);
    let start = (0..=picked).find(|&k| fits(k)).unwrap_or(picked);
    let mut y = list.y;
    for (i, (s, lines)) in app.sq.pick.iter().zip(&rows).enumerate().skip(start) {
        let h = (lines.len() as u16 + 1).min(list.bottom().saturating_sub(y));
        if h == 0 {
            break;
        }
        let rect = Rect::new(list.x, y, list.width, h);
        app.hits.borrow_mut().push((rect, Hit::SquadRow(i)));
        let sel = i == app.sq.pick_selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let mut out = vec![Line::from(vec![Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)), Span::styled(s.name.clone(), base.fg(t.text).add_modifier(Modifier::BOLD))])];
        out.extend(lines.iter().map(|l| Line::from(vec![Span::styled("  ", base), Span::styled(l.clone(), base.fg(t.subtext))])));
        f.render_widget(Paragraph::new(out).style(base), rect);
        y += h + 1;
    }
    let hints: [Hint; 3] = [("enter", "hand it over", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel)), ("↑↓", "move", None)];
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

/// Pad to exactly `n` chars, or cut from the front (`…` first) so that the end stays: a path's last folder.
fn fit_tail(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        format!("{s:<n$}")
    } else {
        fit_start(s, n)
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

    /// What NAME and DIR ask for when the rows are longer than any cap: everything, so the columns share
    /// the room the way they always have.
    const GREEDY: (usize, usize) = (NAME_MAX, DIR_MAX);

    #[test]
    fn the_project_column_takes_its_room_from_name_and_dir_and_never_widens_the_table() {
        let rest = 3 + BAR_WIDTH + 5 + LAST_W;
        let sum = |c: &Cols| c.name + c.project + c.dir + c.branch + c.model + c.status + c.pad.sum();
        for width in 0..=240u16 {
            let plain = cols(width, 0, GREEDY);
            assert_eq!(plain.project, 0);
            let flex = (width as usize).saturating_sub(rest + plain.status + plain.branch + plain.model);
            for want in [PROJECT_MIN, 12, PROJECT_MAX] {
                for content in [(6usize, 5usize), (20, 34), GREEDY] {
                    let c = cols(width, want, content);
                    assert_eq!((c.branch, c.model, c.status), (plain.branch, plain.model, plain.status), "{width}: other drop rules unchanged");
                    assert!(c.project == 0 || (PROJECT_MIN..=want).contains(&c.project), "{width}/{want}: {}", c.project);
                    assert!(c.project == 0 || c.name + c.dir >= NAME_DIR_MIN.min(content.0 + content.1), "{width}/{want}/{content:?}");
                    assert!(rest + sum(&c) <= (width as usize).max(rest + plain.status), "{width}/{want}/{content:?}: past the terminal's right edge");
                }
                // Where the content fills the row anyway, the project column comes out of NAME and DIR, and
                // the table still adds up to exactly the width it did before there was such a column.
                if plain.name + plain.dir == flex {
                    assert_eq!(sum(&cols(width, want, GREEDY)), sum(&plain), "{width}/{want}: the table adds up to the same width");
                }
            }
        }
        // From 126 columns there is room for the widest column (a 40-character name in full) with NAME and DIR
        // keeping their least; a little narrower it shrinks to what is over, and at the narrowest there is none.
        assert_eq!(cols(126, PROJECT_MAX, GREEDY).project, PROJECT_MAX);
        assert!(cols(125, PROJECT_MAX, GREEDY).project < PROJECT_MAX);
        let c = cols(114, PROJECT_MAX, GREEDY);
        assert_eq!((c.project, c.name + c.dir), (31, NAME_DIR_MIN), "it gives way before NAME and DIR drop below their least");
        assert_eq!(cols(40, PROJECT_MAX, GREEDY).project, 0);
    }

    #[test]
    fn name_and_dir_are_sized_to_their_content_and_never_overflow_the_width() {
        let rest = 3 + BAR_WIDTH + 5 + LAST_W; // marker, context bar, last active
        let fixed = rest + 11; // and the status column
        for width in 0..=340u16 {
            for want in [(6, 5), (20, 34), (NAME_MAX, 12), (12, DIR_MAX), GREEDY] {
                for project in [0, PROJECT_MIN, PROJECT_MAX] {
                    let c = cols(width, project, want);
                    let total = rest + c.name + c.project + c.dir + c.branch + c.model + c.status + c.pad.sum();
                    assert!(total <= (width as usize).max(fixed), "{width}/{want:?}/{project}: {total} cells for {width} columns");
                    assert!(c.name <= NAME_MAX && c.dir <= DIR_MAX, "{width}/{want:?}: capped");
                    // Nothing is cut that fits: when there is room for both wants they get exactly that.
                    let flex = (width as usize).saturating_sub(fixed + c.branch + c.model) - c.project;
                    if want.0 + want.1 <= flex {
                        assert_eq!((c.name, c.dir), want, "{width}/{want:?}/{project}: sized to the content");
                        // And the room that is over fills the rest of the row, shared between the columns.
                        assert_eq!(total, (width as usize).max(fixed), "{width}/{want:?}/{project}: the row reaches the right edge");
                    } else {
                        assert_eq!(c.name + c.dir, flex, "{width}/{want:?}/{project}: the two share what there is");
                        assert_eq!(c.pad.sum(), 0, "{width}/{want:?}/{project}: nothing over to share");
                    }
                }
            }
        }
        // Wide screen, short content: the columns keep their own width and the spare cells are shared
        // equally between them (the shares within one cell of each other).
        let c = cols(341, PROJECT_MAX, (10, 18));
        assert_eq!((c.name, c.project, c.dir), (10, PROJECT_MAX, 18));
        let shares = [c.pad.name, c.pad.project, c.pad.dir, c.pad.branch, c.pad.model, c.pad.status, c.pad.context, c.pad.last];
        let (lo, hi) = (shares.iter().min().unwrap(), shares.iter().max().unwrap());
        assert!(hi - lo <= 1, "the eight columns share alike: {shares:?}");
        assert_eq!(3 + c.name + c.project + c.dir + c.branch + c.model + c.status + BAR_WIDTH + 5 + LAST_W + c.pad.sum(), 341);
        // A column that is not there takes no share: with no PROJECT column the other seven share it all.
        let c = cols(341, 0, (10, 18));
        assert_eq!(c.pad.project, 0);
        assert!(c.pad.name - c.pad.last <= 1, "and the rest still share alike: {} vs {}", c.pad.name, c.pad.last);
        // Narrow screen, long content: exactly the sharing of before (two fifths to NAME), nothing over.
        let narrow = cols(80, 0, GREEDY);
        let flex = 80 - (fixed + 0 + 0);
        assert_eq!((narrow.name, narrow.dir), (flex * 2 / 5, flex - flex * 2 / 5));
        assert_eq!(narrow.pad.sum(), 0);
    }

    #[test]
    fn the_projects_list_scrolls_just_far_enough_to_show_the_selected_row_whole() {
        // Rows of 3 (name, empty description, spacer) and 5 (a description of three lines).
        let h = [3u16, 5, 3, 5, 3];
        // Everything up to the selection fits: no scrolling.
        assert_eq!(project_list_start(&h, 0, 9), 0);
        assert_eq!(project_list_start(&h, 1, 9), 0, "3 + the selected row's 4 text lines");
        // The selection's spacer may fall off the bottom, its text may not.
        assert_eq!(project_list_start(&h, 2, 10), 0, "3 + 5 + 2");
        assert_eq!(project_list_start(&h, 2, 9), 1, "5 + 2");
        assert_eq!(project_list_start(&h, 3, 9), 2, "3 + 4");
        assert_eq!(project_list_start(&h, 4, 9), 3, "5 + 2");
        // A row taller than the whole list starts at the top; nothing selected (an empty list) starts at 0.
        assert_eq!(project_list_start(&h, 3, 2), 3);
        assert_eq!(project_list_start(&[], 0, 9), 0);
    }

    #[test]
    fn spare_width_is_shared_equally_and_to_the_cell() {
        // Nothing over, nothing shared.
        assert_eq!(spread(0, [true, true, true]), [0, 0, 0]);
        // An even share, and a remainder that goes to the leftmost columns so the total is exact.
        assert_eq!(spread(9, [true, true, true]), [3, 3, 3]);
        assert_eq!(spread(11, [true, true, true]), [4, 4, 3]);
        assert_eq!(spread(2, [true, false, true, true]), [1, 0, 1, 0]);
        for spare in 0..=400usize {
            for present in [[true; 4], [true, false, true, false], [false, false, false, true]] {
                let out = spread(spare, present);
                assert_eq!(out.iter().sum::<usize>(), spare, "{spare}/{present:?}: every spare cell is used");
                let used: Vec<usize> = out.iter().zip(present).filter(|(_, p)| *p).map(|(n, _)| *n).collect();
                assert!(used.iter().max().unwrap() - used.iter().min().unwrap() <= 1, "{spare}/{present:?}: {used:?}");
                assert!(out.iter().zip(present).all(|(n, p)| p || *n == 0), "{spare}/{present:?}: a column that is not there gets nothing");
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
