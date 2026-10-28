//! The help popup (`?`): every key of the screen showing, from the same table its footer uses (so the two can't drift),
//! with the settings and the search where they work; the keys that work the same all over platoon (with the user's own
//! back and quit keys from `keys.json`); and a few things worth knowing. It scrolls when it is longer than the screen.

use super::{centered, hint_fg, modal_block, popup_ground, screen_hints, tasks_view, wrap};
use crate::app::{App, Hit, TOPS};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

/// One line of the help before it is wrapped to the popup's width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Entry {
    Heading(&'static str),
    Key(String, String),
    Note(String),
    Blank,
}

/// Hints the help lists once, under "Anywhere in platoon", rather than with the screen's own keys: the tabs, quit,
/// this help, and the screen's own esc "back" (the back key's line says esc does the same).
const GLOBAL: &[&str] = &["tabs", "quit", "help", "back"];

/// The keys that work the same on every one of platoon's own screens: the same list wherever the help is opened.
fn anywhere(app: &App) -> Vec<(String, String)> {
    vec![
        (format!("1-{}", TOPS.len()), format!("the tabs: {}", TOPS.join(", "))),
        (app.back_label().to_string(), "back one level (esc does the same)".into()),
        (app.quit_label().to_string(), "quit platoon, on the Sessions, Projects, Tasks and Squads lists (elsewhere it goes back)".into()),
        ("?".into(), "this help".into()),
    ]
}

/// Everything the help says for the screen showing.
pub(super) fn entries(app: &App) -> Vec<Entry> {
    let mut out = vec![Entry::Heading("On this screen")];
    // The Tasks tab's footer has the keys of the row picked; the help has all of them.
    let hints = if app.board_showing() && app.tk.open.is_none() { tasks_view::board_hints(app, true) } else { screen_hints(app) };
    let mut screen: Vec<(String, String)> =
        hints.into_iter().filter(|(_, d, _)| !GLOBAL.contains(d)).map(|(k, d, _)| (k.to_string(), d.to_string())).collect();
    // The settings and the search open from most screens but not all: listed here, on the screens where they work.
    for (works, k, d) in [(app.slash_searches(), "/", "search"), (app.s_opens_settings(), "s", "settings")] {
        if works && !screen.iter().any(|(_, have)| have == d) {
            screen.push((k.into(), d.into()));
        }
    }
    // The footer says only "←→ sessions/notes"; tab does it too, from either pane to the other.
    if let Some(at) = screen.iter().position(|(_, d)| d == "sessions/notes") {
        screen.insert(at + 1, ("tab".into(), "the other pane".into()));
    }
    out.extend(screen.into_iter().map(|(k, d)| Entry::Key(k, d)));
    out.push(Entry::Blank);
    out.push(Entry::Heading("Anywhere in platoon"));
    out.extend(anywhere(app).into_iter().map(|(k, d)| Entry::Key(k, d)));
    out.push(Entry::Blank);
    out.push(Entry::Heading("Good to know"));
    out.push(Entry::Note(
        "platoon uses the mouse itself: to select text with your terminal's own selection, hold Shift while you drag.".into(),
    ));
    // Where sessions are listed: the two ways to make tasks from a conversation (`/to-tickets` is Claude Code's).
    if !app.projects_list_showing() && !app.board_showing() && !app.squads_showing() {
        let skill = if app.providers().claude.is_some() {
            "In a Claude Code session, type /to-tickets to turn the work agreed there into tasks for a squad (platoon setup adds it). "
        } else {
            ""
        };
        out.push(Entry::Note(format!(
            "{skill}t on a session's row drafts tasks from its conversation; either way they go on the Tasks tab, not started."
        )));
    }
    if app.open_project().is_some() {
        out.push(Entry::Note(
            "Notes go to the project's sessions up to 8,000 characters. Updates keep them under that; past it, only the first 8,000 are sent, and the notes pane says so.".into(),
        ));
    }
    // Where tasks are: how a task runs, and what the Verdict line means.
    if app.board_showing() {
        out.push(Entry::Note(
            "A task runs as one session, in Claude Code or Codex as its squad's model decides, that follows its squad's steps in a copy of its folder (a git worktree on the branch task/N). Pushing or publishing waits for your yes: Claude Code asks at its own permission prompt; in Codex platoon holds the command back until you answer in the session. Merging is yours: a Done task says not merged while its branch has commits the default branch hasn't.".into(),
        ));
        out.push(Entry::Note(
            "Active and Archived: a Done or Cancelled task moves to Archived a day after it finished, but a Done task whose work isn't merged stays on Active until it is. a moves a finished task to the other list sooner, and it stays where you put it. A task's session is opened from its task; it isn't listed on the Sessions tab.".into(),
        ));
        out.push(Entry::Note(
            "Tests: platoon runs none itself. The session is told to find how its repo runs its tests, run them and name the command and the result when it finishes.".into(),
        ));
        out.push(Entry::Note(
            "Verdict: when a task's session stops, the squad's verdict model (Claude's or Codex's), starting fresh, reads the task, the session's last message and the change, without changing anything, and says whether it is done (a change to code with no test run named is not). Not done goes back to the session once, then to you; only done makes the task Done.".into(),
        ));
    }
    if app.squads_showing() {
        out.push(Entry::Note(
            "A squad is a Markdown file: front matter (name, description, model, verdict_model, plan_first), then the steps its task's session follows. Yours are in the squads folder of platoon's config; one named like a built-in (feature.md) replaces it. A task keeps the copy it started with.".into(),
        ));
        out.push(Entry::Note(
            "Open a squad (enter) to change its model, its verdict model or whether it plans first: ↑↓ picks the line, enter changes it. A built-in becomes your copy first; the description and the steps are changed with e.".into(),
        ));
    }
    out.push(Entry::Note(
        "Ctrl+Z does nothing on platoon's own screens; in a session it goes to Claude Code or Codex like any other key.".into(),
    ));
    // The marks of the sessions' rows, where sessions are listed: the Sessions tab and a project's screen.
    if !app.projects_list_showing() && !app.board_showing() && !app.squads_showing() {
        out.push(Entry::Note(
            "● idle: the session is running, waiting for you. ○ with no status: nothing runs it now; enter picks it up from its saved history.".into(),
        ));
        out.push(Entry::Note(
            "62% on a session's row: how full its context is, of its model's window. Above 80% it is in the warning colour: time to start fresh or let the session compact it.".into(),
        ));
    }
    if app.open_project().is_some() {
        out.push(Entry::Note(
            "A project's screen lists its sessions, then its tasks. enter on a task opens its session, or its text if it never started; its other keys are on the Tasks tab.".into(),
        ));
    }
    out
}

