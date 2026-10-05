//! Drawing the Briefs tab: the left pane's two lists (Squads or Agents, switched in its title), the selected squad's
//! brief or agent's details on the right, the key hints, and the small list and the text box that open over it.

use super::*;
use crate::app::{ARow, ChoiceFor, SqFocus, SqView, TextFor};
use crate::brief::CheckKind;
use crate::squadfile::Squad;
use crate::squadfile::{self, Kind};
use std::collections::HashMap;

/// The keys of the Briefs tab, for the list it shows, the pane that has them and the row the cursor is on.
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
            out.push(("← →", if agents { "briefs" } else { "agents" }, None));
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

/// Below this width (of the area the tab is given) the Briefs tab is one pane: the list, and enter opens the details
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
    let title = Line::from(vec![Span::raw(" "), seg("Briefs", !agents), Span::raw(" "), seg("Agents", agents), Span::raw(" ")]);
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

/// A check of a brief as the Briefs tab shows it: its mark, the mark's colour and what platoon does about it.
pub fn check_says(kind: &CheckKind, t: &Theme) -> (&'static str, Color, String) {
    match kind {
        CheckKind::Tests => ("✓", t.ok, "platoon runs the project's tests".to_string()),
        CheckKind::Read(_) => ("✓", t.ok, "platoon checks it ran after changes".to_string()),
        CheckKind::Acceptance => ("✓", t.ok, "platoon checks the squad's result".to_string()),
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
/// colour, at the top), then What it does, its Steps as the file numbers them, its Checks, Asks you, and the sections platoon doesn't
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
            let used = lead.width() + shown.width();
            let restore = l.kind == Kind::Modified;
            let keys_w = 9 + if restore { 23 } else { 0 };
            let mut keys = key("e", "edit");
            if restore {
                keys.extend(key("r", "restore the built-in"));
            }
            if used + keys_w <= w {
                let mut spans = vec![Span::styled(lead, Style::new().fg(t.dim)), Span::styled(shown.clone(), Style::new().fg(t.text))];
                spans.extend(keys);
                lines.push(Line::from(spans));
            } else {
                // Too long for the keys beside it: the path wrapped, the keys on a line of their own.
                lines.extend(wrap(&format!("{lead}{shown}"), w).into_iter().map(|p| Line::styled(p, Style::new().fg(t.dim))));
                let mut own = keys;
                if let Some(first) = own.first_mut() {
                    first.content = first.content.trim_start().to_string().into();
                }
                lines.push(Line::from(own));
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
    lines.extend(step_lines(s, t, w));
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
    if !s.agent_ids().is_empty() {
        lines.push(Line::raw(""));
        lines.push(heading(AGENTS_IT_MAY_USE));
        lines.extend(agent_lines(s, &known, &|id: &str| sqt.is_draft_agent(id), t, w));
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

/// The heading of the last section of a brief's pane: the agents its steps name (not a section of the file).
pub const AGENTS_IT_MAY_USE: &str = "Agents it may use";

/// A brief's Steps as the file has them: one numbered line per step, "agent: what it does", the send-back sentence
/// as part of its text, wrapped under the text (not under the number).
pub fn step_lines(s: &Squad, t: &Theme, w: usize) -> Vec<Line<'static>> {
    let num_w = s.workflow.len().max(1).to_string().len() + 2;
    let room = w.saturating_sub(num_w).max(8);
    let mut out = Vec::new();
    for (i, st) in s.workflow.iter().enumerate() {
        let mut text = format!("{}: {}", st.agent, st.does.trim());
        if let Some(b) = &st.back {
            text.push(' ');
            text.push_str(&crate::brief::send_back_sentence(b));
        }
        for (k, part) in wrap(text.trim_end(), room).into_iter().enumerate() {
            let lead = if k == 0 { format!("{:>w$}", format!("{}. ", i + 1), w = num_w) } else { " ".repeat(num_w) };
            let mut spans = vec![Span::styled(lead, Style::new().fg(t.dim))];
            let name = format!("{}:", st.agent);
            match part.strip_prefix(&name).filter(|_| k == 0) {
                Some(rest) => {
                    spans.push(Span::styled(name, Style::new().fg(t.text).add_modifier(Modifier::BOLD)));
                    spans.push(Span::styled(rest.to_string(), Style::new().fg(t.text)));
                }
                None => spans.push(Span::styled(part, Style::new().fg(t.text))),
            }
            out.push(Line::from(spans));
        }
    }
    out
}

/// The agents a brief's steps name, each once, in the order they first come: "name · model · switches".
fn agent_lines(
    s: &Squad,
    known: &[crate::agentfile::Agent],
    draft_agent: &dyn Fn(&str) -> bool,
    t: &Theme,
    w: usize,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for id in s.agent_ids() {
        let mut parts: Vec<(String, Style)> = Vec::new();
        match known.iter().find(|a| a.id == id) {
            None => parts.push((format!("{id} · not found"), Style::new().fg(t.warn))),
            Some(a) => {
                parts.push((a.name.clone(), Style::new().fg(t.text).add_modifier(Modifier::BOLD)));
                parts.push((format!(" · {} · {}", model_words(a), short_switches(a)), Style::new().fg(t.subtext)));
                if let Some(first) = crate::agentfile::problems(a).first() {
                    parts.push((format!(" · needs fixing: {first}"), Style::new().fg(t.warn)));
                } else if draft_agent(&a.id) {
                    parts.push((" · new with the draft".into(), Style::new().fg(t.subtext)));
                } else if a.from_file.is_some() {
                    parts.push((" · from your coding tool".into(), Style::new().fg(t.subtext)));
                }
            }
        }
        out.push(Line::from(cut_spans(parts, w)));
    }
    out
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
    use crate::squadfile::Back;

    fn text(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn steps_read_as_the_numbered_list_of_the_file() {
        let mut s = squadfile::builtin("coding").unwrap();
        s.workflow[3].back = Some(Back { to: 1, times: 2 });
        let lines = text(&step_lines(&s, &Theme::default(), 60));
        assert!(lines[0].starts_with("1. planner: "), "{lines:?}");
        assert!(lines.iter().all(|l| l.chars().count() <= 60), "{lines:?}");
        let joined = lines.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(joined.contains("May send the work back to step 2, at most twice."), "{joined}");
        // A wrapped step goes on under its text, not under its number.
        let at = lines.iter().position(|l| l.starts_with("2. ")).unwrap();
        assert!(lines[at + 1].starts_with("   ") && !lines[at + 1].starts_with("    "), "{lines:?}");
        assert!(lines.iter().all(|l| !l.contains('○') && !l.contains('↩') && !l.contains('◉')), "no rail: {lines:?}");
    }

    #[test]
    fn checks_say_what_platoon_does_beside_them_or_under_them() {
        let s = squadfile::builtin("research").unwrap();
        let t = Theme::default();
        let wide = text(&check_lines(&s, &t, 100));
        assert_eq!(wide.len(), 3);
        assert!(
            wide[0].starts_with("✓ the checker read the change") && wide[0].ends_with("platoon checks it ran after changes"),
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
