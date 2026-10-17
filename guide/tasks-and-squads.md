# Tasks and squads

A **task** is one piece of work, usually in a [project](projects.md). When you start one, platoon opens one ordinary
Claude Code session for it, in a copy of its folder (a git worktree on the branch `task/<N>`), following a **squad**: a
short Markdown file of steps that says how this kind of work is done (plan, build, check, report). The session is a
session like any other: it shows on the Sessions tab, marked with the task's number, and you can open it and talk to it.

## The Tasks tab

The second tab, `2` **Tasks**, lists every task across your projects (and the ones in no project) in four sections, each
with how many it has:

- **Needs you**: its session stopped for you. Its plan is ready for your yes, it asks a question, it waits at a
  permission prompt, the project's tests still fail after it tried twice, or the [check of its work](#the-check-of-the-work)
  still found problems (or could not run).
- **Working**: its session is at it (or platoon is making its worktree, or checking its work).
- **Not started**: nobody has started it yet.
- **Done**: Done or cancelled in the last 24 hours.

Each task takes two lines: its project's badge, its number and title, and at the right what it needs or how long since
it last changed; under it, its squad and its own words (the question it asks), **Suggested: Docs · s starts it** for one
not started yet, **No folder yet** when it has none, and **tests ✓** or **tests ✗** once the tests ran. The header says
how many tasks need you, **● 2 need you**, on every tab.

On a terminal 100 columns wide or more, the picked task's details are beside the list: its state and why, its squad, its
folder, its branch and worktree, what the last test run said, the **Verdict** of the last check of its work (and what it
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

`t` on a session's row asks Claude (with the smart features on) to read the conversation and suggest tasks, each with
the squad that suits it. They open in a popup: `↑` `↓` move, `e` edits one in your editor, `x` removes one, `enter` makes
them all as **Not started**, `esc` makes nothing. Nothing starts until you press `s` on each.

## Starting a task

`s`, then `enter` on a squad. platoon:

1. makes a worktree of the task's folder on a new branch `task/<N>`, from the repository's default branch, beside it in
   `<repo>-worktrees/task-<N>` (a folder that isn't in git is used as it is);
2. writes the task's own folder: `squad.md` (platoon's rules for the task, then the squad's steps) and a copy of every
   squad in `squads/`, so a step can say "follow the Review squad";
3. starts `claude` in the worktree, in a platoon host, with the squad's model, in plan mode when the squad plans first
   (otherwise it edits files without asking), `squad.md` added to its system prompt, and the task's text as its first
   message, and takes you into it.

### How the task follows its session

platoon gives the session a few hooks, and they move the task along:

- It presents its plan: **Needs you · a plan to approve**, with the plan beside the list. Approve it in the session: the
  task is Working again.
- It asks you something (Claude Code's question tool): **Needs you · a question**, with the question. Answer it in the
  session.
- It waits at a permission prompt: **Needs you · asks permission**.
- It stops. With a test command for the project (`T` on the [project's screen](projects.md)), platoon runs the tests in
  the worktree: when they fail, the session gets the end of their output and carries on, twice; the third time the task
  needs you with what failed. The output is in `tests.log` in the task's own folder. When they pass, or the project has
  no test command, platoon [checks the work](#the-check-of-the-work), and only a check that says done makes the task
  **Done**.
- You type into it: a task that needed you, or was Done, is Working again, with its tries counted from nothing.

### The check of the work

When a task's session stops and the tests pass (or there are none), a fresh Claude, on the squad's `verdict_model`,
judges whether the work does what the task asked. It didn't do the work, it can only read (the files in the worktree:
no shell, no edits), and it is given the task's text with its **Done when** boxes, the session's last message (what it
says it did) and the change on the task's branch since it started (what isn't committed yet is listed too). It says, for
each box, whether it is met, and what is missing or wrong, each with the file, the line and the exact text there. The
task shows **Working · checking the work** meanwhile (10 minutes at most).

- **Done**: the task is Done; its details say **Verdict done · checked by fable at 13:52** and its boxes are ticked.
- **Not done**: the session gets what the check found, `file:line: problem` first, and carries on, once. The next time
  it stops, the tests and the check run again; a second "not done" makes the task **Needs you · the check found
  problems**, with what it found in its details.
- platoon looks for the text the check quotes at each line it names. A finding whose text isn't there is left out; a
  "not done" with nothing left that holds up counts as done, and the Verdict line says so.
- **The check could not run** (it took too long, its answer couldn't be read, `claude` wasn't found): the task needs
  you, with why. It never holds the session up for longer than its 10 minutes.

Typing into the session counts the tries from nothing again, as for the tests. The check's whole answer is
`verdict.json` in the task's own folder. It needs no switch: it is part of the task, like its session (see
[Features that use tokens](tokens.md#the-features)).

### Anything other people can see asks you first

The session may change files and commit in its worktree without asking, but Claude Code asks you before `git push` (also
when it is written `git -C <dir> push` or `cd <dir> && git push`),
`gh pr create`, `gh pr merge`, `gh release`, `gh api` calls that write, `glab`, `cargo publish` and `npm publish`.
Merging, pushing and opening pull requests are yours to do (or to ask the session for).

### After the work

When a Done or cancelled task's session has stopped and nothing else is open in its worktree, platoon removes the
worktree (never its branch: `task/<N>` and its commits stay). A worktree with changes not yet committed is kept, and the
task says so. Opening a Done task's session again makes its worktree again from the branch first.

## Squads

| Squad | For | Model | Plans first |
| --- | --- | --- | --- |
| **Feature** | new or changed behaviour, planned first and built in small checked steps | opus | yes |
| **Bug fix** | a defect to reproduce, trace to its root cause and fix with evidence | opus | yes |
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
when it has none). A file of your own in `~/.config/platoon/squads/<id>.md` is a squad of yours, and one named like a
built-in (`feature.md`) replaces it. A file with something wrong in it is listed as **needs fixing** and can't start a
task; the list says what is wrong. A started task keeps the squad it started with (its copy is in the task's own folder).

The built-in squads are adapted from the playbooks of Lauren Tan's
[pstack](https://github.com/cursor/plugins/tree/main/pstack) (MIT; see `THIRD_PARTY_NOTICES.md`).

## Where tasks are kept

A project's tasks are in `~/.config/platoon/projects/<id>/tasks.json`, their texts in `tasks/<NN>-<title>.md` beside it
and each task's own folder in `tasks/<NN>-work/` (`squad.md`, `squads/`, `plan.md`, `tests.log`, `verdict.json`). Tasks in no project are
in `~/.config/platoon/tasks.json` and `~/.config/platoon/tasks/`. Numbers are never reused.

From a shell:

```
platoon task add --project "Web shop" --title "Order emails in plain text" [--body-file text.md] [--folder ~/src/shop]
platoon task list [--project "Web shop"] [--json]
```
