//! Drawing the Squads tab: the left pane's two lists (Squads or Agents, switched in its title), the selected squad's
//! or agent's details on the right, the key hints, and the small list and the text box that open over it.

use super::*;
use crate::app::{agent_line, back_words, detail_rows, ARow, ChoiceFor, DRow, SqFocus, SqView, TextFor};
use crate::squadfile::{self, Kind};
use std::collections::HashMap;

/// The keys of the Squads tab, for the list it shows, the pane that has them and the row the cursor is on.
pub(super) fn squads_tab_hints(app: &App) -> Vec<Hint<'static>> {
    let sqt = &app.sqt;
    let mut out: Vec<Hint<'static>> = Vec::new();
    let agents = sqt.view == SqView::Agents;
    let (draft, kind) = if agents { (sqt.agent_draft_selected(), sqt.agent_listed().map(|l| l.kind)) } else { (sqt.draft_selected(), sqt.listed().map(|l| l.kind)) };
    let remove = if draft {
        "discard"
    } else {
        match kind {
            Some(Kind::Own) => "delete",
            Some(Kind::Modified) => "restore",
            _ => "",
        }
    };
    match sqt.focus {
        SqFocus::List => {
            if agents && sqt.new_agent_selected() {
                out.push(("enter", "new agent", None));
            } else {
                out.push(("enter", "details", None));
            }
            out.push(("← →", if agents { "squads" } else { "agents" }, None));
            if draft {
                out.push(("ctrl+s", "save draft", None));
            }
            out.push(("n", "describe a new squad or agent", None));
            let readable = if agents { sqt.agent().is_some() && !sqt.new_agent_selected() } else { sqt.squad().is_some() };
            if !draft && readable {
                out.push(("c", "copy", None));
            }
            if !remove.is_empty() && !(agents && sqt.new_agent_selected()) {
                out.push(("d", remove, None));
            }
            out.push(("↑↓", "move", None));
        }
        SqFocus::Detail if agents => {
            match app.arow() {
                Some(ARow::Files | ARow::Commands | ARow::Web) => out.push(("space", "switch", None)),
                Some(ARow::Prompt) => out.push(("enter", "edit", None)),
                Some(ARow::UsedIn(_)) => out.push(("enter", "open the squad", None)),
                Some(_) => out.push(("enter", "change", None)),
                None => {}
            }
            if draft {
                out.push(("ctrl+s", "save draft", None));
            }
            out.push(("esc", "back to the list", None));
        }
        SqFocus::Detail => {
            match sqt.drow() {
                Some(DRow::NeedsFolder | DRow::Worktree) => out.push(("space", "switch", None)),
                Some(DRow::LeaderPrompt) => out.push(("enter", "edit", None)),
                Some(DRow::AddStep) => out.push(("enter", "add", None)),
                Some(DRow::StepAgent(_)) => out.push(("enter", "pick the agent", None)),
                Some(_) => out.push(("enter", "change", None)),
                None => {}
            }
            match sqt.drow() {
                Some(DRow::Step(_) | DRow::StepAgent(_)) => {
                    out.extend([("g", "go to the agent", None), ("b", "send back", None), ("[ ]", "move", None), ("a", "add step", None), ("x", "remove", None)]);
                }
                _ => out.push(("a", "add step", None)),
            }
            if draft {
                out.push(("ctrl+s", "save draft", None));
            }
            out.push(("esc", "back to the list", None));
        }
    }
    out.extend([("tab shift+arrows", "panes", None), ("1-3", "tabs", None), ("s", "settings", Some(Action::Settings)), ("q", "quit", Some(Action::Quit))]);
    out
}

fn count_words(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// What the list says about a squad: how many agents (beside its name), and under the name how it came to be.
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
            let note = if broken { format!("{} · needs fixing", l.kind.label()) } else { l.kind.label().to_string() };
            (count_words(s.agent_ids().len(), "agent"), note, broken)
        }
    }
}

