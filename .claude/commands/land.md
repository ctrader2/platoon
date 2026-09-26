---
description: Land a worktree's branch on main - rebase, resolve, verify the combined code, push, write the notes, clean up
---

Land the piece of work `$ARGUMENTS` (worktree `~/repos/platoon-worktrees/$ARGUMENTS`, branch `work/$ARGUMENTS`).
One branch lands at a time; never land two at once.

1. **Ready?** In the worktree: `git status -sb` is clean and the agent's report says it is green. Anything uncommitted: ask
   the agent (SendMessage) rather than committing it yourself.
2. **Rebase onto the latest main**: `git -C ~/repos/platoon fetch -q && git -C ~/repos/platoon merge --ff-only origin/main`
   (the main checkout must be clean and on `main`), then in the worktree `git rebase main`.
   - A **mechanical** conflict (both sides added lines to the same list, the CLAUDE.md file table, imports): resolve it,
     `git add` by explicit path, `git rebase --continue`.
   - A conflict that **needs judgement** (two changes to the same behaviour, a moved function that was also edited):
     `git rebase --abort` and send it back to the agent that wrote the branch, with the conflicting files and what `main`
     changed. It rebases and resolves; then start again at step 1.
   - Never `--force` push, never rewrite `main`; the branch is local, so rebasing it is fine.
3. **Verify the combined code**: run `/verify` in the worktree (tests twice, release build, the user's real files, stray
   processes). A failure that is not in the branch alone is a finding about the combination: fix it on the branch
   (or send it back), commit, and verify again.
4. **Land**: `git -C ~/repos/platoon merge --ff-only work/$ARGUMENTS` (must fast-forward; if `main` moved, go back to
   step 2), then `git -C ~/repos/platoon push origin main`. Check `git log --oneline origin/main | head -3`.
5. **Notes**: add the agent's "built" write-up as a dated section of `docs/requirements.md`, with what you verified, and
   update "Where we are" (mention any other worktrees still in progress).
6. **Clean up**: `git worktree remove ~/repos/platoon-worktrees/$ARGUMENTS` and `git branch -d work/$ARGUMENTS` (`-d`, not
   `-D`: it refuses if the branch did not land). Then every other branch waiting to land must be rebased and re-verified
   before it lands; tell the user which ones those are.
