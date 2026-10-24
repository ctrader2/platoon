//! The Tasks tab (`3`): the board's sections and their two-line rows, the picked task's details beside the list from
//! `SPLIT_MIN_WIDTH` columns (its state and why, its squad, its folder and branch, its plan when one is ready, its "Done
//! when" boxes, its tests and what it has spent), a task's own screen (the same details and its text), the list of
//! squads a task starts with (`s`) and the project picker of a new task (`n`).

use super::{Hint, ROW_H, badge_span, badge_width, centered, draw_footer, fit_end, hint_fg, modal_block, popup_ground, wrap};
use crate::app::{Action, App, Hit, ListItem};
use crate::board::{self, Piece, Section, Tone};
use crate::theme::Theme;
use crate::tickets::{self, Need, State, Ticket};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::cell::Cell;
use unicode_width::UnicodeWidthStr;

/// A tone's color in the theme.
pub(super) fn tone(t: &Theme, x: Tone) -> Color {
    match x {
        Tone::Subtext => t.subtext,
        Tone::Dim => t.dim,
        Tone::Warn => t.warn,
        Tone::Busy => t.busy,
        Tone::Ok => t.ok,
        Tone::Accent => t.accent,
    }
}

/// A section's heading color: what needs the user stands out, what is finished steps back.
fn section_color(t: &Theme, s: Section) -> Color {
    match s {
        Section::NeedsYou => t.warn,
        Section::Working => t.busy,
        Section::NotStarted => t.text,
        Section::Done => t.dim,
    }
}

/// A state's color: what needs the user stands out, what is finished steps back.
pub(crate) fn state_color(t: &Theme, s: &State) -> Color {
    match s {
        State::NeedsYou(_) => t.warn,
        State::Working => t.busy,
        State::NotStarted => t.text,
        State::Done => t.ok,
        State::Cancelled => t.dim,
    }
}

/// The pieces of a row's second line in `room` cells, whole where they fit. When one doesn't, the line ends there with
/// "…": the first piece (the squad's words) is cut after a word, never in the middle of one; a later piece (a step's
/// mark, the tokens) goes whole.
pub(super) fn fit_pieces(t: &Theme, pieces: &[Piece], room: usize, bg: Color) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for (k, p) in pieces.iter().enumerate() {
        let w = p.text.width();
        let last = k + 1 == pieces.len();
        // Whole, with room left for an ellipsis after it unless it is the last.
        if used + w <= room && (last || used + w < room) {
            out.push(Span::styled(p.text.clone(), Style::new().fg(tone(t, p.tone)).bg(bg)));
            used += w;
            continue;
        }
        let left = room.saturating_sub(used + 1);
        let mut cut = String::new();
        for word in p.text.split_inclusive(' ').take_while(|_| k == 0) {
            if cut.width() + word.trim_end().width() > left {
                break;
            }
            cut.push_str(word);
        }
        let cut = cut.trim_end().to_string();
        if !cut.is_empty() {
            used += cut.width();
            out.push(Span::styled(cut, Style::new().fg(tone(t, p.tone)).bg(bg)));
        }
        if used < room {
            out.push(Span::styled("…", Style::new().fg(t.dim).bg(bg)));
        }
        break;
    }
    out
}

/// `s` in at most `n` cells: whole, or cut after a word with "…" (a first word longer than that is cut where it must
/// be).
pub(super) fn fit_words(s: &str, n: usize) -> String {
    if s.width() <= n {
        return s.to_string();
    }
    let room = n.saturating_sub(1);
    let mut out = String::new();
    for word in s.split_inclusive(' ') {
        if out.width() + word.trim_end().width() > room {
            break;
        }
        out.push_str(word);
    }
    let out = out.trim_end();
    if out.is_empty() { fit_end(s, n) } else { format!("{out}…") }
}

