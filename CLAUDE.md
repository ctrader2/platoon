# platoon

A terminal app (Rust, ratatui) for managing Claude Code sessions on Linux and macOS: every conversation in one list, live
shared sessions, archive, themes.

A personal `CLAUDE.local.md` (gitignored) may add a maintainer's own workflow on top of this file.

## Working rules

- **Discuss before building** anything non-trivial: plan, ask the questions that change the answer, then build.
- **Commit only when green**: `cargo test` fully passes (run it twice: a test that fails once is a race to fix, not to
  retry), `cargo build --release` has no warnings, `cargo fmt --check` finds nothing to change and
  `cargo clippy --all-targets -- -D warnings` passes. The commit message says why.
- If there is uncommitted work you did not write, find out what it is before building on top of it, and never sweep it into
  an unrelated commit (`git add` files by explicit path).
- Keep the file table below current when a source file is added or its job changes.
- A change to what the user sees or does updates the matching page in `guide/` (and the README front page if it mentions
  it) in the same commit.

## Safety (platoon runs against your real Claude Code setup)

Whoever runs the tests probably uses Claude Code and platoon on the same machine, so:

- Tests are hermetic: temp dirs for `CLAUDE_CONFIG_DIR`, `PLATOON_CACHE_DIR` and `PLATOON_CONFIG_DIR`. Never write to the real
  `~/.claude`, `~/.claude.json`, `~/.cache/platoon`, `~/.config/platoon`, and never touch the real clipboard from a test.
- Never run `platoon setup` against the real config, never edit `~/.claude/settings.json`, and never `cargo install` or
  replace an installed `platoon` (Claude Code settings may point at it). Use `target/`.
- Never signal or kill a process you did not start: live `platoon host ... claude --resume ...` sessions and Claude Code
  itself may be running. Use `ps`; never broad `pkill -f`/`pgrep -f` patterns (they match your own shell).
- In a unit test the real folders cannot be reached: `usage::cache_dir`, `archive::config_dir`, `claude::claude_root` and
  `claude::claude_json_path` panic (`usage::forbid_real_dir`). Code a test runs takes its folders as arguments: an `App`
  has its own (`App::host_dirs`), and looks for and starts hosts in them (`host::discover_in`, `host::launch`), never in
  the real `~/.cache/platoon/hosts` where the live sessions' sockets are.
- serde_json: mutable indexing (`v["a"]["b"]`) silently creates null keys. Use `get_mut`.

## Build and test

```
export PATH="$HOME/.cargo/bin:$PATH"     # each shell call starts fresh
cargo test                               # ~40s; tests/idle_stop.rs alone is ~20s
cargo build --release                    # must be warning-free
cargo fmt --check                        # the code is kept formatted: run `cargo fmt` to fix
cargo clippy --all-targets -- -D warnings  # no clippy warnings, tests included
```

`tests/fake-claude.sh` is the stand-in `claude -p` (records every call, answers from files) shared by `testkit::AiFake` and `tests/common`.

