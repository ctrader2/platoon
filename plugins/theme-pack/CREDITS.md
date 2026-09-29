# Theme pack: where the colors come from

Only color values are used: a background, a foreground and sixteen terminal colors per theme, plus a few colors
platoon picked by hand where a palette's own doesn't read well enough on its background. No theme code, images or
other files are copied. Each theme belongs to its original authors.

The palettes, backgrounds and foregrounds are the ones [Ghostty](https://ghostty.org) bundles, which it takes from
[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes) (the Ghostty file each came from is named in
the theme file). The choice of which color plays which part in platoon (accent, working, good news, warnings, errors)
and the hand-picked colors (`platoon-accent`, `platoon-busy`, `platoon-ok`, `platoon-warn`, `platoon-err`, and a
changed `foreground`) are platoon's own, and each theme file says what was changed and why.

## Dark

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

## Light

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

Left out:

- Monokai and its variants: Monokai Pro is commercial.
- Themes too close to a built-in: Tokyo Night Moon (to Tokyo Night Storm), Duskfox (to Rosé Pine Moon), Carbonfox (to
  Oxocarbon), Snazzy (to Dracula).
- Licenses that don't fit a small MIT plugin, or none to check: Modus Operandi and Vivendi (GPL-3.0), Bluloco (LGPL-3.0),
  the Xcode themes (Apple's), Oceanic Next (its original repository has no license).
- Light themes whose text and nearly every color would have to change to read on their background, so little of the
  original would be left: Tokyo Night Day, Everforest Light, Ayu Light, Kanagawa Lotus.