pub(super) fn draw_squads_tab(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    let left_w = (area.width * 30 / 100).clamp(26, 40).min(area.width.saturating_sub(20));
    let [left, right] = Layout::horizontal([Constraint::Length(left_w), Constraint::Min(1)]).areas(area);
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
    // The details.
    let title = if agents {
        if sqt.new_agent_selected() { "New agent".to_string() } else { sqt.agent().map_or_else(|| sqt.agent_listed().map(|l| l.id.clone()).unwrap_or_default(), |a| a.name.clone()) }
    } else {
        sqt.squad().map_or_else(|| sqt.listed().map(|l| l.id.clone()).unwrap_or_default(), |s| s.name.clone())
    };
    let dfocus = sqt.focus == SqFocus::Detail;
    let rb = block(dfocus).title(Span::styled(format!(" {title} "), Style::new().fg(if dfocus { t.accent } else { t.dim }).add_modifier(Modifier::BOLD)));
    let ri = rb.inner(right).inner(Margin::new(1, 0));
    f.render_widget(rb, right);
    if agents {
        draw_agent_detail(f, app, ri);
    } else {
        draw_squad_detail(f, app, ri);
    }
}

fn drafting_line(f: &mut Frame, app: &App, li: Rect, y: u16) {
    let t = app.effective_theme();
    if let Some(k) = app.sqt.drafting() {
        if y < li.bottom() {
            let spin = SPINNER[(app.tick % 10) as usize];
            let what = if k == crate::app::Drafting::Agent { "an agent" } else { "a squad" };
            f.render_widget(Paragraph::new(fit_end(&format!("{spin} Claude is drafting {what}…"), li.width as usize)).style(Style::new().fg(t.dim)), Rect::new(li.x, y, li.width, 1));
        }
    }
}

fn draw_squad_list(f: &mut Frame, app: &App, li: Rect, library: &[crate::agentfile::Agent]) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    let per = 3u16;
    let room = (li.height / per).max(1) as usize;
    let start = sqt.selected.saturating_sub(room.saturating_sub(1));
    let mut y = li.y;
    for i in (0..sqt.len()).skip(start) {
        if y + 2 > li.bottom() {
            break;
        }
        let rect = Rect::new(li.x, y, li.width, 2);
        app.hits.borrow_mut().push((rect, Hit::SquadEntry(i)));
        let sel = i == sqt.selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let name = if i == sqt.list.len() { sqt.draft.as_ref().map(|d| d.name.clone()).unwrap_or_default() } else { sqt.list[i].name().to_string() };
        let (count, note, warn) = entry_note(app, i, library);
        let w = li.width.saturating_sub(2) as usize;
        // The count goes beside the name when there is room for both.
        let count_w = count.chars().count();
        let (name_w, count) = if name.chars().count() + 2 + count_w <= w { (w - count_w - 1, count) } else { (w, String::new()) };
        let name = fit_end(&name, name_w);
        let gap = w.saturating_sub(name.chars().count() + count.chars().count());
        let lines = vec![
            Line::from(vec![
                Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)),
                Span::styled(name, base.fg(t.text).add_modifier(Modifier::BOLD)),
                Span::styled(" ".repeat(gap), base),
                Span::styled(count, base.fg(t.dim)),
            ]),
            Line::from(vec![Span::styled("  ", base), Span::styled(fit_end(&note, w), base.fg(if warn { t.warn } else { t.dim }))]),
        ];
        f.render_widget(Paragraph::new(lines).style(base), rect);
        y += per;
    }
    drafting_line(f, app, li, y);
}