/// The lines of an item: a heading, a task's two lines, a blank.
fn item_lines(app: &App, item: &ListItem, width: usize, selected: bool, lit: bool, badges: bool, badge_w: usize) -> Vec<Line<'static>> {
    let t = app.effective_theme();
    let bg = if lit { t.surface } else { t.bg };
    let base = Style::new().bg(bg);
    let lead = Span::styled(if selected { "▎" } else { " " }, base.fg(t.accent));
    match item {
        ListItem::Gap => vec![Line::raw("")],
        ListItem::Heading(s, n) => {
            let c = section_color(t, *s);
            vec![Line::from(vec![
                Span::raw(" "),
                Span::styled(s.heading().to_string(), Style::new().fg(c).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" · {n}"), Style::new().fg(c)),
            ])]
        }
        ListItem::Task(r) => {
            let mut one = vec![lead];
            let mut indent = 1;
            if badges {
                match app.projects.get(&r.project) {
                    Some(p) => {
                        let pad = badge_w.saturating_sub(badge_width(p));
                        one.push(badge_span(t, p));
                        one.push(Span::styled(" ".repeat(pad + 1), base));
                    }
                    None => one.push(Span::styled(" ".repeat(badge_w + 1), base)),
                }
                indent += badge_w + 1;
            }
            let num = format!("#{} ", r.number);
            let closed = r.section == Section::Done;
            let right_w = r.right.width();
            let room = width.saturating_sub(indent + num.width());
            let title_room = if right_w > 0 { room.saturating_sub(right_w + 2) } else { room };
            one.push(Span::styled(num.clone(), base.fg(t.accent)));
            let title = fit_words(&r.title, title_room.max(1));
            let title_w = title.width();
            one.push(Span::styled(title, base.fg(if closed { t.subtext } else { t.text })));
            if right_w > 0 {
                let gap = room.saturating_sub(title_w + right_w);
                one.push(Span::styled(" ".repeat(gap), base));
                one.push(Span::styled(r.right.clone(), base.fg(tone(t, r.right_tone))));
            }
            let mut two = vec![Span::styled(" ".repeat(indent), base)];
            two.extend(fit_pieces(t, &r.second, width.saturating_sub(indent), bg));
            vec![Line::from(one).style(base), Line::from(two).style(base)]
        }
    }
}

/// The board's list: each section's heading and count, its tasks as two-line rows with their project's badge, blank
/// lines between sections. `selected`: the picked task. Scrolled so the picked row shows, with its heading when it is
/// the first of its section; `top` keeps where it was so the list doesn't jump.
pub(super) fn draw_task_list(f: &mut Frame, app: &App, area: Rect, items: &[ListItem], selected: usize, top: &Cell<usize>) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let t = app.effective_theme();
    app.pages.tickets.set((area.height / ROW_H).max(1) as usize);
    let badge_w = items
        .iter()
        .filter_map(|i| if let ListItem::Task(r) = i { app.projects.get(&r.project).map(badge_width) } else { None })
        .max()
        .unwrap_or(0);
    let badges = badge_w > 0;
    // Where each item starts, and the picked one's lines (with its heading above it when it is the first row).
    let mut ys = Vec::with_capacity(items.len());
    let mut y = 0usize;
    let mut k = 0usize;
    let mut pick: Option<(usize, usize)> = None;
    for (i, item) in items.iter().enumerate() {
        ys.push(y);
        if matches!(item, ListItem::Task(_)) {
            if k == selected {
                let from = if i > 0 && matches!(items[i - 1], ListItem::Heading(..)) { y - 1 } else { y };
                pick = Some((from, y + item.height() as usize));
            }
            k += 1;
        }
        y += item.height() as usize;
    }
    let total = y;
    // When it doesn't all fit, the last line says how much more is below.
    let h = if total > area.height as usize && area.height > 2 { area.height as usize - 1 } else { area.height as usize };
    let mut first = top.get().min(total.saturating_sub(h));
    if let Some((a, b)) = pick {
        if a < first {
            first = a;
        } else if b > first + h {
            first = b - h;
        }
    }
    top.set(first);
    let mut k = 0usize;
    for (i, item) in items.iter().enumerate() {
        let index = if matches!(item, ListItem::Task(_)) {
            k += 1;
            Some(k - 1)
        } else {
            None
        };
        let y0 = ys[i];
        let ih = item.height() as usize;
        if y0 + ih <= first || y0 >= first + h {
            continue;
        }
        let rect = Rect::new(area.x, area.y + y0.saturating_sub(first) as u16, area.width, ih as u16);
        let full = rect.intersection(area);
        let sel = index == Some(selected);
        let hover = index.is_some() && full.contains(app.mouse.into());
        let lit = sel || hover;
        let lines = item_lines(app, item, area.width as usize, sel, lit, badges, badge_w);
        for (li, line) in lines.into_iter().enumerate() {
            let ly = y0 + li;
            if ly < first || ly >= first + h {
                continue;
            }
            let at = Rect::new(area.x, area.y + (ly - first) as u16, area.width, 1);
            if let Some(ix) = index {
                app.hits.borrow_mut().push((at, Hit::TicketRow(ix)));
            }
            let bg = if lit { t.surface } else { t.bg };
            f.render_widget(Paragraph::new(line).style(Style::new().bg(bg)), at);
        }
    }
    let below = total.saturating_sub(first + h);
    if below > 0 {
        super::draw_more_below(f, t, Rect::new(area.x, area.bottom() - 1, area.width, 1), below);
    }
}

