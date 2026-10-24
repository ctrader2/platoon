# Projects

A project is a name, a color and a badge, and the conversations you put in it. It isn't tied to a folder: a project can
hold conversations from any number of folders, and each keeps its own. Each project has notes that every one of its
sessions gets in its context, and [tasks](tasks-and-squads.md).

## Putting conversations in a project

A conversation is in at most one project, and any conversation can be put in one, including ones you started outside
platoon: `m` on a conversation puts it in a project, moves it to another, or takes it out ("No project").

Conversations in a project still show on the Sessions tab, with a **PROJECT** column after the name: the project's name in
a pill of its color (round ends where the terminal has the glyphs, like the count badges). The pill is as wide as the
longest project name on the tab needs (a name of up to 40 characters shows in full on a wide enough terminal; longer names
are cut), sized from every conversation on the tab, so it doesn't jump about while you type in the quick filter; on a wide
terminal the column also takes its share of the spare width, as clear air after the pill. Below 90 terminal columns the
pill shows the project's badge letters instead. A tab where no conversation is in a project has no such column.

To start a new session in a project from a regular terminal:

```
platoon open --project "Web shop"   # start a new session in this folder, in that project
```

## The Projects tab

Each row shows the badge and name, [what the project is](#the-line-that-says-what-a-project-is) under it (in full: a long
one wraps onto a further line and that row grows to fit), how many active and archived sessions the project has, when its
notes last changed and when it was last used. Its name column, too, holds only as much as the projects on show need, and
its columns share a wide terminal's spare width the same way as the Sessions tab.

