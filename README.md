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
stops it if something is running it. It asks first if the conversation is in the middle of working, or if it
is running in a terminal platoon didn't start (that copy isn't platoon's to end unasked). The conversation
itself is untouched, and `enter` on an archived one restores it and opens it. The list of archived ids lives
in `~/.config/platoon/archive.json` (`$PLATOON_CONFIG_DIR`, or `$XDG_CONFIG_HOME/platoon`).

### Automatic clean-up

Two things happen on their own, so you don't have to think about them. Nothing is ever deleted.

| What | When | What you see |
| --- | --- | --- |
| A session platoon runs is stopped | nobody has had it open, it hasn't been working, and it has printed nothing, for 2 hours | nothing: the row stays in the list like any conversation, and `enter` resumes it from its saved history |
| A conversation is archived | nothing is running it and it has had no activity for 2 days | a one-line notice (`Auto-archived 143 conversations idle for 2+ days. Find them under Archived.`) that stays until you press a key; the first time you run this version, many old conversations can be archived at once |

Details worth knowing:

- A session is only stopped when Claude itself says it is idle. If anything is running under it (a dev
  server, a background job) it is left alone, because stopping Claude would end those too. Sessions started
  in a plain terminal, and anything with a window open on it, are never stopped.
- Auto-archive never touches a conversation that has a process running, or the one you are looking at.
  Opening or restoring a conversation counts as activity, so restoring an old one sticks: it is not archived
  again until it has sat unused for the full time.
- Restoring is the same as always: the Archived tab, `a` (or `enter` to restore and open).
- Both times are in `~/.config/platoon/settings.json` (`$PLATOON_CONFIG_DIR`, or `$XDG_CONFIG_HOME/platoon`),
  which platoon never creates by itself. A settings screen is planned; until then, edit the file:

  ```json
  {"idle_stop_minutes": 120, "auto_archive_days": 2}
  ```

  `0` turns that clean-up off. Changes apply within about half a minute, to platoon and to sessions already running,
  with no restart. A missing or damaged file just means the defaults (a damaged one is never overwritten
  without keeping a `.bak` copy). Values are limited to 7 days (`idle_stop_minutes`) and 365 days
  (`auto_archive_days`).

### Big histories

platoon keeps an index of your transcripts in `~/.cache/platoon/index.json` and only re-reads files that
changed, in the background, so it opens at once even with thousands of conversations (a small
`indexing N…` shows in the header while it catches up). Deleting the file is harmless: it is rebuilt.

## Sessions started in platoon keep running

Each one runs under a small background host, so quitting platoon (or closing its terminal) never ends it.
`Ctrl+Space` leaves a session without stopping it. A session nobody has looked at for a while does stop
itself (see [Automatic clean-up](#automatic-clean-up)); `enter` picks the conversation up again.

## See a session live from a regular terminal

```
platoon open            # start a new session in this directory
platoon open a1b2       # join a session by (a prefix of) its id, resuming it if nothing is running
```

The session is shared, tmux-style: every window shows it live and any of them can type. The session is
sized to the smallest window attached. `Ctrl+Space` detaches; the session keeps running. A session running
in a plain `claude` platoon doesn't own can't be shared, so opening it asks before ending it and
reopening the same conversation from its saved history.