/// The Tasks tab: the board, and beside it (from `SPLIT_MIN_WIDTH` columns) the picked task's details.
pub(super) fn draw_board(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let items = app.board_items();
    if items.is_empty() {
        let msg = if app.board_filter().is_empty() { board::EMPTY } else { "No tasks match the filter. Esc clears it." };
        let lines: Vec<Line> =
            wrap(msg, area.width.saturating_sub(4) as usize).into_iter().map(|l| Line::styled(l, Style::new().fg(hint_fg(t)))).collect();
        let h = lines.len() as u16;
        let at = Rect::new(area.x, area.y + area.height / 3, area.width, h.min(area.height));
        f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), at);
        return;
    }
    let selected = app.board_selected();
    if f.area().width < SPLIT_MIN_WIDTH {
        draw_task_list(f, app, area, &items, selected, &app.bd.top);
        return;
    }
    let left = area.width * 55 / 100;
    let list = Rect::new(area.x, area.y, left.saturating_sub(1), area.height);
    draw_task_list(f, app, list, &items, selected, &app.bd.top);
    for y in area.y..area.bottom() {
        f.render_widget(Paragraph::new("│").style(Style::new().fg(t.border)), Rect::new(area.x + left, y, 1, 1));
    }
    let details = Rect::new(area.x + left + 2, area.y, area.width.saturating_sub(left + 2), area.height);
    draw_board_details(f, app, details);
}

/// From this many columns the board has the picked task's details beside it.
pub const SPLIT_MIN_WIDTH: u16 = 100;

/// The task the board has picked, as it is now.
fn picked_task(app: &App) -> Option<(String, Ticket)> {
    let (p, n) = app.board_task()?;
    let all = app.all_tasks();
    let t = all.iter().find(|(q, _)| *q == p)?.1.iter().find(|t| t.number == n)?.clone();
    Some((p, t))
}

/// The words of a backtick pair (`npm test`) stay together: a line breaks before it rather than inside it. A pair too
/// long for a line of `room` columns is broken at its spaces after all.
fn units(text: &str, room: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut open = false;
    for word in text.split_inclusive(' ') {
        match out.last_mut() {
            Some(last) if open => last.push_str(word),
            _ => out.push(word.to_string()),
        }
        if word.matches('`').count() % 2 == 1 {
            open = !open;
        }
    }
    out.into_iter()
        .flat_map(|u| if u.trim_end().width() > room { u.split_inclusive(' ').map(String::from).collect() } else { vec![u] })
        .collect()
}

