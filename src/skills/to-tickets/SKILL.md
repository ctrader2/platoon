---
name: to-tickets
description: Turn this conversation, or a plan the user points at, into tickets in the user's platoon project. The user approves the list before anything is written. Only when the user types /to-tickets.
disable-model-invocation: true
argument-hint: "[what to turn into tickets: a plan file, or a part of the conversation]"
---

Turn the work discussed in this conversation into tickets in platoon, the app the user manages their Claude Code
sessions with. Squads of agents will later pick the tickets up, one fresh session per ticket. You make the tickets;
you never start the work, and you never choose who does it.

What to turn into tickets: $ARGUMENTS
(If that is empty, use the work this conversation has agreed on. If it names a file, read it.)

Run platoon only as `platoon` from the PATH, and put every value you pass it in double quotes. Never write, edit or
delete platoon's own files (its config folder, `tickets.json`, the ticket files): everything goes through the
`platoon` commands below. If any `platoon` command fails, stop there, show the user what it printed, word for word,
and say which tickets (if any) were already made. Do not retry with other options and do not guess a project.

## 1. Check that this session is in a project

Run:

```sh
platoon ticket where --session "$CLAUDE_CODE_SESSION_ID"
```

If it fails, tell the user the line it printed and stop. Do nothing else: no draft, no questions, no other command.
(The user adds the session to a project in platoon: pick it on the Sessions tab and press m. Then they can run
/to-tickets again.)

If it works, it prints the project's name. Use that name when you talk to the user.

## 2. Learn what kinds of work the squads can do

Run:

```sh
platoon squads --kinds
```

It lists each squad with what it can do and whether it needs a folder. Use it only to shape the tickets, for
example to split a coding part from a documentation part, or to keep research apart from the change it informs.
Never pick a squad, never name one in a ticket, and never write a ticket so that only one squad could take it: if
two squads do the same kind of work, either should be able to take the ticket.

## 3. Draft the tickets

Each ticket is one thin, complete piece of work:

- It fits in one fresh session. Whoever picks it up has never seen this conversation, so the ticket carries what
  they need: why the work is wanted, what was decided and why, and what is out of scope.
- It says what should work when it is finished, as behaviour someone can see, not a list of file changes.
- It has acceptance checks as checkboxes. Each check must fail before the work and pass after it, and must not
  depend on another ticket's work. Drop a check that would already pass today.
- No file paths, function names or code, because they go stale. The one exception is a short snippet that records
  a decision (an agreed format, a command line, an exact message).
- It has a "How far:" line. The default is `How far: local changes only.` Ask for more (a local commit, a push and a
  merge request, a published page) only when this conversation asked for it.
- Preparation first: a change that makes the rest easier (a rename, moving code, a new test helper) is its own
  ticket, and the others wait for it.
- It says which of the new tickets must be finished before it can start ("waits for"). Only real dependencies:
  two tickets that could be done in either order don't wait for each other.

Cover only what the conversation asked for. Do not invent extra work.

Write each ticket's text in this shape:

```markdown
## What it should do

<the behaviour, and the context a newcomer needs>

How far: local changes only.

## Acceptance

- [ ] <a check that fails today and passes when this is done>
- [ ] <another>
```

## 4. Show the list and ask (write nothing yet)

Show the proposed tickets as a numbered list. For each: its title, what it waits for (by the list's numbers), and
one or two lines on what it delivers. Then ask the user:

- Is each ticket the right size, or should some be merged or split?
- Are the "waits for" links real?
- Is anything missing, or anything here that shouldn't be?

Change the list as the user says and show it again. Repeat until the user clearly approves it. Until then, write
nothing to platoon.

## 5. Write the tickets through platoon

Write them in order, each ticket after the tickets it waits for. `platoon ticket add` prints the new ticket's number
(`#N`); use those printed numbers, not the list's numbers, for `--waits-for`.

Folders: pass every working folder this session has (its main folder and any folders added to it), each with its
own `--folder`. For a ticket that needs no folder at all (pure research, or a summary of something online), use
`--no-folder` instead.

For each ticket, write its text to a new temporary file and pass that file; then remove it. For example, in one
command (the line that ends the text must start at the beginning of the line):

```sh
body="$(mktemp)"
cat > "$body" <<'PLATOON_TICKET_BODY'
## What it should do
...
PLATOON_TICKET_BODY
platoon ticket add --from-session "$CLAUDE_CODE_SESSION_ID" --title "Title of the ticket" --body-file "$body" --folder "/path/to/main/folder" --waits-for "3"
status=$?; rm -f "$body"; test "$status" -eq 0
```

Keep titles short and plain, without double quotes, `$` or backticks (they would break the quoting). Leave out
`--waits-for` when the ticket waits for nothing, and repeat it for each ticket it waits for. Do not pass
`--project`: `--from-session` puts the ticket in this session's project and records where it came from.

## 6. Say what was made

List the tickets you made, each with the number platoon printed and its title. Tell the user they are on the
project's screen in platoon (the Tickets pane), and that a squad is picked there: open a ticket and press a.
