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
| Project notes | a `SessionStart` hook: `<platoon> hook session-start` | hands a project's notes to a session when it starts, resumes, is cleared or compacted |
| Notes updates | a `Stop` hook: `<platoon> hook stop` | notes that a project's session finished answering, so its notes can be brought up to date later |

Before it writes the file, setup copies it to `settings.json.platoon-bak` beside it. Everything else in the file is left
as it was, including your own hooks. A status line of your own is never replaced: setup leaves it and says so.

The hooks are quick and quiet. Each one finishes in a moment, and the guard is the only one that ever stops anything (a
message typed into a second copy of a conversation).

## No Claude Code skill

platoon adds no skill to Claude Code. Earlier platoons installed one, `/to-tickets` (`~/.claude/skills/to-tickets/`);
a copy you have keeps working and is left alone, and `platoon setup --remove to-tickets` or the uninstall takes it out
(see [The old `/to-tickets` skill](install-and-setup.md#the-old-to-tickets-skill)).

## Your Claude plan: nothing until you say yes

The features that ask Claude through your plan (`claude -p`) are **off until you say yes**, in `platoon setup` or in the
settings: smart names and summaries, find a session by description, project notes, project descriptions, and, only when you
ask for them, the hand-off, tasks from a conversation, the squad commander and drafting a squad or an agent. Each call runs with no tools, no
settings and no saved session, under a daily token limit. What each one sends is on
[Features that use tokens](tokens.md).

## Squads

A squad's leader is a Claude Code session that platoon starts for a task. It is started with:

- `--permission-mode acceptEdits`: **Claude edits files without asking you first.** It starts in the task's first
  folder, with the task's other folders and its own folder added (`--add-dir`). For a folder in a git repository that
  is a copy platoon made for the task (a worktree), or, when you chose that at the hand-over, your own checkout on a new
  branch `task/<N>`. **A folder that isn't in git is worked in as it is**, and a task with no folder works only in its own
  folder. Anything else, like running a command, still goes through Claude Code's permission prompt as your own settings
  decide.
- Its tools: the leader may hand work to its agents, read, search and run commands (`Agent`, `Read`, `Grep`, `Glob`,
  `Bash`); it never edits a file itself. Each agent can always read and search (`Read`, `Grep`, `Glob`); its switches add
  the rest: **can change files** adds `Edit` and `Write`, **can run commands** adds `Bash`, **can use the web** adds
  `WebSearch` and `WebFetch`, and its skills and MCP servers add `Skill` and `mcp__<name>`. The built-in agents' switches
  are on [The built-in squads](tasks-and-squads.md#the-built-in-squads). An agent your coding tool already has (a Claude
  Code agent file a brief names) runs with the tools and model its own file gives it.
- Hooks for that session only (`--settings`), never written to your own settings: `platoon hook squad-stop`,
  `squad-gate`, `squad-notify`, `squad-tool` and `squad-prompt`. They set the task's state from what the leader says and
  does. Your own Claude Code settings and hooks still apply to it too.
- A gate on publishing: `squad-gate` refuses, until you press `p` on the task (or always lets through, for a task you
  handed over as "works and publishes"), the commands it recognises as making
  something others can see: `git push`, `git send-email`, `git subtree push`; `gh` and `glab` commands that create, edit,
  merge, comment on or delete pull/merge requests, issues, releases, gists and repositories, `gh workflow run`, and
  `gh api`/`glab api` calls that change something; `hub push`, `hub pull-request` and `hub release create`;
  `cargo`/`npm`/`pnpm`/`yarn`/`poetry publish`; `twine upload`; `gem`/`docker`/`podman push`. It looks through `sudo`,
  `env`, `bash -c` and the like. It is a list, not a sandbox: something it doesn't know (another tool, or an MCP server
  that publishes) is not stopped by it.

**A command of yours runs when a squad finishes.** When a project has a [test command](projects.md#the-test-command),
platoon runs it (`sh -c <your command>`) each time a squad of that project stops at the end of the work: in the task's
copy of its folder, or on the task's branch in your own checkout while the checkout is still on it, never where another
session of yours is open. It runs in the background from the platoon window that is open, with the squad leader's
environment, no terminal, for 10 minutes at most (then it is stopped with everything it started), and what it writes is
kept as `tests-<try>.log` in the task's own folder. It is your command: platoon adds nothing to what it does, network
included. A project without a test command runs nothing. See [Checks](tasks-and-squads.md#checks).

In your git repositories, a squad's task makes a worktree beside the repository, `<repo>-worktrees/task-<N>`, on a new
branch `task/<N>` made from the branch the work lands on (tasks from before platoon 1.0 keep `ticket-<N>` and
`ticket/<N>`). platoon removes the worktree folder (with `git worktree remove`) once the task is Done or Cancelled and its
session has stopped, unless it has changes not yet committed. It never deletes the branch. When you chose "a new branch in
your checkout" instead, platoon runs `git switch -c task/<N> <lands on>` in your folder, and touches nothing there
afterwards. See [Handing a task to a squad](tasks-and-squads.md#handing-a-task-to-a-squad).

## Folders platoon writes

**Config** (`~/.config/platoon`, or `$XDG_CONFIG_HOME/platoon`, or `$PLATOON_CONFIG_DIR`): things you made or chose, kept
because they would be lost or cost tokens to make again.

- `settings.json`, `keys.json` (yours; platoon never writes it), `setup.json` (what an earlier setup remembered about
  the `/to-tickets` skill it installed)
- `names.json` (names you gave), `archive.json` (archived conversations), `summaries.json` (smart names and summaries)
- `projects.json`, and `projects/<id>/`: the notes (`project.md`), their `history/`, `notes-state.json`, and the tasks
  (`tasks.json`, `tasks/` with each task's text, result, its own folder and the squad it was started with; `tickets.json`
  and `tickets/` until platoon has moved a project's, keeping `tickets.json.bak`); `tasks.json` and `tasks/` at the top for
  tasks in no project
- `briefs/` (your squads' briefs, and built-in ones you changed, `<id>.md`), `agents/` (yours, and built-in ones you
  changed), `prompts/` (system prompts you changed), `themes/` (your own themes); `squads/` holds the JSON files squads
  were before briefs, each kept as `<id>.json.bak` once it was turned into a brief
- `<id>.md.bak` and `<id>.json.bak` beside a brief or an agent you deleted or put back as platoon has it

**Cache** (`~/.cache/platoon`, or `$XDG_CACHE_HOME/platoon`, or `$PLATOON_CACHE_DIR`): things platoon can make again, or
small marks its parts leave for each other.

- `index.json` and `prompts.jsonl`: the index of your conversations and the messages you typed, for the list and search.
  Deleting them is harmless: they are rebuilt.
- `hosts/`: one small file and one socket per session platoon runs, in a folder only you can enter
- `status/`: the figures the status line reports, per session
- `ai-usage.json`: tokens used per day (numbers only), and `ai-work/`, the empty folder the `claude -p` calls run in
- `notes-sent/`, `notes-due/`, `notes-updating/`, `notes-editing/`, `notes-helper/`: marks for the project notes
- `project-launches/`, `editor-open/`, `stale/`: marks for sessions started in a project, folders open in an editor, and
  copies of a conversation the guard has turned away
- `deleted-projects/`: one backup of each project you deleted
- `version-check.json`: when platoon last asked GitHub for its latest version, what it said, and which new platoon and
  Claude Code versions it has already told you about (see [below](#one-network-request-no-telemetry))

**Beside your repositories**: the `<repo>-worktrees/` folders for tasks, described [above](#squads).

**In Claude Code's folder**: only what setup adds to `settings.json`, and the backup beside it.
platoon never writes Claude Code's conversations or its other files. Claude Code itself makes one empty folder under
`~/.claude/projects` for platoon's `claude -p` calls, and the sessions platoon starts are ordinary Claude Code sessions,
saved by Claude Code like any other.

## Processes

- Each session started in platoon runs under a small background host, which keeps running when platoon closes. A host
  stops its own session after it has sat unused for the time in the settings (2 hours unless you change it).
- On its own, platoon only stops sessions it runs. Ending any other copy of a conversation (archiving one that runs in a
  terminal platoon didn't start, or opening one platoon can't share) asks you first.
- After stopping a session, platoon asks the MCP servers that session had started to exit too, and never any other
  process.
- The project notes helper runs in the background, one per project, for at most half an hour.
- A project's test command runs when one of its squads finishes (see [Squads](#squads)); platoon stops it, and
  everything it started, when its time is up or platoon closes, and nothing else.

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
running sessions in `~/.claude/sessions`, the skills under `~/.claude/skills`, your MCP servers in `~/.claude.json`
(for idle stop it matches their commands to running processes; the Squads tab shows only their names), and the agents
your coding tool already has: `~/.claude/agents/*.md`, and a task's repository's `.claude/agents/*.md` when the task is
handed to a squad. It only reads those agent files; it never changes, copies or deletes them unless you press `c` on one,
which copies it into platoon's own agents. These files are
not a documented interface, so **a Claude Code update can break platoon**. It was last checked with Claude Code 2.1.288.
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
   `platoon hook stop`. `settings.json.platoon-bak` is the file as it was before the last run of setup that changed it;
   copy it back only if nothing else has changed the file since.
   platoon won't start again without the status line and the guard: running `platoon setup` puts them back.
2. **The skill**, if an earlier platoon installed it. `platoon setup --remove to-tickets`, or delete
   `~/.claude/skills/to-tickets/`.
3. **platoon's own folders.** Delete `~/.config/platoon` and `~/.cache/platoon`. The config folder holds your projects'
   notes, tasks, squads and settings: keep a copy if you want them.
4. **Task worktrees and branches.** Any `<repo>-worktrees/task-<N>` (or `ticket-<N>`) still there can go with
   `git worktree remove <path>`, and the `task/<N>` branches with `git branch -D task/<N>`, once you no longer need the
   work on them.
5. **The program**, the way you installed it: `brew uninstall platoon`; `cargo uninstall platoon-tui`; for the install
   script, delete `~/.local/bin/platoon` (or where it put it), `~/.config/platoon-tui/`, and the line it added to your
   shell's startup files (`. "$HOME/.config/platoon-tui/env.sh"`); with Nix, remove it from your profile or
   configuration.

Sessions platoon was running keep running until they stop on their own or you end them; your conversations themselves
are Claude Code's and stay where they are.
