# platoon

A terminal app for managing Claude Code sessions (Linux and macOS): every conversation across every
directory in one list, with status, model, context use and plan usage. Open one (resuming it if nothing is
running it), start one, and archive the ones you're done with. Group conversations into [projects](#projects) that
span as many folders as you like, each with its own notes.

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
| Project notes | Hands a [project's](#projects) notes to its sessions when they start. Optional: platoon starts without it, projects still work, but their notes don't reach sessions. If you set platoon up before projects existed, run `platoon setup` once more to add it |

## The list

The header has two tabs, **Sessions** and **Projects** (`1` and `2`, or click them), each with how many active ones
it holds. Under it, each tab has **Active** and **Archived**: the left and right arrow keys switch them (or click them).

One row per conversation, built from Claude Code's saved history, so everything you've ever run is there
whether or not it's running. Each row shows the name, directory, branch, model, a status, how full the
context is (`~` marks platoon's estimate rather than Claude's own figure), when it was last active, and a
one-line description. Newest activity first.

Status is `working`, `done` (Claude finished and you haven't looked yet), `idle`, or `Background` (see
[Background sessions](#background-sessions)). Conversations that only ever held slash commands are hidden.

The name shown is, in this order: the one you gave it with `r` (platoon's own), then one you typed with `/rename` inside
Claude, then, when the [smart features](#features-that-use-tokens) are on, a short name platoon made up (marked with a quiet `✦`
at the end of the name, or `*` on the Linux console or when `PLATOON_GLYPHS=plain`, so you can tell it from one you chose), and
otherwise the folder's name. The folder always stays visible in the DIR column: when the path is too long for it, the start is
cut and the end (the folder itself) kept. With a summary made, the selected row's summary shows in two lines just above the
key hints (left out on a terminal too short to spare them).

| Key | Does |
| --- | --- |
| `enter` | open: shows it live if platoon is running it, otherwise resumes it from its saved history in its own directory; a `Background` one is opened with `claude attach` |
| `n` | new session (pick a directory) |
| `a` | archive the selected conversation (on the Archived tab: restore it) |
| `m` | put the selected conversation in a [project](#projects), move it to another, or take it out ("No project") |
| `1` `2` | switch between the Sessions and Projects tabs (or click them); the current tab is underlined and each shows a count badge |
| `←` `→` | switch between Active and Archived |
| `/` | [search](#search): narrow the list as you type, or find a conversation by the words you typed in it |
| `r` | [rename](#renaming) the selected conversation |
| `s` | [settings](#settings) |
| `↑` `↓` `g` `G` | move (`g`/`G` jump to the first/last); the mouse works too |
| `q` | quit (sessions keep running) |

Count badges are drawn with round ends in terminals that ship their own symbol glyphs (Ghostty, Kitty,
WezTerm) and as padded squares elsewhere, so nothing shows as an empty box. Set `PLATOON_BADGES=rounded` or
`PLATOON_BADGES=square` to choose yourself.

### Search

`/` opens a search dialog over the list. It has two kinds of search, and a third when the [smart features](#features-that-use-tokens)
are on; `tab` (or a click on its name) switches between them and keeps what you typed.

- **Quick filter** (the one it opens on): narrows the list as you type, by name, folder, branch and description. `enter` keeps the
  filter and closes the dialog; `esc` clears it and closes. (In the list, `esc` also clears a kept filter.) The dialog lists nothing
  until you type: with an empty box the results area is empty (the list behind still shows everything, and `enter` just closes the
  dialog), and the narrowed rows appear with the first character and go away again if you delete it.
- **Full-text**: finds conversations by the words you typed in them, in every conversation, archived ones too. Every word must be
  there, in any order and in any case, and a word matches inside a longer one (`migrat` finds `migration`). Words spread over
  different messages of the same conversation count. Each result shows the conversation's name and, under it, the matching message
  with your words in the accent color. `↑` `↓` (or `PgUp` `PgDn`, or the wheel) pick a result and `enter` (or a second click)
  closes the dialog and selects that conversation in the list, on the Archived tab if that is where it is. Only what you typed is
  searched: Claude's answers and the output of tools are not.

- **Describe it** (only while "Enable features which require tokens" and "Find a session by description" are both on): for when you
  remember what a conversation was about but not the words. Type a few words about it and press `enter`: platoon picks about 50
  conversations that share words with your description (their names, summaries and what you typed; topped up with the most recently
  used when fewer than 10 do), sends Claude one short list of them and asks for the best matches, and shows up to 10 with a
  one-line reason each. Nothing is sent while you type, only when you press `enter`. `esc` calls the request off (and a second `esc`
  closes the dialog); `enter` on a result opens it as in Full-text; `ctrl+w` searches wider (about 200 conversations) after
  telling you roughly how many tokens that is and waiting for `enter`. It is your own request, so it goes ahead even when the plan is
  used above the pause point that holds the background naming back; it still needs the switches, the daily limit and no back-off,
  and says plainly if one of them stops it. Words in generated names and summaries are found by the Quick filter and by Full-text too.

Results come best first: conversations whose name has all the words, then those where one single message has all of them, then
those where more messages mention them, then the most recently used. `ctrl+u` empties the box.

Behind the scenes platoon keeps a small list of the messages you sent in `~/.cache/platoon/prompts.jsonl`
(`$PLATOON_CACHE_DIR`). It reads only what is new at the end of each transcript, in the background, so a search never waits
for it and starting platoon is not slower; the first time it reads everything once, and while it does the dialog says "Still
reading your conversations…" if nothing is found yet. Deleting the file is harmless: it is rebuilt. Nothing here uses Claude or
any tokens.

### Renaming

`r` on a conversation opens a small box with its current name: `enter` saves, `esc` cancels, and an empty name gives the automatic one
back. A name you give is kept in platoon's own file, `~/.config/platoon/names.json` (`$PLATOON_CONFIG_DIR`), never in Claude's files,
and other things in that file are left alone when platoon writes it. It shows in the NAME column and is found by both searches.

### Background sessions

Claude Code can run a session in the background on its own (`claude --bg`, `claude agents`). Platoon reads
those from Claude's registry (`~/.claude/sessions/*.json`, `"kind":"bg"`) and shows the row with the status
`Background` (on the Active tab) instead of `idle`. `enter` (or `platoon open <id>`) opens it with
`claude attach <id>` inside a platoon session window, never `claude --resume`, which Claude refuses for a
session it is running. Leaving the window or quitting platoon ends only that window: the background session keeps
running. Platoon never stops or archives one: the `a` key says so and does nothing, auto-archive skips it, and idle
stop never applies to it. To end one yourself use Claude's own `claude stop <id>`. A background session is often a
copy of another conversation, so you can see two rows with the same title; platoon doesn't try to tell them apart.

### Archive

Archiving is platoon's own (Claude Code has no archive): it moves a conversation to the Archived tab and
stops it if something is running it. It asks first if the conversation is in the middle of working, or if it
is running in a terminal platoon didn't start (that copy isn't platoon's to end unasked). The conversation
itself is untouched, and `enter` on an archived one restores it and opens it. The list of archived ids lives
in `~/.config/platoon/archive.json` (`$PLATOON_CONFIG_DIR`, or `$XDG_CONFIG_HOME/platoon`).

Stopping a running conversation takes Claude about a second, and platoon doesn't make you wait for it: the row
leaves the list (and the Active count drops) the moment you press the key or answer the question, and the stop
runs in the background. The conversation shows up under Archived once the process has really exited, so a
conversation is never archived while it is still running. If it hasn't exited after about 5 seconds, the row comes
back with a notice (`Couldn't stop "name": ... Try again in a moment.`) and nothing is archived. While a stop is
under way you can carry on as usual (open, rename, filter, restore or archive other conversations). Quitting
platoon then waits up to 3 seconds for it, so the archive gets written, and leaves anyway after that; a
conversation whose process is still running at that point stays in the list.

### Automatic clean-up

Two things happen on their own, so you don't have to think about them. Nothing is ever deleted.

| What | When | What you see |
| --- | --- | --- |
| A session platoon runs is stopped | nobody has had it open, it hasn't been working, and it has printed nothing, for 2 hours | nothing: the row stays in the list like any conversation, and `enter` resumes it from its saved history |
| A conversation is archived | nothing is running it (a `Background` session counts as running) and it has had no activity for 2 days | a one-line notice (`Auto-archived 143 conversations idle for 2+ days. Find them under Archived.`) that stays until you press a key; the first time you run this version, many old conversations can be archived at once |

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
| Theme | The colors platoon is drawn in. `enter` on it opens a list that previews each theme on your real screen (see [Themes](#themes)). | The [built-in themes](#themes) (Catppuccin Mocha by default) and your own |
| Stop idle sessions after | A session platoon is running that nobody has looked at, and that isn't working, is stopped to free memory. Nothing is lost: opening it again resumes it from its saved history. | Off, 30 minutes, 1 hour, 2 hours (default), 4 hours, 8 hours, 1 day |
| Archive conversations after | A conversation nobody has used for this long, with nothing running, moves to the Archived tab. You can restore it from there. | Off, 1 day, 2 days (default), 7 days, 14 days, 30 days |
| Idle stop and MCP servers | Whether the MCP servers of a session count as something running when deciding to stop it (see above). | Ignore MCP servers (default), Count MCP servers |
| Search archived | Whether the [Full-text search](#search) also looks in archived conversations. | Include archived (default), Skip archived |
| Search results shown | The most conversations a full-text search lists. | 10, 20, 50 (default), 100, 200 |
| Search preview length | The most of a matching message a result shows around your words (it is also cut to fit the window). | 40, 80, 120 (default), 200, 400 characters |
| Longest message kept | How much of each message you sent is remembered for searching, so a huge paste doesn't fill the memory. Changing it makes platoon read your conversations again in the background. | 300, 1000, 2000 (default), 5000, 20000 characters |
| Smart features | Opens a screen of its own (below) with everything that uses your Claude plan. The row shows the master switch: Off or On. | `enter` (or `→`, or a click) opens it |

`↑` `↓` pick a setting (the popup opens on Theme); `←` `→` (or `h` `l`, `-` `+`) step through its choices; `enter` goes to the
next choice and wraps around to the first one (Off, or Count MCP servers). On Theme, `enter` opens the theme list
instead, and `←` `→` switch to the previous or next theme and save it at once. With the mouse: click a row to select
it, the `‹` `›` buttons to step, or the value itself to go to the next choice (on Theme, to open the list). Off is the same as `0` in the file.
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
{"idle_stop_minutes": 120, "auto_archive_days": 2, "idle_stop_ignores_mcp": true, "theme": "catppuccin-mocha",
 "search_archived": true, "search_results": 50, "search_snippet_chars": 120, "search_prompt_chars": 2000,
 "tokens_enabled": false, "token_features": {"names": false, "find": false}, "ai_daily_tokens": 500000, "ai_pause_percent": 80,
 "ai_model": "haiku", "ai_batch_size": 30, "ai_refresh_messages": 5, "ai_quiet_seconds": 60, "ai_prompts_per_session": 8,
 "ai_prompt_chars": 300, "ai_archived": "off", "ai_archived_share": 50, "ai_find_candidates": 50, "ai_find_results": 10,
 "ai_find_wide_candidates": 200}
```

A value that isn't one of the steps (say `90` minutes) is shown as it is and left alone until you change that
setting, when it moves to the nearest step. Limits are 7 days (`idle_stop_minutes`) and 365 days
(`auto_archive_days`). `idle_stop_ignores_mcp` and `search_archived` are `true` or `false`; anything else there means the default
(`true`). The search numbers are limited to 5 to 500 results, 30 to 400 preview characters and 200 to 20000 message characters. `theme` is one of the [theme](#themes) ids (`user:...` for one of your own). The token numbers are limited to 20,000 to 100,000,000 tokens a day (there is no "unlimited"), 10 to 100 percent (`0` means never pause), 1 to 100 conversations and 1 to 100 messages; `ai_model` is `haiku` or `sonnet`; `ai_archived` is `"off"` or `"background"`; `ai_quiet_seconds` is 0 to 3600, `ai_prompts_per_session` 1 to 30, `ai_prompt_chars` 50 to 2000, `ai_archived_share` 5 to 100 percent, and the three description-search numbers 10 to 300, 3 to 20 and 50 to 1000. Anything else in the file is kept when platoon saves. A missing or damaged file just means the defaults, and a damaged one is never replaced without keeping a `.bak` copy first.

**Smart features** (`enter` on that row; `esc` goes back to the settings) holds every setting for the token features, so the main
popup stays short. The screen keeps the same rules: it scrolls on a short terminal (80x24 shows a window of it), explains the selected
entry, and has under the entries `Used today: N of M tokens` and `Archived: N conversations, about X tokens to index (an estimate)`.

| Setting | What it does | Choices |
| --- | --- | --- |
| Enable features which require tokens | The master switch for everything that uses your Claude plan (see [Features that use tokens](#features-that-use-tokens)). | Off (default), On |
| Smart names and summaries, Find a session by description | One switch per such feature. A feature runs only when the master switch and its own switch are both On. | Off (default), On |
| Daily token limit | The most tokens those features may use in a day; at the limit they wait until tomorrow. | 100K, 250K, 500K (default), 1M, 2M, 5M tokens |
| Pause above plan usage | Background work stops asking Claude while your plan is used more than this much. (A search you ask for yourself ignores this.) | Never, 50%, 60%, 70%, 80% (default), 90% |
| Model for smart features | Which Claude model does the work. | Haiku (default), Sonnet |
| Conversations per call | How many conversations one request to Claude covers. | 5, 10, 20, 30 (default), 50, 100 |
| Refresh a summary after | How many new substantive messages from you a conversation gets before its name and summary are made again. | 1, 3, 5 (default), 10, 20, 50 messages |
| Wait for quiet before refreshing | A conversation is refreshed only once nothing has been written to it for this long. | 15, 30 seconds, 1 (default), 2, 5, 10 minutes |
| Recent messages sent | How many of a conversation's latest messages go into its summary, besides the first one. | 3, 5, 8 (default), 12, 20 messages |
| Longest message sent | Each message is cut to this many characters before it is sent. | 100, 200, 300 (default), 500, 1000 characters |
| Archived conversations | Whether archived conversations are named too, slowly in the background. | Skip (default), In the background |
| Share for archived | Naming archived conversations may use at most this much of what is left of the day's limit. | 10, 25, 50 (default), 75, 100% |
| Description search: conversations sent / results / wider search | How many conversations a search by description sends, how many results it lists, how many the wider search sends. | 20-150 (50), 5-20 (10), 100-500 (200) |

### Features that use tokens

Two features ask Claude, which uses your own Claude plan. They are **off by default** and switched on by **Enable features which
require tokens** in the settings (under **Smart features**) plus the switch of each feature:

- **Smart names and summaries**: gives each conversation a short name and a one- or two-sentence summary (shown under the list for
  the selected row, found by the search), and keeps them up to date as the conversation goes on.
- **Find a session by description**: the [Describe it](#search) search.

`platoon setup` asks once, in a terminal, with the default "no". It lists the features and says that platoon would send Claude the
prompts you typed plus one line of Claude's last reply for each conversation, in small batches, and that you can switch it off any
time (`s`, then Smart features, then the master switch). Without a terminal setup asks nothing, leaves everything off and prints one
line saying how to turn them on. Turning a switch off stops a call already in progress and throws its answer away; names and
summaries already saved stay.

**The first pass, in `platoon setup`.** After a yes (and also when you said yes earlier and nothing has been named yet), setup shows
a local estimate ("3 active conversations to name, about 2,100 tokens, roughly 1 call (an estimate, made on this computer)") and
names your active conversations right there, newest first, with a one-line progress display. `Ctrl+C` skips it: platoon carries on
in the background while it is open. Setup then asks about archived conversations, showing how many there are and what naming them
would roughly cost, with "skip" as the default and "slowly in the background" as the other choice (the setting `Archived
conversations`). Without a terminal there is no scan, just one line. None of this ever changes what setup does or its exit code, or
stops platoon from starting.

**What is sent.** For each conversation: its first message, its last few messages (8 by default, each cut to 300 characters), the
name and summary platoon made last time (if any) and one line (200 characters) of Claude's last reply. The conversations are
numbered 1, 2, 3 in the request (never their ids or folders), up to 30 to a call. Never tool output, file contents, or anything
else. The description search sends, for about 50 conversations, the name, the folder's own name and the summary (or, for a
conversation with none, a little of its first and last message) and asks for the best matches. Every request tells the model that
the text is data to read, not instructions, and to answer with JSON only; what comes back is checked strictly (unknown numbers,
wrong types and empty text are ignored, names are cut to 60 characters and summaries to 300) and every control character and escape
sequence is removed before it is stored or drawn. A conversation the model skips is tried in a later batch at most twice, then left
until it changes.

**When.** The naming work runs only **while the platoon window is open**, on a background worker inside platoon: nothing keeps
running when you quit (the hosts that keep your sessions alive do no naming), and it catches up the next time platoon starts. One
call at a time, in this order: a description search you asked for, conversations that hit the refresh trigger (5 new substantive
messages, i.e. not "yes", "continue", "ok", "go ahead", "thanks" or anything under 10 characters, and quiet for 60 seconds), a
conversation you just took out of the archive or opened, first-time names for active conversations (newest first), then archived
ones if you chose that. Archived conversations may use at most half (a setting) of what is left of the day's limit, so the
conversations you are using always have room. The header shows `✦ Indexing 12 of 240` while there is work.

**Where it is kept.** Names and summaries are in `~/.config/platoon/summaries.json` (`$PLATOON_CONFIG_DIR`): per conversation the
name, the summary, how many messages it covers, the model and the time, never a message you typed or a reply from Claude. It is
written atomically, kept as it was if platoon can't understand it (a `.bak` copy first), other things in it are left alone, and it is
in the config folder (not the cache) because making it again costs tokens. platoon never writes to Claude's own files.

What keeps it safe, all enforced in one place before any call starts:

- **Daily limit** (default 500K tokens; every number Claude reports counts, input, cache writes, cache reads and output). The day is your local calendar day.
  Every call is added to `~/.cache/platoon/ai-usage.json` (numbers only, kept for 30 days; nothing you typed or Claude answered is ever saved there).
- **Pause above plan usage** (default 80%), using the figure Claude Code's status line reports, for background work. With no figure yet, nothing pauses.
- **Back off after a failure**: not signed in, plan limit reached, no connection, too slow, or any other error stops calls for a while (30 seconds doubling up
  to 30 minutes; 15 or 30 minutes doubling up to 2 hours when signed out or over the plan limit). A success ends it. The footer says
  `Smart features paused: <reason>` once, not once per failed call, and the worker looks again every 30 seconds without repeating itself.
- **Small calls**: `claude -p` with your normal login, no tools, no settings, no saved session, a short system prompt and low effort, run in
  `~/.cache/platoon/ai-work` (Claude makes one empty folder under `~/.claude/projects` for it, and only one), with `PLATOON_AI=1` set.
  `MAX_THINKING_TOKENS=0` is set too, so the model doesn't spend hidden "thinking" tokens on a job that only needs a short answer (about
  half the cost of a naming call). The helper Claude process that runs there is platoon's own: it is never shown as a conversation (not in
  the list, the counts, search or naming, whatever it is doing).

`platoon tokens` shows whether the features are on and today's total. `platoon tokens test` makes one tiny call the same way a feature would
(master switch on, the daily limit, the plan pause and back-off all apply) and prints the reply, the tokens, the cost Claude reports and today's total; if a
gate refuses it says why and how to switch it on.

### Themes

platoon ships with dark color themes: Catppuccin Mocha (the default), Catppuccin Macchiato, Tokyo Night, Dracula,
One Dark, Nord, Gruvbox Dark, Rosé Pine, Kanagawa Wave, Night Owl, Ayu Mirage, Vesper, Claude (Claude Code's own
colors) and Terminal colors. `platoon themes` lists them with a swatch of each and their ids.

**Picking one.** Press `s` (the settings open on the Theme row) and `enter`. A list of themes opens on the right,
next to your real screen, each with a strip of its main colors and a check on the one you use now. Move with `↑` `↓`
(`Home` `End` or `g` `G` jump to the first and last): the whole app is redrawn in the highlighted theme at once, and a
small sample in the panel shows every color it uses even if you have no conversations yet. `enter` keeps the
highlighted theme (it is saved), `esc` puts your old theme back. With the mouse: hover highlights an entry, a click
previews it, a click on the previewed one (or on `enter use`) keeps it, the wheel moves the highlight, and a click
outside the panel puts the old theme back. On the Theme row of the settings, `←` `→` skip the list: they switch to the
previous or next theme and save it straight away.

You can also set it by hand with `theme` in `settings.json`:

```json
{"theme": "tokyo-night"}
```

A running platoon picks the change up within a couple of seconds. A theme id platoon doesn't know shows the default
and is left in the file as it is. The theme also recolors the Claude session you view inside platoon (its 16
terminal colors become the theme's); `platoon open` in a regular terminal is unaffected.

**Your own themes.** platoon reads extra themes from the `themes` folder next to `settings.json`:
`~/.config/platoon/themes/` (`$PLATOON_CONFIG_DIR/themes`, or `$XDG_CONFIG_HOME/platoon/themes`). It uses
[Ghostty](https://ghostty.org)'s theme file format, one theme per file, and the file's name is the theme's name
(`~/.config/platoon/themes/Deep Sea` is "Deep Sea"):

```
background = #0a1622
foreground = #dde6ee
palette = 12=#4fb0ff
```

That is all it needs: the background, the foreground, and any of the 16 `palette = N=#RRGGBB` colors (others are
worked out; the other keys Ghostty themes carry are ignored). Ghostty's own themes are in this format, so borrowing one
is a copy, for example `mkdir -p ~/.config/platoon/themes && cp /usr/share/ghostty/themes/Nord ~/.config/platoon/themes/`. Your themes appear in
the list after the built-ins, marked `yours`, with the id `user:<file name>` (`user:deep-sea`) that goes in
`settings.json`. Things to know:

- Dark themes only. A file whose background is light is skipped, and so is anything that isn't a readable theme file
  (too big, not text, no background or foreground). The picker says how many were skipped and why, and so does
  `platoon themes`.
- Contrast is adjusted for you. Text, the muted grays, the accent and the status colors are lightened as far as it
  takes to read on your background (and kept clearly different from each other), so a sloppy file still gives a usable
  theme. What the file says is used as it is wherever it already reads well.
- A file named like a built-in (a copy of Nord, say) is its own theme, shown as `Nord (yours)`; two files whose names
  differ only in capitals count as one (the first, in name order, is used). `platoon themes` prints every id.
- Changes to the folder are noticed within a couple of seconds; nothing needs restarting. If you pick a user theme and
  later delete its file, platoon shows the default theme and keeps the id in `settings.json`, so putting the file back
  brings your theme back.
- A folder with more than 300 files is read only as far as its first 300 (in name order); subfolders and names starting
  with a dot are ignored.

`terminal` (Terminal colors) draws with your terminal's own default colors and 16-color palette, so platoon follows
whatever theme your terminal has. It relies on your terminal's palette having a black that differs from the
background and a bright black that reads as grey; if your terminal theme's black is the same as its background, the
highlighted row is shown only by the marker beside it.

Themes use 24-bit color. Terminals that don't support it (macOS Terminal.app, some tmux setups) get the nearest of
the 256 xterm colors instead, worked out for each cell as it is drawn. platoon decides from `COLORTERM`
(`truecolor` or `24bit`) and a list of terminals known to support it; set `PLATOON_COLORS=truecolor` or
`PLATOON_COLORS=256` to choose yourself. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for where the palettes
come from.

### Big histories

platoon keeps an index of your transcripts in `~/.cache/platoon/index.json` and only re-reads files that
changed, in the background, so it opens at once even with thousands of conversations (a small
`indexing N…` shows in the header while it catches up). Deleting the file is harmless: it is rebuilt. The words for the
[full-text search](#search) (and, for the smart names, one line of Claude's last reply) are kept the same way in `prompts.jsonl`
next to it, by remembering how far into each transcript it has read.

## Projects

A project is a name, a color and a badge, and the conversations you put in it. It isn't tied to a folder: a project can
hold conversations from any number of folders, and each keeps its own. A conversation is in at most one project, and any
conversation can be put in one, including ones you started outside platoon. Conversations in a project still show on the
Sessions tab, with the project's badge (its first letters, on its color) in front of the name.

On the **Projects** tab each row shows the badge and name, how many active and archived sessions the project has, when its
notes last changed and when it was last used.

| Key (Projects tab) | Does |
| --- | --- |
| `n` | new project (it asks for a name only) |
| `enter` | open the project's screen |
| `r` | rename it |
| `a` | archive it (on the Archived side: restore it) |
| `c` | its color (one of the theme's, so it follows the theme) and badge (up to 3 letters) |
| `d` | delete it (asks first) |
| `/` | narrow the list of projects as you type |
| `←` `→` | Active or Archived projects |

A **project's screen** shows its sessions on the left (active ones first, then an `Archived (N)` group, dimmed) and its
notes on the right. The left and right arrows move between the two sides; `↑` `↓` move in the sessions or scroll the notes.
`enter` on a session opens it as on the Sessions tab (and `a`, `m`, `r` work there too). `n` starts a new session in the
project: the folder picker lists the folders the project's other sessions use first. `/` searches only this project's
sessions, with all three kinds of search. `esc` goes back.

The notes are a file, `project.md`, in `~/.config/platoon/projects/<id>/` (the folder is named by the project's id, so a
rename changes nothing there). For now platoon shows it but doesn't edit it: create or edit it with any editor. When the
Project notes step of `platoon setup` is done, a session in the project gets the notes as context when it starts, when it
is resumed after the notes changed, and again after Claude compacts the conversation; unchanged notes are not sent again.
This works for every session in the project, including ones you run with plain `claude`.

- **Archiving** a project archives its active sessions too (stopping the ones that are running, as archiving a single
  conversation does; it asks first if one is working or runs in a terminal outside platoon). **Restoring** it brings back
  exactly those; sessions that were archived before stay archived. You can restore one session of an archived project on
  its own: it stays in the project and the project stays archived.
- **Deleting** a project takes its sessions out of it (they stay on the Sessions tab), deletes its notes and keeps one
  backup copy in `~/.cache/platoon/deleted-projects/<id>/`. No conversation is ever deleted.
- Automatic archiving after days of no use applies to conversations in projects too.
- Projects live in `~/.config/platoon/projects.json`.

```
platoon open --project "Web shop"   # start a new session in this folder, in that project
```

## Sessions started in platoon keep running

Each one runs under a small background host, so quitting platoon (or closing its terminal) never ends it.
`Ctrl+Space` leaves a session without stopping it. A session nobody has looked at for a while does stop
itself (see [Automatic clean-up](#automatic-clean-up)); `enter` picks the conversation up again.

## Copying text from a session

platoon takes over the mouse while you look at a session (that is what makes the wheel scroll it and the
`ctrl+space back to list` hint clickable), so the terminal's own drag-to-select does not get the drag. platoon
does the selecting itself instead:

| Do | Result |
| --- | --- |
| Drag with the left button | highlights the text under the drag; letting go copies it (like tmux) |
| Double-click | selects a word and copies it; drag on after the second click to add more words |
| Triple-click | selects the whole line (all the rows of it, if the terminal wrapped it) and copies it |
| Click | clears the highlight; copies nothing |
| Hold `shift` and drag | the terminal's own selection, as in any terminal (in Ghostty, Xfce Terminal and most others) |

A note in the status bar says what was copied (`Copied 3 lines`, `Copied 12 characters`), or that nothing could
be, in which case hold `shift` and drag.

- The selection is of what is on screen, in reading order: the rest of the first row, whole rows in between, the start
  of the last. Rows the terminal wrapped are joined without a line break, real line breaks stay, trailing spaces are
  dropped, and a selection of nothing but blanks copies nothing. Dragging past the edge of the session sticks to that
  edge (it does not scroll).
- A word is letters, digits and the characters that live inside paths and URLs (`_ - . / : ~ @ % + = # ? &`), so a
  path, a URL or `--flag=value` is one word. Quotes, brackets, commas, `|`, `$` and box-drawing characters end a
  word, and a full stop, colon or question mark right at its end is left off unless you clicked on it.
- The highlight is drawn in a color made from your theme (its background moved toward its accent; Terminal colors
  draws selected text reversed). It goes away when you type, paste, scroll the wheel, resize the window or leave the
  session. The wheel scrolls as before, and turning it drops the selection.
- The text goes to the **system clipboard and the primary selection** (so middle-click pastes it, too), through
  platoon itself: nothing like `xclip` or `wl-copy` needs to be installed. It is also sent to the terminal as an
  **OSC 52** sequence, which Ghostty, kitty, WezTerm and iTerm2 turn into a copy of their own, and which is what makes
  copying work over SSH. Terminals without OSC 52 (Xfce Terminal, for one) ignore it.
- **On X11** the program that put text on the clipboard is the one that hands it to whoever pastes, so what
  platoon copied stays pasteable while platoon runs and goes away when it quits, unless a clipboard manager (Xfce's
  Clipman, for example, if it is running) took a copy. On Wayland arboard leaves a small background process to do the
  same. Copies made through OSC 52 belong to the terminal and stay.
- `PLATOON_CLIPBOARD` chooses where copies go: `system` (the default: system clipboard and OSC 52), `osc52` (only
  the terminal sequence) or `none` (nothing is copied; selecting still highlights and says what was selected).
- Text over 512 KiB is cut at that size, and one over about 100 KB is not sent as OSC 52 (some terminals refuse
  sequences that long), only to the system clipboard; the status bar says when either happened.

`platoon open` in a regular terminal does not take over the mouse: platoon passes the session's output straight
through, so the terminal's own selection and copy work as usual.

## See a session live from a regular terminal

```
platoon open            # start a new session in this directory
platoon open a1b2       # join a session by (a prefix of) its id, resuming it if nothing is running
platoon open --project shop   # start a new session in this directory, in a project (by its name)
```

The session is shared, tmux-style: every window shows it live and any of them can type. The session is
sized to the smallest window attached. `Ctrl+Space` detaches; the session keeps running. A session running
in a plain `claude` platoon doesn't own can't be shared, so opening it asks before ending it and
reopening the same conversation from its saved history.
