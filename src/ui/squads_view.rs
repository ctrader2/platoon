//! Drawing the Squads tab: the list of squads on the left, the selected one's details on the right (About, Leader,
//! Members, Workflow), its key hints, and the small list and the text box that open over it.

use super::*;
use crate::app::{back_words, detail_rows, ChoiceFor, DRow, SqFocus, TextFor};
use crate::squadfile::{self, Kind};

/// The keys of the Squads tab, for the pane that has them and the row the cursor is on.
pub(super) fn squads_tab_hints(app: &App) -> Vec<Hint<'static>> {
    let sqt = &app.sqt;
    let mut out: Vec<Hint<'static>> = Vec::new();
    let remove = if sqt.draft_selected() {
        "discard"
    } else {
        match sqt.listed().map(|l| l.kind) {
            Some(Kind::Own) => "delete",
            Some(Kind::Modified) => "restore",
            _ => "",
        }
    };
    match sqt.focus {
        SqFocus::List => {
            out.push(("enter", "details", None));
            if sqt.draft_selected() {
                out.push(("ctrl+s", "save draft", None));
            }
            out.push(("n", "describe a new squad", None));
            if !sqt.draft_selected() && sqt.squad().is_some() {
                out.push(("c", "copy", None));
            }
            if !remove.is_empty() {
                out.push(("d", remove, None));
            }
            out.push(("↑↓", "move", None));
        }
        SqFocus::Detail => {
            match sqt.drow() {
                Some(DRow::MemberFiles(_) | DRow::MemberCommands(_) | DRow::MemberWeb(_)) => out.push(("space", "switch", None)),
                Some(DRow::LeaderPrompt | DRow::MemberPrompt(_)) => out.push(("enter", "edit", None)),
                Some(DRow::AddMember | DRow::AddStep) => out.push(("enter", "add", None)),
                Some(_) => out.push(("enter", "change", None)),
                None => {}
            }
            match sqt.drow() {
                Some(DRow::Step(_)) => {
                    out.extend([("m", "member", None), ("b", "send back", None), ("[ ]", "move", None), ("a", "add step", None), ("x", "remove", None)]);
                }
                Some(DRow::AddStep) => out.push(("a", "add step", None)),
                Some(DRow::MemberName(_) | DRow::MemberModel(_) | DRow::MemberFiles(_) | DRow::MemberCommands(_) | DRow::MemberWeb(_) | DRow::MemberPrompt(_)) => {
                    out.extend([("a", "add member", None), ("x", "remove", None)]);
                }
                _ => out.push(("a", "add member", None)),
            }
            if sqt.draft_selected() {
                out.push(("ctrl+s", "save draft", None));
            }
            out.push(("esc", "back to the list", None));
        }
    }
    out.extend([("tab shift+arrows", "panes", None), ("1-3", "tabs", None), ("s", "settings", Some(Action::Settings)), ("q", "quit", Some(Action::Quit))]);
    out
}

/// What the list says about a squad: how many members (beside its name), and under the name how it came to be.
fn entry_note(app: &App, i: usize) -> (String, String, bool) {
    let sqt = &app.sqt;
    let members = |n: usize| format!("{n} member{}", if n == 1 { "" } else { "s" });
    if i == sqt.list.len() {
        let n = sqt.draft.as_ref().map_or(0, |d| d.members.len());
        return (members(n), "Draft, not saved".to_string(), true);
    }
    let Some(l) = sqt.list.get(i) else { return (String::new(), String::new(), false) };
    match &l.squad {
        None => (String::new(), "Can't be read".to_string(), true),
        Some(s) => {
            let broken = !squadfile::problems(s).is_empty();
            let note = if broken { format!("{} · needs fixing", l.kind.label()) } else { l.kind.label().to_string() };
            (members(s.members.len()), note, broken)
        }
    }
}

pub(super) fn draw_squads_tab(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    let left_w = (area.width * 30 / 100).clamp(26, 40).min(area.width.saturating_sub(20));
    let [left, right] = Layout::horizontal([Constraint::Length(left_w), Constraint::Min(1)]).areas(area);
    let pane = |title: String, focused: bool| {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(if focused { t.accent } else { t.border }))
            .title(Span::styled(format!(" {title} "), Style::new().fg(if focused { t.accent } else { t.dim }).add_modifier(Modifier::BOLD)))
            .style(Style::new().bg(t.bg))
    };
    // The list.
    let lb = pane("Squads".into(), sqt.focus == SqFocus::List);
    let li = lb.inner(left).inner(Margin::new(1, 0));
    f.render_widget(lb, left);
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
        let (count, note, warn) = entry_note(app, i);
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
    if sqt.drafting() && y < li.bottom() {
        let spin = SPINNER[(app.tick % 10) as usize];
        f.render_widget(Paragraph::new(fit_end(&format!("{spin} Claude is drafting a squad…"), li.width as usize)).style(Style::new().fg(t.dim)), Rect::new(li.x, y, li.width, 1));
    }
    // The details.
    let title = sqt.squad().map_or_else(|| sqt.listed().map(|l| l.id.clone()).unwrap_or_default(), |s| s.name.clone());
    let rb = pane(title, sqt.focus == SqFocus::Detail);
    let ri = rb.inner(right).inner(Margin::new(1, 0));
    f.render_widget(rb, right);
    draw_squad_detail(f, app, ri);
}

