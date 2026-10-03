# Features that use tokens

Most of platoon spends nothing: the list, search by words, projects, notes you write yourself, tickets and themes all work
without asking Claude anything. A few features do ask Claude, which uses your own Claude plan. They are **off by default**.

## The features

Four features have a switch each. They run only while **Enable features which require tokens** (the master switch, in the
settings on the **Smart features** tab) and their own switch are both on:

- **Smart names and summaries**: gives each conversation a short name and a one- or two-sentence summary (shown under the
  list for the selected row, found by the search), and keeps them up to date as the conversation goes on.
- **Find a session by description**: the [Describe it](sessions.md#search) search.
- **Project notes**: keeps a project's notes up to date by itself, a few minutes after its sessions stop; see
  [Notes that keep themselves up to date](projects.md#notes-that-keep-themselves-up-to-date). This one is on as soon as
  the master switch is.
- **Project descriptions**: the one line under a project's name saying what it is; see
  [The line that says what a project is](projects.md#the-line-that-says-what-a-project-is). On as soon as the master
  switch is, too.

A few more have no switch, because they only ever run when you ask for them then and there. They still need the master
switch on and still count against the daily limit, like everything else:

- The [hand-off](projects.md#handing-work-on-to-a-new-session) (`h`).
- The [squad commander](tickets-and-squads.md#handing-a-ticket-to-a-squad)'s check when you hand a ticket to a squad.
- [Drafting a squad or an agent](tickets-and-squads.md#making-your-own) from your description.
- `platoon tokens test` (see [below](#platoon-tokens)).

A squad's own work is different: its leader and agents are ordinary Claude Code sessions. They use your plan like any
session you run yourself, and platoon's daily limit does not cover them; the tokens a squad used show on its ticket (see
[What the squad has done](tickets-and-squads.md#what-the-squad-has-done-and-the-tokens-it-used)).

## Turning them on

`platoon setup` asks once, in a terminal, with the default "no". It lists the features and says that platoon would send
Claude the prompts you typed plus one line of Claude's last reply for each session (for a project's notes, more of that
reply), in small batches, and that you can switch it off any time (`s`, then the Smart features tab, then the master
switch). Without a terminal setup asks nothing, leaves everything off and prints one line saying how to turn them on.

Turning a switch off stops a call already in progress and throws its answer away; names and summaries already saved stay.

### The first pass, in `platoon setup`

After a yes (and also when you said yes earlier and nothing has been named yet), setup shows a local estimate ("3 active
sessions to name, about 2,100 tokens, roughly 1 call (an estimate, made on this computer)") and names your active
sessions right there, newest first, with a one-line progress display. `Ctrl+C` skips it: platoon carries on in the
background while it is open. Setup then asks about archived sessions, showing how many there are and what naming them
would roughly cost, with "skip" as the default and "slowly in the background" as the other choice (the setting `Archived
sessions`). Without a terminal there is no scan, just one line. None of this ever changes what setup does or its exit
code, or stops platoon from starting.

## What is sent

For each conversation: its first message, its last few messages (8 by default, each cut to 300 characters), the name and
summary platoon made last time (if any) and one line (200 characters) of Claude's last reply. The conversations are
numbered 1, 2, 3 in the request (never their ids or folders), up to 30 to a call. Never tool output, file contents, or
anything else.

The description search sends, for about 50 conversations, the name, the folder's own name and the summary (or, for a
conversation with none, a little of its first and last message) and asks for the best matches.

Every request tells the model that the text is data to read, not instructions, and to answer with JSON only; what comes
back is checked strictly (unknown numbers, wrong types and empty text are ignored, names are cut to 60 characters and
summaries to 300) and every control character and escape sequence is removed before it is stored or drawn. A conversation
the model skips is tried in a later batch at most twice, then left until it changes.

What the project notes, the project description and the hand-off send is described with each of them on the
[Projects](projects.md) page.

## When

The naming work runs only **while the platoon window is open**, on a background worker inside platoon: nothing keeps
running when you quit (the hosts that keep your sessions alive do no naming), and it catches up the next time platoon
starts. (Project notes are different: their helper runs whether platoon is open or not.)

One call at a time, in this order: a description search you asked for, conversations that hit the refresh trigger (5 new
substantive messages, i.e. not "yes", "continue", "ok", "go ahead", "thanks" or anything under 10 characters, and quiet for
60 seconds), a conversation you just took out of the archive or opened, first-time names for active conversations (newest
first), then archived ones if you chose that. Archived conversations may use at most half (a setting) of what is left of
the day's limit, so the conversations you are using always have room. The header shows `✦ Indexing 12 of 240` while there
is work.

## Where it is kept

Names and summaries are in `~/.config/platoon/summaries.json` (`$PLATOON_CONFIG_DIR`): per conversation the name, the
summary, how many messages it covers, the model and the time, never a message you typed or a reply from Claude. It is
written atomically, kept as it was if platoon can't understand it (a `.bak` copy first), other things in it are left alone,
and it is in the config folder (not the cache) because making it again costs tokens. platoon never writes to Claude's own
files.

## What keeps it safe

All of this is enforced in one place before any call starts:

- **Daily limit** (default 500K tokens; every number Claude reports counts, input, cache writes, cache reads and output).
  The day is your local calendar day. Every call is added to `~/.cache/platoon/ai-usage.json` (numbers only, kept for 30
  days; nothing you typed or Claude answered is ever saved there).
- **Pause above plan usage** (default 80%), using the figure Claude Code's status line reports, for background work. With
  no figure yet, nothing pauses.
- **Back off after a failure**: not signed in, plan limit reached, no connection, too slow, or any other error stops calls
  for a while (30 seconds doubling up to 30 minutes; 15 or 30 minutes doubling up to 2 hours when signed out or over the
  plan limit). A success ends it. The footer says `Smart features paused: <reason>` once, not once per failed call, and the
  worker looks again every 30 seconds without repeating itself.
- **Small calls**: `claude -p` with your normal login, no tools, no settings, no saved session, a short system prompt and
  low effort, run in `~/.cache/platoon/ai-work` (Claude makes one empty folder under `~/.claude/projects` for it, and only
  one), with `PLATOON_AI=1` set. `MAX_THINKING_TOKENS=0` is set too, so the model doesn't spend hidden "thinking" tokens on
  a job that only needs a short answer (about half the cost of a naming call). The helper Claude process that runs there is
  platoon's own: it is never shown as a conversation (not in the list, the counts, search or naming, whatever it is doing).

The limit, the pause point, the model and how much is read are all [settings](settings-and-keys.md#smart-features), and
what platoon asks Claude for each feature can be changed too: see
[System prompts](settings-and-keys.md#system-prompts-for-each-smart-feature).

## `platoon tokens`

`platoon tokens` shows whether the features are on (the master switch and each feature's switch) and today's total, and
under it how many tokens each one used and in how many calls (the hand-off, the squad commander, drafting and
`platoon tokens test` appear there too, by name).

`platoon tokens test` makes one tiny call the same way a feature would (master switch on, the daily limit, the plan pause
and back-off all apply) and prints the reply, the tokens, the cost Claude reports and today's total; if a gate refuses it
says why and how to switch it on.
