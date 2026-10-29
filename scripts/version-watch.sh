#!/bin/sh
# The daily version watch (.github/workflows/version-watch.yml runs it once for each program). For one coding agent
# platoon works with, it reads the latest version from the npm registry and, when it is newer than the one in
# checked-with/, opens one issue asking for it to be checked: "Check support for Claude Code X.Y.Z" or "Check support
# for Codex X.Y.Z", labeled claude-code-update or codex-update, assigned to the maintainer. Never twice for the same
# version: an issue with that title, open or closed, means there is nothing to do. It only reports: it never commits,
# pushes or changes a file.
#
#   scripts/version-watch.sh claude-code     # @anthropic-ai/claude-code on npm
#   scripts/version-watch.sh codex           # @openai/codex on npm
#
# Both are read from npm. Its `latest` tag is the release a user gets from each program's own updater, and only that:
# Codex's alphas are on other tags (on GitHub they are releases too, tagged rust-v<X>-alpha.N). The answer is one small
# JSON object, read the same way for both programs, and npm needs no token and has no low rate limit.
#
# Needs curl, jq and (unless DRY_RUN=1 and there's no token) gh. Settings, from the environment:
#   DRY_RUN=1          print what it would do; create nothing (no label, no issue)
#   CHECKED_FILE       the "checked with" file (default: checked-with/<program> beside this script)
#   GH_REPO            owner/repo for gh (the workflow sets it; gh uses it)
#   ASSIGNEE           who the issue is assigned to (default: ctrader2)
#   NPM_URL            where the latest version is read (default: the npm registry's <package>/latest)
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
tool=${1:-}
case "$tool" in
    claude-code) NAME="Claude Code" PACKAGE=@anthropic-ai/claude-code ;;
    codex) NAME=Codex PACKAGE=@openai/codex ;;
    *)
        echo "usage: $0 claude-code|codex" >&2
        exit 2
        ;;
esac
CHECKED_FILE=${CHECKED_FILE:-$root/checked-with/$tool}
ASSIGNEE=${ASSIGNEE:-ctrader2}
NPM_URL=${NPM_URL:-https://registry.npmjs.org/$PACKAGE/latest}
DRY_RUN=${DRY_RUN:-0}
case "$DRY_RUN" in 1 | true) DRY_RUN=1 ;; *) DRY_RUN=0 ;; esac
LABEL=$tool-update

is_version() { printf '%s\n' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; }

