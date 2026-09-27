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
| Session guard | Stops two copies of one conversation from overwriting each other, and keeps a [project's](#projects) notes in each of its conversations while they run |
| Project notes | Hands a [project's](#projects) notes to its sessions when they start or resume. Optional: platoon starts without it, projects still work, but their notes don't reach sessions. If you set platoon up before projects existed, run `platoon setup` once more to add it |
| Notes updates | Lets platoon [keep a project's notes up to date](#notes-that-keep-themselves-up-to-date) after its sessions stop. Also optional, also added by running `platoon setup` again |
| Tickets skill | Adds [`/to-tickets`](#making-tickets-from-a-conversation) to Claude Code, which turns a conversation into tickets. Optional. Unlike the others it is a file, not a setting: `skills/to-tickets/SKILL.md` in Claude Code's folder (`~/.claude`, or `CLAUDE_CONFIG_DIR`). platoon remembers the copy it last wrote (in `setup.json` in its own config folder): when platoon's copy changes and yours is still the one it wrote, `platoon setup` updates it quietly. If you changed the file, setup leaves it alone and says "changed by you"; `platoon setup --replace to-tickets` puts platoon's copy in (yours is kept as `SKILL.md.platoon-bak`). `platoon setup --remove to-tickets` takes it out and remembers that, so later runs leave it out (the line says "removed by you"); `platoon setup --add to-tickets` puts it back |

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

**How wide the columns are.** NAME and DIR hold as much as the rows on show need and no more, up to 60 columns each. A value
longer than that is cut: a name loses its end (`a very long name…`), a path its start (`…/repos/platoon`). BRANCH, MODEL,
STATUS, CONTEXT and LAST hold what they always did. Whatever a wide terminal has over once every column has what it needs is
then **shared equally between the columns**, so the row fills the screen and LAST sits at the right edge; the odd cell that
does not divide evenly goes to the leftmost columns. Each value stays at the left of its column, so a short one simply leaves
a wider gap after it than a long one, and the context bar keeps its own size (how full it looks is what it means) with its
share of the spare width as clear air after the percentage. On a narrow terminal, where the content already wants more room
than there is, nothing is over to share: BRANCH goes first, then MODEL, and NAME and DIR share whatever room is left, as
before.

| Key | Does |
| --- | --- |
| `enter` | open: shows it live if platoon is running it, otherwise resumes it from its saved history in its own directory; a `Background` one is opened with `claude attach` |
| `n` | new session (pick a directory) |
| `a` | archive the selected conversation (on the Archived tab: restore it) |
| `m` | put the selected conversation in a [project](#projects), move it to another, or take it out ("No project") |
| `h` | [hand this work on](#handing-work-on-to-a-new-session) to a new session in the same project (needs the smart features) |
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

`s` opens the settings as a popup over the list; `esc`, `s`, `q`, or a click outside it closes it. The settings are in five
tabs along the top: **General**, **Search**, **Smart features**, **Sessions** and **Projects**. `tab` goes to the next tab and
`shift+tab` to the previous one (both go round), and a click on a tab's name shows it. The popup opens on General, and keeps the
same size on every tab; a tab with more entries than fit scrolls, and on a small terminal only the selected entry is explained. Every setting keeps the name, choices and key in the file it had before the tabs.

**General**

| Setting | What it does | Choices |
| --- | --- | --- |
| Theme | The colors platoon is drawn in. `enter` on it opens a list that previews each theme on your real screen (see [Themes](#themes)). | The [built-in themes](#themes) (Catppuccin Mocha by default) and your own |
| Stop idle sessions after | A session platoon is running that nobody has looked at, and that isn't working, is stopped to free memory. Nothing is lost: opening it again resumes it from its saved history. | Off, 30 minutes, 1 hour, 2 hours (default), 4 hours, 8 hours, 1 day |
| Archive conversations after | A conversation nobody has used for this long, with nothing running, moves to the Archived tab. You can restore it from there. | Off, 1 day, 2 days (default), 7 days, 14 days, 30 days |
| Idle stop and MCP servers | Whether the MCP servers of a session count as something running when deciding to stop it (see above). | Ignore MCP servers (default), Count MCP servers |

**Search**

| Setting | What it does | Choices |
| --- | --- | --- |
| Search archived | Whether the [Full-text search](#search) also looks in archived conversations. | Include archived (default), Skip archived |
| Search results shown | The most conversations a full-text search lists. | 10, 20, 50 (default), 100, 200 |
| Search preview length | The most of a matching message a result shows around your words (it is also cut to fit the window). | 40, 80, 120 (default), 200, 400 characters |
| Longest message kept | How much of each message you sent is remembered for searching, so a huge paste doesn't fill the memory. Changing it makes platoon read your conversations again in the background. | 300, 1000, 2000 (default), 5000, 20000 characters |
| Description search: conversations sent / results / wider search | How many conversations a search by description ([Describe it](#search)) sends, how many results it lists, how many the wider search sends. | 20-150 (50), 5-20 (10), 100-500 (200) |
| System prompt for Describe it | What platoon asks Claude for when you search by describing a conversation (see [System prompts](#system-prompts-for-each-smart-feature)). | Enter to change, User Modified |

**Smart features**: whether your Claude plan is used at all, and how much. Under the entries: `Used today: N of M tokens`, and,
when naming is held up, why (`Naming is waiting: ...`).

| Setting | What it does | Choices |
| --- | --- | --- |
| Enable features which require tokens | The master switch for everything that uses your Claude plan (see [Features that use tokens](#features-that-use-tokens)). | Off (default), On |
| Smart names and summaries, Find a session by description, Project notes, Project descriptions | One switch per such feature. A feature runs only when the master switch and its own switch are both On. | Off / On (the two project ones are On once the master switch is) |
| Daily token limit | The most tokens those features may use in a day; at the limit they wait until tomorrow. | 100K, 250K, 500K (default), 1M, 2M, 5M tokens |
| Pause above plan usage | Background work stops asking Claude while your plan is used more than this much. (A search you ask for yourself ignores this.) | Never, 50%, 60%, 70%, 80% (default), 90% |
| Model for smart features | Which Claude model does the work. | Haiku (default), Sonnet |

**Sessions**: what naming and summarizing your conversations reads, and how often. Under the entries:
`Archived: N conversations, about X tokens to index (an estimate)`.

| Setting | What it does | Choices |
| --- | --- | --- |
| Conversations per call | How many conversations one request to Claude covers. | 5, 10, 20, 30 (default), 50, 100 |
| Refresh a summary after | How many new substantive messages from you a conversation gets before its name and summary are made again. | 1, 3, 5 (default), 10, 20, 50 messages |
| Wait for quiet before refreshing | A conversation is refreshed only once nothing has been written to it for this long. | 15, 30 seconds, 1 (default), 2, 5, 10 minutes |
| Recent messages sent | How many of a conversation's latest messages go into its summary, besides the first one. | 3, 5, 8 (default), 12, 20 messages |
| Longest message sent | Each message is cut to this many characters before it is sent. | 100, 200, 300 (default), 500, 1000 characters |
| Archived conversations | Whether archived conversations are named too, slowly in the background. | Skip (default), In the background |
| Share for archived | Naming archived conversations may use at most this much of what is left of the day's limit. | 10, 25, 50 (default), 75, 100% |
| System prompt for naming | What platoon asks Claude for when it names and summarizes your conversations (see [System prompts](#system-prompts-for-each-smart-feature)). | Enter to change, User Modified |

**Projects**

| Setting | What it does | Choices |
| --- | --- | --- |
| Update notes after | How long a project has to be quiet before platoon brings its [notes](#notes-that-keep-themselves-up-to-date) up to date. A new answer in any of its sessions starts the wait again. | 1, 3 (default), 5, 15, 30 minutes |
| Read Claude's last reply up to | How much of Claude's last answer in each session platoon reads when it brings a project's notes up to date. What is still open is usually in Claude's answers; more of them uses a little more of your plan. | 500, 1500 (default), 3000 characters |
| Squads at work at once | How many [squads](#handing-a-ticket-to-a-squad) may work on tickets at the same time, in all projects. One more handed over waits for a free squad and starts by itself, oldest first. Lowering it never stops a squad at work. | 1 to 10 squads (default 3) |
| System prompt for project notes, for project descriptions, for the hand-off | How platoon asks Claude to write a project's notes, the line under a project's name, and a hand-off (see [System prompts](#system-prompts-for-each-smart-feature)). | Enter to change, User Modified |

#### System prompts for each smart feature

Each thing platoon asks Claude for - names and summaries, Describe it, a project's notes, a project's description, the
hand-off - comes with a system prompt, and each has an entry on the tab it belongs to. It reads **Enter to change** while
it is platoon's default, and **User Modified** once you have changed it. Only the part that says what to aim for is yours to
change: the tone, what to put in and what to leave out. The exact shape of Claude's answer, which platoon checks, and the
rule that what Claude is shown is data and never orders, are kept by platoon and always sent after your text, so an edit
can't break the replies.

- `enter` (or a click on the value) opens the same list of editors as a project's notes. The file starts as platoon's
  default; save your change and it is used from the next call. Leaving the default as it was, emptying the file, or
  making it the default again word for word all count as "not changed", and the file goes (without a message when
  nothing changed).
- `r` restores platoon's default, after asking. It is only offered (and shown in the footer) once you have changed that
  system prompt. Your version is kept as `<name>.md.bak` (one copy, replaced by the next restore).
- When a newer platoon changes the default of a system prompt you have changed, your text is kept and the entry says
  **User Modified · default updated** (in the warning color) until you edit it again or restore the default.

The files are `~/.config/platoon/prompts/<name>.md` (`names`, `describe-it`, `project-notes`, `project-description`,
`hand-off`, `squad-commander`, `squad-draft`, `agent-draft`), only once you have changed something, with `built-in.json` beside them remembering which default each
started from. Every part of platoon reads them, the notes helper and `platoon setup`'s first naming pass included. A file
that is missing, can't be read or is empty means platoon's default, and at most the first 8,000 characters of one are sent.

`↑` `↓` pick a setting on the tab showing (the popup opens on Theme); `←` `→` (or `h` `l`, `-` `+`) step through its choices; `enter` goes to the
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
 "ai_find_wide_candidates": 200, "notes_editor": "ask", "notes_quiet_minutes": 3,
 "notes_reply_chars": 1500}
```

A value that isn't one of the steps (say `90` minutes) is shown as it is and left alone until you change that
setting, when it moves to the nearest step. Limits are 7 days (`idle_stop_minutes`) and 365 days
(`auto_archive_days`). `idle_stop_ignores_mcp` and `search_archived` are `true` or `false`; anything else there means the default
(`true`). The search numbers are limited to 5 to 500 results, 30 to 400 preview characters and 200 to 20000 message characters. `theme` is one of the [theme](#themes) ids (`user:...` for one of your own). The token numbers are limited to 20,000 to 100,000,000 tokens a day (there is no "unlimited"), 10 to 100 percent (`0` means never pause), 1 to 100 conversations and 1 to 100 messages; `ai_model` is `haiku` or `sonnet`; `notes_editor` is the editor you used last, by its command (`code`, `nano`), or `ask` while you haven't used one; an editor this computer no longer has is simply not starred and stays in the file until you use another; `notes_quiet_minutes` is 1 to 60; `squads_at_once` is 1 to 10; `notes_reply_chars` is 200 to 5000; `ai_archived` is `"off"` or `"background"`; `ai_quiet_seconds` is 0 to 3600, `ai_prompts_per_session` 1 to 30, `ai_prompt_chars` 50 to 2000, `ai_archived_share` 5 to 100 percent, and the three description-search numbers 10 to 300, 3 to 20 and 50 to 1000. Anything else in the file is kept when platoon saves. A missing or damaged file just means the defaults, and a damaged one is never replaced without keeping a `.bak` copy first.

### Features that use tokens

Four features ask Claude, which uses your own Claude plan. They are **off by default** and switched on by **Enable features which
require tokens** in the settings (on the **Smart features** tab) plus the switch of each feature:

- **Smart names and summaries**: gives each conversation a short name and a one- or two-sentence summary (shown under the list for
  the selected row, found by the search), and keeps them up to date as the conversation goes on.
- **Find a session by description**: the [Describe it](#search) search.
- **Project notes**: keeps a project's notes up to date by itself, a few minutes after its sessions stop; see
  [Notes that keep themselves up to date](#notes-that-keep-themselves-up-to-date). This one is on as soon as the master switch is.
- **Project descriptions**: the one line under a project's name saying what it is; see
  [The line that says what a project is](#the-line-that-says-what-a-project-is). On as soon as the master switch is, too.

The [hand-off](#handing-work-on-to-a-new-session) (`h`) has no switch: it only ever runs because you pressed the key. It still needs
the master switch on and still counts against the daily limit, like everything else.

`platoon tokens` lists each feature's switch and, under today's total, how many tokens each one used and in how many calls (the
hand-off and `platoon tokens test` appear there too, by name).

`platoon setup` asks once, in a terminal, with the default "no". It lists the features and says that platoon would send Claude the
prompts you typed plus one line of Claude's last reply for each conversation (for a project's notes, more of that reply), in small batches, and that you can switch it off any
time (`s`, then the Smart features tab, then the master switch). Without a terminal setup asks nothing, leaves everything off and prints one
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
Sessions tab, with a **PROJECT** column after the name: the project's name in a pill of its color (round ends where the
terminal has the glyphs, like the count badges). The pill is as wide as the longest project name on the tab needs (a name of
up to 40 characters shows in full on a wide enough terminal; longer names are cut), sized from every conversation on the tab, so it doesn't jump about while you type in the
quick filter; on a wide terminal the column also takes its share of the spare width, as clear air after the pill. Below 90
terminal columns the pill shows the project's badge letters instead. A tab where no conversation is in a project has no such
column.

On the **Projects** tab each row shows the badge and name, [what the project is](#the-line-that-says-what-a-project-is) under it
(in full: a long one wraps onto a further line and that row grows to fit), how many active and archived sessions the project has, when its notes last changed and when it was last used. Its name column, too, holds only as much as the projects on show need, and its
columns share a wide terminal's spare width the same way.

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

A **project's screen** shows its name and that same description across the top (in full, wrapped when it is long), its sessions at the top left (active ones
first, then an `Archived (N)` group, dimmed), its [tickets](#tickets) under them and its notes on the right. `tab` and `shift+tab` go round the three
panes. `shift` with an arrow moves between them too: `shift+↑` to Sessions, `shift+↓` to Tickets, `shift+→` to the notes
and `shift+←` back to whichever of Sessions and Tickets you used last. `ctrl` with an arrow does the same, for terminals
that keep `shift`+arrows for themselves. The plain arrows stay inside a pane: `↑` `↓` move in the sessions or tickets or
scroll the notes, and `←` `→` do nothing there.
`enter` on a session opens it as on the Sessions tab (and `a`, `m`, `r` and `h` work there too). `n` starts a new session in the
project: the folder picker lists the folders the project's other sessions use first. `/` searches only this project's
sessions, with all three kinds of search. `esc` goes back. With the notes in focus, `e` [edits them](#editing-the-notes),
`c` shows what changed last and `u` undoes it.

The notes are a file, `project.md`, in `~/.config/platoon/projects/<id>/` (the folder is named by the project's id, so a
rename changes nothing there). **Every session in the project has them in its context**, and keeps them as they change:

- When the **Project notes** step of `platoon setup` is done, a session gets the notes when it starts, when it is resumed
  after they changed, and again after Claude compacts the conversation.
- While a session is running, the **Session guard** step hands them over again with your next message whenever they have
  changed since that conversation last got them - so notes written an hour into a session still reach it.

Either way they are only ever sent when they are **new or changed for that conversation** (platoon keeps the version each
session has, as a short hash in `~/.cache/platoon/notes-sent/<session id>`), so the same words are never sent twice and the
two hooks never both send the same version. About 2 KB of notes is roughly 500 tokens, on the changes only. It works for
every session in the project, including ones you run with plain `claude`, and the notes pane says how many of the project's
sessions have the version that is on disk now:

```
These notes go to every session in this project; 2 of 3 have the latest version.
```

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

### Editing the notes

With the notes pane in focus (the right arrow on a project's screen):

| Key | Does |
| --- | --- |
| `e` | the list of editors this computer has. The one you used last is **starred and first**, and already picked, so `enter` opens it again |
| `c` | what changed last: the lines that went and the lines that came |
| `u` | undo the last change (and `u` again undoes the undo) |

**Which editors.** platoon uses editors you already have; nothing is installed. It looks at `$VISUAL` and `$EDITOR` first,
then for editors it knows by name: nano, Neovim, Vim, Helix, Micro, Emacs and `vi` in the terminal, and VS Code, Cursor,
Zed, Sublime Text, Kate, gedit, GNOME Text Editor and Mousepad in windows of their own (TextEdit on macOS). Only the ones
it finds are offered, by their plain names. An editor that lives in the terminal **runs inside platoon's window**, like a
session (`ctrl+space` ends it if it ever wedges); one with a window of its own opens beside platoon, which says so on the
notes and reads them again as soon as you are done or the file changes. There is nothing to set up: whichever one you use is
remembered (as `notes_editor` in `settings.json`) and offered first, with a star, the next time. If platoon finds nothing at
all it offers `nano` and `vi`.

**A new file starts from a template**: one line saying what platoon keeps up to date, then the two headings that are
always there, **Summary** and **Decisions**. The sections in between are added as the work needs them (see
[The shape of the notes](#the-shape-of-the-notes)).

**Nothing is lost.** Before every change platoon didn't make itself, the version that was there is copied into
`~/.config/platoon/projects/<id>/history/`, newest first, at most 20 per project (the oldest goes). `c` shows the last
change with `-` in front of the lines that went and `+` in front of the ones that came, so it reads on a screen with no
colors. `u` puts the previous version back, and that counts as a change of its own, so `u` again brings your edit back.
platoon never writes the file while an editor it started is open, and always reads the file from disk before showing it:
if something else changed it, what is on disk wins.

**Length is whatever the project needs.** Nothing is ever cut and platoon doesn't nag about it; the pane does say how many
of the project's sessions already have the notes as they are now (see below).

### The shape of the notes

Three parts of `project.md` are fixed, and everything else fits the work:

- **Summary**, at the top: a few lines saying what this project is and where it stands, so a new session (and you) can
  read it in ten seconds.
- **Open questions**, right after the summary: only what is **still open** - questions nobody has answered yet and
  matters not settled yet - grouped under subheadings that fit the project (by area, or "Waiting on the user",
  "Undecided"), plain bullets, nothing tagged onto them. It is not a to-do list. When a question is answered or a matter
  settled, it is **taken out** (not ticked) and the answer appears as a new entry in Decisions. A notes file that already
  had an Open questions section somewhere in the middle has it moved up here, word for word, the next time platoon
  writes; one without it gets it when something first comes up.
- **Decisions**, at the end: dated entries, added one at a time and never rewritten.
- **In between, whatever the project needs.** Requirements and Status while something is being built; Symptoms, Ruled out
  and Leads while a problem is being chased; Questions and Findings while something is being looked into. When the work
  changes, the headings change with it: platoon may add one, rename one, fold two into one or put them in another order.

**How it is written.** The file is Markdown: short bullets with one fact each, indented bullets for detail, `##`
subheadings to group things inside a section, bold now and then, and short paragraphs only in the summary. A section
platoon wrote as paragraphs before this may come back as bullets. The notes pane shows it the way a Markdown viewer
would: headings without their `#`, bullets as `•` with wrapped lines lined up under the text, bold, italic, code and
quotes styled, links as their words (the address stays in the file). On the Linux console, or with
`PLATOON_GLYPHS=plain`, bullets are `-` and `*`.

**Nothing is ever deleted by a change of heading.** A section platoon doesn't mention stays exactly where it is, word for
word. A heading can only go away by being carried into the one that replaces it, and everything of yours that was under
it has to come across with it or the change is thrown away (below).

Notes written before this - the five fixed headings Overview, Requirements, Status, Decisions and Notes - carry on
working and lose nothing: a summary is added at the top the next time platoon writes, and your old headings become
ordinary sections that platoon may reshape from then on. Your old **Notes** section is one of them: platoon has never
written a word in it, so every line of it is yours and stays word for word.

### Notes that keep themselves up to date

With **Project notes** on (see [features that use tokens](#features-that-use-tokens)), platoon writes the notes for you from
what your sessions actually did. It needs the **Notes updates** line of `platoon setup`, which adds a `Stop` hook to Claude
Code; without it nothing happens automatically.

**How it runs.** Every time a session in a project finishes answering, the hook makes a note of it - instantly, silently, no
tokens - and starts a small helper in the background. Putting a conversation **into** a project does the same: `m`, a session
started under a project, or one the SessionStart hook places there all mark the project, so its notes catch up a few minutes
later without you having to resume anything (a conversation you never open again would otherwise never be read at all). The helper waits until the project has been quiet for as long as
**Update notes after** says (3 minutes unless you change it, in the [settings](#settings) on the Projects tab: 1, 3, 5, 15 or
30 minutes; another turn starts that timer over) and then makes **one** request to Claude. Only one helper runs per project,
and it works whether the platoon window is open or closed. Archived projects, archived sessions, and projects whose notes
file doesn't exist yet are skipped entirely: platoon never starts the notes for you, you do that with `e`.

**What it reads.** Only what is new since the last update: for each active session in the project, the messages you typed
since then (at most 8, each cut to 300 characters - the same settings the smart names use) plus Claude's last reply, up to
**Read Claude's last reply up to** characters (1500 unless you change it), because what is still open in a conversation is
usually in Claude's answers. A session that platoon has never read before - a new project, or one you have just added a session to - is read in
full once: its first message and its last 20.

**What it may write.**

| Part | What an update may do |
| --- | --- |
| Summary | rewrite it, keeping it short; it is added at the top if it isn't there |
| The sections in between | add one, reword **what platoon itself wrote** in one, rename one, fold two into one, put them in another order |
| Open questions | rewrite it; take out what has been answered. **A line of yours there may only go in the same update that adds its answer to Decisions**; it may move to another subheading |
| Decisions | append dated entries; nothing already there is ever changed. platoon writes the date, so a date the answer put at the front of an entry is taken off rather than written twice |

platoon remembers exactly which sentences it wrote itself, under which heading, so every other line of a section is
yours. If an answer comes back without one of your lines in it, word for word, that section is **left exactly as it
was** - and that holds when a heading is renamed too: your lines have to turn up under the new one. The same goes for an
answer that isn't understood and for notes that changed on disk while the request was out: what is on disk wins, and
platoon works the merge out again or leaves it alone. A line of yours that an answer had to repeat stays yours: platoon
only ever counts its own words as its own.

**Seeing it.** The notes pane says `platoon is updating these notes now.` while a request is out, and `platoon last updated
these 5m ago.` afterwards. Every update goes through the same history as your own edits, so `c` shows what changed and `u`
undoes it, and platoon tells you once when the notes change under you.

**When they are not updating, it says so.** Whenever something is stopping the updates, the pane says which and what to do
about it, in one line that goes as soon as the reason does:

```
These notes are not updating right now: "Project notes" is switched off. Press s for the settings to switch it back on.
These notes are not updating right now: Today's token limit is used up (25,000 of 20,000 tokens). It starts over tomorrow, or you can raise it in settings.
These notes are not updating right now: Your Claude plan is 82% used (weekly limit), above the pause point of 80%. Smart features wait until it drops. Press s for the settings to change the pause point.
```

The three that stop **everything** that uses tokens - the day's limit, the plan pause, a pause after failures - are also said
quietly in the header, on every tab, because those arrive out of nowhere:

```
platoon   1 Sessions 3   2 Projects 1   ✦ smart features paused · your Claude plan is 82% used (weekly limit) · press s for the settings
```

A switch you turned off yourself is not in the header (it would sit there for as long as you left it off); it is said where
that feature lives, as above.

**Where it is kept.** The times and the copy of platoon's own sentences, heading by heading, live in
`~/.config/platoon/projects/<id>/notes-state.json` (numbers and platoon's own text only - never a message you typed,
never a reply from Claude). When a heading is renamed, its record moves with it; when a section is folded into
another, its record goes.

### The line that says what a project is

With **Project descriptions** on (see [features that use tokens](#features-that-use-tokens)), each project gets one short line
saying what it is, under its name on the Projects tab and across the top of its own screen:

```
  WB  Web shop              1 active                        notes now                         2h
      A sock shop, now with a basket that adds up.
```

It is written from the summaries platoon already made for the project's conversations plus the project's own notes - no
transcript is read for it, and no conversation's id or folder is ever in the request. It is refreshed **straight after a notes
update**, and at most **once an hour** per project, so it costs one small call at a time and only when something has actually
happened. Archived projects and archived sessions are left out entirely, an answer that isn't understood changes nothing (the
line that is there stays, and the next update tries again), and the line is cut to 100 characters with every control character
and escape sequence removed before it is drawn. It is kept beside the project in `~/.config/platoon/projects.json`.

### Handing work on to a new session

`h` on a conversation that is in a project - on the Sessions tab or in the project's own screen - hands that work on to a fresh
conversation. A box opens straight away and says it is asking Claude; meanwhile platoon reads the conversation and, if the
folder is a git repository, what git says about it (only reading, never changing anything, and giving up after 5 seconds).
Then it shows a **hand-off document** with a fixed shape:

```
# Session hand-off: Basket page

This is context for this session. The user will prompt you for the next course of action.

- Written: 2026-09-25 14:03
- Project: Web shop
- Project notes: ~/.config/platoon/projects/p1/project.md
- Previous session: Basket page, in ~/shop (not running)
- Previous session id: 0b5e7c1a-... (`claude --resume 0b5e7c1a-...` opens it)
- Started: 2026-09-25 11:58, last active 2026-09-25 14:03 (2 h 5 min between)
- Model: Sonnet 5, context 42% full
- Messages from the user: 14 (short replies like "yes" not counted)
- Tokens the previous session used: 45,210 sent to Claude and 12,034 written by Claude
- Git branch: main
- Compared with origin/main, as of the last fetch: 2 commits not pushed yet
- Uncommitted changes: 2 files
- Last commit: a1b2c3d Add the basket
- Commits since the session started: 2, newest first:
  - a1b2c3d Add the basket
  - 9f8e7d6 Start the totals

## Executive summary

The basket page is built and totals are next.

## Work completed

- Added the basket page

## Files affected

Not committed yet:
- src/basket.rs (changed)
Committed since the session started:
- src/totals.rs

## Current state and known gotchas

- Totals ignore VAT for now

## Next steps

1. Add VAT to the totals
2. Take card payments
```

- **The line under the title** tells the new session that the document is background, and that you will say what to do
  next. You can delete it like any other line.
- **The facts** (the list at the top) come from platoon itself: the project and where its notes are, the old session's
  folder, id, whether it is still running, when it started and was last used, its model, how many messages you sent, the
  tokens it used, and the git branch, uncommitted changes, last commit and the commits made during the session. A fact
  platoon can't find out is left out, never guessed.
- **Files affected** comes from git too. When the folder is not a git repository, Claude lists the files instead, and the
  section says so.
- **Claude writes only** the executive summary, the work completed, the current state and known gotchas, and the numbered
  next steps. A section with nothing in it is left out.

Claude is sent the project's name, the folder's name, the conversation's name and summary, your first message and your
last few (each cut short), one line of its last reply and the file names git gave. It is never sent the session id, and
the project's notes are not repeated: the new session is given those by the hooks, as every session in a project is.

**You can edit the document in the box.** The text wraps at the edge of the box and scrolls: the arrows move the cursor
(up and down by a row on screen), Home and End go to the start and end of a line, PageUp and PageDown move a page, the
mouse wheel scrolls, and `enter` starts a new line. The lines above and below the text say how much more there is ("↑ 3
more lines above", "↓ 12 more lines below"). The document can be up to 20,000 characters.

`ctrl+s` starts a **new** session in the same folder and the same project, with the document as its **first message**,
visible in the conversation like anything else you typed. `esc` calls the whole thing off and changes nothing.

The old conversation is left exactly as it is: not archived, not stopped. If Claude can't be asked (the master switch is
off, the day's limit is spent, a failure has paused things), the conversation has nothing in it yet, or the answer isn't
understood, platoon says so plainly and nothing is started.


### Tickets

A **ticket** is one piece of work in a project: a title, a body you write in Markdown (which also says how far to take
the work), the folder or folders it works in, the tickets it **waits for**, and where it is (its state). Each ticket has a number in its project
(`#1`, `#2`, ...), which is never given to another ticket. You can keep them as a list, or hand one to a squad (see
[Handing a ticket to a squad](#handing-a-ticket-to-a-squad)).

The **Tickets** pane sits under Sessions on a project's screen. Open tickets come first; the Done and Cancelled ones are
kept apart under a line that says **Done and cancelled · N**, folded until you press `enter` on it. Each row says the
number, the title, the state and, for an open ticket, what it is still waiting for ("waits for #2, #4")
or a quiet **ready** when it is New and everything it waits for is Done (a Cancelled ticket still holds up the ones
waiting for it: only Done lets them go). On a narrow terminal the state's own column goes first (a Done, Cancelled or
otherwise not-New ticket then says its state at the end of its row).

| Key (Tickets pane) | Does |
| --- | --- |
| `n` | new ticket: type its title, then the body opens in your editor (the same list of editors as the notes). An archived project takes no new tickets |
| `enter` | open the ticket (on the Done and cancelled line: show or hide them) |
| `c` `e` `r` `f` `b` `a` `p` | as on the ticket's own screen, for the ticket picked |

A new ticket starts as **New**, works in the folder of the project's most recently used session (or has "No folder" when
the project has no sessions; `f` changes a ticket's folders, and a ticket may have none, for a squad that needs none), and its body starts from a short template: a **What it should do** heading (what should
work when it is finished, as behaviour, and small enough for one fresh session), with the line **How far: local changes
only (say if you want a commit, or a push and merge request).** under it, and an **Acceptance** heading with two empty
checkboxes. **How far to take the work is said in the ticket's text**: write "a local commit", "push it and open a merge
request" or anything else there. When the text doesn't say, a squad stops at local changes: it changes the files in its
worktree (or the folder) and commits nothing; a squad that works in the ticket's work folder stops once its result is
written there. (Tickets made by an older platoon may have a separate "how far" in `tickets.json`: it is
kept there and no longer read.)

**A ticket's own screen** (`enter`) shows its number and title, its state, **From** (the session it was made in with
[`/to-tickets`](#making-tickets-from-a-conversation): its name as the Sessions tab shows it, else its first prompt), the squad that has it ("Coding squad"), its
folders, its **work folder** once a squad has had it, what it waits for, and then its body, drawn the way the notes are; `↑` `↓` scroll it.

| Key (a ticket's screen) | Does |
| --- | --- |
| `e` | edit the body in your editor |
| `r` | rename it (its file keeps its name) |
| `c` | cancel it, after asking "Cancel ticket #N?" (`y` yes, `n` or `esc` no). When its squad's session is running, the question says it will be stopped too, and yes stops that one session (its conversation is kept). On a ticket that is Cancelled already, `c` only says so |
| `f` | its folders: add one with the folder picker, `d` takes the picked one away |
| `b` | what it waits for: tick the project's other tickets (`enter` or `space`). A ticket can't wait for itself, and a wait that would go round in a circle is refused |
| `a` | hand it to a squad (see below) |
| `p` | let its squad publish, when it is Ready to push/publish |
| `enter` | on a ticket a squad has: its squad's session, beside the ticket. On one made from a session that no squad has yet: that session, opened the way `enter` opens it on the Sessions tab |
| `o` | the session the ticket was made from (the **From** line), also when `enter` is the squad's |
| `esc` | back to the project, on the same ticket |

The states are **New, Working, Waiting on you, Ready to push/publish, Done, Cancelled**. You never set one by hand except
**Cancelled** (`c`): handing a ticket to a squad makes it Working, the squad sets the others, and writing to the squad in
its session makes the ticket Working again (see below). A ticket no squad has had can only be cancelled, and then it
stays cancelled.

Tickets live next to the project's notes: the list in `~/.config/platoon/projects/<id>/tickets.json` and each body in
`tickets/<NN>-<title>.md` in the same folder, yours to edit. Only platoon writes the list. Deleting a project keeps a
copy of its tickets with the rest of its backup; archiving it leaves them as they are.

From the command line (the [`/to-tickets`](#making-tickets-from-a-conversation) skill uses the same command, so tickets are always made through platoon):

```
platoon ticket add --project "Web shop" --title "Log in with a link" --folder ~/repos/web
platoon ticket add --project "Web shop" --title "Basket page" --waits-for 1 < body.md    # the body piped in
platoon ticket list --project "Web shop"            # open ones first, then Done and cancelled
platoon ticket list --json                          # every project, with each body file's full path
```

`add` prints the new ticket's number (`#3`). The project is found by its id or its name (any case). The body comes from
`--body-file`, else from what is piped in, else the template; with no `--folder` the ticket works in the folder you ran the command in
(its full path), and each `--folder` you give replaces that; `--no-folder` makes a ticket without a folder, for a squad that
needs none (Research, Summarize). A project that is archived takes no new tickets (restore it
first; its tickets can still be opened and changed in platoon). A project
that isn't there (or a name two projects share), a ticket to wait for that isn't there or would make a circle, or a folder
that doesn't exist is refused with one plain line and nothing is made. There is no `--how-far`: say how far in the body
(`--how-far` is refused with a line that says so). A running platoon shows the new ticket within a second.

### Making tickets from a conversation

Type **`/to-tickets`** in a Claude Code session that is in a project (the **Tickets skill** step of `platoon setup` adds it;
Claude never runs it by itself). You can add what to turn into tickets after it, such as a plan file; without that it uses
what the conversation has agreed on. It goes like this:

1. It asks platoon which project the session is in (`platoon ticket where`). If the session isn't in one, it says so in
   one line and stops: add the session to a project first (`m` on the Sessions tab), then type `/to-tickets` again.
2. It reads what kinds of work your squads do (`platoon squads --kinds`) and shapes the tickets to fit, for example a
   coding part apart from a documentation part. It never picks a squad: you do that on the ticket (`a`).
3. It drafts the tickets. Each is one thin, complete piece that fits in one fresh session (whoever picks it up has never
   seen the conversation), says what should work when it is done rather than which files change, has acceptance checks
   that would fail before the work, a **How far:** line (local changes only, unless the conversation asked for more), no
   file paths or code, and which of the new tickets it waits for. Preparation work comes first.
4. It shows them as a numbered list (title, what it waits for, what it delivers) and asks whether the sizes are right,
   whether the waits are real, and what to merge or split. **Nothing is written until you approve the list.**
5. It makes them through platoon, the ones others wait for first: one `platoon ticket add --from-session ...` per
   ticket, with the ticket's text piped in, and every working folder the session has (or `--no-folder` for a ticket
   that needs none, such as research). Then it lists the numbers platoon gave them. They are on the project's screen,
   in the Tickets pane.

The skill only runs `platoon ticket` and `platoon squads` commands, and it lists those two in its `allowed-tools`, so
Claude Code doesn't ask before running them. Claude Code clears that permission when you send your next message, so the
tickets it writes after you approve the list may still ask.

Each ticket remembers the session it came from: its screen shows **From** with the session's name, and `enter` there (or
`o`) opens that session. The commands it uses work on their own too:

```
platoon ticket where --session <session id>     # "Web shop (id …)", or one line and exit 1 when it's in no project
platoon squads --kinds [--json]                 # each squad: its name, what it can do, whether it needs a folder
platoon ticket add --from-session <session id> --title "…" [--project "…"]   # in that session's project
```

`--from-session` puts the ticket in the project that session belongs to and records where it came from (`"from":
{"session", "project"}` in `tickets.json` and in `ticket list --json`). A session in no project, or a `--project` that
names a different project, is refused with one line and nothing is made. Inside Claude Code a session's own id is in
`$CLAUDE_CODE_SESSION_ID`.

### Handing a ticket to a squad

A **squad** is a small team of Claude agents that works on a ticket for you: a **leader**, which is one Claude session of
its own, and the **agents** it hands the pieces to, each with its own job and its own model (agents are shared by
squads). platoon comes with four squads,
**Coding**, **Research**, **Docs** and **Summarize** (see [The built-in squads](#the-built-in-squads)), and you can change
them or make your own on the [Squads tab](#squads). As it comes, the Coding leader (Sonnet) hands the ticket to the **planner** (Opus), which reads the code and writes a short plan;
then to the **builder** (Sonnet), which makes the change and runs the tests; then to the **reviewer** (Opus), which checks
the change against the ticket (and that each acceptance check could have failed before it), sending it back to the builder
at most twice; then to the **verifier** (Haiku), which runs the tests and the build and reports the total of every test
result line (added up with a command). The leader waits for each of them before it decides anything, and takes the work
as far as the ticket's text says; when the text doesn't say, it stops at local changes, with nothing committed. In the
squad's session they are named after the squad and the agent, `coding-leader`, `coding-planner`, `coding-builder`, `coding-reviewer`
and `coding-verifier`, so Claude's own lines and status bar show which of them is at work.

Press `a` on a New ticket (in the Tickets pane or on its screen), pick the squad and press `enter`: every squad is
listed by name, with what it can do. A ticket that is not New, or that still waits for another ticket, is refused with
one line saying why, and so is a squad that has something to fix on the Squads tab. A ticket with no folder can go to a
squad that doesn't need one (Research, Summarize); a squad that needs a folder (Coding, Docs) says "Needs a folder, and
#N has none" in the list and refuses it with one line.

- **The commander checks first.** Before anything starts, the **squad commander** (one quick call to Claude, on Haiku,
  counted with your smart features' tokens) reads the ticket and what the squad can do, and says whether the squad can
  handle it. Meanwhile the ticket shows **Working** with "Commander is checking…", and platoon keeps going as usual. If
  the squad fits, the ticket goes on to it. If it doesn't, the ticket stays **New** and says why ("Commander: This is
  docs work; ..."); press `a` and pick the same squad again to be asked "Hand it anyway?", and `y` hands it over without
  another check (another squad gets a check of its own). If the commander can't run - today's token limit is used up,
  or smart features are paused after a failure - **nothing is handed over**: the ticket stays New and says **Session
  limit has been reached** (or "AI features are switched off" when they are). A check that fails says so, and hands
  nothing over either. Its system prompt is **System prompt for the squad commander**, on the Projects tab of the
  settings.

- **Its own worktree, when the squad uses one.** Each squad says whether it works in a git worktree: **always** (Coding),
  **when the folder is a git repo** (Docs), or **never** (Research, Summarize). For a squad that does, for each of the
  ticket's folders that is in a git repository, platoon makes a git worktree next to the repository, `<repo>-worktrees/ticket-<N>`, on a new branch `ticket/<N>` from where the repository is now, and
  the squad works there, never in your own checkout. A worktree already made for the ticket is used again; if the
  branch name `ticket/<N>` is taken by something else, platoon says so and stops. Folders outside git are used as they
  are. platoon never removes these worktrees. The worktree is made in the background (the ticket says "Making the
  worktree…"); if it can't be made, the ticket goes back to New with the reason, and one already made stays for the next
  try.
- **Its own work folder.** Every ticket handed to a squad gets a private work folder, kept with the ticket in
  `~/.config/platoon/projects/<id>/tickets/<NN>-work/` (platoon never removes it). A squad that never uses a worktree,
  and any squad given a ticket without a folder, starts there and writes what it makes there (Research's `report.md`,
  say) unless the ticket says otherwise; the ticket's own folders are given to it for reading only. Any other squad
  starts in the ticket's first folder (its worktree, when it has one) and gets the work folder too, for notes and
  drafts.
- **Its own session.** The leader starts in a platoon session of its own (it keeps running when platoon closes), named
  after its squad (`Coding squad · #N title`), with the ticket as its first message and the project's notes given to it as context. It belongs to the
  ticket only: it is never listed on the Sessions tab, in the project's sessions, in searches or counts, and it never
  joins the project.
- **Beside the ticket.** Opening a ticket that has a squad shows the ticket on the left and the squad's live session on
  the right. What you type goes to the session, as in any open session, and `ctrl+space` goes back to the project. The
  ticket side follows along: its state, and what the squad last said. On a terminal narrower than 100 columns the session
  gets the whole width, with one line about the ticket above it. When the squad's session isn't running (it finished,
  or platoon or the computer restarted), the right side says so, and `enter` continues the same conversation.

**How many squads work at once.** At most 3 squads work at the same time, across all your projects (**Squads at work
at once** on the Projects tab of the settings, 1 to 10). A squad counts while its ticket is Working and its leader's
session is running; one waiting on you, ready to publish, done or cancelled doesn't. When you hand a ticket over (the
commander said it fits, or you said "hand it anyway") and every place is taken, the ticket shows **Working · waiting for
a free squad** and starts by itself when a place frees, the one that has waited longest first, without taking over your
screen (a notice says so). Its worktree is made then, not while it waits. The line is written on the ticket, so it
outlasts platoon: the next platoon you open starts it. Only an open platoon starts tickets from the line; with two
platoon windows open, the first one opened does it (the other takes over when it closes), and a ticket is never started
twice. To take a ticket out of the line, cancel it (`c`). Lowering the number never stops
a squad; new ones just wait.

**What the squad has done, and the tokens it used.** A ticket's screen lists the members' runs in the order the leader
started them, as its session shows them (not the workflow on paper): each member by its agent name, with the short
description the leader gave it, `[x]` once its result has come back and `[>]` while it runs (`[-]` for a run whose
leader stopped before it reported), and "Now: coding-reviewer" while one is at work. Below the ticket's details it also
shows the tokens its squads used: the total, what that counts (what was sent to Claude, what Claude wrote, and what was
read again from Claude's cache, each answer once, the members' own work included), and a line per agent - the leader
and each member - added up over every squad the ticket has had. The Tickets pane shows the short total ("1.2M tokens")
on each ticket that has one. platoon reads this from the squad's session files in the background, every couple of
seconds, only for the open project's tickets that have had a squad.

The ticket's state follows the squad: at the end of each of its turns the leader says in one last line whether it is
done, still working (members running), has a question, is stuck, or is ready to publish, and platoon sets **Working**,
**Done**, **Waiting on you** (with its question or why it is stuck; a turn that ends without that line counts as "the
squad stopped without saying why") or **Ready to push/publish**. **The squad never closes a ticket unasked**: when the
work is finished it writes its short result (what was done, where, what was not checked) and asks "Shall I close this
ticket?", so the ticket shows Waiting on you with that question; only after you say yes does it end with done and the
ticket goes Done. When it is Done, the leader's result shows at the top of
the ticket's screen (it is kept in `tickets/<NN>-result.md`), and that result, and nothing else of the squad's
conversation, goes to the project's notes at their next update, as "Ticket #N (Coding squad)" (once).

While the leader is stuck on Claude's permission prompt, the ticket shows **Waiting on you** ("Waiting for your OK on a
command"); once you answer and it carries on, the ticket is Working again. The leader's session is not stopped for being
idle while its ticket is Working (its members may be at work while nobody watches); once the ticket is Waiting on you,
Ready, Done or Cancelled, it is stopped like any idle session.

**Anything other people can see asks you first**, even when the ticket asks for it: pushing, opening or merging
a merge request, publishing a package or a page. platoon refuses those commands in the squad's session until you say so,
and the squad stops at **Ready to push/publish**, saying what it wants to publish. Press `p` on the ticket to let it: the
ticket goes back to Working and platoon types "Go ahead and publish." into the squad's session. That go-ahead lasts while
the ticket is Working or Ready; once it is Done, Cancelled, New or waiting on you, the next push asks again. Local
changes and local commits never ask.

**Writing to the squad puts the ticket back to Working.** Whatever you send in the squad's session (an answer to its
question, "yes" to closing, more work after Done, the go-ahead `p` types) makes a ticket that is Waiting on you, Ready to
push/publish, Done or Cancelled Working again, with the same squad session: that is how a Done or Cancelled ticket is
opened again (`enter` continues its session when it isn't running, then write to it). A reopened ticket counts toward
the squads at work at once like any Working one, even when that makes one more than the number allowed for a while;
tickets in the line just wait longer. A New ticket and one waiting in the line are left alone. `c` cancels a ticket
after asking; when its squad's session is running, yes stops that one session too (its conversation is kept).

The squad's leader, its workflow and its agents (their models and system prompts) are on the [Squads tab](#squads). The rules
platoon keeps - where the squad works and which folders are for reading only, ask before anything others can see, the
last line of each turn, and "report back to the leader, never
push" for the members - are always sent after your text.

The leader may hand work to its members, read, search and run commands; it never edits a file itself (its members do).

Nothing about the squad is added to your own Claude Code settings: the leader's session alone gets platoon's squad hooks
(`platoon hook squad-stop`, `squad-gate`, `squad-notify`, `squad-tool` and `squad-prompt`), and `platoon setup` is not
involved.

### The built-in squads

- **Coding** (needs a folder; always a worktree): makes a change to code, as described above.
- **Research** (no folder needed; never a worktree): finds things out and writes them up, with sources. The
  **questioner** (Sonnet) turns the ticket into a short list of questions; the **researcher** (Sonnet, can use the web)
  answers each one from the ticket's folders and the web, with sources; the **report-writer** (Sonnet, can change files) writes
  `report.md` in the work folder: the answers, the sources and what is still unknown; the **checker** (Opus, can use the
  web) checks each claim against its source, and may send it back to the researcher once.
- **Docs** (needs a folder; a worktree when the folder is a git repo): writes or updates documentation as Markdown files
  in the ticket's folder (its worktree, in git). The **outliner** (Sonnet) reads the ticket, the existing docs and the
  code and proposes what to write where; the **tech-writer** (Sonnet, can change files) writes it; the **editor** (Opus) checks
  it against the code, the ticket and the project's style, and may send it back to the tech-writer twice. The result lists
  the files. Docs in Coda, Confluence and the like are for your own squads, using your Claude's connectors.
- **Summarize** (no folder needed; never a worktree): the **summarizer** (Sonnet, can use the web, so links can be read)
  reads what the ticket points at and writes a short summary for you; the leader puts it in the ticket's result.

Only Research's researcher and checker and the summarizer can use the web. Every leader runs on Sonnet.

## Squads

The third tab, `3` **Squads**, has two lists: the **squads** and the **agents** they are made of. The title of the left
pane shows which one you are looking at (`[Squads]  Agents`); `←` and `→` on the list switch between them (or click
the title). An agent is its own thing, shared by squads: the same planner can do a step in several squads.

**A squad**, on the right, in four parts:

- **About**: its name and **what it can do** in a sentence or two (the commander reads that before a ticket is handed to
  the squad).
- **Folder**: whether it **needs a folder**, and **Use a worktree for git folders** (`space` switches either). A squad
  of your own starts with no folder needed and no worktree, unless you copied it or its draft says otherwise.
- **Leader**: its model (Opus, Sonnet or Haiku) and its system prompt: how it leads, its tone, what its reports say and
  when it asks you. The leader belongs to the squad; it is not one of the agents.
- **Workflow**: the steps in order. Each step says in one line what it does and which **agent** does it, shown with
  that agent's model and switches, and a step may **send the work back** to an earlier step, a set number of times at
  most ("may send it back to step 2, at most twice"). The same agent can do more than one step. platoon writes the steps
  into the leader's system prompt as a numbered list, after your text and before platoon's own rules, and the leader
  follows them (platoon can't force the order: the agents run inside the leader's session). A squad's agents are the
  different agents its steps name: at most 8, and at most 12 steps.

On a step, `enter` on its agent (or `m`) picks another agent from all of them, or **+ New agent** at the bottom, which
asks for a name, makes a new agent of your own with no system prompt yet, puts it on the step and opens it on the Agents
list for you to write one (until it has one, the squad says so and can't be handed a ticket). `g` goes to the step's
agent on the Agents list. `b` says whether the step may send the work back and where to, `[` `]` move it up and down,
`a` adds a step and `x` takes it out.

**An agent**, on the right: its name, its model, three switches - **can change files**, **can run commands** and **can
use the web** (an agent can always read and search; the switches add the rest, and nothing else) - its system prompt,
and **Used in**: every squad that uses it, with the steps. `enter` on a squad there opens it. **A change to an agent is a
change in every squad that uses it**, so the tab says where it is used; for a variant, copy the agent (`c`) and put the
copy on the steps you want. The list groups the agents as built in, **User Modified** and your own, each with its model
and how many squads use it; **+ New agent** at the end makes a blank one of your own.

`↑` `↓` move in the list and in the details; `enter` (or `tab`, or `shift+→` / `ctrl+→`) goes into the details, and
`esc` (or `tab`, `shift+←`, `ctrl+←`) back to the list. In the details, `enter` changes the line you are on: a name or a
line of text in a small box, a model or an agent from a short list, a system prompt in the same editors as a project's
notes. `space` turns a switch on or off. Every change is saved at once.

**What platoon comes with is built in**: the four squads and their twelve agents (Coding's planner, builder, reviewer
and verifier; Research's questioner, researcher, report-writer and checker; Docs' outliner, tech-writer and editor; and
the summarizer). Everything about them can be changed; the list then says **User Modified**, and `d` puts one back as
platoon has it, after asking (your version is kept as `<id>.json.bak`). They can't be deleted.

**Making your own.** Press `n`: platoon asks whether to describe **a new squad** or **a new agent**, and you describe it
in your own words ("a squad that triages Sentry errors: a reader, a reproducer, a fixer"). One call to Claude, on Sonnet,
counted with your smart features' tokens, drafts it. A squad's draft uses the agents you have wherever one fits (Claude
is shown each agent's name, model, switches and a line of what it does) and brings new agents only for jobs none of
them does; the draft marks those "new with the draft", and saving the squad saves them too. platoon checks the draft
(names, models, steps that name an agent that isn't there, a step that sends work back to a later one) and says plainly
what was wrong if it can't be used. A good draft opens on the tab as **Draft, not saved**: change what you like, then
`ctrl+s` saves it, or `d` throws it away. `c` copies a squad or an agent, built in or your own, under a new name. `d`
deletes one of your own, after asking (its file is kept as `<id>.json.bak`); an agent a squad still uses can't be
deleted, and platoon names the squads. The drafting calls' system prompts are **System prompt for drafting a squad** and
**System prompt for drafting an agent**, on the Projects tab of the settings, next to the commander's.

**A running ticket keeps its squad.** When a squad's leader starts on a ticket, platoon keeps what it was started with
beside the ticket (`tickets/<NN>-squad-<k>.json`). Changing the squad or its agents afterwards doesn't change that
leader, also when it is continued later: it goes on with the squad as it was. The next ticket gets them as they are then.

Your squads and agents, and the built-in ones once you change them, are in `~/.config/platoon/squads/<id>.json` and
`~/.config/platoon/agents/<id>.json`, one file each; a squad's steps name its agents by their id, which is made from the
name once and never changes. Keys platoon doesn't know are kept, and a file that can't be read is shown as such on the tab
and never written over. A squad file from before agents were their own (its members inside it) is converted the first time
platoon reads it: each member becomes an agent of your own (an agent that is exactly the same is used as it is; a
different one already called that gets the squad's name in front, like `triage-reviewer`), and the old file is kept as
`<id>.before-agents.json.bak`. If you had changed the Coding squad's system prompts in the settings before this tab
existed, your text was carried into the Coding leader and the built-in agents the first time platoon read them (the old
files in `prompts/` are left as they were).

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
