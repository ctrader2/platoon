//! The code editor itself (`v`): which folders it opens, the files it is started with, and handing platoon's terminal
//! to it and back. The app decides when (`app/editor_glue.rs`); what is installed where is `editor_install.rs`.
//!
//! How it is started: `<editor>/bin/nvim -u <editor>/config/init.lua`, in the first folder, with
//! - `init.lua` and `lua/platoon/tree.lua` (compiled into platoon from `src/editor/`, rewritten at every opening),
//! - a session file (`config/session-<platoon's pid>.json`, named by `PLATOON_EDITOR_SESSION`): the folders, the commits
//!   a ticket's changes are counted from, platoon's theme turned into the editor's colors, whether there are icons,
//!   where a copy goes (as platoon copies: `PLATOON_CLIPBOARD`, the helper `platoon clipboard-hold`),
//! - its XDG folders inside `<editor>/nvim/` (nvim writes its state even with `--clean`, and nvim-treesitter would put
//!   parsers in `~/.local/share/nvim`), `<editor>/bin` first on PATH, and the user's own values of those variables in
//!   `PLATOON_KEEP_<name>`, which `config/shell` (the editor's 'shell') puts back for the terminal and `:!`.
//!
//! The handoff: platoon leaves its screen (raw mode off, the alternate screen left), runs nvim in the foreground on
//! the same terminal and waits; nothing of platoon reads the keyboard or draws meanwhile (its loop is waiting here, and
//! its threads never touch the terminal). Ctrl+C typed at a `:!` command reaches nvim's program, not platoon. Then
//! platoon takes the screen back and draws it again in full, where the user was.
//!
//! Knobs: `PLATOON_NVIM_BIN` runs that program instead of platoon's nvim (tests: a stand-in that records what it was
//! given); with `PLATOON_FORBID_REAL_EDITOR` set and no stand-in, the real one is refused.

use crate::colordepth::{rgb_to_256, Depth};
use crate::editor_install::Dirs;
use crate::theme::{self, Theme};
use crate::tickets::Ticket;
use ratatui::style::Color;
use serde_json::{json, Map, Value};
use std::{
    fs, io,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

pub const INIT_LUA: &str = include_str!("editor/init.lua");
pub const TREE_LUA: &str = include_str!("editor/tree.lua");
pub const SHELL_SH: &str = include_str!("editor/shell.sh");

/// The variables the editor's own world changes, whose user values the terminal gets back (`shell.sh` has the
/// same list).
pub const KEPT: &[&str] = &[
    "PATH",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_DIRS",
    "NVIM_APPNAME",
    "NVIM_LOG_FILE",
    "VIMINIT",
    "VIMRUNTIME",
    "VIM",
    "MYVIMRC",
];

// ---- the folders --------------------------------------------------------------------------------------------------

/// One folder the editor opens: a top-level entry of its file tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    /// Absolute.
    pub path: String,
    /// What the tree calls it.
    pub name: String,
    /// The top of its git repository, when it is in one.
    pub repo: Option<String>,
    /// What the changed files and the diffs call that repository.
    pub repo_name: Option<String>,
    /// The commit its changes are counted from (a ticket's worktree: the commit it was made from). None: what is
    /// not yet committed.
    pub base: Option<String>,
    /// "folder", "worktree" or "work".
    pub kind: &'static str,
}

/// What `v` opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Shown in the editor's status line: "#3 Fix the parser", a session's name, a folder.
    pub title: String,
    pub roots: Vec<Root>,
}

impl Request {
    pub fn dirs(&self) -> Vec<String> {
        self.roots.iter().map(|r| r.path.clone()).collect()
    }
}

fn base_name(path: &str) -> String {
    let p = Path::new(path);
    match p.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => path.to_string(),
    }
}