/// Pieces of words in their tones, wrapped to `room` columns at a space (never inside a backtick pair).
fn piece_rows(app: &App, pieces: Vec<board::Piece>, room: usize) -> Vec<Vec<Span<'static>>> {
    let t = app.effective_theme();
    let mut rows: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut used = 0;
    for p in pieces {
        let style = Style::new().fg(tone(t, p.tone));
        for word in units(&p.text, room) {
            let word = word.as_str();
            let w = word.trim_end().width();
            if used > 0 && used + w > room {
                rows.push(Vec::new());
                used = 0;
            }
            let word = if used == 0 { word.trim_start() } else { word };
            if word.is_empty() {
                continue;
            }
            used += word.width();
            if let Some(row) = rows.last_mut() {
                row.push(Span::styled(word.to_string(), style));
            }
        }
    }
    rows
}

/// The width of the details' labels ("Folder" and the air after it).
const LABEL_W: usize = 8;

/// The details of a task, `w` cells wide: its number and title, its project, its state and why, its squad, its folder
/// and branch, its plan when one is ready (`plan_most`: at most that many lines of it), its "Done when" boxes as its
/// text has them, the check of its work, what it has spent.
fn detail_lines(app: &App, project: &str, x: &Ticket, w: usize, plan_most: Option<usize>) -> Vec<Line<'static>> {
    let t = app.effective_theme();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let bold = Style::new().fg(t.text).add_modifier(Modifier::BOLD);
    for (i, l) in wrap(&format!("#{} {}", x.number, x.title), w).into_iter().enumerate() {
        if i == 0 {
            let num = format!("#{}", x.number);
            let rest = l.strip_prefix(&num).unwrap_or(&l).to_string();
            lines.push(Line::from(vec![Span::styled(num, bold.fg(t.accent)), Span::styled(rest, bold)]));
        } else {
            lines.push(Line::styled(l, bold));
        }
    }
    let mut place: Vec<Span<'static>> = Vec::new();
    match app.projects.get(project) {
        Some(p) => {
            place.push(Span::styled("in ", Style::new().fg(t.dim)));
            place.push(badge_span(t, p));
            place.push(Span::styled(format!(" {}", p.name), Style::new().fg(t.subtext)));
        }
        None => place.push(Span::styled(tickets::NO_PROJECT_NAME, Style::new().fg(t.subtext))),
    }
    lines.push(Line::from(place));
    lines.push(Line::raw(""));
    // Its state, and why it needs the user.
    let now = crate::usage::now();
    let busy = app.starting_words(project, x.number);
    let unmerged = app.not_merged(project, x.number).filter(|_| x.state == State::Done);
    let (word, color) = match (&x.state, busy) {
        (_, Some(_)) => ("Starting".to_string(), t.busy),
        (State::NeedsYou(n), _) => (format!("Needs you · {}", n.words()), t.warn),
        (State::Done, _) if unmerged.is_some() => (board::NOT_MERGED.to_string(), t.warn),
        (s, _) => (s.label().to_string(), state_color(t, s)),
    };
    let mut state = vec![Span::styled(word.clone(), Style::new().fg(color).add_modifier(Modifier::BOLD))];
    if x.state.closed() {
        state.push(Span::styled(format!(" · {}", board::ago(now, x.updated)), Style::new().fg(t.dim)));
    }
    lines.push(Line::from(state));
    // Done, but its work is only on its branch: how much, and where it isn't yet.
    if let Some(u) = unmerged {
        let commits = if u.commits == 1 { "1 commit".to_string() } else { format!("{} commits", u.commits) };
        for l in wrap(&format!("Branch {} has {commits} not in {}.", x.branch(), u.into), w) {
            lines.push(Line::styled(l, Style::new().fg(t.warn)));
        }
    }
    if let State::NeedsYou(n) = &x.state
        && !n.text().is_empty()
    {
        for l in wrap(n.text(), w).into_iter().take(8) {
            lines.push(Line::styled(l, Style::new().fg(t.warn)));
        }
    }
    lines.push(Line::raw(""));
    let label = |s: &str| Span::styled(format!("{s:<LABEL_W$}"), Style::new().fg(t.dim));
    let room = w.saturating_sub(LABEL_W);
    // Its squad (suggested, before it starts).
    let squad = match (x.squad.is_empty(), x.started()) {
        (true, _) => Span::styled("none picked yet: s picks one", Style::new().fg(t.subtext)),
        (false, true) => {
            // The program its session runs in, when both are here to tell apart.
            let on = app.task_program(project, x).filter(|_| app.providers().installed().len() > 1);
            let name = match on {
                Some(a) => format!("{} · in {}", app.squad_name(&x.squad), a.name()),
                None => app.squad_name(&x.squad),
            };
            Span::styled(fit_end(&name, room), Style::new().fg(t.text))
        }
        (false, false) => Span::styled(fit_end(&format!("{} (suggested)", app.squad_name(&x.squad)), room), Style::new().fg(t.text)),
    };
    lines.push(Line::from(vec![label("Squad"), squad]));
    let folder = match &x.folder {
        Some(f) => Span::styled(fit_end(&crate::dirs::tilde(f, &app.home), room), Style::new().fg(t.text)),
        None => Span::styled("none yet: s asks for one", Style::new().fg(t.subtext)),
    };
    lines.push(Line::from(vec![label("Folder"), folder]));
    if let Some(wt) = &x.worktree {
        let where_ = if wt.removed.is_some() {
            "its worktree was removed after the work".to_string()
        } else if wt.kept_uncommitted {
            format!("kept: changes not yet committed · {}", crate::dirs::tilde(&wt.path, &app.home))
        } else {
            crate::dirs::tilde(&wt.path, &app.home)
        };
        lines.push(Line::from(vec![
            label("Branch"),
            Span::styled(fit_end(&format!("{} · {where_}", wt.branch), room), Style::new().fg(t.text)),
        ]));
    } else if x.started() {
        lines.push(Line::from(vec![label("Branch"), Span::styled("none: its folder is not in git", Style::new().fg(t.subtext))]));
    }
    // The check of the work: what it said, then what it found, file:line first.
    let verdict = board::verdict_line(x, now, &|secs| clock(Some(secs * 1000)));
    if !verdict.is_empty() {
        for (k, row) in piece_rows(app, verdict, room).into_iter().enumerate() {
            let mut spans = vec![if k == 0 { label("Verdict") } else { Span::raw(" ".repeat(LABEL_W)) }];
            spans.extend(row);
            lines.push(Line::from(spans));
        }
        for f in x.verdict.iter().filter(|v| v.failed.is_none() && !board::checking(x, now)).flat_map(|v| &v.findings) {
            let place = format!("{}:{}", f.file, f.line);
            for (k, l) in wrap(&format!("{place}: {}", f.problem), room).into_iter().enumerate() {
                let mut spans = vec![Span::raw(" ".repeat(LABEL_W))];
                match l.strip_prefix(&place).filter(|_| k == 0) {
                    Some(rest) => {
                        spans.push(Span::styled(place.clone(), Style::new().fg(t.text)));
                        spans.push(Span::styled(rest.to_string(), Style::new().fg(t.subtext)));
                    }
                    None => spans.push(Span::styled(l, Style::new().fg(t.subtext))),
                }
                lines.push(Line::from(spans));
            }
        }
    }
    if let Some(s) = x.session.as_deref().and_then(|s| app.session_spent(s)) {
        let agent = app.task_program(project, x).unwrap_or_default();
        for (k, l) in wrap(&crate::spent::details_total(&s, agent), room).into_iter().enumerate() {
            lines.push(Line::from(vec![
                if k == 0 { label("Tokens") } else { Span::raw(" ".repeat(LABEL_W)) },
                Span::styled(l, Style::new().fg(t.subtext)),
            ]));
        }
    }
    // The plan, while it waits for the user's yes.
    if x.state == State::NeedsYou(Need::Plan) {
        lines.push(Line::raw(""));
        lines.push(Line::styled("The plan", bold));
        lines.extend(plan_lines(app, project, x, w, plan_most));
    }
    // The "Done when" boxes: as the last check found them (a tick or a cross), else as its text has them.
    let checked: Vec<(&str, Color, String)> = x
        .verdict
        .iter()
        .filter(|v| v.failed.is_none())
        .flat_map(|v| &v.boxes)
        .map(|b| if b.met { ("✓ ", t.ok, b.text.clone()) } else { ("✗ ", t.warn, b.text.clone()) })
        .collect();
    let boxes: Vec<(&str, Color, String)> = if checked.is_empty() {
        let store = tickets::Tickets::open(&app.config, project);
        let body = store.get(x.number).map(|t| store.body(t)).unwrap_or_default();
        crate::tickets::done_when(&body)
            .into_iter()
            .map(|(ticked, text)| if ticked { ("[x] ", t.ok, text) } else { ("[ ] ", t.subtext, text) })
            .collect()
    } else {
        checked
    };
    if !boxes.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("Done when", bold));
        for (mark, c, text) in boxes {
            let mark = if mark.width() < 4 { format!("{mark:<4}") } else { mark.to_string() };
            for (k, l) in wrap(&text, w.saturating_sub(4)).into_iter().enumerate() {
                let lead = if k == 0 { Span::styled(mark.clone(), Style::new().fg(c)) } else { Span::raw("    ") };
                lines.push(Line::from(vec![lead, Span::styled(l, Style::new().fg(t.text))]));
            }
        }
    }
    lines
}

