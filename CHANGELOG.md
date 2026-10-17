# Changelog

Every release of platoon, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
version numbers follow [Semantic Versioning](https://semver.org/). Each release's notes on GitHub are taken from its entry
here.

## [Unreleased]

### Changed

- **A task is one Claude Code session.** `s` on a task picks a squad (the one suggested for it is picked) and starts one
  ordinary `claude` session in a worktree on `task/<N>` from the repository's default branch, with the squad's model, in
  plan mode when the squad plans first, the squad's steps added to its system prompt and the task's text as its first
  message. It is a session like any other: it shows on the Sessions tab marked `#N`, and `enter` on the task opens it.
  The leader, its team of agents, the commander, the squad line and the hand-over popup are gone.
- **A squad is a Markdown file of steps**: front matter (`name`, `description`, `model`, `verdict_model`, `plan_first`)
  and the steps. Six are built in (Feature, Bug fix, Refactor, Explain, Review, Docs), adapted from pstack's playbooks;
  a file of yours in `~/.config/platoon/squads/<id>.md` adds one, or replaces the built-in of that id.
- **The task's state comes from its session's hooks**: a plan to approve, a question, a permission prompt (Needs you),
  Working again once answered or typed into, and Done when it stops. With a project test command, platoon runs the tests
  when the session stops and hands a failure back to it twice before the task needs you.
- **A task is Done only when a check of its work says so.** When its session stops and the tests pass (or there are
  none), a fresh `claude -p` on the squad's `verdict_model` reads the task, the session's last message and the change,
  with tools that only read, and judges each "Done when" box with the evidence. Not done goes back to the session once
  with what it found (`file:line: problem`, each quote checked in the code), then the task needs you; a check that can't
  run needs you with why. The details show a **Verdict** line, what it found and the boxes ticked or crossed; the whole
  answer is `verdict.json` in the task's folder. It needs no switch, and its tokens show as **Task checks**.
- **Publishing asks you**: Claude Code's own `ask` rules for the session hold `git push` (also `git -C <dir> push` and
  the other forms with git's options before `push`), pull requests, releases, writing `gh api` calls, `glab` and
  `cargo`/`npm publish` for your yes.
- **The Tasks tab** has four sections, Needs you, Working, Not started and Done, and few keys: `n` new, `s` start,
  `enter` open, `e` edit, `c` cancel. Tasks drafted from a conversation (`t`) come with a suggested squad and are made
  Not started.
- The tabs are Sessions, Tasks and Projects (`1` to `3`).

### Removed

- The Briefs tab, briefs, agents and their files; waits between tasks, moving tasks between projects, putting a task
  aside, the per-task autonomy levels and the project's default, the go-ahead (`p`), the answer box (`y`), checks other
  than the tests, the timeline and "what ran", the squads-at-once and leader-model settings, the `alt+n` chord
  (`draft_tasks` in `keys.json`), `platoon ticket` (use `platoon task`), `platoon task where`, `--from-session`,
  `platoon squads --kinds` and `platoon setup --remove`. Task files from earlier versions are not read.

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
