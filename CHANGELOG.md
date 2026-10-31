# Changelog

Every release of platoon, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
version numbers follow [Semantic Versioning](https://semver.org/). Each release's notes on GitHub are taken from its entry
here.

## [Unreleased]

## [1.5.0] - 2026-10-31

### Changed

- **`/to-tasks` is now `/hand-to-squad`**: it hands the work agreed in a conversation to a platoon squad, as one or more
  tasks, the same way as before. Run `platoon setup`: it writes `/hand-to-squad` and deletes platoon's copy under the
  old name, `/to-tasks` or `/to-tickets` from before 1.4 (one you changed is kept, and setup says so). Until then the
  old command keeps working, platoon names it in the status line's reminder (which now reads "/hand-to-squad sends
  this work to a squad") and on the empty Tasks tab, and a line above the key hints says to run setup. Uninstall
  removes platoon's copy under any of the three names.

### Fixed

- Selecting text in Claude Code's full-screen mode (`/tui fullscreen`): dragging past the top or bottom of the session
  did nothing, and what Claude Code copied never reached the clipboard. Now platoon passes the press, the drag and the
  letting go to a program that takes the mouse, so its own selection works, scrolling past an edge included, and puts
  what it copies on the clipboard the way platoon's own copies go, with the same note in the status bar. Such a program
  can't read your clipboard through platoon.

## [1.4.0] - 2026-10-29

### Changed

- **`/to-tickets` is now `/to-tasks`**, to match what platoon calls the work everywhere else. Run `platoon setup`: it
  writes `/to-tasks` and deletes platoon's `/to-tickets` copy (one you changed is kept, and setup says so). Until then
  `/to-tickets` keeps working, platoon names it in the status line's reminder and on the empty Tasks tab, and a line
  above the key hints says to run setup. Uninstall removes platoon's copy under either name.
- US spelling throughout: what platoon shows, its guide, the built-in squads and its AI instructions now say "color",
  "favorite", "behavior". Your own squads and system prompts are kept as they are. If you changed the system prompt
  for tasks from a conversation, the settings show it as "User Modified · default updated": that default's only change
  is the spelling of one word.

## [1.3.0] - 2026-10-29

### Added

- The JetBrains IDEs (IntelliJ IDEA, PyCharm, WebStorm, GoLand, RustRover, CLion, PhpStorm, Rider, Android Studio and
  the Community editions) are in the list of editors `e` offers for a project's notes, a task's text, a squad and the
  system prompts, opened with `--wait` so platoon knows when you close the file.
- Claude Code's status line now ends with "· /to-tickets turns this into tasks" while a conversation has agreed on work
  that could become tasks: when the project's notes update finds it has (one more question in the same request, no
  call of its own), or when you approved a plan in the session. It goes once `/to-tickets` has been used there, when a
  later notes update says no, or after 30 minutes; never in a task's own session. Claude's own one-line suggestion of
  `/to-tickets` was easy to miss.
- The Tasks tab, empty, says that typing `/to-tickets` in a Claude Code session turns the work agreed there into tasks.

### Changed

- **A project's notes are now its memory, not a progress report.** The automatic updates keep what a session starting
  next week would need and what will still be true then: what the project is, how it works, its rules, what was found
  and learned, your preferences, where it is heading, and decisions with their reasons. What one session is in the
  middle of (branches, test counts, "being built", "running now") stays out: that is what the hand-off (`h`) is for.
  The Summary says what the project is and the stage it is at in one line, and changes only when the project does; the
  suggested sections no longer include a Status section; an investigation, a migration or a data analysis gets
  sections of its own kind. Most conversations now leave the notes as they are, and lines are no longer reworded when
  their meaning hasn't changed. If you changed "System prompt for project notes" yourself, yours is kept (Settings shows
  "default updated").
- platoon may now **correct or take out what it wrote itself** when it is no longer true, line by line or a whole
  section of its own; before, nothing was ever deleted, so notes said things that had stopped being true. A line of
  yours is still never changed or removed, and a section with one in it, or a heading you made, is never taken out.
- Decisions are choices with their reasons; an event ("pushed", "released") is no longer one, and the same decision
  given twice in one update is added once.

### Fixed

- Cursor installed only as a Mac app (some builds ship its tool as `bin/code`), and IntelliJ IDEA installed from the
  snap (its command is `intellij-idea`), are now found by `v` and `e`.
- Making the window shorter while you look at a session no longer cuts off the session's newest lines. As in a terminal,
  the lines that no longer fit now go up into the session's history, where the wheel scrolls to them, and come back where
  they were when the window is taller again. Before, the lines at the bottom (the prompt among them) were dropped, and a
  window joining a session while a smaller window had it open could be shown it without them.
- `e`'s list of editors now looks where `v` does: an editor whose command isn't on your `PATH` is found where it
  installed itself (JetBrains Toolbox's scripts, `~/.local/bin`, snaps, `/opt/<app>/bin`, and on macOS the apps in
  `/Applications` and `~/Applications`). Before, VS Code on a Mac was missing from the list unless you had run its
  "Install 'code' command in PATH".