/// The selected squad's details, one line per thing that can be changed, under the four section headings.
fn draw_squad_detail(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    let w = area.width as usize;
    let Some(s) = sqt.squad() else {
        if let Some(l) = sqt.listed() {
            let why = l.corrupt.clone().unwrap_or_default();
            let msg = format!(
                "{} can't be read ({why}), so platoon leaves it alone. Fix or move the file to go on: {}",
                crate::squadfile::path(&app.config, &l.id).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                tilde(&crate::squadfile::path(&app.config, &l.id).to_string_lossy(), &app.home)
            );
            f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }).style(Style::new().fg(t.warn)), area);
        }
        return;
    };
    const LABEL: usize = 19;
    let focused = sqt.focus == SqFocus::Detail;
    // (the row it belongs to, if any; the line)
    let mut lines: Vec<(Option<usize>, Line<'static>)> = Vec::new();
    let kind = if sqt.draft_selected() { "Draft, not saved: ctrl+s saves it, d discards it".to_string() } else { sqt.listed().map_or(String::new(), |l| l.kind.label().to_string()) };
    lines.push((None, Line::styled(fit_end(&kind, w), Style::new().fg(if sqt.draft_selected() { t.warn } else { t.dim }))));
    let problems = squadfile::problems(s);
    if let Some(first) = problems.first() {
        for l in wrap(&format!("Can't be handed a ticket until this is fixed: {first}"), w) {
            lines.push((None, Line::styled(l, Style::new().fg(t.warn))));
        }
    }
    let rows = detail_rows(s);
    let heading = |text: &str| Line::styled(text.to_string(), Style::new().fg(t.accent).add_modifier(Modifier::BOLD));
    let mut section = usize::MAX;
    for (ri, row) in rows.iter().enumerate() {
        if row.section() != section {
            section = row.section();
            lines.push((None, Line::raw("")));
            lines.push((None, heading(["About", "Leader", "Members", "Workflow"][section])));
        }
        if let DRow::MemberName(i) = row {
            if *i > 0 {
                lines.push((None, Line::raw("")));
            }
        }
        let yes_no = |b: bool| if b { "Yes".to_string() } else { "No".to_string() };
        let first_line = |text: &str| text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        let (label, value): (String, String) = match *row {
            DRow::Name => ("Name".into(), s.name.clone()),
            DRow::CanDo => ("What it can do".into(), s.can_do.clone()),
            DRow::LeaderModel => ("Model".into(), squadfile::model_label(&s.leader.model).to_string()),
            DRow::LeaderPrompt => ("System prompt".into(), first_line(&s.leader.guidance)),
            DRow::MemberName(i) => ("Name".into(), s.members[i].name.clone()),
            DRow::MemberModel(i) => ("Model".into(), squadfile::model_label(&s.members[i].model).to_string()),
            DRow::MemberFiles(i) => ("Can change files".into(), yes_no(s.members[i].files)),
            DRow::MemberCommands(i) => ("Can run commands".into(), yes_no(s.members[i].commands)),
            DRow::MemberWeb(i) => ("Can use the web".into(), yes_no(s.members[i].web)),
            DRow::MemberPrompt(i) => ("System prompt".into(), first_line(&s.members[i].guidance)),
            DRow::AddMember => ("+ Add a member".into(), String::new()),
            DRow::Step(i) => (format!("{}. {}", i + 1, s.workflow[i].member), s.workflow[i].does.clone()),
            DRow::AddStep => ("+ Add a step".into(), String::new()),
        };
        let sel = focused && ri == sqt.row;
        let bg = if sel { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let marker = Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent));
        let indent = if matches!(row.section(), 2) && !matches!(row, DRow::AddMember) { "  " } else { "" };
        let value_style = match row {
            DRow::MemberFiles(_) | DRow::MemberCommands(_) | DRow::MemberWeb(_) if value == "Yes" => base.fg(t.ok),
            DRow::MemberFiles(_) | DRow::MemberCommands(_) | DRow::MemberWeb(_) => base.fg(t.dim),
            DRow::LeaderPrompt | DRow::MemberPrompt(_) => base.fg(t.subtext),
            _ => base.fg(t.text),
        };
        let label_style = match row {
            DRow::AddMember | DRow::AddStep => base.fg(t.accent),
            DRow::MemberName(_) | DRow::Step(_) => base.fg(t.text).add_modifier(Modifier::BOLD),
            _ => base.fg(t.dim),
        };
        let lw = LABEL.saturating_sub(indent.len());
        let label_text = format!("{indent}{label:<lw$}");
        let room = w.saturating_sub(2 + label_text.chars().count() + 1);
        // What a squad can do and what a step does wrap (a few lines at most); the rest is one line, cut.
        let wraps = matches!(row, DRow::CanDo | DRow::Step(_));
        let parts: Vec<String> = if wraps {
            let mut all = wrap(&value, room.max(8));
            if all.len() > 4 {
                all.truncate(4);
                if let Some(last) = all.last_mut() {
                    *last = fit_end(&format!("{last} …"), room.max(8));
                }
            }
            all
        } else {
            vec![fit_end(&value, room)]
        };
        for (k, part) in parts.iter().enumerate() {
            let lead = if k == 0 { label_text.clone() } else { " ".repeat(label_text.chars().count()) };
            let m = if k == 0 { marker.clone() } else { Span::styled("  ", base) };
            lines.push((Some(ri), Line::from(vec![m, Span::styled(lead, label_style), Span::styled(" ", base), Span::styled(part.clone(), value_style)]).style(base)));
        }
        if parts.is_empty() {
            lines.push((Some(ri), Line::from(vec![marker, Span::styled(label_text.clone(), label_style)]).style(base)));
        }
        if let DRow::Step(i) = row {
            if let Some(b) = s.workflow[*i].back {
                let pad = " ".repeat(2 + label_text.chars().count() + 1);
                lines.push((Some(ri), Line::from(vec![Span::styled(pad, base), Span::styled(fit_end(&format!("↩ {}", back_words(&b)), room), base.fg(t.warn))]).style(base)));
            }
        }
    }
    // Scroll so the selected row shows.
    let height = area.height as usize;
    let first = lines.iter().position(|(r, _)| *r == Some(sqt.row)).unwrap_or(0);
    let last = lines.iter().rposition(|(r, _)| *r == Some(sqt.row)).unwrap_or(0);
    let mut top = sqt.detail_top.get().min(lines.len().saturating_sub(height));
    if focused || top > first {
        if first < top {
            top = first.saturating_sub(2);
        } else if last >= top + height {
            top = last + 1 - height;
        }
    }
    sqt.detail_top.set(top);
    for (k, (row, line)) in lines.into_iter().skip(top).take(height).enumerate() {
        let rect = Rect::new(area.x, area.y + k as u16, area.width, 1);
        if let Some(r) = row {
            app.hits.borrow_mut().push((rect, Hit::SquadDetail(r)));
        }
        f.render_widget(Paragraph::new(line), rect);
    }
}

