# Changelog

Every release of platoon, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
version numbers follow [Semantic Versioning](https://semver.org/). Each release's notes on GitHub are taken from its entry
here.

## [Unreleased]

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
- **Tasks**: write down a piece of work, from platoon, from the command line (`platoon task`; `platoon ticket` still
  works), or by letting Claude turn a conversation into tasks with the `/to-tickets` skill. Tasks can wait for each other,
  and can live outside any project.
- **Squads**: hand a task to a squad, a lead agent and its team of agents, working in a copy of your folder (a git
  worktree made from the branch the work lands on) or, when you choose it, on a new branch of your own checkout. Four squads
  and twelve agents come built in, and you can change them or make your own. At each hand-over you say how much the squad
  does on its own: it asks you before a push, a pull request or a release, or works and publishes. Only a few work at once.
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