- A notes update that found nothing to change used to count as an answer platoon couldn't read, so the same conversation
  was sent again at the next update; it is now taken as "nothing to change" and not sent again.

## [1.2.0] - 2026-10-28

### Added

- After an update, platoon tells you once when `platoon setup` has something new to add that platoon doesn't need to
  start: "New: /to-tickets in Claude Code sessions — run `platoon setup` to add it", in a line above the key hints (or
  "platoon's /to-tickets skill has an update" when a newer platoon changed it). It goes at your next key or click, and
  for good once setup has put the skill in. Before, updating to 1.1.0 without running setup again left `/to-tickets`
  out without a word.
- **`platoon report`** writes what a bug report needs into one plain-text file and says where it is: versions, where
  setup stands, platoon's settings, counts of your sessions and tasks, the sessions platoon runs and what runs under
  them, the plan usage figures, and platoon's diagnostics log. It holds no conversation content, no command lines and
  no secrets, and nothing is sent anywhere: you read it, then attach it to an issue or an email. `--hide-names` replaces
  your project and folder names with placeholders.
- **A diagnostics log**: platoon now writes down its errors (a hook it couldn't read, a session that couldn't start, a
  crash, a failed call on your plan) in a small file in its cache folder, and with the new setting **Keep a diagnostics
  log** (Settings, General; off unless you switch it on) also what it decides and why: a session stopped or kept and the
  reason, what the usage meter chose, each call on your plan and the tokens it used. It holds words and numbers only,
  never your conversations, and stays under about 2 MB.

## [1.1.0] - 2026-10-27

### Changed

- On a project's screen, `←` and `→` now switch between the sessions and the notes on their own. They needed `ctrl` or
  `shift` before, and `ctrl` with an arrow is the Mac's own key for moving between desktops.
- **Making tasks from a session is a skill again: type `/to-tickets`** in a Claude Code session to turn the work agreed
  there into tasks for a squad; it drafts them, shows you the list and makes them only after you approve. `platoon
  setup` installs it (run setup once after upgrading); a `/to-tickets` skill of your own is left alone, and setup says
  so. It replaces the hook that held back a session's first edit to ask, which Claude Code showed as a hook error, and
  with it the **Offer tasks before starting work** setting. `t` on a session's row works as before, and the new
  `platoon task squads` lists the squads a task can start with.
- Idle stop now asks one question about what runs under a session: is a command Claude or Codex started still going
  (a dev server, a watcher)? If so the session is kept; its MCP servers, hooks and other helpers never keep it, however
  they are set up. platoon no longer reads your MCP configuration for this, and the `idle_stop_ignores_mcp` setting is
  gone (left in `settings.json`, it does nothing).
- A running session that has been quiet for the idle-stop time but is kept says why on its row when you pick it, for
  example `Kept running: a command Claude started is still going (npm run dev)`.

### Fixed

- Selecting text in a session by dragging now scrolls when you drag past the top or bottom edge and hold there, so a
  selection can be longer than the screen; letting go copies all of it. Before, the drag stopped at the edge.
- A session you started by typing a skill (`/land the parser fix`) now gets a smart name, and its row shows what you
  typed. One started with a skill and no words, where you then only answered Claude's questions, is named from the
  skill and Claude's reply. Claude Code's own commands (`/model`, `/plugin`, `/add-dir`) still count as nothing.
  platoon reads your conversations again once after the update to find them.
- `platoon setup`'s first naming pass names only the sessions on the Sessions tab's Active list. On a fresh install
  nothing is archived yet, so it named every session you ever had, at once. Sessions unused for the auto-archive age
  now join the archived ones in its question, skipped unless you say otherwise, and platoon itself no longer names them
  in the moments after it starts, before auto-archive has had its turn.
- The plan usage at the top right shows the real figures. The newest report used to win, and a session that sat idle
  (a task's, whose status line keeps reporting) kept sending an old snapshot: the 5-hour window flipped between a
  figure and 0%, and the weekly one could read 7% while your other sessions said 16%. Each window now shows the
  highest figure any session reports for the window running now (usage only grows within a window), and 0% only when no
  session has a 5-hour window running. Codex's figures, read from several conversations, follow the same rule.
- Idle stop never stopped a session whose MCP server is started by a script (a wrapper that sets things up and then
  runs the real server): the server didn't match its configuration, so it counted as work and kept the session forever.
- Sessions that an older platoon started now follow the new idle-stop rules too: platoon checks them once a minute and
  stops the idle ones, so an update reaches sessions that were already running. Each session's background host now
  records the platoon version that started it.

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

[Unreleased]: https://github.com/ctrader2/platoon/compare/v1.5.0...HEAD
[1.5.0]: https://github.com/ctrader2/platoon/compare/v1.4.0...v1.5.0
[1.4.0]: https://github.com/ctrader2/platoon/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/ctrader2/platoon/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/ctrader2/platoon/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/ctrader2/platoon/compare/v1.0.1...v1.1.0
[1.0.1]: https://github.com/ctrader2/platoon/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/ctrader2/platoon/releases/tag/v1.0.0
