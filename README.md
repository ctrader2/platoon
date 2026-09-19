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
| `1` `2` | switch between the Active and Archived tabs (or click them); the current tab is underlined and each shows a count badge |
| `/` | filter the current tab; `esc` clears it |
| `s` | [settings](#settings) |
| `↑` `↓` `g` `G` | move (`g`/`G` jump to the first/last); the mouse works too |
| `q` | quit (sessions keep running) |

Count badges are drawn with round ends in terminals that ship their own symbol glyphs (Ghostty, Kitty,
WezTerm) and as padded squares elsewhere, so nothing shows as an empty box. Set `PLATOON_BADGES=rounded` or
`PLATOON_BADGES=square` to choose yourself.

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
- The exception is MCP servers: the helper programs Claude starts with every session (the ones you added with
  `claude mcp add`, put in a project's `.mcp.json`, or that come with a plugin). Almost every real session has
  some, and they are closed when the session stops and started again when you open it, so by default they don't
  count as "something running". platoon works out which processes are MCP servers from your Claude Code
  configuration (`~/.claude.json`, or the one inside `$CLAUDE_CONFIG_DIR`; `.mcp.json` in the session's folder
  and its parent folders; enabled plugins), re-reading it at every check. A process only counts as one if
  Claude started it directly and its command matches a configured server (and everything a launcher such
  as `npx` or `docker run` starts under it is treated the same way). A dev server or job Claude started from a
  Bash tool is deeper than that, so it still keeps the session alive, and so does anything platoon is not
  sure about, or can't read the configuration to check. Some MCP servers don't exit when Claude does: after
  a stop, platoon asks the ones it saw before to exit too, and never any other process.
- Auto-archive never touches a conversation that has a process running, or the one you are looking at.
  Opening or restoring a conversation counts as activity, so restoring an old one sticks: it is not archived
  again until it has sat unused for the full time.
- Restoring is the same as always: the Archived tab, `a` (or `enter` to restore and open).
- Both are set on the [settings screen](#settings): press `s`.

### Settings

`s` opens the settings as a popup over the list; `esc`, `s`, `q`, or a click outside it closes it.

| Setting | What it does | Choices |
| --- | --- | --- |
| Stop idle sessions after | A session platoon is running that nobody has looked at, and that isn't working, is stopped to free memory. Nothing is lost: opening it again resumes it from its saved history. | Off, 30 minutes, 1 hour, 2 hours (default), 4 hours, 8 hours, 1 day |
| Archive conversations after | A conversation nobody has used for this long, with nothing running, moves to the Archived tab. You can restore it from there. | Off, 1 day, 2 days (default), 7 days, 14 days, 30 days |
| Idle stop and MCP servers | Whether the MCP servers of a session count as something running when deciding to stop it (see above). | Ignore MCP servers (default), Count MCP servers |

`↑` `↓` (or `j` `k`) pick a setting; `←` `→` (or `h` `l`, `-` `+`) step through its choices; `enter` goes to the
next choice and wraps around to the first one (Off, or Count MCP servers). With the mouse: click a row to select
it, the `‹` `›` buttons to step, or the value itself to go to the next choice. Off is the same as `0` in the file.
Choose Count MCP servers if you'd rather a session with any program running under it, MCP servers included, was
never stopped: that is how idle stop worked before this choice existed.

There is no save step: each change is written to the file at once and applies without restarting anything,
to platoon and to sessions already running (a running session notices within about half a minute, and
platoon within a couple of seconds; auto-archive waits a moment after your last key press so that stepping
past a value on the way to another one doesn't act on it). If the file can't be written, the screen says so
and keeps showing the value that is really in effect.

The settings live in `~/.config/platoon/settings.json` (`$PLATOON_CONFIG_DIR`, or `$XDG_CONFIG_HOME/platoon`;
the screen shows the exact path). platoon doesn't create it until you change something, and you can edit it
by hand too:

```json
{"idle_stop_minutes": 120, "auto_archive_days": 2, "idle_stop_ignores_mcp": true}
```

A value that isn't one of the steps (say `90` minutes) is shown as it is and left alone until you change that
setting, when it moves to the nearest step. Limits are 7 days (`idle_stop_minutes`) and 365 days
(`auto_archive_days`). `idle_stop_ignores_mcp` is `true` or `false`; anything else there means the default
(`true`). Anything else in the file is kept when platoon saves. A missing or damaged file just means the defaults, and a damaged one is never replaced without keeping a `.bak` copy first.

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
