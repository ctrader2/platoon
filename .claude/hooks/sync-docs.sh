#!/usr/bin/env bash
# End of turn: commit and push changes to the notes repo (docs/ is a symlink into it). Never blocks Claude: always exit 0.
root="${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"
target=$(readlink -f "$root/docs" 2>/dev/null) || exit 0
[ -d "$target" ] || exit 0
repo=$(git -C "$target" rev-parse --show-toplevel 2>/dev/null) || exit 0
[ -n "$(git -C "$repo" status --porcelain 2>/dev/null)" ] || exit 0

# The notes are for a private repo, but never let a credential ride along.
if git -C "$repo" diff HEAD -U0 2>/dev/null | grep -E '^\+' | grep -q -E 'gh[opsu]_[A-Za-z0-9]{20,}|sk-ant-[A-Za-z0-9_-]{10,}|AKIA[0-9A-Z]{16}|BEGIN [A-Z ]*PRIVATE KEY'; then
  echo "project-docs: NOT synced, the changes look like they contain a secret. Check ~/repos/project-docs." >&2
  exit 0
fi

git -C "$repo" add -A
git -C "$repo" commit -q -m "docs: update notes" -m "Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>" >/dev/null 2>&1 || exit 0
timeout 30 git -C "$repo" push -q >/dev/null 2>&1 || echo "project-docs: committed but the push failed; it will be retried next turn." >&2
exit 0
