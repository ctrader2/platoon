//! Drawing the Squads tab: the left pane's two lists (Squads or Agents, switched in its title), the selected squad's
//! brief or agent's details on the right, the key hints, and the small list and the text box that open over it.

use super::*;
use crate::app::{ARow, ChoiceFor, SqFocus, SqView, TextFor};
use crate::brief::CheckKind;
use crate::squadfile::Squad;
use crate::squadfile::{self, Kind};
use std::collections::HashMap;

/// The keys of the Squads tab, for the list it shows, the pane that has them and the row the cursor is on.
pub(super) fn squads_tab_hints(app: &App) -> Vec<Hint<'_>> {
    let sqt = &app.sqt;
    let mut out: Vec<Hint> = Vec::new();
    let agents = sqt.view == SqView::Agents;
    let (draft, kind) = if agents {
        (sqt.agent_draft_selected(), sqt.agent_listed().map(|l| l.kind))
    } else {
        (sqt.draft_selected(), sqt.listed().map(|l| l.kind))
    };
    let remove = if draft {
        ("d", "discard")
    } else {
        match kind {
            Some(Kind::Own) => ("d", "delete"),
            Some(Kind::Modified) => ("r", "restore"),
            _ => ("", ""),
        }
    };
    let read_only = agents && sqt.agent_read_only();
    match sqt.focus {
        SqFocus::List => {
            if agents && sqt.new_agent_selected() {
                out.push(("enter", "new agent", None));
            } else {
                out.push(("enter", "details", None));
            }
            if !agents && !draft && sqt.squad().is_some() {
                out.push(("e", "edit the brief", None));
            }
            out.push(("← →", if agents { "squads" } else { "agents" }, None));
            if draft {
                out.push(("ctrl+s", "save draft", None));
            }
            // On a narrow terminal (one pane) the short words, so enter's hint keeps its place beside them.
            out.push(("n", if sqt.one_pane.get() { "new squad or agent" } else { "describe a new squad or agent" }, None));
            let readable = if agents { sqt.agent().is_some() && !sqt.new_agent_selected() } else { sqt.squad().is_some() };
            if !draft && readable {
                out.push(("c", if read_only { "copy into platoon's own" } else { "copy" }, None));
            }
            if !remove.0.is_empty() && !(agents && sqt.new_agent_selected()) {
                out.push((remove.0, remove.1, None));
            }
            out.push(("↑↓", "move", None));
        }
        SqFocus::Detail if agents => {
            if read_only {
                out.push(("c", "copy into platoon's own", None));
            } else {
                match app.arow() {
                    Some(ARow::Files | ARow::Commands | ARow::Web) => out.push(("space", "switch", None)),
                    Some(ARow::Skills | ARow::Mcp) => out.push(("enter", "pick", None)),
                    Some(ARow::Prompt) => out.push(("enter", "edit", None)),
                    Some(ARow::UsedIn(_)) => out.push(("enter", "open the squad", None)),
                    Some(_) => out.push(("enter", "change", None)),
                    None => {}
                }
            }
            if draft {
                out.push(("ctrl+s", "save draft", None));
            }
            out.push(("esc", "back to the list", None));
        }
        SqFocus::Detail => {
            if draft {
                out.push(("ctrl+s", "save draft", None));
            } else {
                out.push(("e", "edit the brief", None));
            }
            out.push(("↑↓", "scroll", None));
            if !remove.0.is_empty() {
                out.push((remove.0, remove.1, None));
            }
            out.push(("esc", "back to the list", None));
        }
    }
    // With one pane at a time (a narrow terminal) enter and esc go between them: there are no panes to switch.
    if !sqt.one_pane.get() {
        out.push(("tab", "next pane", None));
    }
    out.extend([("1-4", "tabs", None), ("s", "settings", Some(Action::Settings))]);
    // In the details the quit key goes back to the list (said above); only the list itself quits.
    out.push(("?", "help", Some(Action::Help)));
    if sqt.focus == SqFocus::List {
        out.push((app.quit_label(), "quit", Some(Action::Quit)));
    }
    out
}

fn count_words(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// What the list says about a squad: how many agents (beside its name), and under the name how it came to be. A
/// built-in squad as platoon has it is what most of the list is, so it says nothing there (its details say "Built
/// in"); the others say what sets them apart: "User Modified", "Your own".
fn entry_note(app: &App, i: usize, library: &[crate::agentfile::Agent]) -> (String, String, bool) {
    let sqt = &app.sqt;
    if i == sqt.list.len() {
        let n = sqt.draft.as_ref().map_or(0, |d| d.agent_ids().len());
        return (count_words(n, "agent"), "Draft, not saved".to_string(), true);
    }
    let Some(l) = sqt.list.get(i) else { return (String::new(), String::new(), false) };
    match &l.squad {
        None => (String::new(), "Can't be read".to_string(), true),
        Some(s) => {
            let broken = !squadfile::problems(s, library).is_empty();
            let kind = if l.kind == Kind::BuiltIn { String::new() } else { l.label() };
            let note = match (kind.is_empty(), broken) {
                (_, false) => kind,
                (true, true) => "Needs fixing".to_string(),
                (false, true) => format!("{kind} · needs fixing"),
            };
            (count_words(s.agent_ids().len(), "agent"), note, broken)
        }
    }
}

/// Below this width (of the area the tab is given) the Squads tab is one pane: the list, and enter opens the details
/// full width in its place (esc goes back to the list, on the same entry). From it, the list and the details side by
/// side. 78 is what an 80-column terminal gives the tab (a column of air at each side): from 80 columns, two panes.
pub const TWO_PANES_FROM: u16 = 78;

pub(super) fn draw_squads_tab(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    let one_pane = area.width < TWO_PANES_FROM;
    sqt.one_pane.set(one_pane);
    let (left, right) = if one_pane {
        // The pane that has the keys takes the whole width; the other isn't drawn.
        let none = Rect::new(area.x, area.y, 0, 0);
        if sqt.focus == SqFocus::List { (area, none) } else { (none, area) }
    } else {
        // A third of the width or so; wider, up to `LIST_MAX`, where the longest entry wants it and the details keep
        // `DETAILS_MIN` (so `report-writer` isn't cut while the details have room to spare).
        let base = (area.width * 30 / 100).clamp(26, 40);
        let left_w = base.max(list_want(app).min(LIST_MAX).min(area.width.saturating_sub(DETAILS_MIN))).min(area.width.saturating_sub(20));
        let [left, right] = Layout::horizontal([Constraint::Length(left_w), Constraint::Min(1)]).areas(area);
        (left, right)
    };
    let block = |focused: bool| {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(if focused { t.accent } else { t.border }))
            .style(Style::new().bg(t.bg))
    };
    // The list, with the switch in its title: the list it shows in brackets, in the accent colour.
    let focused = sqt.focus == SqFocus::List;
    let agents = sqt.view == SqView::Agents;
    let seg = |label: &str, on: bool| {
        let text = if on { format!("[{label}]") } else { format!(" {label} ") };
        let style = if on { Style::new().fg(t.accent).add_modifier(Modifier::BOLD) } else { Style::new().fg(t.dim) };
        Span::styled(text, style)
    };
    let title = Line::from(vec![Span::raw(" "), seg("Squads", !agents), Span::raw(" "), seg("Agents", agents), Span::raw(" ")]);
    if !left.is_empty() {
        let lb = block(focused).title(title);
        let li = lb.inner(left).inner(Margin::new(1, 0));
        f.render_widget(lb, left);
        {
            let mut hits = app.hits.borrow_mut();
            hits.push((Rect::new(left.x + 2, left.y, 8, 1), Hit::SquadView(false)));
            hits.push((Rect::new(left.x + 11, left.y, 8, 1), Hit::SquadView(true)));
        }
        let library = app.agent_library();
        if agents {
            draw_agent_list(f, app, li);
        } else {
            draw_squad_list(f, app, li, &library);
        }
    }
    if right.is_empty() {
        return;
    }
    // The details.
    let title = if agents {
        if sqt.new_agent_selected() {
            "New agent".to_string()
        } else {
            sqt.agent().map_or_else(|| sqt.agent_listed().map(|l| l.id.clone()).unwrap_or_default(), |a| a.name.clone())
        }
    } else {
        sqt.squad().map_or_else(|| sqt.listed().map(|l| l.id.clone()).unwrap_or_default(), |s| s.name.clone())
    };
    let dfocus = sqt.focus == SqFocus::Detail;
    let rb = block(dfocus)
        .title(Span::styled(format!(" {title} "), Style::new().fg(if dfocus { t.accent } else { t.dim }).add_modifier(Modifier::BOLD)));
    let ri = rb.inner(right).inner(Margin::new(1, 0));
    f.render_widget(rb, right);
    if agents {
        draw_agent_detail(f, app, ri);
    } else {
        draw_squad_detail(f, app, ri);
    }
}

