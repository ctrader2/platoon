---
name: to-tasks
description: "Turn work agreed in this conversation, or a plan the user points at, into platoon tasks that a squad will do; the user approves the list before anything is made. The user runs it by typing /to-tasks. When the conversation has settled on concrete work with several steps, you may suggest it once, in one line at the end of a reply (\"/to-tasks turns this into platoon tasks for a squad.\"). Never interrupt work in progress to suggest it, never run it unasked, and don't suggest it again in this conversation."
argument-hint: "[what to turn into tasks: a plan file, or a part of the conversation]"
allowed-tools:
  - "Bash({{platoon}} task squads:*)"
  - "Bash({{platoon}} task add:*)"
---

Turn the work agreed in this conversation into tasks in platoon, the app the user manages their Claude Code and Codex
sessions with. Each task later runs as one fresh session that follows a squad (a list of steps suited to its kind of
work), started by the user from platoon's Tasks tab. You make the tasks; you never start them and never do their work
in this conversation.

What to turn into tasks: $ARGUMENTS
(If that is empty, use the work this conversation agreed on. If it names a file, read it.)

Run platoon exactly as `{{platoon}}`, as written in the commands below: that is the platoon that installed this
skill, so it works whether or not platoon is on the PATH. Never write, edit or delete platoon's own files (`tasks.json`,
its config folder): everything goes through `{{platoon}} task add`. If a platoon command fails, stop, show the user
what it printed word for word, and say which tasks (if any) were already made. Don't retry with other options and don't
guess a project, except as step 5 says.

## 1. The squads

Run:

```sh
{{platoon}} task squads
```

It prints each squad there is, one per line: its id, then what it is for. Suggest for each task the squad that suits
it. If the command fails, leave out `--squad` in step 5 and tell the user they pick a squad when they start a task.

## 2. Draft the tasks

Each task is one thin, complete piece of work:

- It fits in one fresh session. Whoever picks it up has never seen this conversation, so the task carries what they
  need: why the work is wanted, what was decided and why, and what is out of scope.
- It says what should work when it is finished, as behaviour someone can see, not a list of file changes.
- Its "Done when" boxes are checks anyone can make. Each one fails before the work and passes after it, and none
  depends on another task's work. platoon's check of the finished work reads these boxes, so make them exact.
- No file paths, function names or code, because they go stale. The exception is a short snippet that records a
  decision (an agreed format, a command line, an exact message).
- Fewer is better: one task for work that belongs together, more only where pieces can be done and checked apart.
  A preparation step that the rest needs (a rename, a new test helper) is its own task; say in the others' text that
  it must be done first (platoon has no "waits for" link).
- Each works in a folder: this session's main working folder, unless the conversation put that work in another one.

Cover only what the conversation asked for. Do not invent extra work.

Each task's text has this shape (platoon's own template):

```markdown
## What it should do

<the behaviour, and the context a newcomer needs>

## Done when

- [ ] <a check that fails today and passes when this is done>
- [ ] <another>
```

## 3. Show the list and ask (make nothing yet)

Show the tasks as a numbered list: title, suggested squad, the folder it works in, and one or two lines on what each
delivers. Then ask with the AskUserQuestion tool, question "Make these tasks?", options:

- "Make them": make every task on the list.
- "Make one task": merge the list into a single task first (offer this only when there are two or more).
- "Show me the text": print each task's full text, then ask this question again.
- "Cancel": make nothing.

The user can also pick "Other" and type a change: make it, show the list again and ask again. Until a "Make" answer,
make nothing.

## 4. Make the tasks

One command per task, the text piped in after it exactly in this form (`TASK` must stand alone at the start of its
line, and no line of the text may be just that word). Nothing else goes in the command.

```sh
{{platoon}} task add --folder '/path/to/folder' --squad 'docs' --title 'Short plain title' <<'TASK'
## What it should do
...
TASK
```

Put every value in single quotes, and keep titles short and plain, without quotes, `$` or backticks. Leave out
`--squad` when step 1 says to. Each command prints the new task's number (`#N`).

## 5. Which project

platoon puts each task in the project whose sessions worked in its folder, or in "No project" when there is none. If
the user named a project for the tasks, add `--project '<its name>'`. If `task add` fails because sessions of more than
one project worked in the folder, it names them: ask the user which one, then make the task again with
`--project '<that name>'`.

## 6. Say what was made

List the tasks made, each with the number platoon printed, its title and its project. Tell the user they are on
platoon's Tasks tab (key `3`) under Not started, and that `s` on one picks its squad and starts it.
