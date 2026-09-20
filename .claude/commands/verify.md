---
description: Verify a finished piece of work (or an agent's report) instead of trusting it
---

Verify the work just finished. Do not repeat the agent's claims: check them. $ARGUMENTS

1. **Commit**: `git fetch -q`, `git log --oneline origin/main | head -3`, `git status -sb`. Is the work committed and pushed,
   and is the working tree otherwise clean? Confirm `docs/` was not committed (`git ls-files docs | wc -l` is 0).
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
   `docs/requirements.md`.
