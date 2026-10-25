# Sessions

The Sessions tab lists every conversation you have ever had with Claude Code and with [Codex](#codex-conversations),
in every folder, whether or not it is running: one list, mixed by when each was last used. This page covers the list
and everything you do with a session: open, search, rename, archive, copy text, and open its folders in an editor.
Where a Codex conversation behaves differently, [Codex conversations](#codex-conversations) says how.

## The list

The header has four tabs, **Sessions**, **Projects**, **Tasks** and **Squads** (`1` to `4`, or click them). Under Sessions,
Projects and Tasks, each has **Active** and **Archived**, with how many each holds, at the right of the line under the tabs (on a
narrow terminal, on the row under it): the left and right arrow keys switch them (or click them). After the tabs, the
header says how many tasks need you (**● 2 need you**, see [the Tasks tab](tasks-and-squads.md#the-tasks-tab)) and how
many sessions have a reply you haven't seen (**● 1 unread**), each when there are any and the line has room for it. At
its right end is how much of your plan each program has used, under its name (**Claude**, **Codex**; see
[What keeps it safe](tokens.md#what-keeps-it-safe)), shorter when the line has less room.

`?` shows **every key of the screen you are on** in a popup, with the keys that work everywhere: the line at the bottom of
the screen keeps only what fits, and `? help` is always in it. The popup scrolls when it is longer than the screen; `esc`,
the back key or `?` closes it.

platoon needs a terminal of at least **60 columns by 14 rows**. In a smaller one it says so (`platoon needs at least 60×14 —
this window is 40×12`) and waits: no key does anything but quit (`q` or `ctrl+c`), and making the window bigger brings
everything back as it was. In a session's live view nothing you type is sent while the window is too small, and the
session keeps its size until there is room to show it again.

### What a row shows

One row per conversation, built from Claude Code's and Codex's saved history, so everything you've ever run is there
whether or not it's running. A [task's](tasks-and-squads.md) session is the exception: it is reached through its task,
on the Tasks tab or its project's screen, and is on neither list here, nor in their counts, the search or the unread
count, nor archived by auto-archive. Each row shows the name, directory, branch, model, a status, how full the context is, when it was last
active, and a one-line description. Newest activity first.
Each conversation takes two lines, its name line and its prompt line; the selected one is highlighted across both, with a
`▎` in front of each. Conversations that only ever held slash commands are hidden.

**CONTEXT** says how full a conversation's context is, as a share of its model's window: `62%`, in the warning colour
above 80% (time to start fresh or let Claude Code compact). The figure is Claude Code's own when its status line has
reported one, else platoon's: the last reply's input (what it read from the cache, what it wrote to it, and the rest)
over the window. The window is 1,000,000 tokens for the models Claude Code gives 1M (Opus 4.7 and newer, Sonnet 5.5,
Haiku 5.5, Fable), when the model's name says so (`[1m]`) or when the conversation has already gone past 200,000; else
it is 200,000, as it is for every model when `CLAUDE_CODE_DISABLE_1M_CONTEXT` is set (in platoon's environment or in
the `env` of Claude Code's `settings.json`). It is left blank for a conversation with no reply yet. A project's screen shows it too. A Codex conversation's window
is the one Codex itself wrote down for it, so its figure is never a guess about the window.

Status is `working`, `done` (the session finished and you haven't looked yet), `idle` (running, waiting for you: `●`),
`Background`, or `moved away` (see [Background sessions](#background-sessions)). A conversation nothing runs right now
shows a hollow `○` and no status: `enter` picks it up from its saved history.

The name shown is, in this order: the one you gave it with `r` (platoon's own), then one you typed with `/rename` inside
Claude, then, when the [smart features](tokens.md) are on, a short name platoon made up (marked with a quiet `✦` at the end
of the name, or `*` on the Linux console or when `PLATOON_GLYPHS=plain`, so you can tell it from one you chose), and
otherwise the folder's name. A [task's](tasks-and-squads.md) session has the task's number in front of its name: `#5 task-5`.

The folder always stays visible in the DIR column: when the path is too long for it, the start is cut and the end (the
folder itself) kept. Folders you added to a conversation (with `/add-dir` or `--add-dir`) follow its own, separated by
commas (`~/repos/shop, /`); where they don't all fit, only its own folder is shown and the rest are counted
(`~/repos/shop, +1`). The quick filter finds a conversation by an added folder too.

With a summary made, the selected row's summary shows in two lines just above the key hints (left out on a terminal too
short to spare them).

Conversations in a [project](projects.md) show it in a **PROJECT** column after the name.

### How wide the columns are

NAME and DIR hold as much as the rows on show need and no more, up to 60 columns each. A value longer than that is cut: a
name loses its end (`a very long name…`), a path its start (`…/repos/shop`). BRANCH, MODEL, STATUS, CONTEXT and LAST
hold what they always did.

Whatever a wide terminal has over once every column has what it needs is then **shared equally between the columns**, so
the row fills the screen and LAST sits at the right edge; the odd cell that does not divide evenly goes to the leftmost
columns. Each value stays at the left of its column, so a short one simply leaves a wider gap after it than a long one,
and a short figure under CONTEXT has its share of the spare width as clear air after it. Every column ends with at least one blank cell, so no two ever touch.

On a narrow terminal, where the content already wants more room than there is, nothing is over to share, and columns go
as it narrows: BRANCH first, then MODEL, then CONTEXT, then DIR; NAME (and the prompt line under it), STATUS and
LAST stay to the end. NAME and DIR share whatever room is left.

### Keys

| Key | Does |
| --- | --- |
| `enter` | open: shows it live if platoon is running it, otherwise resumes it from its saved history in its own directory; a `Background` one is opened with `claude attach`; a Codex one open in another Codex window is [left where it is](#codex-conversations) |
| `n` | new session: pick a folder; with Claude Code and Codex both installed, then [which one](#new-sessions-in-claude-code-or-codex) |
| `a` | archive the selected conversation (on the Archived tab: restore it) |
| `m` | put the selected conversation in a [project](projects.md), move it to another, or take it out ("No project") |
| `h` | [hand this work on](projects.md#handing-work-on-to-a-new-session) to a new session in the same project (needs the smart features) |
| `t` | [draft tasks](tasks-and-squads.md#making-tasks-from-a-conversation) from this conversation: the model for smart features (Claude's or Codex's) reads it in the background (the row says **Drafting tasks…**) and the tasks it suggests open in a popup, where you change them and make them, or make nothing (needs the smart features) |
| `v` | open the conversation's folders in [your code editor](#opening-the-editor-v) |
| `1` to `4` | switch between the Sessions, Projects, [Tasks](tasks-and-squads.md#the-tasks-tab) and [Squads](tasks-and-squads.md#the-squads-tab) tabs (or click them); the current tab is underlined |
| `←` `→` | switch between Active and Archived |
| `/` | [search](#search): narrow the list as you type, or find a conversation by the words you typed in it |
| `r` | [rename](#renaming) the selected conversation |
| `s` | [settings](settings-and-keys.md) |
| `?` | every key of the screen you are on |
| `↑` `↓` (or `k` `j`) `pgup` `pgdn` `g` `G` | move by one, by a screenful, or to the first/last (`g`/`G`); the mouse works too. The same keys move in every list: Projects, a project's panes, Tasks, and the settings |
| `q` | quit (sessions keep running). Only from the top-level lists: on a project's or a task's screen, `q` goes back one level |
| `ctrl+space` or `esc` | back one level, from anywhere: see [Keys](settings-and-keys.md#keys-back-and-quit) |

Count badges (beside Active and Archived) are drawn with round ends in terminals that ship their own symbol glyphs
(Ghostty, Kitty, WezTerm) and as padded squares elsewhere, so nothing shows as an empty box. Set `PLATOON_BADGES=rounded`
or `PLATOON_BADGES=square` to choose yourself.

## Search

`/` opens a search dialog over the list. It has two kinds of search, and a third when the [smart features](tokens.md) are
on; `tab` (or a click on its name) switches between them and keeps what you typed.

- **Quick filter** (the one it opens on): narrows the list as you type, by name, folder, branch and description. `enter`
  keeps the filter and closes the dialog; `esc` clears it and closes. (In the list, `esc` also clears a kept filter.) The
  dialog lists nothing until you type: with an empty box the results area is empty (the list behind still shows
  everything, and `enter` just closes the dialog), and the narrowed rows appear with the first character and go away again
  if you delete it. The words you typed are marked in each row; when they are only in something the row doesn't show (its
  branch, a folder added to it, its first message or summary), the row shows that part, dimmed, in place of its folder.
- **Full-text**: finds conversations by the words you typed in them, in every conversation, archived ones too. Every word
  must be there, in any order and in any case, and a word matches inside a longer one (`migrat` finds `migration`). Words
  spread over different messages of the same conversation count. Each result shows the conversation's name and, under it,
  the matching message with your words in the accent color. `↑` `↓` (or `PgUp` `PgDn`, or the wheel) pick a result and
  `enter` (or a second click) closes the dialog and selects that conversation in the list, on the Archived tab if that is
  where it is. Only what you typed is searched: the answers and the output of tools are not. Codex conversations are
  searched the same way (what you typed in them, not Codex's own text around it).
- **Describe it** (only while "Enable features which require tokens" and "Find a session by description" are both on):
  for when you remember what a conversation was about but not the words. Type a few words about it and press `enter`:
  platoon picks about 50 conversations that share words with your description (their names, summaries and what you typed;
  topped up with the most recently used when fewer than 10 do), sends the model for smart features (Claude or Codex)
  one short list of them and asks for the best
  matches, and shows up to 10 with a one-line reason each. Nothing is sent while you type, only when you press `enter`.
  `esc` calls the request off (and a second `esc` closes the dialog); `enter` on a result opens it as in Full-text;
  `ctrl+w` searches wider (about 200 conversations) after telling you roughly how many tokens that is and waiting for
  `enter`. It is your own request, so it goes ahead even when the plan is used above the pause point that holds the
  background naming back; it still needs the switches, the daily limit and no back-off, and says plainly if one of them
  stops it. Words in generated names and summaries are found by the Quick filter and by Full-text too.

Results come best first: conversations whose name has all the words, then those where one single message has all of them,
then those where more messages mention them, then the most recently used. `ctrl+u` empties the box.

Behind the scenes platoon keeps a small list of the messages you sent in `~/.cache/platoon/prompts.jsonl`
(`$PLATOON_CACHE_DIR`). It reads only what is new at the end of each transcript, in the background, so a search never waits
for it and starting platoon is not slower; the first time it reads everything once, and while it does the dialog says
"Still reading your conversations…" if nothing is found yet. Deleting the file is harmless: it is rebuilt. Nothing here
asks an AI or uses any tokens.

How much a search lists and how much of each message it keeps are [settings](settings-and-keys.md#search).

## Renaming

`r` on a conversation opens a small box with its current name: `enter` saves, `esc` cancels, and an empty name gives the
automatic one back. A name you give is kept in platoon's own file, `~/.config/platoon/names.json` (`$PLATOON_CONFIG_DIR`),
never in Claude Code's or Codex's files, and other things in that file are left alone when platoon writes it. It shows in the NAME column
and is found by both searches.

## Codex conversations

When OpenAI's Codex CLI is on this computer, its conversations are in the list too, mixed in with Claude Code's by when
they were last used. platoon reads them from Codex's own folder (`~/.codex`, or `$CODEX_HOME`): the saved sessions in
`sessions/` and the names Codex gives them in `session_index.jsonl`. A Codex row looks like any other:

- **MODEL** is the model's name (`GPT-6-Luna`), or `Codex` before the first reply (where a Claude Code row shows `-`).
- The name Codex gave the conversation is its description line, as Claude Code's own titles are.
- **CONTEXT** comes from the figures Codex writes down after each reply.
- It is `working` while Codex is in the middle of a reply, and `idle` (`●`) while a Codex window has it open and is
  waiting for you. Codex keeps a lock on a conversation while a window has it open; platoon only looks at who holds it
  and never takes it.

Archive, projects (`m`), rename (`r`), both searches, `v` and auto-archive work as for Claude Code's conversations.
`enter` opens it under platoon with `codex resume <id> --no-daemon`, in its own folder, live in platoon like any session
(`--no-daemon` keeps it in the process platoon started, rather than in Codex's shared background server). If Codex
hasn't been told to trust that folder yet, Codex asks you there, in the session: answer it as you would in a terminal.
`platoon open <id>` works for a Codex id too.

Codex lets only one window have a conversation open, and platoon keeps to that: a conversation **open in another Codex
window** (in another terminal, or in Codex's own background server) shows as running, and `enter` only says
`"repo" is open in another Codex window. Close it there to open it here.` platoon never opens it a second time, never
stops it, and won't archive it while it is open there (`a` says so); archiving a project leaves it out.

The [smart features](tokens.md) (generated names and summaries, "Describe it"), the
[hand-off](projects.md#handing-work-on-to-a-new-session) (`h`) and [drafting tasks](tasks-and-squads.md#making-tasks-from-a-conversation)
(`t`) work on Codex conversations as on Claude Code's: they read what you typed and a line of Codex's last reply, never
a tool's output. The name Codex gives a conversation stays its description line, as Claude Code's own title does, and
platoon's generated name is its name. A Codex conversation is handed on to a new **Codex** session. A
[task](tasks-and-squads.md#tasks-on-codex) runs in Codex when its squad's model is one of Codex's; its session is
reached through its task, like any task's.

### New sessions in Claude Code or Codex

With both installed, `n` asks after the folder: **Start in Claude Code or Codex?** The one you chose last time is starred
and already picked, so `enter` starts it again; `↑` `↓` (or a click) pick the other, and `esc` starts nothing. Your
choice is remembered (`new_session_agent` in the [settings file](settings-and-keys.md#the-settings-file)). With only one
of them installed (or one [switched off](settings-and-keys.md#switching-a-program-off) in the settings) there is no
question: that one starts. `platoon open` with no id starts the one you chose last.

A new Codex session starts as `codex --no-daemon` in the folder, under platoon, like any session. Codex saves a
conversation only when you send your first message, so until then its row stands for the session itself: its folder's
name, `Codex` under MODEL, running, and in its project as soon as Codex has opened it (after Codex's own questions,
such as an update or trusting the folder). `enter` opens it; archiving, renaming, `m`, `h` and `t` wait for that first
message (they say so). Once Codex saves the conversation, the row shows it: its branch, model and the name Codex gives
it. A Codex session started outside platoon that hasn't been sent anything yet shows the folder its process works in.

## Background sessions

Claude Code can run a session in the background on its own (`claude --bg`, `claude agents`). platoon reads those from
Claude's registry (`~/.claude/sessions/*.json`, `"kind":"bg"`) and shows the row with the status `Background` (on the
Active tab) instead of `idle`. `enter` (or `platoon open <id>`) opens it with `claude attach <id>` inside a platoon session
window, never `claude --resume`, which Claude refuses for a session it is running. Leaving the window or quitting platoon
ends only that window: the background session keeps running.

platoon never stops or archives one: the `a` key says so and does nothing, auto-archive skips it, and idle stop never
applies to it. To end one yourself use Claude's own `claude stop <id>`. A background session is often a copy of another
conversation, so you can see two rows with the same title; platoon doesn't try to tell them apart.

When you move a session you were running in a terminal to the background, Claude Code carries on in a new background
session and leaves the old process waiting (its registry entry gets a `parkedJobId`). platoon shows the old row as `moved
away` (a hollow `◇`) instead of `working`, and `enter` (or `platoon open <id>`) opens the background session it moved to.
Once that background session has ended, the row is an ordinary idle conversation again. The waiting process is never
counted as a second copy, never offered for "Take over?", and idle stop leaves it alone.

## Archive

Archiving is platoon's own (Claude Code has no archive): it moves a conversation to the Archived tab and stops it if
something is running it. It asks first if the conversation is in the middle of working, or if it is running in a terminal
platoon didn't start (that copy isn't platoon's to end unasked). The conversation itself is untouched, and `enter` on an
archived one restores it and opens it. The list of archived ids lives in `~/.config/platoon/archive.json`
(`$PLATOON_CONFIG_DIR`, or `$XDG_CONFIG_HOME/platoon`).

Stopping a running conversation takes about a second, and platoon doesn't make you wait for it: the row leaves the
list (and the Active count drops) the moment you press the key or answer the question, and the stop runs in the
background. The conversation shows up under Archived once the process has really exited, so a conversation is never
archived while it is still running. If it hasn't exited after about 5 seconds, the row comes back with a notice
(`Couldn't stop "name": ... Try again in a moment.`) and nothing is archived. While a stop is under way you can carry on as
usual (open, rename, filter, restore or archive other conversations). Quitting platoon then waits up to 3 seconds for it,
so the archive gets written, and leaves anyway after that; a conversation whose process is still running at that point
stays in the list.

## Automatic clean-up

Two things happen on their own, so you don't have to think about them. Nothing is ever deleted.

| What | When | What you see |
| --- | --- | --- |
| A session platoon runs is stopped | nobody has had it open, it hasn't been working, and it has printed nothing, for 2 hours | nothing: the row stays in the list like any conversation, and `enter` resumes it from its saved history |
| A conversation is archived | nothing is running it (a `Background` session counts as running) and it has had no activity for 2 days | a one-line notice (`Auto-archived 143 sessions idle for 2+ days. Find them in Archived.`) that stays until you press a key; the first time you run platoon, many old conversations can be archived at once |

Details worth knowing:

- A session is only stopped when Claude Code itself says it is idle. If anything is running under it (a dev server, a
  background job) it is left alone, because stopping the session would end those too. Sessions started in a plain terminal,
  and anything with a window open on it, are never stopped.
- For a Codex session, idle is what Codex's saved session says (its last reply has finished), and its MCP servers are
  the ones in Codex's `config.toml` (`[mcp_servers.<name>]`, read only; one switched off with `enabled = false`, or reached
  by `url`, has no process). Anything else under it keeps it running, as for Claude.
- The exception is MCP servers: the helper programs Claude starts with every session (the ones you added with
  `claude mcp add`, put in a project's `.mcp.json`, or that come with a Claude Code plugin). Almost every real session has
  some, and they are closed when the session stops and started again when you open it, so by default they don't count as
  "something running". platoon works out which processes are MCP servers from your Claude Code configuration
  (`~/.claude.json`, or the one inside `$CLAUDE_CONFIG_DIR`; `.mcp.json` in the session's folder and its parent folders;
  enabled Claude Code plugins), re-reading it at every check. A process only counts as one if Claude started it directly
  and its command matches a configured server (and everything a launcher such as `npx` or `docker run` starts under it is
  treated the same way). A dev server or job Claude started from a Bash tool is deeper than that, so it still keeps the
  session alive, and so does anything platoon is not sure about, or can't read the configuration to check. Some MCP
  servers don't exit when Claude does: after a stop, platoon asks the ones it saw before to exit too, and never any other
  process.
- Auto-archive never touches a conversation that has a process running, the one you are looking at, or a task's session
  (a finished task moves to the Tasks tab's Archived list by itself instead: see
  [the Tasks tab](tasks-and-squads.md#the-tasks-tab)). Opening or
  restoring a conversation counts as activity, so restoring an old one sticks: it is not archived again until it has sat
  unused for the full time.
- Restoring is the same as always: the Archived tab, `a` (or `enter` to restore and open).
- Both times are on the [settings screen](settings-and-keys.md#general): press `s`. Whether MCP servers count is only in
  [`settings.json`](settings-and-keys.md#the-settings-file) (`"idle_stop_ignores_mcp": false` makes them count).

## Big histories

platoon keeps an index of your transcripts in `~/.cache/platoon/index.json` (Codex's in `codex-index.json` beside it) and
only re-reads files that changed, in the background, so it opens at once even with thousands of conversations (a small `indexing N…` shows in the header while it
catches up). Deleting the file is harmless: it is rebuilt. The words for the [full-text search](#search) (and, for the
smart names, one line of the last reply) are kept the same way in `prompts.jsonl` next to it, by remembering how far
into each transcript it has read.

## Sessions started in platoon keep running

Each one runs under a small background host, so quitting platoon (or closing its terminal) never ends it. The
[back key](settings-and-keys.md#keys-back-and-quit) (`ctrl+space`) leaves a session without stopping it. A session nobody
has looked at for a while does stop itself (see [Automatic clean-up](#automatic-clean-up)); `enter` picks the conversation
up again.

Inside a session, every key goes to Claude Code or Codex but one: the back key, which the line under the session names
(`ctrl+space back to list`). It can be changed in [`keys.json`](settings-and-keys.md#keys-back-and-quit).

## Copying text from a session

platoon takes over the mouse while you look at a session (that is what makes the wheel scroll it and the
`ctrl+space back to list` hint clickable), so the terminal's own drag-to-select does not get the drag. platoon does the
selecting itself instead:

| Do | Result |
| --- | --- |
| Drag with the left button | highlights the text under the drag; letting go copies it (like tmux) |
| Double-click | selects a word and copies it; drag on after the second click to add more words |
| Triple-click | selects the whole line (all the rows of it, if the terminal wrapped it) and copies it |
| Click | clears the highlight; copies nothing |
| Hold `shift` and drag | the terminal's own selection, as in any terminal (in Ghostty, Xfce Terminal and most others) |

A note in the status bar says what was copied (`Copied 3 lines`, `Copied 12 characters`), or that nothing could be, in
which case hold `shift` and drag.

- The selection is of what is on screen, in reading order: the rest of the first row, whole rows in between, the start of
  the last. Rows the terminal wrapped are joined without a line break, real line breaks stay, trailing spaces are dropped,
  and a selection of nothing but blanks copies nothing. Dragging past the edge of the session sticks to that edge (it does
  not scroll).
- A word is letters, digits and the characters that live inside paths and URLs (`_ - . / : ~ @ % + = # ? &`), so a path, a
  URL or `--flag=value` is one word. Quotes, brackets, commas, `|`, `$` and box-drawing characters end a word, and a full
  stop, colon or question mark right at its end is left off unless you clicked on it.
- The highlight is drawn in a color made from your theme (its background moved toward its accent; Terminal colors draws
  selected text reversed). It goes away when you type, paste, scroll the wheel, resize the window or leave the session. The
  wheel scrolls as before, and turning it drops the selection.
- The text goes to the **system clipboard and the primary selection** (so middle-click pastes it, too), through platoon
  itself: nothing like `xclip` or `wl-copy` needs to be installed. It is also sent to the terminal as an **OSC 52**
  sequence, which Ghostty, kitty, WezTerm and iTerm2 turn into a copy of their own, and which is what makes copying work
  over SSH. Terminals without OSC 52 (Xfce Terminal, for one) ignore it.
- **On X11** the program that put text on the clipboard is the one that hands it to whoever pastes, so what platoon copied
  stays pasteable while platoon runs and goes away when it quits, unless a clipboard manager (Xfce's Clipman, for example,
  if it is running) took a copy. On Wayland a small background process does the same. Copies made through OSC 52 belong
  to the terminal and stay.
- `PLATOON_CLIPBOARD` chooses where copies go: `system` (the default: system clipboard and OSC 52), `osc52` (only the
  terminal sequence) or `none` (nothing is copied; selecting still highlights and says what was selected).
- Text over 512 KiB is cut at that size, and one over about 100 KB is not sent as OSC 52 (some terminals refuse sequences
  that long), only to the system clipboard; the status bar says when either happened.

`platoon open` in a regular terminal does not take over the mouse: platoon passes the session's output straight through,
so the terminal's own selection and copy work as usual.

## See a session live from a regular terminal

```
platoon open                  # start a new session in this directory (in the program you chose last with n)
platoon open a1b2             # join a session by (a prefix of) its id, resuming it if nothing is running
platoon open --project shop   # start a new session in this directory, in a project (by its name)
```

The session is shared, tmux-style: every window shows it live and any of them can type. The session is sized to the
smallest window attached. The [back key](settings-and-keys.md#keys-back-and-quit) (`ctrl+space`) detaches; the session
keeps running. A session running in a plain `claude` platoon doesn't own can't be shared, so opening it asks before ending
it and reopening the same conversation from its saved history. A Codex conversation open in another Codex window is never
ended: `platoon open` says where it is and stops.

## Opening the editor (`v`)

Press `v` anywhere in platoon's lists to look at the work in a code editor of your own:

| Where | What it opens |
| --- | --- |
| a conversation's row (Sessions tab, a project's screen) | its folder, and the folders added to it with `/add-dir` |
| a task's row or its screen | the task's copy of its folder (its worktree), or its folder as it is when that isn't in git |
| anywhere else (the Projects list, the notes, an empty list) | a folder you pick, in the same picker as a new session |

In a session's live view every key goes to the session (but the back key): press the [back key](settings-and-keys.md#keys-back-and-quit)
(`ctrl+space`) to go back to the list, then `v`. Typed into a box (search, a name, the folder picker), `v` is just a
letter.

`v` opens the work in an editor this computer already has, one that shows a whole folder with a file tree. platoon
installs none of its own and only offers the ones it finds installed: on your `PATH`, or where they install themselves
(JetBrains Toolbox's scripts in `~/.local/share/JetBrains/Toolbox/scripts`, snaps in `/snap/bin`, `/opt/<app>/bin`,
`~/.local/bin`; on macOS the apps in `/Applications` and `~/Applications`). When it finds none, `v` says so in one line
at the bottom ("No editor found: install ...") and nothing opens.

| Editor | How it opens | Several folders |
| --- | --- | --- |
| VS Code, Cursor, Zed, Sublime Text, Kate | a new window of its own, beside platoon | all of them in that one window |
| IntelliJ IDEA (and Community), PyCharm (and Community), WebStorm, GoLand, RustRover, CLion, PhpStorm, Rider, Android Studio | a window of its own | each in a window of its own, at most three (it says so when there were more) |
| Helix, and **your own Neovim setup** (only when you have `~/.config/nvim`) | here, in platoon's terminal, with your own settings and environment: platoon's screen goes, the editor takes the whole terminal, and closing it brings platoon back exactly where you were | the first one (it says which it left out) |

A JetBrains IDE that is already running may open the folder in a window it has open, or ask: its command line has no
documented way to ask for a new window (JetBrains' "Command-line interface" help lists none), so that is up to the IDE's
own setting (Settings, Appearance & Behavior, System Settings, "Open project in"). platoon never changes an IDE's
settings.

Editors without a file tree (vi, Vim, nano, Micro, gedit, Mousepad) aren't offered here; they stay in the
[notes' list](projects.md#editing-the-notes) (`e`).

Which one opens: your **favourite**, the first one found until you choose one (Settings, Editors: "Editor for folders (v)").
With "Always ask which editor" on, `v` shows the list every time, the favourite starred and picked (enter opens it);
choosing another there makes it the favourite. A favourite that is no longer installed opens the first one found instead,
and platoon says so once. Sessions keep running meanwhile. A few things happen first when needed, whichever
editor it is:

- **The task's session is still working** on it: platoon asks before opening, since what you change may clash with it.
- **The task's copy of the folder was removed** after the work (Done or cancelled): it is made again from its branch first, as
  when you open its session again. While an editor in platoon's terminal is open, platoon never removes a worktree it has
  open.

An editor with a window of its own starts beside platoon and is left alone: platoon stays on screen, and closing platoon
doesn't close it. platoon can't tell when that window closes, so **a task's copy of the folder opened in one is kept** after the
task is Done or cancelled, and platoon says so: remove it with git (`git worktree remove <folder>`) when you no longer
need it. An editor in
platoon's terminal needs none of this: platoon waits for it to close.
