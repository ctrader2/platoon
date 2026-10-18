---
name: Refactor
description: A change to the structure of code that keeps its behaviour exactly as it is.
model: opus
verdict_model: fable
plan_first: true
---
You own the contract. The structure changes; the behaviour does not. If you find a missing feature or a real bug on
the way, note it and leave it for its own task. Copy these steps into your todo list first, and mark each one done or
skipped with a one-line reason.

1. Pin the behaviour before anything moves. Learn the contract from the code and its callers, then make sure a
   characterization test, snapshot or comparison script captures what it does now. Write one if nothing covers it.
   A type check or a lint is not a pin.
2. Name the structure the code is missing: a state machine, a table, a typed model, a boundary. The reshape must
   delete branches or impossible states, not add indirection. Code whose shape is already clear stays.
3. Name the target: the module layout, the types and the call graph as you would write them today. Present the plan
   with `ExitPlanMode`: the pin, the target, the steps.
4. Subtract first. Delete dead code, collapse wrappers with one caller, drop redundant checks and stale references,
   then reshape. The smallest change that reaches the target ships.
5. Move in small steps that each keep the pin green, and commit each one. When an internal API changes, move every
   caller and delete the old API in the same change: no shims, no old and new paths side by side. Search for each
   renamed name in strings, docs and comments too.
6. If you hand mechanical edits to a subagent (`Agent` tool, `subagent_type: "general-purpose"`, `model: "sonnet"`),
   give it the files, the names and the behaviour to hold, and check its diff yourself.
7. Prove the behaviour is unchanged on the real thing: run the pin, and for a larger reshape compare old and new
   outputs or run the program end to end. Then find how this repo runs its tests (its CLAUDE.md, README, or build
   files) and run them all; fix what fails.
8. Keep the change only if the code is easier to read afterwards. If it isn't, revert it and say so.
9. Reply with the structure that changed, the pin you held it against, the proof that behaviour is unchanged, the
   test command you ran and its result, and what you reverted. Say how you checked each "Done when" box.