/// The top of the git repository `dir` is in.
pub fn git_top(git: &str, dir: &Path) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    crate::squads::git(git, dir, &["rev-parse", "--show-toplevel"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// A folder as it is: its repository (if any) with what is not yet committed.
fn plain_root(path: &str, name: String, kind: &'static str, git: &str) -> Root {
    let repo = git_top(git, Path::new(path));
    let repo_name = repo.as_deref().map(base_name);
    Root { path: path.to_string(), name, repo, repo_name, base: None, kind }
}

/// Two entries may not share a name in the tree: a second one gets its parent folder's name too.
fn unique_names(roots: &mut [Root]) {
    for i in 1..roots.len() {
        if roots[..i].iter().any(|r| r.name == roots[i].name) {
            let parent = Path::new(&roots[i].path).parent().map(|p| base_name(&p.to_string_lossy())).unwrap_or_default();
            roots[i].name = format!("{} ({parent})", roots[i].name);
        }
    }
}

fn push_unique(out: &mut Vec<Root>, r: Root) {
    if !out.iter().any(|o| o.path == r.path) {
        out.push(r);
    }
}

/// A conversation's folder and the folders added to it (`/add-dir`, home-abbreviated as the list shows them). Folders
/// that aren't there any more are left out.
pub fn session_roots(cwd: &str, extra: &[String], home: &str, git: &str) -> Vec<Root> {
    let mut out = Vec::new();
    let all = std::iter::once(cwd.to_string()).chain(extra.iter().filter_map(|d| crate::dirs::expand(d, home)));
    for path in all {
        if Path::new(&path).is_dir() {
            push_unique(&mut out, plain_root(&path, base_name(&path), "folder", git));
        }
    }
    unique_names(&mut out);
    out
}

/// A folder picked by hand.
pub fn folder_roots(dir: &str, git: &str) -> Vec<Root> {
    if !Path::new(dir).is_dir() {
        return Vec::new();
    }
    vec![plain_root(dir, base_name(dir), "folder", git)]
}

/// A ticket's worktrees (with the commit each was made from), its folders that have no worktree (not in git, or not
/// handed to a squad yet) as they are, and its work folder when there is something in it. Worktrees that are gone are
/// left out: the app brings them back first.
pub fn ticket_roots(t: &Ticket, work: &Path, git: &str) -> Vec<Root> {
    let mut out = Vec::new();
    let tag = format!("#{}", t.number);
    for f in &t.folders {
        match t.worktrees.iter().find(|w| w.folder == *f) {
            Some(w) if !w.is_gone() => {
                let repo = git_top(git, Path::new(&w.path));
                let repo_name = git_top(git, Path::new(&w.folder)).map(|r| base_name(&r)).or_else(|| Some(base_name(&w.folder)));
                let base = crate::worktrees::diff_base(w, git);
                push_unique(&mut out, Root { path: w.path.clone(), name: format!("{} · {tag}", base_name(&w.folder)), repo, repo_name, base, kind: "worktree" });
            }
            Some(_) => {}
            None if Path::new(f).is_dir() => push_unique(&mut out, plain_root(f, base_name(f), "folder", git)),
            None => {}
        }
    }
    // A worktree whose folder was taken off the ticket later is still its work.
    for w in t.worktrees.iter().filter(|w| !t.folders.contains(&w.folder) && !w.is_gone()) {
        let repo = git_top(git, Path::new(&w.path));
        let base = crate::worktrees::diff_base(w, git);
        push_unique(&mut out, Root { path: w.path.clone(), name: format!("{} · {tag}", base_name(&w.folder)), repo, repo_name: Some(base_name(&w.folder)), base, kind: "worktree" });
    }
    let has_files = fs::read_dir(work).is_ok_and(|mut d| d.next().is_some());
    if has_files {
        let path = work.to_string_lossy().into_owned();
        push_unique(&mut out, plain_root(&path, format!("{tag} work folder"), "work", git));
    }
    unique_names(&mut out);
    out
}

// ---- colors ---------------------------------------------------------------------------------------------------------

/// How the editor gets platoon's theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    /// 24-bit colors (`termguicolors`).
    TrueColor,
    /// The terminal has 256 colors: nearest xterm-256 indices only, never a 24-bit color.
    Indexed256,
    /// "Terminal colors" (the theme has no fixed palette), or NO_COLOR: the terminal's own 16 colors, nothing forced.
    Terminal,
}

pub fn color_mode(t: &Theme, depth: Depth, no_color: bool) -> ColorMode {
    let fixed = t.ansi.is_some() && t.roles().iter().all(|(_, c)| matches!(c, Color::Rgb(..)));
    if no_color || !fixed {
        ColorMode::Terminal
    } else if depth.is_truecolor() {
        ColorMode::TrueColor
    } else {
        ColorMode::Indexed256
    }
}

fn hex(c: Color) -> Option<String> {
    match c {
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        _ => None,
    }
}

/// The terminal's number for a color: the nearest of the 256 for a 24-bit one, the index for a named one, None for
/// the terminal's own default.
pub fn cterm(c: Color) -> Option<u8> {
    Some(match c {
        Color::Rgb(r, g, b) => rgb_to_256(r, g, b),
        Color::Indexed(i) => i,
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Reset => return None,
    })
}

/// A color as the session file holds it: `{"gui": "#rrggbb", "cterm": n}`, the 24-bit form only in truecolor.
fn value(c: Color, mode: ColorMode) -> Option<Value> {
    let gui = if mode == ColorMode::TrueColor { hex(c) } else { None };
    let n = cterm(c);
    if gui.is_none() && n.is_none() {
        return None;
    }
    Some(json!({"gui": gui, "cterm": n}))
}

/// `a` moved `t` of the way to `b`, when both are 24-bit colors.
fn mixed(a: Color, b: Color, t: f64) -> Option<Color> {
    (matches!(a, Color::Rgb(..)) && matches!(b, Color::Rgb(..))).then(|| theme::mix(a, b, t))
}

/// Base16 for syntax colors, from the theme's own: the background, surface, selection, dim, subtext and text for the
/// greys, the palette's bright colors for the rest, each lifted until it reads on the background like platoon's own
/// roles (`theme::MIN_ROLE`). None for Terminal colors.
pub fn base16(t: &Theme) -> Option<[(String, Color); 16]> {
    let ansi = t.ansi?;
    let lift = |c: Color| theme::lift_contrast(c, t.bg, t.text, theme::MIN_ROLE);
    let white = Color::Rgb(255, 255, 255);
    let sel = t.selection_bg().unwrap_or_else(|| theme::mix(t.bg, t.accent, 0.25));
    let cols = [
        t.bg,
        t.surface,
        sel,
        t.dim,
        t.subtext,
        t.text,
        theme::mix(t.text, white, 0.3),
        theme::mix(t.text, white, 0.6),
        lift(ansi[9]),
        lift(theme::mix(ansi[9], ansi[11], 0.5)),
        lift(ansi[11]),
        lift(ansi[10]),
        lift(ansi[14]),
        lift(ansi[12]),
        lift(ansi[13]),
        lift(theme::mix(ansi[1], ansi[3], 0.5)),
    ];
    let names = ["base00", "base01", "base02", "base03", "base04", "base05", "base06", "base07", "base08", "base09", "base0A", "base0B", "base0C", "base0D", "base0E", "base0F"];
    Some(std::array::from_fn(|i| (names[i].to_string(), cols[i])))
}

/// A highlight group: foreground, background, bold; or a link to another group.
struct Group {
    name: &'static str,
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    link: Option<&'static str>,
}

fn g(name: &'static str, fg: Option<Color>, bg: Option<Color>) -> Group {
    Group { name, fg, bg, bold: false, link: None }
}
fn gb(name: &'static str, fg: Option<Color>, bg: Option<Color>) -> Group {
    Group { name, fg, bg, bold: true, link: None }
}
fn link(name: &'static str, to: &'static str) -> Group {
    Group { name, fg: None, bg: None, bold: false, link: Some(to) }
}

/// The groups the editor's pieces are drawn with, from platoon's roles: added is `ok`, changed `warn`, removed `err`
/// (in the diffs, the change marks, the tree), the accent for what is picked or matched, `busy` for new files.
fn groups(t: &Theme) -> Vec<Group> {
    let s = Some;
    let sel = t.selection_bg();
    let (bg, surface, border, dim, subtext, text, accent, busy, ok, warn, err) = (t.bg, t.surface, t.border, t.dim, t.subtext, t.text, t.accent, t.busy, t.ok, t.warn, t.err);
    vec![
        g("Normal", s(text), s(bg)),
        g("NormalFloat", s(text), s(surface)),
        g("FloatBorder", s(border), s(surface)),
        gb("FloatTitle", s(accent), s(surface)),
        g("Pmenu", s(text), s(surface)),
        gb("PmenuSel", s(text), sel),
        g("PmenuSbar", None, s(surface)),
        g("PmenuThumb", None, s(border)),
        g("CursorLine", None, s(surface)),
        gb("CursorLineNr", s(accent), None),
        g("LineNr", s(dim), None),
        g("SignColumn", None, s(bg)),
        g("Visual", None, sel),
        g("WinSeparator", s(border), None),
        g("StatusLine", s(subtext), s(surface)),
        g("StatusLineNC", s(dim), s(surface)),
        g("Search", s(t.on_accent), s(warn)),
        g("CurSearch", s(t.on_accent), s(accent)),
        g("IncSearch", s(t.on_accent), s(accent)),
        gb("MatchParen", s(accent), None),
        g("Directory", s(accent), None),
        g("Title", s(accent), None),
        g("DiffAdd", None, mixed(bg, ok, 0.22)),
        g("DiffChange", None, mixed(bg, warn, 0.14)),
        g("DiffText", None, mixed(bg, warn, 0.32)),
        g("DiffDelete", mixed(bg, err, 0.6), mixed(bg, err, 0.18)),
        g("Added", s(ok), None),
        g("Changed", s(warn), None),
        g("Removed", s(err), None),
        link("diffAdded", "Added"),
        link("diffChanged", "Changed"),
        link("diffRemoved", "Removed"),
        g("GitSignsAdd", s(ok), None),
        g("GitSignsChange", s(warn), None),
        g("GitSignsDelete", s(err), None),
        g("GitSignsTopdelete", s(err), None),
        g("GitSignsChangedelete", s(warn), None),
        g("GitSignsUntracked", s(busy), None),
        link("GitSignsAddPreview", "DiffAdd"),
        link("GitSignsDeletePreview", "DiffDelete"),
        g("NeoTreeNormal", s(text), s(bg)),
        g("NeoTreeNormalNC", s(text), s(bg)),
        g("NeoTreeWinSeparator", s(border), s(bg)),
        gb("NeoTreeRootName", s(accent), None),
        g("NeoTreeDirectoryName", s(text), None),
        g("NeoTreeDirectoryIcon", s(accent), None),
        g("NeoTreeFileName", s(text), None),
        g("NeoTreeIndentMarker", s(border), None),
        g("NeoTreeExpander", s(dim), None),
        g("NeoTreeDimText", s(dim), None),
        g("NeoTreeMessage", s(dim), None),
        g("NeoTreeModified", s(warn), None),
        g("NeoTreeGitAdded", s(ok), None),
        g("NeoTreeGitModified", s(warn), None),
        g("NeoTreeGitDeleted", s(err), None),
        g("NeoTreeGitRenamed", s(warn), None),
        g("NeoTreeGitUntracked", s(busy), None),
        gb("NeoTreeGitConflict", s(err), None),
        g("NeoTreeGitStaged", s(ok), None),
        g("NeoTreeGitUnstaged", s(warn), None),
        g("NeoTreeGitIgnored", s(dim), None),
        gb("NeoTreeTabActive", s(t.on_accent), s(accent)),
        g("NeoTreeTabInactive", s(subtext), s(surface)),
        g("NeoTreeTabSeparatorActive", s(accent), s(accent)),
        g("NeoTreeTabSeparatorInactive", s(surface), s(surface)),
        g("WhichKey", s(accent), None),
        g("WhichKeyGroup", s(busy), None),
        g("WhichKeyDesc", s(text), None),
        g("WhichKeySeparator", s(dim), None),
        g("WhichKeyValue", s(subtext), None),
        g("WhichKeyNormal", s(text), s(surface)),
        g("WhichKeyBorder", s(border), s(surface)),
        gb("WhichKeyTitle", s(accent), s(surface)),
        g("SnacksNormal", s(text), s(surface)),
        g("SnacksPickerBorder", s(border), s(surface)),
        gb("SnacksPickerTitle", s(accent), s(surface)),
        g("SnacksPickerPrompt", s(accent), None),
        gb("SnacksPickerMatch", s(accent), None),
        g("SnacksPickerDir", s(dim), None),
        g("SnacksPickerListCursorLine", None, sel),
        gb("DiffviewFilePanelTitle", s(accent), None),
        g("DiffviewFilePanelRootPath", s(accent), None),
        g("DiffviewFilePanelCounter", s(busy), None),
        g("DiffviewFilePanelInsertions", s(ok), None),
        g("DiffviewFilePanelDeletions", s(err), None),
        g("DiffviewStatusAdded", s(ok), None),
        g("DiffviewStatusUntracked", s(busy), None),
        g("DiffviewStatusModified", s(warn), None),
        g("DiffviewStatusRenamed", s(warn), None),
        g("DiffviewStatusDeleted", s(err), None),
        link("BlinkCmpMenu", "Pmenu"),
        g("BlinkCmpMenuBorder", s(border), s(surface)),
        link("BlinkCmpMenuSelection", "PmenuSel"),
        gb("BlinkCmpLabelMatch", s(accent), None),
        g("BlinkCmpKind", s(busy), None),
    ]
}

/// Everything about colors the editor is told: the mode, base16 (not for Terminal colors), the 16 terminal colors
/// (truecolor only: in 256 colors the editor's terminal shows the terminal's own), platoon's roles, the groups.
pub fn colors_json(t: &Theme, mode: ColorMode) -> Value {
    let mut out = Map::new();
    out.insert("mode".into(), json!(match mode {
        ColorMode::TrueColor => "truecolor",
        ColorMode::Indexed256 => "256",
        ColorMode::Terminal => "terminal",
    }));
    if mode != ColorMode::Terminal {
        if let Some(b) = base16(t) {
            let pal: Map<String, Value> = b.iter().filter_map(|(k, c)| hex(*c).map(|h| (k.clone(), json!(h)))).collect();
            out.insert("base16".into(), Value::Object(pal));
            if mode == ColorMode::Indexed256 {
                let ct: Map<String, Value> = b.iter().filter_map(|(k, c)| cterm(*c).map(|n| (k.clone(), json!(n)))).collect();
                out.insert("base16_cterm".into(), Value::Object(ct));
            }
        }
        if mode == ColorMode::TrueColor {
            if let Some(a) = t.ansi {
                out.insert("terminal".into(), Value::Array(a.iter().filter_map(|c| hex(*c)).map(Value::String).collect()));
            }
        }
    }
    let mut roles = Map::new();
    for (name, c) in t.roles().into_iter().chain([("on_accent", t.on_accent)]) {
        if let Some(v) = value(c, mode) {
            roles.insert(name.into(), v);
        }
    }
    out.insert("roles".into(), Value::Object(roles));
    let mut list = Vec::new();
    for gr in groups(t) {
        if let Some(to) = gr.link {
            list.push(json!({"name": gr.name, "link": to}));
            continue;
        }
        let fg = gr.fg.and_then(|c| value(c, mode));
        let bg = gr.bg.and_then(|c| value(c, mode));
        if fg.is_none() && bg.is_none() {
            // Nothing this terminal can be told (the terminal's own default, or a tint of colors it doesn't have).
            continue;
        }
        let mut o = Map::new();
        o.insert("name".into(), json!(gr.name));
        if let Some(v) = fg {
            o.insert("fg".into(), v);
        }
        if let Some(v) = bg {
            o.insert("bg".into(), v);
        }
        if gr.bold {
            o.insert("bold".into(), json!(true));
        }
        list.push(Value::Object(o));
    }
    out.insert("groups".into(), Value::Array(list));
    Value::Object(out)
}

// ---- the session file -----------------------------------------------------------------------------------------------

/// How the editor looks: platoon's theme as it is drawn now, the terminal's colors, the font's icons; and where its
/// copies go.
pub struct Look<'a> {
    pub theme: &'a Theme,
    pub mode: ColorMode,
    pub icons: bool,
    /// A parser platoon didn't build can be built on first open (tree-sitter and a C compiler are there).
    pub compile_parsers: bool,
    pub copying: Copying,
}

