//! The Tasks tab (`2`): the board's sections and their two-line rows (the same rows a project's Tasks pane draws,
//! `draw_task_list`), the picked task's details beside the list from `SPLIT_MIN_WIDTH` columns (what the squad asks or
//! wants to publish and the keys that answer, what it has done as a timeline, what ran), the timeline and "what ran"
//! that a task's own screen shows too, the answer box (`y`) and the project picker (`n`, `m`).

use super::{Hint, ROW_H, badge_span, badge_width, centered, draw_footer, fit, fit_end, hint_fg, modal_block, popup_ground, wrap};
use crate::app::{Action, App, Hit, ListItem};
use crate::board::{self, Piece, Section, Tone};
use crate::squadwork::{self, RunState};
use crate::theme::Theme;
use crate::tickets::{self, Leader, State, Ticket};
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
        Section::CanStart => t.ok,
        Section::WaitsFor => t.subtext,
        Section::Later | Section::Finished => t.dim,
        Section::Closed => t.subtext,
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

/// The lines of an item: a heading, a task's two lines, a fold, a blank.
#[allow(clippy::too_many_arguments)]
fn item_lines(
    app: &App,
    item: &ListItem,
    width: usize,
    selected: bool,
    lit: bool,
    badges: bool,
    badge_w: usize,
    plain: bool,
) -> Vec<Line<'static>> {
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
        ListItem::Fold { section, count, open } => {
            let mark = match (open, plain) {
                (true, false) => "▾",
                (false, false) => "▸",
                (true, true) => "-",
                (false, true) => "+",
            };
            let text = if *section == Section::Closed {
                format!("{mark} Done and cancelled · {count}")
            } else if *open {
                format!("{mark} the first {} only", board::FINISHED_SHOWN)
            } else {
                format!("{mark} {count} more")
            };
            let style = if *section == Section::Closed { base.fg(t.subtext).add_modifier(Modifier::BOLD) } else { base.fg(t.subtext) };
            vec![Line::from(vec![lead, Span::styled(fit_end(&text, width.saturating_sub(1)), style)]).style(base)]
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
            let closed = matches!(r.section, Section::Finished | Section::Closed);
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

/// A list of tasks as the board draws it: each section's heading and count, its tasks as two-line rows (the project's
/// badge with `badges`), folds, blank lines between the board's sections. `selected`: the picked row among those that
/// can be picked; `focused`: the list has the keys (the picked row is lit only then). Scrolled so the picked row
/// shows, with its heading when it is the first of its section; `top` keeps where it was so the list doesn't jump.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_task_list(
    f: &mut Frame,
    app: &App,
    area: Rect,
    items: &[ListItem],
    selected: usize,
    focused: bool,
    badges: bool,
    top: &Cell<usize>,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let t = app.effective_theme();
    app.pages.tickets.set((area.height / ROW_H).max(1) as usize);
    let badge_w = if badges {
        items
            .iter()
            .filter_map(|i| if let ListItem::Task(r) = i { app.projects.get(&r.project).map(badge_width) } else { None })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    let badges = badges && badge_w > 0;
    // Where each item starts, and the picked one's lines (with its heading above it when it is the first row).
    let mut ys = Vec::with_capacity(items.len());
    let mut y = 0usize;
    let mut k = 0usize;
    let mut pick: Option<(usize, usize)> = None;
    for (i, item) in items.iter().enumerate() {
        ys.push(y);
        if item.selectable() {
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
    let plain = app.plain_marker;
    let mut k = 0usize;
    for (i, item) in items.iter().enumerate() {
        let index = if item.selectable() {
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
        let lit = (sel && focused) || hover;
        let lines = item_lines(app, item, area.width as usize, sel, lit, badges, badge_w, plain);
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
    // More below what shows: said on the last line (the list scrolls as the picked row moves).
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
    let wide = f.area().width >= crate::app::SPLIT_MIN_WIDTH;
    let selected = app.board_selected();
    if !wide {
        draw_task_list(f, app, area, &items, selected, true, true, &app.bd.top);
        return;
    }
    let left = area.width * 55 / 100;
    let list = Rect::new(area.x, area.y, left.saturating_sub(1), area.height);
    draw_task_list(f, app, list, &items, selected, true, true, &app.bd.top);
    for y in area.y..area.bottom() {
        f.render_widget(Paragraph::new("│").style(Style::new().fg(t.border)), Rect::new(area.x + left, y, 1, 1));
    }
    let details = Rect::new(area.x + left + 2, area.y, area.width.saturating_sub(left + 2), area.height);
    draw_board_details(f, app, details);
}

/// The task the board has picked, as it is now.
fn picked_task(app: &App) -> Option<(String, Ticket)> {
    let (p, n) = app.board_task()?;
    let all = app.all_tasks();
    let t = all.iter().find(|(q, _)| *q == p)?.1.iter().find(|t| t.number == n)?.clone();
    Some((p, t))
}

/// "in WS Web shop · Docs squad · 11 min": where a task is and who has it (how long it has been at it).
pub(super) fn where_line(app: &App, project: &str, x: &Ticket, w: usize) -> Line<'static> {
    let t = app.effective_theme();
    let mut spans: Vec<Span> = Vec::new();
    match app.projects.get(project) {
        Some(p) => {
            spans.push(Span::styled("in ", Style::new().fg(t.dim)));
            spans.push(badge_span(t, p));
            spans.push(Span::styled(format!(" {}", p.name), Style::new().fg(t.subtext)));
        }
        None => spans.push(Span::styled(tickets::NO_PROJECT_NAME, Style::new().fg(t.subtext))),
    }
    if let Some(l) = x.leader() {
        let mut who = format!(" · {} squad", app.squad_name(&l.squad));
        if !x.state.closed() && l.started > 0 {
            let at = board::since(crate::usage::now(), l.started);
            if !at.is_empty() {
                who.push_str(&format!(" · {}", if at == "just now" { "just started".to_string() } else { at }));
            }
        }
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        spans.push(Span::styled(fit_end(&who, w.saturating_sub(used)), Style::new().fg(t.subtext)));
    }
    Line::from(spans)
}

/// The details of the board's picked task: its title and where it is, what the squad asks (or wants to publish) and
/// the keys that answer, what it has done, what ran.
fn draw_board_details(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let w = area.width as usize;
    if w < 10 {
        return;
    }
    let Some((project, x)) = picked_task(app) else {
        let msg = match app.board_pick() {
            Some(crate::app::BRow::Fold) => "Enter shows the rest of today's finished tasks.",
            _ => "",
        };
        f.render_widget(Paragraph::new(fit_end(msg, w)).style(Style::new().fg(t.dim)), Rect::new(area.x, area.y, area.width, 1));
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
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
    lines.push(where_line(app, &project, &x, w));
    lines.push(Line::raw(""));
    let key = |k: &str, d: &str| {
        vec![Span::styled(k.to_string(), Style::new().fg(t.accent)), Span::styled(format!(" {d}"), Style::new().fg(hint_fg(t)))]
    };
    // The keys that answer, each whole: the ones that don't fit beside the others are left to the footer.
    let keys_line = |pairs: &[(&str, &str)]| {
        let mut spans = Vec::new();
        let mut used = 0;
        for (i, (k, d)) in pairs.iter().enumerate() {
            let gap = if i > 0 { 3 } else { 0 };
            let need = gap + k.width() + 1 + d.width();
            if used + need > w {
                break;
            }
            if i > 0 {
                spans.push(Span::raw("   "));
            }
            spans.extend(key(k, d));
            used += need;
        }
        Line::from(spans)
    };
    let siblings: Vec<Ticket> = app.all_tasks().into_iter().find(|(p, _)| *p == project).map(|(_, l)| l).unwrap_or_default();
    if let Some(need) = board::need_of(&x) {
        let aside = x.is_aside();
        lines.push(Line::styled(need.heading(), bold));
        for l in wrap(&board::squad_said(&x), w).into_iter().take(8) {
            lines.push(Line::styled(l, Style::new().fg(if aside { t.subtext } else { t.warn })));
        }
        lines.push(Line::raw(""));
        let y = match need {
            board::Need::GoAhead => "go ahead",
            board::Need::Permission => "open its session",
            _ => "answer",
        };
        let h = if aside { "bring it back" } else { "later" };
        lines.push(keys_line(&[("y", y), ("enter", "open the session"), ("h", h)]));
        lines.push(Line::raw(""));
    } else {
        let says = tickets::words(&x, &siblings);
        let color = super::words_color(t, &x, &says);
        let mut state = vec![Span::styled(says.clone(), Style::new().fg(color).add_modifier(Modifier::BOLD))];
        let tail = x.busy.clone().or_else(|| x.queued.as_ref().map(|_| tickets::QUEUED_WORDS.to_string()));
        if let Some(tail) = tail {
            state.push(Span::styled(fit_end(&format!(" · {tail}"), w.saturating_sub(says.width())), Style::new().fg(t.subtext)));
        } else if x.state.closed() {
            state.push(Span::styled(format!(" · {}", board::ago(crate::usage::now(), x.updated)), Style::new().fg(t.dim)));
        }
        lines.push(Line::from(state));
        if x.state == State::New {
            if x.folders.is_empty() {
                lines.push(Line::styled("No folder of its own: a squad works in the task's own folder.", Style::new().fg(t.subtext)));
            } else {
                let names: Vec<String> = x.folders.iter().map(|f| crate::squads::folder_name(f)).collect();
                for l in wrap(&format!("Works in {}", names.join(", ")), w) {
                    lines.push(Line::styled(l, Style::new().fg(t.subtext)));
                }
            }
            let waits = tickets::waiting_for(&x, &siblings);
            for n in &waits {
                let title = siblings.iter().find(|o| o.number == *n).map(|o| o.title.clone()).unwrap_or_default();
                lines.push(Line::styled(fit_end(&format!("Waits for #{n} {title}"), w), Style::new().fg(t.subtext)));
            }
            if let Some(h) = &x.handover {
                for l in wrap(&h.says, w).into_iter().take(4) {
                    lines.push(Line::styled(l, Style::new().fg(t.warn)));
                }
            }
            lines.push(Line::raw(""));
            if waits.is_empty() {
                lines.push(keys_line(&[("S", "hand to a squad"), ("enter", "open")]));
            } else {
                lines.push(keys_line(&[("enter", "open")]));
            }
            lines.push(Line::raw(""));
        } else {
            lines.push(Line::raw(""));
        }
        // A finished task: its result first.
        if x.state == State::Done
            && let Ok(text) = std::fs::read_to_string(tickets::bodies_dir(&app.config, &project).join(tickets::result_file(x.number)))
            && !text.trim().is_empty()
        {
            lines.push(Line::styled("Result", bold));
            for l in text.lines().filter(|l| !l.trim().is_empty()).flat_map(|l| wrap(l, w)).take(6) {
                lines.push(Line::styled(l, Style::new().fg(t.text)));
            }
            lines.push(Line::raw(""));
        }
    }
    if let Some(l) = x.leader() {
        let lanes = timeline_lines(app, &x, l, w, true);
        if !lanes.is_empty() {
            lines.push(Line::styled("What it has done", bold));
            lines.extend(lanes);
            lines.push(Line::raw(""));
        }
    }
    let ran = ran_lines(app, &x, w, Some(squadwork::RAN_SHOWN));
    if !ran.is_empty() {
        lines.push(Line::styled("What ran", bold));
        lines.extend(ran);
    }
    lines.truncate(area.height as usize);
    f.render_widget(Paragraph::new(lines), area);
}

/// A clock time in the user's own time zone: "13:40" ("" when unknown).
fn clock(ms: Option<u64>) -> String {
    ms.and_then(|ms| crate::handoff::local_time(ms / 1000))
        .map(|s| s.chars().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect())
        .unwrap_or_default()
}

/// A leader's timeline in `w` cells: one line per member run (the agent, a bar for when it ran within the squad's
/// time so far, how long it took, done / now / stopped), then (`upcoming`, while the task is open) the steps of its
/// workflow that haven't run yet. Nothing until its transcript has been read.
pub(super) fn timeline_lines(app: &App, x: &Ticket, l: &Leader, w: usize, upcoming: bool) -> Vec<Line<'static>> {
    let t = app.effective_theme();
    let Some(log) = app.leader_log(&l.session) else { return Vec::new() };
    let current = x.leader().is_some_and(|c| c.session == l.session);
    let running = current && x.queued.is_none() && app.leader_session_running(&l.session);
    let steps: Vec<String> = if upcoming && current && !x.state.closed() {
        app.sqt
            .list
            .iter()
            .find(|s| s.id == l.squad)
            .and_then(|s| s.squad.as_ref())
            .map(|s| s.agent_ids().iter().map(|a| crate::squads::agent_name(&l.squad, a, l.names)).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if log.runs.is_empty() && steps.is_empty() {
        return Vec::new();
    }
    let from = log.first_at.unwrap_or(l.started * 1000);
    let ends = log.runs.iter().filter_map(|r| r.ended_at.or(r.started_at)).max().unwrap_or(from);
    let to = if running { (crate::usage::now() * 1000).max(ends) } else { log.last_at.unwrap_or(ends).max(ends) };
    let names: Vec<String> = log
        .runs
        .iter()
        .map(|r| squadwork::agent_label(&r.agent, &l.squad))
        .chain(steps.iter().map(|s| squadwork::agent_label(s, &l.squad)))
        .collect();
    let name_w = names.iter().map(|n| n.width()).max().unwrap_or(0).min(16).min(w / 3);
    const TOOK: usize = 7;
    const WORD: usize = 7;
    let bar_w = w.saturating_sub(name_w + 1 + 1 + 1 + TOOK + 2 + WORD);
    let bar_w = if bar_w >= 4 { bar_w } else { 0 };
    let lanes = squadwork::timeline(&log.runs, running, &l.squad, &steps, from, to, bar_w);
    lanes
        .into_iter()
        .map(|lane| {
            let (word, color) = match lane.state {
                Some(s @ RunState::Finished) => (s.word(), t.ok),
                Some(s @ RunState::Running) => (s.word(), t.busy),
                Some(s) => (s.word(), t.dim),
                None => (squadwork::NOT_YET, t.dim),
            };
            let name_color = if lane.state.is_none() { t.dim } else { t.text };
            let mut spans = vec![Span::styled(fit(&lane.agent, name_w), Style::new().fg(name_color)), Span::raw(" ")];
            if bar_w > 0 {
                let mut bar = vec![Span::raw(" ")];
                match lane.bar {
                    Some((at, len)) => {
                        bar.push(Span::raw(" ".repeat(at)));
                        bar.push(Span::styled("━".repeat(len), Style::new().fg(color)));
                        bar.push(Span::raw(" ".repeat(bar_w.saturating_sub(at + len))));
                    }
                    None if lane.state.is_none() => bar.push(Span::raw(" ".repeat(bar_w))),
                    None => bar.push(Span::styled(fit("·", bar_w), Style::new().fg(t.dim))),
                }
                spans.extend(bar);
            }
            let took = lane.took.map(squadwork::took).unwrap_or_default();
            spans.push(Span::styled(format!(" {took:>TOOK$}"), Style::new().fg(t.subtext)));
            spans.push(Span::styled(format!("  {word}"), Style::new().fg(color)));
            Line::from(spans)
        })
        .collect()
}

/// What ran for a task, in `w` cells, grouped by run: platoon's own steps and the commands its squads ran in the shell,
/// each with its time. `last`: only the latest that many (the board); None: all of them (a task's screen).
pub(super) fn ran_lines(app: &App, x: &Ticket, w: usize, last: Option<usize>) -> Vec<Line<'static>> {
    let t = app.effective_theme();
    let logs: Vec<(&Leader, Option<&crate::claude::LeaderLog>)> = x.leaders.iter().map(|l| (l, app.leader_log(&l.session))).collect();
    let ran = squadwork::task_ran(x, &logs, &|id| app.squad_name(id));
    let mut out = Vec::new();
    for g in squadwork::grouped(&ran, last) {
        out.push(Line::styled(fit_end(&g.who, w), Style::new().fg(t.subtext)));
        for (at, text) in g.lines {
            let time = clock(at);
            let lead = format!("  {time:<5}  ");
            out.push(Line::from(vec![
                Span::styled(lead.clone(), Style::new().fg(t.dim)),
                Span::styled(fit_words(&text, w.saturating_sub(lead.width())), Style::new().fg(t.text)),
            ]));
        }
    }
    out
}

/// The keys of the Tasks tab for the picked row (the footer, and with `all` every key the board has, for the help).
pub(super) fn board_hints(app: &App, all: bool) -> Vec<Hint<'_>> {
    let mut out: Vec<Hint> = Vec::new();
    let row = if all { None } else { app.board_row() };
    let fold = !all && matches!(app.board_pick(), Some(crate::app::BRow::Fold));
    if all {
        out.extend([
            ("y", "answer the squad (on a go-ahead: let it publish)", None),
            ("enter", "open the task, beside its squad's session when it runs", None),
            ("h", "later: put a task that needs you aside until its squad says something new (h again brings it back)", None),
            ("S", "hand to a squad", None),
            ("c", "cancel", None),
            ("m", "move it to another project (or to no project)", None),
            ("e", "edit its text", None),
            ("p", "let it publish", None),
        ]);
    } else if fold {
        out.push(("enter", "show/hide", None));
    } else if let Some(r) = &row {
        match r.section {
            Section::NeedsYou | Section::Later => {
                let y = match r.need {
                    Some(board::Need::GoAhead) => "go ahead",
                    Some(board::Need::Permission) => "its session",
                    _ => "answer",
                };
                out.push(("y", y, None));
                out.push(("enter", "open", Some(Action::Jump)));
                out.push(("h", if r.section == Section::Later { "bring back" } else { "later" }, None));
            }
            Section::CanStart => {
                out.push(("S", "hand to a squad", None));
                out.push(("enter", "open", Some(Action::Jump)));
            }
            _ => out.push(("enter", "open", Some(Action::Jump))),
        }
        if !matches!(r.section, Section::Finished | Section::Closed) || r.right != "Cancelled" {
            let cancelled = app.ticket(r.number).is_some_and(|t| t.state == State::Cancelled);
            if !cancelled {
                out.push(("c", "cancel", None));
            }
        }
        out.push(("m", "move", None));
    }
    out.push(("n", "new task", None));
    out.extend([
        ("/", "filter", None),
        ("1-4", "tabs", None),
        ("↑↓", "move", None),
        ("?", "help", Some(Action::Help)),
        (app.quit_label(), "quit", Some(Action::Quit)),
    ]);
    out
}

/// "2 of 3 squads at work", for the board's footer while nothing else is said there.
pub(super) fn at_work_line(app: &App) -> Option<String> {
    let n = app.squads_at_work();
    (n > 0).then(|| format!("{n} of {} squads at work", app.settings().get().squads_at_once))
}

/// The answer box: "Answer the Docs squad on #6", the question above, one line typed into the squad's session.
pub(super) fn draw_answer(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    app.hits.borrow_mut().clear();
    let a = &app.bd.answer;
    let Some(x) = app.ticket(a.number) else { return };
    let w = area.width.saturating_sub(8).clamp(30, 76);
    let inner_w = w.saturating_sub(4) as usize;
    let question: Vec<String> = wrap(&board::squad_said(x), inner_w).into_iter().take(5).collect();
    let note = "Sent to the squad's session as your reply; the task goes back to Working.";
    let notes = wrap(note, inner_w);
    let h = (question.len() + notes.len() + 7) as u16;
    let r = centered(area, w, h.min(area.height.saturating_sub(2)));
    popup_ground(f, t, area, r, true);
    let title = format!("Answer the {} squad on #{}", app.answer_squad_name(), a.number);
    let block = modal_block(t, &title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    app.hits.borrow_mut().push((r, Hit::Inert));
    let [q_area, _, input, _, note_area, _, foot] = Layout::vertical([
        Constraint::Length(question.len() as u16),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(notes.len() as u16),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    f.render_widget(Paragraph::new(question.into_iter().map(|l| Line::styled(l, Style::new().fg(t.warn))).collect::<Vec<_>>()), q_area);
    // A long answer shows its end, where the typing is.
    let room = (input.width as usize).saturating_sub(3);
    let len = a.input.chars().count();
    let shown: String = a.input.chars().skip(len.saturating_sub(room)).collect();
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled("❯ ", Style::new().fg(t.accent)), Span::styled(shown, Style::new().fg(t.text))])),
        input,
    );
    f.set_cursor_position((input.x + 2 + (len as u16).min(input.width.saturating_sub(3)), input.y));
    f.render_widget(Paragraph::new(notes.into_iter().map(|l| Line::styled(l, Style::new().fg(t.dim))).collect::<Vec<_>>()), note_area);
    let hints: [Hint; 3] =
        [("enter", "send", Some(Action::Confirm)), ("ctrl+o", "open the session instead", None), ("esc", "cancel", Some(Action::Cancel))];
    draw_footer(f, app, foot, &hints, None);
}

/// The project picker: which project a new task goes in, or where a task moves to.
pub(super) fn draw_task_project(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    app.hits.borrow_mut().clear();
    let (title, head) = match &app.bd.pick_for {
        crate::app::PickFor::NewTask => ("New task".to_string(), "Which project is it in?".to_string()),
        crate::app::PickFor::Move(from, n) => (format!("Move #{n}"), format!("Where to? It is in {} now.", app.task_project_name(from))),
    };
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