/// The small list over the tab: a model, a member, where a step sends the work back, how often.
pub(super) fn draw_squad_choice(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let c = &app.sqt.choice;
    app.hits.borrow_mut().clear();
    let h = (c.options.len() as u16 + 5).min(area.height.saturating_sub(2)).max(6);
    let r = centered(area, area.width.saturating_sub(8).clamp(30, 76), h);
    f.render_widget(Clear, r);
    let title = match c.purpose {
        ChoiceFor::LeaderModel | ChoiceFor::MemberModel(_) => "model",
        ChoiceFor::StepMember(_) | ChoiceFor::NewStepMember => "member",
        ChoiceFor::BackTo(_) | ChoiceFor::BackTimes(..) => "send back",
    };
    let block = modal_block(t, title);
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let [head, sep, list, foot] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(inner);
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
        let spans = vec![Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)), Span::styled(fit_end(label, (rect.width as usize).saturating_sub(2)), base.fg(t.text))];
        f.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
    }
    let hints: [Hint; 3] = [("enter", "choose", Some(Action::Confirm)), ("esc", "cancel", Some(Action::Cancel)), ("↑↓", "move", None)];
    draw_footer(f, app, foot, &hints, None);
}

/// The text box over the tab: a new squad described for Claude, what a squad can do, what a step does.
pub(super) fn draw_squad_text(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let tx = &app.sqt.text;
    app.hits.borrow_mut().clear();
    let (title, note) = match &app.sqt.text_for {
        TextFor::Describe => ("describe a new squad".to_string(), "What it is for, and who is in it. Claude drafts it on Sonnet; you change what you like before it is saved."),
        TextFor::CanDo => ("what it can do".to_string(), "The commander reads this before a ticket is handed to the squad."),
        TextFor::StepDoes(i) => (format!("step {}", i + 1), "One line: what this step does. The leader reads it."),
        TextFor::NewStep(m) => (format!("a new step for {m}"), "One line: what this step does. The leader reads it."),
    };
    let body_h: u16 = if app.sqt.text_for == TextFor::Describe { 6 } else { 4 };
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
    let keep = if app.sqt.text_for == TextFor::Describe { "ask Claude to draft it" } else { "save" };
    let hints: [Hint; 2] = [("enter", keep, None), ("esc", "cancel", Some(Action::Cancel))];
    draw_footer(f, app, foot, &hints, None);
}