/// The agents list: grouped (built in, changed by the user, the user's own, a draft), one line each with its model and
/// how many squads use it, and "+ New agent" last.
fn draw_agent_list(f: &mut Frame, app: &App, li: Rect) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    // How many squads use each agent (the draft counts).
    let mut used: HashMap<String, usize> = HashMap::new();
    for s in sqt.list.iter().filter_map(|l| l.squad.as_ref()).chain(sqt.draft.as_ref()) {
        for id in s.agent_ids() {
            *used.entry(id).or_default() += 1;
        }
    }
    // (the entry it is, if any; the line)
    let mut lines: Vec<(Option<usize>, Line<'static>)> = Vec::new();
    let heading = |text: &str| Line::styled(text.to_string(), Style::new().fg(t.dim).add_modifier(Modifier::BOLD));
    let w = li.width as usize;
    let sel_bg = |i: usize| if i == sqt.agent_selected { t.surface } else { t.bg };
    let row = |i: usize, name: String, right: String, warn: bool| -> Line<'static> {
        let base = Style::new().bg(sel_bg(i));
        let inner = w.saturating_sub(2);
        // What goes beside the name, as much of it as fits: "Opus · 2 squads", "Opus", nothing.
        let right = [right.clone(), right.split(" · ").next().unwrap_or_default().to_string()]
            .into_iter()
            .find(|r| name.chars().count() + 2 + r.chars().count() <= inner)
            .unwrap_or_default();
        let right_w = right.chars().count();
        let name_w = if right.is_empty() { inner } else { inner - right_w - 1 };
        let name = fit_end(&name, name_w);
        let gap = inner.saturating_sub(name.chars().count() + right.chars().count());
        Line::from(vec![
            Span::styled(if i == sqt.agent_selected { "▎ " } else { "  " }, base.fg(t.accent)),
            Span::styled(name, base.fg(t.text)),
            Span::styled(" ".repeat(gap), base),
            Span::styled(right, base.fg(if warn { t.warn } else { t.dim })),
        ])
        .style(base)
    };
    let mut group = None;
    for (i, l) in sqt.agents.iter().enumerate() {
        if group != Some(l.kind) {
            if group.is_some() {
                lines.push((None, Line::raw("")));
            }
            group = Some(l.kind);
            lines.push((None, heading(l.kind.label())));
        }
        let (right, warn) = match &l.agent {
            None => ("can't be read".to_string(), true),
            Some(a) if !crate::agentfile::problems(a).is_empty() => ("needs fixing".to_string(), true),
            Some(a) => {
                let n = used.get(&a.id).copied().unwrap_or(0);
                let usage = if n == 0 { "not used".to_string() } else { count_words(n, "squad") };
                (format!("{} · {usage}", squadfile::model_label(&a.model)), false)
            }
        };
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
        Line::from(vec![Span::styled(if sqt.new_agent_selected() { "▎ " } else { "  " }, base.fg(t.accent)), Span::styled("+ New agent", base.fg(t.accent))]).style(base),
    ));
    // Scroll so the selected entry shows (with its heading when it can).
    let height = (li.height.saturating_sub(u16::from(sqt.drafting().is_some())) as usize).max(1);
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

const LABEL: usize = 19;

/// One row of details: a marker, a label and its value (wrapped to at most `wraps` lines).
#[allow(clippy::too_many_arguments)]
fn detail_line(lines: &mut Vec<(Option<usize>, Line<'static>)>, t: &Theme, w: usize, ri: usize, sel: bool, indent: &str, label: &str, value: &str, label_style: Style, value_style: Style, wraps: usize) {
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
        lines.push((Some(ri), Line::from(vec![m, Span::styled(lead, base.patch(label_style)), Span::styled(" ", base), Span::styled(part.clone(), base.patch(value_style))]).style(base)));
    }
    if parts.is_empty() {
        lines.push((Some(ri), Line::from(vec![marker, Span::styled(label_text, base.patch(label_style))]).style(base)));
    }
}

