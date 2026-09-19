# platoon

A terminal app for managing Claude Code sessions (Linux and macOS): every conversation across every
directory in one list, with status, model, context use and plan usage. Open one (resuming it if nothing is
running it), start one, and archive the ones you're done with.

## Install

```
cargo install --path .
platoon setup
platoon
```

`platoon setup` is a one-time step, and `platoon` won't start until it's done. It shows a checklist of
what it changes in Claude Code's `settings.json`, and running it again is harmless: finished steps show as
already done, and a step whose command points at a platoon that no longer exists is repaired. It backs up
`settings.json` first and leaves everything else in it alone.

| Step | Why |
| --- | --- |
| Status line | Plan usage (5-hour and weekly) and Claude's own context percentage |
| Session guard | Stops two copies of one conversation from overwriting each other |

## The list

One row per conversation, built from Claude Code's saved history, so everything you've ever run is there
whether or not it's running. Each row shows the name, directory, branch, model, a status, how full the
context is (`~` marks platoon's estimate rather than Claude's own figure), when it was last active, and a
one-line description. Newest activity first.

Status is `working`, `done` (Claude finished and you haven't looked yet) or `idle`. Conversations that only
ever held slash commands are hidden.

| Key | Does |
| --- | --- |
| `enter` | open: shows it live if platoon is running it, otherwise resumes it from its saved history in its own directory |
| `n` | new session (pick a directory) |
| `a` | archive the selected conversation (on the Archived tab: restore it) |
| `1` `2` | switch between the Active and Archived tabs (or click their labels) |
| `/` | filter the current tab; `esc` clears it |
| `j` `k` `↑` `↓` `g` `G` | move; the mouse works too |
| `q` | quit (sessions keep running) |

### Archive

Archiving is platoon's own (Claude Code has no archive): it moves a conversation to the Archived tab and
stops it if something is running it (asking first only if it is in the middle of working). The conversation
itself is untouched, and `enter` on an archived one restores it and opens it. The list of archived ids lives
in `~/.config/platoon/archive.json` (`$PLATOON_CONFIG_DIR`, or `$XDG_CONFIG_HOME/platoon`).

### Big histories

platoon keeps an index of your transcripts in `~/.cache/platoon/index.json` and only re-reads files that
changed, in the background, so it opens at once even with thousands of conversations (a small
`indexing N…` shows in the header while it catches up). Deleting the file is harmless: it is rebuilt.

## Sessions started in platoon keep running

Each one runs under a small background host, so quitting platoon (or closing its terminal) never ends it.
`Ctrl+Space` leaves a session without stopping it.

## See a session live from a regular terminal

```
platoon open            # start a new session in this directory
platoon open a1b2       # join a session by (a prefix of) its id, resuming it if nothing is running
```

The session is shared, tmux-style: every window shows it live and any of them can type. The session is
sized to the smallest window attached. `Ctrl+Space` detaches; the session keeps running. A session running
in a plain `claude` platoon doesn't own can't be shared, so opening it asks before ending it and
reopening the same conversation from its saved history.