# True when version $1 is newer than $2, comparing each part as a number (2.1.10 is newer than 2.1.9).
newer() {
    a=$1 b=$2
    for _ in 1 2 3; do
        x=${a%%.*} y=${b%%.*}
        [ "$x" -gt "$y" ] && return 0
        [ "$x" -lt "$y" ] && return 1
        a=${a#*.} b=${b#*.}
    done
    return 1
}

checked=$(tr -d '[:space:]' < "$CHECKED_FILE")
is_version "$checked" || { echo "error: $CHECKED_FILE should hold one version like 1.2.3, not '$checked'" >&2; exit 1; }

latest=$(curl -fsSL --retry 3 --max-time 30 -H 'Accept: application/json' "$NPM_URL" | jq -r '.version // empty')
is_version "$latest" || { echo "error: npm gave '$latest' as the latest $NAME version" >&2; exit 1; }

echo "$NAME on npm: $latest; platoon last checked with: $checked"
if ! newer "$latest" "$checked"; then
    echo "Nothing to do: npm's version is not newer."
    exit 0
fi

title="Check support for $NAME $latest"

# Never twice for the same version: look for the title among open and closed issues, with the label and without it
# (the label may have been taken off). The exact title must match; search only narrows the list.
existing="" have_gh=""
if command -v gh > /dev/null 2>&1 && { [ -n "${GH_TOKEN:-}" ] || gh auth status > /dev/null 2>&1; }; then
    have_gh=1
    by_label=$(gh issue list --state all --label "$LABEL" --limit 1000 --json number,title \
        --jq ".[] | select(.title == \"$title\") | .number")
    by_title=$(gh issue list --state all --search "\"$title\" in:title" --limit 100 --json number,title \
        --jq ".[] | select(.title == \"$title\") | .number")
    existing=$(printf '%s\n%s\n' "$by_label" "$by_title" | grep -E '^[0-9]+$' | sort -un | head -1 || true)
elif [ "$DRY_RUN" = 1 ]; then
    echo "(dry run without gh access: not checking for an existing issue)"
else
    echo "error: gh is needed (with GH_TOKEN) to look for and open the issue" >&2
    exit 1
fi

if [ -n "$existing" ]; then
    echo "Nothing to do: issue #$existing \"$title\" already exists."
    exit 0
fi

if [ "$tool" = claude-code ]; then
    body=$(cat <<EOF
Claude Code $latest is out ([npm](https://www.npmjs.com/package/@anthropic-ai/claude-code)). platoon was last checked with $checked. platoon reads Claude Code's own files, which aren't a documented format, so check that everything still works before saying it supports $latest.

Check it on your own machine (your Pro plan is enough):

- [ ] Update Claude Code to $latest.
- [ ] Reinstall or build platoon from \`main\`.
- [ ] Run \`cargo test\`.
- [ ] Open platoon and check:
  - [ ] the session list (names, states, model, context)
  - [ ] opening a session, and resuming one
  - [ ] the status line (plan usage)
  - [ ] a project's notes reaching a new session
  - [ ] a hand-off (\`h\`)
  - [ ] a squad run on a small task
  - [ ] tasks drafted from a session (\`t\`), and a plan-first squad stopping with its plan
- [ ] Read [Claude Code's changelog](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md) from $checked to $latest for changes to: transcripts, \`settings.json\` hooks, the status line input, skills, \`--agents\` and \`--permission-mode\`.
- [ ] When it all works: in one commit, change \`checked-with/claude-code\` to $latest and the "last checked with Claude Code" line in README.md and the guide (\`scripts/check-versions.sh\` checks they agree), then close this issue.

If something broke, note it here and fix it first.

_Opened by the daily version watch (.github/workflows/version-watch.yml)._
EOF
    )
else
    body=$(cat <<EOF
Codex $latest is out ([npm](https://www.npmjs.com/package/@openai/codex), [release notes](https://github.com/openai/codex/releases/tag/rust-v$latest)). platoon was last checked with $checked. platoon reads Codex's own files (its saved sessions, \`session_index.jsonl\`, \`models_cache.json\`, the writer locks), adds hooks to its \`hooks.json\` and talks to its app-server; none of that is a documented format, so check that everything still works before saying it supports $latest.

Check it on your own machine (your ChatGPT plan is enough):

- [ ] Update Codex to $latest (\`codex update\`).
- [ ] Reinstall or build platoon from \`main\`, and run \`platoon setup\` (its Codex steps say whether Codex trusts platoon's hooks).
- [ ] Run \`cargo test\`.
- [ ] Open platoon and check:
  - [ ] Codex rows in the session list (names, states, model, context) and Codex's plan usage in the header
  - [ ] opening a Codex conversation, resuming one, and one open in another Codex window
  - [ ] a new Codex session (\`n\`): its row before and after the first message
  - [ ] a project's notes reaching a new Codex session
  - [ ] a hand-off (\`h\`) and tasks drafted (\`t\`) from a Codex conversation
  - [ ] a task on a squad with one of Codex's models, plain and plan-first, and the check of its work
- [ ] Read [Codex's release notes](https://github.com/openai/codex/releases) from $checked to $latest for changes to: the saved session files, hooks and what they are given, the app-server's \`hooks/list\` and \`config/batchWrite\`, \`codex exec --json\`, and the sandbox and approval flags.
- [ ] When it all works: in one commit, change \`checked-with/codex\` to $latest and the "last checked with ... Codex" line in README.md and the guide (\`scripts/check-versions.sh\` checks they agree), then close this issue.

If something broke, note it here and fix it first.

_Opened by the daily version watch (.github/workflows/version-watch.yml)._
EOF
    )
fi

label_missing=unknown
if [ -n "$have_gh" ]; then
    if gh label list --limit 1000 --json name --jq '.[].name' | grep -qx "$LABEL"; then label_missing=no; else label_missing=yes; fi
fi

if [ "$DRY_RUN" = 1 ]; then
    case "$label_missing" in
        yes) echo "Dry run: would create the label \"$LABEL\" (it is missing)." ;;
        no) echo "Dry run: the label \"$LABEL\" is there already." ;;
        *) echo "Dry run: would create the label \"$LABEL\" if it is missing." ;;
    esac
    echo "Dry run: would open issue \"$title\", labeled $LABEL, assigned to $ASSIGNEE, with this text:"
    echo "----"
    printf '%s\n' "$body"
    echo "----"
    exit 0
fi

if [ "$label_missing" = yes ]; then
    gh label create "$LABEL" --color FBCA04 --description "A new $NAME version to check platoon against"
fi
gh issue create --title "$title" --label "$LABEL" --assignee "$ASSIGNEE" --body "$body"
