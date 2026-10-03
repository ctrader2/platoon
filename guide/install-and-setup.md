# Install and set up

## What you need

- Linux or macOS.
- Claude Code, installed as `claude` and signed in. platoon runs
  your sessions with it and reads its files (see [What platoon changes](what-platoon-changes.md)).
- Only to build it yourself (`cargo install`, or from source): Rust 1.88 or newer, with `cargo`.
- A terminal of at least 60 columns by 14 rows.
- `git`, for [tickets handed to squads](tickets-and-squads.md#handing-a-ticket-to-a-squad) (they work in git
  worktrees) and for the git facts in a [hand-off](projects.md#handing-work-on-to-a-new-session). Everything else works
  without it.

## Install

Any one of these. Each puts the `platoon` command on your path (the package is called `platoon-tui`); then run
[`platoon setup`](#set-it-up-platoon-setup) once.

| How | Install | Update |
| --- | --- | --- |
| Homebrew (macOS, Linux) | `brew install ctrader2/tap/platoon` | `brew upgrade platoon` |
| The install script (macOS, Linux) | `curl -LsSf https://github.com/ctrader2/platoon/releases/latest/download/platoon-tui-installer.sh \| sh` | the same command again |
| crates.io | `cargo install platoon-tui` | the same command again |
| Nix | `nix run github:ctrader2/platoon` (or add the flake to your configuration) | `nix run --refresh github:ctrader2/platoon` |
| From source | `git clone https://github.com/ctrader2/platoon && cd platoon && cargo install --path .` | `git pull`, then `cargo install --path .` |

- **The install script** downloads the release for your computer (Linux and macOS, Intel and ARM), checks its checksum
  when your system has the `sha256sum` command, and puts `platoon` in `~/.local/bin` (or `$XDG_BIN_HOME`), adding that
  folder to your path if it isn't there. It keeps a note of where it put platoon (and the lines that add the folder to
  your path) in `~/.config/platoon-tui/`. Each release on [GitHub](https://github.com/ctrader2/platoon/releases) also
  has the files themselves, with their checksums.
- The Linux builds are static: one file that runs on any distribution.
- `cargo install` and building from source need Rust 1.88 or newer.

Whichever you choose, platoon was last checked with the Claude Code version named in
[What platoon changes](what-platoon-changes.md#it-reads-claude-codes-own-files), and it
[tells you](#when-claude-code-or-platoon-is-newer) when your Claude Code is newer, or when a newer platoon is out.

## Set it up: `platoon setup`

`platoon setup` is a one-time step, and `platoon` won't start until it's done. It shows a checklist of what it changes in
Claude Code's `settings.json` (`~/.claude/settings.json`, or the one in `CLAUDE_CONFIG_DIR`). It backs that file up
first, as `settings.json.platoon-bak` beside it, and leaves everything else in it alone, your own hooks included.

| Step | Why |
| --- | --- |
| Status line | Plan usage (5-hour and weekly) and Claude's own context percentage |
| Session guard | Stops two copies of one conversation from overwriting each other, and keeps a [project's](projects.md) notes in each of its conversations while they run |
| Project notes | Hands a [project's](projects.md) notes to its sessions when they start or resume. Optional: platoon starts without it, projects still work, but their notes don't reach sessions |
| Notes updates | Lets platoon [keep a project's notes up to date](projects.md#notes-that-keep-themselves-up-to-date) after its sessions stop. Also optional |
| Tickets skill | Adds [`/to-tickets`](tickets-and-squads.md#making-tickets-from-a-conversation) to Claude Code, which turns a session into tickets. Optional. Unlike the others it is a file, not a setting (see [The tickets skill](#the-tickets-skill)) |

The status line and the session guard are the two platoon can't start without. If you already have a status line of
your own, setup leaves it alone and says so: make your own also run `platoon statusline` if you want plan usage in
platoon.

After the checklist, setup asks one more question: may platoon use your Claude plan for its
[smart features](tokens.md)? The answer is "no" unless you say yes; see
[Turning them on](tokens.md#turning-them-on).

### Running it again

Running `platoon setup` again is harmless: finished steps show as already done, and a step whose command points at a
platoon that no longer exists (you moved or reinstalled it) is repaired. When a newer platoon adds a step,
run `platoon setup` once more to add it.

If you run setup from a build folder (a path with `/target/` in it), it warns you: the hooks would point at a binary
that moves. Install platoon first, then run `platoon setup` again.

### The tickets skill

The Tickets skill step writes `skills/to-tickets/SKILL.md` in Claude Code's folder (`~/.claude`, or
`CLAUDE_CONFIG_DIR`). platoon remembers the copy it last wrote (in `setup.json` in its own config folder):

- When platoon's copy changes and yours is still the one it wrote, `platoon setup` updates it quietly.
- If you changed the file, setup leaves it alone and says "changed by you". `platoon setup --replace to-tickets` puts
  platoon's copy in (yours is kept as `SKILL.md.platoon-bak`).
- `platoon setup --remove to-tickets` takes it out and remembers that, so later runs leave it out (the line says
  "removed by you"). `platoon setup --add to-tickets` puts it back.

## When Claude Code or platoon is newer

platoon knows the Claude Code version it was last checked with. When it starts, it asks your `claude` for its version
(`claude --version`), and if yours is newer it says so once, in a line above the key hints:

```
Claude Code 2.2.0 is newer than this platoon was checked with (2.1.288). If something looks wrong, update platoon.
```

`platoon setup` prints the same line each time it runs.

Once a day at most, platoon also asks GitHub whether a newer platoon is out (see
[What platoon changes](what-platoon-changes.md#one-network-request-no-telemetry) for exactly what that sends). When one
is, it says so once, with the command that updates the way you installed it: `brew upgrade platoon`, `cargo install
platoon-tui`, the install script again, or else a link to the releases page. When both apply, it is one line. Each new
version is said once; the line goes at your next key or click. "Check for a newer platoon" on the General tab of the
[settings](settings-and-keys.md#general) turns the GitHub check off, and `PLATOON_NO_UPDATE_CHECK=1` turns off both checks.

## Undoing it

[What platoon changes on your computer](what-platoon-changes.md#removing-platoon) lists everything setup added and how to
take it out again.