/// The widest the list pane gets to fit its longest entry, and the room the details keep beside it.
const LIST_MAX: u16 = 56;
const DETAILS_MIN: u16 = 60;

/// How many squads use each agent (the draft counts).
fn squads_using(app: &App) -> HashMap<String, usize> {
    let sqt = &app.sqt;
    let mut used: HashMap<String, usize> = HashMap::new();
    for s in sqt.list.iter().filter_map(|l| l.squad.as_ref()).chain(sqt.draft.as_ref()) {
        for id in s.agent_ids() {
            *used.entry(id).or_default() += 1;
        }
    }
    used
}

/// The ids of platoon's own agents (built in or the user's): a coding tool's agent of the same id isn't used.
fn platoon_ids(app: &App) -> Vec<String> {
    app.sqt.agents.iter().filter(|l| l.kind != Kind::Claude).map(|l| l.id.clone()).collect()
}

/// How wide the list pane wants to be to show every entry whole: the marker, the longest name with what goes beside it
/// (an agent's model and squads, a squad's count of agents), the borders and the air inside them. Both lists count, so
/// the pane keeps its width when `←` `→` switch between them.
fn list_want(app: &App) -> u16 {
    let sqt = &app.sqt;
    let used = squads_using(app);
    let own = platoon_ids(app);
    let agents = sqt
        .agents
        .iter()
        .map(|l| l.name().width() + 1 + agent_right(l, &used, &own).0.width())
        .chain(sqt.agent_draft.as_ref().map(|d| d.name.width() + 1 + squadfile::model_label(&d.model).width()));
    let squads =
        sqt.list.iter().map(|l| l.name().width() + 2 + l.squad.as_ref().map_or(0, |s| count_words(s.agent_ids().len(), "agent").width()));
    let longest = agents.chain(squads).max().unwrap_or(0);
    u16::try_from(longest + 2 + 4).unwrap_or(u16::MAX)
}

fn drafting_line(f: &mut Frame, app: &App, li: Rect, y: u16) {
    let t = app.effective_theme();
    if let Some(k) = app.sqt.drafting()
        && y < li.bottom()
    {
        let spin = SPINNER[(app.tick % 10) as usize];
        let what = if k == crate::app::Drafting::Agent { "an agent" } else { "a squad" };
        f.render_widget(
            Paragraph::new(fit_end(&format!("{spin} Claude is drafting {what}…"), li.width as usize)).style(Style::new().fg(t.dim)),
            Rect::new(li.x, y, li.width, 1),
        );
    }
}

fn draw_squad_list(f: &mut Frame, app: &App, li: Rect, library: &[crate::agentfile::Agent]) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    // An entry is its name line, a line under it when there is something to say (see `entry_note`), and a spacer.
    let notes: Vec<(String, String, bool)> = (0..sqt.len()).map(|i| entry_note(app, i, library)).collect();
    let heights: Vec<u16> = notes.iter().map(|(_, note, _)| if note.is_empty() { 2 } else { 3 }).collect();
    app.pages.squads.set((li.height / 2).max(1) as usize);
    let start = project_list_start(&heights, sqt.selected, li.height);
    let mut y = li.y;
    for (i, (count, note, warn)) in notes.into_iter().enumerate().skip(start) {
        let h = heights[i] - 1;
        if y + h > li.bottom() {
            break;
        }
        let rect = Rect::new(li.x, y, li.width, h);
        app.hits.borrow_mut().push((rect, Hit::SquadEntry(i)));
        let sel = i == sqt.selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let name = if i == sqt.list.len() {
            sqt.draft.as_ref().map(|d| d.name.clone()).unwrap_or_default()
        } else {
            sqt.list[i].name().to_string()
        };
        let w = li.width.saturating_sub(2) as usize;
        // The count goes beside the name when there is room for both.
        let count_w = count.chars().count();
        let (name_w, count) = if name.chars().count() + 2 + count_w <= w { (w - count_w - 1, count) } else { (w, String::new()) };
        let name = fit_end(&name, name_w);
        let gap = w.saturating_sub(name.chars().count() + count.chars().count());
        let mut lines = vec![Line::from(vec![
            Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)),
            Span::styled(name, base.fg(t.text).add_modifier(Modifier::BOLD)),
            Span::styled(" ".repeat(gap), base),
            Span::styled(count, base.fg(t.dim)),
        ])];
        if !note.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  ", base),
                Span::styled(fit_end(&note, w), base.fg(if warn { t.warn } else { t.dim })),
            ]));
        }
        f.render_widget(Paragraph::new(lines).style(base), rect);
        y += h + 1;
    }
    drafting_line(f, app, li, y);
}