| Key (Projects tab) | Does |
| --- | --- |
| `n` | new project (it asks for a name only) |
| `enter` | open the project's screen |
| `r` | rename it |
| `a` | archive it (on the Archived side: restore it) |
| `c` | its color (one of the theme's, so it follows the theme) and badge (up to 3 letters) |
| `d` | delete it (asks first) |
| `/` | narrow the list of projects as you type |
| `←` `→` | Active or Archived projects |

## A project's screen

A project's screen shows its name and its description across the top (in full, wrapped when it is long), then its
sessions and tasks on the left and its notes on the right.

The left pane has two sections. **Sessions** lists the project's conversations that are not a task's session (active
ones first, then an `Archived (N)` group, dimmed), each with how full its context is and when it was last active.
**Tasks** lists the project's tasks, all but the cancelled ones, open ones first and Done last: `#N`, the title and its
state in the words and colours of the [Tasks tab](tasks-and-squads.md#the-tasks-tab) (Needs you, Working, Not started,
Done). A section with nothing in it says so in one line. A task's session is counted and listed as its task, here and
in the project's numbers.

- `tab` and `shift+tab` switch between the two panes. `shift+←` goes to the sessions and `shift+→` to the notes; `ctrl`
  with an arrow does the same, for terminals that keep `shift`+arrows for themselves. The plain arrows stay inside a
  pane: `↑` `↓` move in the sessions or scroll the notes, and `←` `→` do nothing there.
- `enter` on a session opens it as on the Sessions tab (and `a`, `m`, `r`, `h` and `t` work there too).
- `enter` on a task does what it does on the Tasks tab: opens its session, or shows its text when it never started
  (`esc` comes back here). Its other keys (`s`, `e`, `c`) are on the Tasks tab.
- `n` starts a new session in the project: the folder picker lists the folders the project's other sessions use first.
- `/` searches only this project's sessions, with all three kinds of [search](sessions.md#search); a filter narrows
  the tasks by their titles too.
- The [back key](settings-and-keys.md#keys-back-and-quit) (`ctrl+space`), `esc` or `q` goes back.
- With the notes in focus, `e` [edits them](#editing-the-notes), `c` shows what changed last and `u` undoes it.

## The notes reach every session

The notes are a file, `project.md`, in `~/.config/platoon/projects/<id>/` (the folder is named by the project's id, so a
rename changes nothing there). **Every session in the project has them in its context**, and keeps them as they change:

- When the **Project notes** step of [`platoon setup`](install-and-setup.md#set-it-up-platoon-setup) is done, a session
  gets the notes when it starts, when it is resumed after they changed, and again after Claude compacts the conversation.
- While a session is running, the **Session guard** step hands them over again with your next message whenever they have
  changed since that conversation last got them - so notes written an hour into a session still reach it.

Either way they are only ever sent when they are **new or changed for that conversation** (platoon keeps the version each
session has, as a short hash in `~/.cache/platoon/notes-sent/<session id>`), so the same words are never sent twice and the
two hooks never both send the same version. About 2 KB of notes is roughly 500 tokens, on the changes only. It works for
every session in the project, including ones you run with plain `claude`, and the notes pane says how many of the
project's sessions have the version that is on disk now:

```
These notes go to every session in this project; 2 of 3 have the latest version.
```

**At most 8,000 characters go to a session** (about 2,000 tokens). The updates keep the notes under that (see
[Kept short](#kept-short)). Notes you write past it are sent cut at the last line end before 8,000 characters, with one
line saying they were cut, and the notes pane says so in the warning colour:

```
Notes are 12,400 characters; only the first 8,000 go to sessions.
```

## Archiving and deleting a project

- **Archiving** a project archives its active sessions too (stopping the ones that are running, as archiving a single
  conversation does; it asks first if one is working or runs in a terminal outside platoon). **Restoring** it brings back
  exactly those; sessions that were archived before stay archived. You can restore one session of an archived project on
  its own: it stays in the project and the project stays archived.
- **Deleting** a project takes its sessions out of it (they stay on the Sessions tab), deletes its notes and keeps one
  backup copy in `~/.cache/platoon/deleted-projects/<id>/`. No conversation is ever deleted.
- Automatic archiving after days of no use applies to conversations in projects too.
- Projects live in `~/.config/platoon/projects.json`.

## Editing the notes

With the notes pane in focus (the right arrow on a project's screen):

| Key | Does |
| --- | --- |
| `e` | the list of editors this computer has. The one you used last is **starred and first**, and already picked, so `enter` opens it again |
| `c` | what changed last: the lines that went and the lines that came |
| `u` | undo the last change (and `u` again undoes the undo) |

**Which editors.** platoon uses editors you already have; nothing is installed. It looks at `$VISUAL` and `$EDITOR` first,
then for editors it knows by name: nano, Neovim, Vim, Helix, Micro, Emacs and `vi` in the terminal, and VS Code, Cursor,
Zed, Sublime Text, Kate, gedit, GNOME Text Editor and Mousepad in windows of their own (TextEdit on macOS). Only the ones
it finds are offered, by their plain names. An editor that lives in the terminal **runs inside platoon's window**, like a
session (`ctrl+space` then `y` ends it if it ever wedges); one with a window of its own opens beside platoon, which says so on the
notes and reads them again as soon as you are done or the file changes. There is nothing to set up: whichever one you use
is remembered (as `notes_editor` in `settings.json`, also chosen in Settings, Editors) and offered first, with a star, the
next time. If platoon finds nothing at all it offers `nano` and `vi`.

The same list of editors is used for a task's text and for the
[system prompts](settings-and-keys.md#system-prompts-for-each-smart-feature).

**A new file starts from a template**: the two headings that are always there, **Summary** and **Decisions**. The
sections in between are added as the work needs them (see [The shape of the notes](#the-shape-of-the-notes)). You don't
have to start it: with Project notes on, platoon writes the template itself the first time a session of the project
finishes answering, or a session joins it, and fills it in a few minutes later (see
[Notes that keep themselves up to date](#notes-that-keep-themselves-up-to-date)). Until then the notes pane says so;
`e` starts the file by hand straight away if you would rather write the first lines yourself.

**Nothing is lost.** Before every change platoon didn't make itself, the version that was there is copied into
`~/.config/platoon/projects/<id>/history/`, newest first, at most 20 per project (the oldest goes). `c` shows the last
change with `-` in front of the lines that went and `+` in front of the ones that came, so it reads on a screen with no
colors. `u` puts the previous version back, and that counts as a change of its own, so `u` again brings your edit back.
platoon never writes the file while an editor it started is open, and always reads the file from disk before showing it:
if something else changed it, what is on disk wins.

**Length: 8,000 characters reach the sessions.** platoon keeps the notes it writes under that; past it, the pane says
so and only the first 8,000 characters are sent (see [The notes reach every session](#the-notes-reach-every-session)).

## The shape of the notes

Three parts of `project.md` are fixed, and everything else fits the work:

- **Summary**, at the top: a few lines saying what this project is and where it stands, so a new session (and you) can
  read it in ten seconds.
- **Open questions**, right after the summary: only what is **still open** - questions nobody has answered yet and
  matters not settled yet - grouped under subheadings that fit the project (by area, or "Waiting on the user",
  "Undecided"), plain bullets, nothing tagged onto them. It is not a to-do list. When a question is answered or a matter
  settled, it is **taken out** (not ticked) and the answer appears as a new entry in Decisions. A notes file that already
  had an Open questions section somewhere in the middle has it moved up here, word for word, the next time platoon writes;
  one without it gets it when something first comes up.
- **Decisions**, at the end: dated entries, added one at a time and never rewritten.
- **In between, whatever the project needs.** Requirements and Status while something is being built; Symptoms, Ruled out
  and Leads while a problem is being chased; Questions and Findings while something is being looked into. When the work
  changes, the headings change with it: platoon may add one, rename one, fold two into one or put them in another order.

**How it is written.** The file is Markdown: short bullets with one fact each, indented bullets for detail, `##`
subheadings to group things inside a section, bold now and then, and short paragraphs only in the summary. A section
platoon wrote as paragraphs before may come back as bullets. The notes pane shows it the way a Markdown viewer would:
headings without their `#`, bullets as `•` with wrapped lines lined up under the text, bold, italic, code and quotes
styled, links as their words (the address stays in the file). On the Linux console, or with `PLATOON_GLYPHS=plain`,
bullets are `-` and `*`.

**Nothing is ever deleted by a change of heading.** A section platoon doesn't mention stays exactly where it is, word for
word. A heading can only go away by being carried into the one that replaces it, and everything of yours that was under it
has to come across with it or the change is thrown away (see [What it may write](#what-it-may-write)).

## Notes that keep themselves up to date

With **Project notes** on (see [Features that use tokens](tokens.md)), platoon writes the notes for you from what your
sessions actually did. It needs the **Notes updates** line of
[`platoon setup`](install-and-setup.md#set-it-up-platoon-setup), which adds a `Stop` hook to Claude Code; without it
nothing happens automatically.

**How it runs.** Every time a session in a project finishes answering, the hook makes a note of it - instantly, silently,
no tokens - and starts a small helper in the background. Putting a conversation **into** a project does the same: `m`, a
session started under a project, or one the SessionStart hook places there all mark the project, so its notes catch up a
few minutes later without you having to resume anything (a conversation you never open again would otherwise never be read
at all). The helper waits until the project has been quiet for as long as **Update notes after** says (3 minutes unless you
change it, in the [settings](settings-and-keys.md#projects) on the Projects tab: 1, 3, 5, 15 or 30 minutes; another turn
starts that timer over) and then makes **one** request to Claude, always on Sonnet whatever the model for smart features is (the notes go into
every session, and Haiku left out changes it had been shown). Only one helper runs per project, and it works whether
the platoon window is open or closed. **A project with no notes file yet gets one**: the hook writes the template
first, then marks the project as above, so the first update has something to fill in. Archived projects and archived
sessions are skipped entirely, and so is a notes file that is there but empty: if you emptied it, platoon leaves it to
you (`e` starts it again).

**What it reads.** Only what is new since the last update: for each active session in the project, the messages you typed
since then (at most 8, each cut to 300 characters - the same settings the smart names use) plus Claude's last reply, up to
**Read Claude's last reply up to** characters (1500 unless you change it), because what is still open in a conversation is
usually in Claude's answers. A session that platoon has never read before - a new project, or one you have just added a
session to - is read in full once: its first message and its last 20.

### What it may write

| Part | What an update may do |
| --- | --- |
| Summary | rewrite it, keeping it short; it is added at the top if it isn't there |
| The sections in between | add one, reword **what platoon itself wrote** in one, rename one, fold two into one, put them in another order |
| Open questions | rewrite it; take out what has been answered. **A line of yours there may only go in the same update that adds its answer to Decisions**; it may move to another subheading |
| Decisions | append dated entries; nothing already there is changed, except when the notes are over 8,000 characters: then older entries may be merged into fewer dated lines, and **every line of yours there** (one that doesn't start with `- ` and a date) has to come back word for word. platoon writes the date, so a date the answer put at the front of an entry is taken off rather than written twice |

platoon remembers exactly which sentences it wrote itself, under which heading, so every other line of a section is yours.
If an answer comes back without one of your lines in it, word for word, that section is **left exactly as it was** - and
that holds when a heading is renamed too: your lines have to turn up under the new one. The same goes for an answer that
isn't understood and for notes that changed on disk while the request was out: what is on disk wins, and platoon works the
merge out again or leaves it alone. A line of yours that an answer had to repeat stays yours: platoon only ever counts its
own words as its own.

### Kept short

The notes stay under **8,000 characters**, the most that goes to a session. Each request says how long the notes are;
the instructions ask Claude to condense when they near it: shorter and merged older detail in platoon's own sections,
sections folded together, older Decisions merged, and what was decided and what is still open kept. An answer that
leaves the notes over 8,000 characters (and no shorter than they were) is asked for again **once**, condensed; if the
second answer is still too long, the notes stay as they were. Notes that are over because of what you wrote are let
shrink a step at a time: an answer that makes them shorter goes in even while they are still over.

The first time an update condenses notes that were over the limit, the file as it was is kept beside it as
`project.md.bak`, once: a copy that is already there is never replaced (the usual history copies are kept as well).

### Seeing it

The notes pane says `platoon is updating these notes now.` while a request is out, and `platoon last updated these 5m ago.`
afterwards. Every update goes through the same history as your own edits, so `c` shows what changed and `u` undoes it, and
platoon tells you once when the notes change under you.

**When they are not updating, it says so.** Whenever something is stopping the updates, the pane says which and what to do
about it, in one line that goes as soon as the reason does:

```
These notes are not updating right now: "Project notes" is switched off. Press s for the settings to switch it back on.
These notes are not updating right now: Today's token limit is used up (25,000 of 20,000 tokens). It starts over tomorrow, or you can raise it in settings.
These notes are not updating right now: Your Claude plan is 82% used (weekly limit), above the pause point of 80%. Smart features wait until it drops. Press s for the settings to change the pause point.
```

The three that stop **everything** that uses tokens - the day's limit, the plan pause, a pause after failures - are also
said quietly in the header, on every tab, because those arrive out of nowhere:

```
platoon   1 Sessions 3   2 Projects 1   ✦ smart features paused · your Claude plan is 82% used (weekly limit) · press s for the settings
```

A switch you turned off yourself is not in the header (it would sit there for as long as you left it off); it is said
where that feature lives, as above.

### Where it is kept

The times and the copy of platoon's own sentences, heading by heading, live in
`~/.config/platoon/projects/<id>/notes-state.json` (numbers and platoon's own text only - never a message you typed, never
a reply from Claude). When a heading is renamed, its record moves with it; when a section is folded into another, its
record goes.

## The line that says what a project is

With **Project descriptions** on (see [Features that use tokens](tokens.md)), each project gets one short line saying what
it is, under its name on the Projects tab and across the top of its own screen:

```
  WB  Web shop              1 active                        notes now                         2h
      A sock shop, now with a basket that adds up.
```

It is written from the summaries platoon already made for the project's conversations plus the project's own notes - no
transcript is read for it, and no conversation's id or folder is ever in the request. It is refreshed **straight after a
notes update**, and at most **once an hour** per project, so it costs one small call at a time and only when something has
actually happened. Archived projects and archived sessions are left out entirely, an answer that isn't understood changes
nothing (the line that is there stays, and the next update tries again), and the line is cut to 100 characters with every
control character and escape sequence removed before it is drawn. It is kept beside the project in
`~/.config/platoon/projects.json`.

## Handing work on to a new session

`h` on a conversation that is in a project - on the Sessions tab or in the project's own screen - hands that work on to a
fresh conversation. A box opens straight away and says it is asking Claude; meanwhile platoon reads the conversation and,
if the folder is a git repository, what git says about it (only reading, never changing anything, and giving up after 5
seconds). Then it shows a **hand-off document** with a fixed shape, like this one:

```markdown
# Session hand-off: Basket page

This is context for this session. The user will prompt you for the next course of action.

- Written: 2026-09-25 14:03
- Project: Web shop
- Project notes: ~/.config/platoon/projects/p1/project.md
- Previous session: Basket page, in ~/shop (not running)
- Previous session id: 0b5e7c1a-... (`claude --resume 0b5e7c1a-...` opens it)
- Started: 2026-09-25 11:58, last active 2026-09-25 14:03 (2 h 5 min between)
- Model: Sonnet 5, context 42% full
- Messages from the user: 14 (short replies like "yes" not counted)
- Tokens the previous session used: 45,210 sent to Claude and 12,034 written by Claude
- Git branch: main
- Compared with origin/main, as of the last fetch: 2 commits not pushed yet
- Uncommitted changes: 2 files
- Last commit: a1b2c3d Add the basket
- Commits since the session started: 2, newest first:
  - a1b2c3d Add the basket
  - 9f8e7d6 Start the totals

## Executive summary

The basket page is built and totals are next.

## Work completed

- Added the basket page

## Files affected

Not committed yet:
- src/basket.rs (changed)
Committed since the session started:
- src/totals.rs

## Current state and known gotchas

- Totals ignore VAT for now

## Next steps

1. Add VAT to the totals
2. Take card payments
```

- **The line under the title** tells the new session that the document is background, and that you will say what to do
  next. You can delete it like any other line.
- **The facts** (the list at the top) come from platoon itself: the project and where its notes are, the old session's
  folder, id, whether it is still running, when it started and was last used, its model, how many messages you sent, the
  tokens it used, and the git branch, uncommitted changes, last commit and the commits made during the session. A fact
  platoon can't find out is left out, never guessed.
- **Files affected** comes from git too. When the folder is not a git repository, Claude lists the files instead, and the
  section says so.
- **Claude writes only** the executive summary, the work completed, the current state and known gotchas, and the numbered
  next steps. A section with nothing in it is left out.

Claude is sent the project's name, the folder's name, the conversation's name and summary, your first message and your last
few (each cut short), one line of its last reply and the file names git gave. It is never sent the session id, and the
project's notes are not repeated: the new session is given those by the hooks, as every session in a project is.

**You can edit the document in the box.** The text wraps at the edge of the box and scrolls: the arrows move the cursor (up
and down by a row on screen), Home and End go to the start and end of a line, PageUp and PageDown move a page, the mouse
wheel scrolls, and `enter` starts a new line. The lines above and below the text say how much more there is ("↑ 3 more
lines above", "↓ 12 more lines below"). The document can be up to 20,000 characters.

`ctrl+s` starts a **new** session in the same folder and the same project, with the document as its **first message**,
visible in the conversation like anything else you typed. `esc` calls the whole thing off and changes nothing.

The old conversation is left exactly as it is: not archived, not stopped. If Claude can't be asked (the master switch is
off, the day's limit is spent, a failure has paused things), the conversation has nothing in it yet, or the answer isn't
understood, platoon says so plainly and nothing is started.

**A Codex conversation is handed on the same way, to a new Codex session.** The document is made from the Codex
conversation (what you typed, a line of Codex's last reply, the tokens it used as Codex counted them), its id line says
`codex resume <id>` opens it, and `ctrl+s` starts `codex --no-daemon` with the document as its first message, in the same
folder and project. The new session always runs on the same program as the conversation it came from; if that program
isn't installed on this computer any more, `h` says so and nothing is asked or started.

The hand-off has no switch of its own: see [Features that use tokens](tokens.md#the-features).
