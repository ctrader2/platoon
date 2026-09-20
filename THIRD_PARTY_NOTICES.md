# Third-party notices

platoon's built-in color themes are the color palettes of well-known editor and terminal themes. Only color
values are used (a handful of hex numbers per theme, compiled into the binary); no theme code, images or files
are copied. Each theme belongs to its original authors and is named here so it can be found and credited.

## Where the palettes come from

The 16-color palettes, backgrounds and foregrounds are the ones bundled with [Ghostty](https://ghostty.org)
(`share/ghostty/themes`), which in turn takes them from
[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes). platoon reads them once, at
development time, and stores the numbers in `src/theme.rs`; it has no runtime dependency on Ghostty.

The Debian package of Ghostty lists the bundled iTerm2-Color-Schemes collection as `public-domain` with no
copyright holder in its `copyright` file. That describes the collection as packaged there; it is not a
statement about the licenses of the individual themes, so each upstream project is credited below.

The picks of which palette color plays which role in platoon's interface (accent, busy, and so on), and the few
places where a role uses a color that is not in the palette to stay readable, are platoon's own.

## Themes

| Theme | Original author / project | License |
| --- | --- | --- |
| Catppuccin Mocha, Catppuccin Macchiato | [Catppuccin](https://github.com/catppuccin/catppuccin) (Catppuccin Org) | MIT |
| Tokyo Night | [Enkia, tokyo-night-vscode-theme](https://github.com/enkia/tokyo-night-vscode-theme) | see the upstream repository's license |
| Dracula | [Dracula Theme](https://github.com/dracula/dracula-theme) (Zeno Rocha and contributors) | MIT |
| One Dark | Atom's One Dark syntax theme, [atom/one-dark-syntax](https://github.com/atom/one-dark-syntax) (GitHub) | MIT |
| Nord | [Nord](https://github.com/nordtheme/nord) (Arctic Ice Studio / Sven Greb) | MIT |
| Gruvbox Dark | [gruvbox](https://github.com/morhetz/gruvbox) (Pavel Pertsev) | MIT |
| Rosé Pine | [Rosé Pine](https://github.com/rose-pine/rose-pine-theme) | MIT |
| Kanagawa Wave | [kanagawa.nvim](https://github.com/rebelot/kanagawa.nvim) (rebelot) | see the upstream repository's license |
| Night Owl | [night-owl-vscode-theme](https://github.com/sdras/night-owl-vscode-theme) (Sarah Drasner) | see the upstream repository's license |
| Ayu Mirage | [ayu](https://github.com/ayu-theme/ayu-colors) (Ike Ku) | see the upstream repository's license |
| Vesper | [vesper](https://github.com/raunofreiberg/vesper) (Rauno Freiberg) | see the upstream repository's license |
| Claude | The colors of Claude Code's own terminal interface (Anthropic) | not a third-party theme: only color values, no code or files |
| Terminal colors | uses your terminal's own colors | none |

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