/// The agents list: grouped (built in, changed by the user, the user's own, the coding tool's "From Claude Code", a
/// draft), one line each with its model and how many squads use it, and "+ New agent" last.
fn draw_agent_list(f: &mut Frame, app: &App, li: Rect) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    let used = squads_using(app);
    let own = platoon_ids(app);
    // (the entry it is, if any; the line)
    let mut lines: Vec<(Option<usize>, Line<'static>)> = Vec::new();
    let heading = |text: &str| Line::styled(text.to_string(), Style::new().fg(t.dim).add_modifier(Modifier::BOLD));
    let w = li.width as usize;
    let sel_bg = |i: usize| if i == sqt.agent_selected { t.surface } else { t.bg };
    // What goes beside each name: "Opus · 2 squads", or only "Opus" when that doesn't fit on every row with the names
    // keeping `AGENT_NAME_MIN` cells (a longer name is cut with "…", never the data beside it), or nothing.
    let entries: Vec<(String, String)> = sqt
        .agents
        .iter()
        .map(|l| (l.name().to_string(), agent_right(l, &used, &own).0))
        .chain(sqt.agent_draft.as_ref().map(|d| (d.name.clone(), squadfile::model_label(&d.model).to_string())))
        .collect();
    let inner = w.saturating_sub(2);
    let fits = |part: &dyn Fn(&str) -> String| {
        entries.iter().all(|(n, r)| n.width().min(AGENT_NAME_MIN) + 1 + part(r).width() <= inner || part(r).is_empty())
    };
    let whole = |r: &str| r.to_string();
    let first = |r: &str| r.split(" · ").next().unwrap_or_default().to_string();
    let level = if fits(&whole) {
        0
    } else if fits(&first) {
        1
    } else {
        2
    };
    let row = |i: usize, name: String, right: String, warn: bool| -> Line<'static> {
        let base = Style::new().bg(sel_bg(i));
        let right = match level {
            0 => right,
            1 => first(&right),
            _ => String::new(),
        };
        let right_w = right.width();
        let name_w = if right.is_empty() { inner } else { inner.saturating_sub(right_w + 1) };
        let name = fit_end(&name, name_w);
        let gap = inner.saturating_sub(name.width() + right_w);
        Line::from(vec![
            Span::styled(if i == sqt.agent_selected { "▎ " } else { "  " }, base.fg(t.accent)),
            Span::styled(name, base.fg(t.text)),
            Span::styled(" ".repeat(gap), base),
            Span::styled(right, base.fg(if warn { t.warn } else { t.dim })),
        ])
        .style(base)
    };
    // The groups' headings say how each agent came to be. When every agent is built in, as platoon has them,
    // there is nothing to tell apart and no heading.
    let only_built_in = sqt.agents.iter().all(|l| l.kind == Kind::BuiltIn);
    let mut group: Option<String> = only_built_in.then(|| Kind::BuiltIn.label().to_string());
    for (i, l) in sqt.agents.iter().enumerate() {
        if group.as_deref() != Some(l.label().as_str()) {
            if group.is_some() {
                lines.push((None, Line::raw("")));
            }
            group = Some(l.label());
            lines.push((None, heading(&l.label())));
        }
        let (right, warn) = agent_right(l, &used, &own);
        lines.push((Some(i), row(i, l.name().to_string(), right, warn)));
    }
    if let Some(d) = sqt.agent_draft.as_ref() {
        lines.push((None, Line::raw("")));
        lines.push((None, Line::styled("Draft, not saved".to_string(), Style::new().fg(t.warn).add_modifier(Modifier::BOLD))));
        lines.push((Some(sqt.agents.len()), row(sqt.agents.len(), d.name.clone(), squadfile::model_label(&d.model).to_string(), false)));
    }
    lines.push((None, Line::raw("")));
    let new_i = sqt.agent_len() - 1;
    let base = Style::new().bg(sel_bg(new_i));
    lines.push((
        Some(new_i),
        Line::from(vec![
            Span::styled(if sqt.new_agent_selected() { "▎ " } else { "  " }, base.fg(t.accent)),
            Span::styled("+ New agent", base.fg(t.accent)),
        ])
        .style(base),
    ));
    // Scroll so the selected entry shows (with its heading when it can).
    let height = (li.height.saturating_sub(u16::from(sqt.drafting().is_some())) as usize).max(1);
    app.pages.squads.set(height);
    let at = lines.iter().position(|(r, _)| *r == Some(sqt.agent_selected)).unwrap_or(0);
    let mut top = sqt.agent_list_top.get().min(lines.len().saturating_sub(height));
    if at < top {
        top = at.saturating_sub(1);
    } else if at >= top + height {
        top = at + 1 - height;
    }
    sqt.agent_list_top.set(top);
    let mut y = li.y;
    for (entry, line) in lines.into_iter().skip(top).take(height) {
        let rect = Rect::new(li.x, y, li.width, 1);
        if let Some(i) = entry {
            app.hits.borrow_mut().push((rect, Hit::SquadEntry(i)));
        }
        f.render_widget(Paragraph::new(line), rect);
        y += 1;
    }
    drafting_line(f, app, li, y);
}

/// An agent's name keeps at least this many cells beside its model and squads (a longer one is cut with "…").
const AGENT_NAME_MIN: usize = 8;

/// A model as the list and the details say it: platoon's three by name; a coding tool's agent's as its file says it
/// ("inherits the leader's" when it says nothing or `inherit`).
fn model_words(a: &crate::agentfile::Agent) -> String {
    match a.model.as_str() {
        "" | "inherit" if a.from_file.is_some() => "Leader's model".to_string(),
        m => squadfile::model_label(m).to_string(),
    }
}

/// What the Agents list says beside an agent's name: "Opus · 2 squads", "Opus · not used", what is wrong with it, or,
/// for a coding tool's agent platoon has one of the same id of, that platoon's is used.
fn agent_right(l: &crate::agentfile::Listed, used: &HashMap<String, usize>, own: &[String]) -> (String, bool) {
    match &l.agent {
        None => ("can't be read".to_string(), true),
        Some(a) if l.kind == Kind::Claude && own.contains(&a.id) => (format!("{} · platoon's is used", model_words(a)), false),
        Some(a) if !crate::agentfile::problems(a).is_empty() => ("needs fixing".to_string(), true),
        Some(a) => {
            let n = used.get(&a.id).copied().unwrap_or(0);
            let usage = if n == 0 { "not used".to_string() } else { count_words(n, "squad") };
            (format!("{} · {usage}", model_words(a)), false)
        }
    }
}

const LABEL: usize = 19;

/// One row of details: a marker, a label and its value (wrapped to at most `wraps` lines).
#[allow(clippy::too_many_arguments)]
fn detail_line(
    lines: &mut Vec<(Option<usize>, Line<'static>)>,
    t: &Theme,
    w: usize,
    ri: usize,
    sel: bool,
    indent: &str,
    label: &str,
    value: &str,
    label_style: Style,
    value_style: Style,
    wraps: usize,
) {
    let base = Style::new().bg(if sel { t.surface } else { t.bg });
    let marker = Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent));
    let lw = LABEL.saturating_sub(indent.len());
    let label_text = format!("{indent}{label:<lw$}");
    let room = w.saturating_sub(2 + label_text.chars().count() + 1);
    let parts: Vec<String> = if wraps > 1 {
        let mut all = wrap(value, room.max(8));
        if all.len() > wraps {
            all.truncate(wraps);
            if let Some(last) = all.last_mut() {
                *last = fit_end(&format!("{last} …"), room.max(8));
            }
        }
        all
    } else {
        vec![fit_end(value, room)]
    };
    for (k, part) in parts.iter().enumerate() {
        let lead = if k == 0 { label_text.clone() } else { " ".repeat(label_text.chars().count()) };
        let m = if k == 0 { marker.clone() } else { Span::styled("  ", base) };
        lines.push((
            Some(ri),
            Line::from(vec![
                m,
                Span::styled(lead, base.patch(label_style)),
                Span::styled(" ", base),
                Span::styled(part.clone(), base.patch(value_style)),
            ])
            .style(base),
        ));
    }
    if parts.is_empty() {
        lines.push((Some(ri), Line::from(vec![marker, Span::styled(label_text, base.patch(label_style))]).style(base)));
    }
}