/// A task's plan (`plan.md` in its own folder) drawn from its Markdown in `w` cells; `most`: at most that many lines,
/// with a last one saying how many more there are.
fn plan_lines(app: &App, project: &str, x: &Ticket, w: usize, most: Option<usize>) -> Vec<Line<'static>> {
    let t = app.effective_theme();
    let Some(text) = tickets::Tickets::open(&app.config, project).plan(x.number) else {
        return wrap("Its plan is in its session: enter opens it.", w)
            .into_iter()
            .map(|l| Line::styled(l, Style::new().fg(t.subtext)))
            .collect();
    };
    let mut lines = crate::mdview::render(text.trim(), w, t, app.plain_marker);
    if let Some(most) = most.filter(|m| lines.len() > *m) {
        let more = lines.len() - (most - 1);
        lines.truncate(most - 1);
        lines.push(Line::styled(fit_end(&format!("… {more} more lines in its session"), w), Style::new().fg(t.dim)));
    }
    lines
}

/// The details of the board's picked task.
fn draw_board_details(f: &mut Frame, app: &App, area: Rect) {
    let w = area.width as usize;
    if w < 10 {
        return;
    }
    let Some((project, x)) = picked_task(app) else { return };
    let mut lines = detail_lines(app, &project, &x, w, Some((area.height as usize / 2).max(6)));
    lines.truncate(area.height as usize);
    f.render_widget(Paragraph::new(lines), area);
}

