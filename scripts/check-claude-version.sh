#!/bin/sh
# The docs say which Claude Code version platoon was last checked with ("It was last checked with Claude Code X").
# checked-with/claude-code holds that version; this fails when README.md or a guide page names another one, or when
# README.md or guide/what-platoon-changes.md no longer say it at all. Run from anywhere: `scripts/check-claude-version.sh`.
# CI runs it on every push and pull request. Shell and POSIX tools only.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

file=checked-with/claude-code
[ -f "$file" ] || { echo "error: $file is missing" >&2; exit 1; }
want=$(tr -d '[:space:]' < "$file")
if ! printf '%s\n' "$want" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "error: $file should hold one version like 2.1.288, not '$want'" >&2
    exit 1
fi

# Where the line must be. Every other guide page is checked too when it names a version (troubleshooting.md does).
required="README.md guide/what-platoon-changes.md"
status=0

for doc in README.md guide/*.md; do
    [ -f "$doc" ] || continue
    # The sentence can wrap across lines: read each file as one line with single spaces.
    found=$(tr -s '[:space:]' ' ' < "$doc" | grep -Eio 'last checked with Claude Code [0-9]+(\.[0-9]+)*' | awk '{print $NF}' || true)
    case " $required " in
        *" $doc "*)
            if [ -z "$found" ]; then
                echo "error: $doc no longer says \"last checked with Claude Code $want\"" >&2
                status=1
            fi
            ;;
    esac
    for v in $found; do
        if [ "$v" != "$want" ]; then
            echo "error: $doc says last checked with Claude Code $v, but $file says $want" >&2
            status=1
        fi
    done
done

if [ "$status" -eq 0 ]; then
    echo "ok: README.md and the guide say last checked with Claude Code $want, as $file does"
else
    echo "Change $file and the docs together, in one commit." >&2
fi
exit "$status"