/// Draw `lines`, scrolled so the selected row shows, with a hit for each row's lines.
fn draw_scrolled(
    f: &mut Frame,
    app: &App,
    area: Rect,
    lines: Vec<(Option<usize>, Line<'static>)>,
    row: usize,
    top_cell: &std::cell::Cell<usize>,
    focused: bool,
) {
    // Text longer than the pane keeps its last row for "more lines below".
    let (_, height, _) = scroll_view(lines.len(), 0, area.height as usize);
    let first = lines.iter().position(|(r, _)| *r == Some(row)).unwrap_or(0);
    let last = lines.iter().rposition(|(r, _)| *r == Some(row)).unwrap_or(0);
    let mut top = top_cell.get().min(lines.len().saturating_sub(height));
    if focused || top > first {
        if first < top {
            // The first row shows what is above it too (what it is, what is wrong with it).
            top = first.saturating_sub(2);
        } else if last >= top + height {
            top = last + 1 - height;
        }
        // On the first row, what is above it shows too (what it is, what is wrong with it).
        if lines[..first].iter().all(|(r, _)| r.is_none()) && last < height {
            top = 0;
        }
    }
    top_cell.set(top);
    let below = lines.len().saturating_sub(top + height);
    for (k, (r, line)) in lines.into_iter().skip(top).take(height).enumerate() {
        let rect = Rect::new(area.x, area.y + k as u16, area.width, 1);
        if let Some(r) = r {
            app.hits.borrow_mut().push((rect, Hit::SquadDetail(r)));
        }
        f.render_widget(Paragraph::new(line), rect);
    }
    draw_more_below(f, app.effective_theme(), Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1), below);
}

fn unreadable(f: &mut Frame, app: &App, area: Rect, path: &std::path::Path, why: &str) {
    let t = app.effective_theme();
    let msg = format!(
        "{} can't be read ({why}), so platoon leaves it alone. Fix or move the file to go on: {}",
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        tilde(&path.to_string_lossy(), &app.home)
    );
    f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }).style(Style::new().fg(t.warn)), area);
}

fn yes_no(b: bool) -> String {
    if b { "Yes".into() } else { "No".into() }
}

/// A check of a brief as the Squads tab shows it: its mark, the mark's colour and what platoon does about it.
pub fn check_says(kind: &CheckKind, t: &Theme) -> (&'static str, Color, String) {
    match kind {
        CheckKind::Tests => ("✓", t.ok, "platoon runs the project's test command".to_string()),
        CheckKind::Read(_) => ("✓", t.ok, "platoon checks it ran after the last change".to_string()),
        CheckKind::Acceptance => ("✓", t.ok, "platoon checks the result ticks each box".to_string()),
        CheckKind::Gate => ("‖", t.subtext, "held by platoon".to_string()),
        CheckKind::Prose => ("·", t.dim, "the squad checks this itself".to_string()),
    }
}

/// The lines of a brief's Checks: each check with its mark, and what platoon does about it beside it when there is
/// room for both, else under it.
pub fn check_lines(s: &Squad, t: &Theme, w: usize) -> Vec<Line<'static>> {
    let kinds = s.check_kinds();
    let says: Vec<(&str, Color, String)> = kinds.iter().map(|(_, k)| check_says(k, t)).collect();
    // The column the words beside start at: after the longest check, when that leaves them room.
    let longest = kinds.iter().map(|(l, _)| l.width()).max().unwrap_or(0);
    let col = 2 + longest + 3;
    let mut out = Vec::new();
    for ((line, _), (mark, color, what)) in kinds.iter().zip(says) {
        let line = line.trim();
        if col + what.width() <= w {
            let pad = col.saturating_sub(2 + line.width());
            out.push(Line::from(vec![
                Span::styled(format!("{mark} "), Style::new().fg(color)),
                Span::styled(line.to_string(), Style::new().fg(t.text)),
                Span::raw(" ".repeat(pad)),
                Span::styled(what, Style::new().fg(t.subtext)),
            ]));
            continue;
        }
        for (k, part) in wrap(line, w.saturating_sub(2).max(8)).into_iter().enumerate() {
            let lead = if k == 0 { Span::styled(format!("{mark} "), Style::new().fg(color)) } else { Span::raw("  ") };
            out.push(Line::from(vec![lead, Span::styled(part, Style::new().fg(t.text))]));
        }
        for part in wrap(&what, w.saturating_sub(4).max(8)) {
            out.push(Line::from(vec![Span::raw("    "), Span::styled(part, Style::new().fg(t.subtext))]));
        }
    }
    out
}

/// The selected squad's brief, as it reads: what it is and where its file is, what is wrong with it (in the warn
/// colour, at the top), then What it does, its Steps as a rail, its Checks, Asks you, and the sections platoon doesn't
/// know. The arrows scroll it while the details have the keys.
fn draw_squad_detail(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    let w = area.width as usize;
    let Some(s) = sqt.squad() else {
        if let Some(l) = sqt.listed() {
            unreadable(f, app, area, &l.file, l.corrupt.as_deref().unwrap_or_default());
        }
        return;
    };
    let known = sqt.known_agents();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let draft = sqt.draft_selected();
    let kind = if draft {
        "Draft, not saved: ctrl+s saves it, d discards it".to_string()
    } else {
        sqt.listed().map_or(String::new(), squadfile::Listed::label)
    };
    lines.push(Line::styled(fit_end(&kind, w), Style::new().fg(if draft { t.warn } else { t.dim })));
    if let Some(l) = sqt.listed() {
        let shown = tilde(&l.file.to_string_lossy(), &app.home);
        let key = |k: &str, d: &str| {
            vec![Span::styled(format!("   {k}"), Style::new().fg(t.accent)), Span::styled(format!(" {d}"), Style::new().fg(hint_fg(t)))]
        };
        if l.file.exists() {
            let lead = "A file you own: ";
            let mut spans = vec![Span::styled(lead, Style::new().fg(t.dim)), Span::styled(shown.clone(), Style::new().fg(t.text))];
            let used = lead.width() + shown.width();
            let edit = key("e", "edit");
            if used + 9 <= w {
                spans.extend(edit);
            }
            if l.kind == Kind::Modified && used + 9 + 23 <= w {
                spans.extend(key("r", "restore the built-in"));
            }
            if used > w {
                lines.extend(wrap(&format!("{lead}{shown}"), w).into_iter().map(|p| Line::styled(p, Style::new().fg(t.dim))));
            } else {
                lines.push(Line::from(spans));
            }
        } else {
            for p in wrap(&format!("Built into platoon. e writes it to {shown} for you to change."), w) {
                lines.push(Line::styled(p, Style::new().fg(t.dim)));
            }
        }
    }
    let problems = squadfile::problems(s, &known);
    if !problems.is_empty() {
        let lead = if problems.len() == 1 {
            "It can't be handed a task until this is fixed:"
        } else {
            "It can't be handed a task until these are fixed:"
        };
        for p in wrap(lead, w) {
            lines.push(Line::styled(p, Style::new().fg(t.warn).add_modifier(Modifier::BOLD)));
        }
        for p in &problems {
            for (k, part) in wrap(p, w.saturating_sub(2).max(8)).into_iter().enumerate() {
                lines.push(Line::styled(format!("{}{part}", if k == 0 { "• " } else { "  " }), Style::new().fg(t.warn)));
            }
        }
    }
    let heading = |text: &str| Line::styled(text.to_string(), Style::new().fg(t.accent).add_modifier(Modifier::BOLD));
    lines.push(Line::raw(""));
    lines.push(heading(crate::brief::WHAT_IT_DOES));
    if s.can_do.trim().is_empty() {
        lines.push(Line::styled("(nothing yet)", Style::new().fg(t.dim)));
    } else {
        lines.extend(crate::mdview::render(&s.can_do, w, t, app.plain_marker));
    }
    for l in wrap("Every squad has the same leader: its system prompt and model are in Settings\u{a0}(s)\u{a0}→\u{a0}Projects.", w) {
        lines.push(Line::styled(l, Style::new().fg(t.dim)));
    }
    lines.push(Line::raw(""));
    lines.push(heading(crate::brief::STEPS));
    let rail = Rail { squad: s, known: &known, draft_agent: &|id: &str| sqt.is_draft_agent(id) };
    lines.extend(rail.lines(t, w));
    if !s.checks.is_empty() {
        lines.push(Line::raw(""));
        lines.push(heading(crate::brief::CHECKS));
        lines.extend(check_lines(s, t, w));
    }
    if !s.asks_you.trim().is_empty() {
        lines.push(Line::raw(""));
        lines.push(heading(crate::brief::ASKS_YOU));
        lines.extend(crate::mdview::render(&s.asks_you, w, t, app.plain_marker));
    }
    for (title, text) in &s.other {
        lines.push(Line::raw(""));
        if !title.is_empty() {
            lines.push(heading(title));
        }
        lines.extend(crate::mdview::render(text, w, t, app.plain_marker));
    }
    let total = lines.len();
    let (top, rows, below) = scroll_view(total, sqt.detail_top.get(), area.height as usize);
    sqt.detail_top.set(top);
    sqt.detail_size.set((total, rows));
    for (k, line) in lines.into_iter().skip(top).take(rows).enumerate() {
        let rect = Rect::new(area.x, area.y + k as u16, area.width, 1);
        app.hits.borrow_mut().push((rect, Hit::SquadDetail(top + k)));
        f.render_widget(Paragraph::new(line), rect);
    }
    draw_more_below(f, t, Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1), below);
}