/// The widest the key column gets; a longer key wraps its description under itself.
const KEY_MAX: usize = 18;

/// The help's lines wrapped to `width`: keys in a column of their own in the key color, descriptions beside them with
/// a hanging indent; each with whether it is a section's heading.
fn lines_marked(app: &App, width: usize) -> Vec<(Line<'static>, bool)> {
    let t = app.effective_theme();
    let all = entries(app);
    let key_w =
        all.iter().filter_map(|e| if let Entry::Key(k, _) = e { Some(k.chars().count()) } else { None }).max().unwrap_or(0).min(KEY_MAX)
            + 2;
    let mut out = Vec::new();
    for e in all {
        match e {
            Entry::Heading(h) => out.push((Line::styled(h, Style::new().fg(t.text).add_modifier(Modifier::BOLD)), true)),
            Entry::Blank => out.push((Line::raw(""), false)),
            Entry::Note(n) => out.extend(wrap(&n, width).into_iter().map(|l| (Line::styled(l, Style::new().fg(t.subtext)), false))),
            Entry::Key(k, d) => {
                let room = width.saturating_sub(key_w).max(10);
                let keyed = k.chars().count() + 2 <= key_w;
                let mut desc = wrap(&d, room).into_iter();
                let first = if keyed { desc.next().unwrap_or_default() } else { String::new() };
                let pad = key_w.saturating_sub(k.chars().count());
                out.push((
                    Line::from(vec![
                        Span::styled(k, Style::new().fg(t.accent)),
                        Span::raw(" ".repeat(pad)),
                        Span::styled(first, Style::new().fg(t.subtext)),
                    ]),
                    false,
                ));
                out.extend(
                    desc.map(|l| (Line::from(vec![Span::raw(" ".repeat(key_w)), Span::styled(l, Style::new().fg(t.subtext))]), false)),
                );
            }
        }
    }
    out
}

/// The popup, over the screen it is about: centered, as tall as its text or the screen, scrolled to `help_top`.
pub(super) fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    // Nothing behind it is clicked while it is open: a click outside closes it (`App::help_mouse`).
    app.hits.borrow_mut().clear();
    let w = area.width.saturating_sub(4).min(76);
    // Borders, a column of air each side.
    let text_w = w.saturating_sub(4) as usize;
    let text = lines_marked(app, text_w);
    // The text, a blank line and the keys of the popup itself, inside the borders.
    let want = text.len() as u16 + 4;
    let r = centered(area, w, want.min(area.height.saturating_sub(2)));
    // The screen it is about stays in view behind it, faded as it is behind every popup.
    popup_ground(f, t, area, r, true);
    let block = modal_block(t, "Keys");
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    app.hits.borrow_mut().push((r, Hit::Inert));
    let view_h = inner.height.saturating_sub(2) as usize;
    let max_top = text.len().saturating_sub(view_h);
    let top = app.help_top.get().min(max_top);
    app.help_top.set(top);
    let total = text.len();
    let mut shown: Vec<(Line, bool)> = text.into_iter().skip(top).take(view_h).collect();
    // A heading is never the last line in view, alone above the fold with nothing of its section: it waits below the
    // fold with the rest of it (and the blank line before it with it), and the "more below" count says so.
    if top + shown.len() < total {
        while shown.last().is_some_and(|(l, heading)| *heading || l.width() == 0) && shown.len() > 1 {
            shown.pop();
        }
    }
    let below = total - top - shown.len();
    let shown: Vec<Line> = shown.into_iter().map(|(l, _)| l).collect();
    f.render_widget(Paragraph::new(shown), Rect::new(inner.x, inner.y, inner.width, view_h as u16));
    let mut keys = vec![Span::styled("esc", Style::new().fg(t.accent)), Span::styled(" close", Style::new().fg(hint_fg(t)))];
    if max_top > 0 {
        keys.extend([
            Span::raw("   "),
            Span::styled("↑↓", Style::new().fg(t.accent)),
            Span::styled(" scroll", Style::new().fg(hint_fg(t))),
        ]);
        let more = if below > 0 { format!("   {below} more lines below") } else { String::new() };
        keys.push(Span::styled(more, Style::new().fg(t.dim)));
    }
    f.render_widget(Paragraph::new(Line::from(keys)), Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1));
}
