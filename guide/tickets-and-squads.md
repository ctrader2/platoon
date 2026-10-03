# Tickets and squads

A **ticket** is one piece of work in a [project](projects.md). You can keep tickets as a list, or hand one to a **squad**: a
small team of Claude agents that does the work for you in a git worktree of its own and asks you before anything other
people can see.

## Tickets

A ticket has a title, a body you write in Markdown (which also says how far to take the work), the folder or folders it
works in, the tickets it **waits for**, and where it is (its state). Each ticket has a number in its project (`#1`, `#2`,
...), which is never given to another ticket.

### The Tickets pane

The **Tickets** pane sits under Sessions on a project's screen. Open tickets come first; the Done and Cancelled ones are
kept apart under a line that says **Done and cancelled · N**, folded until you press `enter` on it. Each row says the
number, the title, the state and, for an open ticket, what it is still waiting for ("Waits for #2, #4") or **Can start**
when it is New and everything it waits for is Done (a Cancelled ticket still holds up the ones waiting for it: only Done
lets them go). These words are never cut: on a narrow terminal the title is. Narrower still, the state and what it waits
for share one column (a row says what it waits for or **Can start**, else its state), and on the narrowest that column uses
short words (**Waiting**, **To publish**, **Waits #1**, **In line**).

| Key (Tickets pane) | Does |
| --- | --- |
| `n` | new ticket: type its title, then the body opens in your editor (the same list of editors as the [notes](projects.md#editing-the-notes)). An archived project takes no new tickets |
| `enter` | open the ticket (on the Done and cancelled line: show or hide them) |
| `c` `e` `r` `f` `b` `S` `p` `w` | as on the ticket's own screen, for the ticket picked |

### A new ticket

A new ticket starts as **New**, works in the folder of the project's most recently used session (one made with
`platoon ticket add`: the folder it was run in, which for `/to-tickets` is the session's own), and its body starts from a
short template. When the project has no sessions it has no folder of its own yet ("None: a squad works in its work
folder"); `f` changes a ticket's folders. A ticket without a folder of its own can go to any squad, which then works in
the ticket's private [work folder](#handing-a-ticket-to-a-squad).

The template has a **What it should do** heading (what should work when it is finished, as behaviour, and small enough for
one fresh session), with the line **How far: local changes only (say if you want a commit, or a push and merge request).**
under it, and an **Acceptance** heading with two empty checkboxes.

**How far to take the work is said in the ticket's text**: write "a local commit", "push it and open a merge request" or
anything else there. When the text doesn't say, a squad stops at local changes: it changes the files in its worktree (or
the folder) and commits nothing; a ticket with no folder of its own stops once its result is written in its work folder.

### A ticket's own screen

`enter` on a ticket shows its number and title, its state, **From** (the session it was made in with
[`/to-tickets`](#making-tickets-from-a-conversation): its name as the Sessions tab shows it, else its first prompt), the
squad that has it ("Coding squad"), its folders, its **work folder** once a squad has had it, what it waits for, and then
its body, drawn the way the notes are; `↑` `↓` scroll it.

| Key (a ticket's screen) | Does |
| --- | --- |
| `e` | edit the body in your editor |
| `v` | open its work in [your code editor](sessions.md#opening-the-editor-v): its worktrees, its folders not in git and its work folder (also on its row in the Tickets pane) |
| `r` | rename it (its file keeps its name) |
| `c` | cancel it, after asking "Cancel ticket #N?" (`y` yes, `n` or `esc` no). When its squad's session is running, the question says it will be stopped too, and yes stops that one session (its conversation is kept). On a ticket that is Cancelled already, `c` only says so |
| `f` | its folders: add one with the folder picker, `d` takes the picked one away |
| `b` | what it waits for: tick the project's other tickets (`enter` or `space`). A ticket can't wait for itself, and a wait that would go round in a circle is refused |
| `S` (shift+s) | hand it to a squad (see [below](#handing-a-ticket-to-a-squad)). `a` does nothing on a ticket: it says `S` does this |
| `p` | let its squad publish, when it is Ready to publish |
| `w` | remove its worktree anyway, when it was kept because it has changes not yet committed (Done or Cancelled, its squad's session not running): asks first, says the changes will be lost; the branch is kept |
| `enter` | on a ticket a squad has: its squad's session, beside the ticket. On one made from a session that no squad has yet: that session, opened the way `enter` opens it on the Sessions tab |
| `o` | the session the ticket was made from (the **From** line), also when `enter` is the squad's |
| `ctrl+space`, `esc` or `q` | back to the project, on the same ticket (the [back key](settings-and-keys.md#keys-back-and-quit)) |

### States

The states are **New, Working, Waiting on you, Ready to publish, Done, Cancelled**. You never set one by hand except
**Cancelled** (`c`): handing a ticket to a squad makes it Working, the squad sets the others, and writing to the squad in
its session makes the ticket Working again (see [Writing to the squad](#writing-to-the-squad)). A ticket no squad has had
can only be cancelled, and then it stays cancelled.

### Where tickets are kept

Tickets live next to the project's notes: the list in `~/.config/platoon/projects/<id>/tickets.json` and each body in
`tickets/<NN>-<title>.md` in the same folder, yours to edit. Only platoon writes the list. Deleting a project keeps a copy
of its tickets with the rest of its backup; archiving it leaves them as they are.

### From the command line

The [`/to-tickets`](#making-tickets-from-a-conversation) skill uses the same command, so tickets are always made through
platoon:

```
platoon ticket add --project "Web shop" --title "Log in with a link" --folder ~/code/web
platoon ticket add --project "Web shop" --title "Basket page" --waits-for 1 < body.md    # the body piped in
platoon ticket list --project "Web shop"            # open ones first, then Done and cancelled
platoon ticket list --json                          # every project, with each body file's full path
```

`add` prints the new ticket's number (`#3`). The project is found by its id or its name (any case). The body comes from
`--body-file`, else from what is piped in, else the template. With no `--folder` the ticket works in the folder you ran the
command in (its full path), and each `--folder` you give replaces that; `--no-folder` makes a ticket without a folder of
its own (research or a summary of something online, say): any squad can take it, and works in the ticket's private work
folder.

A project that is archived takes no new tickets (restore it first; its tickets can still be opened and changed in
platoon). A project that isn't there (or a name two projects share), a ticket to wait for that isn't there or would make a
circle, or a folder that doesn't exist is refused with one plain line and nothing is made. There is no `--how-far`: say how
far in the body (`--how-far` is refused with a line that says so). A running platoon shows the new ticket within a second.

## Making tickets from a conversation

Type **`/to-tickets`** in a Claude Code session that is in a project (the **Tickets skill** step of
[`platoon setup`](install-and-setup.md#the-tickets-skill) adds it). Claude never starts it unasked: when a session in a
project leaves agreed work for later, Claude may offer it once, and uses it only after you say yes (after a no, it says
you can type `/to-tickets` any time and doesn't offer again in that conversation). You can add what to turn into tickets
after it, such as a plan file; without that it uses what the conversation has agreed on. It goes like this:

1. It asks platoon which project the session is in (`platoon ticket where`). If the session isn't in one, it says so in
   one line and stops: add the session to a project first (`m` on the Sessions tab), then type `/to-tickets` again.
2. It reads what kinds of work your squads do (`platoon squads --kinds`) and shapes the tickets to fit, for example a
   coding part apart from a documentation part. It never picks a squad: you do that on the ticket (`S`).
3. It drafts the tickets. Each is one thin, complete piece that fits in one fresh session (whoever picks it up has never
   seen the conversation), says what should work when it is done rather than which files change, has acceptance checks
   that would fail before the work, a **How far:** line (local changes only, unless the conversation asked for more), no
   file paths or code, and which of the new tickets it waits for. Preparation work comes first.
4. It shows them as a numbered list (title, what it waits for, what it delivers) and asks whether the sizes are right,
   whether the waits are real, and what to merge or split. **Nothing is written until you approve the list.**
5. It makes them through platoon, the ones others wait for first: one `platoon ticket add --from-session ...` per ticket,
   with the ticket's text piped in, and every working folder the session has (or `--no-folder` for a ticket that needs
   none, such as research). Then it lists the numbers platoon gave them. They are on the project's screen, in the Tickets
   pane.

The skill only runs `platoon ticket` and `platoon squads` commands, and it lists those two in its `allowed-tools`, so Claude
Code doesn't ask before running them. Claude Code clears that permission when you send your next message, so the tickets it
writes after you approve the list may still ask.

Each ticket remembers the session it came from: its screen shows **From** with the session's name, and `enter` there (or
`o`) opens that session. The commands it uses work on their own too:

```
platoon ticket where --session <session id>     # "Web shop (id …)", or one line and exit 1 when it's in no project
platoon squads --kinds [--json]                 # each squad: its name and what it can do
platoon ticket add --from-session <session id> --title "…" [--project "…"]   # in that session's project
```

`--from-session` puts the ticket in the project that session belongs to and records where it came from (`"from":
{"session", "project"}` in `tickets.json` and in `ticket list --json`). A session in no project, or a `--project` that
names a different project, is refused with one line and nothing is made. Inside Claude Code a session's own id is in
`$CLAUDE_CODE_SESSION_ID`.

## Handing a ticket to a squad

A **squad** is a small team of Claude agents that works on a ticket for you: a **leader**, which is one Claude session of
its own, and the **agents** it hands the pieces to, each with its own job and its own model (agents are shared by squads).
Every squad has the same kind of leader: its system prompt (**System prompt for squad leaders**) and its model (**Model for
squad leaders**, Sonnet unless you pick Opus or Haiku) are on the [Projects tab of the settings](settings-and-keys.md#projects),
and platoon adds to it what the squad can do, the squad's workflow and its own rules. platoon comes with four squads,
**Coding**, **Research**, **Docs** and **Summarize** (see [The built-in squads](#the-built-in-squads)), and you can change
them or make your own on the [Squads tab](#the-squads-tab).

As it comes, the Coding leader (Sonnet) hands the ticket to the **planner** (Opus), which reads the code and writes a short
plan; then to the **builder** (Sonnet), which makes the change and runs the tests; then to the **reviewer** (Opus), which
checks the change against the ticket (and that each acceptance check could have failed before it), sending it back to the
builder at most twice; then to the **verifier** (Haiku), which runs the tests and the build and reports the total of every
test result line (added up with a command). The leader waits for each of them before it decides anything, and takes the
work as far as the ticket's text says; when the text doesn't say, it stops at local changes, with nothing committed. In
the squad's session they are named after the squad and the agent, `coding-leader`, `coding-planner`, `coding-builder`,
`coding-reviewer` and `coding-verifier`, so Claude's own lines and status bar show which of them is at work.

Press `S` (shift+s) on a New ticket (in the Tickets pane or on its screen), pick the squad and press `enter`: every squad is
listed by name, with what it can do. A ticket that is not New, or that still waits for another ticket, is refused with one
line saying why, and so is a squad that has something to fix on the Squads tab. Any squad can take a ticket with no folder
of its own: it works in the ticket's work folder.

- **The commander checks first.** Before anything starts, the **squad commander** (one quick call to Claude, on Haiku,
  counted with your [smart features'](tokens.md) tokens) reads the ticket and what the squad can do, and says whether the
  squad can handle it. Meanwhile the ticket shows **Working** with "Commander is checking…", and platoon keeps going as
  usual. If the squad fits, the ticket goes on to it. If it doesn't, the ticket stays **New** and says why ("Commander:
  This is docs work; ..."); press `S` and pick the same squad again to be asked "Hand it anyway?", and `y` hands it over
  without another check (another squad gets a check of its own). If the commander can't run - today's token limit is used
  up, or smart features are paused after a failure - **nothing is handed over**: the ticket stays New and says **Session
  limit has been reached** (or "AI features are switched off" when they are). A check that fails says so, and hands
  nothing over either. Its system prompt is **System prompt for the squad commander**, on the Projects tab of the
  settings.
- **Its own worktree.** Whatever the squad, for each of the ticket's folders that is in a git repository, platoon makes a
  git worktree next to the repository, `<repo>-worktrees/ticket-<N>`, on a new branch `ticket/<N>` from where the
  repository is now, and the squad works there, never in your own checkout. A worktree already made for the ticket is used
  again; if the branch name `ticket/<N>` is taken by something else, platoon says so and stops. Folders outside git are
  used as they are. platoon writes down on the ticket the commit each worktree was made from. The worktree is made in the
  background (the ticket says "Making the worktree…"); if it can't be made, the ticket goes back to New with the reason,
  and one already made stays for the next try.
- **Worktrees go when the ticket is finished; branches stay.** Once a ticket is Done or Cancelled and its squad's session
  has stopped (never while it, or any session, is working in the worktree), platoon removes the worktree folder with
  `git worktree remove` and **keeps the branch** `ticket/<N>` with every commit on it (deleting the branch is up to you). A
  worktree with changes not yet committed (changed files, or new files git doesn't ignore) is kept: the ticket's screen
  says "Kept because it has changes not yet committed.", and `w` removes it anyway after asking (those changes are lost;
  the branch stays). Cancelling a ticket with such a worktree says so in its question. A removed worktree shows as
  "Removed; its branch ticket/<N> is kept." Either way the status line says it once, one line per ticket (for example
  "Removed #7's worktree (its branch ticket/7 is kept)." or "Kept #7's worktree: it has changes not yet committed (w on the
  ticket removes it)."). Continuing the squad later (enter on the ticket, or writing to its session) makes the worktree
  again from its branch, at the same place, before the squad goes on. platoon only ever removes the folder it made for
  that ticket, when git lists it as that repository's worktree on the ticket's branch.
- **Its own work folder.** Every ticket handed to a squad gets a private work folder, kept with the ticket in
  `~/.config/platoon/projects/<id>/tickets/<NN>-work/` (platoon never removes it). The squad starts in the ticket's first
  folder (its worktree, when it has one), with its other folders and the work folder beside it, and writes what it makes
  for the ticket rather than for the project there, never in your repository: Research's `report.md`, notes, drafts. A
  ticket with no folder of its own has the work folder as its folder: the squad starts there.
- **Its own session.** The leader starts in a platoon session of its own (it keeps running when platoon closes), named
  after its squad (`Coding squad · #N title`), with the ticket as its first message and the project's notes given to it as
  context. It belongs to the ticket only: it is never listed on the Sessions tab, in the project's sessions, in searches or
  counts, and it never joins the project.
- **Beside the ticket.** Opening a ticket that has a squad shows the ticket on the left and the squad's live session on
  the right. What you type goes to the session, as in any open session, and `ctrl+space` goes back to the project. The
  ticket side follows along: its state, and what the squad last said. On a terminal narrower than 100 columns the session
  gets the whole width, with one line about the ticket above it. When the squad's session isn't running (it finished, or
  platoon or the computer restarted), the right side says so, and `enter` continues the same conversation.

### How many squads work at once

At most 3 squads work at the same time, across all your projects (**Squads at work at once** on the Projects tab of the
settings, 1 to 10). A squad counts while its ticket is Working and its leader's session is running; one waiting on you,
ready to publish, done or cancelled doesn't. When you hand a ticket over (the commander said it fits, or you said "hand it
anyway") and every place is taken, the ticket shows **Working · waiting for a free squad** and starts by itself when a
place frees, the one that has waited longest first, without taking over your screen (a notice says so). Its worktree is
made then, not while it waits. The line is written on the ticket, so it outlasts platoon: the next platoon you open starts
it. Only an open platoon starts tickets from the line; with two platoon windows open, the first one opened does it (the
other takes over when it closes), and a ticket is never started twice. To take a ticket out of the line, cancel it (`c`).
Lowering the number never stops a squad; new ones just wait.

### What the squad has done, and the tokens it used

A ticket's screen lists the members' runs in the order the leader started them, as its session shows them (not the
workflow on paper): each member by its agent name, with the short description the leader gave it, `[x]` once its result
has come back and `[>]` while it runs (`[-]` for a run whose leader stopped before it reported), and "Now: coding-reviewer"
while one is at work.

Below the ticket's details it also shows the tokens its squads used: the total, what that counts (what was sent to Claude,
what Claude wrote, and what was read again from Claude's cache, each answer once, the members' own work included), and a
line per agent - the leader and each member - added up over every squad the ticket has had. The Tickets pane shows the
short total ("1.2M tokens") on each ticket that has one. platoon reads this from the squad's session files in the
background, every couple of seconds, only for the open project's tickets that have had a squad.

### How the ticket's state follows the squad

At the end of each of its turns the leader says in one last line whether it is done, still working (members running), has
a question, is stuck, or is ready to publish, and platoon sets **Working**, **Done**, **Waiting on you** (with its question
or why it is stuck; a turn that ends without that line counts as "the squad stopped without saying why") or **Ready to
publish**.

**The squad never closes a ticket unasked**: when the work is finished it writes its short result (what was done, where,
what was not checked) and asks "Shall I close this ticket?", so the ticket shows Waiting on you with that question; only
after you say yes does it end with done and the ticket goes Done. When it is Done, the leader's result shows at the top of
the ticket's screen (it is kept in `tickets/<NN>-result.md`), and that result, and nothing else of the squad's
conversation, goes to the project's notes at their next update, as "Ticket #N (Coding squad)" (once).

While the leader is stuck on Claude's permission prompt, the ticket shows **Waiting on you** ("Waiting for your OK on a
command"); once you answer and it carries on, the ticket is Working again. The leader's session is not stopped for being
idle while its ticket is Working (its members may be at work while nobody watches); once the ticket is Waiting on you,
Ready, Done or Cancelled, it is stopped like any idle session.

### Anything other people can see asks you first

Even when the ticket asks for it: pushing, opening or merging a merge request, publishing a package or a page. platoon
refuses those commands in the squad's session until you say so, and the squad stops at **Ready to publish**, saying what it
wants to publish. Press `p` on the ticket to let it: the ticket goes back to Working and platoon types "Go ahead and
publish." into the squad's session. That go-ahead lasts while the ticket is Working or Ready; once it is Done, Cancelled,
New or waiting on you, the next push asks again. Local changes and local commits never ask.

What platoon recognises is listed in [What platoon changes](what-platoon-changes.md#squads).

### Writing to the squad

Whatever you send in the squad's session (an answer to its question, "yes" to closing, more work after Done, the go-ahead
`p` types) makes a ticket that is Waiting on you, Ready to publish, Done or Cancelled Working again, with the same squad
session: that is how a Done or Cancelled ticket is opened again (`enter` continues its session when it isn't running, then
write to it). A reopened ticket counts toward the squads at work at once like any Working one, even when that makes one
more than the number allowed for a while; tickets in the line just wait longer. A New ticket and one waiting in the line
are left alone. `c` cancels a ticket after asking; when its squad's session is running, yes stops that one session too (its
conversation is kept).

### What you can change, and what platoon keeps

A squad's workflow and its agents (their models, switches, skills, MCP servers and system prompts) are on the
[Squads tab](#the-squads-tab); the leader's system prompt and model are in the settings. The rules platoon keeps - where
the squad works and that what it writes for the ticket goes in the work folder, ask before anything others can see, the
last line of each turn, and "report back to the leader, never push" for the members - are always sent after your text.

The leader may hand work to its members, read, search and run commands; it never edits a file itself (its members do).

Nothing about the squad is added to your own Claude Code settings: the leader's session alone gets platoon's squad hooks
(`platoon hook squad-stop`, `squad-gate`, `squad-notify`, `squad-tool` and `squad-prompt`), and `platoon setup` is not
involved. See [What platoon changes](what-platoon-changes.md#squads) for how a leader is started.

## The built-in squads

- **Coding**: makes a change to code, as described [above](#handing-a-ticket-to-a-squad).
- **Research**: finds things out and writes them up, with sources. The **questioner** (Sonnet) turns the ticket into a
  short list of questions; the **researcher** (Sonnet, can use the web) answers each one from the ticket's folders and the
  web, with sources; the **report-writer** (Sonnet, can change files) writes `report.md` in the ticket's work folder (never
  in your repository): the answers, the sources and what is still unknown; the **checker** (Opus, can use the web) checks
  each claim against its source, and may send it back to the researcher once.
- **Docs**: writes or updates documentation as Markdown files in the ticket's folder (its worktree, in git). The
  **outliner** (Sonnet) reads the ticket, the existing docs and the code and proposes what to write where; the
  **tech-writer** (Sonnet, can change files) writes it; the **editor** (Opus) checks it against the code, the ticket and
  the project's style, and may send it back to the tech-writer twice. The result lists the files. Docs in Coda, Confluence
  and the like are for your own squads, using your Claude's connectors.
- **Summarize**: the **summarizer** (Sonnet, can use the web, so links can be read) reads what the ticket points at and
  writes a short summary for you; its step tells the leader that the summary is the result, so it goes on the ticket.

Only Research's researcher and checker and the summarizer can use the web. Every leader runs on the model in the settings
(Sonnet unless you change it). No built-in agent has skills or MCP servers.

## The Squads tab

The third tab, `3` **Squads**, has two lists: the **squads** and the **agents** they are made of. The title of the left
pane shows which one you are looking at (`[Squads]  Agents`); `←` and `→` on the list switch between them (or click the
title). An agent is its own thing, shared by squads: the same planner can do a step in several squads.

### A squad

On the right, in two parts:

- **About**: its name and **what it can do** in a sentence or two (the commander reads that before a ticket is handed to
  the squad, and so does its leader). The leader isn't here: every squad's leader has the same system prompt and model,
  set in the settings (**System prompt for squad leaders** and **Model for squad leaders**, on the Projects tab).
- **Workflow**, drawn as a rail: one mark per step down a line (`○` the first, `●` the others), beside it the step's
  number, its **agent** with that agent's model and switches (`files`, `commands`, `web`, and `skills` or `MCP` when it
  has them), and under it one line of what the step does. A step may **send the work back** to an earlier step, a set
  number of times at most: that is drawn as a loop on the left from the step back up to the one it returns to, with "↩ back
  to step 2 (builder), at most twice" beside it (loops inside others are drawn inside them; on a narrow screen only the
  words stay). `◉ Done` ends the rail, and **+ Add a step** is under it. The same agent can do more than one step. platoon
  writes the steps into the leader's system prompt as a numbered list, after what the squad can do and before platoon's
  own rules, and the leader follows them (platoon can't force the order: the agents run inside the leader's session). A
  squad's agents are the different agents its steps name: at most 8, and at most 12 steps.

Each step is two lines you can move to: its agent, and what it does (`enter` there changes the line). On a step, `enter`
on its agent (or `m`) picks another agent from all of them, or **+ New agent** at the bottom, which asks for a name, makes
a new agent of your own with no system prompt yet, puts it on the step and opens it on the Agents list for you to write one
(until it has one, the squad says so and can't be handed a ticket). `g` goes to the step's agent on the Agents list. `b`
says whether the step may send the work back and where to, `[` `]` move it up and down, `a` adds a step and `x` takes it
out.

In the Squads list, a built-in squad as platoon has it says nothing under its name; the others say what sets them apart:
**User Modified** or **Your own**.

### An agent

On the right: its name, its model, three switches - **can change files**, **can run commands** and **can use the web** (an
agent can always read and search; the switches add the rest, and nothing else) - its **skills** and **MCP servers**, its
system prompt, and **Used in**: every squad that uses it, with the steps. `enter` on a squad there opens it.

**A change to an agent is a change in every squad that uses it**, so the tab says where it is used; for a variant, copy the
agent (`c`) and put the copy on the steps you want. The list groups the agents as built in, **User Modified** and your own,
each with its model and how many squads use it (with only built-in agents there are no headings); **+ New agent** at the
end makes a blank one of your own.

### Skills and MCP servers

`enter` on **Skills** lists the skills installed for you (each `skills/<name>/SKILL.md` in Claude Code's folder,
`~/.claude` or `CLAUDE_CONFIG_DIR`), each ticked when the agent has it; `enter` ticks or unticks one and the list stays
open, `esc` closes it. **Type the name of a skill…** at the bottom adds one that isn't installed here, such as a skill in a
ticket's own folder (`.claude/skills` in the repository). **MCP servers** works the same way with the servers you added for
every folder (`claude mcp add`, kept in `~/.claude.json` or the one in `CLAUDE_CONFIG_DIR`; platoon reads only their
names, never what is in them). A name that isn't installed here says "not found here" but is kept: it may be there where
the squad works.

When a squad starts, an agent's skills are loaded into it from the start and it may use other skills too (`Skill` is added
to its tools), and it may use each of its MCP servers (`mcp__<name>` is added to its tools: without that, naming a server
gives the agent nothing). The skills of Claude Code's own plugins aren't listed (type their name, `plugin:skill`), and
neither are servers from claude.ai connectors or a project's `.mcp.json`.

### Moving around and changing things

`↑` `↓` move in the list and in the details; `enter` (or `tab`, or `shift+→` / `ctrl+→`) goes into the details, and the
back key `ctrl+space`, `esc` or `q` (or `tab`, `shift+←`, `ctrl+←`) back to the list. On a terminal under about 80 columns
the tab shows one pane at a time: the list, and `enter` opens the details in its place, full width; `esc` goes back to the
list, on the same entry. In the details, `enter` changes the line you are on: a name or a line of text in a small box, a
model or an agent from a short list, a system prompt in the same editors as a project's notes. `space` turns a switch on
or off. Every change is saved at once.

### What is built in

**What platoon comes with is built in**: the four squads and their twelve agents (Coding's planner, builder, reviewer and
verifier; Research's questioner, researcher, report-writer and checker; Docs' outliner, tech-writer and editor; and the
summarizer). Everything about them can be changed; the list then says **User Modified**, and `d` puts one back as platoon
has it, after asking (your version is kept as `<id>.json.bak`). They can't be deleted.

### Making your own

Press `n`: platoon asks whether to describe **a new squad** or **a new agent**, and you describe it in your own words ("a
squad that triages Sentry errors: a reader, a reproducer, a fixer"). One call to Claude, on Sonnet, counted with your
smart features' tokens, drafts it. A squad's draft uses the agents you have wherever one fits (Claude is shown each agent's
name, model, switches and a line of what it does) and brings new agents only for jobs none of them does; the draft marks
those "new with the draft", and saving the squad saves them too. Claude is also told which skills and MCP servers are
installed, and a new agent may get only those. platoon checks the draft (names, models, steps that name an agent that isn't
there, a step that sends work back to a later one) and says plainly what was wrong if it can't be used. A good draft opens
on the tab as **Draft, not saved**: change what you like, then `ctrl+s` saves it, or `d` throws it away.

`c` copies a squad or an agent, built in or your own, under a new name. `d` deletes one of your own, after asking (its file
is kept as `<id>.json.bak`); an agent a squad still uses can't be deleted, and platoon names the squads. The drafting
calls' system prompts are **System prompt for drafting a squad** and **System prompt for drafting an agent**, on the
Projects tab of the settings, next to the commander's.

### A running ticket keeps its squad

When a squad's leader starts on a ticket, platoon keeps what it was started with beside the ticket
(`tickets/<NN>-squad-<k>.json`). Changing the squad or its agents afterwards doesn't change that leader, also when it is
continued later: it goes on with the squad as it was. The next ticket gets them as they are then.

### Where squads and agents are kept

Your squads and agents, and the built-in ones once you change them, are in `~/.config/platoon/squads/<id>.json` and
`~/.config/platoon/agents/<id>.json`, one file each; a squad's steps name its agents by their id, which is made from the
name once and never changes. Keys platoon doesn't know are kept, and a file that can't be read is shown as such on the tab
and never written over.