/// Draw `lines`, scrolled so the selected row shows, with a hit for each row's lines.
fn draw_scrolled(f: &mut Frame, app: &App, area: Rect, lines: Vec<(Option<usize>, Line<'static>)>, row: usize, top_cell: &std::cell::Cell<usize>, focused: bool) {
    let height = area.height as usize;
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
    for (k, (r, line)) in lines.into_iter().skip(top).take(height).enumerate() {
        let rect = Rect::new(area.x, area.y + k as u16, area.width, 1);
        if let Some(r) = r {
            app.hits.borrow_mut().push((rect, Hit::SquadDetail(r)));
        }
        f.render_widget(Paragraph::new(line), rect);
    }
}

fn unreadable(f: &mut Frame, app: &App, area: Rect, path: std::path::PathBuf, why: &str) {
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

fn first_line(text: &str) -> String {
    text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

/// The selected squad's details, one line per thing that can be changed, under the four section headings.
fn draw_squad_detail(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    let w = area.width as usize;
    let Some(s) = sqt.squad() else {
        if let Some(l) = sqt.listed() {
            unreadable(f, app, area, crate::squadfile::path(&app.config, &l.id), l.corrupt.as_deref().unwrap_or_default());
        }
        return;
    };
    let known = sqt.known_agents();
    let focused = sqt.focus == SqFocus::Detail;
    let mut lines: Vec<(Option<usize>, Line<'static>)> = Vec::new();
    let kind = if sqt.draft_selected() { "Draft, not saved: ctrl+s saves it, d discards it".to_string() } else { sqt.listed().map_or(String::new(), |l| l.kind.label().to_string()) };
    lines.push((None, Line::styled(fit_end(&kind, w), Style::new().fg(if sqt.draft_selected() { t.warn } else { t.dim }))));
    if let Some(first) = squadfile::problems(s, &known).first() {
        for l in wrap(&format!("Can't be handed a ticket until this is fixed: {first}"), w) {
            lines.push((None, Line::styled(l, Style::new().fg(t.warn))));
        }
    }
    let heading = |text: &str| Line::styled(text.to_string(), Style::new().fg(t.accent).add_modifier(Modifier::BOLD));
    let name_of = |id: &str| known.iter().find(|a| a.id == id).map_or_else(|| id.to_string(), |a| a.name.clone());
    let mut section = usize::MAX;
    for (ri, row) in detail_rows(s).iter().enumerate() {
        if row.section() != section {
            section = row.section();
            lines.push((None, Line::raw("")));
            lines.push((None, heading(["About", "Folder", "Leader", "Workflow"][section])));
        }
        if let DRow::Step(i) = row {
            if *i > 0 {
                lines.push((None, Line::raw("")));
            }
        }
        let mut warn = false;
        let (label, value): (String, String) = match *row {
            DRow::Name => ("Name".into(), s.name.clone()),
            DRow::CanDo => ("What it can do".into(), s.can_do.clone()),
            DRow::NeedsFolder => ("Needs a folder".into(), yes_no(s.needs_folder)),
            DRow::Worktree => ("Use a worktree for git folders".into(), yes_no(s.use_worktree)),
            DRow::LeaderModel => ("Model".into(), squadfile::model_label(&s.leader.model).to_string()),
            DRow::LeaderPrompt => ("System prompt".into(), first_line(&s.leader.guidance)),
            DRow::Step(i) => (format!("{}. {}", i + 1, name_of(&s.workflow[i].agent)), s.workflow[i].does.clone()),
            DRow::StepAgent(i) => {
                let id = &s.workflow[i].agent;
                let value = match known.iter().find(|a| a.id == *id) {
                    None => {
                        warn = true;
                        format!("{} isn't there: enter picks an agent", if id.is_empty() { "none" } else { id })
                    }
                    Some(a) => {
                        let mut v = agent_line(a);
                        if let Some(p) = crate::agentfile::problems(a).first() {
                            warn = true;
                            v = format!("{v} · needs fixing: {p}");
                        } else if sqt.is_draft_agent(id) {
                            v.push_str(" · new with the draft");
                        }
                        v
                    }
                };
                ("Agent".into(), value)
            }
            DRow::AddStep => ("+ Add a step".into(), String::new()),
        };
        let sel = focused && ri == sqt.row;
        let indent = if matches!(row, DRow::StepAgent(_)) { "  " } else { "" };
        let value_style = match row {
            DRow::NeedsFolder | DRow::Worktree if value == "Yes" => Style::new().fg(t.ok),
            DRow::NeedsFolder | DRow::Worktree => Style::new().fg(t.dim),
            DRow::LeaderPrompt => Style::new().fg(t.subtext),
            DRow::StepAgent(_) if warn => Style::new().fg(t.warn),
            DRow::StepAgent(_) => Style::new().fg(t.subtext),
            _ => Style::new().fg(t.text),
        };
        let label_style = match row {
            DRow::AddStep => Style::new().fg(t.accent),
            DRow::Step(_) => Style::new().fg(t.text).add_modifier(Modifier::BOLD),
            _ => Style::new().fg(t.dim),
        };
        let wraps = if matches!(row, DRow::CanDo | DRow::Step(_)) { 4 } else { 1 };
        detail_line(&mut lines, &t, w, ri, sel, indent, &label, &value, label_style, value_style, wraps);
        if let DRow::StepAgent(i) = row {
            if let Some(b) = s.workflow[*i].back {
                let base = Style::new().bg(if sel { t.surface } else { t.bg });
                let pad = " ".repeat(2 + LABEL + 1);
                lines.push((Some(ri), Line::from(vec![Span::styled(pad, base), Span::styled(fit_end(&format!("↩ {}", back_words(&b)), w.saturating_sub(LABEL + 3)), base.fg(t.warn))]).style(base)));
            }
        }
    }
    draw_scrolled(f, app, area, lines, sqt.row, &sqt.detail_top, focused);
}

/// The selected agent's details: what it is, its switches and system prompt, and the squads it is used in.
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
            unreadable(f, app, area, crate::agentfile::path(&app.config, &l.id), l.corrupt.as_deref().unwrap_or_default());
        }
        return;
    };
    let focused = sqt.focus == SqFocus::Detail;
    let users = app.agent_users(&a.id);
    let mut lines: Vec<(Option<usize>, Line<'static>)> = Vec::new();
    let draft = sqt.agent_draft_selected();
    let kind = if draft { "Draft, not saved: ctrl+s saves it, d discards it".to_string() } else { sqt.agent_listed().map_or(String::new(), |l| l.kind.label().to_string()) };
    lines.push((None, Line::styled(fit_end(&kind, w), Style::new().fg(if draft { t.warn } else { t.dim }))));
    if let Some(first) = crate::agentfile::problems(a).first() {
        for l in wrap(&format!("Squads can't use it until this is fixed: {first}"), w) {
            lines.push((None, Line::styled(l, Style::new().fg(t.warn))));
        }
    }
    let heading = |text: &str| Line::styled(text.to_string(), Style::new().fg(t.accent).add_modifier(Modifier::BOLD));
    lines.push((None, Line::raw("")));
    lines.push((None, heading("Agent")));
    for l in wrap("A change here is a change in every squad that uses it.", w) {
        lines.push((None, Line::styled(l, Style::new().fg(t.dim))));
    }
    let mut used_heading = false;
    for (ri, row) in crate::app::agent_rows(users.len()).iter().enumerate() {
        let sel = focused && ri == sqt.agent_row;
        let switch = |on: bool| (yes_no(on), Style::new().fg(if on { t.ok } else { t.dim }));
        let (label, value, style, wraps): (String, String, Style, usize) = match *row {
            ARow::Name => ("Name".into(), a.name.clone(), Style::new().fg(t.text), 1),
            ARow::Model => ("Model".into(), squadfile::model_label(&a.model).to_string(), Style::new().fg(t.text), 1),
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
            ARow::Prompt if a.guidance.trim().is_empty() => ("System prompt".into(), "None yet: enter writes one".into(), Style::new().fg(t.warn), 1),
            ARow::Prompt => ("System prompt".into(), a.guidance.split_whitespace().collect::<Vec<_>>().join(" "), Style::new().fg(t.subtext), 6),
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
        let label_style = if matches!(row, ARow::UsedIn(_)) { Style::new().fg(t.text).add_modifier(Modifier::BOLD) } else { Style::new().fg(t.dim) };
        detail_line(&mut lines, &t, w, ri, sel, "", &label, &value, label_style, style, wraps);
    }
    if users.is_empty() {
        lines.push((None, Line::raw("")));
        lines.push((None, heading("Used in")));
        for l in wrap("No squad uses it yet. To use it, pick it for a step: enter on a step's agent, on the Squads list.", w) {
            lines.push((None, Line::styled(l, Style::new().fg(t.dim))));
        }
    }
    draw_scrolled(f, app, area, lines, sqt.agent_row, &sqt.agent_top, focused);
}

/// The small list over the tab: a model, an agent, where a step sends the work back, how often, what to describe.
pub(super) fn draw_squad_choice(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let c = &app.sqt.choice;
    app.hits.borrow_mut().clear();
    let h = (c.options.len() as u16 + 5).min(area.height.saturating_sub(2)).max(6);
    let r = centered(area, area.width.saturating_sub(8).clamp(30, 76), h);
    f.render_widget(Clear, r);
    let title = match c.purpose {
        ChoiceFor::LeaderModel | ChoiceFor::AgentModel => "model",
        ChoiceFor::StepAgent(_) | ChoiceFor::NewStepAgent => "agent",
        ChoiceFor::BackTo(_) | ChoiceFor::BackTimes(..) => "send back",
        ChoiceFor::Describe => "describe",
    };
    let block = modal_block(t, title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [head, sep, list, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(Paragraph::new(fit_end(&format!("{}:", c.title), head.width as usize)).style(Style::new().fg(t.text)), head);
    f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(Style::new().fg(t.border)), sep);
    let rows = list.height as usize;
    let start = c.selected.saturating_sub(rows.saturating_sub(1));
    for (i, (label, value)) in c.options.iter().enumerate().skip(start).take(rows) {
        let rect = Rect::new(list.x, list.y + (i - start) as u16, list.width, 1);
        app.hits.borrow_mut().push((rect, Hit::SquadChoiceRow(i)));
        let sel = i == c.selected;
        let bg = if sel || rect.contains(app.mouse.into()) { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let fg = if value == crate::app::NEW_AGENT { t.accent } else { t.text };
        let spans = vec![Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)), Span::styled(fit_end(label, (rect.width as usize).saturating_sub(2)), base.fg(fg))];
        f.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
    }
    let hints: [Hint; 3] = [("enter", "choose", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel)), ("↑↓", "move", None)];
    draw_footer(f, app, foot, &hints, None);
}

/// The text box over the tab: a new squad or agent described for Claude, what a squad can do, what a step does.
pub(super) fn draw_squad_text(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let tx = &app.sqt.text;
    app.hits.borrow_mut().clear();
    let (title, note) = match &app.sqt.text_for {
        TextFor::Describe => (
            "describe a new squad".to_string(),
            "What it is for and the work it does. Claude drafts it on Sonnet, using your agents where they fit; you change what you like before it is saved.",
        ),
        TextFor::DescribeAgent => ("describe a new agent".to_string(), "The one job it does. Claude drafts it on Sonnet; you change what you like before it is saved."),
        TextFor::CanDo => ("what it can do".to_string(), "The commander reads this before a ticket is handed to the squad."),
        TextFor::StepDoes(i) => (format!("step {}", i + 1), "One line: what this step does. The leader reads it."),
        TextFor::NewStep(m) => (format!("a new step for {m}"), "One line: what this step does. The leader reads it."),
    };
    let describing = matches!(app.sqt.text_for, TextFor::Describe | TextFor::DescribeAgent);
    let body_h: u16 = if describing { 6 } else { 4 };
    let r = centered(area, area.width.saturating_sub(8).clamp(40, 84), body_h + 7);
    f.render_widget(Clear, r);
    let block = modal_block(t, &title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [body, sep, note_area, _, foot] =
        Layout::vertical([Constraint::Length(body_h), Constraint::Length(1), Constraint::Length(2), Constraint::Min(0), Constraint::Length(1)]).areas(inner);
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
    let keep = if describing { "ask Claude to draft it" } else { "save" };
    let hints: [Hint; 2] = [("enter", keep, None), ("esc", "cancel", Some(Action::Cancel))];
    draw_footer(f, app, foot, &hints, None);
}
