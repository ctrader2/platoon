---
name: Incident
description: Something is broken for users now: stop the damage, find the cause from the evidence, fix it and write it up.
model: opus
verdict_model: fable
plan_first: false
---
You own this incident until it is understood. Stop the damage first, then be scientific: every claim about the cause
traces to evidence you saw. Copy these steps into your todo list first, and mark each one done or skipped with a
one-line reason.

1. Stabilise first. Find the smallest safe way to stop the damage: roll back the change that started it, turn off a
   feature flag, change a config value. Others see it, so ask the user with `AskUserQuestion` before you do it, saying
   what it changes, what it costs and how to undo it. If nothing safe exists, say so and go on.
2. Gather the evidence. Use whatever tools this session has: error tracking, logs, metrics, team chat, recent deploys
   (MCP tools), and the repository's history (`git log` around the time it started, `git blame` on the lines
   involved, reverts and "fix for" commits). When there are several sources, spawn one explorer per source in a
   single message with the `Agent` tool (`subagent_type: "general-purpose"`, `model: "opus"`), each told the time
   window and the symptom and asked for what it found with links or file:line. Record what a source did not have too.
3. Build the timeline: when it started, what changed just before, when it was noticed, what was done since. Times
   with their source.
4. Prove the cause before you believe it. Write down the candidate causes, then cut them down with evidence: reproduce
   it, or show it in the logs or the code. The most recent change is not the cause until the evidence says so.
5. Make the smallest fix that removes the cause, where it first goes wrong, not where it shows. Write a test that fails
   without the fix first when a cheap one exists, commit it, then the fix on top. Nothing that "might help" ships.
6. Verify on the surface where it broke: the reproduction passes now, or the evidence shows the symptom has stopped.
   Then find how this repo runs its tests (its CLAUDE.md, README, or build files) and run them all; fix what fails.
7. Reply with a short postmortem: impact (who, what, for how long), the timeline, the cause and its evidence, what was
   done to stabilise, the fix, the test command you ran and its result, and what would have caught it sooner. Say
   what is still unknown, and how you checked each "Done when" box.