/// Where a yank in the editor goes: where platoon's own copies go (`clipboard::Target`, from `PLATOON_CLIPBOARD`).
/// For the system clipboard the editor runs `helper` (this platoon, `clipboard-hold`) with the text; OSC 52 it writes
/// itself (`init.lua`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copying {
    pub target: crate::clipboard::Target,
    pub helper: Option<PathBuf>,
}

impl Copying {
    /// Copies go where platoon's do; the helper is this very program.
    pub fn like(target: crate::clipboard::Target) -> Copying {
        use crate::clipboard::Target;
        let helper = if target == Target::System { std::env::current_exe().ok() } else { None };
        Copying { target, helper }
    }

    /// Nothing leaves the editor.
    #[cfg(test)]
    pub fn off() -> Copying {
        Copying { target: crate::clipboard::Target::Off, helper: None }
    }

    fn json(&self) -> Value {
        use crate::clipboard::{Target, HOLD_COMMAND, MAX_OSC52_PAYLOAD};
        let target = match self.target {
            Target::System => "system",
            Target::Osc52 => "osc52",
            Target::Off => "off",
        };
        let helper = match (&self.helper, self.target) {
            (Some(p), Target::System) => json!([p, HOLD_COMMAND]),
            _ => Value::Null,
        };
        json!({"target": target, "helper": helper, "osc52_max": MAX_OSC52_PAYLOAD})
    }
}

