# Install and set up

## What you need

- Linux or macOS.
- Claude Code, installed as `claude` and signed in. platoon runs
  your sessions with it and reads its files (see [What platoon changes](what-platoon-changes.md)).
- Rust 1.88 or newer, with `cargo`, to build platoon.
- A terminal of at least 60 columns by 14 rows.
- `git`, for [tickets handed to squads](tickets-and-squads.md#handing-a-ticket-to-a-squad) (they work in git
  worktrees) and for the git facts in a [hand-off](projects.md#handing-work-on-to-a-new-session). Everything else works
  without it.

## Install

For now platoon is installed from source:

```
cargo install --git https://github.com/ctrader2/platoon
platoon setup
platoon
```

The command is `platoon` (the package is called `platoon-tui`).

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
| Tickets skill | Adds [`/to-tickets`](tickets-and-squads.md#making-tickets-from-a-conversation) to Claude Code, which turns a conversation into tickets. Optional. Unlike the others it is a file, not a setting (see [The tickets skill](#the-tickets-skill)) |

The status line and the session guard are the two platoon can't start without. If you already have a status line of
your own, setup leaves it alone and says so: make your own also run `platoon statusline` if you want plan usage in
platoon.

After the checklist, setup asks one more question: may platoon use your Claude plan for its
[smart features](tokens.md)? The answer is "no" unless you say yes; see
[Turning them on](tokens.md#turning-them-on).

### Running it again

Running `platoon setup` again is harmless: finished steps show as already done, and a step whose command points at a
platoon that no longer exists (you moved or reinstalled it) is repaired. If you set platoon up before a step existed
(an older platoon had no Project notes or Notes updates step), run `platoon setup` once more to add it.

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

## Undoing it

[What platoon changes on your computer](what-platoon-changes.md#removing-platoon) lists everything setup added and how to
take it out again.
