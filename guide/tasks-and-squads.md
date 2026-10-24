# Tasks and squads

A **task** is one piece of work, usually in a [project](projects.md). When you start one, platoon opens one ordinary
Claude Code or Codex session for it (the squad's model decides which: see [Tasks on Codex](#tasks-on-codex)), in a copy
of its folder (a git worktree on the branch `task/<N>`), following a **squad**: a
short Markdown file of steps that says how this kind of work is done (plan, build, check, report). The session is a
session like any other: it shows on the Sessions tab, marked with the task's number, and you can open it and talk to it.

## The Tasks tab

The third tab, `3` **Tasks**, lists every task across your projects (and the ones in no project) in four sections, each
with how many it has:

- **Needs you**: its session stopped for you. Its plan is ready for your yes, it asks a question, it waits at a
  permission prompt, or the [check of its work](#the-check-of-the-work) still found problems (or could not run).
- **Working**: its session is at it (or platoon is making its worktree, or checking its work).
- **Not started**: nobody has started it yet.
- **Done**: Done or cancelled in the last 24 hours. Done means the check passed, not that the work is merged: while
  its branch `task/<N>` has commits your repository's default branch hasn't, the task reads **Done · not merged** (see
  [After the work](#after-the-work)).

Each task takes two lines: its project's badge, its number and title, and at the right what it needs or how long since
it last changed; under it, its squad and its own words (the question it asks), **Suggested: Docs · s starts it** for one
not started yet, and **No folder yet** when it has none. The header says
how many tasks need you, **● 2 need you**, on every tab.

On a terminal 100 columns wide or more, the picked task's details are beside the list: its state and why, its squad, its
folder, its branch and worktree, the **Verdict** of the last check of its work (and what it
found, `file:line: problem`), the tokens its session used, **The plan** when one waits for your yes, and its **Done when**
boxes (ticked ✓ or crossed ✗ by the check, once it has run).

| Key (Tasks tab) | Does |
| --- | --- |
| `n` | a new task: which project it is in (or **No project**), then its title; its text opens in your editor. A task in no project asks for its folder first |
| `s` | start it: the list of squads opens with the suggested one picked (else **Feature**); `enter` starts it. A task with no folder asks for one first |
| `enter` | open the task's session (continued if it isn't running); on a task not started yet, its text |
| `e` | edit its text in your editor |
| `c` | cancel it, after asking. Its running session is stopped; the conversation is kept |
| `v` | open its worktree (or its folder) in your [editor](sessions.md#opening-the-editor-v) |
| `/` | filter by words in the title |

## Writing a task

A task's text is Markdown in the shape platoon starts it with:

```
## What it should do

## Done when

- [ ]
- [ ]
```

The session gets the text as its first message, and its squad tells it to end by saying how it checked each **Done when**
box; then [the check of the work](#the-check-of-the-work) judges each box itself.

### Making tasks from a conversation

`t` on a session's row (a Claude Code conversation or a Codex one) asks Claude (with the smart features on) to read the
conversation and suggest tasks, each with the squad that suits it. They open in a popup: `↑` `↓` move, `e` edits one in your editor, `x` removes one, `enter` makes
them all as **Not started**, `esc` makes nothing. Nothing starts until you press `s` on each.

### Claude offers to make them

A session platoon starts or resumes (not a task's own session) is told, through platoon's SessionStart hook, to watch
for the moment the conversation settles on concrete work that could be handed off. Then it asks you once, with Claude
Code's question tool: **Ready to create tasks?** On a yes it makes each task itself with `platoon task add` (by
platoon's full path, so it works without `platoon` on your PATH), in the session's project and folder, with a **Done
when** list and the squad that suits it; then it says how many it made and that they are on the Tasks tab. It starts
none of them. It doesn't ask again in that conversation unless you bring it up. It needs the **Project notes** line of
[`platoon setup`](install-and-setup.md#set-it-up-platoon-setup) (that is the SessionStart hook); sessions you start with
plain `claude` outside platoon are not asked.

A Codex session platoon starts or resumes is told the same, through platoon's Codex hook (which Codex runs with your
first message), except that it asks in its reply, in plain words, and you answer with your next message. Codex keeps
its work inside the session's folder, and `platoon task add` writes in platoon's own folder, so Codex asks you to allow
that command: allow it, and the tasks are made. Sessions you start with plain `codex` outside platoon are not asked.

## Starting a task

`s`, then `enter` on a squad. platoon:

1. makes a worktree of the task's folder on a new branch `task/<N>`, from the repository's default branch, beside it in
   `<repo>-worktrees/task-<N>` (a folder that isn't in git is used as it is);
2. writes the task's own folder: `squad.md` (platoon's rules for the task, then the squad's steps) and a copy of every
   squad in `squads/`, so a step can say "follow the Review squad";
3. starts `claude` in the worktree, in a platoon host, with the squad's model, in plan mode when the squad plans first
   (otherwise it edits files without asking), `squad.md` added to its system prompt, and the task's text as its first
   message, and takes you into it. On one of Codex's models it starts `codex` instead (see
   [Tasks on Codex](#tasks-on-codex)).

### How the task follows its session

platoon gives the session a few hooks, and they move the task along:

- It presents its plan: **Needs you · a plan to approve**, with the plan beside the list. Approve it in the session: the
  task is Working again.
- It asks you something (Claude Code's question tool): **Needs you · a question**, with the question. Answer it in the
  session.
- It waits at a permission prompt: **Needs you · asks permission**.
- It stops: platoon [checks the work](#the-check-of-the-work), and only a check that says done makes the task
  **Done**.
- You type into it: a task that needed you, or was Done, is Working again, with its tries counted from nothing.

### The tests

platoon runs no tests itself. Its rules for every task tell the session to find how its repo runs its tests (its
`CLAUDE.md`, its README or its build files), run them before it finishes, and name the command and the result in its
last message (or say the repo has none, and how it checked the change instead). The Feature, Bug fix and Refactor
squads say so again where they check their work. A repo whose `CLAUDE.md` says how to build and test is found
fastest.

### The check of the work

When a task's session stops, a fresh call, on the squad's `verdict_model` (Claude's, or one of Codex's),
judges whether the work does what the task asked. It didn't do the work, it can only read (the files in the worktree:
on Claude Code no shell and no edits; on Codex its read-only sandbox, where it can also run commands that change
nothing), and it is given the task's text with its **Done when** boxes, the session's last message (what it
says it did) and the change on the task's branch since it started (what isn't committed yet is listed too). It says, for
each box, whether it is met, and what is missing or wrong, each with the file, the line and the exact text there. The
task shows **Working · checking the work** meanwhile (10 minutes at most).

- **Done**: the task is Done; its details say **Verdict done · checked by fable at 13:52** and its boxes are ticked.
- For a change to code, a last message that names no test run is **not done**: the check crosses the box **Tests run
  and reported**.
- **Not done**: the session gets what the check found, `file:line: problem` first, and carries on, once. The next time
  it stops, the check runs again; a second "not done" makes the task **Needs you · the check found
  problems**, with what it found in its details.
- platoon looks for the text the check quotes at each line it names. A finding whose text isn't there is left out; a
  "not done" with nothing left that holds up counts as done, and the Verdict line says so.
- **The check could not run** (it took too long, its answer couldn't be read, `claude` or `codex` wasn't found, its
  model isn't installed here): the task needs you, with why. It never holds the session up for longer than its 10 minutes.

Typing into the session counts the tries from nothing again. The check's whole answer is
`verdict.json` in the task's own folder. It needs no switch: it is part of the task, like its session (see
[Features that use tokens](tokens.md#the-features)).

### Anything other people can see asks you first

The session may change files and commit in its worktree without asking, but Claude Code asks you before `git push` (also
when it is written `git -C <dir> push` or `cd <dir> && git push`),
`gh pr create`, `gh pr merge`, `gh release`, `gh api` calls that write, `glab`, `cargo publish` and `npm publish`.
Merging, pushing and opening pull requests are yours to do (or to ask the session for). A task on Codex holds the
same commands back until you say yes in its session (see [Tasks on Codex](#tasks-on-codex)).

### After the work

When a Done or cancelled task's session has stopped and nothing else is open in its worktree, platoon removes the
worktree (never its branch: `task/<N>` and its commits stay). A worktree with changes not yet committed is kept, and the
task says so. Opening a Done task's session again makes its worktree again from the branch first.

**Merging is yours.** The work stays on `task/<N>`, and the session's last message says so when it committed something
(a task that changes nothing, such as an Incident diagnosis, leaves the branch out). While the branch has
commits the default branch hasn't (here, or on `origin` as git last fetched it), the task's row says **Done · not
merged** and its details say how many, **Branch task/5 has 2 commits not in main.** Merge it (or ask the session to
open a pull request) and it reads plain **Done**. A branch merged by squashing or rebasing gets new commits on the
default branch, so it says not merged until you delete `task/<N>`.

## Tasks on Codex

A squad whose model is one of Codex's (**GPT-6-Luna**, say) runs its task in Codex instead of Claude Code: the model
decides the program. Everything above holds (the worktree, the squad, the Tasks tab, the check of the work before it
is Done); what differs is how Codex does each thing.

- **Run `platoon setup` once more** after updating platoon: it adds platoon's task hooks to Codex's `hooks.json` and has
  Codex trust them, as it did for the notes hooks (see [Codex](install-and-setup.md#codex)). Without them the task still
  runs, but its state doesn't follow the session.
- **It starts in Codex's `workspace-write` sandbox**, asking you when it wants more (`-a on-request`), with the task in
  its environment (`PLATOON_TASK`) and without Codex's update question. Codex takes no name and no session id from
  platoon: platoon learns the session's id from Codex as the session starts, and marks its row `#N` itself.
- **Its folder is trusted in Codex for you.** When platoon makes the task's worktree, it has Codex itself trust that
  folder (the way Codex's own "Do you trust this folder?" does, written by Codex in its `config.toml`), and takes that
  trust away when it removes the worktree after the work. An entry of your own for that folder is left alone. If Codex
  can't be asked, the task starts all the same and says so: Codex asks you in the window.
- **platoon makes its commits.** Codex keeps git's data read-only in its sandbox (for a worktree also the main
  repository's `.git/worktrees/…`), so the session can change files but can't commit them, and its instructions tell it
  not to try. Each time the session stops, platoon commits everything it changed in the worktree on `task/<N>`: one
  commit per stop, its message **#5 Add a basket** (the task's number and title), with your repository's own git
  identity (only when git knows none at all is the commit made as `platoon <platoon@localhost>`, for that commit alone;
  platoon never changes git's settings) and your repository's own hooks. Then the work is checked, and the check sees
  the commit. A stop that changed nothing makes no commit. A commit that fails (a `pre-commit` hook that refuses it,
  say) makes the task **Needs you · its work couldn't be committed**, with git's first lines; the changes stay in the
  worktree, nothing is checked, and the next stop tries again once you have typed into the session.
- **The network is on**, so it can fetch dependencies and run tests that need it. Pushing, pull requests and publishing
  still wait for you: platoon's hook reads each command the session runs (also inside `bash -lc '...'`, after `cd ... &&`,
  as `git -C <dir> push`, after `VAR=value`, and the like) and holds back the same commands Claude Code asks about. The
  task shows **Needs you · a question**: "It wants to run `git push origin task/5`: say yes in its session to let it, or
  tell it not to." Say yes in the session and the session runs that same command once more; it goes through, once. What
  isn't caught: a script or program that pushes for you, a git alias, and an MCP tool that publishes. This is weaker than
  Codex's sandbox with the network off, by choice: fewer interruptions.
- **Plans first, without Codex's plan mode.** A squad that plans first tells Codex to look around, write the plan as its
  reply and stop. The task shows **Needs you · a plan to approve**, with the plan beside the list (and no check of the
  work, since nothing was done yet). Answer in the session ("go ahead", or what to change): your answer is your say on
  the plan, and the task is Working again. Until you answer, platoon refuses the session's file edits (Codex's
  `apply_patch`, also run from the shell) and tells it to present the plan and wait. Other shell commands can still
  write files; the instructions are what keeps it from that.
- **Questions.** Codex asks with its question tool where it has one (in its own plan mode), else in its reply: it ends
  the reply with a line starting **Question for you:**, and the task shows **Needs you · a question** with it, without
  checking the work. Answer in the session.
- **A permission prompt** (Codex asking to run something outside its sandbox) shows **Needs you · asks permission**, as
  on Claude Code.
- **Helpers.** Where a squad's step starts helpers (subagents), Codex uses its own (`spawn_agent`), on its own models;
  where it can't, it does each helper's part itself, one pass each. The **Review** squad's four reviewers are Codex
  helpers on Codex's models, or four passes by the session itself.
- **The check of the work** runs on the squad's verdict model, Claude's or Codex's, as for any task.

### On a computer with only Codex

Without Claude Code, the built-in squads you haven't changed run on Codex's first model, for the session and for the
check of its work. The Squads tab and the list `s` opens say so: **Model GPT-6-Luna, since Claude Code isn't installed**.
A squad of your own that names one of Claude's models says **(needs Claude Code, not installed)** and doesn't start.

## Squads

| Squad | For | Model | Plans first |
| --- | --- | --- | --- |
| **Feature** | new or changed behaviour, planned first and built in small checked steps | opus | yes |
| **Bug fix** | a defect to reproduce, trace to its root cause and fix with evidence | opus | yes |
| **Incident** | something broken in production: find what is causing it from the evidence, changing nothing (no fix, no rollback) | opus | no |
| **Refactor** | a change of structure that keeps the behaviour exactly as it is | opus | yes |
| **Explain** | a read-only question about how code works or which option to pick | opus | no |
| **Review** | an adversarial review of a change or a plan, with a verdict | opus | no |
| **Docs** | documentation to write or improve | sonnet | no |

A squad is a Markdown file: front matter, then its steps.

```
---
name: Docs
description: Documentation to write or improve: guides, READMEs, reference pages, comments for readers.
model: sonnet
verdict_model: opus
plan_first: false
---
1. Read what is there ...
```

`description` is what the drafting reads to suggest a squad; `model` is the session's model; `plan_first: true` starts
it in plan mode; `verdict_model` is the model that [checks the work](#the-check-of-the-work) (the session's own model
when it has none). A model is written as its program names it (`opus`, a full Claude model name, or one of Codex's such
as `gpt-6-luna`) and kept exactly so; the model decides the program that runs it.

- **The session's model is any model installed**, Claude's or Codex's: a Claude model runs the task in Claude Code, one
  of Codex's in Codex (see [Tasks on Codex](#tasks-on-codex)). A squad whose model this computer can't run doesn't
  start: `s` says why, and nothing is made.
- **The verdict model can be any model installed**, Claude's or Codex's (see
  [Choosing a model](settings-and-keys.md#choosing-a-model)): the check is one call, which either program can make. On
  Codex it runs in Codex's read-only sandbox in the task's folder (see [Calls on Codex](tokens.md#calls-on-codex)). A file of your own in `~/.config/platoon/squads/<id>.md` is a squad of yours, and one named like a
built-in (`feature.md`) replaces it. A file with something wrong in it is listed as **needs fixing** and can't start a
task; the list says what is wrong. A started task keeps the squad it started with (its copy is in the task's own folder).

### The Squads tab

The fourth tab, `4` **Squads**, lists every squad: the built-ins first, then yours by name, each with what it is for and
where it comes from (**built in**, **yours**, or **yours, replaces the built-in**), and **needs fixing** when its file has
something wrong. From 80 columns the picked squad is beside the list: what it is for, its model and verdict model (by
their names; with both programs installed, the one that runs each, **Opus (Claude Code)**, **GPT-6-Luna (Codex)**; and
**(needs Codex, not installed)** or the like for one this computer can't run), whether it plans first,
its steps, and under them what is wrong with its file, one line each.

| Key (Squads tab) | Does |
| --- | --- |
| `enter` | the squad in the whole width, with its **Model** line picked; `esc` goes back to the list |
| `↑` `↓` (squad open) | pick its **Model**, **Verdict model** or **Plans first** line; past them, scroll the squad |
| `enter` (squad open) | change the picked line to the next choice: the model goes round every model installed, Claude's then Codex's (the model decides the program the task runs in); the verdict model the same, then **the same as the model**; plans first yes or no. A click on a line picks it, a click on the picked one changes it |
| `e` | open its file in your editor (the same list of editors as the notes); the tab reads it again when the editor is done. On a built-in, its text is copied to `~/.config/platoon/squads/<id>.md` first, and if you leave it unchanged the copy goes again, so a built-in becomes yours only when you change it |
| `n` | a new squad: its name, then its file (`~/.config/platoon/squads/<name>.md`, the Feature squad's settings and one step to write over) opens in your editor |
| `d` | delete one of yours, after asking. The file is kept as `<id>.md.bak`; a built-in it replaced comes back |

Changing a setting rewrites only that line of the squad's file, everything else in it as it was. On a built-in, its
text is copied to `~/.config/platoon/squads/<id>.md` first, as `e` does, so it reads **yours, replaces the built-in**;
change it back to the built-in's own and the copy goes again. A file that has no front matter at its top is left
alone (`e` opens it to fix that), and so is one open in your editor. The description and the steps are changed with
`e`.

Changing or deleting a squad never touches a task that has started: it follows its own copy.

The built-in squads are adapted from the playbooks of Lauren Tan's
[pstack](https://github.com/cursor/plugins/tree/main/pstack) (MIT; see `THIRD_PARTY_NOTICES.md`).

## Where tasks are kept

A project's tasks are in `~/.config/platoon/projects/<id>/tasks.json`, their texts in `tasks/<NN>-<title>.md` beside it
and each task's own folder in `tasks/<NN>-work/` (`squad.md`, `squads/`, `plan.md`, `verdict.json`). Tasks in no project are
in `~/.config/platoon/tasks.json` and `~/.config/platoon/tasks/`. Numbers are never reused.

From a shell:

```
platoon task add --project "Web shop" --title "Order emails in plain text" [--body-file text.md] [--folder ~/src/shop] [--squad bug-fix]
platoon task list [--project "Web shop"] [--json]
```

The text comes from `--body-file`, else from what is piped in, else the template. `--squad` suggests the squad that
starts it: an id from the Squads tab (one there isn't is refused, naming the ones there are). Each task made prints one
line, its number (`#4`).
