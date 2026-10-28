# Changelog

Every release of platoon, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
version numbers follow [Semantic Versioning](https://semver.org/). Each release's notes on GitHub are taken from its entry
here.

## [Unreleased]

### Changed

- On a project's screen, `←` and `→` now switch between the sessions and the notes on their own. They needed `ctrl` or
  `shift` before, and `ctrl` with an arrow is the Mac's own key for moving between desktops.
- **Making tasks from a session is a skill again: type `/to-tickets`** in a Claude Code session to turn the work agreed
  there into tasks for a squad; it drafts them, shows you the list and makes them only after you approve. `platoon
  setup` installs it (run setup once after upgrading); a `/to-tickets` skill of your own is left alone, and setup says
  so. It replaces the hook that held back a session's first edit to ask, which Claude Code showed as a hook error, and
  with it the **Offer tasks before starting work** setting. `t` on a session's row works as before, and the new
  `platoon task squads` lists the squads a task can start with.

### Fixed

- Selecting text in a session by dragging now scrolls when you drag past the top or bottom edge and hold there, so a
  selection can be longer than the screen; letting go copies all of it. Before, the drag stopped at the edge.

## [1.0.1] - 2026-10-25

### Fixed

- platoon no longer freezes while its terminal window is behind another window (seen in Ghostty): spinners stood still,
  the list stopped updating, keys did nothing and a reply in an open session didn't show until you scrolled. It now keeps
  going in the background, and draws the whole screen again when you come back to the window.

## [1.0.0] - 2026-10-25

The first public release. platoon is a terminal app for Claude Code and Codex, on Linux and macOS: have either one, or
both.

### Added

- **Every conversation in one list**: Claude Code's and Codex's, from every folder, mixed by when each was last used.
  Each row says what the conversation is called, whether it is working, waiting for you or finished, its model and how
  full its context is; the header shows how much of each plan you have used. Search by name, by the words you typed or
  by describing it, rename, and archive the ones you are done with (by hand, or by themselves after a quiet spell).
- **Sessions keep running** after platoon closes, each under a small background host. Open one again from platoon, or
  from any terminal with `platoon open`; several windows can share one session live. A session nobody has used for a
  while is stopped to free memory and resumes where it was. A guard stops two copies of one Claude Code conversation
  from writing over each other; Codex keeps one window per conversation itself, and platoon never opens a second.
- **`n` starts a new session** in Claude Code or Codex (with both installed it asks which, and remembers your choice).
- **Projects** group conversations from any number of folders. A project's notes reach every one of its sessions, in
  Claude Code and in Codex, and can keep themselves up to date as you work, under 8,000 characters, with history and
  undo. `h` hands a long conversation on to a fresh session, in the same program, with a document of where the work
  stands.
- **Tasks**: write down a piece of work as a task, from platoon or from the command line (`platoon task add`), or press
  `t` on a session and platoon drafts tasks from the conversation, each with the squad that suits it. A session platoon
  starts also offers once, just before it starts working, to turn the work into tasks for a squad (**Offer tasks before
  starting work** in the settings switches this off).
- **Squads**: a squad is a short Markdown file of steps for one kind of work. Seven are built in (Feature, Bug fix,
  Incident, Refactor, Explain, Review, Docs), and you can change them or write your own on the Squads tab. Starting a
  task runs one Claude Code or Codex session (the squad's model decides which) in a git worktree on the branch
  `task/<N>`, following the squad. It can plan first and wait for your yes, and it waits for you before a push, a pull
  request or a release. The session runs your repository's tests and names them in its last message.
- **The check of the work**: when a task's session stops, a fresh call that can only read checks the work against the
  task's "Done when" boxes. Only a check that says done makes the task Done; otherwise the session gets what it found
  once, then the task needs you.
- **Watch a task work**: the session marks each of its squad's steps as it goes (`platoon task step`), and the Tasks tab
  shows the step it is on and its steps checked off. In a Claude Code task's session, platoon's status line shows the
  task, its step, its helpers at work and the check of its work.
- **The Tasks tab**: every task across your projects, on two lists. Active is grouped by what each task needs (Needs
  you, Working, Not started, Done); Archived holds the other finished tasks. A Done task whose branch isn't merged says
  so and stays on Active until it is. The header says how many tasks need you.
- **Your own editor**: `v` opens a session's or a task's folders in VS Code, Zed, a JetBrains IDE, Helix, Neovim and
  others found on your computer.
- **Smart features** that use your plan (Claude's, or your ChatGPT plan through Codex): smart names and summaries,
  finding a session by description, notes that update themselves and project descriptions. They are off until you say
  yes, with a daily token limit and a pause when your plan is nearly used. `platoon tokens` says what they used.
- **Claude Code or Codex switched off**: with both installed, the settings can have platoon use only one of them.
- **52 themes**, dark and light, your own theme files, and a fallback for terminals with 256 colors.
- **`platoon setup`**: a one-time checklist that adds a status line and hooks to Claude Code's settings and platoon's
  hooks to Codex's `hooks.json` (for each one you have), backs the files up first, and is safe to run again.
  **`platoon uninstall`** (or the uninstall script) takes them out again.
- **Keys you can change**: the back key (`ctrl+space`) and quit (`q`), in `keys.json`.
- **Version notices**: platoon says once when your Claude Code or Codex is newer than the version it was checked with,
  and, once a day at most, whether a newer platoon is out (a setting switches this check off).

[Unreleased]: https://github.com/ctrader2/platoon/compare/v1.0.1...HEAD
[1.0.1]: https://github.com/ctrader2/platoon/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/ctrader2/platoon/releases/tag/v1.0.0
