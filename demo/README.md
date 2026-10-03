# The README animation

`platoon.gif` (top of the README: the headline features, about 36 s) and `platoon-tour.mp4` (the whole tour, about
80 s) are recorded in a made-up world: every conversation, project, ticket, name and path in them is invented by
`make-world.sh`, which builds a home folder, a Claude Code folder and platoon's folders in a temp directory. Nothing on
your machine is read or shown, and no real `claude` or editor can start: `claude` and `code` are stand-ins that only
show text or do nothing.

To make it again, from the repository root:

```
cargo build --release
vhs demo/demo.tape        # records demo/raw.mp4 (not committed)
demo/render.sh            # title card, captions and zooms: demo/platoon-tour.mp4 and demo/platoon.gif
```

It needs Linux (GNU `date`), [vhs](https://github.com/charmbracelet/vhs) with `ttyd`, `ffmpeg` built with drawtext,
and the DejaVu Sans Mono and Noto Sans fonts. Where Chromium can't use its sandbox (Ubuntu 24.04 and later), run
`VHS_NO_SANDBOX=true vhs demo/demo.tape`. The camera, the captions and the GIF's pieces are tables at the top of
`render.sh`, in seconds of `raw.mp4`: if you change the tape's timing, move them to match. Keep the GIF under 10 MB
(GitHub does not show bigger README images).
