# platoon

A terminal app (Rust, ratatui) for managing Claude Code sessions on Linux and macOS: every conversation in one list, live
shared sessions, archive, themes. Private repo `ctrader2/platoon`, branch `main`.

## Start of every session

1. Read **"Where we are"** at the top of `docs/requirements.md` (a session-start hook prints it). It says what is built,
   what is in progress (including any uncommitted work), what is unverified and what is next.
2. `git status -sb`, `git log --oneline -5` and `git worktree list` (work in progress lives in worktrees, see "Parallel
   work"). If there is uncommitted work you did not write, find out what it is before building on top of it, and never
   sweep it into an unrelated commit (`git add` files by explicit path).

## The notes (`docs/`)

`docs/` is a symlink to `~/repos/project-docs/platoon/` (private repo `ctrader2/project-docs`), and it is gitignored here.
It holds `requirements.md` (design notes and decisions) and `agent-brief-template.md`.

- When a decision is made or something is built, **update `docs/requirements.md`** in the same piece of work: append a dated
  section, and keep the short **"Where we are"** section at the top current (built / in progress / unverified / next).
- A Stop hook commits and pushes changes to the notes repo at the end of every turn. If it ever fails, commit and push in
  `~/repos/project-docs` by hand. Never put secrets or tokens in the notes.

## Working rules

- **Discuss before building** anything non-trivial: plan, ask the questions that change the answer, then build.
- **Commit when a change is finished and green**: `cargo test` fully passes and `cargo build --release` has no warnings.
  Commit with a message that says why, ending with the trailer
  `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>`. Never commit a failing or half-built state. No force push, no
  amending or rewriting pushed commits.
- **Implementation steps go to a fresh agent** with a full brief (`docs/agent-brief-template.md`): context, the user's
  decisions, the task, the safety rules, the quality bar, and the report format. An agent's report is a claim: verify it
  yourself before telling the user it is done.

## Parallel work: one worktree per piece of work (decided 2026-09-25)

Up to **three** pieces of work run at once, each in its own git worktree. `~/repos/platoon` itself stays on a clean `main`
(the user reinstalls from it) and nobody builds features there.

1. **Start**: `git worktree add ~/repos/platoon-worktrees/<name> -b work/<name> main`. The agent's brief names that folder;
   the agent works, builds and tests only there (its own `target/`, so the first build is from scratch).
2. **The agent** commits on `work/<name>` when green (full suite twice, release build clean) and stops. It does **not**
   rebase, merge or push, and it does **not** edit `docs/` (a worktree has no `docs/`): its "built" write-up for the notes
   goes in its final report.
3. **Landing** (`/land <name>`), done by the orchestrator, **one branch at a time**: rebase the branch onto the latest
   `main`, resolve conflicts, then `/verify` checks on the combined code (tests twice, release build, the user's files,
   stray processes); only when green, fast-forward `main`, `git push origin main`, write the notes, and remove the worktree
   and the branch. After each landing, every branch still waiting is rebased and retested again before it can land.
4. **Conflicts**: the orchestrator resolves the mechanical ones; anything that needs judgement goes back to the agent that
   wrote the code (it keeps its context), and the result is retested.
5. **Branches stay local** until they land (so they can be rebased freely without a force push); only `main` is pushed.
6. Parallel work goes best when the pieces touch different areas: `app.rs`, `ui.rs`, `main.rs`, `settings.rs` and the
   file table below are where conflicts will come from; say so in the brief when two pieces share one.
- Tell the user plainly what was verified and what was not. Nobody here can see colors, run macOS, or use a real `claude`
  past its trust-folder prompt: say so, and say what to eyeball.
- The user's terminal is Xfce Terminal on X11 (Ghostty is installed too). Explain in plain language; the user does not want
  jargon in the UI.

## Safety (the user's live setup runs on this machine)

- Tests are hermetic: temp dirs for `CLAUDE_CONFIG_DIR`, `PLATOON_CACHE_DIR` and `PLATOON_CONFIG_DIR`. Never write to the real
  `~/.claude`, `~/.claude.json`, `~/.cache/platoon`, `~/.config/platoon`, and never touch the real clipboard from a test.
- Never run `platoon setup` against the real config, never edit `~/.claude/settings.json`, never run `cargo install` or replace
  `~/.cargo/bin/platoon` (the user's Claude Code settings point at it; the user reinstalls when they want to). Use `target/`.
- Never signal or kill a process you did not start. The user's own `platoon host ... claude --resume ...` sessions and their
  Claude Code session are running. Use `ps`; never broad `pkill -f`/`pgrep -f` patterns (they match your own shell).
- serde_json: mutable indexing (`v["a"]["b"]`) silently creates null keys. Use `get_mut`.

## Build and test

```
export PATH="$HOME/.cargo/bin:$PATH"     # each shell call starts fresh
cargo test                               # ~40s; tests/idle_stop.rs alone is ~20s
cargo build --release                    # must be warning-free
```

`tests/fake-claude.sh` is the stand-in `claude -p` (records every call, answers from files) shared by `testkit::AiFake` and `tests/common`.

Test environment knobs: `PLATOON_CLAUDE_BIN` (a stand-in for `claude`), `PLATOON_SKIP_SETUP_CHECK=1`, `PLATOON_BADGES`,
`PLATOON_COLORS`, `PLATOON_IDLE_SECS_PER_MINUTE`, `PLATOON_CLIPBOARD`, `PLATOON_NOTES_QUIET_SECS`, `PLATOON_NOTES_MAX_LIFE_SECS`,
`PLATOON_NOTES_HELPER=off`. `tests/common` `Term::spawn` fixes them for
terminal tests. Run the whole suite twice before trusting it and fix races instead of retrying. In a terminal test, a frame arrives
as a stream and `wait_for` can return halfway through one: wait for every line you rely on (`Term::wait_for_all`) and
snapshot only from `Term::stable()` when the claim is about the whole frame (what is missing, what order, how many). A real `claude` cannot be
used in tests (trust-folder prompt): use `sh` or a fake script.

## Where things live (`src/`)

| File | What it does |
| --- | --- |
| `main.rs` | entry point, subcommands (`setup`, `open`, `host`, `statusline`, `hook guard`, `hook session-start`, `hook stop`, `hook squad-stop`, `hook squad-gate`, `hook squad-notify`, `hook squad-tool`, `hook squad-prompt`, `themes`, `ticket`), event loop; most unit tests live here |
| `app.rs` | app state and all input handling (modes: list, filter, picker, confirm, attached, settings, theme picker); tabs Sessions, Projects, Squads (`1` `2` `3`) |
| `ui.rs` | all drawing (header and tabs, rows, footer, popups, attached view; the Squads tab in `ui/squads_view.rs`); `model.rs` the row types |
| `claude.rs`, `index.rs` | read Claude Code's undocumented on-disk state (also the installed skills, `skills/*/SKILL.md`, and the names only of the user's MCP servers in `.claude.json`); the persistent incremental index of conversations |
| `host.rs`, `client.rs`, `open.rs` | the detached per-session host (owns claude's pty, serves many windows), the client, `platoon open` |
| `guard.rs`, `setup.rs`, `usage.rs` | one-writer-per-conversation hook (which also carries a project's notes into a running session), the `platoon setup` checklist and gate, statusline plan usage |
| `archive.rs`, `settings.rs`, `dirs.rs` | archive list, settings file and the settings screen catalog, directory picker sources |
| `mcp.rs`, `procs.rs` | telling MCP servers from other child processes (idle stop), process table |
| `theme.rs`, `usertheme.rs`, `colordepth.rs` | built-in themes and contrast rules, user theme files, 256-color fallback |
| `ai.rs`, `ledger.rs` | the token plumbing: feature table, gate (switches, daily limit, plan pause, back-off), the `claude -p` runner, `platoon tokens`; the numbers-only usage ledger (`ai-usage.json`) |
| `select.rs`, `clipboard.rs` | text selection and copy in the attached view (in progress, see "Where we are") |
| `ai.rs`, `ledger.rs` | the token plumbing: feature table, gate (switches, daily limit, plan pause, back-off; `gate_for` says "user initiated"), the `claude -p` runner, numbers-only usage ledger |
| `instructions.rs` | each AI action's instructions: the table (entry, built-in guidance, the fixed part platoon keeps), the user's own guidance in `<config>/prompts/<action>.md` with fallback and an 8,000-character cap, `built-in.json` (which built-in text a file started from), edit/restore (the user sees them as "System prompt for ..."); every caller builds its system prompt as guidance + fixed part (the squad leaders' entry, `squad-leader`, is shared by every squad; its fixed part is `squads::leader_fixed`) |
| `smart.rs`, `summaries.rs`, `scan.rs`, `app/smart_glue.rs` | smart names and "Describe it": what counts as a substantive message, digests, reply validation, who is due and in what order (`smart.rs`, pure); `summaries.json` in the config dir; the first pass inside `platoon setup`; the app's background worker and the find flow |
| `projects.rs`, `sessionhook.rs`, `app/projects_glue.rs` | projects: `projects.json` (projects, membership, archived-by-project), notes paths, color slots, sessions started under a project (`<cache>/project-launches`); `platoon hook session-start` (notes as SessionStart context, sent hashes in `<cache>/notes-sent`); the app's tabs, Projects tab, project screen, picker and lifecycle |
| `notesupdate.rs`, `notesrun.rs` | keeping a project's notes up to date: the fixed parts (Summary, Open questions, Decisions) and the sections between, the prompt, the reply and the merge rules (`notesupdate.rs`, pure, plus `notes-state.json`); the `Stop` hook, the detached helper (`platoon notes-update <id>`), the quiet timer, the per-project lock and the marks in `<cache>/notes-due`, `notes-updating`, `notes-editing`, `notes-helper` |
| `handoff.rs`, `gitfacts.rs`, `textbox.rs`, `app/handoff_glue.rs` | the hand-off (`h`): the fixed document, the facts platoon fills, the one piece of instructions, reply checking (`handoff.rs`, pure); what git says about a folder, read-only and time-boxed (`gitfacts.rs`); wrapping, cursor and scrolling for a text box (`textbox.rs`, pure); the app side: git and the call off the UI thread, the box, starting the new session |
| `tickets.rs`, `app/tickets_glue.rs` | tickets: `tickets.json` per project (numbers never reused, unknown keys and states kept, lock + atomic writes, a corrupt file never written), states, waits for / ready / loop refusal, the body template, where a ticket came from (`from`), `platoon ticket add/list/where` and `--from-session` (`tickets.rs`); the ticket screen's From line and opening that session are in `app/tickets_glue.rs`; the Tickets pane, a ticket's screen, `c` (cancel, after asking: the only state the user sets by hand), its lists (folders, waits for) and making one (`app/tickets_glue.rs`; the body editor goes through `app/notes_glue.rs`); also each ticket's squad leaders, worktrees, what the squad said, the go-ahead, `user_typed` (the prompt hook's Working again), its result file, and `leader_ids`/`find_leader` across projects |
| `squads.rs`, `squadfile.rs`, `agentfile.rs`, `app/squads_glue.rs`, `app/squadtab_glue.rs`, `ui/squads_view.rs` | squads: building a leader session from a squad resolved against its agents (`squadfile::Crew`; switches, skills and MCP servers -> tools, the `--agents` JSON with `skills`/`mcpServers`, the leader's prompt = the shared "System prompt for squad leaders" + what the squad can do + the workflow platoon writes + the fixed rules, its model from the settings (`LeaderSetup`), `Place` (worktrees of the git folders, or the work folder for a ticket with no folder), `Launch`, which is also the snapshot kept beside the ticket), the commander, the leader's first message, the turn-ending line (the leader asks "Shall I close this ticket?" before `SQUAD: done`) and the squad-stop decision, the publish matcher, `platoon hook squad-stop`/`squad-gate`/`squad-notify`/`squad-tool`/`squad-prompt` (a message from the user makes the ticket Working again), worktrees, the leader's flags (`squads.rs`, pure but for git and the hooks); the definitions: the built-in squads in code, the user's own and changed built-ins in `<config>/squads/<id>.json` (steps name agents by id; an old file with members inside is converted once, `convert_old`, `.before-agents.json.bak`) (unknown keys kept, lock + atomic writes, a corrupt file never written, restore/delete keep a `.bak`), the rules every squad must follow (`problems`), drafting a squad (`claude -p` on Sonnet, `parse_draft`, reusing agents, `Draft.new_agents`), carrying old `prompts/coding-*.md` edits over once into the built-in agents (`squadfile.rs`; a squad has no leader or folder switches of its own: an old file's `leader`/`needs_folder`/`use_worktree` are kept as unknown keys); agents, shared by squads: the 12 built-ins, `<config>/agents/<id>.json` (same rules as squads; `skills` and `mcp_servers` by name), an agent's `problems`, `delete` refused while a squad uses it (`in_use`), drafting an agent, `Installed` (`agentfile.rs`); the app side: the squad picker (`a`, every squad by name), starting and continuing a leader in a host (from its snapshot; a leader without one uses the squad as it is now), the split view, `p`, stopping a cancelled ticket's leader (`stop_squad`), and the set of leader ids kept out of every list (`app/squads_glue.rs`); the Squads tab (`3`): the Squads/Agents lists (`←` `→`), details, every edit, the step's agent picker with "+ New agent", `g`, "Used in", an agent's skills/MCP pickers (installed, or typed), `n` (squad or agent), copy/delete/restore (`app/squadtab_glue.rs`), drawn in `ui/squads_view.rs` (a squad's workflow as a rail with its send-back loops, `Rail`) |
| `worktrees.rs`, `app/worktree_glue.rs` | a ticket's worktrees after the work: the diff base (`diff_base`: the `base` commit written on the ticket when `squads::prepare_worktrees` made it, else the merge-base with `main`/`master`/HEAD), "changes not yet committed", and `remove` (only the `<repo>-worktrees/ticket-<N>` folder the ticket's record points at, listed by git on `ticket/<N>`; `git worktree remove`, never the branch) (`worktrees.rs`); the app removes a Done or Cancelled ticket's worktrees once none of its squad's sessions runs and no session is in them, off the UI thread, marks `removed` / `kept_uncommitted` on the ticket (`Tickets::mark_worktree`), `w` removes a kept one after a yes, and continuing the squad (enter) makes a removed one again from its branch first (`app/worktree_glue.rs`; the `squad-prompt` hook does the same for a leader resumed some other way, `squads::bring_worktrees_back`) |
| `squadqueue.rs`, `squadwork.rs`, `app/squadwork_glue.rs` | squads at work at once and the line: who holds a place (Working + its leader running, or written on the ticket under a minute ago), the line oldest first (`squadqueue.rs`, pure; the ticket's `queued` mark and `claim_queued` under the tickets lock are in `tickets.rs`; `start_or_queue`/`start_from_line` and the `squad-line.lock` only one window holds are in `app/squads_glue.rs`); what a squad has done: tokens per ticket and per agent, the members' runs `[x]`/`[>]`, "Now:" (`squadwork.rs`, pure, from `claude::read_leader`); reading the open project's leader transcripts off the UI thread, cached by size and time (`app/squadwork_glue.rs`) |
| `totickets.rs`, `skills/to-tickets/SKILL.md` | the `/to-tickets` Claude Code skill: its text (`SKILL.md`, compiled in), writing it to `<claude config dir>/skills/to-tickets/`, out of date, removing it (the Tickets skill step of `setup.rs`, `platoon setup --remove to-tickets`); `platoon squads --kinds` |
| `mdview.rs` | Markdown to styled, wrapped terminal lines (headings, lists with hanging indents, bold/italic, code, quotes, links as text; plain-glyph bullets), pure; draws the notes pane |
| `prompts.rs` | the local prompt index (also holds one line of Claude's last reply and the substantive-message count per conversation) |
| `testkit.rs`, `perf.rs` | test fixtures, scale checks with thousands of synthetic transcripts |

Notes that matter: rows are conversations keyed by session id (several live processes can share one); all knowledge of
Claude's file formats stays in `claude.rs`; the settings file preserves keys it does not know.

## Commands

`/verify` checks a finished piece of work the way I check an agent's report. `/land <name>` brings a worktree's branch
onto `main` (rebase, resolve, `/verify` on the combined code, push, notes, clean up).
