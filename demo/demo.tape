# The README animation's raw recording: a tour of platoon in a made-up world (demo/make-world.sh).
# Run from the repository root after `cargo build --release`:  vhs demo/demo.tape   (then demo/render.sh)
# It writes demo/raw.mp4 (not committed: demo/.gitignore), which demo/render.sh turns into demo/platoon.gif and demo/platoon-tour.mp4.

Output demo/raw.mp4

Require bash

Set Shell bash
Set FontFamily "DejaVu Sans Mono"
Set FontSize 22
Set Width 1920
Set Height 1080
Set Padding 24
Set Margin 36
Set MarginFill "#11111b"
Set WindowBar Colorful
Set BorderRadius 12
Set Theme "Catppuccin Mocha"
Set Framerate 15
Set TypingSpeed 90ms
Set CursorBlink false

# ---- hidden: build the world, point everything into it, start the two stand-in running sessions ----------------
Hide
Type `P="$PWD/target/release/platoon"; W="$(mktemp -d -t platoon-demo.XXXXXX)" && demo/make-world.sh "$W" > /dev/null && ln -s "$P" "$W/bin/platoon" && . "$W/env.sh" && FINISH_AFTER=8.5 "$W/live.sh" start && PS1='$ ' && clear`
Enter
Sleep 1.5s
Show

# ---- the Sessions list (one session is working and finishes while we look) ------------------------------------
Sleep 600ms
Type "platoon"
Sleep 400ms
Enter
Sleep 6s
Down@600ms 4
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

# ---- open a session, and back -----------------------------------------------------------------------------------
Down
Sleep 600ms
Enter
Sleep 3s
Ctrl+Space
Sleep 1.2s

# ---- archive -----------------------------------------------------------------------------------------------------
Down 5
Sleep 800ms
Type "a"
Sleep 1.5s
Right
Sleep 2s
Left
Sleep 800ms

# ---- projects, notes ---------------------------------------------------------------------------------------------
Type "2"
Sleep 2.2s
Down
Sleep 600ms
Enter
Sleep 3s
Tab 2
Sleep 3s

# ---- tickets, a ticket, handing it to a squad, the editor --------------------------------------------------------
Shift+Tab
Sleep 1.2s
Enter
Sleep 2.5s
Type "S"
Sleep 1.2s
Down
Sleep 600ms
Down
Sleep 600ms
Up 2
Sleep 1s
Escape
Sleep 600ms
Type "v"
Sleep 1.8s
Enter
Sleep 2.2s

# ---- the Squads tab -------------------------------------------------------------------------------------------------
Type "3"
Sleep 4.5s
Right
Sleep 2.5s
Left
Sleep 500ms

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

# ---- themes: preview a few, then a light one ------------------------------------------------------------------------
Type "s"
Sleep 900ms
Enter
Sleep 1.2s
Down@600ms 4
Sleep 600ms
Type "G"
Sleep 400ms
Up@150ms 8
Sleep 1.5s
Enter
Sleep 900ms
Escape
Sleep 4s

# ---- hidden: quit, stop what the world started, remove it ----------------------------------------------------------
Hide
Type "q"
Sleep 500ms
Type `"$W/live.sh" stop; rm -rf "$W"`
Enter
Sleep 1s