/// A task's own screen: its details, then its text drawn from its Markdown, scrolled with the arrows.
pub(super) fn draw_task_screen(f: &mut Frame, app: &App, number: u32, area: Rect) {
    let t = app.effective_theme();
    let w = area.width.saturating_sub(2) as usize;
    let Some(project) = app.task_scope() else { return };
    let Some(x) = app.ticket(number).cloned() else { return };
    let mut lines = detail_lines(app, &project, &x, w, None);
    lines.push(Line::raw(""));
    lines.push(Line::styled("Its text", Style::new().fg(t.text).add_modifier(Modifier::BOLD)));
    let body = app.tickets_store().map(|s| s.body(&x)).unwrap_or_default();
    if body.trim().is_empty() {
        lines.push(Line::styled("Nothing written yet: e opens it in your editor.", Style::new().fg(hint_fg(t))));
    } else {
        lines.extend(crate::mdview::render(body.trim(), w, t, app.plain_marker));
    }
    let view = Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), area.height);
    let h = view.height as usize;
    let top = app.tk.body_top.get().min(lines.len().saturating_sub(h));
    app.tk.body_top.set(top);
    let below = lines.len().saturating_sub(top + h);
    let shown: Vec<Line> = lines.into_iter().skip(top).take(if below > 0 { h.saturating_sub(1) } else { h }).collect();
    f.render_widget(Paragraph::new(shown), view);
    if below > 0 {
        super::draw_more_below(f, t, Rect::new(view.x, view.bottom() - 1, view.width, 1), below);
    }
}

