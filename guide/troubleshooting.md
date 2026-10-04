# Troubleshooting

Short answers to things that can go wrong, with a link to where each is explained.

## Starting

**`platoon` won't start and lists setup steps.** The status line or the session guard is missing or points at a platoon
that isn't there any more (you moved or reinstalled it). Run `platoon setup`: it adds what is missing and repairs the rest.
See [Running it again](install-and-setup.md#running-it-again).

**Setup says I have my own status line.** It leaves yours alone. To see plan usage in platoon, make your status line also
run `platoon statusline`. See [Set it up](install-and-setup.md#set-it-up-platoon-setup).

**It says `platoon needs at least 60×14`.** Make the window bigger: everything comes back as it was. Until then only `q` or
`ctrl+c` does anything. See [The list](sessions.md#the-list).

**Claude Code was updated and something looks wrong** (sessions missing, wrong status, no context figure). platoon reads
Claude Code's files, which are not a documented interface; a new version can change them. It was last checked with Claude
Code 2.1.288 (platoon says so once when yours is newer). Update platoon (a newer one may be checked with your Claude Code:
see [Install](install-and-setup.md#install) for the command), and if that doesn't help, see
[It reads Claude Code's own files](what-platoon-changes.md#it-reads-claude-codes-own-files).

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
editor with a window of its own. `w` on the task removes it after asking. See
[Handing a task to a squad](tasks-and-squads.md#handing-a-task-to-a-squad).

## Projects and notes

**A project's notes don't reach its sessions.** The Project notes step of `platoon setup` isn't done: run `platoon setup`
again. See [The notes reach every session](projects.md#the-notes-reach-every-session).

**The notes don't update by themselves.** The notes pane says why in one line. It needs the Notes updates step of
`platoon setup`, the Project notes switch on, a notes file that exists (platoon never starts one: press `e`), and a project
that isn't archived. See [Notes that keep themselves up to date](projects.md#notes-that-keep-themselves-up-to-date).

**The header says `smart features paused`.** Today's token limit is used up, your plan is above the pause point, or a
failure paused things for a while. The header says which. See [What keeps it safe](tokens.md#what-keeps-it-safe).

**The hand-off (`h`) doesn't start.** It needs the master switch on and a conversation with something in it, and it stops
when the day's limit is spent or calls are paused; it says which. See
[Handing work on to a new session](projects.md#handing-work-on-to-a-new-session).

## Tasks and squads

**`/to-tickets` says the session isn't in a project.** Add the session to a project (`m` on the Sessions tab), then type
`/to-tickets` again. See [Making tasks from a conversation](tasks-and-squads.md#making-tasks-from-a-conversation).

**`/to-tickets` asks before running `platoon task add`.** Claude Code clears the skill's permission when you send your
next message, so the tasks written after you approve the list may ask. See
[Making tasks from a conversation](tasks-and-squads.md#making-tasks-from-a-conversation).

**`S` refuses to hand a task to a squad.** The task must be one nobody has yet (Can start, or Needs a folder) and not
waiting for another task, its folders that aren't in git must be free (one task at a time there: the line names the
other task), and the squad must have nothing to fix on the Squads tab. If the commander said the squad doesn't fit, pick
the same squad again for "Hand it anyway?". "Session limit has been reached" means the commander couldn't run (today's
token limit, or a pause after a failure). See [Handing a task to a squad](tasks-and-squads.md#handing-a-task-to-a-squad).

**"A new branch in your checkout" isn't offered.** The checkout has changes not yet committed, another open task already
works there on a branch, or a session of yours is running in it; the line under the folder says which. A copy (the
usual) works regardless. See [The hand-over](tasks-and-squads.md#the-hand-over).

**The task says its branch name `task/<N>` is taken.** Something else already uses that branch name in the repository;
platoon stops rather than reuse it. Rename or remove that branch, then hand the task over again. See
[Handing a task to a squad](tasks-and-squads.md#handing-a-task-to-a-squad).

**The task says there is no branch called what it lands on.** The branch was renamed or deleted since the hand-over:
press `S`, `l` on the folder's line, and type one git knows. See [The hand-over](tasks-and-squads.md#the-hand-over).

**A task says "Waiting for your OK on a command".** The leader is at Claude Code's permission prompt: open the task
and answer it in the squad's session. See
[How the task's state follows the squad](tasks-and-squads.md#how-the-tasks-state-follows-the-squad).

**A task says "Working · Waiting for a free squad".** Every place is taken; it starts by itself when one frees. See
[How many squads work at once](tasks-and-squads.md#how-many-squads-work-at-once).

**A task Needs you and says it wants to publish.** Press `p` on the task to let it, or hand tasks over as "works and
publishes" when you don't want to be asked. See
[Anything other people can see asks you first](tasks-and-squads.md#anything-other-people-can-see-asks-you-first).

**The status line said "Moved Web shop's tickets to tasks.json."** platoon renamed that project's task files once (tasks
were called tickets before 1.0); the old list is kept beside them as `tickets.json.bak`. See
[Where tasks are kept](tasks-and-squads.md#where-tasks-are-kept).
