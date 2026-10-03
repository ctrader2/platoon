#!/usr/bin/env bash
# Turns vhs's raw recording (demo/raw.mp4, from demo/demo.tape) into the README animation:
#   demo/platoon-tour.mp4  the full tour, 1920x1200, 30 fps (H.264)
#   demo/platoon.gif       the headline features (GIF_PIECES), 1000 px wide, for the README
# It adds a title card, a caption band under the terminal naming each feature, and a camera that eases in on
# the part of the screen each feature is about and back out between them. Needs ffmpeg with drawtext (libfreetype)
# and fontconfig's fc-match (the captions use Noto Sans). Run from anywhere: demo/render.sh
set -euo pipefail
cd "$(dirname "$0")"

RAW=raw.mp4
START=1.2        # where the finished video starts in raw.mp4: as `platoon` is started (the title card covers it)
GIF_WIDTH=1000
GIF_FPS=10
GIF_COLORS=64
MP4_CRF=28
# The README GIF is the headline features only (GitHub shows README images up to 10 MB); the MP4 is the whole tour.
# Its pieces, in seconds of raw.mp4: the list, projects and notes, a ticket handed to a squad, the Squads tab, themes.
GIF_PIECES="1.2-11.0 29.3-36.3 40.5-46.3 51.3-55.6 70.4-79.6"
CARD=4.0        # how long the title card stays, in seconds of the finished video (it fades out over its last 0.7 s)
END_AT=77.6     # from here (seconds of raw.mp4) to the end the caption band shows the project's address

# The camera, in seconds of raw.mp4: "time zoom cx cy". At each time the camera is at that zoom (1 = the whole
# screen), centred on (cx, cy) as fractions of the terminal's width and height; between two lines it eases from one
# to the next, so two lines with the same values hold it still.
CAMERA="
0     1     0.5   0.5
5.4   1     0.5   0.5
6.2   1.7   0.76  0.2
8.6   1.7   0.76  0.2
9.3   1     0.5   0.5
11.9  1     0.5   0.5
12.6  1.6   0.5   0.3
17.4  1.6   0.5   0.3
19.2  1.5   0.34  0.16
22.3  1.5   0.34  0.16
23.0  1     0.5   0.5
24.6  1     0.5   0.5
29.0  1     0.5   0.5
29.6  1.6   0.31  0.12
32.2  1.6   0.31  0.12
33.0  1.15  0.5   0.5
35.0  1.15  0.5   0.5
35.7  1.45  0.72  0.55
38.6  1.45  0.72  0.55
39.3  1.6   0.27  0.62
39.9  1.6   0.27  0.62
40.5  1.5   0.36  0.26
42.2  1.5   0.36  0.26
42.8  1.5   0.42  0.32
46.4  1.5   0.42  0.32
47.1  1.7   0.48  0.46
48.9  1.7   0.48  0.46
49.5  1.7   0.76  0.9
51.2  1.7   0.76  0.9
52.0  1.5   0.62  0.52
55.8  1.5   0.62  0.52
56.5  1.4   0.4   0.35
58.4  1.4   0.4   0.35
59.2  1.25  0.5   0.45
65.3  1.25  0.5   0.45
65.9  1.25  0.5   0.47
69.0  1.25  0.5   0.47
70.4  1.3   0.76  0.5
76.0  1.3   0.76  0.5
76.8  1     0.5   0.5
"

# Captions, in seconds of raw.mp4: "from to text".
CAPTIONS="
5.2  11.5  Every session at a glance: working or done, model, context, plan usage
11.5 18.6  Search by name, or by anything you ever typed to Claude
18.6 23.5  Open a session inside platoon; ctrl+space brings you back
23.5 29.3  Archive what you're done with
29.3 32.6  Projects group sessions from any folder
32.6 38.7  ...and share notes that every session in the project gets
38.7 46.5  Tickets: write the work down, then hand it to a squad of agents
46.5 51.3  Open a session's or a ticket's folders in your own editor
51.3 55.9  Squads: a lead agent and its team, each step with its own model
55.9 58.8  Agents are shared between squads
58.8 65.6  Every setting in one place, saved as you change it
65.6 69.2  Press ? for every key on the screen you're on
69.2 77.6  52 themes, dark and light
"

font() { fc-match -f '%{file}' "$1"; }
BOLD=$(font 'Noto Sans:bold')
REGULAR=$(font 'Noto Sans')

