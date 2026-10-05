# Tasks and squads

A **task** is one piece of work, usually in a [project](projects.md). You can keep tasks as a list, or hand one to a
**squad**: a small team of Claude agents that does the work for you in a copy of your folder (a git worktree) and, unless
you tell it otherwise, asks you before anything other people can see.

(Tasks were called tickets until platoon 1.0: `platoon ticket ...` still works, the same way as `platoon task ...`.)

## The Tasks tab

The second tab, `2` **Tasks**, lists every task in play, in every project and outside any project, grouped by what it
needs from you, each group with how many it has:

- **Needs you**: its squad stopped for you. It has a question, its plan is ready for your yes, it is stuck, it is waiting
  for your OK on a command, it wants to publish something, or its tests still fail (see [Checks](#checks)). The one that
  has waited longest comes first.
- **Working**: a squad is at it (or it is being handed to one, or waits for a free one).
- **Can start**: nobody has it yet and nothing it waits for is left; also the ones that **Need a folder**.
- **Waits for**: it waits for another task to be Done.
- **Later**: the ones you put aside with `h` until their squad says something new.
- **Finished today**: Done or Cancelled in the last 24 hours. After five, the rest are folded: `enter` on the fold shows
  them.

Inside a group the latest change comes first (but for Needs you). The tasks of an archived project that nobody has started
are left out.

Each task takes two lines. The first says what it is: its project's badge (none for a task in no project), its number in
its project (so `#5` can show twice, with different badges), its title, and at the right the word for it: what it needs
(**a question**, **a plan to approve**, **a go-ahead**, **stuck**, **your OK on a command**, **tests failing**), how long
since its squad last wrote, **Needs a folder**, or when it finished. The second, quieter, says who has it and what it needs from you: the squad's
own words (its question, what it wants to publish, "wrote its plan: y goes ahead, n changes it"), the squad and its steps as
marks (`planner ✓  builder ✓  reviewer ●  verifier ·`:
done, at work now, not yet; `✗` for a run that stopped without its result) and the tokens it used, `New · S hands it to a
squad`, which tasks it waits for, or, once finished, the squad, whether its result is on the task, the branch its work is on
and the tokens. Once platoon has run the project's tests for it, the line adds **tests ✓** or **tests ✗** (after the steps'
marks; **running the tests** while they run, **tests not run** when they couldn't be). A line too long for the screen ends
with "…" after a whole word, and what the tests said is kept: the words before it are cut first.

On a terminal 100 columns wide or more, the picked task's details are beside the list: its title, where it is, which squad
has it and for how long; once a squad has had it, **Tests** and what platoon's last run of the tests said (see
[Checks](#checks)); what the squad asks (or wants to publish, or why it is stuck) with the keys that answer, or **The
plan** when its plan waits for your yes (drawn from its `plan.md`, as much as fits; the task's own screen shows all of it);
**What it has done**, a timeline with a line per run of a member (its agent, a bar for when it ran within the squad's time so far, how
long it took, and **done**, **now** or **stopped**), then the steps of the squad's workflow that haven't run yet; and **What
ran**: the last eight things that ran for it, each with its time and grouped by run: the commands the squad ran in the shell
(the leader's and its members', such as git, the tests, a build) and platoon's own steps (the copy of a folder it made and
from which branch, the squad started). Narrower, the list has the whole width, and `enter` opens the task.

On every tab the header says how many tasks need you, **● 2 need you**, when any do (the ones under Later don't count).
While nothing else is said there, the footer of the Tasks tab says how many squads are at work, **1 of 3 squads at work**.
With nothing in play, the tab says: "Nothing in play. Press n to write a task, or S on a task that can start."

| Key (Tasks tab) | Does |
| --- | --- |
| `y` | answer the squad. On a question (or a squad that is stuck, or tests that still fail): a one-line box, **Answer the Docs squad on #6**, with what it asks above it. `enter` types your words into the squad's session as your reply, as if you had written them there (the task goes back to Working once the session takes them, as described in [Writing to the squad](#writing-to-the-squad)); `ctrl+o` opens the session instead; `esc` cancels. On a plan to approve, `y` types "Go ahead with the plan." into the squad's session and the task is Working again (see [The plan step](#the-plan-step)). On a task that wants to publish, `y` is the go-ahead (what `p` does). On one waiting for your OK on a command, it opens the squad's session, where that question is. When the squad's session isn't running, `y` continues it, beside the task, and says so: type your answer there |
| `enter` | open the task, beside its squad's session when that runs (on the fold under Finished today: show or hide the rest) |
| `h` | later: put a task that needs you aside, under **Later**, until its squad says something new (then it is back under Needs you). `h` on one under Later brings it back now |
| `n` | a new task: first which project it is in, or **No project**, then its title; then its text opens in your editor. In a project it works in the folder of the project's most recently used session, as the [pane's](#the-tasks-pane) `n` does. A task in no project needs a folder to work in: the folder picker opens before it is written (leave it, and nothing is made). On a plan to approve, `n` is the change instead: a one-line box, **Change the Coding squad's plan for #5**; `enter` types "Change the plan: " and your words into the squad's session, and the squad plans again |
| `m` | move it to another project, or out of any (**No project**). It gets that project's next number and keeps the one it had for its branch (`task/5` stays `task/5`); its text, its result, its own folder and what its squads were started with go with it; its copies of folders and its branch stay as they are; what it waited for is dropped (those were the other project's tasks). Not while its squad's session is running, or while it waits for a free squad: the line says why |
| `S` | hand it to a squad (see [The hand-over](#the-hand-over)) |
| `c` | cancel it, after asking |
| `p` | let its squad publish, when it needs you for that (as on the [task's own screen](#a-tasks-own-screen)) |
| `e` `r` `f` `b` `w` `v` | as on the [task's own screen](#a-tasks-own-screen) |
| `/` | narrow the board to the tasks whose title, project or squad has what you type (it narrows as you type); `esc` clears it |
| `↑` `↓` | move (`j` `k`, `PgUp` `PgDn`, `g` `G` too) |

After `y`, `p` or `h`, the cursor stays where it was, on the next task that needs you, rather than following the task
to its new group. `1` to `4` switch tabs, `?` lists every key, and `q` quits from here as from the other top-level lists.

## Tasks

A task has a title, a text you write in Markdown (which also says how far to take the work), the folder or folders it
works in, the tasks it **waits for**, and where it is (its state). Once it has been handed to a squad it also says how much
the squad does **on its own**, and for each of its git folders how the squad **works in** it and which branch the work
**lands on**. Each task has a number in its project (`#1`, `#2`, ...), which is never given to another task.

### The Tasks pane

The **Tasks** pane sits under Sessions on a project's screen. It shows the same rows as the [Tasks tab](#the-tasks-tab),
that project's tasks only (without its badge), under the same headings, with the same two lines each; all of its Done and
Cancelled tasks are kept apart at the end under a line that says **Done and cancelled · N**, folded until you press
`enter` on it (there, each says **Done** or **Cancelled** at its right). A Cancelled task still holds up the ones waiting
for it: only Done lets them go. The word at the right of a row is never cut: on a narrow terminal the title is, after a
whole word. When the pane is too short for them all it says how many more lines are below, and scrolls as you move.

| Key (Tasks pane) | Does |
| --- | --- |
| `n` | new task: type its title, then the text opens in your editor (the same list of editors as the [notes](projects.md#editing-the-notes)). An archived project takes no new tasks |
| `enter` | open the task (on the Done and cancelled line: show or hide them) |
| `y` `h` `m` | as on the [Tasks tab](#the-tasks-tab): answer the squad (or give the go-ahead, or go ahead with its plan), put it aside for later, move it to another project. On a plan to approve, `n` is the change to it, as on the Tasks tab |
| `c` `e` `r` `f` `b` `S` `p` `w` | as on the task's own screen, for the task picked |

### A new task

A new task works in the folder of the project's most recently used session (one made with `platoon task add`: the folder
it was run in; one [drafted from a conversation](#making-tasks-from-a-conversation): that session's folders), and its
text starts from a short template. When the project
has no sessions it has no folder of its own yet, and its row says **Needs a folder**; `f` changes a task's folders. A task
without a folder of its own can still go to any squad, which then works in the task's own folder, kept by platoon (see
[Handing a task to a squad](#handing-a-task-to-a-squad)).

The template has a **What it should do** heading (what should work when it is finished, as behaviour, and small enough for
one fresh session), with the line **How far: local changes only (say if you want a commit, or a push and merge request).**
under it, and an **Acceptance** heading with two empty checkboxes.

**How far to take the work is said in the task's text**: write "a local commit", "push it and open a merge request" or
anything else there. When the text doesn't say, a squad stops at local changes: it changes the files in its copy of the
folder (or the folder) and commits nothing; a task with no folder of its own stops once its result is written in its own
folder.

### A task's own screen

`enter` on a task shows its number and title, its project (or **No project**), its state, **From** (the session it was made
from (a task [drafted from a conversation](#making-tasks-from-a-conversation)): its name as the Sessions tab shows it, else its first prompt),
what the squad said when it stopped (or, when the tests fail, what platoon says), the squad that has it, by its brief
(**Squad** Coding brief), **Tests** (what platoon's last run of the project's tests said; see [Checks](#checks)), how much the squad does
**on its own**, where it **works in** (for each folder: "a copy of shop (task/5) · lands on main", "shop, on a new branch task/5 ·
lands on main", or the folder as it is, with the copy's path under it), **its own folder** once a squad has had it, what it
waits for, and the tokens its squads used. Under a line: its squad's result (when it has one), **The plan** (when its
squad wrote one: a task that plans first; with `y go ahead   n change the plan` under it while it waits for you, and kept
there after the go-ahead), **What the squad has done** (the same timeline as beside the Tasks tab, for each squad it has
had), **What ran** (all of it, grouped by run), and then its text, drawn the way the notes are; `↑` `↓` scroll them (on a
short terminal the tokens scroll with them).

| Key (a task's screen) | Does |
| --- | --- |
| `e` | edit the text in your editor |
| `v` | open its work in [your code editor](sessions.md#opening-the-editor-v): its copies of the folders (its worktrees), its folders not in git and its own folder (also on its row in the Tasks pane) |
| `r` | rename it (its file keeps its name) |
| `c` | cancel it, after asking "Cancel task #N?" (`y` yes, `n` or `esc` no). When its squad's session is running, the question says it will be stopped too, and yes stops that one session (its conversation is kept). On a task that is Cancelled already, `c` only says so |
| `f` | its folders: add one with the folder picker, `d` takes the picked one away |
| `b` | what it waits for: tick the project's other tasks (`enter` or `space`). A task can't wait for itself, and a wait that would go round in a circle is refused |
| `S` (shift+s) | hand it to a squad (see [below](#handing-a-task-to-a-squad)). `a` does nothing on a task: it says `S` does this |
| `p` | let its squad publish, when it needs you for that |
| `y` `h` `m` | as on the [Tasks tab](#the-tasks-tab): answer the squad (or give the go-ahead, or go ahead with its plan), put it aside for later, move it |
| `n` | when its plan waits for you: say what should change in it (see [The plan step](#the-plan-step)) |
| `w` | remove its copy of the folder anyway, when it was kept because it has changes not yet committed (Done or Cancelled, its squad's session not running): asks first, says the changes will be lost; the branch is kept |
| `enter` | on a task a squad has: its squad's session, beside the task. On one made from a session that no squad has yet: that session, opened the way `enter` opens it on the Sessions tab |
| `o` | the session the task was made from (the **From** line), also when `enter` is the squad's |
| `ctrl+space`, `esc` or `q` | back to the project (or the Tasks tab), on the same task (the [back key](settings-and-keys.md#keys-back-and-quit)) |

### States

A task is in one of five states, each said in plain words:

- **Can start**: nobody has it yet, it has a folder and everything it waits for is Done. `S` hands it to a squad. A task
  like this with no folder of its own reads **Needs a folder** instead (it can still be handed over: the squad works in the
  task's own folder), and one that still waits reads **Waits for #N**.
- **Working**: a squad has it (the commander is checking, its copy of the folder is being made, it is in the line for a
  free squad, or its agents are at work).
- **Needs you**: the squad stopped for you: it has a question, its plan is ready for your yes (a task that plans first),
  it is stuck, it is waiting for your OK on a command, or it wants to publish something (push, open a merge request, a
  page), which `p` lets it do. The reason is on the task's screen.
- **Done**, and **Cancelled** (listed together under Done and cancelled).

You never set a state by hand except **Cancelled** (`c`): handing a task to a squad makes it Working, the squad sets the
others, and writing to the squad in its session makes the task Working again (see [Writing to the squad](#writing-to-the-squad)).
A task no squad has had can only be cancelled, and then it stays cancelled.

### Where tasks are kept

Tasks live next to the project's notes: the list in `~/.config/platoon/projects/<id>/tasks.json` and each text in
`tasks/<NN>-<title>.md` in the same folder, yours to edit. Only platoon writes the list. Deleting a project keeps a copy
of its tasks with the rest of its backup; archiving it leaves them as they are.

A project from before platoon 1.0 has them as `tickets.json` and `tickets/`. platoon reads and writes those names until no
squad of that project is running, then moves them to the new names once (keeping `tickets.json.bak`) and says so in the
status line: "Moved Web shop's tickets to tasks.json." A task that already had a branch keeps it (`ticket/<N>`); new ones
get `task/<N>`.

Tasks made outside any project (`n` on the [Tasks tab](#the-tasks-tab), or `platoon task add` below) are in
`~/.config/platoon/tasks.json` and `~/.config/platoon/tasks/`, with a number space of their own. They are on the Tasks tab
without a badge, and in `platoon task list` under "No project". `m` moves a task between projects, or out of them.

### From the command line

Tasks are always made through platoon, also from a script or another program:

```
platoon task add --project "Web shop" --title "Log in with a link" --folder ~/code/web
platoon task add --project "Web shop" --title "Basket page" --waits-for 1 < text.md    # the text piped in
platoon task add --title "Try the new linter"        # in the project whose sessions worked here, else in no project
platoon task list --project "Web shop"               # open ones first, then Done and cancelled
platoon task list --json                             # every project, then "No project", with each text file's full path
```

`add` prints the new task's number (`#3`). The project is found by its id or its name (any case). The text comes from
`--body-file`, else from what is piped in, else the template. With no `--folder` the task works in the folder you ran the
command in (its full path), and each `--folder` you give replaces that; `--no-folder` makes a task without a folder of
its own (research or a summary of something online, say): any squad can take it, and works in the task's own folder.
With neither `--project` nor `--from-session`, the task goes in the project whose sessions have worked in the current
folder (as platoon's index knows them); outside any project's folders it goes in no project, and when sessions of more
than one project worked there you are asked to say which with `--project`.

A project that is archived takes no new tasks (restore it first; its tasks can still be opened and changed in platoon). A
project that isn't there (or a name two projects share), a task to wait for that isn't there or would make a circle, or
a folder that doesn't exist is refused with one plain line and nothing is made. There is no `--how-far`: say how far in
the text (`--how-far` is refused with a line that says so). A running platoon shows the new task within a second.

A task can remember the session it came from, as the ones [drafted from a conversation](#making-tasks-from-a-conversation)
do:

```
platoon task where --session <session id>       # "Web shop (id …)", or one line and exit 1 when it's in no project
platoon squads --kinds [--json]                 # each squad: its name and what it can do
platoon task add --from-session <session id> --title "…" [--project "…"]   # in that session's project
```

`--from-session` puts the task in the project that session belongs to and records where it came from (`"from":
{"session", "project"}` in `tasks.json` and in `task list --json`). A session in no project, or a `--project` that
names a different project, is refused with one line and nothing is made. Inside Claude Code a session's own id is in
`$CLAUDE_CODE_SESSION_ID`.

`list --json` gives each task's `number`, `title`, `state` (the words the screen shows), `folders`, `blocked_by`,
`can_start`, `publish_wanted`, `autonomy` (`plan`, `ask` or `publish`), `plan_ready` when its plan waits for you,
`works_on` when it has been set, `reason` when the squad left one, `from` when it came from a session, and `file`.
`project` and `project_id` are `null` for a task in no project.

## Making tasks from a conversation

When a conversation has agreed on some work and left it for later, platoon can turn it into tasks for you: press **`t`**
on the session's row (on the Sessions tab, or in a project's Sessions pane), or **`alt+n`** inside the open session (it
goes back to the list first; the session keeps running). Both do the same. The chord can be changed: `draft_tasks` in
[`keys.json`](settings-and-keys.md#keys-back-and-quit). The session may be in a project or in none, and it needs a folder
platoon can find (its own, or one added to it).

1. platoon asks Claude once, in the background, over the conversation, read the way the [hand-off](projects.md#handing-work-on-to-a-new-session)
   reads it (its first message and the last twelve you sent, each cut to 300 characters, and a line of Claude's last
   reply), together with the name of each of your squads and what it can do. Meanwhile the session's row says
   **Drafting tasks…** and platoon keeps going. One conversation at a time.
2. The draft opens in a popup, **Tasks from this conversation**, as soon as you are on platoon's lists (if you are in a
   session then, a line says it is ready and the back key shows it). Each task is a block: its title, the squad
   suggested for it (one of yours that fits its kind of work, or none), the folders it works in (the session's), which
   of the new tasks it waits for, and its text: **What it should do**, a **How far:** line, and **Acceptance** checks.
   **Nothing is made until you say so.**

| Key (the draft) | Does |
| --- | --- |
| `↑` `↓` | move between the tasks (the blocks scroll; `PgUp` `PgDn` and the wheel too) |
| `x` | take the picked task out of the draft (the others that waited for it don't any more) |
| `e` | open the picked task in your editor (the same list of editors as the [notes](projects.md#editing-the-notes)): its title as the first heading, then its text. What the file says when the editor is done replaces it in the draft |
| `enter` | make every task left, then open the [hand-over](#the-hand-over) popup for each one that can start, in turn, with its suggested squad picked and its terms pre-filled. `esc` there leaves that task as **Can start** and goes on to the next; a task that waits for another of them is left for later. A line at the end says what was made |
| `k` | make every task left, as **Can start**, and stop there |
| `esc` | make nothing |

The tasks go in the session's project, or in no project when it is in none, each after the ones it waits for, and each
remembers the session it came from: its screen shows **From** with the session's name, and `enter` there (or `o`) opens
that session. After they are made, the Tasks tab (or the project's Tasks pane, when you are on its screen) shows them.

What Claude is asked for, as platoon comes: one thin, complete piece per task that fits in one fresh session (whoever picks
it up has never seen the conversation), what should work when it is done rather than which files change, acceptance
checks that would fail before the work, a **How far:** line (local changes only, unless the conversation asked for a
commit, a push or a merge request), no file paths or code, the tasks shaped to the kinds of work your squads do (a coding
part apart from a documentation part), and preparation first. You can change that in the settings: **System prompt for
tasks from a conversation**, on the [Projects tab](settings-and-keys.md#projects). platoon checks the answer (a title and
what each task should do; which of the new tasks it waits for, never in a circle; a suggested squad you have) and says in
one line when it can't use it; nothing is written then.

It uses your plan's tokens like the hand-off does: it needs the [smart features](tokens.md) on, counts toward the daily
limit, and says in one line why when it can't run. A session whose project is archived, or with no folder platoon can
find, is refused with one line too, and nothing is asked.

An older `/to-tickets` skill (which earlier platoons installed in Claude Code) keeps working, but platoon no longer
installs it: see [The old `/to-tickets` skill](install-and-setup.md#the-old-to-tickets-skill).

## Handing a task to a squad

A **squad** is a small team of Claude agents that works on a task for you: a **leader**, which is one Claude session of
its own, and the **agents** it hands the pieces to, each with its own job and its own model (agents are shared by squads).
Every squad has the same kind of leader: its system prompt (**System prompt for squad leaders**) and its model (**Model for
squad leaders**, Sonnet unless you pick Opus or Haiku) are on the [Projects tab of the settings](settings-and-keys.md#projects),
and platoon adds to it what the squad's **brief** says (what it does, its steps, when it asks you and what it checks) and
its own rules. A brief is a short Markdown file of yours. platoon comes with four squads, **Coding**, **Research**,
**Docs** and **Summarize** (see [The built-in squads](#the-built-in-squads)), which work as they are; you change one, or
make your own, by changing its brief on the [Squads tab](#the-squads-tab).

As it comes, the Coding leader (Sonnet) hands the task to the **planner** (Opus), which reads the code and writes a short
plan; then to the **builder** (Sonnet), which makes the change and runs the tests; then to the **reviewer** (Opus), which
checks the change against the task (and that each acceptance check could have failed before it), sending it back to the
builder at most twice; then to the **verifier** (Haiku), which runs the tests and the build and reports the total of every
test result line (added up with a command). The leader waits for each of them before it decides anything, and takes the
work as far as the task's text says; when the text doesn't say, it stops at local changes, with nothing committed. In
the squad's session they are named after the squad and the agent, `coding-leader`, `coding-planner`, `coding-builder`,
`coding-reviewer` and `coding-verifier`, so Claude's own lines and status bar show which of them is at work.

### The hand-over

Press `S` (shift+s) on a task that can start (in the Tasks pane or on its screen). A task a squad already has, or that
still waits for another task, is refused with one line saying why, and so is one whose folder that isn't in git another
open task already works in (a folder outside git takes one task at a time, named in the line). The popup **Hand to a
squad** opens, with:

- **The squads**, by name, with what the picked one can do under it. A squad that has something to fix on the Squads
  tab says so, and can't be picked.
- **On its own**: how much the squad does without asking. `space` on the line goes round three levels: **plans first,
  then waits for your yes** (the squad makes a plan and stops with it before anything is changed; see
  [The plan step](#the-plan-step); after your yes it works to the end and still asks before anything others can see),
  **asks before anything others can see** (the usual: a push, a merge request or a page waits for your `p`) and **works
  and publishes** (the squad pushes, opens and merges merge requests and publishes when the task's text asks for it, and
  says what it published in its result). It is asked at every hand-over and kept on the task; the popup starts from the
  project's default, which `o` changes on the [project's screen](projects.md#how-squads-start) (for a task in no project:
  asks before anything others can see).
- **One line per folder.** A folder in git says how the squad works there and which branch the work lands on: "shop: a
  copy on task/5, lands on main". `space` switches between **a copy** (a git worktree beside the repository, the usual)
  and **a new branch in your checkout** (`git switch -c task/5 main` run in your own folder). The branch is offered only
  when nothing else works in that checkout: it has no changes not yet committed, no other open task works there on a
  branch, and no session of yours is running in it; otherwise the line says why not. `l` changes the branch the work
  **lands on**: it starts as the repository's default branch (what `origin/HEAD` points at, else `main` or `master`,
  else the branch checked out), and must be a branch git finds in that repository. A folder not in git says "used as it
  is".
- `enter` hands it over; `esc` cancels. What the lines say is written on the task before the commander's check, so a
  hand-over that doesn't happen keeps it for the next try.

- **The commander checks first.** Before anything starts, the **squad commander** (one quick call to Claude, on Haiku,
  counted with your [smart features'](tokens.md) tokens) reads the task and what the squad can do, and says whether the
  squad can handle it. Meanwhile the task shows **Working** with "Commander is checking…", and platoon keeps going as
  usual. If the squad fits, the task goes on to it. If it doesn't, the task is back to **Can start** and says why
  ("Commander: This is docs work; ..."); press `S` and pick the same squad again to be asked "Hand it anyway?", and `y`
  hands it over without another check (another squad gets a check of its own). If the commander can't run - today's
  token limit is used up, or smart features are paused after a failure - **nothing is handed over**: the task says
  **Session limit has been reached** (or "AI features are switched off" when they are). A check that fails says so, and
  hands nothing over either. Its system prompt is **System prompt for the squad commander**, on the Projects tab of the
  settings.
- **Its own copy of the folder.** Whatever the squad, for each of the task's folders that is in a git repository and
  works as a copy, platoon makes a git worktree next to the repository, `<repo>-worktrees/task-<N>`, on a new branch
  `task/<N>` made from the branch the work lands on, and the squad works there, never in your own checkout. A copy
  already made for the task is used again; if the branch name `task/<N>` is taken by something else, platoon says so and
  stops. For a folder that works as a new branch in your checkout, platoon runs `git switch -c task/<N> <lands on>` there
  (refused, with the reason, if the checkout has changes not yet committed meanwhile), and the squad works in your own
  folder on that branch. Folders outside git are used as they are. platoon writes down on the task the commit each copy was
  made from. The copy is made in the background (the task says "Making its copy of the folder…"); if it can't be made,
  the task goes back to **Can start** with the reason, and one already made stays for the next try.
- **Copies go when the task is finished; branches stay.** Once a task is Done or Cancelled and its squad's session has
  stopped (never while it, or any session, is working in the copy), platoon removes the worktree folder with
  `git worktree remove` and **keeps the branch** `task/<N>` with every commit on it (deleting the branch is up to you). A
  copy with changes not yet committed (changed files, or new files git doesn't ignore) is kept: the task's screen says
  "Kept because it has changes not yet committed.", and `w` removes it anyway after asking (those changes are lost; the
  branch stays). Cancelling a task with such a copy says so in its question. A removed copy shows as "The copy is removed;
  its branch task/<N> is kept." Either way the status line says it once, one line per task (for example "Removed #7's
  worktree (its branch task/7 is kept)." or "Kept #7's worktree: it has changes not yet committed (w on the task removes
  it)."). Continuing the squad later (enter on the task, or writing to its session) makes the copy again from its branch,
  at the same place, before the squad goes on. platoon only ever removes the folder it made for that task, when git lists
  it as that repository's worktree on the task's branch. A branch made in your own checkout is never touched: it is yours.
- **Its own folder.** Every task handed to a squad gets a folder of its own, kept with the task in
  `~/.config/platoon/projects/<id>/tasks/<NN>-work/` (platoon never removes it). The squad starts in the task's first
  folder (its copy, when it has one), with its other folders and its own folder beside it, and writes what it makes for
  the task rather than for the project there, never in your repository: Research's `report.md`, notes, drafts. A task
  with no folder of its own has that folder as its folder: the squad starts there.
- **Its own session.** The leader starts in a platoon session of its own (it keeps running when platoon closes), named
  after its squad (`Coding squad · #N title`), with the task as its first message (which names each folder's branch and
  what it lands on) and the project's notes given to it as context. It belongs to the task only: it is never listed on
  the Sessions tab, in the project's sessions, in searches or counts, and it never joins the project.
- **Beside the task.** Opening a task that has a squad shows the task on the left and the squad's live session on the
  right. What you type goes to the session, as in any open session, and `ctrl+space` goes back to the project. The task
  side follows along: its state, and what the squad last said. On a terminal narrower than 100 columns the session gets
  the whole width, with one line about the task above it. When the squad's session isn't running (it finished, or
  platoon or the computer restarted), the right side says so, and `enter` continues the same conversation.

### How many squads work at once

At most 3 squads work at the same time, across all your projects (**Squads at work at once** on the Projects tab of the
settings, 1 to 10). A squad counts while its task is Working and its leader's session is running; one that needs you, is
done or cancelled doesn't. When you hand a task over (the commander said it fits, or you said "hand it anyway") and every
place is taken, the task shows **Working · Waiting for a free squad** and starts by itself when a place frees, the one that
has waited longest first, without taking over your screen (a notice says so). Its copy of the folder is made then, not
while it waits. The line is written on the task, so it outlasts platoon: the next platoon you open starts it. Only an open
platoon starts tasks from the line; with two platoon windows open, the first one opened does it (the other takes over when
it closes), and a task is never started twice. To take a task out of the line, cancel it (`c`). Lowering the number never
stops a squad; new ones just wait.

### What the squad has done, and the tokens it used

A task's screen (and the details beside the [Tasks tab](#the-tasks-tab)) shows the members' runs in the order the leader
started them, as its session shows them (not the workflow on paper), as a timeline: one line per run, its agent's name (the
squad's own part of it: `planner` for `coding-planner`), a bar for when it ran within the squad's time so far (from the
leader's first line to now while it runs, else to its last line), how long it took, and **done** once its result has come
back, **now** while it runs, **stopped** for one that failed or whose leader stopped before it reported. While the task is
open, the steps of the squad's workflow that haven't run yet follow, saying **not yet**.

**What ran** lists the commands the squad ran in the shell, the leader's and each member's, with the time each was asked
for, beside platoon's own steps for the task (each copy of a folder it made and the branch it came from, each squad it
started), grouped by the run they belong to: the Tasks tab shows the latest eight, a task's screen all of them (the latest
50 commands a task's squads ran are kept).

Below the task's details it also shows the tokens its squads used: the total, what that counts (what was sent to Claude,
what Claude wrote, and what was read again from Claude's cache, each answer once, the members' own work included), and a
line per agent - the leader and each member - added up over every squad the task has had. The Tasks tab and pane show the
short total ("1.2M tokens") on a working or finished task's second line. platoon reads all this from the squad's session
files in the background, every couple of seconds, only for the tasks that have had a squad: the open project's, or on the
Tasks tab every task on it.

### How the task's state follows the squad

At the end of each of its turns the leader says in one last line whether it is done, still working (members running), has
a question, is stuck, wants to publish, or (a task that plans first) has its plan ready, and platoon sets **Working**,
**Done** or **Needs you** (with its question, why it is stuck, what it wants to publish, or "The plan is ready"; a turn
that ends without that line counts as "the squad stopped without saying why").

**The squad never closes a task unasked**: when the work is finished it writes its short result (what was done, where,
what was not checked) and asks "Shall I close this task?", so the task shows Needs you with that question; only after
you say yes does it end with done and the task goes Done. When it is Done, the leader's result shows at the top of the
task's screen (it is kept in `tasks/<NN>-result.md`), and that result, and nothing else of the squad's conversation,
goes to the project's notes at their next update, as "Task #N (Coding squad)" (once).

While the leader is stuck on Claude's permission prompt, the task shows **Needs you** ("Waiting for your OK on a
command"); once you answer and it carries on, the task is Working again. The leader's session is not stopped for being
idle while its task is Working (its members may be at work while nobody watches); once the task needs you, is Done or
Cancelled, it is stopped like any idle session.

### Checks

A squad says when it is finished, and it can be wrong. Its [brief's](#a-squads-brief) **Checks** say what platoon checks
itself each time the squad stops at the end of the work (when its leader asks "Shall I close this task?", wants to publish,
or is done; nothing else the squad says starts them): the tests, an agent reading the change, the acceptance boxes. The
checks are the ones the brief had when its leader started; a squad started before briefs has the test check alone.

**The tests pass.** When the task's project has a [test command](projects.md#the-test-command) (`cargo test`, `npm test`,
`make check`: whatever tests your code), **platoon runs it itself**. A task outside a project, or in a project without a
test command, has no test check (its Tests line says "none set for this project").

- **Where**: in the folder the squad works in: the task's copy of its first folder in git, or your own checkout when the
  task works on a new branch there, and then only while that checkout is still on the task's branch (else it isn't run,
  and the task says "your checkout of shop isn't on task/5 any more"). Never in a folder where another session of yours
  is open (the task says "a session of yours is open there"), and never in a folder that isn't the task's. A task whose
  folders aren't in git has no test check.
- **How**: `sh -c` with your command, from the platoon window that is open (with two open, the first one opened does
  it), in the background, one task at a time, with the same environment as the squad's leader (less platoon's own
  `PLATOON_*` variables), and no terminal. It may run for 10 minutes (`test_timeout_minutes` on the project in
  `projects.json` changes that); then platoon stops it and everything it started. What it writes is kept in the task's
  own folder as `tasks/<NN>-work/tests-<try>.log`, for you to read (`v` on the task opens that folder too).
- **The result** is one line: with cargo's `test result:` lines, their counts added up ("214 passed", or "211 passed, 3
  failed"); otherwise the last line the command wrote ("sh: 1: cargo: not found"), or "timed out after 10 minutes".
  platoon keeps the last five on the task, and says each in the status line.
- **A pass** leaves the task as the squad left it: asking you to close it, wanting to publish, or Done.
- **A failure** is typed into the squad's session as a message from you: "The tests failed when platoon ran `cargo test`
  in <folder>:", the last 30 lines of what the command wrote, and "Fix what makes them fail, then end your turn as
  before: platoon runs them again when you stop." The task goes back to **Working**, the squad fixes what it can and stops
  again, and the tests run again. That happens **at most twice in a row**: the third failure leaves the task **Needs
  you**, "The tests still fail after the squad tried twice", and from then on platoon tells you rather than the squad
  until the tests pass again. When the squad's session isn't running, nothing is typed: the task Needs you, "The tests
  failed; the squad's session isn't running".
- A task you have written to meanwhile (it is Working again), or that its squad has stopped again since, keeps the
  result, and platoon types nothing.

**<agent> read the change** (say, "the reviewer read the change"). platoon reads the squad's session (as it does for
the timeline) and checks that the agent ran after the last run of any agent that can change files ("reviewer ran after
builder's last change"; when nothing was changed there is nothing to read, and it passes). When it didn't, platoon types
one message into the squad's session as a message from you: "Your brief asks that the reviewer read the change after the
last change; it hasn't since the builder's last run." The task goes back to **Working**. That happens **once**: when the
squad stops again without it, the task **Needs you** ("The change still wasn't read after the squad was told once").

**Every acceptance box is ticked, with how.** The task's text lists its acceptance checks as boxes (`- [ ] ...` under
**Acceptance**); the squad's result (what it says above its last line when it asks to close the task, wants to publish or
is done: platoon keeps it as the task's result) must list each one ticked, with how it was shown: `- [x] The basket
shows its total: a test adds two items`. The words are matched loosely (case, punctuation, a word or two). When one is
missing, or has no "how", platoon types one message naming the boxes not ticked, and the task goes back to **Working**;
**once**, then the task **Needs you** ("Not every acceptance box is ticked after the squad was told once"). A task with no
boxes passes.

**Nothing others see without your yes** is the gate platoon always keeps (see
[Anything other people can see asks you first](#anything-other-people-can-see-asks-you-first)): it never fails. Any other
line of a brief's Checks is one the squad checks itself: its leader is reminded of it, and the task's screen lists them.

When several fail at one stop, the squad gets them in one message; when one of them needs you, you are told rather than
the squad. On the Tasks tab a task that needs you for one says **a check failing** (**tests failing** for the tests), and
its screen says what platoon said (**platoon says**). With the squad's session not running, nothing is typed: the task
needs you at once.

The rest of this section is about the tests.

What you see: the task's **Tests** line, beside the Tasks tab and on its screen, says what the last run said, in one line:

| It says | When |
| --- | --- |
| Tests **214 passed** · platoon ran `cargo test` at 13:52 (green) | they passed |
| Tests **failed** · the squad is fixing them (try 2 of 2) (warning color) | they failed, and the squad has the work back |
| Tests **failed** · 211 passed, 3 failed · platoon ran `cargo test` at 13:52 | they failed, and you have been told |
| Tests **running…** | they are running, or about to |
| Tests **not run** · a session of yours is open there | they couldn't be run, and why |
| Tests not run yet | the project has a command, and the squad hasn't finished yet |
| Tests not run: its folders aren't in git | the task has no copy of a folder in git, or branch, to run them in |
| Tests none set for this project (quiet) | the project has no test command (or the task is in no project) |

On the Tasks tab, a task whose tests still fail after the squad's two tries says **tests failing** at the right of its row;
`y` answers its squad as for a question (to tell it what to do about them), and its screen says what platoon said
(**platoon says** The tests still fail after the squad tried twice).

Under the Tests line, beside the Tasks tab and on the task's screen, a line for each of its brief's other checks: **Review**
("✓ reviewer ran after builder's last change", "✗ reviewer hasn't run since builder's last change · the squad is on it",
or "not checked yet"), **Acceptance** ("✓ 2 of 2 ticked, with how", "✗ 1 of 2 ticked; not ticked: ..."), **Publishing**
("held by platoon: nothing others see without your yes", or "you let it publish on its own") and **Its own checks** (the
brief's other lines, which the squad checks itself).

### The plan step

A task handed over as **plans first, then waits for your yes** stops once before anything is changed. The leader has the
workflow's first step (the planner, in Coding) make a short plan: what will change and where, in steps, the risks, and how
each acceptance check will be shown. It ends its turn with the plan just above its last line, `SQUAD: plan ready`, and
platoon keeps the plan as `plan.md` in the task's own folder (`tasks/<NN>-work/plan.md`). The task is **Needs you** with
"The plan is ready": its row on the Tasks tab says **a plan to approve**, and the plan shows under **The plan** beside the
board and on the task's own screen.

- `y` types "Go ahead with the plan." into the squad's session: the task is Working again, and the squad runs the rest of
  its workflow to the end without stopping for the plan again. It still asks before anything others can see, as **asks
  before anything others can see** does.
- `n` opens a one-line box for what should change; `enter` types "Change the plan: " and your words into the squad's
  session. The squad plans again and stops again with the new plan.
- You can also say either in the squad's session yourself (`enter` on the task), in any words.

When the squad's session isn't running, `y` and `n` continue it beside the task and say so: tell it there. The plan file
stays with the task: its screen keeps showing it after the go-ahead, under the result once there is one. With the other
two levels there is no plan stop.

### Anything other people can see asks you first

Unless you said **works and publishes** at the hand-over, pushing, opening or merging a merge request, publishing a package
or a page waits for you even when the task asks for it: platoon refuses those commands in the squad's session until you
say so, and the squad stops at **Needs you**, saying what it wants to publish. Press `p` on the task to let it: the task
goes back to Working and platoon types "Go ahead and publish." into the squad's session. That go-ahead lasts while the
task is Working, or needs you for that very thing; once it is Done, Cancelled, back to Can start or needs you for
something else, the next push asks again. Local changes and local commits never ask. A merge request is opened against
the branch the work lands on.

With **works and publishes**, the gate lets those commands through while the task is open (Working or Needs you), and the
leader's rules tell it to publish when the task's text asks for it, never otherwise, and to say what it published.

What platoon recognises is listed in [What platoon changes](what-platoon-changes.md#squads).

### Writing to the squad

Whatever you send in the squad's session (an answer to its question, "yes" to closing, more work after Done, the go-ahead
`p` types) makes a task that needs you, is Done or Cancelled Working again, with the same squad session: that is how a
Done or Cancelled task is opened again (`enter` continues its session when it isn't running, then write to it). A reopened
task counts toward the squads at work at once like any Working one, even when that makes one more than the number allowed
for a while; tasks in the line just wait longer. A task nobody has yet and one waiting in the line are left alone. `c`
cancels a task after asking; when its squad's session is running, yes stops that one session too (its conversation is
kept).

### What you can change, and what platoon keeps

A squad's brief (its steps, checks and when it asks you) and its agents (their models, switches, skills, MCP servers and
system prompts) are on the [Squads tab](#the-squads-tab); the leader's system prompt and model are in the settings. The rules platoon keeps - where
the squad works and that what it writes for the task goes in the task's own folder, which branch the work lands on,
whether it plans first, may publish on its own or asks first, the last line of each turn, and "report back to the leader, never push"
for the members - are always sent after your text.

The leader may hand work to its members, read, search and run commands; it never edits a file itself (its members do).

Nothing about the squad is added to your own Claude Code settings: the leader's session alone gets platoon's squad hooks
(`platoon hook squad-stop`, `squad-gate`, `squad-notify`, `squad-tool` and `squad-prompt`), and `platoon setup` is not
involved. See [What platoon changes](what-platoon-changes.md#squads) for how a leader is started.

## The built-in squads

- **Coding**: makes a change to code, as described [above](#handing-a-task-to-a-squad).
- **Research**: finds things out and writes them up, with sources. The **questioner** (Sonnet) turns the task into a
  short list of questions; the **researcher** (Sonnet, can use the web) answers each one from the task's folders and the
  web, with sources; the **report-writer** (Sonnet, can change files) writes `report.md` in the task's own folder (never
  in your repository): the answers, the sources and what is still unknown; the **checker** (Opus, can use the web) checks
  each claim against its source, and may send it back to the researcher once.
- **Docs**: writes or updates documentation as Markdown files in the task's folder (its copy, in git). The
  **outliner** (Sonnet) reads the task, the existing docs and the code and proposes what to write where; the
  **tech-writer** (Sonnet, can change files) writes it; the **editor** (Opus) checks it against the code, the task and
  the project's style, and may send it back to the tech-writer twice. The result lists the files. Docs in Coda, Confluence
  and the like are for your own squads, using your Claude's connectors.
- **Summarize**: the **summarizer** (Sonnet, can use the web, so links can be read) reads what the task points at and
  writes a short summary for you; its step tells the leader that the summary is the result, so it goes on the task.

Only Research's researcher and checker and the summarizer can use the web. Every leader runs on the model in the settings
(Sonnet unless you change it). No built-in agent has skills or MCP servers.

Their checks (see [A squad's brief](#a-squads-brief)): Coding's are the tests pass, the reviewer read the change, every
acceptance box is ticked with how, and nothing others see without your yes; Docs' the tests pass, the editor read the
change, and the yes; Research's the checker read the change, every claim in the report names its source (the squad's
own), and the yes; Summarize's the yes. Each asks you before anything others can see, and when it is stuck (Summarize:
when it can't read what the task points at).

## The Squads tab

The fourth tab, `4` **Squads**, has two lists: the **squads** and the **agents** they are made of. The title of the left
pane shows which one you are looking at (`[Squads]  Agents`); `←` and `→` on the list switch between them (or click the
title). An agent is its own thing, shared by squads: the same planner can do a step in several squads.

### A squad's brief

A squad is a **brief**: a short Markdown file of yours that says what the squad does and how. On the right, the tab shows
the selected squad's brief as it reads: whether it is built in, **User Modified** or **Your own**, where its file is ("A file
you own: ~/.config/platoon/briefs/coding.md"; a built-in as platoon has it has no file yet, and says so), and then:

- **What it does**, in a sentence or two. The commander reads it before a task is handed to the squad, and so do its
  leader and the drafting of tasks from a conversation.
- **Steps**, drawn as a rail: one mark per step down a line (`○` the first, `●` the others), beside it the step's number,
  its **agent** with that agent's model and switches (`files`, `commands`, `web`, and `skills` or `MCP` when it has them),
  and under it what the step does. A step may **send the work back** to an earlier step, a set number of times at most:
  that is drawn as a loop on the left from the step back up to the one it returns to, with "↩ back to step 2 (builder), at
  most twice" beside it (on a narrow screen only the words stay). `◉ Done` ends the rail. platoon writes the steps into the
  leader's system prompt as a numbered list, and the leader follows them (platoon can't force the order: the agents run
  inside the leader's session). A squad's agents are the different agents its steps name: at most 8, and at most 12
  steps.
- **Checks**: what must be true before the work counts as done, one a line. Four are platoon's own, and it says so beside
  each: **the tests pass** (platoon runs the project's test command), **<agent> read the change** (platoon checks that
  agent ran after the last change), **every acceptance box is ticked, with how** (platoon checks the result ticks each of
  the task's acceptance boxes) and **nothing others see without your yes** (held by platoon: the gate it always keeps).
  Any other line is one the squad checks itself: its leader is reminded of it. See [Checks](#checks).
- **Asks you**: when the squad asks you, in your words. Its leader is told.
- Any section of your own (**Notes**, say), shown as you wrote it.

A brief with something wrong in it says so at the top, in the warning colour, one line each with where it is ("Steps,
line 11: no agent called verifer; there are planner, builder, reviewer and verifier"); until it is fixed the squad can't be
handed a task. The list marks it **needs fixing**. Every squad's leader is the same, set in the settings (**System prompt
for squad leaders** and **Model for squad leaders**, on the Projects tab): the brief has no leader of its own.

The file looks like this (Coding's, as it comes):

```
---
id: coding
name: Coding
---
# What it does
Makes a change to code in a git repository: plans it, builds it in a worktree of its own, reviews it and runs the
tests, then takes it as far as the task says.

# Steps
1. planner: Reads the task and the code it touches and writes a short plan that restates the acceptance checks.
2. builder: Makes the change the plan describes in the task's folder and runs the tests.
3. reviewer: Reads the change against the task and the plan, and checks that each acceptance check could have failed
   before the change. May send the work back to step 2, at most twice.
4. verifier: Runs the tests and the build and reports plainly what passed and what failed. May send the work back to
   step 2, at most once.

# Checks
- the tests pass
- the reviewer read the change
- every acceptance box is ticked, with how
- nothing others see without your yes

# Asks you
Before anything others can see, and when it is stuck.
```

A step is its number, the id of the agent that does it, a colon and what it does; it may go on over the lines under it. A
step that sends the work back says so in a sentence of its own: "May send the work back to step 2, at most once." (or
"twice", or "3 times", up to 5). Case and spacing don't matter (`## steps` reads as `# Steps`); the meaning does.

**`e` opens the brief in your editor**, the same editors as a project's notes (a built-in as platoon has it is written
there first, and taken away again if you leave it unchanged). When the editor is done, platoon reads the file again and
says in one line whether the squad can be handed tasks or what is wrong. A brief you save word for word as platoon has it
is the built-in again: its file goes.

### An agent

On the right: its name, its model, three switches - **can change files**, **can run commands** and **can use the web** (an
agent can always read and search; the switches add the rest, and nothing else) - its **skills** and **MCP servers**, its
system prompt, and **Used in**: every squad that uses it, with the steps. `enter` on a squad there opens its brief.

**A change to an agent is a change in every squad that uses it**, so the tab says where it is used; for a variant, copy the
agent (`c`) and name the copy in the steps you want. The list groups the agents as built in, **User Modified** and your
own, each with its model and how many squads use it (with only built-in agents there are no headings); **+ New agent** at
the end makes a blank one of your own. A brief's step names an agent by its id (the name it was made with).

### Agents your coding tool already has

Agents you made for Claude Code itself (`~/.claude/agents/*.md`, or the `agents` folder in `CLAUDE_CONFIG_DIR`) are listed
too, under **From Claude Code**, each with the file it comes from. A brief's step may name one by its name. platoon only
reads them: it never changes, copies or deletes them by itself. To change one for platoon, press `c`: it is copied into
platoon's own agents under the same name (its system prompt, its model when platoon offers it, else Sonnet, and switches
from its tools), and from then on briefs use the copy. A task's squad may also use the agents of the task's first folder
(its `.claude/agents`, or the one at the top of its repository): those are found when the task is handed over.

When an agent of the same name is in several places, platoon's own wins (built in or yours), then the task's folder's,
then your coding tool's. Such an agent runs with its own tools and model as its file has them (no tools listed: every
tool; no model: the leader's).

### Skills and MCP servers

`enter` on **Skills** lists the skills installed for you (each `skills/<name>/SKILL.md` in Claude Code's folder,
`~/.claude` or `CLAUDE_CONFIG_DIR`), each ticked when the agent has it; `enter` ticks or unticks one and the list stays
open, `esc` closes it. **Type the name of a skill…** at the bottom adds one that isn't installed here, such as a skill in a
task's own folder (`.claude/skills` in the repository). **MCP servers** works the same way with the servers you added for
every folder (`claude mcp add`, kept in `~/.claude.json` or the one in `CLAUDE_CONFIG_DIR`; platoon reads only their
names, never what is in them). A name that isn't installed here says "not found here" but is kept: it may be there where
the squad works.

When a squad starts, an agent's skills are loaded into it from the start and it may use other skills too (`Skill` is added
to its tools), and it may use each of its MCP servers (`mcp__<name>` is added to its tools: without that, naming a server
gives the agent nothing). The skills of Claude Code's own plugins aren't listed (type their name, `plugin:skill`), and
neither are servers from claude.ai connectors or a project's `.mcp.json`.

### Moving around and changing things

`↑` `↓` move in the list and in the details (a brief scrolls; an agent's rows are picked); `enter` (or `tab`, or `shift+→`
/ `ctrl+→`) goes into the details, and the back key `ctrl+space`, `esc` or `q` (or `tab`, `shift+←`, `ctrl+←`) back to the
list. On a terminal under about 80 columns the tab shows one pane at a time: the list, and `enter` opens the details in
its place, full width; `esc` goes back to the list, on the same entry. On a squad, `e` (or `enter` in its brief) opens the
brief in your editor. In an agent's details, `enter` changes the line you are on: a name in a small box, a model from a
short list, a system prompt in the same editors as a project's notes. `space` turns a switch on or off. Every change to an
agent is saved at once.

| Key | On the Squads list | On the Agents list |
| --- | --- | --- |
| `e` | Open the brief in your editor | |
| `n` | Describe a new squad (or agent) for Claude to draft | The same, an agent first |
| `c` | Copy the brief under a new name | Copy the agent (one of your coding tool's: into platoon's own) |
| `d` | Delete one of your own, or discard a draft | Delete one of your own, restore a changed built-in, discard a draft |
| `r` | Put a changed built-in back as platoon has it | Put a changed built-in back as platoon has it |
| `ctrl+s` | Save the draft | Save the draft |

### What is built in

**What platoon comes with is built in**: the four squads' briefs and their twelve agents (Coding's planner, builder,
reviewer and verifier; Research's questioner, researcher, report-writer and checker; Docs' outliner, tech-writer and
editor; and the summarizer). Everything about them can be changed; the list then says **User Modified**, and `r` puts
one back as platoon has it, after asking (your brief is kept as `<id>.md.bak`, an agent as `<id>.json.bak`; `d` does the
same on a changed built-in). They can't be deleted.

### Making your own

Press `n`: platoon asks whether to describe **a new squad** or **a new agent**, and you describe it in your own words ("a
squad that triages Sentry errors: a reader, a reproducer, a fixer"). One call to Claude, on Sonnet, counted with your
smart features' tokens, drafts it: for a squad, its brief, with its steps, checks and when it asks you. A brief's draft
uses the agents you have wherever one fits (Claude is shown each agent's name, model, switches and a line of what it does)
and brings new agents only for jobs none of them does; the draft marks those "new with the draft", and saving the squad
saves them too. Claude is also told which skills and MCP servers are installed, and a new agent may get only those.
platoon checks the draft (names, models, steps that name an agent that isn't there, a step that sends work back to a
later one) and says plainly what was wrong if it can't be used. A good draft opens on the tab as **Draft, not saved**:
`ctrl+s` saves it (then `e` opens it in your editor), or `d` throws it away.

`c` copies a squad (as a brief of your own) or an agent, built in or your own, under a new name. `d` deletes one of your
own, after asking (its file is kept as `<id>.md.bak` or `<id>.json.bak`); an agent a squad still uses can't be deleted, and
platoon names the squads. You can also make a brief by hand: any `<id>.md` in the briefs folder is a squad of yours. The
drafting calls' system prompts are **System prompt for drafting a squad** and **System prompt for drafting an agent**, on
the Projects tab of the settings, next to the commander's.

### A running task keeps its squad

When a squad's leader starts on a task, platoon keeps what it was started with beside the task
(`tasks/<NN>-squad-<k>.json`), its rules included (where it works, what the work lands on, whether it plans first or
publishes on its own). Changing the squad or its agents afterwards doesn't change that leader, also when it is continued later: it goes on
with the squad as it was. The next task gets them as they are then.

### Where squads and agents are kept

Your squads' briefs, and the built-in ones once you change them, are in `~/.config/platoon/briefs/<id>.md`; your agents in
`~/.config/platoon/agents/<id>.json`, one file each. A brief's file name is its id (the front matter says it too, and the
two must be the same); a brief's steps name agents by their id, which is made from the name once and never changes.
Sections and front matter lines platoon doesn't know are kept, an agent's keys too, and a file that can't be read is
shown as such on the tab and never written over.

Squads were JSON files before (`~/.config/platoon/squads/<id>.json`): each is turned into a brief once, the first time
platoon reads them, and kept beside where it was as `<id>.json.bak`. A brief that is there already wins over an old
file. A squad of your own gets the two checks every squad had before (the tests pass, and nothing others see without your
yes); a changed built-in keeps the built-in's checks. A task a squad has already started keeps what it started with.
