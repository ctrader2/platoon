#!/usr/bin/env bash
# Turns vhs's raw recording (demo/raw.mp4, from demo/demo.tape) into the README animation:
#   demo/platoon-tour.mp4  the full tour, 1264x760, 20 fps (H.264)
#   demo/platoon.gif       the headline features (GIF_PIECES), 1264x760, for the README
# It adds a caption band under the terminal naming each feature. There is no title card and no camera: the terminal is
# recorded at a size that reads as it is, and a GIF whose every frame moves (a zoom, a pan) plays jerkily and weighs
# many times more. Needs ffmpeg with drawtext (libfreetype) and fontconfig's fc-match (the captions use Noto Sans).
# Run from anywhere: demo/render.sh
set -euo pipefail
cd "$(dirname "$0")"

RAW=raw.mp4
FPS=20           # the recording's own rate (demo.tape's Framerate), kept in the MP4
GIF_FPS=10       # the GIF's: every frame a whole 10 hundredths of a second, half the frames of a spinner to store
BAND=56          # the caption band under the terminal, in pixels
GIF_COLORS=64
MP4_CRF=20
# The README GIF is the headline features only (GitHub shows README images up to 10 MB); the MP4 is the whole tour.
# Its pieces, in seconds of raw.mp4: the list (Claude Code and Codex), a project and its notes, the Tasks tab (a task
# running in Codex, starting one with a squad), the Squads tab, and the list of themes. The themes are only listed
# there, not tried on: a preview recolours every pixel of every frame, which costs a GIF most of its size and its
# palette (the MP4 shows them).
GIF_PIECES="0.0-8.7 29.0-35.6 36.4-48.5 53.2-58.3 69.2-72.2"
END_AT=75.9      # from here (seconds of raw.mp4) to the end the caption band shows the project's address

# Captions, in seconds of raw.mp4: "from to text".
CAPTIONS="
0.0  8.7  Claude Code and Codex sessions in one list, with both plans' usage
8.7  14.6 Search by name, or by anything you ever typed
14.6 21.0 Open any session inside platoon, Codex's too; ctrl+space brings you back
21.0 26.7 Archive what you're done with
26.7 32.5 Projects group sessions from any folder
32.5 36.3 ...and share notes that every session in the project gets
36.3 42.0 Tasks: this one runs in Codex, that one waits for your answer
42.0 48.6 Start a task with a squad: its model decides Claude Code or Codex
48.6 53.1 Open a session's or a task's folders in your own editor
53.1 58.3 Squads: a Markdown file of steps, each squad with its own model
58.3 65.3 Every setting in one place, saved as you change it
65.3 69.0 Press ? for every key on the screen you're on
69.0 75.9 52 themes, or your own
"

font() { fc-match -f '%{file}' "$1"; }
REGULAR=$(font 'Noto Sans')

W=$(ffprobe -v error -select_streams v:0 -show_entries stream=width -of csv=p=0 "$RAW")
H=$(ffprobe -v error -select_streams v:0 -show_entries stream=height -of csv=p=0 "$RAW")

GIF_SELECT=$(printf '%s\n' "$GIF_PIECES" | tr ' ' '\n' | awk -F- '{ printf "%sbetween(t,%s,%s)", (NR > 1 ? "+" : ""), $1, $2 }')

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# The captions, each from its own text file (no escaping needed), in the band under the terminal. They change at once
# (no fade): a fade redraws the band in every frame of it, which a GIF pays for in size and smoothness.
captions=""
i=0
while read -r from to text; do
    [ -z "$from" ] && continue
    printf '%s' "$text" > "$WORK/caption$i.txt"
    captions+=",drawtext=fontfile='$REGULAR':textfile='$WORK/caption$i.txt':fontsize=26:fontcolor=0xf8f8f2"
    captions+=":x=(w-text_w)/2:y=$H+($BAND-text_h)/2-3:enable='between(t,$from,$to)'"
    i=$((i + 1))
done <<< "$CAPTIONS"

printf '%s' 'github.com/ctrader2/platoon' > "$WORK/url.txt"

cat > "$WORK/graph" <<EOF
[0:v]fps=$FPS,pad=w=$W:h=$H+$BAND:x=0:y=0:color=0x1e1f29$captions,
drawtext=fontfile='$REGULAR':textfile='$WORK/url.txt':fontsize=26:fontcolor=0xbd93f9:x=(w-text_w)/2:y=$H+($BAND-text_h)/2-3:enable='gte(t,$END_AT)',
split=2[m][g];
[m]format=yuv420p[mp4];
[g]select='$GIF_SELECT',setpts=N/($FPS*TB),fps=$GIF_FPS,mpdecimate=hi=64:lo=32:frac=0.1,split[g1][g2];
[g1]palettegen=max_colors=$GIF_COLORS:stats_mode=full[pal];
[g2][pal]paletteuse=dither=none:diff_mode=rectangle[gif]
EOF

ffmpeg -v error -y -i "$RAW" -/filter_complex "$WORK/graph" \
    -map '[mp4]' -c:v libx264 -preset slow -tune animation -crf "$MP4_CRF" -pix_fmt yuv420p -r "$FPS" -movflags +faststart platoon-tour.mp4 \
    -map '[gif]' -fps_mode vfr -loop 0 platoon.gif
ls -l platoon-tour.mp4 platoon.gif