/// The keys of a task's own screen.
pub(super) fn task_screen_hints(app: &App, n: u32) -> Vec<Hint<'_>> {
    let t = app.ticket(n);
    let mut out: Vec<Hint> = Vec::new();
    match t {
        Some(t) if t.started() => out.push(("enter", "its session", None)),
        Some(t) if t.state != State::Cancelled => out.push(("s", "start", None)),
        _ => {}
    }
    out.push(("e", "edit", None));
    if t.is_some_and(|t| t.state != State::Cancelled) {
        out.push(("c", "cancel", None));
    }
    out.extend([
        ("v", "editor", Some(Action::Editor)),
        ("↑↓", "scroll", None),
        ("?", "help", Some(Action::Help)),
        ("esc", "back", Some(Action::Leave)),
    ]);
    out
}

/// The keys of the Tasks tab for the picked row (the footer, and with `all` every key the board has, for the help).
pub(super) fn board_hints(app: &App, all: bool) -> Vec<Hint<'_>> {
    let mut out: Vec<Hint> = Vec::new();
    if all {
        out.extend([
            ("n", "a new task", None),
            ("s", "start the task: pick its squad (and its folder, when it has none)", None),
            ("enter", "open the task's session; a task that never started shows its text", None),
            ("e", "edit its text", None),
            ("c", "cancel", None),
        ]);
    } else if let Some((p, n)) = app.board_task() {
        let t = app.all_tasks().into_iter().find(|(q, _)| *q == p).and_then(|(_, l)| l.into_iter().find(|t| t.number == n));
        match &t {
            Some(t) if t.started() => out.push(("enter", "its session", Some(Action::Jump))),
            Some(t) if t.state != State::Cancelled => {
                out.push(("s", "start", None));
                out.push(("enter", "open", Some(Action::Jump)));
            }
            _ => out.push(("enter", "open", Some(Action::Jump))),
        }
        out.push(("e", "edit", None));
        if t.is_some_and(|t| t.state != State::Cancelled) {
            out.push(("c", "cancel", None));
        }
    }
    if !all {
        out.push(("n", "new task", None));
    }
    out.extend([
        ("/", "filter", None),
        ("1-4", "tabs", None),
        ("↑↓", "move", None),
        ("?", "help", Some(Action::Help)),
        (app.quit_label(), "quit", Some(Action::Quit)),
    ]);
    out
}