/// The narrowest the text beside the rail may get before the send-back loops give way to their words alone.
pub const RAIL_TEXT_MIN: usize = 30;

/// A squad's steps as a rail: one mark per step on a line down the left (`○` the first, `●` the others), the step's
/// agent with its model and switches beside it, what it does under that, the send-back loops drawn on the left from
/// the step that sends the work back up to the step it goes back to (nested, so they cross only when they must),
/// each with "↩ back to …", then `◉ Done`.
pub struct Rail<'a> {
    pub squad: &'a Squad,
    /// Every agent a step may name.
    pub known: &'a [crate::agentfile::Agent],
    /// Is this agent one the draft brings?
    pub draft_agent: &'a dyn Fn(&str) -> bool,
}

/// One send-back loop, from step `from` back to step `to`.
#[derive(Clone, Copy, Debug)]
struct Loop {
    from: usize,
    to: usize,
    /// Its place among the loops going back to the same step: the farthest sender comes first (it is outermost).
    rank: usize,
    lane: usize,
}

impl Loop {
    /// Where it starts and ends, in the order of the lines: it goes in at the `rank`th line of step `to`, and comes
    /// out at step `from`'s last line.
    fn span(&self) -> ((usize, usize), (usize, usize)) {
        ((self.to, self.rank), (self.from, usize::MAX))
    }
}

/// The lanes the loops take, left (outermost) to right: longer loops first; a loop inside another goes to its right;
/// two that share no line may share a lane. The loops of `s`, with their lanes, and how many lanes there are.
fn loops_of(s: &Squad) -> (Vec<Loop>, usize) {
    let mut loops: Vec<Loop> = s
        .workflow
        .iter()
        .enumerate()
        .filter_map(|(from, st)| st.back.filter(|b| b.to < from).map(|b| Loop { from, to: b.to, rank: 0, lane: 0 }))
        .collect();
    for to in 0..s.workflow.len() {
        let mut into: Vec<usize> = (0..loops.len()).filter(|&k| loops[k].to == to).collect();
        into.sort_by_key(|&k| std::cmp::Reverse(loops[k].from));
        for (rank, k) in into.into_iter().enumerate() {
            loops[k].rank = rank;
        }
    }
    let mut order: Vec<usize> = (0..loops.len()).collect();
    order.sort_by_key(|&k| (std::cmp::Reverse(loops[k].from - loops[k].to), loops[k].rank, loops[k].from));
    let mut done: Vec<usize> = Vec::new();
    for k in order {
        let (a0, a1) = loops[k].span();
        let inside = done.iter().filter(|&&d| {
            let (b0, b1) = loops[d].span();
            b0 <= a0 && a1 <= b1
        });
        let min = inside.map(|&d| loops[d].lane + 1).max().unwrap_or(0);
        let lane = (min..)
            .find(|&l| {
                !done.iter().any(|&d| {
                    let (b0, b1) = loops[d].span();
                    loops[d].lane == l && a0 <= b1 && b0 <= a1
                })
            })
            .unwrap_or(min);
        loops[k].lane = lane;
        done.push(k);
    }
    let lanes = loops.iter().map(|l| l.lane + 1).max().unwrap_or(0);
    (loops, lanes)
}

/// What a line of the rail is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RailLine {
    /// Step `i`'s first line: its mark and its agent.
    Head(usize),
    /// A line of what step `i` does (`k`th).
    Does(usize, usize),
    /// A line with only the rail, so every loop going back to step `i` has a line of its own to go in at.
    Pad(usize),
    /// Where step `i`'s loop comes out: "↩ back to …".
    Back(usize),
}

/// A short word for each thing an agent may do besides reading: "files · commands", or "reads only".
fn short_switches(a: &crate::agentfile::Agent) -> String {
    let mut on: Vec<&str> =
        [(a.files, "files"), (a.commands, "commands"), (a.web, "web")].into_iter().filter(|(on, _)| *on).map(|(_, w)| w).collect();
    if !a.skills.is_empty() {
        on.push("skills");
    }
    if !a.mcp.is_empty() {
        on.push("MCP");
    }
    if on.is_empty() { "reads only".into() } else { on.join(" · ") }
}

/// Spans cut to `width` columns, the last one kept with "…" when they don't fit.
fn cut_spans(parts: Vec<(String, Style)>, width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut left = width;
    for (text, style) in parts {
        if left == 0 {
            break;
        }
        let n = text.chars().count();
        if n <= left {
            left -= n;
            out.push(Span::styled(text, style));
        } else {
            out.push(Span::styled(fit_end(&text, left), style));
            left = 0;
        }
    }
    out
}