# One eased, piecewise expression of the time (`it`, zoompan's input time) for column $1 of the camera table.
# zoompan works on the frame scaled up to 4K first, so its whole-pixel steps are half a pixel of the result.
camera_expr() {
    printf '%s\n' "$CAMERA" | awk -v col="$1" -v start="$START" '
        BEGIN { n = 0 }
        NF == 4 { t[n] = $1 - start; v[n] = $col; n++ }
        END {
            e = v[n - 1]
            for (i = n - 2; i >= 0; i--) {
                d = t[i + 1] - t[i]
                seg = sprintf("%s+(%s)*(1-cos(PI*(it-%s)/%s))/2", v[i], v[i + 1] - v[i], t[i], d)
                if (v[i] == v[i + 1]) seg = v[i]
                e = sprintf("if(lt(it,%s),%s,%s)", t[i + 1], seg, e)
            }
            print e
        }'
}
Z=$(camera_expr 2)
CX=$(camera_expr 3)
CY=$(camera_expr 4)

GIF_SELECT=$(printf '%s\n' $GIF_PIECES | awk -F- -v start="$START" '
    { printf "%sbetween(t,%s,%s)", (NR > 1 ? "+" : ""), $1 - start, $2 - start }')

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# The captions, each from its own text file (no escaping needed), fading in and out in the band under the terminal.
captions=""
i=0
while read -r from to text; do
    [ -z "$from" ] && continue
    from=$(awk "BEGIN { print $from - $START }")
    to=$(awk "BEGIN { print $to - $START }")
    printf '%s' "$text" > "$WORK/caption$i.txt"
    fade="if(lt(t,$from+0.35),(t-$from)/0.35,if(gt(t,$to-0.35),($to-t)/0.35,1))"
    captions+=",drawtext=fontfile='$REGULAR':textfile='$WORK/caption$i.txt':fontsize=40:fontcolor=0xcdd6f4"
    captions+=":x=(w-text_w)/2:y=1080+(120-text_h)/2-4:enable='between(t,$from,$to)':alpha='$fade'"
    i=$((i + 1))
done <<< "$CAPTIONS"

printf '%s' 'platoon' > "$WORK/title.txt"
printf '%s' 'Every AI coding session, in one terminal app' > "$WORK/tagline.txt"
printf '%s' 'github.com/ctrader2/platoon' > "$WORK/url.txt"

cat > "$WORK/graph" <<EOF
[0:v]trim=start=$START,setpts=PTS-STARTPTS,fps=30,
scale=3840:2160:flags=lanczos,
zoompan=z='$Z':x='clip(($CX)*iw-iw/zoom/2,0,iw-iw/zoom)':y='clip(($CY)*ih-ih/zoom/2,0,ih-ih/zoom)':d=1:s=1920x1080:fps=30,
pad=w=1920:h=1200:x=0:y=0:color=0x11111b,format=yuv420p$captions,
drawtext=fontfile='$REGULAR':textfile='$WORK/url.txt':fontsize=40:fontcolor=0xcba6f7:x=(w-text_w)/2:y=1080+(120-text_h)/2-4:enable='gte(t,$(awk "BEGIN { print $END_AT - $START }"))'[base];
color=c=0x11111b@0.9:s=1920x1200:r=30:d=$CARD,format=yuva420p,
drawtext=fontfile='$BOLD':textfile='$WORK/title.txt':fontsize=150:fontcolor=0xcba6f7:x=(w-text_w)/2:y=400,
drawtext=fontfile='$REGULAR':textfile='$WORK/tagline.txt':fontsize=52:fontcolor=0xcdd6f4:x=(w-text_w)/2:y=620,
fade=t=out:st=$(awk "BEGIN { print $CARD - 0.7 }"):d=0.7:alpha=1[card];
[base][card]overlay=eof_action=pass:format=auto,format=yuv420p,split=2[mp4][g];
[g]select='$GIF_SELECT',setpts=N/(30*TB),fps=$GIF_FPS,mpdecimate=hi=2048:lo=640:frac=0.5,
scale=$GIF_WIDTH:-1:flags=lanczos,split[g1][g2];
[g1]palettegen=max_colors=$GIF_COLORS:stats_mode=full[pal];
[g2][pal]paletteuse=dither=none:diff_mode=rectangle[gif]
EOF

ffmpeg -v error -y -i "$RAW" -/filter_complex "$WORK/graph" \
    -map '[mp4]' -c:v libx264 -preset slow -tune animation -crf "$MP4_CRF" -pix_fmt yuv420p -movflags +faststart platoon-tour.mp4 \
    -map '[gif]' -fps_mode vfr -loop 0 platoon.gif
ls -l platoon-tour.mp4 platoon.gif