Test environment knobs: `PLATOON_CLAUDE_BIN` (a stand-in for `claude`), `PLATOON_SKIP_SETUP_CHECK=1`, `PLATOON_BADGES`,
`PLATOON_COLORS`, `PLATOON_IDLE_SECS_PER_MINUTE`, `PLATOON_CLIPBOARD`, `PLATOON_NOTES_QUIET_SECS`, `PLATOON_NOTES_MAX_LIFE_SECS`,
`PLATOON_NOTES_HELPER=off`, `PLATOON_NO_UPDATE_CHECK=1` (no version checks: no request to GitHub, no `claude --version`;
every test that starts the binary sets it, and a unit test's `App` never starts them), `PLATOON_EDITOR_PATH` (the only place the notes editors and `v`'s editors are looked for; `v`'s
tests put stand-in `code`/`hx` there: `tests/editor.rs`'s `hx` records what it was started with), `PLATOON_EDITOR_BIN`, and
`PLATOON_FORBID_REAL_EDITOR` (set by `Term::spawn`: no real editor, VS Code included, can start). A unit test's app finds no
`v` editors at all (`App::set_file_editors_for_test` gives it stand-ins). Run the whole suite twice before trusting it and fix races instead of retrying. In a terminal test, a frame arrives
as a stream and `wait_for` can return halfway through one: wait for every line you rely on (`Term::wait_for_all`) and
snapshot only from `Term::stable()` when the claim is about the whole frame (what is missing, what order, how many). A real `claude` cannot be
used in tests (trust-folder prompt): use `sh` or a fake script.

## CI

`.github/workflows/ci.yml` runs on every push to `main` and every pull request, on Linux: the checks above (warnings are
errors: `RUSTFLAGS=-D warnings`), `scripts/check-claude-version.sh`, and a build on the minimum Rust (1.88,
`rust-version`). macOS (arm64: test and release build) runs on pushes to `main` and when started by hand (`gh workflow run ci.yml
--ref main`), not on pull requests while the repo is private (10x minutes); once public, on pull requests too. Every
job has a `timeout-minutes` (Linux 20, minimum Rust 15, macOS 25, deny 10, watch 5) so a hang cannot eat the free minutes.
The macOS tests run with `--no-fail-fast -- --nocapture`, so a failure's message is in the log even if a later test hangs.
Tests must hold on macOS too: no `/proc`, no GNU-only flags (`sed -i`, `date -d`), no `/bin/true`, no case-sensitive file
names, and a temp folder that may be under the home folder (paths there are shown from `~`).
`deny.yml` runs `cargo deny check` (`deny.toml`: licenses, advisories, crates.io only) with CI and weekly.
`codeql.yml` runs CodeQL (Rust and the workflows, no build) on pushes to `main`, pull requests and weekly; while the
repo is private it skips itself (code scanning needs a public repo or GitHub Code Security). Dependabot (`dependabot.yml`)
opens weekly grouped minor/patch updates for cargo and the actions.
`checked-with/claude-code` is the Claude Code version platoon was last checked with (compiled in: the notice when a user's
is newer): README.md and the guide must name the same one (the script fails CI otherwise), so change them in one commit. `claude-code-watch.yml` checks npm daily and
opens an issue "Check support for Claude Code X" for a newer version; no workflow commits, pushes or edits anything.
`release.yml` runs only on a pushed version tag (never on `main` or pull requests): written by `dist` from
`dist-workspace.toml` (`dist generate`, then put back its one hand edit, the permissions; see the file's top), it builds
the release binaries, the install script and the Homebrew formula (tap `ctrader2/homebrew-tap`, secret
`HOMEBREW_TAP_TOKEN`) and calls `publish-crate.yml` (crates.io trusted publishing; skipped until the repository variable
`CRATES_IO_TRUSTED_PUBLISHING` is `true`). Release notes come from `CHANGELOG.md`.

## Where things live (`src/`)

| File | What it does |
| --- | --- |
| `main.rs` | entry point, subcommands (`setup`, `open`, `host`, `statusline`, `hook guard`, `hook session-start`, `hook stop`, `hook squad-stop`, `hook squad-gate`, `hook squad-notify`, `hook squad-tool`, `hook squad-prompt`, `themes`, `ticket`), event loop (draws only when something may have changed, and sends a frame only when it differs from the one showing: `Screen`; `App::animating` says when a tick needs a frame), handing the terminal to a terminal editor of the user's (`v`) and back (`open_own_editor`, `hand_over`); most unit tests live here (the density and narrow-width render tests in `density_app_tests.rs`; popups, cut text, notices, the help's sections, the words on screen, the search's marks and the settings' choosers in `polish_app_tests.rs`; the description line ending at STATUS, running vs not running, the notes editor's leave question, the editor's one name, non-breaking wraps and help headings at the fold in `clarity_app_tests.rs`); the line quitting prints (`quit_line`) |
| `app.rs` | app state and all input handling (modes: list, filter, picker, confirm, attached, settings, theme picker); tabs Sessions, Projects, Squads (`1` `2` `3`); a notice belongs to the screen it was given on (`settle_notice`, `ScreenId`) |
| `ui.rs` | all drawing (header and tabs, rows (two lines each, `ROW_H`; the columns and the order they go in as the table narrows, `cols`), footer, popups (every one drawn on `popup_ground`: the screen behind faded, no sliver at its sides, titles in sentence case), scrolling panes that say what is below them (`scroll_view`), attached view, the message below the smallest size `app::MIN_WIDTH` x `MIN_HEIGHT`; no frame around the screens; the `?` help popup, built from the footers' own hint tables, in `ui/help_view.rs`; the Squads tab in `ui/squads_view.rs` (one pane under `TWO_PANES_FROM`)); `model.rs` the row types |
| `claude.rs`, `index.rs` | read Claude Code's undocumented on-disk state (also the installed skills, `skills/*/SKILL.md`, and the names only of the user's MCP servers in `.claude.json`); the persistent incremental index of conversations |
| `host.rs`, `client.rs`, `open.rs` | the detached per-session host (owns claude's pty, serves many windows; its files in `<cache>/hosts` of the window that started it: `Dirs`, passed on to the host only when they are not the environment's), the client, `platoon open` |
| `editor.rs`, `app/editor_glue.rs` | `v`, opening a ticket's or a session's folders in an editor the user has: which folders it opens (a session's with its `/add-dir` folders, a ticket's worktrees, its plain folders and work folder, a picked folder: `Request`/`Root`), running a terminal editor in the foreground on platoon's terminal (`Launch`, `run`; `main.rs` hands the terminal over and back), marks of open folders (`<cache>/editor-open`) that keep the worktree clean-up away (`editor.rs`); the app side: what `v` opens from where, the favourite (none chosen: the first found), "Always ask", the squad-working question, bringing a removed worktree back first, `NO_EDITOR` when this computer has none (`app/editor_glue.rs`). platoon installs no editor |
| `guard.rs`, `setup.rs`, `usage.rs` | one-writer-per-conversation hook (which also carries a project's notes into a running session), the `platoon setup` checklist and gate, statusline plan usage |
| `archive.rs`, `settings.rs`, `dirs.rs` | archive list, settings file and the settings screen catalog, directory picker sources |
| `keys.rs` | the keys the user can change in `<config>/keys.json` (`back`, default `ctrl+space`; `quit`, default `q`; the `ACTIONS` table is where more join): key strings (`Chord::parse`, `label`), the back key's rules (`check_back`: a ctrl/alt chord both places can tell apart, Claude Code's own chords warned), matching a crossterm `KeyEvent`, the raw byte forms for `platoon open` (`raw_forms`, `find_back`), `KeysStore` (re-read on change, each problem said once; never written). The app turns the back key into esc on its own screens (`App::on_key`); `q`/quit goes back below the top-level lists (`list_key`, `at_top_level`) |
| `mcp.rs`, `procs.rs` | telling MCP servers from other child processes (idle stop), process table (Linux `/proc`; macOS `ps`, and the kernel for a process's arguments and whether it is a zombie) |
| `theme.rs`, `usertheme.rs`, `colordepth.rs`, `src/themes/` | built-in themes, dark and light, and contrast rules (symmetric: `Theme::light`, `text_on` for text on a colored fill; the selection fill `surface` at `MIN_SURFACE` from the page, and text on it held to its page floor where it is drawn: `Theme::readable_on`/`floor_of`, applied to every frame by `ui::keep_readable_on_selection`; key hints in `ui::hint_fg`, at 4.5:1; the fade behind popups, backgrounds too, is `ui::fade_behind`): the ones in `SPECS` (all dark) and twenty kept as theme files in `src/themes/` (`FILES`, built as a user's file is; 8 light; ids are their names' slugs; once the Theme pack plugin, a saved `plugin:theme-pack/<slug>` reads as `<slug>` in `settings.rs`; credits in `THIRD_PARTY_NOTICES.md`), user theme files (`theme_from`: a dark one lifted until it reads, a light one held to `problems`, the floors the built-in theme files meet as they are; ids from `slug`, accented Latin letters as their plain letter; `build`; a file's own role colors `platoon-accent` ... and which slots a light theme's roles come from: `Parsed::role_picks`), the `Registry`, 256-color fallback; the theme files' tests (each the colors the plugin gave it: `testdata/theme_pack_colors.tsv`) and the light-theme rendering tests in `light_theme_tests.rs`; contrast as drawn in every built-in (hints, the selected row, the fade) in `contrast_app_tests.rs` |
| `ai.rs`, `ledger.rs` | the token plumbing: feature table, gate (switches, daily limit, plan pause, back-off), the `claude -p` runner, `platoon tokens`; the numbers-only usage ledger (`ai-usage.json`) |
| `select.rs`, `clipboard.rs` | text selection and copy in the attached view (in progress, see "Where we are") |
| `ai.rs`, `ledger.rs` | the token plumbing: feature table, gate (switches, daily limit, plan pause, back-off; `gate_for` says "user initiated"), the `claude -p` runner, numbers-only usage ledger |
| `instructions.rs` | each AI action's instructions: the table (entry, built-in guidance, the fixed part platoon keeps), the user's own guidance in `<config>/prompts/<action>.md` with fallback and an 8,000-character cap, `built-in.json` (which built-in text a file started from), edit/restore (the user sees them as "System prompt for ..."); every caller builds its system prompt as guidance + fixed part (the squad leaders' entry, `squad-leader`, is shared by every squad; its fixed part is `squads::leader_fixed`) |
| `smart.rs`, `summaries.rs`, `scan.rs`, `app/smart_glue.rs` | smart names and "Describe it": what counts as a substantive message, digests, reply validation, who is due and in what order (`smart.rs`, pure); `summaries.json` in the config dir; the first pass inside `platoon setup`; the app's background worker and the find flow |
| `projects.rs`, `sessionhook.rs`, `app/projects_glue.rs` | projects: `projects.json` (projects, membership, archived-by-project), notes paths, color slots, sessions started under a project (`<cache>/project-launches`); `platoon hook session-start` (notes as SessionStart context, sent hashes in `<cache>/notes-sent`); the app's tabs, Projects tab, project screen, picker and lifecycle |
| `notesupdate.rs`, `notesrun.rs` | keeping a project's notes up to date: the fixed parts (Summary, Open questions, Decisions) and the sections between, the prompt, the reply and the merge rules (`notesupdate.rs`, pure, plus `notes-state.json`); the `Stop` hook, the detached helper (`platoon notes-update <id>`), the quiet timer, the per-project lock and the marks in `<cache>/notes-due`, `notes-updating`, `notes-editing`, `notes-helper` |
| `handoff.rs`, `gitfacts.rs`, `textbox.rs`, `app/handoff_glue.rs` | the hand-off (`h`): the fixed document, the facts platoon fills, the one piece of instructions, reply checking (`handoff.rs`, pure); what git says about a folder, read-only and time-boxed (`gitfacts.rs`); wrapping, cursor and scrolling for a text box (`textbox.rs`, pure); the app side: git and the call off the UI thread, the box, starting the new session |
| `tickets.rs`, `app/tickets_glue.rs` | tickets: `tickets.json` per project (numbers never reused, unknown keys and states kept, lock + atomic writes, a corrupt file never written), states, waits for / ready / loop refusal, the body template, where a ticket came from (`from`), `platoon ticket add/list/where` and `--from-session` (`tickets.rs`); the ticket screen's From line and opening that session are in `app/tickets_glue.rs`; the Tickets pane, a ticket's screen, `c` (cancel, after asking: the only state the user sets by hand), `S` hands it to a squad (`a` there only says so), its lists (folders, waits for) and making one (`app/tickets_glue.rs`; the body editor goes through `app/notes_glue.rs`); also each ticket's squad leaders, worktrees, what the squad said, the go-ahead, `user_typed` (the prompt hook's Working again), its result file, and `leader_ids`/`find_leader` across projects |
| `squads.rs`, `squadfile.rs`, `agentfile.rs`, `app/squads_glue.rs`, `app/squadtab_glue.rs`, `ui/squads_view.rs` | squads: building a leader session from a squad resolved against its agents (`squadfile::Crew`; switches, skills and MCP servers -> tools, the `--agents` JSON with `skills`/`mcpServers`, the leader's prompt = the shared "System prompt for squad leaders" + what the squad can do + the workflow platoon writes + the fixed rules, its model from the settings (`LeaderSetup`), `Place` (worktrees of the git folders, or the work folder for a ticket with no folder), `Launch`, which is also the snapshot kept beside the ticket), the commander, the leader's first message, the turn-ending line (the leader asks "Shall I close this ticket?" before `SQUAD: done`) and the squad-stop decision, the publish matcher, `platoon hook squad-stop`/`squad-gate`/`squad-notify`/`squad-tool`/`squad-prompt` (a message from the user makes the ticket Working again), worktrees, the leader's flags (`squads.rs`, pure but for git and the hooks); the definitions: the built-in squads in code, the user's own and changed built-ins in `<config>/squads/<id>.json` (steps name agents by id; an old file with members inside is converted once, `convert_old`, `.before-agents.json.bak`) (unknown keys kept, lock + atomic writes, a corrupt file never written, restore/delete keep a `.bak`), the rules every squad must follow (`problems`), drafting a squad (`claude -p` on Sonnet, `parse_draft`, reusing agents, `Draft.new_agents`), carrying old `prompts/coding-*.md` edits over once into the built-in agents (`squadfile.rs`; a squad has no leader or folder switches of its own: an old file's `leader`/`needs_folder`/`use_worktree` are kept as unknown keys); agents, shared by squads: the 12 built-ins, `<config>/agents/<id>.json` (same rules as squads; `skills` and `mcp_servers` by name), an agent's `problems`, `delete` refused while a squad uses it (`in_use`), drafting an agent, `Installed` (`agentfile.rs`); the app side: the squad picker (`a`, every squad by name), starting and continuing a leader in a host (from its snapshot; a leader without one uses the squad as it is now), the split view, `p`, stopping a cancelled ticket's leader (`stop_squad`), and the set of leader ids kept out of every list (`app/squads_glue.rs`); the Squads tab (`3`): the Squads/Agents lists (`←` `→`), details, every edit, the step's agent picker with "+ New agent", `g`, "Used in", an agent's skills/MCP pickers (installed, or typed), `n` (squad or agent), copy/delete/restore (`app/squadtab_glue.rs`), drawn in `ui/squads_view.rs` (a squad's workflow as a rail with its send-back loops, `Rail`) |
| `worktrees.rs`, `app/worktree_glue.rs` | a ticket's worktrees after the work: "changes not yet committed", and `remove` (only the `<repo>-worktrees/ticket-<N>` folder the ticket's record points at, listed by git on `ticket/<N>`; `git worktree remove`, never the branch) (`worktrees.rs`); the app removes a Done or Cancelled ticket's worktrees once none of its squad's sessions runs and no session is in them, off the UI thread, marks `removed` / `kept_uncommitted` on the ticket (`Tickets::mark_worktree`), `w` removes a kept one after a yes, and continuing the squad (enter) makes a removed one again from its branch first (`app/worktree_glue.rs`; the `squad-prompt` hook does the same for a leader resumed some other way, `squads::bring_worktrees_back`) |
| `squadqueue.rs`, `squadwork.rs`, `app/squadwork_glue.rs` | squads at work at once and the line: who holds a place (Working + its leader running, or written on the ticket under a minute ago), the line oldest first (`squadqueue.rs`, pure; the ticket's `queued` mark and `claim_queued` under the tickets lock are in `tickets.rs`; `start_or_queue`/`start_from_line` and the `squad-line.lock` only one window holds are in `app/squads_glue.rs`); what a squad has done: tokens per ticket and per agent, the members' runs `[x]`/`[>]`, "Now:" (`squadwork.rs`, pure, from `claude::read_leader`); reading the open project's leader transcripts off the UI thread, cached by size and time (`app/squadwork_glue.rs`) |
| `totickets.rs`, `skills/to-tickets/SKILL.md` | the `/to-tickets` Claude Code skill: its text (`SKILL.md`, compiled in), writing it to `<claude config dir>/skills/to-tickets/`, out of date, removing it (the Tickets skill step of `setup.rs`, `platoon setup --remove to-tickets`); `platoon squads --kinds` |
| `fileeditors.rs` | the editors `v` can open a folder in: finding the ones installed (PATH, JetBrains Toolbox scripts, snaps, `/opt/*/bin`, `~/.local/bin`, macOS `.app` bundles; the user's own Neovim only with a config folder), the argv per editor (one window for all folders; JetBrains a window each, at most 3; Helix/own nvim the first), starting a windowed one detached, a terminal one's `editor::Launch`; the choice itself (favourite, "Always ask", the list, a worktree kept `opened_in` a windowed editor) is in `app/editor_glue.rs` and `app/worktree_glue.rs` |
| `mdview.rs` | Markdown to styled, wrapped terminal lines (headings, lists with hanging indents, bold/italic, code, quotes, links as text; plain-glyph bullets), pure; draws the notes pane |
| `prompts.rs` | the local prompt index (also holds one line of Claude's last reply and the substantive-message count per conversation) |
| `testkit.rs`, `perf.rs` | test fixtures, scale checks with thousands of synthetic transcripts |
| `.github/workflows/`, `deny.toml` | CI (`ci.yml`: Linux checks, minimum Rust, macOS on `main`), `deny.yml` (`cargo deny check` against `deny.toml`, also weekly), `claude-code-watch.yml` (daily: an issue for a newer Claude Code), `release.yml` (written by `dist`: a tagged release) and `publish-crate.yml` (crates.io, called by it) |
| `versions.rs` | the two version notices: Claude Code newer than `checked-with/claude-code` (compiled in; `claude --version`), and a newer platoon from GitHub's latest release (`curl`, once a day at most: platoon's only network request), each said once per version, with the update command for how platoon was installed (`install_kind`); `<cache>/version-check.json`; the `update_check` setting and `PLATOON_NO_UPDATE_CHECK`; run off the UI thread when the app starts (`App::start_version_checks`), shown above the footer of the top-level lists (`App::news_showing`, tests in `versions_app_tests.rs`); `setup_line` for `platoon setup` |
| `CHANGELOG.md`, `dist-workspace.toml`, `flake.nix`, `flake.lock` | the changelog (Keep a Changelog; each release's notes); `dist`'s config for releases; the Nix flake (`nix run github:ctrader2/platoon`) |
| `checked-with/`, `scripts/` | `checked-with/claude-code`: the Claude Code version platoon was last checked with (one file per tool later); `scripts/check-claude-version.sh` (the docs name that version), `scripts/claude-code-watch.sh` (the watch's logic; `DRY_RUN=1` locally) |
| `.github/workflows/codeql.yml`, `.github/dependabot.yml` | CodeQL for Rust and the workflows (actions pinned by commit, skipped while the repo is private), Dependabot weekly for cargo and github-actions (minor and patch grouped, at most 3 open each) |
| `.github/ISSUE_TEMPLATE/`, `.github/pull_request_template.md`, `.github/CODEOWNERS` | the issue forms (bug: versions, OS, terminal, theme; feature: the problem first), `config.yml` (no blank issues; questions to Discussions, vulnerabilities to private reporting), the pull request checklist, `* @ctrader2` |
| `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md` | for contributors: before you start, build and test, the hermetic-test rule, a good pull request, AI-assisted contributions (welcome, disclosed, the contributor owns every line), MIT in = out; Contributor Covenant 2.1 (reports through GitHub, no email); what counts as a vulnerability and how to report it privately (latest release only) |
| `demo/social-preview.png` | the repository's social preview (1280 x 640), uploaded by hand in the settings; how it was made is in `demo/README.md` |
| `demo/` (repo root) | the README animation: `make-world.sh` (a made-up world in a temp folder: conversations, projects, notes, tickets, stand-in `claude` and `code`, running sessions as shell loops), `demo.tape` (the vhs recording, to the ignored `raw.mp4`), `render.sh` (ffmpeg: title card, captions, eased zooms; `platoon.gif`, the headline cut under 10 MB, and `platoon-tour.mp4`, the whole tour); how to re-record in `demo/README.md` |

Notes that matter: rows are conversations keyed by session id (several live processes can share one); all knowledge of
Claude's file formats stays in `claude.rs`; the settings file preserves keys it does not know.
