# The README animation

`platoon.gif` (top of the README: the headline features, about 36 s) and `platoon-tour.mp4` (the whole tour, about
78 s) are recorded in a made-up world: every conversation, project, task, name and path in them is invented by
`make-world.sh`, which builds a home folder, a Claude Code folder, a Codex folder (conversations in Codex 0.163's
record shapes) and platoon's folders in a temp directory. Nothing on your machine is read or shown, and no real
`claude`, `codex` or editor can start: `claude`, `codex` and `code` are stand-ins that only show text or do nothing,
and the running Codex conversations are `flock` holding their lock files, as a Codex window does.

The tour shows Claude Code and Codex conversations in one list, search, opening a Codex conversation, archive, a project
and its notes, the Tasks tab with a task running in Codex, starting a task with a squad, the editor, the Squads tab,
the settings, the help and the themes, all in Dracula. It starts straight on the app: there is no title card.

To make it again, from the repository root:

```
cargo build --release
vhs demo/demo.tape        # records demo/raw.mp4 (not committed)
demo/render.sh            # captions: demo/platoon-tour.mp4 and demo/platoon.gif
```

It needs Linux (GNU `date`, `flock`), [vhs](https://github.com/charmbracelet/vhs) with `ttyd`, `ffmpeg` built with
drawtext, and the DejaVu Sans Mono and Noto Sans fonts. Where Chromium can't use its sandbox (Ubuntu 24.04 and later),
run `VHS_NO_SANDBOX=true vhs demo/demo.tape`.

The terminal is recorded at 120 columns by 34 rows (1264 x 704 pixels), the size the GIF is, so the text reads without
any zooming; the caption band is added under it. The captions and the GIF's pieces are tables at the top of
`render.sh`, in seconds of `raw.mp4`: if you change the tape's timing, move them to match (`ffmpeg -i demo/raw.mp4
-vf "select='gt(scene,0.02)',showinfo" -f null -` lists the moments the screen changes). The GIF keeps a steady 10
frames a second (each frame lasts a whole 10 hundredths of a second, longer where nothing moves), has no camera moves
and no fades, and only lists the themes rather than trying them on: every one of those redraws the whole picture,
which a GIF pays for in smoothness and size. Keep it well under 10 MB (GitHub does not show bigger README images, and
a big one loads in pieces); it is about 3 MB.

`social-preview.png` (1280 x 640, the picture GitHub shows when the repository is linked) is the Sessions list from the
raw recording at 6.6 s, under the name, "Linux and macOS · Claude Code and Codex" and the README's first line, in
Dracula's colors. It is made with ffmpeg from that frame (an `overlay` on a `color` background and three `drawtext`s
in Noto Sans) and uploaded by hand in the repository's settings (Social preview).
