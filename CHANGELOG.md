# Changelog

Every release of platoon, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
version numbers follow [Semantic Versioning](https://semver.org/). Each release's notes on GitHub are taken from its entry
here.

## [Unreleased]

### Changed

- **A squad is a brief**: a short Markdown file of yours, `~/.config/platoon/briefs/<id>.md`, with what the squad does, its
  steps in order (each an agent and what it does, and when it may send the work back), its checks and when it asks you.
  The Squads tab shows the brief as it reads, with what is wrong in it at the top, and `e` opens it in your editor; the
  in-app step editor is gone. The four built-in squads are briefs too, and work as they are. Squads you had as JSON files
  are turned into briefs once (each old file kept as `.bak`).
- **The agents your coding tool already has** (Claude Code's `~/.claude/agents`, and a task's repository's
  `.claude/agents`) can do a brief's steps. The Agents list shows yours under **From Claude Code**; platoon only reads
  them, and `c` copies one into platoon's own to change it.
- A task's screen names its squad by its brief ("Coding brief").

## [1.0.0] - unreleased

<!-- "unreleased" becomes the date (YYYY-MM-DD) in the commit that is tagged v1.0.0. -->

The first public release. platoon works with Claude Code (last checked with 2.1.288) on Linux and macOS.

### Added

- **Sessions**: every Claude Code conversation in one list, across every folder: its name, whether it is working,
  waiting for you or finished, its model, how full its context is, and your plan usage in the header. Search by name or by
  the words you typed, rename, and archive the ones you are done with (by hand, or by themselves after a quiet spell).
- **Sessions keep running** after platoon closes, each under a small background host. Open one again from platoon or from
  any terminal with `platoon open`; several windows can share one session live. Idle sessions are stopped to free memory
  and resume where they were.
- **One writer per conversation**: a guard stops a second copy of a running conversation from overwriting it.
- **Projects** group conversations from any number of folders. A project's notes reach every one of its sessions, can keep
  themselves up to date as you work, and a long conversation can be handed on to a fresh session with one key.
- **Tasks**: write down a piece of work, from platoon or from the command line (`platoon task`; `platoon ticket` still
  works), or press `t` on a session (`alt+n` inside it) and Claude drafts tasks from the conversation, shaped to your
  squads: you change them, take some out, make them and hand each to a squad in one popup. Tasks can wait for each other,
  can live outside any project, and move between projects.
- **The Tasks tab**: every task in play across your projects, grouped by what it needs from you (Needs you, Working, Can
  start, Waits for, Finished today), with the picked task's details beside it on a wide terminal: what its squad asks,
  a timeline of what the squad has done, and the commands it ran. Answer a squad's question in place (`y`), put a task
  aside until its squad says something new (`h`), and see in the header how many tasks need you.
- **Squads**: hand a task to a squad, a lead agent and its team of agents, working in a copy of your folder (a git
  worktree made from the branch the work lands on) or, when you choose it, on a new branch of your own checkout. Four squads
  and twelve agents come built in, and you can change them or make your own. At each hand-over you say how much the squad
  does on its own: it plans first and waits for your yes (the plan shows on the task; `y` goes ahead, `n` asks for a
  change), it asks you before a push, a pull request or a release, or it works and publishes; a project's screen sets
  where that starts (`o`). Only a few work at once.
- **Tests platoon runs itself**: a project can name its test command (`T` on its screen; a `CLAUDE.md` with a "Build and
  test" block suggests one). Each time a squad says it is finished, platoon runs the command in the squad's copy of the
  folder, types a failure back into the squad's session for it to fix (twice at most), and otherwise tells you.
- **Your own editor**: `v` opens a session's or a task's folders in VS Code, Zed, a JetBrains IDE, Helix, Neovim and
  others found on your computer.
- **Smart features** that use your Claude plan (smart names and summaries, finding a session by description, notes that
  update themselves), off until you say yes, with a daily token limit and a pause when your plan is nearly used.
- **52 themes**, dark and light, your own theme files, and a fallback for terminals with 256 colors.
- **`platoon setup`**: a one-time checklist that adds a status line and hooks to Claude Code's settings, backs the file up
  first, and is safe to run again.
- **Keys you can change**: the back key (`ctrl+space`) and quit (`q`), in `keys.json`.
- **Version notices**: platoon says once when your Claude Code is newer than the version it was checked with, and, once a
  day at most, whether a newer platoon is out (a setting switches this check off).

[Unreleased]: https://github.com/ctrader2/platoon/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/ctrader2/platoon/releases/tag/v1.0.0