pub fn config_dir(dirs: &Dirs) -> PathBuf {
    dirs.editor.join("config")
}

/// nvim's own XDG folders, all inside platoon's editor folder.
pub fn xdg_dir(dirs: &Dirs) -> PathBuf {
    dirs.editor.join("nvim")
}

pub fn session_json(req: &Request, look: &Look, dirs: &Dirs) -> Value {
    let roots: Vec<Value> = req
        .roots
        .iter()
        .map(|r| json!({"path": r.path, "name": r.name, "repo": r.repo, "repo_name": r.repo_name, "base": r.base, "kind": r.kind}))
        .collect();
    json!({
        "version": 1,
        "title": req.title,
        "site": dirs.site(),
        "config_dir": config_dir(dirs),
        "shell": config_dir(dirs).join("shell"),
        "icons": look.icons,
        "compile_parsers": look.compile_parsers,
        "roots": roots,
        "colors": colors_json(look.theme, look.mode),
        "clipboard": look.copying.json(),
    })
}

// ---- the environment ------------------------------------------------------------------------------------------------

/// What nvim's environment changes from platoon's (`None`: taken away). Its XDG folders are inside the editor
/// folder, platoon's programs come first on PATH, nothing of the user's own nvim setup is read, and the user's values
/// of all those are kept in `PLATOON_KEEP_<name>` for the terminal (`shell.sh`).
pub fn nvim_env(dirs: &Dirs, session: &Path, env: &dyn Fn(&str) -> Option<String>) -> Vec<(String, Option<String>)> {
    let x = xdg_dir(dirs);
    let s = |p: PathBuf| Some(p.to_string_lossy().into_owned());
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for k in KEPT {
        out.push((format!("PLATOON_KEEP_{k}"), env(k)));
    }
    let path = match env("PATH").filter(|p| !p.is_empty()) {
        Some(p) => format!("{}:{p}", dirs.bin().display()),
        None => dirs.bin().to_string_lossy().into_owned(),
    };
    out.push(("PATH".into(), Some(path)));
    out.push(("XDG_CONFIG_HOME".into(), s(x.join("config"))));
    out.push(("XDG_DATA_HOME".into(), s(x.join("data"))));
    out.push(("XDG_STATE_HOME".into(), s(x.join("state"))));
    out.push(("XDG_CACHE_HOME".into(), s(x.join("cache"))));
    out.push(("XDG_CONFIG_DIRS".into(), s(x.join("none"))));
    out.push(("XDG_DATA_DIRS".into(), s(x.join("none"))));
    for k in ["NVIM_APPNAME", "NVIM_LOG_FILE", "VIMINIT", "VIMRUNTIME", "VIM", "MYVIMRC"] {
        out.push((k.into(), None));
    }
    // If platoon itself was started from inside Claude Code, that isn't passed on (as for the other editors).
    out.push(("CLAUDECODE".into(), None));
    out.push(("CLAUDE_CODE_ENTRYPOINT".into(), None));
    out.push(("PLATOON_EDITOR_ENV".into(), Some("1".into())));
    out.push(("PLATOON_EDITOR_SESSION".into(), s(session.to_path_buf())));
    out
}

/// The program to run: the stand-in a test names, else platoon's nvim. Err in plain words.
pub fn program(dirs: &Dirs, env: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, String> {
    if let Some(p) = env("PLATOON_NVIM_BIN").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    if env("PLATOON_FORBID_REAL_EDITOR").is_some() {
        return Err("PLATOON_FORBID_REAL_EDITOR is set: refusing to start the real editor".into());
    }
    dirs.nvim().ok_or_else(|| "the code editor isn't installed".into())
}

/// A test names its own stand-in: then nothing needs to be installed.
pub fn stand_in(env: &dyn Fn(&str) -> Option<String>) -> bool {
    env("PLATOON_NVIM_BIN").is_some_and(|v| !v.is_empty())
}

/// Enough of the editor is installed to open it: nvim and every plugin (up to date or not). Parsers, the tools
/// and the font may be missing; the editor works without them.
pub fn ready(st: &crate::editor_install::Status) -> bool {
    use crate::editor_install::{ItemState, Part};
    !st.unsupported && st.items.iter().filter(|i| matches!(i.part, Part::Neovim | Part::Plugins)).all(|i| i.state != ItemState::Missing)
}

// ---- starting it ----------------------------------------------------------------------------------------------------

/// Everything needed to run the editor once.
#[derive(Debug)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, Option<String>)>,
    pub cwd: PathBuf,
    /// Removed when the editor has closed.
    pub session: PathBuf,
}

fn write_if_changed(path: &Path, text: &str) -> io::Result<()> {
    if fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    crate::projects::write_atomic(path, text)
}

/// The user's own git settings folder (`$XDG_CONFIG_HOME/git`, else `~/.config/git`) is linked into the editor's
/// config folder, so git run by the editor's plugins (with the editor's XDG folders) still reads the user's global
/// ignore file and settings. Nothing there is written.
fn link_git_config(dirs: &Dirs, env: &dyn Fn(&str) -> Option<String>) {
    let theirs = env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|c| c.join("git"));
    let ours = xdg_dir(dirs).join("config/git");
    let is_link = fs::symlink_metadata(&ours).is_ok_and(|m| m.file_type().is_symlink());
    match theirs.filter(|t| t.is_dir()) {
        Some(t) => {
            if is_link && fs::read_link(&ours).is_ok_and(|l| l == t) {
                return;
            }
            if is_link {
                let _ = fs::remove_file(&ours);
            }
            if !ours.exists() {
                let _ = std::os::unix::fs::symlink(&t, &ours);
            }
        }
        None if is_link => {
            let _ = fs::remove_file(&ours);
        }
        None => {}
    }
}

/// Session files of platoons that are gone (one killed while its editor was open) are taken away.
fn forget_old_sessions(cfg: &Path) {
    for e in fs::read_dir(cfg).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_prefix("session-").and_then(|r| r.strip_suffix(".json")).and_then(|p| p.parse::<i32>().ok()) else { continue };
        // SAFETY: signal 0 only asks whether the process is there.
        if pid > 0 && unsafe { libc::kill(pid, 0) } != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Write the editor's config and this opening's session file, make its folders, and say how to run it.
pub fn prepare(dirs: &Dirs, req: &Request, look: &Look, env: &dyn Fn(&str) -> Option<String>, id: u32) -> Result<Launch, String> {
    let program = program(dirs, env)?;
    let cfg = config_dir(dirs);
    let err = |what: &str, e: io::Error| format!("couldn't write the editor's {what}: {e}");
    write_if_changed(&cfg.join("init.lua"), INIT_LUA).map_err(|e| err("settings", e))?;
    write_if_changed(&cfg.join("lua/platoon/tree.lua"), TREE_LUA).map_err(|e| err("settings", e))?;
    let shell = cfg.join("shell");
    write_if_changed(&shell, SHELL_SH).map_err(|e| err("shell script", e))?;
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).map_err(|e| err("shell script", e))?;
    // (with nvim's own folder in each: plugins write their logs there without making it first)
    for d in ["config/nvim", "data/nvim", "state/nvim", "cache/nvim", "none"] {
        fs::create_dir_all(xdg_dir(dirs).join(d)).map_err(|e| err("folders", e))?;
    }
    link_git_config(dirs, env);
    forget_old_sessions(&cfg);
    let session = cfg.join(format!("session-{id}.json"));
    let text = serde_json::to_string_pretty(&session_json(req, look, dirs)).unwrap_or_default();
    crate::projects::write_atomic(&session, &text).map_err(|e| err("session", e))?;
    let cwd = req.roots.first().map(|r| PathBuf::from(&r.path)).filter(|p| p.is_dir()).unwrap_or_else(|| PathBuf::from(env("HOME").unwrap_or_else(|| "/".into())));
    Ok(Launch {
        program,
        args: vec!["-u".into(), cfg.join("init.lua").to_string_lossy().into_owned()],
        env: nvim_env(dirs, &session, env),
        cwd,
        session,
    })
}