impl Rail<'_> {
    /// The rail's lines, for a pane `w` columns wide.
    pub fn lines(&self, t: &Theme, w: usize) -> Vec<Line<'static>> {
        let s = self.squad;
        let (loops, lanes) = loops_of(s);
        // The loops' gutter: one column per lane, the arrow's column and a space. Too wide for the text beside it:
        // no loops drawn, their words stay.
        let mut gutter = if lanes > 0 { lanes + 2 } else { 0 };
        if w < 2 + gutter + 3 + RAIL_TEXT_MIN {
            gutter = 0;
        }
        let drawn = gutter > 0;
        let text_w = w.saturating_sub(2 + gutter + 3);
        let num_w = s.workflow.len().max(1).to_string().len();
        let under = text_w.saturating_sub(num_w + 1).max(1);
        let agent = |id: &str| self.known.iter().find(|a| a.id == id);
        let name_w = s
            .workflow
            .iter()
            .map(|st| agent(&st.agent).map_or(st.agent.chars().count(), |a| a.name.chars().count()))
            .max()
            .unwrap_or(0)
            .min(16);
        // The plan: which lines there are, in order.
        let mut plan: Vec<RailLine> = Vec::new();
        let mut does_text: Vec<Vec<String>> = Vec::new();
        for (i, st) in s.workflow.iter().enumerate() {
            plan.push(RailLine::Head(i));
            let mut d = wrap(&st.does, under);
            if d.len() > 3 {
                d.truncate(3);
                if let Some(last) = d.last_mut() {
                    *last = fit_end(&format!("{last} …"), under);
                }
            }
            if d.is_empty() {
                d.push(String::new());
            }
            for k in 0..d.len() {
                plan.push(RailLine::Does(i, k));
            }
            let entries = loops.iter().filter(|l| l.to == i).count();
            for _ in 0..entries.saturating_sub(1 + d.len()) {
                plan.push(RailLine::Pad(i));
            }
            if st.back.is_some_and(|b| b.to < i) {
                plan.push(RailLine::Back(i));
            }
            does_text.push(d);
        }
        // The loops on the gutter: which line each goes in at and comes out at.
        let mut grid: Vec<Vec<char>> = vec![vec![' '; gutter]; plan.len()];
        if drawn {
            let line_of = |want: RailLine| plan.iter().position(|p| *p == want);
            let mut vert = vec![vec![false; gutter]; plan.len()];
            let mut horiz = vec![vec![false; gutter]; plan.len()];
            let mut ends: Vec<(usize, usize, bool)> = Vec::new();
            for l in &loops {
                let first = line_of(RailLine::Head(l.to)).unwrap_or(0);
                let enter = first + l.rank;
                let Some(out) = line_of(RailLine::Back(l.from)) else { continue };
                for row in vert.iter_mut().take(out).skip(enter + 1) {
                    row[l.lane] = true;
                }
                for cell in horiz[enter].iter_mut().take(lanes).skip(l.lane + 1) {
                    *cell = true;
                }
                for cell in horiz[out].iter_mut().take(gutter).skip(l.lane + 1) {
                    *cell = true;
                }
                ends.push((enter, l.lane, true));
                ends.push((out, l.lane, false));
            }
            for (y, row) in grid.iter_mut().enumerate() {
                for (x, ch) in row.iter_mut().enumerate() {
                    *ch = match (vert[y][x], horiz[y][x]) {
                        (true, true) => '┼',
                        (true, false) => '│',
                        (false, true) => '─',
                        _ => ' ',
                    };
                }
            }
            for (y, lane, enter) in ends {
                grid[y][lane] = if enter { '┌' } else { '└' };
                if enter {
                    grid[y][lanes] = '▶';
                }
            }
        }
        let base = Style::new().bg(t.bg);
        let mut out = Vec::new();
        for (y, p) in plan.iter().enumerate() {
            let i = match *p {
                RailLine::Head(i) | RailLine::Does(i, _) | RailLine::Pad(i) | RailLine::Back(i) => i,
            };
            let mut spans = vec![Span::styled("  ", base)];
            spans.push(Span::styled(grid[y].iter().collect::<String>(), base.fg(t.dim)));
            let st = &s.workflow[i];
            match *p {
                RailLine::Head(_) => {
                    let mark = if i == 0 { "○" } else { "●" };
                    spans.push(Span::styled(mark, base.fg(t.subtext)));
                    spans.push(Span::styled("  ", base));
                    let mut parts = vec![(format!("{:>num_w$} ", i + 1), base.fg(t.dim))];
                    match agent(&st.agent) {
                        None => {
                            let id = if st.agent.is_empty() { "no agent" } else { st.agent.as_str() };
                            parts.push((format!("{id} · not found"), base.fg(t.warn)));
                        }
                        Some(a) => {
                            let pad = name_w.saturating_sub(a.name.chars().count());
                            parts.push((format!("{}{}", a.name, " ".repeat(pad)), base.fg(t.text).add_modifier(Modifier::BOLD)));
                            let model = model_words(a);
                            parts.push((format!("  {model:<6} "), base.fg(t.subtext)));
                            parts.push((short_switches(a), base.fg(t.subtext)));
                            if let Some(first) = crate::agentfile::problems(a).first() {
                                parts.push((format!(" · needs fixing: {first}"), base.fg(t.warn)));
                            } else if (self.draft_agent)(&a.id) {
                                parts.push((" · new with the draft".into(), base.fg(t.subtext)));
                            } else if a.from_file.is_some() {
                                parts.push((" · from your coding tool".into(), base.fg(t.subtext)));
                            }
                        }
                    }
                    spans.extend(cut_spans(parts, text_w));
                }
                RailLine::Does(_, k) => {
                    spans.push(Span::styled("│", base.fg(t.dim)));
                    spans.push(Span::styled("  ", base));
                    spans.push(Span::styled(" ".repeat(num_w + 1), base));
                    spans.push(Span::styled(does_text[i][k].clone(), base.fg(t.text)));
                }
                RailLine::Pad(_) => spans.push(Span::styled("│", base.fg(t.dim))),
                RailLine::Back(_) => {
                    spans.push(Span::styled(if drawn { "┤" } else { "│" }, base.fg(t.dim)));
                    spans.push(Span::styled("  ", base));
                    spans.push(Span::styled(" ".repeat(num_w + 1), base));
                    if let Some(b) = st.back {
                        let to =
                            s.workflow.get(b.to).map(|x| agent(&x.agent).map_or(x.agent.clone(), |a| a.name.clone())).unwrap_or_default();
                        // As much as fits: which step and its agent, then without the agent, then only how often.
                        let (n, times) = (b.to + 1, crate::brief::times_words(b.times));
                        let words = [
                            format!("↩ back to step {n} ({to}), at most {times}"),
                            format!("↩ back to step {n}, at most {times}"),
                            format!("↩ back to step {n}, {times}"),
                        ]
                        .into_iter()
                        .find(|w| w.chars().count() <= under)
                        .unwrap_or_else(|| fit_end(&format!("↩ step {n}, {times}"), under));
                        spans.push(Span::styled(words, base.fg(t.subtext)));
                    }
                }
            }
            out.push(Line::from(spans).style(base));
        }
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(" ".repeat(gutter)),
            Span::styled("◉", Style::new().fg(t.subtext)),
            Span::styled(fit_end("  Done: the result goes on the task", text_w + 2), Style::new().fg(t.dim)),
        ]));
        out
    }
}

