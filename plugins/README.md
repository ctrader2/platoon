# platoon's plugin catalog

This folder is the catalog platoon offers on its **Plugins** tab (`4`) and with `platoon plugin list`. Each plugin is a
folder here, named after its id, and `index.json` lists them in the order platoon shows them.

A plugin holds **files, never code**: a `plugin.json` that says what it is and what it adds, and those files. In this
version a plugin can add **color themes**, or **platoon's code editor** (one plugin, `editor`; see "The code editor
kind" below). (Later: skills, and squads and agents.)

## Making a theme plugin

1. Make a folder named after your plugin's id: lowercase letters, digits and dashes, starting with a letter
   (`sea-themes`).
2. Put your themes in it, one file each, in the same format as your own theme files and Ghostty's (see "Your own themes"
   in platoon's README). The file's name is the theme's name, so `themes/Harbor Night` is the theme "Harbor Night":

   ```
   background = #0f1a24
   foreground = #d6e2ea
   palette = 1=#f07a7a
   palette = 2=#7fcf9a
   palette = 3=#e8c07a
   palette = 4=#6fb3e8
   palette = 5=#c39be8
   palette = 9=#ff8f8f
   palette = 10=#93e0ad
   palette = 11=#f5d08c
   palette = 12=#86c4f2
   palette = 13=#d2adf2
   ```

   platoon picks its colors from these slots: the accent from 12 (or 4), "working" from 13 (or 5), "done" and other good
   news from 10 (or 2), warnings from 11 (or 3), errors from 9 (or 1). Give all sixteen if you can; sessions shown inside
   platoon are recolored with them.
3. Write `plugin.json`:

   ```json
   {
     "id": "sea-themes",
     "name": "Sea themes",
     "description": "Two calm dark themes named after the sea at night.",
     "version": "1.0.0",
     "author": "Your Name",
     "license": "MIT",
     "platoon": "0.1.0",
     "themes": ["themes/Harbor Night", "themes/Deep Water"]
   }
   ```

   - `id`: the folder's name. `name`: how platoon shows it (at most 40 characters). `description`: one or two sentences.
   - `version`: your plugin's version, like `1.0.0`. Raise it whenever you change it.
   - `author`, and `license` (an SPDX name like `MIT`; optional, but please give one).
   - `platoon`: the oldest platoon version it works with: the `version` in platoon's `Cargo.toml` when you made it. An
     older platoon says the plugin needs a newer one instead of installing it.
   - `themes`: the theme files, as paths inside the folder.

   Keys platoon doesn't know are left alone, so a plugin made for a newer platoon doesn't break an older one.

## Checking it

```
platoon plugin check ./sea-themes
```

says what is wrong, in plain words, or "Looks good". It checks that:

- `plugin.json` is there, readable, and has every field above;
- every file it names is in the folder (paths inside it only: no `/` at the start, no `..`);
- the folder holds plain files only (no links), at most 200 of them and 1 MiB in all; a theme file is at most 64 KiB;
- every theme is a **dark** theme that reads well **as it is**: the text at 7:1 against the background, the five colors
  platoon uses for meaning (accent, working, good news, warnings, errors) at 4.5:1, and those five clearly different from
  each other. (Your own themes folder lifts colors until they read; a plugin's themes have to read without help, like
  the built-in ones.)

Try it for real with `platoon plugin add ./sea-themes`: its themes appear in the settings (`s`, then Theme), with your
plugin's name beside them. `platoon plugin remove sea-themes` takes it off again.

## The code editor kind

The `editor` plugin here is platoon's code editor (Neovim, opened with `v`). platoon has the code that installs and runs
it; the plugin has the data: what to download and the editor's settings files.

```json
"editor": {
  "downloads": "downloads.json",
  "config": ["config/init.lua", "config/lua/platoon/tree.lua", "config/shell.sh"]
}
```

- `downloads`: the download table. `tools` (Neovim and the programs it uses: an `id`, the `version`, the oldest of the
  user's own copies to use instead as `min_yours`, the `names` it goes by on PATH, the program inside the download as
  `exe`, and one download per computer: `linux-x64`, `linux-arm64`, `macos-arm64`, `macos-x64`), `plugins` (Neovim
  plugins as commit tarballs, installed as native packages), `parsers` (syntax highlighting, built with the C compiler;
  revisions as the pinned nvim-treesitter's `parsers.lua` names them) and the `font`. **Every download has an https
  address, its sha256 and its exact size in bytes**: platoon refuses a file that differs in either, and a table missing
  one is refused whole, so what lands on someone's computer is exactly what was reviewed here. `note` and `about` are
  free text.
- `config`: the editor's settings files, all under `config/`. Each lands at the same path under the editor's own config
  folder every time it opens (`config/init.lua` is what Neovim starts with and must be there). They are text.

`platoon plugin check plugins/editor` also checks: the table reads, only https, every hash and size there, no single
download over 300 MB and no more than 600 MB in all on any computer, every name platoon makes a folder or a file of is
a plain name and every path stays inside its download, and the settings files are text inside `config/`. Only one code
editor plugin can be installed at a time.

These files run inside Neovim on the user's computer, and the table decides what programs are downloaded: a change to
this plugin is reviewed like a change to platoon's code. To move a download to a newer version, change its `rev` (or
version), `url`, `sha256` and `size` together, from the release's published digest or two downloads that match, and
raise the plugin's `version`; installed editors then fetch only what changed. platoon keeps a copy of this plugin in its
own build (for taking over an editor installed before the editor was a plugin), and a test checks the copy is whole.

## Sending it

1. Fork platoon's repository and put your folder in `plugins/`.
2. Add it to `plugins/index.json`, with the same `id` and `version` as its `plugin.json`:

   ```json
   { "id": "sea-themes", "name": "Sea themes", "version": "1.0.0" }
   ```

3. Run `platoon plugin check plugins/sea-themes` and `cargo test` (a test checks every plugin here and the index).
4. Open a pull request that says what the plugin adds.

## What is reviewed

- It passes `platoon plugin check`, and the index matches.
- It is files only and adds only what it says. For the code editor: every download pinned, from its project's own
  releases or commits.
- **Where the colors come from.** Your own, or a theme whose license allows it: say which, and keep its license and credit
  in the plugin (a `LICENSE` or a line in the theme file). Commercial themes can't be accepted.
- The name and description are plain and honest, and nothing in it is personal (no paths, emails or machine names beyond
  the author's name).
- A new version of a plugin raises `version`.

The `example-themes` plugin here is a small working example: two themes, Harbor Night and Moss Lantern, with colors made
for platoon (MIT).
