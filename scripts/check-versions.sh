#!/bin/sh
# The docs say which Claude Code and Codex versions platoon was last checked with ("It was last checked with Claude Code
# X and Codex Y"). checked-with/claude-code and checked-with/codex hold those versions; this fails when README.md or a
# guide page names another one, or when README.md or guide/what-platoon-changes.md no longer say them at all. Run from
# anywhere: `scripts/check-versions.sh`. CI runs it on every push and pull request. Shell and POSIX tools only.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# Where the line must be. Every other guide page is checked too when it names a version (troubleshooting.md does).
required="README.md guide/what-platoon-changes.md"
status=0

# check <file in checked-with/> <the program's name in the docs> <extended regex for the phrase, ending in the version>
check() {
    file=checked-with/$1 name=$2 phrase=$3
    if [ ! -f "$file" ]; then
        echo "error: $file is missing" >&2
        status=1
        return
    fi
    want=$(tr -d '[:space:]' < "$file")
    if ! printf '%s\n' "$want" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
        echo "error: $file should hold one version like 1.2.3, not '$want'" >&2
        status=1
        return
    fi
    bad=0
    for doc in README.md guide/*.md; do
        [ -f "$doc" ] || continue
        # The sentence can wrap across lines: read each file as one line with single spaces.
        found=$(tr -s '[:space:]' ' ' < "$doc" | grep -Eio "$phrase" | awk '{print $NF}' || true)
        case " $required " in
            *" $doc "*)
                if [ -z "$found" ]; then
                    echo "error: $doc no longer says it was last checked with $name $want" >&2
                    bad=1
                fi
                ;;
        esac
        for said in $found; do
            if [ "$said" != "$want" ]; then
                echo "error: $doc says last checked with $name $said, but $file says $want" >&2
                bad=1
            fi
        done
    done
    if [ "$bad" -eq 0 ]; then
        echo "ok: README.md and the guide say last checked with $name $want, as $file does"
    else
        status=1
    fi
}

version='[0-9]+(\.[0-9]+)*'
check claude-code "Claude Code" "last checked with Claude Code $version"
# "last checked with Claude Code X and Codex Y", or "last checked with Codex Y" on its own.
check codex "Codex" "last checked with (Claude Code $version and )?Codex $version"

if [ "$status" -ne 0 ]; then
    echo "Change checked-with/ and the docs together, in one commit." >&2
fi
exit "$status"