/// The list of squads a task starts with (`s`): each squad's name and what it is for, the picked one's model and
/// whether it plans first under the list; a squad whose file has something wrong says what.
pub(super) fn draw_squad_list(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    app.hits.borrow_mut().clear();
    let (_, number) = &app.sq.pick_task;
    let n = app.sq.pick.len();
    let w = area.width.saturating_sub(8).clamp(40, 90);
    let iw = w.saturating_sub(4) as usize;
    let picked = app.sq.pick.get(app.sq.pick_selected);
    let about: Vec<String> = match picked {
        Some(s) if !s.usable() => wrap(&format!("Can't be used: {}", s.problems.join("; ")), iw).into_iter().take(4).collect(),
        Some(s) => {
            let mut v = wrap(&s.description, iw);
            let plan = if s.plan_first { "plans first, then waits for your yes" } else { "starts working at once" };
            v.push(format!("Model {} · {plan}", crate::squads::Setting::Model.words(s, &app.catalog())));
            v
        }
        None => Vec::new(),
    };
    let h = (n as u16 + about.len() as u16 + 7).min(area.height.saturating_sub(2)).max(8);
    let r = centered(area, w, h);
    popup_ground(f, t, area, r, true);
    let title = format!("Start #{number} with a squad");
    let block = modal_block(t, &title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [list, _, about_a, _, foot] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(about.len() as u16),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let rows = list.height as usize;
    let start = app.sq.pick_selected.saturating_sub(rows.saturating_sub(1));
    let name_w = app.sq.pick.iter().map(|s| s.name.width().max(s.id.width())).max().unwrap_or(0).min(20);
    for (i, s) in app.sq.pick.iter().enumerate().skip(start).take(rows) {
        let rect = Rect::new(list.x, list.y + (i - start) as u16, list.width, 1);
        app.hits.borrow_mut().push((rect, Hit::SquadRow(i)));
        let sel = i == app.sq.pick_selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let name = if s.name.is_empty() { s.id.clone() } else { s.name.clone() };
        let (name_c, rest, rest_c) =
            if s.usable() { (t.text, s.description.clone(), t.subtext) } else { (t.dim, "needs fixing".to_string(), t.warn) };
        let room = (rect.width as usize).saturating_sub(2 + name_w + 2);
        let spans = vec![
            Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)),
            Span::styled(format!("{:<name_w$}  ", fit_end(&name, name_w)), base.fg(name_c).add_modifier(Modifier::BOLD)),
            Span::styled(fit_end(&rest, room), base.fg(rest_c)),
        ];
        f.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
    }
    let about_lines: Vec<Line> = about.into_iter().map(|l| Line::styled(l, Style::new().fg(t.subtext))).collect();
    f.render_widget(Paragraph::new(about_lines), about_a);
    app.hits.borrow_mut().push((r, Hit::Inert));
    let hints: [Hint; 3] = [("enter", "start", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel)), ("↑↓", "move", None)];
    draw_footer(f, app, foot, &hints, None);
}

/// A clock time in the user's own time zone: "13:40" ("" when unknown).
fn clock(ms: Option<u64>) -> String {
    ms.and_then(|ms| crate::handoff::local_time(ms / 1000))
        .map(|s| s.chars().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect())
        .unwrap_or_default()
}

/// The project picker: which project a new task goes in.
pub(super) fn draw_task_project(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    app.hits.borrow_mut().clear();
    let (title, head) = ("New task".to_string(), "Which project is it in?".to_string());
    let n = app.bd.pick.len();
    let h = (n as u16 + 6).min(area.height.saturating_sub(2)).max(7);
    let r = centered(area, area.width.saturating_sub(8).clamp(30, 60), h);
    popup_ground(f, t, area, r, true);
    let block = modal_block(t, &title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [head_a, sep, list, _, foot] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)])
            .areas(inner);
    f.render_widget(Paragraph::new(fit_end(&head, head_a.width as usize)).style(Style::new().fg(t.text)), head_a);
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);
    let rows = list.height as usize;
    let start = app.bd.pick_selected.saturating_sub(rows.saturating_sub(1));
    for (i, id) in app.bd.pick.iter().enumerate().skip(start).take(rows) {
        let rect = Rect::new(list.x, list.y + (i - start) as u16, list.width, 1);
        app.hits.borrow_mut().push((rect, Hit::ProjectPickRow(i)));
        let sel = i == app.bd.pick_selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let mut spans = vec![Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent))];
        match app.projects.get(id) {
            Some(p) => {
                spans.push(badge_span(t, p));
                spans.push(Span::styled(
                    format!(" {}", fit_end(&p.name, (rect.width as usize).saturating_sub(4 + badge_width(p)))),
                    base.fg(t.text),
                ));
            }
            None => spans.push(Span::styled(tickets::NO_PROJECT_NAME, base.fg(t.subtext))),
        }
        f.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
    }
    let hints: [Hint; 3] = [("enter", "pick", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel)), ("↑↓", "move", None)];
    draw_footer(f, app, foot, &hints, None);
}
