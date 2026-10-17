# Third-party notices

platoon's built-in color themes, dark and light, are the color palettes of well-known editor and terminal themes
(and two of platoon's own). Only color values are used (a handful of hex numbers per theme, compiled into the
binary); no theme code, images or files are copied. Each theme belongs to its original authors and is named here so
it can be found and credited.

## Where the palettes come from

The 16-color palettes, backgrounds and foregrounds are the ones bundled with [Ghostty](https://ghostty.org)
(`share/ghostty/themes`), which in turn takes them from
[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes). platoon reads them once, at
development time, and stores the numbers in `src/theme.rs`, or, for the themes in "More themes, dark and light"
below, in a small theme file each in `src/themes/` (Ghostty's format, naming the Ghostty file it came from and what
platoon changed); it has no runtime dependency on Ghostty.

The Debian package of Ghostty lists the bundled iTerm2-Color-Schemes collection as `public-domain` with no
copyright holder in its `copyright` file. That describes the collection as packaged there; it is not a
statement about the licenses of the individual themes, so each upstream project is credited below.

The picks of which palette color plays which role in platoon's interface (accent, busy, and so on), and the few
places where a role uses a color that is not in the palette to stay readable, are platoon's own.

## Themes

| Theme | Original author / project | License |
| --- | --- | --- |
| Catppuccin Mocha, Catppuccin Macchiato, Catppuccin Frappé | [Catppuccin](https://github.com/catppuccin/catppuccin) (Catppuccin Org) | MIT |
| Tokyo Night, Tokyo Night Storm | [Enkia, tokyo-night-vscode-theme](https://github.com/enkia/tokyo-night-vscode-theme); the terminal palettes are from [folke/tokyonight.nvim](https://github.com/folke/tokyonight.nvim) | MIT (tokyo-night-vscode-theme); Apache-2.0 (tokyonight.nvim) |
| Dracula | [Dracula Theme](https://github.com/dracula/dracula-theme) (Zeno Rocha and contributors) | MIT |
| One Dark | Atom's One Dark syntax theme, [atom/one-dark-syntax](https://github.com/atom/one-dark-syntax) (GitHub) | MIT |
| Nord | [Nord](https://github.com/nordtheme/nord) (Arctic Ice Studio / Sven Greb) | MIT |
| Gruvbox Dark | [gruvbox](https://github.com/morhetz/gruvbox) (Pavel Pertsev) | MIT |
| Gruvbox Material Dark | [gruvbox-material](https://github.com/sainnhe/gruvbox-material) (sainnhe) | MIT |
| Rosé Pine, Rosé Pine Moon | [Rosé Pine](https://github.com/rose-pine/rose-pine-theme) | MIT |
| Kanagawa Wave, Kanagawa Dragon | [kanagawa.nvim](https://github.com/rebelot/kanagawa.nvim) (rebelot) | MIT |
| Night Owl | [night-owl-vscode-theme](https://github.com/sdras/night-owl-vscode-theme) (Sarah Drasner) | see the upstream repository's license |
| Ayu Mirage | [ayu](https://github.com/ayu-theme/ayu-colors) (Ike Ku) | see the upstream repository's license |
| Vesper | [vesper](https://github.com/raunofreiberg/vesper) (Rauno Freiberg) | see the upstream repository's license |
| Solarized Dark | [Solarized](https://github.com/altercation/solarized) (Ethan Schoonover); the palette is iTerm2's own Solarized Dark preset ("iTerm2 Solarized Dark") | MIT |
| GitHub Dark | GitHub's "Dark Default" theme, [primer/github-vscode-theme](https://github.com/primer/github-vscode-theme) (GitHub) | MIT |
| Material Ocean | [material.vim](https://github.com/kaicataldo/material.vim) (Kai Cataldo), after Mattia Astorino's Material Theme | MIT |
| Everforest Dark Hard | [everforest](https://github.com/sainnhe/everforest) (sainnhe), terminal port [everforest.iterm2](https://github.com/icewind/everforest.iterm2) | MIT |
| Nightfox | [nightfox.nvim](https://github.com/EdenEast/nightfox.nvim) (EdenEast) | MIT |
| Sonokai | [sonokai](https://github.com/sainnhe/sonokai) (sainnhe) | MIT |
| Moonfly | [vim-moonfly-colors](https://github.com/bluz71/vim-moonfly-colors) (bluz71) | MIT |
| Melange Dark | [melange-nvim](https://github.com/savq/melange-nvim) (savq) | MIT |
| Flexoki Dark | [Flexoki](https://github.com/kepano/flexoki) (Steph Ango) | MIT |
| Oxocarbon | [oxocarbon.nvim](https://github.com/nyoom-engineering/oxocarbon.nvim) (Nyoom Engineering); two roles use colors of IBM's [Carbon](https://github.com/carbon-design-system/carbon) design system | MIT (oxocarbon.nvim); Apache-2.0 (Carbon) |
| Poimandres | [poimandres-theme](https://github.com/drcmda/poimandres-theme) (drcmda) | MIT |
| Horizon | [horizon-theme-vscode](https://github.com/jolaleye/horizon-theme-vscode) (jolaleye) | MIT |
| Synthwave | contributed straight to [iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes) (brettstil, 2017), with no separate upstream project | MIT (the iTerm2-Color-Schemes collection) |
| Claude | The colors of Claude Code's own terminal interface (Anthropic) | not a third-party theme: only color values, no code or files |
| Terminal colors | uses your terminal's own colors | none |

### More themes, dark and light

These were once platoon's "Theme pack" plugin and are now built in. The palettes, backgrounds and foregrounds are
Ghostty's, as above. The choice of which color plays which part in platoon (accent, working, good news, warnings,
errors) and the hand-picked colors (`platoon-accent`, `platoon-busy`, `platoon-ok`, `platoon-warn`, `platoon-err`, and
a changed `foreground`) are platoon's own; each theme file says what was changed and why.

Dark:

| Theme | Original author / project | License | Changed for platoon |
| --- | --- | --- | --- |
| Harbor Night, Moss Lantern | made for platoon | MIT | (their own colors) |
| Ayu Dark | [ayu](https://github.com/ayu-theme/ayu-colors) (Ike Ku) | MIT | nothing |
| Doom One | [doom-themes](https://github.com/doomemacs/themes) (Henrik Lissner) | MIT | nothing |
| Embark | [Embark](https://github.com/embark-theme/vim) (embark-theme contributors) | MIT | working and errors: its normal purple and red |
| Everblush | [Everblush](https://github.com/Everblush/everblush) (mangeshrex and contributors) | MIT | nothing |
| GitHub Dark Dimmed | [github-vscode-theme](https://github.com/primer/github-vscode-theme) (GitHub) | MIT | nothing |
| Iceberg Dark | [iceberg.vim](https://github.com/cocopon/iceberg.vim) (cocopon) | MIT | accent: its light blue |
| Jellybeans | [jellybeans.vim](https://github.com/nanotech/jellybeans.vim) (NanoTech) | MIT | nothing |
| Srcery | [srcery-vim](https://github.com/srcery-colors/srcery-vim) (Daniel Berg and contributors) | MIT | nothing |
| Tomorrow Night | [tomorrow-theme](https://github.com/chriskempson/tomorrow-theme) (Chris Kempson) | MIT | errors: its red a shade lighter |
| Zenbones Dark | [zenbones.nvim](https://github.com/zenbones-theme/zenbones.nvim) (Michael Chris Lopez) | MIT | nothing |

Light:

| Theme | Original author / project | License | Changed for platoon |
| --- | --- | --- | --- |
| Catppuccin Latte | [Catppuccin](https://github.com/catppuccin/catppuccin) | MIT | accent, working, good news, warnings: its blue, mauve, green and yellow, darker |
| Dayfox | [nightfox.nvim](https://github.com/EdenEast/nightfox.nvim) (EdenEast) | MIT | nothing |
| Flexoki Light | [Flexoki](https://github.com/kepano/flexoki) (Steph Ango) | MIT | good news, warnings: its green and yellow, darker |
| GitHub Light | [github-vscode-theme](https://github.com/primer/github-vscode-theme) (GitHub) | MIT | warnings: GitHub's own attention color |
| Gruvbox Light | [gruvbox](https://github.com/morhetz/gruvbox) (Pavel Pertsev) | MIT | good news, warnings: its faded green and yellow, darker |
| One Half Light | [onehalf](https://github.com/sonph/onehalf) (Son A. Pham) | MIT | accent, good news, warnings, errors: its blue, green, yellow and red, darker |
| Rosé Pine Dawn | [Rosé Pine](https://github.com/rose-pine/rose-pine-theme) | MIT | text a shade darker; accent (iris), warnings (gold) and errors (love) darker; working is its pine; good news a green of platoon's own |
| Solarized Light | [Solarized](https://github.com/altercation/solarized) (Ethan Schoonover) | MIT | text darker (like platoon's Solarized Dark, lifted); accent (violet), working (cyan), green, yellow and red darker |

Left out: commercial themes; themes too close to another built-in (Tokyo Night Moon,
Duskfox, Carbonfox, Snazzy); licenses that don't fit, or none to check (Modus Operandi and Vivendi, GPL-3.0; Bluloco,
LGPL-3.0; the Xcode themes, Apple's; Oceanic Next, whose original repository has no license); and light themes whose
text and nearly every color would have to change to read on their background (Tokyo Night Day, Everforest Light, Ayu
Light, Kanagawa Lotus).

"MIT" is the license the upstream project publishes; where the table says to see the upstream license, check
the repository linked before redistributing anything beyond the color values. Catppuccin Mocha's roles are
platoon's own hand-tuned selection from the Catppuccin palette.

Claude and Claude Code are trademarks of Anthropic. The Claude theme only borrows the look of Claude Code's
interface; it is not an official Anthropic product.

## Libraries

platoon copies selected text to the system clipboard with [arboard](https://github.com/1Password/arboard) (1Password),
which is dual-licensed MIT OR Apache-2.0 (its `Cargo.toml` says `license = "MIT OR Apache-2.0"`, and both license
texts ship in the crate). It is used as a library, built without its image support and with its Wayland support.

What that adds to the build, by the license each crate declares in its metadata: MIT OR Apache-2.0 (arboard, x11rb,
x11rb-protocol, percent-encoding, petgraph, cc, pkg-config, shlex, find-msvc-tools), MIT (wayland-backend,
wayland-client, wayland-protocols, wayland-protocols-wlr, wayland-scanner, wayland-sys, os_pipe, quick-xml,
tree_magic_mini), MIT/Apache-2.0 (wl-clipboard-rs) and Apache-2.0 (gethostname). Versions are pinned in `Cargo.lock`.
(The macOS and Windows crates arboard uses on those systems are not part of a Linux build.)

## The built-in squads

The six built-in squads in `src/squads/` (Feature, Bug fix, Refactor, Explain, Review, Docs) are adapted from the
playbooks and skills of pstack: Lauren Tan's [pstack](https://github.com/cursor/plugins/tree/main/pstack), as ported to
Claude Code by Michael Denyer ([pstack-claude](https://github.com/michael-denyer/pstack-claude)). platoon took the
substance of its `poteto-mode` skill and its feature, bug-fix, refactoring and investigation playbooks, and of its
`how`, `interrogate`, `technical-writing` and `unslop` skills, and rewrote and renamed it as squads. No text was taken
from the skills that pstack-claude marks as Cursor's. pstack is published under this license:

```
MIT License

Copyright (c) 2026 Lauren Tan
Copyright (c) 2026 Michael Denyer

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
