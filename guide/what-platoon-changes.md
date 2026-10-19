# What platoon changes on your computer

platoon runs against your real Claude Code setup. This page says plainly what it adds, what it writes, what it starts,
and how to take it out again.

## In Claude Code's settings

[`platoon setup`](install-and-setup.md#set-it-up-platoon-setup) adds these to Claude Code's `settings.json`
(`~/.claude/settings.json`, or the one in `CLAUDE_CONFIG_DIR`). Each one runs the platoon that ran setup, by its full path.

| What | Where in `settings.json` | What it does |
| --- | --- | --- |
| Status line | `statusLine`: `<platoon> statusline` | Claude Code hands it the plan usage and the session's context percentage; platoon keeps those figures in its cache and prints the status line |
| Session guard | a `UserPromptSubmit` hook: `<platoon> hook guard` | refuses a message in a second copy of a conversation that is already running, and passes a project's changed notes to a running session |
| Project notes | a `SessionStart` hook: `<platoon> hook session-start` | hands a project's notes to a session when it starts, resumes, is cleared or compacted (at most 8,000 characters); a session platoon starts or resumes (it carries `PLATOON_OFFER_TASKS=1`) is also told to offer making tasks with `platoon task add` |
| Notes updates | a `Stop` hook: `<platoon> hook stop` | notes that a project's session finished answering, so its notes can be brought up to date later |

Before it writes the file, setup copies it to `settings.json.platoon-bak` beside it. Everything else in the file is left
as it was, including your own hooks. A status line of your own is never replaced: setup leaves it and says so.

The hooks are quick and quiet. Each one finishes in a moment, and the guard is the only one that ever stops anything (a
message typed into a second copy of a conversation).

## In Codex's hooks

When Codex is on this computer, [`platoon setup`](install-and-setup.md#codex) adds three hooks to Codex's `hooks.json`
(`~/.codex/hooks.json`, or the one in `CODEX_HOME`), each running the platoon that ran setup by its full path, and the
same for every session:

| What | Where in `hooks.json` | What it does |
| --- | --- | --- |
| Project notes | a `SessionStart` hook: `<platoon> hook session-start --agent codex` | hands a project's notes to its Codex session when it starts (at most 8,000 characters) |
| Changed notes | a `UserPromptSubmit` hook: `<platoon> hook guard --agent codex` | passes a project's changed notes to a running Codex session; it never refuses a message (Codex keeps one window per conversation itself) |
| Notes updates | a `Stop` hook: `<platoon> hook stop --agent codex` | notes that a project's Codex session finished answering, so its notes can be brought up to date later |

Before it writes the file, setup copies it to `hooks.json.platoon-bak` beside it; everything else in the file stays as it
was. Codex runs a hook only once you have trusted it: it asks you the next time it starts. platoon never writes Codex's
`config.toml`, where Codex keeps that record (and the rest of its settings).

## No Claude Code skill

platoon adds no skill to Claude Code. Earlier platoons installed one, `/to-tickets` (`~/.claude/skills/to-tickets/`);
platoon no longer touches it: delete that folder if you have it and don't want it.

## Your Claude plan: nothing until you say yes

The features that ask Claude through your plan (`claude -p`) are **off until you say yes**, in `platoon setup` or in the
settings: smart names and summaries, find a session by description, project notes, project descriptions, and, only when you
ask for them, the hand-off and tasks from a conversation. Each call runs with no tools, no
settings and no saved session, under a daily token limit. What each one sends is on
[Features that use tokens](tokens.md). The one call that is not a smart feature is the check of a task's work, below.

## Tasks

A task's session is a Claude Code session that platoon starts for the task, in a platoon host. It is started with:

- `--permission-mode acceptEdits` (or `plan` first, for a squad that plans first): **Claude edits files without asking
  you first**, in the task's copy of its folder (a git worktree), or, for a folder that isn't in git, **in the folder as
  it is**. The task's own folder in platoon's config is added (`--add-dir`), so it can read its squad and the others.
- The squad's model (`--model`), and the squad with platoon's rules for the task added to its system prompt
  (`--append-system-prompt-file`).
- Hooks for that session only (`--settings`), never written to your own settings: `platoon hook task-plan`,
  `task-question`, `task-notify`, `task-tool`, `task-prompt` and `task-stop`. They set the task's state from what the
  session does. Your own Claude Code settings and hooks still apply to it too.
- `ask` rules in the same `--settings`: Claude Code asks you before `git push` (also written `git -C <dir> push`,
  `git -c ... push`, `git --git-dir ... push` or `git --work-tree ... push`, and inside `cd x && ...`), `gh pr create`, `gh pr merge`,
  `gh release`, `gh api` calls with `-X`, `--method`, `-f`, `-F`, `--field`, `--raw-field` or `--input`, `glab`,
  `cargo publish` and `npm publish`, whatever your own settings allow. They are Claude Code's own permission rules, not a
  sandbox: something they don't name (another tool, a script that pushes, `sh -c 'git push'`, `/usr/bin/git push`, or
  an MCP server that publishes) is not stopped by them.

**When a task's session stops, Claude checks the work.** From the session's `Stop` hook platoon runs
`claude -p` on the squad's `verdict_model`, in the task's worktree, with only the tools that read files (`Read`, `Grep`,
`Glob`: no shell, no edits), no settings (`--setting-sources ""`), no saved session, for 10 minutes at most. It is sent
the task's text, the session's last message and the change on the task's branch (`git diff`), and its answer is kept as
`verdict.json` in the task's own folder. It runs for every task whatever the smart features' switches say (it is part of
the task, like its session), and its tokens are counted with the others in `platoon tokens`, as **Task checks**.

In your git repositories, a task makes a worktree beside the repository, `<repo>-worktrees/task-<N>`, on a new branch
`task/<N>` made from the repository's default branch. platoon removes the worktree folder (with `git worktree remove`)
once the task is Done or Cancelled and its session has stopped, unless it has changes not yet committed or was opened in
an editor with a window of its own. It never deletes the branch. See [Starting a task](tasks-and-squads.md#starting-a-task).

## Folders platoon writes

**Config** (`~/.config/platoon`, or `$XDG_CONFIG_HOME/platoon`, or `$PLATOON_CONFIG_DIR`): things you made or chose, kept
because they would be lost or cost tokens to make again.

- `settings.json`, `keys.json` (yours; platoon never writes it), `setup.json` (what an earlier setup remembered)
- `names.json` (names you gave), `archive.json` (archived conversations), `summaries.json` (smart names and summaries)
- `projects.json`, and `projects/<id>/`: the notes (`project.md`), their `history/`, `project.md.bak` (the notes as they were the first time an update condensed them), `notes-state.json`, and the tasks
  (`tasks.json`, `tasks/` with each task's text and its own folder: the squad it was started with, its plan, the last
  check of its work); `tasks.json` and `tasks/` at the top for tasks in no project
- `squads/` (your own squads, and built-in ones you replaced, `<id>.md`), `prompts/` (system prompts you changed),
  `themes/` (your own themes)

**Cache** (`~/.cache/platoon`, or `$XDG_CACHE_HOME/platoon`, or `$PLATOON_CACHE_DIR`): things platoon can make again, or
small marks its parts leave for each other.

- `index.json` (and `codex-index.json`, for Codex's conversations) and `prompts.jsonl`: the index of your conversations
  and the messages you typed, for the list and search. Deleting them is harmless: they are rebuilt.
- `hosts/`: one small file and one socket per session platoon runs, in a folder only you can enter
- `status/`: the figures the status line reports, per session
- `ai-usage.json`: tokens used per day (numbers only), and `ai-work/`, the empty folder the `claude -p` calls run in
- `notes-sent/`, `notes-due/`, `notes-updating/`, `notes-editing/`, `notes-helper/`: marks for the project notes
- `project-launches/`, `editor-open/`, `stale/`: marks for sessions started in a project, folders open in an editor, and
  copies of a conversation the guard has turned away
- `deleted-projects/`: one backup of each project you deleted
- `version-check.json`: when platoon last asked GitHub for its latest version, what it said, and which new platoon and
  Claude Code versions it has already told you about (see [below](#one-network-request-no-telemetry))

**Beside your repositories**: the `<repo>-worktrees/` folders for tasks, described [above](#tasks).

**In Codex's folder**: only what setup adds to `hooks.json`, and the backup beside it.

**In Claude Code's folder**: only what setup adds to `settings.json`, and the backup beside it.
platoon never writes Claude Code's conversations or its other files. Claude Code itself makes one empty folder under
`~/.claude/projects` for platoon's `claude -p` calls, and the sessions platoon starts are ordinary Claude Code sessions,
saved by Claude Code like any other.

## Processes

- Each session started in platoon runs under a small background host, which keeps running when platoon closes. A host
  stops its own session after it has sat unused for the time in the settings (2 hours unless you change it).
- On its own, platoon only stops sessions it runs. Ending any other copy of a conversation (archiving one that runs in a
  terminal platoon didn't start, or opening one platoon can't share) asks you first. A Codex conversation open in a Codex
  window platoon didn't start (or in Codex's background server) is never stopped at all: platoon says where it is.
- After stopping a session, platoon asks the MCP servers that session had started to exit too, and never any other
  process.
- The project notes helper runs in the background, one per project, for at most half an hour.

## One network request, no telemetry

platoon has **no telemetry**: it sends nothing about you or your use anywhere. It makes **one network request**, to see
whether a newer platoon is out:

- **What**: an HTTPS `GET` of `https://api.github.com/repos/ctrader2/platoon/releases/latest`, made with your `curl`
  (if there is none, nothing is asked). All it says about itself is platoon's version, in the `User-Agent` header
  (`platoon/1.0.0`), which GitHub requires: no account, no token, nothing about your computer, your sessions or your
  use. Like any request, it shows GitHub your IP address.
- **How often**: once a day at most, when platoon starts, off to the side (the list never waits for it), and it gives
  up after five seconds. The time of the last try is kept in `version-check.json` in the cache folder.
- **What comes of it**: if GitHub names a newer version, platoon says so once, with the command that updates it (see
  [When Claude Code or platoon is newer](install-and-setup.md#when-claude-code-or-platoon-is-newer)). If the request
  fails for any reason, nothing is said.
- **Switching it off**: "Check for a newer platoon" on the General tab of the settings (`"update_check": false` in
  `settings.json`), or `PLATOON_NO_UPDATE_CHECK=1` in the environment, which also stops platoon asking `claude` for its
  version. With either, platoon makes no network request at all.

Otherwise platoon talks only to the programs it starts on your computer (`claude`, `git`, `curl` for the check above,
your editors) and, over sockets in its cache folder, to its own session hosts. `claude` itself talks to Anthropic, as
it always does, and so do the `claude -p` calls of the features you switch on.

## It reads Claude Code's own files

To list your conversations, platoon reads Claude Code's files: the transcripts under `~/.claude/projects`, the registry of
running sessions in `~/.claude/sessions`, and your MCP servers in `~/.claude.json` (for idle stop it matches their
commands to running processes). When Codex is installed it reads Codex's too, in `~/.codex` (or `$CODEX_HOME`): the saved
sessions under `sessions/`, the names in `session_index.jsonl`, which conversations are open (who holds the lock files in
`thread-writer-locks/`, looked up without touching them: on Linux in `/proc/locks`, on macOS with `lsof`), and, for idle
stop, the MCP servers in `config.toml`. It never writes Codex's conversations or `config.toml`. These files are
not a documented interface, so **a Claude Code (or Codex) update can break platoon**. Codex support was checked with Codex
0.163.0. It was last checked with Claude Code 2.1.288.
When your Claude Code is newer than that, platoon says so once (it runs `claude --version` when it starts), and
`platoon setup` says so each time it runs.

## Removing platoon

`curl -LsSf https://raw.githubusercontent.com/ctrader2/platoon/main/scripts/uninstall.sh | sh` removes everything above
but your conversations, after asking about your own data and any running sessions; task worktrees are listed, not
deleted. [Uninstall](install-and-setup.md#uninstall) says exactly what it does, and `platoon uninstall` does the same
but for removing the program.

### Removing platoon by hand

If you'd rather do it yourself, or the uninstaller can't:

1. **The settings.** Open Claude Code's `settings.json` and delete the `statusLine` entry (if its command ends in
   `platoon statusline`) and each hook whose command ends in `platoon hook guard`, `platoon hook session-start` or
   `platoon hook stop`. In Codex's `hooks.json`, delete each hook whose command ends in `platoon hook guard --agent
   codex`, `platoon hook session-start --agent codex` or `platoon hook stop --agent codex`. `settings.json.platoon-bak` is the file as it was before the last run of setup that changed it;
   copy it back only if nothing else has changed the file since.
   platoon won't start again without the status line and the guard: running `platoon setup` puts them back.
2. **The skill**, if an earlier platoon installed it: delete `~/.claude/skills/to-tickets/`.
3. **platoon's own folders.** Delete `~/.config/platoon` and `~/.cache/platoon`. The config folder holds your projects'
   notes, tasks, squads and settings: keep a copy if you want them.
4. **Task worktrees and branches.** Any `<repo>-worktrees/task-<N>` still there can go with
   `git worktree remove <path>`, and the `task/<N>` branches with `git branch -D task/<N>`, once you no longer need the
   work on them.
5. **The program**, the way you installed it: `brew uninstall platoon`; `cargo uninstall platoon-tui`; for the install
   script, delete `~/.local/bin/platoon` (or where it put it), `~/.config/platoon-tui/`, and the line it added to your
   shell's startup files (`. "$HOME/.config/platoon-tui/env.sh"`); with Nix, remove it from your profile or
   configuration.

Sessions platoon was running keep running until they stop on their own or you end them; your conversations themselves
are Claude Code's and stay where they are.