/// While the editor runs, Ctrl+C, Ctrl+\ and Ctrl+Z typed at a program it started (`:!`) reach that program, not
/// platoon: platoon ignores them, and gives its old handling back afterwards.
struct Ignoring(Vec<(i32, libc::sigaction)>);

impl Ignoring {
    fn start() -> Ignoring {
        let mut old = Vec::new();
        for sig in [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP, libc::SIGTTOU, libc::SIGTTIN] {
            // SAFETY: plain sigaction calls with zeroed structs; the old handling is kept to be put back.
            unsafe {
                let mut ign: libc::sigaction = std::mem::zeroed();
                ign.sa_sigaction = libc::SIG_IGN;
                let mut was: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(sig, &ign, &mut was) == 0 {
                    old.push((sig, was));
                }
            }
        }
        Ignoring(old)
    }
}

impl Drop for Ignoring {
    fn drop(&mut self) {
        for (sig, was) in &self.0 {
            // SAFETY: puts back exactly what `start` found.
            unsafe {
                libc::sigaction(*sig, was, std::ptr::null_mut());
            }
        }
    }
}

/// Run the editor in the foreground on platoon's terminal and wait until it has closed. The caller has handed the
/// terminal over (and takes it back after).
pub fn run(l: &Launch) -> io::Result<ExitStatus> {
    use std::os::unix::process::ExitStatusExt;
    let mut cmd = Command::new(&l.program);
    cmd.args(&l.args).current_dir(&l.cwd);
    for (k, v) in &l.env {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    // SAFETY: only async-signal-safe calls between fork and exec: the editor gets the usual signal handling back.
    unsafe {
        cmd.pre_exec(|| {
            for sig in [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP, libc::SIGTTOU, libc::SIGTTIN] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    let _ignoring = Ignoring::start();
    let child = cmd.spawn().map_err(|e| io::Error::other(format!("couldn't start {}: {e}", l.program.display())))?;
    let pid = child.id() as libc::pid_t;
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: waits for our own child.
        let r = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if libc::WIFSTOPPED(status) {
            // It stopped itself (a suspend): platoon is waiting behind it and can't be the shell that resumes it, so
            // it carries on. (The editor's Ctrl+Z does nothing; `q` goes back.)
            // SAFETY: our own child.
            unsafe { libc::kill(pid, libc::SIGCONT) };
            continue;
        }
        return Ok(ExitStatus::from_raw(status));
    }
}

/// Done with one opening: its session file goes.
pub fn finish(l: &Launch) {
    let _ = fs::remove_file(&l.session);
}

// ---- which folders an editor has open ---------------------------------------------------------------------------

/// Where platoon notes the folders an editor has open, one file per platoon (by pid), so that no platoon window
/// removes a ticket's worktree from under an editor another window opened.
fn marks_dir(cache: &Path) -> PathBuf {
    cache.join("editor-open")
}

/// Note that this platoon's editor has `dirs` open.
pub fn mark_open(cache: &Path, pid: u32, dirs: &[String]) -> io::Result<PathBuf> {
    let p = marks_dir(cache).join(format!("{pid}.json"));
    crate::projects::write_atomic(&p, &json!({"pid": pid, "dirs": dirs}).to_string())?;
    Ok(p)
}

pub fn unmark(path: &Path) {
    let _ = fs::remove_file(path);
}

/// The folders editors have open now, from every platoon whose process is still there (a mark left by one that is
/// gone is tidied away).
pub fn open_now(cache: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(marks_dir(cache)) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        let Some(v) = fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let pid = v.get("pid").and_then(Value::as_u64).unwrap_or(0) as i32;
        // SAFETY: signal 0 only asks whether the process is there.
        if pid <= 0 || unsafe { libc::kill(pid, 0) } != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            let _ = fs::remove_file(&p);
            continue;
        }
        out.extend(v.get("dirs").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_install::testing;
    use crate::tickets::Worktree;
    use std::collections::HashMap;
    use std::process::Stdio;

    fn tmp(name: &str) -> PathBuf {
        testing::tmp(&format!("ed-{name}"))
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn repo(at: &Path) -> String {
        fs::create_dir_all(at).unwrap();
        fs::write(at.join("README"), "hi\n").unwrap();
        git(at, &["init", "-q"]);
        git(at, &["add", "."]);
        git(at, &["commit", "-q", "-m", "first"]);
        git(at, &["rev-parse", "HEAD"])
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn a_session_opens_its_folder_and_the_folders_added_to_it() {
        let d = tmp("session");
        let home = d.join("home");
        let main = home.join("repos/app");
        repo(&main);
        let docs = home.join("notes/docs");
        fs::create_dir_all(&docs).unwrap();
        let other = d.join("srv/app");
        fs::create_dir_all(&other).unwrap();
        let extra = vec!["~/notes/docs".to_string(), s(&other), "~/gone".to_string(), "~/repos/app".to_string()];
        let roots = session_roots(&s(&main), &extra, &s(&home), "git");
        let paths: Vec<&str> = roots.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec![s(&main), s(&docs), s(&other)], "home expanded, missing and repeated folders left out");
        assert_eq!(roots[0].repo.as_deref(), Some(s(&main).as_str()));
        assert_eq!(roots[0].repo_name.as_deref(), Some("app"));
        assert_eq!(roots[0].base, None, "a session shows what is not yet committed");
        assert_eq!(roots[1].repo, None, "not in git");
        assert_eq!(roots[0].name, "app");
        assert_eq!(roots[2].name, "app (srv)", "two entries never share a name");
        assert!(roots.iter().all(|r| r.kind == "folder"));
    }

    #[test]
    fn a_folder_picked_by_hand_is_opened_as_it_is() {
        let d = tmp("folder");
        assert_eq!(folder_roots(&s(&d.join("nope")), "git"), vec![]);
        let r = folder_roots(&s(&d), "git");
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].path.as_str(), r[0].repo.as_deref(), r[0].kind), (s(&d).as_str(), None, "folder"));
    }

    fn ticket(number: u32, folders: Vec<String>, worktrees: Vec<Worktree>) -> Ticket {
        Ticket {
            number,
            title: "Basket".into(),
            file: String::new(),
            state: crate::tickets::State::Working,
            folders,
            blocked_by: vec![],
            created: 0,
            updated: 0,
            leaders: vec![],
            worktrees,
            reason: String::new(),
            go_ahead: false,
            handover: None,
            permission: false,
            busy: None,
            queued: None,
            from: None,
        }
    }

    #[test]
    fn a_ticket_opens_its_worktrees_with_their_base_its_plain_folders_and_its_work_folder() {
        let d = tmp("ticket");
        let shop = d.join("repos/shop");
        let base = repo(&shop);
        let plain = d.join("plain");
        fs::create_dir_all(&plain).unwrap();
        let folders = vec![s(&shop), s(&plain)];
        let made = crate::squads::prepare_worktrees("git", &folders, 4, &[]).unwrap();
        assert_eq!(made.len(), 1, "only the git folder gets a worktree");
        let w = made[0].clone();
        assert_eq!(w.base.as_deref(), Some(base.as_str()));
        fs::write(Path::new(&w.path).join("basket.txt"), "b\n").unwrap();
        git(Path::new(&w.path), &["add", "."]);
        git(Path::new(&w.path), &["commit", "-q", "-m", "basket"]);
        let work = d.join("04-work");
        fs::create_dir_all(&work).unwrap();
        let t = ticket(4, folders.clone(), made.clone());
        let roots = ticket_roots(&t, &work, "git");
        assert_eq!(roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>(), vec![w.path.clone(), s(&plain)], "an empty work folder is left out");
        assert_eq!(roots[0].kind, "worktree");
        assert_eq!(roots[0].base.as_deref(), Some(base.as_str()), "the commit it was made from, not the branch's tip");
        assert_eq!(roots[0].repo.as_deref(), Some(w.path.as_str()));
        assert_eq!(roots[0].repo_name.as_deref(), Some("shop"));
        assert_eq!(roots[0].name, "shop · #4");
        assert_eq!((roots[1].kind, roots[1].repo.as_deref(), roots[1].base.as_deref()), ("folder", None, None));

        fs::write(work.join("notes.md"), "x").unwrap();
        let roots = ticket_roots(&t, &work, "git");
        assert_eq!(roots.last().map(|r| (r.path.clone(), r.kind, r.name.clone())), Some((s(&work), "work", "#4 work folder".into())));

        // An old ticket with no base written down: where its branch left main.
        let mut old = w.clone();
        old.base = None;
        let roots = ticket_roots(&ticket(4, folders.clone(), vec![old]), &work, "git");
        assert_eq!(roots[0].base.as_deref(), Some(base.as_str()));

        // A removed worktree is left out (the app brings it back before opening).
        let mut gone = w.clone();
        gone.removed = Some(1);
        let roots = ticket_roots(&ticket(4, folders.clone(), vec![gone]), &work, "git");
        assert!(roots.iter().all(|r| r.kind != "worktree"), "{roots:?}");
        assert!(roots.iter().all(|r| r.path != s(&shop)), "nor is the repository itself opened in its place");
    }

    #[test]
    fn a_ticket_not_handed_to_a_squad_yet_opens_its_folders_with_what_is_not_committed() {
        let d = tmp("new-ticket");
        let shop = d.join("shop");
        repo(&shop);
        let roots = ticket_roots(&ticket(2, vec![s(&shop)], vec![]), &d.join("none"), "git");
        assert_eq!(roots.len(), 1);
        assert_eq!((roots[0].path.clone(), roots[0].repo.clone(), roots[0].base.clone()), (s(&shop), Some(s(&shop)), None));
    }

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| m.get(k).cloned()
    }

    #[test]
    fn nvim_keeps_everything_inside_the_editor_folder_and_the_terminal_gets_the_users_environment_back() {
        let d = tmp("env");
        let dirs = testing::dirs(&d, vec![]);
        let env = env_of(&[("PATH", "/usr/bin:/bin"), ("XDG_CONFIG_HOME", "/home/u/.config"), ("NVIM_APPNAME", "mine"), ("HOME", "/home/u")]);
        let session = d.join("s.json");
        let got: HashMap<String, Option<String>> = nvim_env(&dirs, &session, &env).into_iter().collect();
        let x = xdg_dir(&dirs);
        for (k, sub) in [("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache"), ("XDG_CONFIG_DIRS", "none"), ("XDG_DATA_DIRS", "none")] {
            let v = got[k].clone().unwrap();
            assert_eq!(v, s(&x.join(sub)), "{k}");
            assert!(Path::new(&v).starts_with(&dirs.editor), "{k} inside the editor folder");
        }
        assert_eq!(got["PATH"].as_deref(), Some(format!("{}:/usr/bin:/bin", dirs.bin().display()).as_str()));
        for k in ["NVIM_APPNAME", "NVIM_LOG_FILE", "VIMINIT", "VIMRUNTIME", "MYVIMRC", "CLAUDECODE"] {
            assert_eq!(got[k], None, "{k} taken away");
        }
        // The user's own values, for the terminal: set ones kept, unset ones said to be unset.
        assert_eq!(got["PLATOON_KEEP_PATH"].as_deref(), Some("/usr/bin:/bin"));
        assert_eq!(got["PLATOON_KEEP_XDG_CONFIG_HOME"].as_deref(), Some("/home/u/.config"));
        assert_eq!(got["PLATOON_KEEP_NVIM_APPNAME"].as_deref(), Some("mine"));
        assert_eq!(got["PLATOON_KEEP_XDG_DATA_HOME"], None);
        assert_eq!(got["PLATOON_EDITOR_SESSION"].as_deref(), Some(s(&session).as_str()));
        assert_eq!(got["PLATOON_EDITOR_ENV"].as_deref(), Some("1"));
        for k in KEPT {
            assert!(SHELL_SH.contains(k), "the shell script puts {k} back");
            assert!(got.contains_key(&format!("PLATOON_KEEP_{k}")));
        }
    }

    /// The shell script, run for real: the user's values back, the unset ones unset, the editor's own gone.
    #[test]
    fn the_editors_shell_puts_the_users_environment_back() {
        let d = tmp("shell");
        let script = d.join("shell");
        fs::write(&script, SHELL_SH).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let dirs = testing::dirs(&d, vec![]);
        let env = env_of(&[("PATH", "/usr/bin:/bin"), ("XDG_CONFIG_HOME", "/home/u/.config")]);
        let mut cmd = Command::new(&script);
        cmd.env_clear().env("SHELL", "/bin/sh").args(["-c", "echo \"P=$PATH C=${XDG_CONFIG_HOME-unset} D=${XDG_DATA_HOME-unset} K=${PLATOON_KEEP_PATH-unset} E=${PLATOON_EDITOR_ENV-unset}\""]);
        for (k, v) in nvim_env(&dirs, &d.join("s.json"), &env) {
            if let Some(v) = v {
                cmd.env(k, v);
            }
        }
        let out = cmd.output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "P=/usr/bin:/bin C=/home/u/.config D=unset K=unset E=unset");
        // Run again from inside (a shell started from that shell): nothing more changes.
        let out = Command::new(&script).env_clear().env("SHELL", "/bin/sh").env("PATH", "/x").args(["-c", "echo $PATH"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/x");
    }

    #[test]
    fn a_stand_in_or_nothing_when_the_real_editor_is_forbidden() {
        let d = tmp("program");
        let dirs = testing::dirs(&d, vec![]);
        assert_eq!(program(&dirs, &env_of(&[("PLATOON_NVIM_BIN", "/x/fake")])), Ok(PathBuf::from("/x/fake")));
        assert!(program(&dirs, &env_of(&[("PLATOON_FORBID_REAL_EDITOR", "1")])).unwrap_err().contains("refusing"));
        assert!(program(&dirs, &env_of(&[])).unwrap_err().contains("isn't installed"));
        testing::write_exe(&dirs.bin().join("nvim"), "exit 0");
        assert_eq!(program(&dirs, &env_of(&[])), Ok(dirs.bin().join("nvim")));
        assert!(stand_in(&env_of(&[("PLATOON_NVIM_BIN", "/x")])) && !stand_in(&env_of(&[])));
    }

    fn group<'a>(c: &'a Value, name: &str) -> &'a Value {
        c["groups"].as_array().unwrap().iter().find(|g| g["name"] == name).unwrap_or_else(|| panic!("no group {name}"))
    }

    #[test]
    fn truecolor_sends_the_themes_own_colors() {
        let t = Theme::by_id("catppuccin-mocha").unwrap();
        assert_eq!(color_mode(&t, Depth::TrueColor, false), ColorMode::TrueColor);
        let c = colors_json(&t, ColorMode::TrueColor);
        assert_eq!(c["mode"], "truecolor");
        let h = |col: Color| hex(col).unwrap();
        assert_eq!(c["base16"]["base00"], h(t.bg).as_str());
        assert_eq!(c["base16"]["base05"], h(t.text).as_str());
        assert_eq!(c["base16"]["base03"], h(t.dim).as_str());
        assert!(c.get("base16_cterm").is_none());
        let ansi = t.ansi.unwrap();
        let term: Vec<String> = c["terminal"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        assert_eq!(term, ansi.iter().map(|a| h(*a)).collect::<Vec<_>>(), "the terminal's 16 colors are the theme's");
        // Added is ok, changed warn, removed err: in the change marks, the tree and the diffs.
        assert_eq!(group(&c, "GitSignsAdd")["fg"]["gui"], h(t.ok).as_str());
        assert_eq!(group(&c, "GitSignsChange")["fg"]["gui"], h(t.warn).as_str());
        assert_eq!(group(&c, "GitSignsDelete")["fg"]["gui"], h(t.err).as_str());
        assert_eq!(group(&c, "NeoTreeGitModified")["fg"]["gui"], h(t.warn).as_str());
        assert_eq!(group(&c, "NeoTreeGitAdded")["fg"]["gui"], h(t.ok).as_str());
        assert_eq!(group(&c, "DiffAdd")["bg"]["gui"], h(theme::mix(t.bg, t.ok, 0.22)).as_str());
        assert_eq!(group(&c, "DiffDelete")["bg"]["gui"], h(theme::mix(t.bg, t.err, 0.18)).as_str());
        assert_eq!(group(&c, "Visual")["bg"]["gui"], h(t.selection_bg().unwrap()).as_str());
        assert_eq!(group(&c, "Normal")["fg"]["cterm"], json!(cterm(t.text)), "a fallback number too");
        assert_eq!(c["roles"]["accent"]["gui"], h(t.accent).as_str());
        assert_eq!(group(&c, "diffAdded")["link"], "Added");
    }

    #[test]
    fn syntax_colors_read_on_the_background_like_platoons_own() {
        for t in Theme::all().into_iter().filter(|t| t.ansi.is_some()) {
            let b = base16(&t).unwrap();
            for (name, c) in &b[8..] {
                assert!(theme::contrast(*c, t.bg) >= theme::MIN_ROLE - 0.01, "{} {name}: {:.2}", t.id, theme::contrast(*c, t.bg));
            }
            assert_eq!(b[0].1, t.bg);
            assert_eq!(b[5].1, t.text);
        }
    }

    #[test]
    fn in_256_colors_nothing_24_bit_is_sent() {
        let t = Theme::by_id("tokyo-night").unwrap_or_default();
        assert_eq!(color_mode(&t, Depth::Indexed256, false), ColorMode::Indexed256);
        let c = colors_json(&t, ColorMode::Indexed256);
        assert_eq!(c["mode"], "256");
        assert!(c.get("terminal").is_none(), "the editor's terminal keeps the terminal's own colors");
        for g in c["groups"].as_array().unwrap() {
            for k in ["fg", "bg"] {
                if let Some(v) = g.get(k) {
                    assert!(v["gui"].is_null(), "{g}");
                    assert!(v["cterm"].as_u64().is_some_and(|n| n >= 16), "{g}: the 16 colors belong to the terminal's theme");
                }
            }
        }
        let Color::Rgb(r, gg, b) = t.ok else { panic!() };
        assert_eq!(group(&c, "GitSignsAdd")["fg"]["cterm"], json!(rgb_to_256(r, gg, b)));
        let Color::Rgb(r, gg, b) = t.bg else { panic!() };
        assert_eq!(c["base16_cterm"]["base00"], json!(rgb_to_256(r, gg, b)), "base16 in the same numbers platoon draws with");
        assert!(c["roles"]["bg"]["gui"].is_null());
    }

    #[test]
    fn terminal_colors_use_the_terminals_own_sixteen_and_force_nothing_else() {
        let t = Theme::terminal();
        assert_eq!(color_mode(&t, Depth::TrueColor, false), ColorMode::Terminal);
        assert_eq!(color_mode(&Theme::default(), Depth::TrueColor, true), ColorMode::Terminal, "NO_COLOR");
        let c = colors_json(&t, ColorMode::Terminal);
        assert_eq!(c["mode"], "terminal");
        assert!(c.get("base16").is_none() && c.get("terminal").is_none());
        assert_eq!(group(&c, "GitSignsAdd")["fg"], json!({"gui": null, "cterm": 10}), "LightGreen");
        assert_eq!(group(&c, "GitSignsDelete")["fg"]["cterm"], json!(9));
        assert_eq!(group(&c, "NeoTreeGitModified")["fg"]["cterm"], json!(11));
        let names: Vec<&str> = c["groups"].as_array().unwrap().iter().filter_map(|g| g["name"].as_str()).collect();
        for tinted in ["DiffAdd", "DiffChange", "Visual"] {
            assert!(!names.contains(&tinted), "{tinted}: a tint of colors the terminal decides is left to it");
        }
        // Normal has the terminal's own text on its own background: nothing to say, so nothing is said.
        assert!(!names.contains(&"Normal"));
        for g in c["groups"].as_array().unwrap() {
            for k in ["fg", "bg"] {
                if let Some(v) = g.get(k) {
                    assert!(v["gui"].is_null() && v["cterm"].as_u64().is_some_and(|n| n < 16), "{g}");
                }
            }
        }
    }

    #[test]
    fn the_session_file_says_the_folders_bases_icons_and_colors() {
        let d = tmp("sessionjson");
        let dirs = testing::dirs(&d, vec![]);
        let t = Theme::default();
        let req = Request {
            title: "#3 Basket".into(),
            roots: vec![Root { path: "/r/shop-worktrees/ticket-3".into(), name: "shop · #3".into(), repo: Some("/r/shop-worktrees/ticket-3".into()), repo_name: Some("shop".into()), base: Some("abc123".into()), kind: "worktree" }],
        };
        for icons in [true, false] {
            let v = session_json(&req, &Look { theme: &t, mode: ColorMode::TrueColor, icons, compile_parsers: true, copying: Copying::off() }, &dirs);
            assert_eq!(v["icons"], icons);
            assert_eq!(v["title"], "#3 Basket");
            assert_eq!(v["roots"][0]["base"], "abc123");
            assert_eq!(v["roots"][0]["repo_name"], "shop");
            assert_eq!(v["site"], s(&dirs.site()).as_str());
            assert_eq!(v["shell"], s(&dirs.editor.join("config/shell")).as_str());
            assert_eq!(v["colors"]["mode"], "truecolor");
        }
        // A folder's name with quotes and backslashes stays itself (JSON, never pasted into Lua or a shell).
        let odd = Request { title: "a \"b\" \\ c".into(), roots: vec![] };
        let v = session_json(&odd, &Look { theme: &t, mode: ColorMode::TrueColor, icons: false, compile_parsers: false, copying: Copying::off() }, &dirs);
        let back: Value = serde_json::from_str(&v.to_string()).unwrap();
        assert_eq!(back["title"], "a \"b\" \\ c");
    }

    #[test]
    fn the_session_file_says_where_copies_go_as_platoon_copies() {
        use crate::clipboard::Target;
        let d = tmp("sessionclip");
        let dirs = testing::dirs(&d, vec![]);
        let t = Theme::default();
        let req = Request { title: "x".into(), roots: vec![] };
        let clip = |copying: Copying| session_json(&req, &Look { theme: &t, mode: ColorMode::TrueColor, icons: false, compile_parsers: false, copying }, &dirs)["clipboard"].clone();
        // The default (system): this very platoon holds the text on the system clipboard, and OSC 52 too.
        let sys = Copying::like(Target::System);
        let me = std::env::current_exe().unwrap();
        assert_eq!(sys.helper.as_deref(), Some(me.as_path()));
        assert_eq!(clip(sys), json!({"target": "system", "helper": [me, "clipboard-hold"], "osc52_max": crate::clipboard::MAX_OSC52_PAYLOAD}));
        // OSC 52 only, or nothing: no helper is ever named.
        assert_eq!(clip(Copying::like(Target::Osc52)), json!({"target": "osc52", "helper": null, "osc52_max": crate::clipboard::MAX_OSC52_PAYLOAD}));
        assert_eq!(clip(Copying::like(Target::Off))["target"], "off");
        assert_eq!(clip(Copying::like(Target::Off))["helper"], Value::Null);
        assert_eq!(clip(Copying { target: Target::Osc52, helper: Some(me.clone()) })["helper"], Value::Null, "a helper only for the system clipboard");
    }

    #[test]
    fn the_editors_settings_send_a_yank_out_and_paste_from_its_own_registers() {
        // `y` into the unnamed register goes out (TextYankPost); deletes don't; 'clipboard' stays empty, so `p`
        // never asks the provider, and the provider's paste is the editor's own last copy (never the terminal).
        assert!(INIT_LUA.contains("o.clipboard = ''"));
        assert!(INIT_LUA.contains("vim.api.nvim_create_autocmd('TextYankPost'"));
        assert!(INIT_LUA.contains("ev.operator == 'y' and (ev.regname == nil or ev.regname == '')"));
        assert!(INIT_LUA.contains("vim.g.clipboard = {\n  name = 'platoon',"));
        assert!(INIT_LUA.contains("require('vim.ui.clipboard.osc52').copy('+')(out)"), "OSC 52 by Neovim's own provider");
        assert!(!INIT_LUA.contains("osc52').paste"), "never OSC 52 paste: many terminals don't answer it");
        assert!(INIT_LUA.contains("return { last.lines, last.regtype }"));
        assert!(INIT_LUA.contains("pcall(vim.system, CB.helper, { stdin = text, detach = true"));
        assert!(INIT_LUA.contains("CB.target == 'system' and type(CB.helper) == 'table'"), "the helper only for the system clipboard");
    }

    #[test]
    fn preparing_writes_the_config_and_links_the_users_git_settings() {
        let d = tmp("prepare");
        let dirs = testing::dirs(&d, vec![]);
        let home = d.join("home");
        fs::create_dir_all(home.join(".config/git")).unwrap();
        let t = Theme::default();
        let root = d.join("proj");
        fs::create_dir_all(&root).unwrap();
        let req = Request { title: "x".into(), roots: folder_roots(&s(&root), "git") };
        let env = env_of(&[("PLATOON_NVIM_BIN", "/bin/true"), ("HOME", home.to_str().unwrap()), ("PATH", "/usr/bin")]);
        let l = prepare(&dirs, &req, &Look { theme: &t, mode: ColorMode::TrueColor, icons: false, compile_parsers: false, copying: Copying::off() }, &env, 7).unwrap();
        let cfg = config_dir(&dirs);
        assert_eq!(fs::read_to_string(cfg.join("init.lua")).unwrap(), INIT_LUA);
        assert_eq!(fs::read_to_string(cfg.join("lua/platoon/tree.lua")).unwrap(), TREE_LUA);
        assert!(fs::metadata(cfg.join("shell")).unwrap().permissions().mode() & 0o111 != 0);
        assert_eq!(l.session, cfg.join("session-7.json"));
        let v: Value = serde_json::from_str(&fs::read_to_string(&l.session).unwrap()).unwrap();
        assert_eq!(v["roots"][0]["path"], s(&root).as_str());
        assert_eq!(l.args, vec!["-u".to_string(), s(&cfg.join("init.lua"))]);
        assert_eq!(l.cwd, root);
        assert_eq!(l.program, PathBuf::from("/bin/true"));
        assert_eq!(fs::read_link(xdg_dir(&dirs).join("config/git")).unwrap(), home.join(".config/git"));
        for sub in ["data/nvim", "state/nvim", "cache/nvim"] {
            assert!(xdg_dir(&dirs).join(sub).is_dir());
        }
        finish(&l);
        assert!(!l.session.exists());
        // One left behind by a platoon that is gone is tidied away at the next opening; a live one's stays.
        let stale = cfg.join("session-1073741800.json");
        let live = cfg.join(format!("session-{}.json", std::process::id()));
        fs::write(&stale, "{}").unwrap();
        fs::write(&live, "{}").unwrap();
        // The user's git folder gone: the link goes too.
        fs::remove_dir(home.join(".config/git")).unwrap();
        prepare(&dirs, &req, &Look { theme: &t, mode: ColorMode::TrueColor, icons: false, compile_parsers: false, copying: Copying::off() }, &env, 7).unwrap();
        assert!(fs::symlink_metadata(xdg_dir(&dirs).join("config/git")).is_err());
        assert!(!stale.exists() && live.exists());
    }

    #[test]
    fn open_folders_are_known_while_their_platoon_is_there() {
        let d = tmp("marks");
        let mine = mark_open(&d, std::process::id(), &["/a/ticket-1".into()]).unwrap();
        // A platoon that is gone (a pid no process has): its mark is tidied away.
        let gone = mark_open(&d, 0x3fff_fff0, &["/b".into()]).unwrap();
        assert_eq!(open_now(&d), vec![PathBuf::from("/a/ticket-1")]);
        assert!(!gone.exists());
        unmark(&mine);
        assert!(open_now(&d).is_empty());
    }
}
