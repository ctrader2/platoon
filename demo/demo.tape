# The README animation's raw recording: a tour of platoon in a made-up world (demo/make-world.sh).
# Run from the repository root after `cargo build --release`:  vhs demo/demo.tape   (then demo/render.sh)
# It writes demo/raw.mp4 (not committed: demo/.gitignore), which demo/render.sh turns into demo/platoon.gif and
# demo/platoon-tour.mp4. The terminal is 120 columns by 34 rows: big enough letters to read at the size GitHub shows
# the GIF, with no zooming.

Output demo/raw.mp4

Require bash
Require flock

Set Shell bash
Set FontFamily "DejaVu Sans Mono"
Set FontSize 16
Set Width 1264
Set Height 704
Set Padding 16
Set Theme "Dracula"
Set Framerate 20
Set TypingSpeed 90ms
Set CursorBlink false

# ---- hidden: build the world, point everything into it, start the stand-in running sessions and platoon ----------
Hide
Type `P="$PWD/target/release/platoon"; W="$(mktemp -d -t platoon-demo.XXXXXX)" && demo/make-world.sh "$W" > /dev/null && ln -s "$P" "$W/bin/platoon" && . "$W/env.sh" && FINISH_AFTER=7 "$W/live.sh" start && PS1='$ ' && clear && platoon`
Enter
Sleep 2.5s
Show

# ---- the Sessions list: Claude Code and Codex side by side (one session finishes while we look) -------------------
Sleep 4s
Down@700ms 4
Sleep 2s

# ---- search: the quick filter, then full text -------------------------------------------------------------------
Type "/"
Sleep 700ms
Type@120ms "webhook"
Sleep 1.2s
Tab
Sleep 2.8s
Enter
Sleep 1s

# ---- open a Codex conversation, and back -----------------------------------------------------------------------
Down 5
Sleep 800ms
Enter
Sleep 3s
Ctrl+Space
Sleep 1.2s

# ---- archive -----------------------------------------------------------------------------------------------------
Down 2
Sleep 800ms
Type "a"
Sleep 1.5s
Right
Sleep 2s
Left
Sleep 800ms

# ---- projects, notes ---------------------------------------------------------------------------------------------
Type "2"
Sleep 2s
Down
Sleep 600ms
Enter
Sleep 2.5s
Tab
Sleep 600ms
Down@250ms 6
Sleep 1.5s
Escape
Sleep 600ms

# ---- tasks: one running in Codex, one waiting for an answer; starting one with a squad ----------------------------
Type "3"
Sleep 2.5s
Down
Sleep 3s
Down
Sleep 1s
Type "s"
Sleep 1.2s
Down@250ms 7
Sleep 2.5s
Escape
Sleep 600ms

# ---- the editor ----------------------------------------------------------------------------------------------------
Type "v"
Sleep 1.8s
Enter
Sleep 2s

# ---- the Squads tab -------------------------------------------------------------------------------------------------
Type "4"
Sleep 2s
Type "G"
Sleep 3s

# ---- settings, help -------------------------------------------------------------------------------------------------
Type "1"
Sleep 800ms
Type "s"
Sleep 1.4s
Tab@1s 3
Sleep 1s
Escape
Sleep 600ms
Type "?"
Sleep 3s
Escape
Sleep 600ms

# ---- themes: preview a few dark ones, then back to Dracula ----------------------------------------------------------
Type "s"
Sleep 900ms
Enter
Sleep 2s
Down@1s 2
Sleep 1.2s
Escape
Sleep 700ms
Escape
Sleep 3s

# ---- hidden: quit, stop what the world started, remove it ----------------------------------------------------------
Hide
Type "q"
Sleep 500ms
Type `"$W/live.sh" stop; rm -rf "$W"`
Enter
Sleep 1s
