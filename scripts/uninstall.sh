#!/bin/sh
# Uninstall platoon completely:
#
#   curl -LsSf https://raw.githubusercontent.com/ctrader2/platoon/main/scripts/uninstall.sh | sh
#   curl -LsSf https://raw.githubusercontent.com/ctrader2/platoon/main/scripts/uninstall.sh | sh -s -- --all
#
# 1. When a platoon is found, `platoon uninstall` takes platoon's status line and hooks out of Claude Code, deletes platoon's cache folder, asks before deleting your projects, notes, tasks and settings, and
#    asks before stopping the sessions it runs (a "no" there stops everything, with nothing changed).
#    When none is found (or one too old to have `uninstall`), this script asks the same questions and deletes the same
#    folders itself, and says how to take the entries out of Claude Code's settings by hand.
# 2. Then it removes the program the way it was installed: Homebrew, cargo, or the install script (its receipt in
#    ~/.config/platoon-tui says where it put platoon). Anything else: it says where the program is.
#
# Flags: --all (also delete your projects, notes, tasks and settings, without asking), --stop-sessions (stop the
# sessions platoon runs without asking), --help.
#
# It honours the variables platoon does (HOME, CLAUDE_CONFIG_DIR, PLATOON_CONFIG_DIR, PLATOON_CACHE_DIR,
# XDG_CONFIG_HOME, XDG_CACHE_HOME, XDG_DATA_HOME, CARGO_HOME, ZDOTDIR), so tests run it in made-up folders.
# Test knobs: PLATOON_UNINSTALL_TTY (a file the answers are read from instead of /dev/tty) and
# PLATOON_UNINSTALL_TEST_HOME (refuse to run unless every folder is inside it).
#
# POSIX sh (dash, bash, macOS sh), with no GNU-only flags. Everything is in functions and runs from the last line, so
# `curl ... | sh` has read the whole script before anything starts.

set -u

GUIDE="https://github.com/ctrader2/platoon/blob/main/guide/install-and-setup.md#uninstall"

say() {
    printf '%s\n' "$*"
}

usage() {
    say "usage: uninstall.sh [--all] [--stop-sessions]"
    say "  Removes platoon: its entries in Claude Code's settings, its cache folder and the program itself."
    say "  Asks before deleting your projects, notes, tasks and settings, and before stopping the sessions"
    say "  platoon is running. Claude Code's conversations are never touched."
    say "  --all             also delete your projects, notes, tasks and settings, without asking"
    say "  --stop-sessions   stop the sessions platoon is running without asking"
    say "  With flags through curl: curl -LsSf <url> | sh -s -- --all"
}

# Single-quote for the shell when needed, for commands the user may copy.
quote() {
    case $1 in
        *[!A-Za-z0-9/._+:@%=,-]* | '') printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")" ;;
        *) printf '%s' "$1" ;;
    esac
}

