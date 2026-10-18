# Settings and keys

## The settings screen

`s` opens the settings as a popup over the list; the [back key](#keys-back-and-quit) (`ctrl+space`), `esc`, `s`, `q`, or a
click outside it closes it. The settings are in six tabs along the top: **General**, **Search**, **Smart features**,
**Sessions**, **Projects** and **Editors** (on a narrow terminal "Smart features" is shortened to "Smart", and where even
then they don't all fit, the strip shows the tabs around the one showing, with `‹` or `›` on the side that has more; a
click on one goes there). `tab` goes to the next tab and `shift+tab` to the previous one (both go round), and a click on a
tab's name shows it. The popup opens on General, and keeps the same size on every tab; a tab with more entries than fit
scrolls, and on a small terminal only the selected entry is explained.

### General

| Setting | What it does | Choices |
| --- | --- | --- |
| Theme | The colors platoon is drawn in. `enter` on it opens a list that previews each theme on your real screen (see [Themes](themes.md)). | The [built-in themes](themes.md#the-built-in-themes) (Catppuccin Mocha by default) and your own |
| Stop idle sessions after | A session platoon is running that nobody has looked at, and that isn't working, is stopped to free memory. Nothing is lost: opening it again resumes it from its saved history. | Off, 30 minutes, 1 hour, 2 hours (default), 4 hours, 8 hours, 1 day |
| Archive sessions after | A session nobody has used for this long, with nothing running, moves to the Archived tab. You can restore it from there. | Off, 1 day, 2 days (default), 7 days, 14 days, 30 days |
| Idle stop and MCP servers | Whether the MCP servers of a session count as something running when deciding to stop it (see [Automatic clean-up](sessions.md#automatic-clean-up)). | Ignore MCP servers (default), Count MCP servers |
| Check for a newer platoon | Once a day at most, platoon asks GitHub for the number of its latest version and says once when a newer one is out (see [One network request](what-platoon-changes.md#one-network-request-no-telemetry)). Off: platoon makes no network request at all. Applies from the next start. | Once a day (default), Off |

Choose Count MCP servers if you'd rather a session with any program running under it, MCP servers included, was never
stopped: that is how idle stop worked before this choice existed.

### Search

| Setting | What it does | Choices |
| --- | --- | --- |
| Search archived | Whether the [Full-text search](sessions.md#search) also looks in archived conversations. | Include archived (default), Skip archived |
| Search results shown | The most conversations a full-text search lists. | 10, 20, 50 (default), 100, 200 |
| Search preview length | The most of a matching message a result shows around your words (it is also cut to fit the window). | 40, 80, 120 (default), 200, 400 characters |
| Longest message kept | How much of each message you sent is remembered for searching, so a huge paste doesn't fill the memory. Changing it makes platoon read your conversations again in the background. | 300, 1000, 2000 (default), 5000, 20000 characters |
| Description search: sessions sent / results / wider search | How many conversations a search by description ([Describe it](sessions.md#search)) sends, how many results it lists, how many the wider search sends. | 20-150 (50), 5-20 (10), 100-500 (200) |
| System prompt for Describe it | What platoon asks Claude for when you search by describing a session (see [System prompts](#system-prompts-for-each-smart-feature)). | Enter to change, User Modified |

### Smart features

Whether your Claude plan is used at all, and how much (see [Features that use tokens](tokens.md)). Under the entries:
`Used today: N of M tokens`, and, when naming is held up, why (`Naming is waiting: ...`).

| Setting | What it does | Choices |
| --- | --- | --- |
| Enable features which require tokens | The master switch for everything that uses your Claude plan. | Off (default), On |
| Smart names and summaries, Find a session by description, Project notes, Project descriptions | One switch per such feature. A feature runs only when the master switch and its own switch are both On. | Off / On (the two project ones are On once the master switch is) |
| Daily token limit | The most tokens those features may use in a day; at the limit they wait until tomorrow. | 100K, 250K, 500K (default), 1M, 2M, 5M tokens |
| Pause above plan usage | Background work stops asking Claude while your plan is used more than this much. (A search you ask for yourself ignores this.) | Never, 50%, 60%, 70%, 80% (default), 90% |
| Model for smart features | Which Claude model does the work. | Haiku (default), Sonnet |

### Sessions

What naming and summarizing your conversations reads, and how often. Under the entries:
`Archived: N conversations, about X tokens to index (an estimate)`.

| Setting | What it does | Choices |
| --- | --- | --- |
| Sessions per call | How many sessions one request to Claude covers. | 5, 10, 20, 30 (default), 50, 100 |
| Refresh a summary after | How many new substantive messages from you a conversation gets before its name and summary are made again. | 1, 3, 5 (default), 10, 20, 50 messages |
| Wait for quiet before refreshing | A conversation is refreshed only once nothing has been written to it for this long. | 15, 30 seconds, 1 (default), 2, 5, 10 minutes |
| Recent messages sent | How many of a conversation's latest messages go into its summary, besides the first one. | 3, 5, 8 (default), 12, 20 messages |
| Longest message sent | Each message is cut to this many characters before it is sent. | 100, 200, 300 (default), 500, 1000 characters |
| Archived sessions | Whether archived sessions are named too, slowly in the background. | Skip (default), In the background |
| Share for archived | Naming archived conversations may use at most this much of what is left of the day's limit. | 10, 25, 50 (default), 75, 100% |
| System prompt for naming | What platoon asks Claude for when it names and summarizes your sessions (see [System prompts](#system-prompts-for-each-smart-feature)). | Enter to change, User Modified |

### Projects

| Setting | What it does | Choices |
| --- | --- | --- |
| Update notes after | How long a project has to be quiet before platoon brings its [notes](projects.md#notes-that-keep-themselves-up-to-date) up to date. A new answer in any of its sessions starts the wait again. | 1, 3 (default), 5, 15, 30 minutes |
| Read Claude's last reply up to | How much of Claude's last answer in each session platoon reads when it brings a project's notes up to date. What is still open is usually in Claude's answers; more of them uses a little more of your plan. | 500, 1500 (default), 3000 characters |
| System prompt for project notes, for project descriptions, for the hand-off, for tasks from a conversation | How platoon asks Claude to write a project's notes, the line under a project's name, a hand-off, and the tasks it drafts from a session (`t`; see [Making tasks from a conversation](tasks-and-squads.md#making-tasks-from-a-conversation) and [System prompts](#system-prompts-for-each-smart-feature)). | Enter to change, User Modified |

### Editors

In two groups:

| Group | Setting | What it does | Choices |
| --- | --- | --- | --- |
| Editing platoon's text | Editor for notes and tasks (e) | The editor for a project's notes, a task's text and the system prompts (`e`): the one starred in the list of editors `e` shows. Choosing another there changes it too. | The [editors found](projects.md#editing-the-notes) on this computer |
| Editing files | Editor for folders (v) | The editor `v` opens a task's or a session's folders in. Choosing another from `v`'s list changes it too. | The [editors with a file tree](sessions.md#opening-the-editor-v) found on this computer; the first one found until you choose |
| Editing files | Always ask which editor | Off: `v` opens the editor above at once. On: `v` shows the list every time, with that one starred and picked. | Off (default), On |

### Moving around the settings

`↑` `↓` pick a setting on the tab showing (the popup opens on Theme); `←` `→` (or `h` `l`, `-` `+`) step through its
choices; `enter` goes to the next choice and wraps around to the first one (Off, or Count MCP servers). On Theme, `enter`
opens the theme list instead, and `←` `→` switch to the previous or next theme and save it at once. With the mouse: click a
row to select it, the `‹` `›` buttons to step, or the value itself to go to the next choice (on Theme, to open the list).

### Changes apply at once

There is no save step: each change is written to the file at once and applies without restarting anything, to platoon and
to sessions already running (a running session notices within about half a minute, and platoon within a couple of seconds;
auto-archive waits a moment after your last key press so that stepping past a value on the way to another one doesn't act
on it). If the file can't be written, the screen says so and keeps showing the value that is really in effect.

## The settings file

The settings live in `~/.config/platoon/settings.json` (`$PLATOON_CONFIG_DIR`, or `$XDG_CONFIG_HOME/platoon`; the screen
shows the exact path). platoon doesn't create it until you change something, and you can edit it by hand too:

```json
{"idle_stop_minutes": 120, "auto_archive_days": 2, "idle_stop_ignores_mcp": true, "theme": "catppuccin-mocha",
 "search_archived": true, "search_results": 50, "search_snippet_chars": 120, "search_prompt_chars": 2000,
 "tokens_enabled": false, "token_features": {"names": false, "find": false}, "ai_daily_tokens": 500000, "ai_pause_percent": 80,
 "ai_model": "haiku", "ai_batch_size": 30, "ai_refresh_messages": 5, "ai_quiet_seconds": 60, "ai_prompts_per_session": 8,
 "ai_prompt_chars": 300, "ai_archived": "off", "ai_archived_share": 50, "ai_find_candidates": 50, "ai_find_results": 10,
 "ai_find_wide_candidates": 200, "notes_editor": "ask", "files_editor": "", "files_editor_ask": false, "notes_quiet_minutes": 3,
 "notes_reply_chars": 1500, "update_check": true}
```

A value that isn't one of the steps (say `90` minutes) is shown as it is and left alone until you change that setting, when
it moves to the nearest step. Off is the same as `0` in the file. The limits:

- `idle_stop_minutes` up to 7 days, `auto_archive_days` up to 365 days.
- `idle_stop_ignores_mcp` and `search_archived` are `true` or `false`; anything else there means the default (`true`).
- The search numbers: 5 to 500 results, 30 to 400 preview characters and 200 to 20000 message characters.
- `theme` is one of the [theme](themes.md) ids (`user:...` for one of your own).
- The token numbers: 20,000 to 100,000,000 tokens a day (there is no "unlimited"), 10 to 100 percent (`0` means never
  pause), 1 to 100 conversations and 1 to 100 messages; `ai_model` is `haiku` or `sonnet`.
- `notes_editor` is the editor you used last, by its command (`code`, `nano`), or `ask` while you haven't used one; an
  editor this computer no longer has is simply not starred and stays in the file until you use another.
- `files_editor` is empty until you choose (the first editor found opens) or an editor's id (`code`, `cursor`, `zed`,
  `subl`, `kate`, `idea`, `idea-community`, `pycharm`, `pycharm-community`, `webstorm`, `goland`, `rustrover`, `clion`,
  `phpstorm`, `rider`, `android-studio`, `helix`, `nvim`), kept as it is when that editor isn't installed;
  `files_editor_ask` is `true` or `false`.
- `notes_quiet_minutes` is 1 to 60; `notes_reply_chars` is 200 to 5000.
- `update_check` is `true` or `false`; anything else there means the default (`true`).
- `ai_archived` is `"off"` or `"background"`; `ai_quiet_seconds` is 0 to 3600, `ai_prompts_per_session` 1 to 30,
  `ai_prompt_chars` 50 to 2000, `ai_archived_share` 5 to 100 percent, and the three description-search numbers 10 to 300,
  3 to 20 and 50 to 1000.

Anything else in the file is kept when platoon saves. A missing or damaged file just means the defaults, and a damaged one
is never replaced without keeping a `.bak` copy first.

## System prompts for each smart feature

Each thing platoon asks Claude for - names and summaries, Describe it, a project's notes, a project's description, the
hand-off and tasks from a conversation - comes with a system prompt,
and each has an entry on the tab it belongs to. It reads **Enter to change** while it is platoon's default, and **User
Modified** once you have changed it. Only the part that says what to aim for is yours to change: the tone, what to put in
and what to leave out. The exact shape of Claude's answer, which platoon checks, and the rule that what Claude is shown is
data and never orders, are kept by platoon and always sent after your text, so an edit can't break the replies.

- `enter` (or a click on the value) opens the same list of editors as a project's notes. The file starts as platoon's
  default; save your change and it is used from the next call. Leaving the default as it was, emptying the file, or making
  it the default again word for word all count as "not changed", and the file goes (without a message when nothing
  changed).
- `r` restores platoon's default, after asking. It is only offered (and shown in the footer) once you have changed that
  system prompt. Your version is kept as `<name>.md.bak` (one copy, replaced by the next restore).
- When a newer platoon changes the default of a system prompt you have changed, your text is kept and the entry says
  **User Modified · default updated** (in the warning color) until you edit it again or restore the default.

The files are `~/.config/platoon/prompts/<name>.md` (`names`, `describe-it`, `project-notes`, `project-description`,
`hand-off`, `tasks-from-conversation`), only once you have changed something, with
`built-in.json` beside them remembering which default each started from. Every part of platoon reads them, the notes helper
and `platoon setup`'s first naming pass included. A file that is missing, can't be read or is empty means platoon's
default, and at most the first 8,000 characters of one are sent.

## Keys: back and quit

One key goes **back one level everywhere**: `ctrl+space` by default. It leaves a session's live view (the session keeps
running), detaches `platoon open`, leaves a notes editor running inside platoon (after asking "Leave the editor?": `y` leaves
and ends it, anything else stays, since what it hasn't saved would be lost), goes from a task's screen to the Tasks tab, from a project's screen to the list, leaves the settings, the theme list and the changes view,
and cancels every question, box and list that pops up. `esc` does the same on platoon's own screens and popups (not in a
session or an editor, where it is theirs), so the key hints there say `esc back`; in a session's live view and the notes
editor they name the back key. On the top-level lists, with nothing open, it does what `esc` does there (clears a kept
filter) and never quits.

`q` quits, but only from the top-level lists (Sessions, Projects, Tasks, Squads with nothing open). On a project's or a
task's screen, or a squad opened from its tab, it goes back one level instead. In a box you type in, `q` is just a letter.

Both can be changed in `keys.json`, beside `settings.json` (`~/.config/platoon/keys.json`, or `$PLATOON_CONFIG_DIR`).
platoon never writes this file; there is no entry for it on the settings screen. For example:

```json
{ "back": "alt+b", "quit": "x" }
```

A key is written in lower case: `ctrl+`, `alt+` or `shift+`, then the key (`space`, `esc`, `enter`, `tab`, `f1` to `f12`,
`up`, `pageup`, ... or one character). The hints at the bottom of each screen show the keys in use. The rules:

- **`back`** must be `ctrl` with `space` or a letter, or `alt` with `space`, a letter or a digit. Anything else would be
  typed into Claude or the editor, or can't be told apart by the terminal: `ctrl+h`, `ctrl+i`, `ctrl+j`, `ctrl+m` and
  `ctrl+[` are what a terminal sends for backspace, tab, enter and esc. Ctrl and alt together, and shift with either, are
  refused too. A key Claude Code uses itself (`ctrl+c`, `ctrl+r`, `ctrl+o`, `alt+b`, ...) is accepted with a notice: in a
  session it then goes back instead of doing what Claude does with it.
- **`quit`** may be any key other than the back key; it only matters on the top-level lists.
- A missing file means the defaults. An entry that breaks the rules is left out with one notice at the bottom of the
  screen, saying which entry and which key is used instead; a file that isn't JSON means the defaults, also said once.
  Names platoon doesn't know are left alone. An edit applies at once, without restarting (`platoon open` reads the file as
  it starts).

## Environment variables

Each is explained where it matters; this is the list.

| Variable | What it does |
| --- | --- |
| `PLATOON_CONFIG_DIR` | platoon's config folder, instead of `$XDG_CONFIG_HOME/platoon` or `~/.config/platoon` |
| `PLATOON_CACHE_DIR` | platoon's cache folder, instead of `$XDG_CACHE_HOME/platoon` or `~/.cache/platoon` |
| `CLAUDE_CONFIG_DIR` | Claude Code's own: where platoon looks for Claude Code's folder, instead of `~/.claude` |
| `PLATOON_COLORS` | `truecolor` or `256`: see [Terminals without 24-bit color](themes.md#terminals-without-24-bit-color) |
| `PLATOON_BADGES` | `rounded` or `square`: the [count badges](sessions.md#keys) |
| `PLATOON_GLYPHS` | `plain` for `*` and `-` in place of `✦` and `•` (the default on the Linux console); `fancy` for the symbols |
| `PLATOON_CLIPBOARD` | `system`, `osc52` or `none`: see [Copying text from a session](sessions.md#copying-text-from-a-session) |
| `VISUAL`, `EDITOR` | looked at first for the [notes editor](projects.md#editing-the-notes) |
| `NO_COLOR` | set to anything, platoon draws without colors |
| `PLATOON_NO_UPDATE_CHECK` | `1` (anything but empty or `0`): no [version checks](install-and-setup.md#when-claude-code-or-platoon-is-newer) at all, whatever the setting says: no request to GitHub, and `claude --version` is not run |
