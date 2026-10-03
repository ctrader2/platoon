# Themes

## The built-in themes

platoon ships with 52 color themes.

- **43 dark**: Catppuccin Mocha (the default), Catppuccin Macchiato, Catppuccin Frappé, Tokyo Night, Tokyo Night Storm,
  Dracula, One Dark, Nord, Gruvbox Dark, Gruvbox Material Dark, Rosé Pine, Rosé Pine Moon, Kanagawa Wave, Kanagawa Dragon,
  Night Owl, Ayu Mirage, Vesper, Solarized Dark, GitHub Dark, Material Ocean, Everforest Dark Hard, Nightfox, Sonokai,
  Moonfly, Melange Dark, Flexoki Dark, Oxocarbon, Poimandres, Horizon, Synthwave, Claude (Claude Code's own colors),
  Harbor Night, Moss Lantern, Ayu Dark, Doom One, Embark, Everblush, GitHub Dark Dimmed, Iceberg Dark, Jellybeans, Srcery,
  Tomorrow Night and Zenbones Dark.
- **8 light**: Catppuccin Latte, Dayfox, Flexoki Light, GitHub Light, Gruvbox Light, One Half Light, Rosé Pine Dawn and
  Solarized Light.
- **Terminal colors**, which uses your terminal's own (see [below](#terminal-colors)).

`platoon themes` lists them with a swatch of each and their ids. Where the palettes come from, and who made them, is in
[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).

## Picking one

Press `s` (the settings open on the Theme row) and `enter`. A list of themes opens on the right, next to your real screen,
each with a strip of its main colors and a check on the one you use now. Move with `↑` `↓` (`Home` `End` or `g` `G` jump to
the first and last): the whole app is redrawn in the highlighted theme at once, and a small sample in the panel shows every
color it uses even if you have no conversations yet. `enter` keeps the highlighted theme (it is saved), `esc` puts your old
theme back.

With the mouse: hover highlights an entry, a click previews it, a click on the previewed one (or on `enter use`) keeps it,
the wheel moves the highlight, and a click outside the panel puts the old theme back. On the Theme row of the settings,
`←` `→` skip the list: they switch to the previous or next theme and save it straight away.

You can also set it by hand with `theme` in [`settings.json`](settings-and-keys.md#the-settings-file):

```json
{"theme": "tokyo-night"}
```

A running platoon picks the change up within a couple of seconds. A theme id platoon doesn't know shows the default and is
left in the file as it is. The theme also recolors the Claude session you view inside platoon (its 16 terminal colors
become the theme's); `platoon open` in a regular terminal is unaffected.

## Your own themes

platoon reads extra themes from the `themes` folder next to `settings.json`: `~/.config/platoon/themes/`
(`$PLATOON_CONFIG_DIR/themes`, or `$XDG_CONFIG_HOME/platoon/themes`). It uses [Ghostty](https://ghostty.org)'s theme file
format, one theme per file, and the file's name is the theme's name (`~/.config/platoon/themes/Deep Sea` is "Deep Sea"):

```
background = #0a1622
foreground = #dde6ee
palette = 12=#4fb0ff
```

That is all it needs: the background, the foreground, and any of the 16 `palette = N=#RRGGBB` colors (others are worked
out; the other keys Ghostty themes carry are ignored). Ghostty's own themes are in this format, so borrowing one is a copy,
for example:

```
mkdir -p ~/.config/platoon/themes && cp /usr/share/ghostty/themes/Nord ~/.config/platoon/themes/
```

Your themes appear in the list after the built-ins, marked `yours`, with the id `user:<file name>` (`user:deep-sea`) that
goes in `settings.json`. The id keeps only plain letters and digits: an accented letter becomes its plain one
(`Crème Brûlée` is `user:creme-brulee`). Things to know:

- On a dark theme, contrast is adjusted for you. Text, the muted grays, the accent and the status colors are lightened as
  far as it takes to read on your background (and kept clearly different from each other), so a sloppy file still gives a
  usable theme. What the file says is used as it is wherever it already reads well.
- A light theme (a light background) is used as it is, so it has to read well already, by the same rules as the built-in
  light themes: the text at 7:1 against the background, the accent and the four status colors at 4.5:1 and clearly
  different from each other. platoon takes the accent from palette 12 (or 4), "working" from 13 (or 5), good news from 10
  (or 2), warnings from 11 (or 3) and errors from 9 (or 1); on a light background the normal color (4, 5, 2, 3, 1) when it
  reads, else the bright one. Where a palette color doesn't reach the floor, you can pick platoon's color yourself, without
  touching the palette sessions use, with `platoon-accent = #1d62eb` and likewise `platoon-busy`, `platoon-ok`,
  `platoon-warn` and `platoon-err` lines; the built-in light themes' files in platoon's source
  ([`src/themes/`](../src/themes/)) are good examples.
- A file that can't be used is skipped: a light theme that doesn't read well enough (the reason says what falls short), or
  anything that isn't a readable theme file (too big, not text, no background or foreground). The picker says how many were
  skipped and why, and so does `platoon themes`.
- A file named like a built-in (a copy of Nord, say) is its own theme, shown as `Nord (yours)`; two files whose names
  differ only in capitals count as one (the first, in name order, is used). `platoon themes` prints every id.
- Changes to the folder are noticed within a couple of seconds; nothing needs restarting. If you pick a theme of your own
  and later delete its file, platoon shows the default theme and keeps the id in `settings.json`, so putting the file back
  brings your theme back.
- A folder with more than 300 files is read only as far as its first 300 (in name order); subfolders and names starting
  with a dot are ignored.

## Light themes

On a light theme everything platoon draws is dark on light. The light themes' ids are made from their names
(`catppuccin-latte`, `rose-pine-dawn`). The Claude session you view inside platoon gets the theme's background and colors
too, but Claude Code picks some colors of its own: with a light theme in platoon, choose a light theme in Claude Code as
well (its `/theme` command).

## Terminal colors

`terminal` (Terminal colors) draws with your terminal's own default colors and 16-color palette, so platoon follows
whatever theme your terminal has. It relies on your terminal's palette having a black that differs from the background and
a bright black that reads as grey; if your terminal theme's black is the same as its background, the highlighted row is
shown only by the marker beside it. It assumes a dark terminal theme: on a light one the highlighted row is a black band
that is hard to read, so pick one of the light themes instead.

## Terminals without 24-bit color

Themes use 24-bit color. Terminals that don't support it (macOS Terminal.app, some tmux setups) get the nearest of the 256
xterm colors instead, worked out for each cell as it is drawn. platoon decides from `COLORTERM` (`truecolor` or `24bit`)
and a list of terminals known to support it; set `PLATOON_COLORS=truecolor` or `PLATOON_COLORS=256` to choose yourself.