# The lists for the summary, one entry per line.
REMOVED=""
KEPT=""
FAILED=""
removed() { REMOVED="$REMOVED  - $*
"; }
kept() { KEPT="$KEPT  - $*
"; }
failed() { FAILED="$FAILED  - $*
"; }

# ---- answers: from the terminal itself, so `curl ... | sh` can still ask ----------------------------------------

HAVE_TTY=0
open_tty() {
    tty_in=${PLATOON_UNINSTALL_TTY:-/dev/tty}
    # Tried in a subshell first: a failed redirection on `exec` would end the script.
    if [ -r "$tty_in" ] && (exec 3<"$tty_in") 2>/dev/null; then
        exec 3<"$tty_in"
        HAVE_TTY=1
    fi
}

# ask "question": true for a clear yes. Anything else, or no answer, is a no.
ask() {
    printf '%s [y/N] ' "$1"
    if [ "$HAVE_TTY" = 1 ] && read -r answer <&3; then
        case $answer in
            [yY] | [yY][eE][sS]) return 0 ;;
        esac
        return 1
    fi
    say ""
    return 1
}

# ---- folders ----------------------------------------------------------------------------------------------------

# May this folder be deleted? Absolute, not the home folder or above it, not Claude Code's folder or above it.
deletable() {
    case $1 in
        /*/*) ;;
        *) return 1 ;;
    esac
    case $1 in
        */../* | */.. | */./* | */.) return 1 ;;
    esac
    case "$HOME/" in "$1"/*) return 1 ;; esac
    case "$CLAUDE_ROOT/" in "$1"/*) return 1 ;; esac
    return 0
}

remove_dir() { # remove_dir <folder> <what it is>
    [ -e "$1" ] || return 1
    if ! deletable "$1"; then
        failed "$2 ($1) was left alone: it is your home folder, Claude Code's, or holds one of them"
        return 1
    fi
    if rm -rf "$1" 2>/dev/null && [ ! -e "$1" ]; then
        removed "$2 ($1)"
        return 0
    fi
    failed "couldn't delete $2 ($1)"
    return 1
}

# ---- what platoon is running (only when platoon itself can't say) ---------------------------------------------

# Prints "pid<TAB>dir<TAB>id" for each session one of platoon's hosts runs whose claude is alive.
running_sessions() {
    for f in "$CACHE"/hosts/*.json; do
        [ -f "$f" ] || continue
        pid=$(sed -n 's/.*"claude_pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$f" | head -n 1)
        [ -n "$pid" ] && [ "$pid" -gt 1 ] 2>/dev/null || continue
        kill -0 "$pid" 2>/dev/null || continue
        dir=$(sed -n 's/.*"dir":[[:space:]]*"\([^"]*\)".*/\1/p' "$f" | head -n 1)
        id=$(sed -n 's/.*"--resume",[[:space:]]*"\([^"]*\)".*/\1/p' "$f" | head -n 1)
        printf '%s\t%s\t%s\n' "$pid" "$dir" "$id"
    done
}

# SIGTERM each claude, as platoon's own stop does, and wait up to 5 seconds for them to exit.
stop_sessions() {
    for pid in $1; do
        kill -TERM "$pid" 2>/dev/null
    done
    tries=0
    while [ "$tries" -lt 50 ]; do
        alive=0
        for pid in $1; do
            if kill -0 "$pid" 2>/dev/null; then alive=1; fi
        done
        [ "$alive" = 0 ] && return 0
        sleep 0.1 2>/dev/null || sleep 1
        tries=$((tries + 1))
    done
    return 1
}

# The task worktrees the tasks files record that are still there (folders `task-N`), with the git commands that
# remove them.
list_worktrees() {
    found=""
    for t in "$CONFIG"/tasks.json "$CONFIG"/projects/*/tasks.json; do
        [ -f "$t" ] || continue
        found="$found$(grep -o '"path":[[:space:]]*"[^"]*-worktrees/task-[0-9][0-9]*[^"]*"' "$t" |
            sed 's/^"path":[[:space:]]*"//; s/"$//; s#\(-worktrees/task-[0-9][0-9]*\).*#\1#')
"
    done
    printf '%s' "$found" | sort -u | while IFS= read -r d; do
        [ -n "$d" ] && [ -d "$d" ] || continue
        name=${d##*/}
        n=${name##*-}
        branch="${name%-*}/$n"
        repo=""
        if [ -f "$d/.git" ]; then
            repo=$(sed -n 's#^gitdir: \(.*\)/\.git/worktrees/.*#\1#p' "$d/.git" | head -n 1)
        fi
        say "  #$n:"
        if [ -n "$repo" ]; then
            say "    git -C $(quote "$repo") worktree remove $(quote "$d")"
            say "    git -C $(quote "$repo") branch -D $branch"
        else
            say "    git worktree remove $(quote "$d")   (in the repository it was made from)"
        fi
    done
}

# ---- without platoon: the same questions, the folders, and the settings by hand ----------------------------------

without_platoon() {
    sessions=$(running_sessions)
    if [ -n "$sessions" ]; then
        count=$(printf '%s\n' "$sessions" | wc -l | tr -d ' ')
        them=them
        if [ "$count" = 1 ]; then
            say "platoon is running 1 session:"
            them=it
        else
            say "platoon is running $count sessions:"
        fi
        printf '%s\n' "$sessions" | while IFS="$(printf '\t')" read -r pid dir id; do
            line="  - a session"
            [ -n "$dir" ] && line="$line in $dir"
            line="$line (pid $pid)"
            [ -n "$id" ] && line="$line: \`claude --resume $id\` opens it again"
            say "$line"
        done
        say "Their conversations are saved: \`claude --resume <id>\` opens them again."
        if [ "$STOP" = 1 ]; then
            say "Stopping them (--stop-sessions)."
        elif [ "$HAVE_TTY" = 1 ]; then
            if ! ask "Stop $them and go on?"; then
                say "Left them running. Nothing was changed."
                return 1
            fi
        else
            say "There is no terminal to ask in, so they were left running and nothing was changed."
            say "Run this again in a terminal, or add --stop-sessions to stop them without asking."
            return 1
        fi
    fi
    delete_config=0
    if [ -d "$CONFIG" ]; then
        if [ "$ALL" = 1 ]; then
            delete_config=1
        elif [ "$HAVE_TTY" = 1 ]; then
            say "Your projects, notes, tasks, squads and settings are in $CONFIG."
            if ask "Also delete your projects, notes, tasks and settings?"; then delete_config=1; fi
        fi
    fi
    say ""
    WORKTREES=$(list_worktrees)

    if [ -n "$sessions" ]; then
        pids=$(printf '%s\n' "$sessions" | cut -f 1 | tr '\n' ' ')
        if stop_sessions "$pids"; then
            removed "the sessions platoon was running: stopped (their conversations are saved)"
        else
            say "Couldn't stop the sessions: they didn't exit in time. Nothing else was changed: run this again in a moment."
            return 1
        fi
    fi
    return 0
}

remove_folders() { # after the program
    remove_dir "$CACHE" "platoon's cache folder"
    if [ -d "$CONFIG" ]; then
        if [ "$delete_config" = 1 ]; then
            remove_dir "$CONFIG" "your projects, notes, tasks and settings"
        else
            why=""
            [ "$HAVE_TTY" = 1 ] || why=" (there was no terminal to ask in; --all deletes it)"
            kept "your projects, notes, tasks and settings in $CONFIG$why. To delete them later: rm -rf $(quote "$CONFIG")"
        fi
    fi
    remove_dir "$DATA/platoon" "an older platoon's data folder"
    if remove_dir "$DATA/fonts/platoon" "an older platoon's font"; then
        if command -v fc-cache >/dev/null 2>&1; then fc-cache -f >/dev/null 2>&1 </dev/null; fi
    fi
    if [ "$(uname -s 2>/dev/null)" = Darwin ]; then
        remove_dir "$HOME/Library/Application Support/platoon" "an older platoon's data folder"
    fi
}

BY_HAND=0
by_hand_notes() {
    settings="$CLAUDE_ROOT/settings.json"
    if [ -f "$settings" ] && grep -q 'platoon' "$settings" 2>/dev/null; then
        BY_HAND=1
        say ""
        say "Claude Code's settings still mention platoon, and without platoon they can't be taken out for you."
        say "Open $settings and delete the \"statusLine\" entry if its command ends in \`platoon statusline\`, and"
        say "each hook whose command ends in \`platoon hook guard\`, \`platoon hook session-start\` or \`platoon hook stop\`."
        say "Until then Claude Code reports those hooks failing. More: $GUIDE"
    fi
}

# ---- the program ------------------------------------------------------------------------------------------------

# Take the line the install script added to shell startup files out again: exactly `. "<...>/platoon-tui/env.sh"`.
strip_path_line() {
    f=$1
    [ -f "$f" ] || return 0
    grep -q 'platoon-tui/env\.sh"' "$f" 2>/dev/null || return 0
    tmp="$f.platoon-uninstall.$$"
    grep -v -e '^\. "[^"]*/platoon-tui/env\.sh"$' -e '^source "[^"]*/platoon-tui/env\.sh"$' "$f" >"$tmp"
    if [ $? -le 1 ]; then
        # Written over in place, so the file keeps its permissions (and stays a link if it is one).
        cat "$tmp" >"$f" && removed "the line the install script added to $f"
    else
        failed "couldn't take the install script's line out of $f"
    fi
    rm -f "$tmp"
}

remove_program() {
    # Homebrew
    if command -v brew >/dev/null 2>&1 && brew list --formula platoon >/dev/null 2>&1 </dev/null; then
        if brew uninstall platoon </dev/null; then
            removed "the program, with Homebrew (brew uninstall platoon)"
        else
            failed "brew uninstall platoon didn't work: run it yourself"
        fi
    fi
    # cargo
    if command -v cargo >/dev/null 2>&1 && cargo install --list </dev/null 2>/dev/null | grep -q '^platoon-tui v'; then
        if cargo uninstall platoon-tui </dev/null; then
            removed "the program, with cargo (cargo uninstall platoon-tui)"
        else
            failed "cargo uninstall platoon-tui didn't work: run it yourself"
        fi
    fi
    # The install script: its receipt says where it put platoon.
    receipt="$RECEIPT_DIR/platoon-tui-receipt.json"
    if [ -f "$receipt" ]; then
        prefix=$(sed -n 's/.*"install_prefix":[[:space:]]*"\([^"]*\)".*/\1/p' "$receipt" | head -n 1)
        layout=$(sed -n 's/.*"install_layout":[[:space:]]*"\([^"]*\)".*/\1/p' "$receipt" | head -n 1)
        case $prefix in
            /*)
                bin="$prefix/platoon"
                [ "$layout" = flat ] || bin="$prefix/bin/platoon"
                if [ -f "$bin" ] || [ -L "$bin" ]; then
                    if rm -f "$bin"; then removed "the program, put there by the install script ($bin)"; else failed "couldn't delete $bin"; fi
                fi
                ;;
            *) failed "the install script's receipt ($receipt) doesn't say where platoon is" ;;
        esac
        for rc in .profile .bashrc .bash_profile .bash_login; do
            strip_path_line "$HOME/$rc"
        done
        for rc in .zshrc .zshenv; do
            strip_path_line "${ZDOTDIR:-$HOME}/$rc"
        done
        fish="$HOME/.config/fish/conf.d/platoon-tui.env.fish"
        if [ -f "$fish" ] && grep -q 'platoon-tui/env\.fish' "$fish" 2>/dev/null; then
            rm -f "$fish" && removed "the install script's fish file ($fish)"
        fi
        remove_dir "$RECEIPT_DIR" "the install script's folder"
    fi
    # Whatever platoon is still on the path.
    hash -r 2>/dev/null
    left=$(command -v platoon 2>/dev/null)
    if [ -n "$left" ]; then
        case $left in
            /nix/store/* | */.nix-profile/*)
                kept "the program at $left: it came from Nix, so take it out of your Nix profile or configuration" ;;
            *)
                kept "the program at $left: it wasn't installed with Homebrew, cargo or the install script. Delete it: rm $(quote "$left")" ;;
        esac
    fi
}

# ---- the whole of it ---------------------------------------------------------------------------------------------

main() {
    ALL=0
    STOP=0
    for arg in "$@"; do
        case $arg in
            --all) ALL=1 ;;
            --stop-sessions) STOP=1 ;;
            -h | --help)
                usage
                return 0
                ;;
            *)
                say "uninstall.sh: don't know \"$arg\""
                usage
                return 2
                ;;
        esac
    done
    if [ -z "${HOME:-}" ]; then
        say "HOME isn't set, so platoon's folders can't be found. Nothing was changed."
        return 1
    fi

    # The folders, found the way platoon finds them.
    CLAUDE_ROOT=${CLAUDE_CONFIG_DIR:-$HOME/.claude}
    CONFIG=${PLATOON_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/platoon}
    CACHE=${PLATOON_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/platoon}
    DATA=${XDG_DATA_HOME:-$HOME/.local/share}
    RECEIPT_DIR=${XDG_CONFIG_HOME:-$HOME/.config}/platoon-tui

    if [ -n "${PLATOON_UNINSTALL_TEST_HOME:-}" ]; then
        for p in "$HOME" "$CLAUDE_ROOT" "$CONFIG" "$CACHE" "$DATA" "$RECEIPT_DIR" "${CARGO_HOME:-$HOME}" "${ZDOTDIR:-$HOME}"; do
            case "$p/" in
                "$PLATOON_UNINSTALL_TEST_HOME"/*) ;;
                *)
                    say "PLATOON_UNINSTALL_TEST_HOME is $PLATOON_UNINSTALL_TEST_HOME, but $p is outside it: refusing to run."
                    return 1
                    ;;
            esac
        done
    fi

    say "Uninstalling platoon"
    say ""
    platoon=$(command -v platoon 2>/dev/null)
    if [ -n "$platoon" ] && "$platoon" --version >/dev/null 2>&1 </dev/null; then
        set --
        [ "$ALL" = 1 ] && set -- "$@" --all
        [ "$STOP" = 1 ] && set -- "$@" --stop-sessions
        PLATOON_UNINSTALL_FROM_SCRIPT=1 "$platoon" uninstall "$@" </dev/null
        code=$?
        if [ "$code" != 0 ]; then
            say ""
            say "platoon uninstall stopped (see above), so the program was left where it is."
            return "$code"
        fi
        say ""
        remove_program
        if [ -n "$REMOVED$FAILED$KEPT" ]; then
            say "The program:"
            printf '%s' "$REMOVED$KEPT"
            [ -z "$FAILED" ] || printf "Couldn't remove:\n%s" "$FAILED"
        fi
    else
        if [ -n "$platoon" ]; then
            say "The platoon at $platoon is too old to uninstall itself, so this script does what it can."
        else
            say "No platoon program was found on your path, so this script does what it can without it."
        fi
        open_tty
        without_platoon || return 1
        remove_program
        remove_folders
        kept "your conversations: they are Claude Code's, and \`claude --resume\` opens any of them"
        if [ -n "$REMOVED" ]; then printf 'Removed:\n%s' "$REMOVED"; else say "Nothing of platoon's was left to remove."; fi
        [ -z "$FAILED" ] || printf "Couldn't remove:\n%s" "$FAILED"
        printf 'Kept:\n%s' "$KEPT"
        if [ -n "$WORKTREES" ]; then
            say "Task worktrees and branches (never deleted: remove each once you no longer need its work):"
            say "$WORKTREES"
        fi
        by_hand_notes
    fi
    if [ -n "$FAILED" ]; then
        say "Fix what couldn't be removed and run this again: what is gone stays gone."
        return 1
    fi
    say ""
    if [ "$BY_HAND" = 1 ]; then
        say "platoon is uninstalled, but for what is above to do by hand."
    else
        say "platoon is uninstalled."
    fi
    return 0
}

main "$@"
