# Install and set up

## What you need

- Linux or macOS.
- Claude Code (installed as `claude`), OpenAI's Codex CLI (`codex`), or both, signed in. platoon runs your sessions
  with them and reads their files (see [What platoon changes](what-platoon-changes.md)). With Claude Code, everything
  works; with only Codex, platoon lists and opens Codex's conversations (see
  [Codex conversations](sessions.md#codex-conversations)), keeps projects and their notes, runs its smart features on
  Codex's models, and runs tasks in Codex (see [A computer with only Codex](#a-computer-with-only-codex)).
  With neither, `platoon` says what to install instead of starting.
- Only to build it yourself (`cargo install`, or from source): Rust 1.88 or newer, with `cargo`.
- A terminal of at least 60 columns by 14 rows.
- `git`, for [tasks](tasks-and-squads.md#starting-a-task) (they work in git worktrees) and for the git facts in a [hand-off](projects.md#handing-work-on-to-a-new-session). Everything else works
  without it.

## Install

Any one of these. Each puts the `platoon` command on your path (the package is called `platoon-tui`); then run
[`platoon setup`](#set-it-up-platoon-setup) once.

| How | Install | Update |
| --- | --- | --- |
| Homebrew (macOS, Linux) | `brew install ctrader2/tap/platoon` | `brew upgrade platoon` |
| The install script (macOS, Linux) | `curl -LsSf https://github.com/ctrader2/platoon/releases/latest/download/platoon-tui-installer.sh \| sh` | the same command again |
| crates.io | `cargo install --locked platoon-tui` | the same command again |
| Nix | `nix run github:ctrader2/platoon` (or add the flake to your configuration) | `nix run --refresh github:ctrader2/platoon` |
| From source | `git clone https://github.com/ctrader2/platoon && cd platoon && cargo install --path .` | `git pull`, then `cargo install --path .` |

- **The install script** downloads the release for your computer (Linux and macOS, Intel and ARM), checks its checksum
  when your system has the `sha256sum` command, and puts `platoon` in `~/.local/bin` (or `$XDG_BIN_HOME`), adding that
  folder to your path if it isn't there. It keeps a note of where it put platoon (and the lines that add the folder to
  your path) in `~/.config/platoon-tui/`. Each release on [GitHub](https://github.com/ctrader2/platoon/releases) also
  has the files themselves, with their checksums.
- The Linux builds are static: one file that runs on any distribution.
- `cargo install` and building from source need Rust 1.88 or newer.
- `platoon --version` says which version you have.

Whichever you choose, platoon was last checked with the Claude Code and Codex versions named in
[What platoon changes](what-platoon-changes.md#it-reads-claude-codes-and-codexs-own-files), and it
[tells you](#when-claude-code-codex-or-platoon-is-newer) when your Claude Code or Codex is newer, or when a newer platoon is
out.

## Set it up: `platoon setup`

`platoon setup` is a one-time step, and where Claude Code is installed `platoon` won't start until it's done. It shows a
checklist of what it changes in Claude Code's `settings.json` (`~/.claude/settings.json`, or the one in `CLAUDE_CONFIG_DIR`). When that file is already
there, it backs it up first, as `settings.json.platoon-bak` beside it (and says so), and leaves everything else in it
alone, your own hooks included. When it isn't there yet (Claude Code is installed but you haven't run it), setup makes
it, and its folder if needed.

| Step | Why |
| --- | --- |
| Status line | Plan usage (5-hour and weekly) and Claude's own context percentage |
| Session guard | Stops two copies of one conversation from overwriting each other, and keeps a [project's](projects.md) notes in each of its conversations while they run |
| Project notes | Hands a [project's](projects.md) notes to its sessions when they start or resume. Optional: platoon starts without it, projects still work, but their notes don't reach sessions |
| Notes updates | Lets platoon [keep a project's notes up to date](projects.md#notes-that-keep-themselves-up-to-date) after its sessions stop. Also optional |
| /to-tickets | A Claude Code skill: type `/to-tickets` in a session to [turn the work agreed there into tasks](tasks-and-squads.md#to-tickets-in-a-session). Written to `skills/to-tickets/SKILL.md` in Claude Code's folder. Optional too |

The status line and the session guard are the two platoon can't start without. If you already have a status line of
your own, setup leaves it alone and says so: make your own also run `platoon statusline` if you want plan usage in
platoon.

### Codex

When Codex is on this computer (its folder `~/.codex`, or `$CODEX_HOME`, is there, or `codex` is on your path), setup also
adds platoon's hooks to Codex's `hooks.json` in that folder, so a [project's](projects.md) notes reach its Codex sessions as
they reach Claude Code's, and a [task](tasks-and-squads.md#tasks-on-codex) can run in Codex:

| Step | Why |
| --- | --- |
| Project notes | Hands a project's notes to its Codex sessions when they start (Codex starts them with your first message) |
| Changed notes | Passes notes that changed since to a running Codex session, with your next message |
| Notes updates | Lets platoon keep a project's notes up to date after its Codex sessions stop |
| Tasks | Six hooks that let a task run in Codex: its questions and plans, the check of its work, and pushing that waits for you. In any Codex session that isn't a task's they end at once and do nothing |

Each is a fixed command, the same for every session (`<platoon> hook session-start --agent codex`, `hook guard --agent
codex`, `hook stop --agent codex`, and `hook task-start`, `task-before`, `task-notify`, `task-tool`, `task-prompt`,
`task-stop`, each with `--agent codex`). A file that is already there is backed up first (`hooks.json.platoon-bak`), and
everything else in it is left as it was. None of these steps is needed to start platoon, and if `hooks.json` can't be read setup says so and
leaves it alone. **If you ran setup before platoon could run tasks in Codex, run `platoon setup` again**: it adds the
task hooks beside the three it added then.

**Setup marks them trusted in Codex.** Codex runs a hook only once it is trusted. Running `platoon setup` is your
say-so for these, so setup asks Codex itself to trust them, the way Codex's own "Trust" does: it starts Codex's
app-server for a moment, and Codex works out each hook's fingerprint and writes the record in its own `config.toml`.
Only the hooks setup wrote are trusted, never another hook in the file or anywhere else. Each step's line says
whether Codex trusts it now; running setup again when they are trusted changes nothing.

If Codex can't be asked (it doesn't start, doesn't answer within 20 seconds, or answers something platoon doesn't
understand), setup says why, and Codex asks you once instead: the next time you start Codex it says the new hooks need
review. Approve them there. Until you do they simply don't run; nothing else in platoon waits for them, and the rest of
setup is unaffected.

After the checklist, setup asks one more question: may platoon use your Claude plan (on a computer with only Codex,
your ChatGPT plan through Codex) for its [smart features](tokens.md)? The answer is "no" unless you say yes; see
[Turning them on](tokens.md#turning-them-on).

### A computer with only Codex

Claude Code is installed when `claude` is on your path (or `PLATOON_CLAUDE_BIN` names it). Where it isn't, `platoon setup`
says so and leaves Claude Code's settings alone (it doesn't create `~/.claude/settings.json`), and `platoon` starts
without asking for Claude Code's steps: only Codex's hooks are set up, and they never block starting. The models offered
are Codex's own (the first one Codex lists is the default for the smart features and the notes), the header shows Codex's
plan, and the check for a newer Claude Code is skipped (the one for a newer Codex isn't). Tasks run in Codex: the built-in squads you haven't changed use
Codex's first model, for the session and for the check of its work, and the Squads tab says so ("GPT-6-Luna, since
Claude Code isn't installed"). A squad of your own that names one of Claude's models says it isn't installed.

With neither Claude Code nor Codex, `platoon` and `platoon setup` say plainly that there is nothing to manage and where to
get each, and change nothing.

With both installed, either one can be [switched off](settings-and-keys.md#switching-a-program-off) in platoon's settings
(General, Programs): platoon then treats it as not installed. `platoon setup` skips it and says so ("Codex is switched off
in platoon's settings, so setup skipped it"), leaving what it added there before as it is, and with Claude Code switched
off `platoon` starts without asking for Claude Code's steps.

### Running it again

Running `platoon setup` again is harmless: finished steps show as already done, and a step whose command points at a
platoon that no longer exists (you moved or reinstalled it) is repaired. When a newer platoon adds a step,
run `platoon setup` once more to add it.

If you run setup from a build folder (a path with `/target/` in it), it warns you: the hooks would point at a binary
that moves. Install platoon first, then run `platoon setup` again.

### The `/to-tickets` skill

Setup writes the [`/to-tickets`](tasks-and-squads.md#to-tickets-in-a-session) skill into Claude Code's folder
(`skills/to-tickets/SKILL.md`), naming platoon by its full path so the skill works whether or not `platoon` is on your
path. The file's last line says platoon wrote it, with a checksum of the rest. Running setup again updates platoon's
copy when a newer platoon ships a newer text (or platoon moved), and says "already done" otherwise.

A `/to-tickets` skill platoon didn't write (your own, one an older platoon installed, or platoon's copy once you edit
it) is never overwritten: setup leaves it as it is and says so in one line. To have platoon's instead, delete that
folder and run `platoon setup` again. The skill is not needed to start platoon, so an upgraded platoon starts even
before you run setup again.

So that you still hear about it, platoon says so once, in a line above the key hints of the Sessions and Projects
lists, when Claude Code is installed (and not switched off in the settings) and the skill is missing:

```
New: /to-tickets in Claude Code sessions — run `platoon setup` to add it
```

or when platoon's copy is older than the one this platoon would write (a newer platoon changed the skill, or platoon
moved):

```
platoon's /to-tickets skill has an update — run `platoon setup`
```

The line goes at your next key or click, and doesn't come back for the same reason; it goes by itself once setup has
put the skill in. A skill of yours by that name is your choice: platoon says nothing about it. When a version notice
(below) shows at the same time, this line is just above it.

Codex has skills too, but keeps a person's in `~/.agents/skills` (a folder shared with other tools), not in Codex's own
folder, so setup doesn't write one there yet. In a Codex session, `t` on its row makes tasks from it.

## When Claude Code, Codex or platoon is newer

platoon knows the Claude Code and Codex versions it was last checked with. When it starts, it asks your `claude` and
your `codex` for their versions (`claude --version`, `codex --version`; each only if it is installed), and if yours is
newer it says so once, in a line above the key hints:

```
Claude Code 2.2.0 is newer than this platoon was checked with (2.1.312). If something looks wrong, update platoon.
Codex 0.166.0 is newer than this platoon was checked with (0.165.0). If something looks wrong, update platoon.
```

When both are newer, it is one line that names both. `platoon setup` prints the same each time it runs.

Once a day at most, platoon also asks GitHub whether a newer platoon is out (see
[What platoon changes](what-platoon-changes.md#one-network-request-no-telemetry) for exactly what that sends). When one
is, it says so once, with the command that updates the way you installed it: `brew upgrade platoon`, `cargo install --locked
platoon-tui`, the install script again, or else a link to the releases page. Whatever applies goes in one line. Each
new version is said once; the line goes at your next key or click. "Check for a newer platoon" on the General tab of the
[settings](settings-and-keys.md#general) turns the GitHub check off, and `PLATOON_NO_UPDATE_CHECK=1` turns off every
check: platoon then runs neither `claude --version` nor `codex --version`.

## Uninstall

One command removes platoon completely, however you installed it:

```
curl -LsSf https://raw.githubusercontent.com/ctrader2/platoon/main/scripts/uninstall.sh | sh
```

To add a flag, put `-s --` before it: `curl -LsSf <the same address> | sh -s -- --all`. If you would rather not pipe a
script into your shell, download it, read it, then run `sh uninstall.sh`.

It first runs `platoon uninstall` (below), then removes the program the way it was installed:

- **Homebrew**: `brew uninstall platoon`.
- **cargo** (crates.io, or from source with `cargo install --path .`): `cargo uninstall platoon-tui`.
- **The install script**: the program where its note in `~/.config/platoon-tui/` says it put it, that folder, and the
  line it added to your shell's startup files (`.profile`, `.bashrc`, `.bash_profile`, `.bash_login`, `.zshrc`,
  `.zshenv`, and fish's `conf.d/platoon-tui.env.fish`). Only that line goes: a line another tool added stays.
- **Anything else** (Nix, a copy you put somewhere yourself): it says where the program is and how to delete it.

If `platoon uninstall` stops (you said no, or something couldn't be done), the script stops too and leaves the program
where it is, so you can run it again.

### `platoon uninstall`

`platoon uninstall` does everything but remove the program. It looks first, asks, and only then changes anything, so a
"no" leaves everything as it was.

1. **Sessions platoon is running.** If there are any, it lists them (name or folder, and process id) and asks:

   ```
   platoon is running 1 session:
     - a session in /home/you/app (pid 41235): `claude --resume 0f6a2c1e-7b4d-4c1a-9e55-3d2b8f1a6c90` opens it again
   Their conversations are saved: `claude --resume <id>` opens them again.
   Stop it and go on? [y/N]
   ```

   "No" (or just Enter) stops the uninstall with nothing changed. "Yes" stops them the way platoon itself does: Claude
   Code is asked to exit and shuts down cleanly.
2. **Your own data.** If the config folder is there, it asks:

   ```
   Also delete your projects, notes, tasks and settings? [y/N]
   ```

   The answer is "no" unless you say yes: the folder is kept, and the end says how to delete it later.
3. **Claude Code's settings.** It takes out of `settings.json` exactly what `platoon setup` added: the status line (if
   it is platoon's), and the session guard, project notes and notes updates hooks. It writes a backup first, as
   `settings.json.platoon-bak` beside it, and leaves everything else in the file as it was, your own hooks included. A
   hook event, or the whole `hooks` entry, goes only when platoon's hook was the last thing in it. A status line or
   hook of your own that also runs platoon is left alone, and the end names it so you can take that part out yourself.
   If `settings.json` can't be read, it stops before changing anything.
   **The `/to-tickets` skill.** It deletes the copy setup wrote (and its folder), only while it is unchanged; a skill of
   that name that platoon didn't write, or that you changed, stays, and the end says so.
   **Codex's hooks.** It takes the hooks setup added out of Codex's `hooks.json` the same way (backup first, as
   `hooks.json.platoon-bak`), and leaves everything else of Codex's alone, your own hooks included. Codex's records that
   those hooks are trusted go too, taken out through Codex itself as setup put them in (the records of any other
   hook stay), and so does Codex's trust in the folders of tasks' worktrees that platoon had it trust and that the tasks
   still record (only those: never a folder you trusted yourself); if Codex can't be asked, uninstall says what was
   left (it is harmless) and carries on. If `hooks.json` can't be read, it says so and leaves it as it is.
4. **Folders.** The cache folder (`~/.cache/platoon`) always goes; the config folder (`~/.config/platoon`) only if you
   said yes. Folders older platoons made are removed too, if they are there: `~/.local/share/platoon` and
   `~/.local/share/fonts/platoon` (on macOS also `~/Library/Application Support/platoon`). The same environment
   variables as everywhere else decide where these are (`PLATOON_CONFIG_DIR`, `PLATOON_CACHE_DIR`, `XDG_CONFIG_HOME`,
   `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `CLAUDE_CONFIG_DIR`).
5. **Task worktrees and branches** are never deleted: they may hold work you want. Each one still there is listed
   with the commands that remove it once you no longer need it, for example:

   ```
   Task worktrees and branches (never deleted: remove each once you no longer need its work):
     #3 (app):
       git -C /home/you/app worktree remove /home/you/app-worktrees/task-3
       git -C /home/you/app branch -D task/3
   ```

   `git worktree remove` refuses a worktree with changes not yet committed, so nothing is lost by accident.

It ends with what was removed and what was kept. Your conversations are never touched: they are Claude Code's (and
Codex's), and `claude --resume` (or `codex resume`) opens any of them. Run on its own (not by the script), its last line says how to remove the program.
Running it again is harmless: it says "Nothing of platoon's was left to remove."

| Flag | What it does |
| --- | --- |
| `--all` | Also deletes your projects, notes, tasks and settings, without asking |
| `--stop-sessions` | Stops the sessions platoon is running without asking. `--all` doesn't do this: it has to be said |
| `--help` | Says what it does and its flags |

Both the script and `platoon uninstall` take these flags. The questions are asked in your terminal even when the script
is piped into `sh`. With no terminal at all (a script, CI), nothing is asked: the config folder is kept, and if
sessions are running the uninstall stops with nothing changed unless you add `--stop-sessions`.

### Without platoon

If the `platoon` program is already gone (or is too old to have `uninstall`), the script still asks the same two
questions, stops the sessions and deletes the folders, and the `/to-tickets` skill when it is platoon's unchanged copy
(it checks the file's last line with `cksum`). It can't safely edit Claude Code's settings without platoon, so
it tells you what to take out by hand, as described in
[Removing platoon by hand](what-platoon-changes.md#removing-platoon-by-hand).
