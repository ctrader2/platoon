#!/usr/bin/env bash
# Orients a new session: repo state and the "Where we are" section of the notes. Output is added to the session's context.
cd "${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}" || exit 0
echo "== platoon: where things stand =="
git status -sb 2>/dev/null | head -12
echo
git log --oneline -5 2>/dev/null
echo
if [ "$(git worktree list 2>/dev/null | wc -l)" -gt 1 ]; then
  echo "Work in progress (worktrees, land with /land <name>):"
  git worktree list 2>/dev/null | tail -n +2
  echo
fi
if [ -f docs/requirements.md ]; then
  awk '/^## Where we are/{p=1; print; next} p && /^## /{exit} p' docs/requirements.md
else
  echo "(docs/ is missing. Clone ctrader2/project-docs to ~/repos/project-docs and link it:"
  echo "  ln -s ../project-docs/platoon ~/repos/platoon/docs)"
fi
exit 0
