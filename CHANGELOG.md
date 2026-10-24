# Changelog

Every release of platoon, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
version numbers follow [Semantic Versioning](https://semver.org/). Each release's notes on GitHub are taken from its entry
here.

## [Unreleased]

### Changed

- **Project notes have a model of their own, Sonnet by default.** Whatever the model for smart features is, a notes
  update uses **Model for project notes** (Settings, Projects tab), which is Sonnet unless you pick another. On Haiku, an
  update could leave out a change it had just been shown (a feature taken back out stayed in the notes), and shortening
  notes over the limit kept about twice as many lines that were no longer true.
- **platoon's own calls run on Codex for an OpenAI model.** Names and summaries, Describe it, project descriptions and
  notes, the hand-off, tasks from a conversation and a task's check run as `codex exec` when their model is one of
  OpenAI's: ephemeral, without your `config.toml`, in Codex's read-only sandbox (Codex can't switch its tools off), the
  request on standard input. Every token Codex reports counts against the daily limit; its own instructions are about
  20,000 tokens a call. `platoon tokens test` says which program it asks. Conversations of Codex's still aren't
  named, described or handed off.
- **Limits per program.** Claude Code's calls and Codex's are counted apart: each against the daily limit, each with
  its own back-off after a failure ("Codex isn't signed in" doesn't stop Claude's calls), and the pause above plan usage
  follows the plan of the program the call would use. Codex's limits come from its saved sessions, whatever windows the
  plan has (a free plan: one of 30 days). The header shows each installed program's windows
  (`5h ━━ 47%   7d ━ 21%   Codex 30d ━ 12%`), and `platoon tokens` and the settings say each one's use.
- **Every model setting offers the models installed, by name.** The model for smart features, the new model for project
  notes and a squad's verdict model offer Claude Code's models (Opus, Sonnet, Haiku, Fable) when Claude Code is
  installed and Codex's (as Codex lists them in its `models_cache.json`) when Codex is; the model decides the program
  that runs it. A squad's own model stays one of Claude's until a task's session can run on Codex. A model you picked
  is kept as you wrote it: one this computer can't run is shown as such (**GPT-6-Luna (needs Codex, not installed)**)
  and whatever would use it stops and says what is missing, never quietly using another. The Squads tab and
  `platoon tokens` show models by their names.
- **A project's notes start by themselves.** A project with no notes file gets one from the template the first time
  one of its sessions finishes answering, or a session joins it, and the first update fills it in a few minutes later;
  you no longer have to press `e` first. A notes file you emptied is left to you, and nothing starts with Project
  notes switched off.
- **A task that changes nothing leaves its branch out of its last message.** The rule that the last message says the
  work is on `task/<N>` and not merged now applies only when the session committed something, so an Incident diagnosis
  no longer ends with a line about a branch it never used.
- **A squad's pane no longer lists its file.** The "File" line (the path, or "built in") under the settings is gone; the
  list rows still say where a squad comes from.
- **A task is one Claude Code session.** `s` on a task picks a squad (the one suggested for it is picked) and starts one
  ordinary `claude` session in a worktree on `task/<N>` from the repository's default branch, with the squad's model, in
  plan mode when the squad plans first, the squad's steps added to its system prompt and the task's text as its first
  message. It is a session like any other: it shows on the Sessions tab marked `#N`, and `enter` on the task opens it.
  The leader, its team of agents, the commander, the squad line and the hand-over popup are gone.
- **A squad is a Markdown file of steps**: front matter (`name`, `description`, `model`, `verdict_model`, `plan_first`)
  and the steps. Seven are built in (Feature, Bug fix, Incident, Refactor, Explain, Review, Docs), adapted from pstack's
  playbooks (Incident from its bug-fix and runtime-forensics playbooks and its postmortem sources); a file of yours in
  `~/.config/platoon/squads/<id>.md` adds one, or replaces the built-in of that id.
- **The task's state comes from its session's hooks**: a plan to approve, a question, a permission prompt (Needs you),
  Working again once answered or typed into, and Done when it stops and the check of its work passes.
- **The session runs the tests, not platoon.** platoon's rules for every task tell the session to find how its repo runs
  its tests (its `CLAUDE.md`, README or build files), run them before it finishes and name the command and the result
  in its last message; the Feature, Bug fix and Refactor squads say so where they verify; Incident only diagnoses and changes nothing.
- **A task is Done only when a check of its work says so.** When its session stops, a fresh `claude -p` on the squad's `verdict_model` reads the task, the session's last message and the change,
  with tools that only read, and judges each "Done when" box with the evidence. Not done goes back to the session once
  with what it found (`file:line: problem`, each quote checked in the code), then the task needs you; a check that can't
  run needs you with why. For a change to code, a last message that names no test run is not done (the box **Tests run
  and reported** is crossed). The details show a **Verdict** line, what it found and the boxes ticked or crossed; the whole
  answer is `verdict.json` in the task's folder. It needs no switch, and its tokens show as **Task checks**.
- **Done says when the work is not merged.** A Done task whose branch `task/<N>` has commits the repository's default
  branch hasn't (locally or on `origin`) reads **Done · not merged**, and its details say how many and where; the
  session's last message says the work is on its branch and not merged. Merging stays yours.
- **Publishing asks you**: Claude Code's own `ask` rules for the session hold `git push` (also `git -C <dir> push` and
  the other forms with git's options before `push`), pull requests, releases, writing `gh api` calls, `glab` and
  `cargo`/`npm publish` for your yes.
- **The Tasks tab** has four sections, Needs you, Working, Not started and Done, and few keys: `n` new, `s` start,
  `enter` open, `e` edit, `c` cancel. Tasks drafted from a conversation (`t`) come with a suggested squad and are made
  Not started.
- The tabs are Sessions, Projects, Tasks and Squads (`1` to `4`): Projects comes second, next to the sessions it groups.
- **CONTEXT** on a session's row is a plain figure, `62%` of the model's window (the last reply's input; a 1M window
  when the model's name says so or the conversation is past 200,000 tokens), in the warning colour above 80%. It
  replaces the bar and its `~`. A project's screen shows it too.
- **A project's screen lists its sessions and its tasks apart**, in two sections, Sessions and Tasks. A task's row is
  `#N`, its title and its state as the Tasks tab says it (cancelled ones left out, Done last); `enter` on it opens its
  session, or its text when it never started. A task's session is listed (and counted) as its task, not as a session.

### Added

- **Codex conversations in the list.** When OpenAI's Codex CLI is installed, its conversations show beside Claude
  Code's, newest activity first, with their model's name (`GPT-6-Luna`, or `Codex` before the first reply), the name
  Codex gave them, how full their context is and whether they are working. Search, archive, projects, rename and `v`
  work on them; `enter` opens one in platoon (`codex resume <id> --no-daemon`), and `platoon open <id>` takes a Codex id.
  One open in another Codex window is shown running and never opened a second time or stopped: platoon says where it
  is. A Codex session platoon runs stops itself when idle like any other, its MCP servers read from Codex's
  `config.toml`. The smart features, the hand-off and task drafts stay on Claude Code's conversations for now.
- **Project notes reach Codex sessions too.** `platoon setup` adds three hooks to Codex's `hooks.json` when Codex is
  installed (notes at the start, changed notes with a message, notes updates after a turn), and has Codex itself mark
  exactly those three trusted, so Codex doesn't ask you about them (Codex writes the record in its own `config.toml`;
  when it can't be asked, setup says why and Codex asks you once). They never block starting platoon, and
  `platoon uninstall` takes them out again, with Codex's records of trusting them.
- **The Squads tab** (`4`) lists every squad, built in or yours, with where it comes from and **needs fixing** when its
  file has something wrong, and shows the picked one beside the list: what it is for, its models, whether it plans first,
  its steps and what is wrong with it. `e` opens a squad's file in your editor (a built-in's text is copied into
  `~/.config/platoon/squads/` first, and the copy goes again if you leave it unchanged), `n` makes a new one from a
  template, `d` deletes one of yours after asking (kept as `<id>.md.bak`; a built-in it replaced comes back).
- **Sessions offer to make tasks.** A session platoon starts or resumes (not a task's own) asks you once, when the
  conversation settles on concrete work, "Ready to create tasks?", and on a yes makes them itself with `platoon task add`
  in its project and folder, each with "Done when" lines and a suggested squad, Not started. `platoon task add` takes
  `--squad <id>`.
- **Project notes are kept under 8,000 characters.** The notes update condenses them when they near it (an answer that
  leaves them over is asked for again once, then the notes stay as they were); the first time it condenses notes that
  were over, the old file is kept as `project.md.bak`. Notes you write past the limit are sent to sessions cut at a line
  end, with a line saying so, and the project's screen warns about it.
- **A squad's settings without an editor**: open a squad on the Squads tab (`enter`), pick its Model, Verdict model or
  Plans first line with `↑` `↓` (or a click) and `enter` changes it to the next choice (opus, sonnet, haiku, fable; the
  verdict model also "the same as the model"; yes or no). Only that line of the file changes; a built-in is copied to
  your squads folder first, and is built in again when changed back.

### Removed

- The Briefs tab, briefs, agents and their files; waits between tasks, moving tasks between projects, putting a task
  aside, the per-task autonomy levels and the project's default, the go-ahead (`p`), the answer box (`y`), the checks
  platoon ran itself, the project's test command (`T` on its screen, its timeout and the suggestion from `CLAUDE.md`)
  with the tests platoon ran when a session stopped, the Tests line and "tests failing", the timeline and "what ran", the squads-at-once and leader-model settings, the `alt+n` chord
  (`draft_tasks` in `keys.json`), `platoon ticket` (use `platoon task`), `platoon task where`, `--from-session`,
  `platoon squads --kinds` and `platoon setup --remove`. Task files from earlier versions are not read.

### Fixed

- `platoon setup` names a backup ("Saved … (backup: settings.json.platoon-bak)") only when there was a file to back up.
- With no sessions yet, the list says so ("No sessions yet. Press n to start one.") as soon as platoon has looked,
  instead of "Reading your sessions…" and `indexing…` coming back at every once-a-second pass on a busy computer.

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
