---
description: Verify a finished piece of work (or an agent's report) instead of trusting it
---

Verify the work just finished. Do not repeat the agent's claims: check them. $ARGUMENTS

Run every step **in the folder that holds the code being checked**: during `/land` that is the worktree
(`~/repos/platoon-worktrees/<name>`) after its branch was rebased onto the latest `main`, so the checks are on the combined
code, before anything is pushed.

1. **Commit**: `git fetch -q`, `git status -sb`, `git log --oneline origin/main..HEAD`. Is all the work committed, the
   working tree clean, and the branch directly on top of `origin/main` (`git merge-base --is-ancestor origin/main HEAD`)?
   Confirm `docs/` was not committed (`git ls-files docs | wc -l` is 0). (After landing: `origin/main` equals `HEAD`.)
2. **Baseline of the user's real files** (do this before running tests): `~/.claude/settings.json`, `~/.claude.json` (mtime),
   `ls -la --time-style=full-iso ~/.config/platoon`, `ls ~/.cache/platoon`, and that `~/.config/platoon/themes` does not exist.
3. **Tests, twice**: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test 2>&1 | grep -E 'test result|FAILED|panicked'` two times.
   Compare the counts run to run and against the report. A flaky test is a finding: find its race.
4. **Release build**: `cargo build --release 2>&1 | grep -E 'warning|error'` prints nothing.
5. **Hygiene**: compare the baseline from step 2 with now. `~/.claude/settings.json` must be unchanged (2026-09-19 08:19:39);
   `~/.claude.json` and the user's own `archive.json`/`settings.json` may change on their own, so only flag a change your
   test run could have caused. No stale markers in `~/.cache/platoon/stale`.
6. **Processes**: `ps -eo pid,args | grep -E '[f]ake-claude|[s]leep 6[0-9][0-9]|target/(debug|release)/platoon'` must be empty.
   The user's own `platoon host ... claude --resume ...` processes are theirs; leave them.
7. **Report** to the user: what you verified yourself, what you only know from the agent's report, what nobody has verified
   (real Claude, macOS, anything that needs eyes on colors), and any decision that is theirs. Then update "Where we are" in
   `docs/requirements.md` (from `~/repos/platoon/docs`; a worktree has no `docs/`).