/// The selected agent's details: what it is, its switches and system prompt, and the squads it is used in. One of the
/// user's coding tool's says where its file is, and that platoon only reads it.
fn draw_agent_detail(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    let w = area.width as usize;
    if sqt.new_agent_selected() {
        let msg = "Enter makes a new agent of your own: give it a name, then write its system prompt. Or n asks Claude to draft one from your description.";
        f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }).style(Style::new().fg(t.dim)), area);
        return;
    }
    let Some(a) = sqt.agent() else {
        if let Some(l) = sqt.agent_listed() {
            unreadable(f, app, area, &crate::agentfile::path(&app.config, &l.id), l.corrupt.as_deref().unwrap_or_default());
        }
        return;
    };
    let focused = sqt.focus == SqFocus::Detail;
    let users = app.agent_users(&a.id);
    let mut lines: Vec<(Option<usize>, Line<'static>)> = Vec::new();
    let draft = sqt.agent_draft_selected();
    let read_only = sqt.agent_read_only();
    let kind = if draft {
        "Draft, not saved: ctrl+s saves it, d discards it".to_string()
    } else {
        sqt.agent_listed().map_or(String::new(), crate::agentfile::Listed::label)
    };
    lines.push((None, Line::styled(fit_end(&kind, w), Style::new().fg(if draft { t.warn } else { t.dim }))));
    if let Some(file) = a.from_file.as_ref() {
        let shown = tilde(&file.to_string_lossy(), &app.home);
        lines.push((None, Line::styled(fit_start(&shown, w), Style::new().fg(t.text))));
        for l in wrap("Platoon only reads it: c copies it into platoon's own agents, to change it.", w) {
            lines.push((None, Line::styled(l, Style::new().fg(t.dim))));
        }
        if platoon_ids(app).contains(&a.id) {
            for l in wrap(&format!("Platoon has an agent called {} of its own: briefs use that one.", a.id), w) {
                lines.push((None, Line::styled(l, Style::new().fg(t.warn))));
            }
        }
    }
    if let Some(first) = crate::agentfile::problems(a).first() {
        for l in wrap(&format!("Squads can't use it until this is fixed: {first}"), w) {
            lines.push((None, Line::styled(l, Style::new().fg(t.warn))));
        }
    }
    let heading = |text: &str| Line::styled(text.to_string(), Style::new().fg(t.accent).add_modifier(Modifier::BOLD));
    lines.push((None, Line::raw("")));
    lines.push((None, heading("Agent")));
    if !read_only {
        for l in wrap("A change here is a change in every squad that uses it.", w) {
            lines.push((None, Line::styled(l, Style::new().fg(t.dim))));
        }
    }
    let mut used_heading = false;
    let installed = sqt.installed();
    // "a, b (not found here)": a name that isn't installed here is shown, and said so.
    let names = |has: &[String], there: &[String]| -> (String, bool) {
        if has.is_empty() {
            return (if read_only { "None".into() } else { "None: enter picks from what is installed".into() }, false);
        }
        let away = has.iter().any(|n| !there.contains(n));
        (
            has.iter().map(|n| if there.contains(n) { n.clone() } else { format!("{n} (not found here)") }).collect::<Vec<_>>().join(", "),
            away,
        )
    };
    for (ri, row) in crate::app::agent_rows(users.len()).iter().enumerate() {
        let sel = focused && ri == sqt.agent_row;
        let switch = |on: bool| (yes_no(on), Style::new().fg(if on { t.ok } else { t.dim }));
        let (label, value, style, wraps): (String, String, Style, usize) = match *row {
            ARow::Name => ("Name".into(), a.name.clone(), Style::new().fg(t.text), 1),
            ARow::Model => ("Model".into(), model_words(a), Style::new().fg(t.text), 1),
            ARow::Files => {
                let (v, s) = switch(a.files);
                ("Can change files".into(), v, s, 1)
            }
            ARow::Commands => {
                let (v, s) = switch(a.commands);
                ("Can run commands".into(), v, s, 1)
            }
            ARow::Web => {
                let (v, s) = switch(a.web);
                ("Can use the web".into(), v, s, 1)
            }
            ARow::Skills | ARow::Mcp => {
                let (label, (v, away)) = if *row == ARow::Skills {
                    ("Skills", names(&a.skills, &installed.skills))
                } else {
                    ("MCP servers", names(&a.mcp, &installed.mcp))
                };
                let none = if *row == ARow::Skills { a.skills.is_empty() } else { a.mcp.is_empty() };
                let style = Style::new().fg(if away && !read_only {
                    t.warn
                } else if none {
                    t.dim
                } else {
                    t.text
                });
                (label.into(), v, style, 3)
            }
            ARow::Prompt if a.guidance.trim().is_empty() => {
                ("System prompt".into(), "None yet: enter writes one".into(), Style::new().fg(t.warn), 1)
            }
            ARow::Prompt => {
                ("System prompt".into(), a.guidance.split_whitespace().collect::<Vec<_>>().join(" "), Style::new().fg(t.subtext), 6)
            }
            ARow::UsedIn(k) => {
                if !used_heading {
                    used_heading = true;
                    lines.push((None, Line::raw("")));
                    lines.push((None, heading("Used in")));
                }
                let u = &users[k];
                (u.name.clone(), squadfile::steps_words(&u.steps), Style::new().fg(t.text), 1)
            }
        };
        let label_style =
            if matches!(row, ARow::UsedIn(_)) { Style::new().fg(t.text).add_modifier(Modifier::BOLD) } else { Style::new().fg(t.dim) };
        detail_line(&mut lines, t, w, ri, sel, "", &label, &value, label_style, style, wraps);
    }
    if users.is_empty() {
        lines.push((None, Line::raw("")));
        lines.push((None, heading("Used in")));
        let says = format!(
            "No squad uses it yet. To use it, name it in a brief's step: \"1. {}: what it does\" (e on a squad opens its brief).",
            a.id
        );
        for l in wrap(&says, w) {
            lines.push((None, Line::styled(l, Style::new().fg(t.dim))));
        }
    }
    draw_scrolled(f, app, area, lines, sqt.agent_row, &sqt.agent_top, focused);
}

/// The small list over the tab: a model, an agent's skills or MCP servers, what to describe.
pub(super) fn draw_squad_choice(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let c = &app.sqt.choice;
    app.hits.borrow_mut().clear();
    let h = (c.options.len() as u16 + 5).min(area.height.saturating_sub(2)).max(6);
    let r = centered(area, area.width.saturating_sub(8).clamp(30, 76), h);
    popup_ground(f, t, area, r, true);
    let title = match c.purpose {
        ChoiceFor::AgentModel => "Model",
        ChoiceFor::AgentSkills => "Skills",
        ChoiceFor::AgentMcp => "MCP servers",
        ChoiceFor::Describe => "Describe",
    };
    let block = modal_block(t, title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [head, sep, list, foot] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(Paragraph::new(fit_end(&format!("{}:", c.title), head.width as usize)).style(Style::new().fg(t.text)), head);
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);
    let rows = list.height as usize;
    let start = c.selected.saturating_sub(rows.saturating_sub(1));
    for (i, (label, _)) in c.options.iter().enumerate().skip(start).take(rows) {
        let rect = Rect::new(list.x, list.y + (i - start) as u16, list.width, 1);
        app.hits.borrow_mut().push((rect, Hit::SquadChoiceRow(i)));
        let sel = i == c.selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let spans = vec![
            Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)),
            Span::styled(fit_end(label, (rect.width as usize).saturating_sub(2)), base.fg(t.text)),
        ];
        f.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
    }
    let hints: [Hint; 3] = [("enter", "choose", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel)), ("↑↓", "move", None)];
    draw_footer(f, app, foot, &hints, None);
}

