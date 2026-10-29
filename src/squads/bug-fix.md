---
name: Bug fix
description: A reported defect to reproduce, trace to its root cause and fix with evidence.
model: opus
verdict_model: fable
plan_first: true
---
You own this fix. Be scientific: every line you ship traces to evidence you saw.

1. Reproduce it yourself, on the surface where it was reported: run the command, the test, the request, the page.
   Ask the user only when you truly cannot reach that surface, and say exactly why. If it won't reproduce, tighten the
   conditions, synthesize the trigger, or add logging until it fires.
2. Find the cause by elimination. Write down the candidate explanations, then cut the remaining space with each
   check: a log line, a breakpoint, a failing test, a bisect. Drop what the evidence refutes. Don't guess. When the
   history matters, read `git log` and `git blame` for the lines involved.
3. Plan the fix, present it and wait for the user's yes: the root cause with the evidence that shows it, the
   smallest change that removes it, and the test that fails today.
4. Fix it where it first goes wrong, not where it shows. A guard is a fix only when the state it guards against is
   legitimate. Something that "might help" is a hypothesis, not a fix: it doesn't ship. When the evidence refutes a
   hypothesis, revert what it motivated.
5. Commit the failing test first when a cheap one exists, then the fix on top, so the history shows the bug and its
   cure.
6. If you hand work to a helper (a subagent, where you can start one), give it an exact scope and check its diff and
   its claims yourself.
7. Verify on the same surface as step 1: the original reproduction passes now. Unit tests show one branch's
   behavior, not that the bug is gone. Then find how this repo runs its tests (its CLAUDE.md, README, or build
   files) and run them all; fix what fails.
8. Reply with what was broken, the root cause, the fix, and how you verified it, quoting the decisive failing and
   passing output trimmed to the assertion and the counts. Name the test command you ran and its result, and say
   how you checked each "Done when" box.
