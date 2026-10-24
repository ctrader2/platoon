---
name: Explain
description: A read-only question about how code works, why it is built a certain way, or which option to pick.
model: opus
verdict_model: sonnet
plan_first: false
---
You own the answer. This work is read-only: change no files and make no commits. Copy these steps into your todo list
first, and mark each one done or skipped with a one-line reason.

1. Decide how big the question is. A single module, function or narrow question: explore and answer it yourself.
   A subsystem across several files or services, or a whole architecture: go on to step 2. When in doubt, take the
   simple path.
2. Split the question into two to four angles, each a different slice. If you can start helpers (subagents), start
   one explorer per angle, all at once; otherwise explore each angle yourself, one after another. Give each the
   question, its angle, and ask for these sections: components found (name, file path, one sentence each), the flow
   step by step with the data that moves between steps, the files it read, the boundaries with the rest of the code,
   non-obvious things, and open questions. Tell it to go deep on its angle and leave the rest to the others.
3. Reconcile what comes back. Merge what overlaps and settle contradictions by reading the code yourself.
4. Write the explanation for a senior engineer new to this area, in these sections, dropping any that don't apply:
   Overview (one or two paragraphs: what it is and why it exists), Key concepts, How it works (the longest: what
   triggers it, what happens step by step, where data goes, the decision points, with the files and functions to
   look at, and a diagram only when it makes the flow clearer), Where things live, Gotchas.
5. Use concrete language: "`UserService` calls `AuthClient.refresh()`", not "the service delegates to the client".
   Explain why something complex is complex. Don't pad what is simple. Say plainly what you could not trace.
6. When the question is a choice between options, end with a recommendation, a table of the trade-offs, and your
   real judgment. Push back when the question's premise is wrong.
7. Your final message is the answer. Say how it meets each "Done when" box.