/// The text box over the tab: a new squad or agent described for Claude.
pub(super) fn draw_squad_text(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let tx = &app.sqt.text;
    app.hits.borrow_mut().clear();
    let (title, note) = match &app.sqt.text_for {
        TextFor::Describe => (
            "Describe a new squad",
            "What it is for and the work it does. Claude drafts its brief on Sonnet, using your agents where they fit; you can change anything once it is saved.",
        ),
        TextFor::DescribeAgent => {
            ("Describe a new agent", "The one job it does. Claude drafts it on Sonnet; you change what you like before it is saved.")
        }
    };
    let body_h: u16 = 6;
    let r = centered(area, area.width.saturating_sub(8).clamp(40, 84), body_h + 7);
    popup_ground(f, t, area, r, true);
    let block = modal_block(t, title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [body, sep, note_area, _, foot] = Layout::vertical([
        Constraint::Length(body_h),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    let width = crate::textbox::content_width(body.width as usize);
    tx.view.set((width, body.height as usize));
    let chars: Vec<char> = tx.text.chars().collect();
    let rows = crate::textbox::layout(&chars, width);
    let (row, col) = crate::textbox::locate(&rows, &chars, tx.cursor);
    let top = crate::textbox::clamp_top(tx.top.get(), row, body.height as usize, rows.len());
    tx.top.set(top);
    for (n, rr) in rows.iter().skip(top).take(body.height as usize).enumerate() {
        let line: String = chars[rr.start..rr.end].iter().collect();
        f.render_widget(Paragraph::new(Span::styled(line, Style::new().fg(t.text))), Rect::new(body.x, body.y + n as u16, body.width, 1));
    }
    if row >= top && row < top + body.height as usize {
        f.set_cursor_position((body.x + (col as u16).min(body.width.saturating_sub(1)), body.y + (row - top) as u16));
    }
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);
    f.render_widget(Paragraph::new(note).wrap(Wrap { trim: true }).style(Style::new().fg(t.dim)), note_area);
    let hints: [Hint; 2] = [("enter", "ask Claude to draft it", None), ("esc", "cancel", Some(Action::Cancel))];
    draw_footer(f, app, foot, &hints, None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::squadfile::{Back, Step};

    /// Twelve steps, five send-backs: nested ones, and one that crosses another.
    fn long() -> Squad {
        let mut s = squadfile::builtin("coding").unwrap();
        let agents = ["planner", "builder", "reviewer", "verifier"];
        s.workflow =
            (0..12).map(|i| Step { agent: agents[i % 4].into(), does: format!("Does part {}.", i + 1), back: None, line: 0 }).collect();
        for (from, to, times) in [(11, 0, 1), (10, 1, 2), (7, 2, 3), (5, 3, 1), (8, 6, 2)] {
            s.workflow[from].back = Some(Back { to, times });
        }
        s
    }

    fn text(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    fn rail_of(s: &Squad, w: usize) -> Vec<String> {
        let known = crate::agentfile::builtins();
        let rail = Rail { squad: s, known: &known, draft_agent: &|_| false };
        text(&rail.lines(&Theme::default(), w))
    }

    #[test]
    fn five_loops_each_get_a_line_to_go_in_and_come_out_and_never_share_a_cell() {
        let s = long();
        let (loops, lanes) = loops_of(&s);
        assert_eq!((loops.len(), lanes), (5, 4), "{loops:?}");
        let lines = rail_of(&s, 50);
        let rail_col = 2 + lanes + 2;
        assert_eq!(lines.iter().filter(|l| l.contains('▶')).count(), 5, "one way in each");
        assert_eq!(lines.iter().filter(|l| l.chars().nth(rail_col) == Some('┤')).count(), 5, "one way out each");
        assert_eq!(lines.iter().filter(|l| l.contains("↩ back to step")).count(), 5);
        for l in &lines {
            assert!(l.chars().count() <= 50, "{l}");
            let rail = l.chars().nth(rail_col).unwrap_or(' ');
            assert!("○●│┤◉".contains(rail), "the rail runs down one column: {l:?}");
            // The gutter only ever holds loop pieces; an arrow always sits just left of the rail.
            let gutter: String = l.chars().skip(2).take(lanes + 2).collect();
            assert!(gutter.chars().all(|c| " │─┌└┼▶".contains(c)), "{gutter:?}");
            if let Some(at) = gutter.chars().position(|c| c == '▶') {
                assert_eq!(at, lanes, "{l:?}");
            }
        }
        // Each loop's vertical stays in its own lane from its way in to its way out: no two loops share a lane where
        // they overlap, and a loop inside another is to its right.
        for a in &loops {
            for b in &loops {
                let (a0, a1) = a.span();
                let (b0, b1) = b.span();
                if a.from != b.from && a0 <= b1 && b0 <= a1 {
                    assert_ne!(a.lane, b.lane, "{a:?} {b:?}");
                }
                if a.from != b.from && a0 <= b0 && b1 <= a1 {
                    assert!(a.lane < b.lane, "{a:?} holds {b:?}");
                }
            }
        }
        assert_eq!(lines.iter().filter(|l| l.contains('┼')).count(), 1, "the one crossing that can't be helped");
        assert!(lines[lines.len() - 1].contains("◉  Done"), "the rail ends at Done: the brief is the editor, no add-a-step row");
    }

    #[test]
    fn too_narrow_for_the_loops_keeps_their_words_only() {
        let s = long();
        let lines = rail_of(&s, 40);
        assert!(lines.iter().all(|l| !l.contains('▶') && !l.contains('┤') && !l.contains('┌')), "{lines:#?}");
        assert_eq!(lines.iter().filter(|l| l.contains("↩ back to step")).count(), 5, "{lines:#?}");
        for l in &lines {
            assert!(l.chars().count() <= 40, "{l}");
            assert!("○●│◉".contains(l.chars().nth(2).unwrap()), "the rail at the left edge: {l:?}");
        }
        // No send-backs at all: no gutter, the rail right after the margin.
        let plain = squadfile::builtin("summarize").unwrap();
        let lines = rail_of(&plain, 50);
        assert_eq!(lines[0].chars().nth(2), Some('○'));
        assert!(lines.iter().all(|l| !l.contains('↩')));
    }

    #[test]
    fn checks_say_what_platoon_does_beside_them_or_under_them() {
        let s = squadfile::builtin("research").unwrap();
        let t = Theme::default();
        let wide = text(&check_lines(&s, &t, 100));
        assert_eq!(wide.len(), 3);
        assert!(
            wide[0].starts_with("✓ the checker read the change") && wide[0].ends_with("platoon checks it ran after the last change"),
            "{wide:?}"
        );
        assert!(wide[1].starts_with("· every claim in the report names its source") && wide[1].ends_with("the squad checks this itself"));
        assert!(wide[2].starts_with("‖ nothing others see without your yes") && wide[2].ends_with("held by platoon"));
        let col = |l: &str| {
            let at = l.find("platoon").or_else(|| l.find("the squad")).or_else(|| l.find("held")).unwrap();
            l[..at].chars().count()
        };
        assert_eq!(col(&wide[0]), col(&wide[1]), "the words beside line up");
        let narrow = text(&check_lines(&s, &t, 40));
        assert!(narrow.len() > 3 && narrow.iter().all(|l| l.chars().count() <= 40), "{narrow:?}");
        assert!(narrow[1].starts_with("    platoon checks"), "under it, indented: {narrow:?}");
    }
}
