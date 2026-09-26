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
| `main.rs` | entry point, subcommands (`setup`, `open`, `host`, `statusline`, `hook guard`, `themes`), event loop; most unit tests live here |
| `app.rs` | app state and all input handling (modes: list, filter, picker, confirm, attached, settings, theme picker) |
| `ui.rs` | all drawing (header and tabs, rows, footer, popups, attached view); `model.rs` the row types |
| `claude.rs`, `index.rs` | read Claude Code's undocumented on-disk state; the persistent incremental index of conversations |
| `host.rs`, `client.rs`, `open.rs` | the detached per-session host (owns claude's pty, serves many windows), the client, `platoon open` |
| `guard.rs`, `setup.rs`, `usage.rs` | one-writer-per-conversation hook (which also carries a project's notes into a running session), the `platoon setup` checklist and gate, statusline plan usage |
| `archive.rs`, `settings.rs`, `dirs.rs` | archive list, settings file and the settings screen catalog, directory picker sources |
| `mcp.rs`, `procs.rs` | telling MCP servers from other child processes (idle stop), process table |
| `theme.rs`, `usertheme.rs`, `colordepth.rs` | built-in themes and contrast rules, user theme files, 256-color fallback |
| `ai.rs`, `ledger.rs` | the token plumbing: feature table, gate (switches, daily limit, plan pause, back-off), the `claude -p` runner, `platoon tokens`; the numbers-only usage ledger (`ai-usage.json`) |
| `select.rs`, `clipboard.rs` | text selection and copy in the attached view (in progress, see "Where we are") |
| `ai.rs`, `ledger.rs` | the token plumbing: feature table, gate (switches, daily limit, plan pause, back-off; `gate_for` says "user initiated"), the `claude -p` runner, numbers-only usage ledger |
| `instructions.rs` | each AI action's instructions: the table (entry, built-in guidance, the fixed part platoon keeps), the user's own guidance in `<config>/prompts/<action>.md` with fallback and an 8,000-character cap, `built-in.json` (which built-in text a file started from), edit/restore (the user sees them as "System prompt for ..."); every caller builds its system prompt as guidance + fixed part |
| `smart.rs`, `summaries.rs`, `scan.rs`, `app/smart_glue.rs` | smart names and "Describe it": what counts as a substantive message, digests, reply validation, who is due and in what order (`smart.rs`, pure); `summaries.json` in the config dir; the first pass inside `platoon setup`; the app's background worker and the find flow |
| `projects.rs`, `sessionhook.rs`, `app/projects_glue.rs` | projects: `projects.json` (projects, membership, archived-by-project), notes paths, color slots, sessions started under a project (`<cache>/project-launches`); `platoon hook session-start` (notes as SessionStart context, sent hashes in `<cache>/notes-sent`); the app's tabs, Projects tab, project screen, picker and lifecycle |
| `notesupdate.rs`, `notesrun.rs` | keeping a project's notes up to date: the fixed parts (Summary, Open questions, Decisions) and the sections between, the prompt, the reply and the merge rules (`notesupdate.rs`, pure, plus `notes-state.json`); the `Stop` hook, the detached helper (`platoon notes-update <id>`), the quiet timer, the per-project lock and the marks in `<cache>/notes-due`, `notes-updating`, `notes-editing`, `notes-helper` |
| `handoff.rs`, `gitfacts.rs`, `textbox.rs`, `app/handoff_glue.rs` | the hand-off (`h`): the fixed document, the facts platoon fills, the one piece of instructions, reply checking (`handoff.rs`, pure); what git says about a folder, read-only and time-boxed (`gitfacts.rs`); wrapping, cursor and scrolling for a text box (`textbox.rs`, pure); the app side: git and the call off the UI thread, the box, starting the new session |
| `mdview.rs` | Markdown to styled, wrapped terminal lines (headings, lists with hanging indents, bold/italic, code, quotes, links as text; plain-glyph bullets), pure; draws the notes pane |
| `prompts.rs` | the local prompt index (also holds one line of Claude's last reply and the substantive-message count per conversation) |
| `testkit.rs`, `perf.rs` | test fixtures, scale checks with thousands of synthetic transcripts |

Notes that matter: rows are conversations keyed by session id (several live processes can share one); all knowledge of
Claude's file formats stays in `claude.rs`; the settings file preserves keys it does not know.

## Commands

`/verify` checks a finished piece of work the way I check an agent's report. `/land <name>` brings a worktree's branch
onto `main` (rebase, resolve, `/verify` on the combined code, push, notes, clean up).
