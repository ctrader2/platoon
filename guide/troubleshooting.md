# Troubleshooting

Short answers to things that can go wrong, with a link to where each is explained. When none of them fits, see
[Reporting a problem](#reporting-a-problem) at the end.

## Starting

**`platoon` won't start and lists setup steps.** The status line or the session guard is missing or points at a platoon
that isn't there any more (you moved or reinstalled it). Run `platoon setup`: it adds what is missing and repairs the rest.
See [Running it again](install-and-setup.md#running-it-again).

**Setup says I have my own status line.** It leaves yours alone. To see plan usage in platoon, make your status line also
run `platoon statusline`. See [Set it up](install-and-setup.md#set-it-up-platoon-setup).

**It says `platoon needs at least 60×14`.** Make the window bigger: everything comes back as it was. Until then only `q` or
`ctrl+c` does anything. See [The list](sessions.md#the-list).

**Claude Code or Codex was updated and something looks wrong** (sessions missing, wrong status, no context figure).
platoon reads their files, which are not a documented interface; a new version can change them. It was last checked
with Claude Code 2.1.312 and Codex 0.165.0 (platoon says so once when yours is newer). Update platoon (a newer one may be
checked with your version: see [Install](install-and-setup.md#install) for the command), and if that doesn't help, see
[It reads Claude Code's and Codex's own files](what-platoon-changes.md#it-reads-claude-codes-and-codexs-own-files).

## The list and search

**Two rows have the same title.** A background session is often a copy of another conversation; platoon doesn't try to
tell them apart. See [Background sessions](sessions.md#background-sessions).

**Full-text search finds nothing right after the first start.** It reads your conversations once, in the background, and
says "Still reading your conversations…" meanwhile. See [Search](sessions.md#search).

**The list or search seems out of date or broken.** Deleting `~/.cache/platoon/index.json` or `prompts.jsonl` is harmless:
platoon rebuilds them. See [Big histories](sessions.md#big-histories).

**`Couldn't stop "name": ... Try again in a moment.`** The session didn't exit within about 5 seconds, so nothing was
archived. Try again. See [Archive](sessions.md#archive).

**A conversation running in a plain `claude` can't be opened in platoon.** It isn't platoon's to share: opening it asks
before ending that copy and reopening the conversation from its saved history. See
[See a session live from a regular terminal](sessions.md#see-a-session-live-from-a-regular-terminal).

**A Codex conversation says it is open in another Codex window.** Codex lets one window have a conversation open, and
platoon never opens it a second time or stops that window. Close it there (or quit that Codex), then `enter` opens it in
platoon. Codex's shared background server can hold conversations too: quitting Codex lets them go. See
[Codex conversations](sessions.md#codex-conversations).

**Codex says platoon's hooks need review.** `platoon setup` normally has Codex trust its hooks; when it couldn't,
it said why. Approve them in Codex, or run `platoon setup` again once Codex works. A moved or reinstalled platoon changes
the hooks' command, so Codex asks about them again: `platoon setup` repairs them and has Codex trust them again. See
[Codex](install-and-setup.md#codex).

**A Codex conversation opened in platoon asks whether to trust its folder.** That is Codex's own question, asked once
per folder: answer it there, in the session. See [Codex conversations](sessions.md#codex-conversations). A task's
worktree is trusted in Codex by platoon when it makes it; if Codex couldn't be asked then, the task said so, and Codex
asks you this once.

**A task on Codex stays Working whatever its session does.** Its hooks aren't in Codex yet: run `platoon setup`
once more (it adds the task hooks and has Codex trust them). See [Tasks on Codex](tasks-and-squads.md#tasks-on-codex).

**My Codex conversations aren't listed.** platoon reads them from `~/.codex` (or `$CODEX_HOME` when it is set): check
that it is the folder your Codex uses. platoon reads only the saved sessions in its `sessions` folder.
Deleting `~/.cache/platoon/codex-index.json` is harmless. See [Codex conversations](sessions.md#codex-conversations).

## Keys and the mouse

**The back key doesn't work, or I want another one.** Set it in `keys.json`. A key that breaks the rules is left out with a
notice saying which key is used instead. See [Keys: back and quit](settings-and-keys.md#keys-back-and-quit).

**`shift`+arrows don't move between a project's panes.** Some terminals keep them for themselves: use `ctrl` with the
arrows. See [A project's screen](projects.md#a-projects-screen).

**Dragging in a session doesn't select text the way my terminal does.** platoon selects and copies itself; hold `shift` and
drag for the terminal's own selection. See [Copying text from a session](sessions.md#copying-text-from-a-session).

**What I copied is gone after quitting platoon (X11).** On X11 the program that copied hands the text over, so it goes when
platoon quits, unless a clipboard manager kept a copy. Terminals with OSC 52 (Ghostty, kitty, WezTerm, iTerm2) keep their
own copy; Xfce Terminal doesn't support it. `PLATOON_CLIPBOARD` chooses where copies go. See
[Copying text from a session](sessions.md#copying-text-from-a-session).

## How it looks

**Colors look wrong or flat.** Your terminal may not support 24-bit color: set `PLATOON_COLORS=256` (or `truecolor` if it
does and platoon guessed wrong). See [Terminals without 24-bit color](themes.md#terminals-without-24-bit-color).

**With Terminal colors, the highlighted row is hard to read.** Terminal colors assumes a dark terminal theme, and a black
that differs from the background. Pick one of the light themes instead. See [Terminal colors](themes.md#terminal-colors).

**The session inside platoon looks wrong on a light theme.** Claude Code picks some colors of its own: choose a light theme
in Claude Code too (`/theme`). See [Light themes](themes.md#light-themes).

**My own theme doesn't show up.** The picker and `platoon themes` say how many theme files were skipped and why; a light
theme has to read well as it is. See [Your own themes](themes.md#your-own-themes).

**Count badges or the `✦` mark show as empty boxes.** Set `PLATOON_BADGES=square`, and `PLATOON_GLYPHS=plain` for plain
marks. See [Environment variables](settings-and-keys.md#environment-variables).

## Editors

**`v` says "No editor found".** platoon only offers editors with a file tree that it finds installed. Install one
(VS Code, Zed, Helix, a JetBrains IDE, ...). See [Opening the editor](sessions.md#opening-the-editor-v).

**A JetBrains IDE opened the folder in a window it already had.** That is the IDE's own setting ("Open project in"); platoon
can't ask for a new window. See [Opening the editor](sessions.md#opening-the-editor-v).

**A task's copy of the folder (its worktree) wasn't removed.** It has changes not yet committed, or it was opened in an
editor with a window of its own. Commit or throw away the changes, or remove it with `git worktree remove <folder>`;
its branch stays either way. See [After the work](tasks-and-squads.md#after-the-work).

## Projects and notes

**A project's notes don't reach its sessions.** The Project notes step of `platoon setup` isn't done: run `platoon setup`
again. See [The notes reach every session](projects.md#the-notes-reach-every-session).

**The notes don't update by themselves.** The notes pane says why in one line. It needs the Notes updates step of
`platoon setup`, the Project notes switch on, and a project that isn't archived. A project with no notes file gets one
the next time one of its sessions finishes answering; a notes file you emptied stays empty until you press `e`. See [Notes that keep themselves up to date](projects.md#notes-that-keep-themselves-up-to-date).

**The header says `smart features paused`.** Today's token limit is used up, your plan is above the pause point, or a
failure paused things for a while. The header says which. See [What keeps it safe](tokens.md#what-keeps-it-safe).

**The hand-off (`h`) doesn't start.** It needs the master switch on and a conversation with something in it, and it stops
when the day's limit is spent or calls are paused; it says which. See
[Handing work on to a new session](projects.md#handing-work-on-to-a-new-session).

## Tasks and squads

**`t` drafts no tasks.** It says why in one line: the smart features are off, today's limit is spent or calls are
paused (as for the hand-off); the session has no folder platoon can find; its project is archived (restore it first);
another conversation's tasks are being drafted (one at a time); or Claude found no work left for later in it. See
[Making tasks from a conversation](tasks-and-squads.md#making-tasks-from-a-conversation).

**`s` doesn't start a task.** It says why in one line: the squad picked has something wrong in its file (the list
shows **needs fixing**, and what), the task was changed meanwhile, or its worktree couldn't be made (the branch name
`task/<N>` is already taken in the repository, or the folder `<repo>-worktrees/task-<N>` is already there: rename or move
it, then try again). See [Starting a task](tasks-and-squads.md#starting-a-task).

**A task says "asks permission".** Its session is at Claude Code's permission prompt: `enter` opens the session, where
the question is. See [How the task follows its session](tasks-and-squads.md#how-the-task-follows-its-session).

**A task's check says "Tests run and reported: not met".** platoon runs no tests itself: the session is told to find
how its repo runs its tests and to name the command and the result in its last message. The check found no such
line. Tell the session how to run them (`enter` opens it), or put it in the repo's `CLAUDE.md` so every session finds
it. See [The check of the work](tasks-and-squads.md#the-check-of-the-work).

## Reporting a problem

**`platoon report`** gathers what someone fixing a bug needs into one plain-text file, says where it put it, and sends
nothing anywhere: you read it, then attach it to a [bug report](https://github.com/ctrader2/platoon/issues/new/choose)
or an email.

```
platoon report                 # writes platoon-report-<date>.txt in the current folder
platoon report --hide-names    # the same, with every project and folder name replaced by project-1, folder-2, ...
platoon report --stdout        # prints it instead (--out <file> writes it somewhere else)
```

**What is in it**: platoon's version and how it was installed, Claude Code's and Codex's versions, your system and
terminal (its name and size); where each step of `platoon setup` stands; platoon's own settings; how many conversations,
projects and tasks there are (counts, and each task's number, state, squad and worktree state); the sessions platoon
runs, with their folders, how long they have run, whether they are idle, why one is kept, and the programs running
under them by name; the plan usage figures each session reported and what the meter chose; what the smart features
spent each day; and the last lines of the diagnostics log. Folders are shown from `~`, so your user name isn't in it.

**What is never in it**: anything said in a conversation (prompts, replies, notes, task and squad text, summaries, the
names you gave sessions); a program's arguments (they can hold tokens), environment variables' values, your `.env`
files; anything from Claude Code's `~/.claude.json` or from Codex's `config.toml` and `auth.json`; and nothing from
Claude Code's `settings.json` but whether platoon's own entries are in place. Project and folder names are in it unless
you add `--hide-names`.

**The diagnostics log** (`diagnostics.log` in platoon's cache folder) is what lets a report say why platoon did
something, not just how things stand. platoon always writes down its errors there: a hook given something it couldn't
read, a session that couldn't start, a crash (where it happened, not what it was working on), a failed call on your
plan. Switch on **Keep a diagnostics log** (Settings, General) and it also writes down what it decides and why: a
session stopped or kept and the reason, what the usage meter chose and from which reports, each call on your plan
(which feature, which model, how long, how many tokens), each task moving from one state to another, each hook that
ran and how long it took. Each line is a time, the part of platoon that wrote it, and a few words and numbers: session
ids, program names, durations, counts. It never holds conversation content, commands or paths. It stays small: at 1 MB
it starts over, keeping the one before as `diagnostics.log.1`. To report a problem that comes and goes, switch it on,
wait for the problem to happen again, then run `platoon report`.
