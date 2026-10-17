---
name: Feature
description: New or changed behaviour in code, planned first and built in small checked steps.
model: opus
verdict_model: fable
plan_first: true
---
You own the design: plan it, build it, verify it. Copy these steps into your todo list first, and mark each one done
or skipped with a one-line reason.

1. Learn how the affected part works before you plan. Read the code yourself. When it spans many files or services,
   split the question into two to four angles and spawn one explorer per angle in a single message with the `Agent`
   tool (`subagent_type: "general-purpose"`, `model: "opus"`), asking each for the components it found (name, file,
   one sentence), the flow step by step, the boundaries, and anything surprising. Check what they report against the
   code.
2. Name the data shape first: the types, and the structure that organises them. Prefer a state machine to scattered
   flags, a table to a chain of branches, a typed model to the same shape assumed in several files.
3. Write the plan: the shape, the files you will touch, the order of small steps that each end in a check, and how
   you will prove the whole works. If the design is contested, follow the Review squad on the plan first. Present
   the plan with `ExitPlanMode` and wait for the user's yes.
4. Subtract before you add: remove dead code and stale paths that stand in the way, then build on the simpler base.
   Make the smallest change that does the job. No layers, options or "just in case" code nobody asked for.
5. Build in small steps. After each one, run its check (a test, the command, the page) and commit. Write tests of what
   a user can observe, not of how the code is built inside.
6. If you hand code-writing to a subagent, give it an exact scope: the files, the data shape, what success looks like.
   Review its diff yourself and run the checks yourself. Its report is a claim, not proof.
7. Prove it works on the real thing: run the feature the way a user would. "It compiles" and "the tests pass" are not
   proof that the behaviour is there. If you could not check something, say so.
8. Before you finish, follow the Review squad on your diff. Fix what it puts under "Act on"; mention the rest.
9. Reply with what you built, what you chose and why, how you checked each "Done when" box, and any decision left
   for the user. Short declarative sentences; each claim says whether you measured it or infer it.
