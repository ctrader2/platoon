//! The optional code editor's install engine: getting Neovim, its tools, its plugins, syntax highlighting and a Nerd
//! Font onto the machine, and taking them off again. Nothing here runs the editor (that is the app's `v`).
//!
//! What to fetch is not platoon's: it is the download table (`downloads.json`) of the editor plugin (`plugins/editor`
//! in the catalog; `editorplugin.rs` finds it), read with `parse_table` and refused unless `table_problems` is empty:
//! every download https, with its sha256 and its exact size. This file is the engine that installs from a table.
//!
//! Everything platoon installs lives in one folder of its own, `<data>/editor` (`data_dir`), laid out so an nvim
//! started with platoon's own config finds it all without touching the user's `~/.config/nvim` or nvim data:
//!
//! ```text
//! <data>/editor/
//!   bin/nvim, bin/rg, bin/fd, bin/tree-sitter   links: to the user's own copy on PATH, or to one under tools/
//!   tools/<id>/...                              downloaded releases (tools/nvim/bin/nvim, tools/rg/rg, ...)
//!   site/                                       put first on 'packpath' and 'runtimepath'
//!     pack/platoon/start/<plugin>/              the pinned plugins, as native packages (no plugin manager)
//!     parser/<lang>.so                          syntax highlighting, compiled here with the C compiler
//!     parser-info/<lang>.revision               what nvim-treesitter reads to know a parser is current
//!     queries/<lang> -> ../pack/platoon/start/nvim-treesitter/runtime/queries/<lang>
//!   installed.json                              what is installed: versions, hashes, where each tool came from
//!   install.lock, tmp/                          one install at a time; downloads are checked here, then moved
//! ```
//!
//! `site/` is exactly nvim-treesitter's `install_dir`, so its own `install()` (phase 3, for languages not built up
//! front) and `:checkhealth` see the parsers built here as installed and up to date.
//!
//! Every download is pinned by sha256 and size and checked before anything is moved into place, so a failed or interrupted
//! install never leaves a part that looks complete; `installed.json` only lists what was checked and moved. A second
//! run only fetches what is missing or has a different pin. Downloads use `curl`, archives `tar` (and `gzip` for the
//! one single-file download); nothing needs sudo or a package manager.
//!
//! Knobs: `PLATOON_DATA_DIR` (instead of the platform's data folder), `PLATOON_FONT_DIR` (where the font goes),
//! `PLATOON_EDITOR_MIRROR=<dir>` (every download is read from `<dir>/<last part of its URL>`, never the network),
//! `PLATOON_SKIP_FC_CACHE=1` (don't refresh the font cache).

use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    fs, io,
    os::unix::{fs::PermissionsExt, io::AsRawFd},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

// ---- the pins -----------------------------------------------------------------------------------------------------

/// The computers there are downloads for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    LinuxX64,
    LinuxArm64,
    MacArm64,
    MacX64,
}

impl Platform {
    #[cfg(test)]
    pub const ALL: [Platform; 4] = [Platform::LinuxX64, Platform::LinuxArm64, Platform::MacArm64, Platform::MacX64];

    /// This computer, or None when there are no downloads for it.
    pub fn current() -> Option<Platform> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Some(Platform::LinuxX64),
            ("linux", "aarch64") => Some(Platform::LinuxArm64),
            ("macos", "aarch64") => Some(Platform::MacArm64),
            ("macos", "x86_64") => Some(Platform::MacX64),
            _ => None,
        }
    }

    pub fn is_mac(self) -> bool {
        matches!(self, Platform::MacArm64 | Platform::MacX64)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Download {
    pub url: String,
    pub sha256: String,
    /// Its exact size in bytes: a download of another size is refused, and the sizes add up to what the install
    /// fetches.
    pub size: u64,
}

/// A program the editor needs: the user's own copy when it is on PATH (and new enough), else a pinned release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolPin {
    /// Its name under `bin/` and its key in `installed.json`.
    pub id: String,
    pub version: String,
    /// The user's own copy is used only from this version on (None: any version).
    pub min_yours: Option<[u32; 3]>,
    /// The names it goes by on PATH (`fd` is `fdfind` on Debian and Ubuntu).
    pub names: Vec<String>,
    /// The program inside the unpacked release.
    pub exe: String,
    /// The download is the program itself, gzipped (tree-sitter), not a tar archive.
    pub single_gz: bool,
    pub assets: Vec<(Platform, Download)>,
}

/// A plugin: a GitHub repository at a pinned commit, installed as a native package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginPin {
    /// Its folder under `pack/platoon/start/`.
    pub name: String,
    pub repo: String,
    pub rev: String,
    /// The commit's tarball (`https://codeload.github.com/<repo>/tar.gz/<rev>`).
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// A syntax-highlighting parser, as the pinned nvim-treesitter's `parsers.lua` describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParserPin {
    pub lang: String,
    pub repo: String,
    /// Exactly the `revision` in `parsers.lua` (a commit, or a tag like `v0.25.0`): written to
    /// `parser-info/<lang>.revision`, which is how nvim-treesitter tells an installed parser is current.
    pub rev: String,
    /// The grammar's folder inside the repository ("" for the top).
    pub location: String,
    /// The commit's tarball.
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// Languages with queries only (no parser) this one's queries build on (`ecma`, `jsx`, `html_tags`).
    pub requires: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontPin {
    /// The family name a terminal lists it under ("JetBrainsMono Nerd Font").
    pub family: String,
    pub version: String,
    pub download: Download,
    /// The files taken from the archive: one family's four styles, and its licence.
    pub files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub tools: Vec<ToolPin>,
    pub plugins: Vec<PluginPin>,
    pub parsers: Vec<ParserPin>,
    pub font: FontPin,
}

// ---- the download table (a plugin's `downloads.json`) --------------------------------------------------------------
//
// Everything above is what platoon's editor plugin carries as data: which programs, plugins, parsers and font to fetch,
// each with where from (https only), its sha256 and its exact size. platoon itself pins nothing; it installs what
// the installed editor plugin's table says (`editorplugin.rs`), and refuses a table that isn't pinned all the way.

/// A table's whole download, on its biggest computer, is at most this (the real one is under 100 MB).
pub const MAX_TABLE_BYTES: u64 = 600_000_000;
/// One download at most.
const MAX_DOWNLOAD_BYTES: u64 = 300_000_000;
/// Entries of each kind at most.
const MAX_ENTRIES: usize = 200;

impl Platform {
    /// Its key in a download table.
    pub fn key(self) -> &'static str {
        match self {
            Platform::LinuxX64 => "linux-x64",
            Platform::LinuxArm64 => "linux-arm64",
            Platform::MacArm64 => "macos-arm64",
            Platform::MacX64 => "macos-x64",
        }
    }

    fn from_key(k: &str) -> Option<Platform> {
        [Platform::LinuxX64, Platform::LinuxArm64, Platform::MacArm64, Platform::MacX64].into_iter().find(|p| p.key() == k)
    }
}

#[cfg(test)]
fn version_text(v: [u32; 3]) -> String {
    format!("{}.{}.{}", v[0], v[1], v[2])
}

#[cfg(test)]
fn download_json(d: &Download) -> Value {
    json!({"url": d.url, "sha256": d.sha256, "size": d.size})
}

/// A table as `downloads.json` holds it (tests write their fake tables with it).
#[cfg(test)]
pub fn table_json(m: &Manifest) -> Value {
    let tools: Vec<Value> = m
        .tools
        .iter()
        .map(|t| {
            let downloads: Map<String, Value> = t.assets.iter().map(|(p, d)| (p.key().to_string(), download_json(d))).collect();
            json!({"id": t.id, "version": t.version, "min_yours": t.min_yours.map(version_text), "names": t.names, "exe": t.exe, "single_gz": t.single_gz, "downloads": downloads})
        })
        .collect();
    let plugins: Vec<Value> = m.plugins.iter().map(|p| json!({"name": p.name, "repo": p.repo, "rev": p.rev, "url": p.url, "sha256": p.sha256, "size": p.size})).collect();
    let parsers: Vec<Value> = m
        .parsers
        .iter()
        .map(|p| json!({"lang": p.lang, "repo": p.repo, "rev": p.rev, "location": p.location, "requires": p.requires, "url": p.url, "sha256": p.sha256, "size": p.size}))
        .collect();
    let f = &m.font;
    json!({"format": 1, "tools": tools, "plugins": plugins, "parsers": parsers,
        "font": {"family": f.family, "version": f.version, "url": f.download.url, "sha256": f.download.sha256, "size": f.download.size, "files": f.files}})
}

/// Read a download table (`downloads.json`). Keys it doesn't know (`about`, `note`) are left alone. Only the shape is
/// checked here; `table_problems` says whether it is pinned well enough to install.
pub fn parse_table(text: &str) -> Result<Manifest, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("the download table isn't valid JSON ({e})"))?;
    let obj = v.as_object().ok_or("the download table must be a JSON object")?;
    if let Some(f) = obj.get("format") {
        if f.as_u64() != Some(1) {
            return Err("the download table is in a format this platoon doesn't know (it reads format 1)".into());
        }
    }
    fn s(v: &Value, k: &str, what: &str) -> Result<String, String> {
        match v.get(k) {
            Some(Value::String(x)) => Ok(x.clone()),
            None | Some(Value::Null) => Err(format!("{what} has no \"{k}\"")),
            Some(_) => Err(format!("{what}: \"{k}\" must be text")),
        }
    }
    fn opt_s(v: &Value, k: &str, what: &str) -> Result<String, String> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(String::new()),
            _ => s(v, k, what),
        }
    }
    fn texts(v: &Value, k: &str, what: &str) -> Result<Vec<String>, String> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(a)) => a.iter().map(|x| x.as_str().map(String::from).ok_or_else(|| format!("{what}: \"{k}\" must be a list of text"))).collect(),
            Some(_) => Err(format!("{what}: \"{k}\" must be a list of text")),
        }
    }
    fn size(v: &Value, what: &str) -> Result<u64, String> {
        match v.get("size") {
            Some(n) => n.as_u64().ok_or_else(|| format!("{what}: \"size\" must be a whole number of bytes")),
            None => Err(format!("{what} has no \"size\"")),
        }
    }
    fn download(v: &Value, what: &str) -> Result<Download, String> {
        Ok(Download { url: s(v, "url", what)?, sha256: s(v, "sha256", what)?, size: size(v, what)? })
    }
    fn list<'a>(obj: &'a Map<String, Value>, k: &str) -> Result<&'a [Value], String> {
        match obj.get(k) {
            Some(Value::Array(a)) if a.len() <= MAX_ENTRIES => Ok(a),
            Some(Value::Array(_)) => Err(format!("the download table has more than {MAX_ENTRIES} \"{k}\"")),
            None => Ok(&[]),
            Some(_) => Err(format!("the download table's \"{k}\" must be a list")),
        }
    }
    let mut tools = Vec::new();
    for (i, t) in list(obj, "tools")?.iter().enumerate() {
        let what = format!("tool {}", t.get("id").and_then(Value::as_str).map_or_else(|| format!("#{}", i + 1), String::from));
        let min_yours = match t.get("min_yours") {
            None | Some(Value::Null) => None,
            Some(Value::String(m)) => Some(parse_version(m).ok_or_else(|| format!("{what}: \"min_yours\" isn't a version like 0.12.0"))?),
            Some(_) => return Err(format!("{what}: \"min_yours\" must be a version like 0.12.0")),
        };
        let mut assets = Vec::new();
        match t.get("downloads") {
            Some(Value::Object(d)) => {
                for (k, dv) in d {
                    let p = Platform::from_key(k).ok_or_else(|| format!("{what}: \"{k}\" isn't a computer platoon knows (linux-x64, linux-arm64, macos-arm64, macos-x64)"))?;
                    assets.push((p, download(dv, &format!("{what} for {k}"))?));
                }
            }
            _ => return Err(format!("{what} has no \"downloads\" (one per computer)")),
        }
        tools.push(ToolPin {
            id: s(t, "id", &what)?,
            version: s(t, "version", &what)?,
            min_yours,
            names: texts(t, "names", &what)?,
            exe: s(t, "exe", &what)?,
            single_gz: t.get("single_gz").and_then(Value::as_bool).unwrap_or(false),
            assets,
        });
    }
    let mut plugins = Vec::new();
    for (i, p) in list(obj, "plugins")?.iter().enumerate() {
        let what = format!("plugin {}", p.get("name").and_then(Value::as_str).map_or_else(|| format!("#{}", i + 1), String::from));
        let d = download(p, &what)?;
        plugins.push(PluginPin { name: s(p, "name", &what)?, repo: opt_s(p, "repo", &what)?, rev: s(p, "rev", &what)?, url: d.url, sha256: d.sha256, size: d.size });
    }
    let mut parsers = Vec::new();
    for (i, p) in list(obj, "parsers")?.iter().enumerate() {
        let what = format!("parser {}", p.get("lang").and_then(Value::as_str).map_or_else(|| format!("#{}", i + 1), String::from));
        let d = download(p, &what)?;
        parsers.push(ParserPin {
            lang: s(p, "lang", &what)?,
            repo: opt_s(p, "repo", &what)?,
            rev: s(p, "rev", &what)?,
            location: opt_s(p, "location", &what)?,
            url: d.url,
            sha256: d.sha256,
            size: d.size,
            requires: texts(p, "requires", &what)?,
        });
    }
    let f = obj.get("font").ok_or("the download table has no \"font\"")?;
    let font = FontPin { family: s(f, "family", "the font")?, version: s(f, "version", "the font")?, download: download(f, "the font")?, files: texts(f, "files", "the font")? };
    Ok(Manifest { tools, plugins, parsers, font })
}

/// A name platoon makes a folder or a file of (a tool's id, a plugin's folder, a language): letters, digits, `.`,
/// `_` and `-`, not starting with a dot.
fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && !s.starts_with('.') && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A path inside an unpacked download: relative, no `..`, no hidden parts.
fn inside(s: &str) -> bool {
    !s.is_empty() && s.len() <= 200 && !s.starts_with('/') && s.split('/').all(|p| !p.is_empty() && p != "." && p != ".." && !p.starts_with('.') && !p.contains('\\'))
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// What the table fetches in all on `platform`, in bytes (a download used twice, like one grammar's two languages, counts
/// once).
pub fn download_bytes(m: &Manifest, platform: Platform) -> u64 {
    let mut all: Vec<(&str, u64)> = Vec::new();
    for t in &m.tools {
        if let Some((_, d)) = t.assets.iter().find(|(p, _)| *p == platform) {
            all.push((&d.sha256, d.size));
        }
    }
    all.extend(m.plugins.iter().map(|p| (p.sha256.as_str(), p.size)));
    all.extend(m.parsers.iter().map(|p| (p.sha256.as_str(), p.size)));
    all.push((&m.font.download.sha256, m.font.download.size));
    all.sort();
    all.dedup_by(|a, b| a.0 == b.0);
    all.iter().map(|(_, n)| n).sum()
}

/// What is wrong with one download's pin: an https link ending in a plain file name, a sha256, a size.
fn download_problems(what: &str, d: &Download) -> Vec<String> {
    let mut out = Vec::new();
    if !d.url.starts_with("https://") || d.url.contains(char::is_whitespace) || d.url.len() > 400 {
        out.push(format!("{what}: its address must be an https:// link"));
    } else if d.url.rsplit('/').next().is_none_or(|b| !plain_name(b)) {
        out.push(format!("{what}: its address must end in a plain file name"));
    }
    if !hex64(&d.sha256) {
        out.push(format!("{what}: it needs its sha256 (64 lowercase hex digits), so a changed file is never used"));
    }
    if d.size == 0 {
        out.push(format!("{what}: it needs its size in bytes"));
    } else if d.size > MAX_DOWNLOAD_BYTES {
        out.push(format!("{what}: {} is more than one download may be ({})", megabytes(d.size), megabytes(MAX_DOWNLOAD_BYTES)));
    }
    out
}

/// Why a table can't be installed from, in plain words (empty: it can). Every download is https, has a sha256 and
/// its size, none too big, all of them together under `MAX_TABLE_BYTES`; every name platoon makes a folder or a file
/// of is a plain name and every path stays inside its folder; nvim is there (the editor is nothing without it).
pub fn table_problems(m: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    if !m.tools.iter().any(|t| t.id == "nvim") {
        out.push("it has no tool \"nvim\": the editor is Neovim".into());
    }
    for t in &m.tools {
        if t.assets.is_empty() {
            out.push(format!("tool {} has no downloads", t.id));
        }
        for (p, d) in &t.assets {
            out.extend(download_problems(&format!("tool {} for {}", t.id, p.key()), d));
        }
    }
    for p in &m.plugins {
        out.extend(download_problems(&format!("plugin {}", p.name), &Download { url: p.url.clone(), sha256: p.sha256.clone(), size: p.size }));
    }
    for p in &m.parsers {
        out.extend(download_problems(&format!("parser {}", p.lang), &Download { url: p.url.clone(), sha256: p.sha256.clone(), size: p.size }));
    }
    out.extend(download_problems("the font", &m.font.download));
    for t in &m.tools {
        if !plain_name(&t.id) {
            out.push(format!("the tool id \"{}\" isn't a plain name", t.id));
        }
        if !inside(&t.exe) {
            out.push(format!("tool {}: \"exe\" must be a path inside its download", t.id));
        }
        if t.names.is_empty() || t.names.iter().any(|n| !plain_name(n)) {
            out.push(format!("tool {}: \"names\" must be the plain program names it goes by", t.id));
        }
    }
    for p in &m.plugins {
        if !plain_name(&p.name) {
            out.push(format!("the plugin name \"{}\" isn't a plain name", p.name));
        }
    }
    for p in &m.parsers {
        if !plain_name(&p.lang) || p.requires.iter().any(|r| !plain_name(r)) {
            out.push(format!("parser {}: its language names must be plain names", p.lang));
        }
        if !p.location.is_empty() && !inside(&p.location) {
            out.push(format!("parser {}: \"location\" must be a folder inside its download", p.lang));
        }
        if !plain_name(&p.rev) {
            out.push(format!("parser {}: \"rev\" must be a commit or a tag", p.lang));
        }
    }
    if m.font.files.is_empty() || m.font.files.iter().any(|f| !plain_name(f)) {
        out.push("the font: \"files\" must be plain file names in its archive".into());
    }
    let mut ids: Vec<String> = m.tools.iter().map(|t| format!("tool {}", t.id)).chain(m.plugins.iter().map(|p| format!("plugin {}", p.name))).chain(m.parsers.iter().map(|p| format!("parser {}", p.lang))).collect();
    ids.sort();
    for w in ids.windows(2) {
        if w[0] == w[1] {
            out.push(format!("{} is listed twice", w[0]));
        }
    }
    for p in [Platform::LinuxX64, Platform::LinuxArm64, Platform::MacArm64, Platform::MacX64] {
        let total = download_bytes(m, p);
        if total > MAX_TABLE_BYTES {
            out.push(format!("on {} it downloads {}, more than the {} a plugin may", p.key(), megabytes(total), megabytes(MAX_TABLE_BYTES)));
        }
    }
    out.dedup();
    out
}

// ---- where things go ------------------------------------------------------------------------------------------------

/// Where platoon keeps what it installs: `$PLATOON_DATA_DIR`, else on macOS `~/Library/Application Support/platoon`,
/// else `$XDG_DATA_HOME/platoon`, else `~/.local/share/platoon`.
pub fn data_dir() -> PathBuf {
    data_dir_from(&|k| std::env::var(k).ok(), cfg!(target_os = "macos"))
}

fn data_dir_from(env: &dyn Fn(&str) -> Option<String>, mac: bool) -> PathBuf {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(d) = set("PLATOON_DATA_DIR") {
        return PathBuf::from(d);
    }
    let home = PathBuf::from(set("HOME").unwrap_or_default());
    if mac {
        return home.join("Library/Application Support/platoon");
    }
    set("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("platoon")
}

/// Where the font goes: `$PLATOON_FONT_DIR`, else on macOS `~/Library/Fonts` (it doesn't look in folders below it),
/// else `$XDG_DATA_HOME/fonts/platoon` (`~/.local/share/fonts/platoon`), a folder of platoon's own.
fn font_dir_from(env: &dyn Fn(&str) -> Option<String>, mac: bool) -> PathBuf {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(d) = set("PLATOON_FONT_DIR") {
        return PathBuf::from(d);
    }
    let home = PathBuf::from(set("HOME").unwrap_or_default());
    if mac {
        return home.join("Library/Fonts");
    }
    set("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("fonts/platoon")
}

/// Everything an install, a status check or a removal needs to know about this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirs {
    /// `<data>/editor`.
    pub editor: PathBuf,
    pub fonts: PathBuf,
    /// Read downloads from here instead of the network (`PLATOON_EDITOR_MIRROR`).
    pub mirror: Option<PathBuf>,
    /// Where to look for the user's own nvim, rg, fd, tree-sitter and a C compiler (PATH).
    pub search_path: Vec<PathBuf>,
    /// Refresh the font cache for `fonts` after installing the font (Linux only; off with `PLATOON_SKIP_FC_CACHE`).
    pub refresh_fonts: bool,
    pub platform: Option<Platform>,
}

impl Dirs {
    pub fn from_env() -> Dirs {
        let env = |k: &str| std::env::var(k).ok();
        let mac = cfg!(target_os = "macos");
        Dirs {
            editor: data_dir().join("editor"),
            fonts: font_dir_from(&env, mac),
            mirror: env("PLATOON_EDITOR_MIRROR").filter(|v| !v.is_empty()).map(PathBuf::from),
            search_path: env("PATH").unwrap_or_default().split(':').filter(|p| !p.is_empty()).map(PathBuf::from).collect(),
            refresh_fonts: !mac && env("PLATOON_SKIP_FC_CACHE").is_none_or(|v| v.is_empty() || v == "0"),
            platform: Platform::current(),
        }
    }

    pub fn bin(&self) -> PathBuf {
        self.editor.join("bin")
    }
    /// What goes first on nvim's 'packpath' and 'runtimepath', and nvim-treesitter's `install_dir`.
    pub fn site(&self) -> PathBuf {
        self.editor.join("site")
    }
    pub fn plugins(&self) -> PathBuf {
        self.site().join("pack/platoon/start")
    }
    fn tools(&self) -> PathBuf {
        self.editor.join("tools")
    }
    fn tmp(&self) -> PathBuf {
        self.editor.join("tmp")
    }
    fn record_path(&self) -> PathBuf {
        self.editor.join("installed.json")
    }
    /// The nvim to run: platoon's link to the user's own or to the downloaded one. None until installed.
    pub fn nvim(&self) -> Option<PathBuf> {
        let p = self.bin().join("nvim");
        runnable(&p).then_some(p)
    }
}

fn runnable(p: &Path) -> bool {
    fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// The first of `names` on the search path, leaving out platoon's own folder (a PATH that holds `<editor>/bin`
/// mustn't make platoon's copy look like the user's).
fn find_on_path(dirs: &Dirs, names: &[String]) -> Option<PathBuf> {
    for name in names {
        for d in &dirs.search_path {
            if d.starts_with(&dirs.editor) {
                continue;
            }
            let p = d.join(name);
            if runnable(&p) {
                return Some(p);
            }
        }
    }
    None
}

/// The first `x.y[.z]` in a program's `--version` output: "NVIM v0.12.5" -> [0, 12, 5].
fn parse_version(text: &str) -> Option<[u32; 3]> {
    let first = text.lines().next()?;
    for word in first.split_whitespace() {
        let w = word.trim_start_matches('v');
        let parts: Vec<&str> = w.split(['.', '-', '+']).collect();
        if parts.len() >= 2 && parts[0].chars().all(|c| c.is_ascii_digit()) && !parts[0].is_empty() {
            let n = |i: usize| parts.get(i).and_then(|p| p.parse::<u32>().ok());
            if let (Some(a), Some(b)) = (n(0), n(1)) {
                return Some([a, b, n(2).unwrap_or(0)]);
            }
        }
    }
    None
}

fn version_of(program: &Path) -> Option<[u32; 3]> {
    let out = Command::new(program).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    parse_version(&String::from_utf8_lossy(&out.stdout))
}

/// The user's own copy of a tool, when there is one this editor can use.
fn yours(dirs: &Dirs, tool: &ToolPin) -> Option<PathBuf> {
    let p = find_on_path(dirs, &tool.names)?;
    match tool.min_yours {
        None => Some(p),
        Some(min) => version_of(&p).filter(|v| *v >= min).map(|_| p),
    }
}

/// A parser platoon didn't build up front can be built when a file of its language is first opened: the tree-sitter
/// CLI is in platoon's folder and there is a C compiler.
pub fn can_build_parsers(dirs: &Dirs) -> bool {
    runnable(&dirs.bin().join("tree-sitter")) && compiler(dirs).is_some()
}

/// The C compiler parsers are built with: `cc`, else `gcc`, else `clang`, on the search path.
fn compiler(dirs: &Dirs) -> Option<PathBuf> {
    find_on_path(dirs, &["cc".into(), "gcc".into(), "clang".into()])
}

// ---- installed.json -------------------------------------------------------------------------------------------------
//
// `{"version": 1, "items": {"tool:nvim": {...}, "plugin:<name>": {...}, "parser:<lang>": {...}, "font": {...}},
//   "no_compiler": true}`. An item is listed only once its files were checked and moved into place.

fn load_record(dirs: &Dirs) -> Map<String, Value> {
    fs::read_to_string(dirs.record_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("items").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}

fn record_flag(dirs: &Dirs, key: &str) -> bool {
    fs::read_to_string(dirs.record_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get(key).and_then(Value::as_bool))
        .unwrap_or(false)
}

fn save_record(dirs: &Dirs, items: &Map<String, Value>, no_compiler: bool) -> Result<(), String> {
    let mut v = json!({"version": 1, "items": items});
    if no_compiler {
        if let Some(o) = v.as_object_mut() {
            o.insert("no_compiler".into(), Value::Bool(true));
        }
    }
    let path = dirs.record_path();
    crate::projects::write_atomic(&path, &(serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"))
        .map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

// ---- status ---------------------------------------------------------------------------------------------------------

/// One part of the install, as the progress lines and the status name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Part {
    Neovim,
    Tools,
    Plugins,
    Highlighting,
    Font,
}

impl Part {
    pub const ALL: [Part; 5] = [Part::Neovim, Part::Tools, Part::Plugins, Part::Highlighting, Part::Font];

    pub fn label(self) -> &'static str {
        match self {
            Part::Neovim => "Neovim",
            Part::Tools => "Tools",
            Part::Plugins => "Plugins",
            Part::Highlighting => "Syntax highlighting",
            Part::Font => "Font",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemState {
    Ok,
    Missing,
    /// Installed, but platoon now pins something else.
    OutOfDate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub part: Part,
    /// `tool:nvim`, `plugin:neo-tree.nvim`, `parser:lua`, `font`.
    pub key: String,
    pub name: String,
    pub state: ItemState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub items: Vec<Item>,
    /// The last install found no C compiler, so no parsers were built (highlighting is what nvim ships with).
    pub no_compiler: bool,
    /// No downloads exist for this computer.
    pub unsupported: bool,
}

impl Status {
    /// Nothing of the editor is there at all.
    #[cfg(test)]
    pub fn nothing(&self) -> bool {
        self.items.iter().all(|i| i.state == ItemState::Missing)
    }

    /// Everything is there and current. Parsers don't count when the last install had no C compiler.
    pub fn complete(&self) -> bool {
        !self.unsupported && self.items.iter().all(|i| i.state == ItemState::Ok || (self.no_compiler && i.part == Part::Highlighting))
    }

    /// What is missing or out of date, by name.
    #[cfg(test)]
    pub fn needs(&self) -> Vec<&str> {
        self.items.iter().filter(|i| i.state != ItemState::Ok && !(self.no_compiler && i.part == Part::Highlighting)).map(|i| i.name.as_str()).collect()
    }
}

fn tool_ok(dirs: &Dirs, tool: &ToolPin, rec: &Value, platform: Platform) -> ItemState {
    let link = dirs.bin().join(&tool.id);
    if !runnable(&link) {
        return ItemState::Missing;
    }
    match str_of(rec, "source") {
        "yours" => ItemState::Ok,
        "download" => {
            let want = tool.assets.iter().find(|(p, _)| *p == platform).map(|(_, d)| d.sha256.as_str()).unwrap_or("");
            if str_of(rec, "sha256") == want && runnable(&dirs.tools().join(&tool.id).join(&tool.exe)) { ItemState::Ok } else { ItemState::OutOfDate }
        }
        _ => ItemState::Missing,
    }
}

pub fn status(dirs: &Dirs, m: &Manifest) -> Status {
    let rec = load_record(dirs);
    let get = |k: &str| rec.get(k).cloned();
    let mut items = Vec::new();
    let platform = dirs.platform;
    for t in &m.tools {
        let key = format!("tool:{}", t.id);
        let state = match (get(&key), platform) {
            (Some(r), Some(p)) => tool_ok(dirs, t, &r, p),
            _ => ItemState::Missing,
        };
        let part = if t.id == "nvim" { Part::Neovim } else { Part::Tools };
        items.push(Item { part, key, name: t.id.clone(), state });
    }
    for p in &m.plugins {
        let key = format!("plugin:{}", p.name);
        let there = dirs.plugins().join(&p.name).is_dir();
        let state = match get(&key) {
            Some(r) if there && str_of(&r, "rev") == p.rev && str_of(&r, "sha256") == p.sha256 => ItemState::Ok,
            Some(_) if there => ItemState::OutOfDate,
            _ => ItemState::Missing,
        };
        items.push(Item { part: Part::Plugins, key, name: p.name.clone(), state });
    }
    for p in &m.parsers {
        let key = format!("parser:{}", p.lang);
        let there = dirs.site().join("parser").join(format!("{}.so", p.lang)).is_file();
        let state = match get(&key) {
            Some(r) if there && str_of(&r, "rev") == p.rev && str_of(&r, "sha256") == p.sha256 => ItemState::Ok,
            Some(_) if there => ItemState::OutOfDate,
            _ => ItemState::Missing,
        };
        items.push(Item { part: Part::Highlighting, key, name: format!("{} highlighting", p.lang), state });
    }
    let font_state = match get("font") {
        Some(r) => {
            let dir = PathBuf::from(str_of(&r, "dir"));
            let files: Vec<String> = r.get("files").and_then(Value::as_array).map(|a| a.iter().filter_map(|f| f.as_str().map(String::from)).collect()).unwrap_or_default();
            let there = !files.is_empty() && files.iter().all(|f| dir.join(f).is_file());
            if !there {
                ItemState::Missing
            } else if str_of(&r, "sha256") == m.font.download.sha256 && dir == dirs.fonts {
                ItemState::Ok
            } else {
                ItemState::OutOfDate
            }
        }
        None => ItemState::Missing,
    };
    items.push(Item { part: Part::Font, key: "font".into(), name: "font".into(), state: font_state });
    Status { items, no_compiler: record_flag(dirs, "no_compiler"), unsupported: platform.is_none() }
}

/// A Nerd Font is there for the editor's icons: the one platoon installed, or (Linux) any the font list knows, or
/// (macOS) any in the user's or the system's font folder. Phase 3 uses plain icons without one.
pub fn nerd_font_present(dirs: &Dirs) -> bool {
    nerd_font_present_with(dirs, &system_has_nerd_font)
}

fn nerd_font_present_with(dirs: &Dirs, system: &dyn Fn() -> bool) -> bool {
    let rec = load_record(dirs);
    let ours = rec.get("font").is_some_and(|r| {
        let dir = PathBuf::from(str_of(r, "dir"));
        r.get("files").and_then(Value::as_array).is_some_and(|a| a.iter().filter_map(Value::as_str).any(|f| f.ends_with(".ttf") && dir.join(f).is_file()))
    });
    ours || system()
}

fn system_has_nerd_font() -> bool {
    if cfg!(target_os = "macos") {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        return [home.join("Library/Fonts"), PathBuf::from("/Library/Fonts")].iter().any(|d| {
            fs::read_dir(d).is_ok_and(|rd| rd.flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                n.contains("nerdfont") || n.contains("nerd font")
            }))
        });
    }
    Command::new("fc-list")
        .args([":", "family"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("Nerd Font"))
}

// ---- install --------------------------------------------------------------------------------------------------------

/// How far one part has got: "Plugins 9/11".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub part: Part,
    pub done: usize,
    pub total: usize,
    pub failed: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub took: Duration,
    /// What platoon's editor folder and the font take on disk now.
    pub disk_bytes: u64,
    /// How many things were downloaded (0 on a run where everything was already there).
    pub fetched: usize,
    /// The user's own programs used instead of downloads: "nvim (/usr/bin/nvim)".
    pub yours: Vec<String>,
    /// Said after the summary: no C compiler, parsers that didn't build.
    pub notes: Vec<String>,
    /// The Nerd Font is in place (installed now or before).
    pub font: bool,
    /// The family name to pick it by in the terminal.
    pub font_family: String,
}

enum Job<'a> {
    Tool(&'a ToolPin),
    Plugin(&'a PluginPin),
    Parser(&'a ParserPin),
    Font,
}

impl Job<'_> {
    fn part(&self) -> Part {
        match self {
            Job::Tool(t) if t.id == "nvim" => Part::Neovim,
            Job::Tool(_) => Part::Tools,
            Job::Plugin(_) => Part::Plugins,
            Job::Parser(_) => Part::Highlighting,
            Job::Font => Part::Font,
        }
    }
}

/// What a finished job adds to the record.
struct Done {
    key: String,
    record: Value,
    fetched: bool,
    yours: Option<String>,
}

/// One install at a time (two platoon windows could both start one): an exclusive lock on `install.lock`.
struct Lock(#[allow(dead_code)] fs::File);

fn lock(dirs: &Dirs) -> Result<Lock, String> {
    fs::create_dir_all(&dirs.editor).map_err(|e| format!("couldn't make {}: {e}", dirs.editor.display()))?;
    let f = fs::OpenOptions::new().create(true).truncate(false).write(true).open(dirs.editor.join("install.lock")).map_err(|e| e.to_string())?;
    // A lock just let go can look held for an instant: an flock stays until every copy of the descriptor is closed,
    // and a process another thread is starting holds a copy until its `exec`. So "held" only after a short wait; a
    // real install elsewhere holds it for many seconds.
    let start = Instant::now();
    loop {
        // SAFETY: flock on a descriptor this function owns; the lock goes with the file when `Lock` is dropped.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Lock(f));
        }
        if start.elapsed() > Duration::from_millis(1500) {
            return Err("The code editor is being installed or removed by another platoon right now; try again when it has finished.".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn install(dirs: &Dirs, m: &Manifest, progress: &(dyn Fn(Progress) + Sync)) -> Result<Report, String> {
    let start = Instant::now();
    let Some(platform) = dirs.platform else {
        return Err("The code editor can't be installed on this computer: platoon only has downloads for Linux and macOS, on x86_64 and ARM.".into());
    };
    let _lock = lock(dirs)?;
    // Whatever an interrupted install left half done goes; nothing in it was ever recorded.
    let _ = fs::remove_dir_all(dirs.tmp());
    fs::create_dir_all(dirs.tmp()).map_err(|e| format!("couldn't make {}: {e}", dirs.tmp().display()))?;
    let before = status(dirs, m);
    let ok = |key: &str| before.items.iter().any(|i| i.key == key && i.state == ItemState::Ok);
    let cc = compiler(dirs);

    let mut all: Vec<(Job, bool)> = Vec::new();
    all.extend(m.tools.iter().map(|t| (Job::Tool(t), ok(&format!("tool:{}", t.id)))));
    all.extend(m.plugins.iter().map(|p| (Job::Plugin(p), ok(&format!("plugin:{}", p.name)))));
    if cc.is_some() {
        all.extend(m.parsers.iter().map(|p| (Job::Parser(p), ok(&format!("parser:{}", p.lang)))));
    }
    all.push((Job::Font, ok("font")));
    let mut jobs: Vec<Job> = Vec::new();
    let mut totals: BTreeMap<Part, usize> = BTreeMap::new();
    let mut done: BTreeMap<Part, (usize, usize)> = BTreeMap::new();
    for (job, already) in all {
        let part = job.part();
        *totals.entry(part).or_default() += 1;
        if already {
            done.entry(part).or_default().0 += 1;
        } else {
            jobs.push(job);
        }
    }
    for part in Part::ALL {
        if let Some(&total) = totals.get(&part) {
            let (d, f) = done.get(&part).copied().unwrap_or_default();
            progress(Progress { part, done: d, total, failed: f });
        }
    }

    // Big downloads first, so the slowest one starts at once.
    jobs.sort_by_key(|j| match j {
        Job::Tool(t) if t.id == "nvim" => 0,
        Job::Font => 1,
        Job::Tool(_) => 2,
        Job::Plugin(_) => 3,
        Job::Parser(_) => 4,
    });
    let queue = Mutex::new(jobs.into_iter().collect::<VecDeque<_>>());
    let counts = Mutex::new(done);
    let results: Mutex<Vec<Result<Done, (Part, String)>>> = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(4, 12);
    std::thread::scope(|s| {
        for w in 0..workers {
            let (queue, counts, results, totals, cc) = (&queue, &counts, &results, &totals, &cc);
            s.spawn(move || {
                let mut n = 0;
                loop {
                    let Some(job) = queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front() else { break };
                    n += 1;
                    let part = job.part();
                    let work = dirs.tmp().join(format!("w{w}-{n}"));
                    let r = run_job(dirs, m, platform, &job, &work, cc.as_deref()).map_err(|e| (part, e));
                    let _ = fs::remove_dir_all(&work);
                    let failed = r.is_err();
                    results.lock().unwrap_or_else(|e| e.into_inner()).push(r);
                    let p = {
                        let mut c = counts.lock().unwrap_or_else(|e| e.into_inner());
                        let e = c.entry(part).or_default();
                        if failed { e.1 += 1 } else { e.0 += 1 }
                        Progress { part, done: e.0, total: totals.get(&part).copied().unwrap_or(0), failed: e.1 }
                    };
                    progress(p);
                }
            });
        }
    });

    // The record: what was already right, plus everything that finished now.
    let mut items = load_record(dirs);
    let mut report = Report::default();
    let mut failures: Vec<(Part, String)> = Vec::new();
    for r in results.into_inner().unwrap_or_else(|e| e.into_inner()) {
        match r {
            Ok(d) => {
                items.insert(d.key, d.record);
                report.fetched += usize::from(d.fetched);
                report.yours.extend(d.yours);
            }
            Err(f) => failures.push(f),
        }
    }
    // Nothing platoon no longer pins stays around (a plugin dropped from the list, a parser for an old language).
    tidy(dirs, m, &mut items);
    save_record(dirs, &items, cc.is_none())?;
    let _ = fs::remove_dir_all(dirs.tmp());

    report.yours.sort();
    if cc.is_none() {
        report.notes.push("No C compiler was found, so syntax highlighting is limited to the few languages Neovim comes with. Install one (gcc or clang) and run `platoon plugin add editor` again for the rest.".into());
    }
    // A parser that didn't build doesn't stop the editor: it only means plainer colors for that language.
    let (parser_fails, other): (Vec<_>, Vec<_>) = failures.into_iter().partition(|(p, _)| *p == Part::Highlighting);
    if !parser_fails.is_empty() {
        report.notes.push(format!(
            "Highlighting for {} couldn't be prepared ({}); those files will show with plainer colors. Running `platoon plugin add editor` again tries once more.",
            parser_fails.iter().map(|(_, e)| e.split(':').next().unwrap_or("").to_string()).collect::<Vec<_>>().join(", "),
            parser_fails.first().map(|(_, e)| e.as_str()).unwrap_or("")
        ));
    }
    let now = status(dirs, m);
    report.font = now.items.iter().any(|i| i.key == "font" && i.state == ItemState::Ok);
    report.font_family = m.font.family.clone();
    report.disk_bytes = size_of(&dirs.editor) + font_bytes(dirs);
    report.took = start.elapsed();
    if !other.is_empty() {
        let lines: Vec<String> = other.iter().map(|(p, e)| format!("  {}: {e}", p.label())).collect();
        return Err(format!(
            "Couldn't install everything; what did install is kept, and running it again carries on from there.\n{}",
            lines.join("\n")
        ));
    }
    Ok(report)
}

/// Take out what platoon no longer pins: plugin folders, parsers and query links, and their records.
fn tidy(dirs: &Dirs, m: &Manifest, items: &mut Map<String, Value>) {
    let plugins: Vec<&str> = m.plugins.iter().map(|p| p.name.as_str()).collect();
    if let Ok(rd) = fs::read_dir(dirs.plugins()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !plugins.contains(&name.as_str()) {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
    let langs: Vec<&str> = m.parsers.iter().map(|p| p.lang.as_str()).collect();
    let deps: Vec<&str> = m.parsers.iter().flat_map(|p| p.requires.iter().map(String::as_str)).collect();
    if let Ok(rd) = fs::read_dir(dirs.site().join("parser")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let lang = name.strip_suffix(".so").unwrap_or(&name);
            if !langs.contains(&lang) {
                let _ = fs::remove_file(e.path());
                let _ = fs::remove_file(dirs.site().join("parser-info").join(format!("{lang}.revision")));
            }
        }
    }
    if let Ok(rd) = fs::read_dir(dirs.site().join("queries")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !langs.contains(&name.as_str()) && !deps.contains(&name.as_str()) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    items.retain(|k, _| match k.split_once(':') {
        Some(("tool", id)) => m.tools.iter().any(|t| t.id == id),
        Some(("plugin", n)) => plugins.contains(&n),
        Some(("parser", l)) => langs.contains(&l),
        _ => k == "font",
    });
}

fn run_job(dirs: &Dirs, m: &Manifest, platform: Platform, job: &Job, work: &Path, cc: Option<&Path>) -> Result<Done, String> {
    fs::create_dir_all(work).map_err(|e| e.to_string())?;
    match job {
        Job::Tool(t) => install_tool(dirs, t, platform, work),
        Job::Plugin(p) => install_plugin(dirs, p, work),
        Job::Parser(p) => install_parser(dirs, p, work, cc.unwrap_or(Path::new("cc"))).map_err(|e| format!("{}: {e}", p.lang)),
        Job::Font => install_font(dirs, &m.font, work),
    }
}

/// Put `from` where `to` is, replacing what was there: the old copy is moved aside first and deleted after, so `to`
/// is never a half-written folder.
fn move_into_place(from: &Path, to: &Path, work: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("couldn't make {}: {e}", parent.display()))?;
    }
    let old = work.join("old");
    let had = fs::symlink_metadata(to).is_ok();
    if had {
        fs::rename(to, &old).map_err(|e| format!("couldn't move {} aside: {e}", to.display()))?;
    }
    if let Err(e) = fs::rename(from, to) {
        if had {
            let _ = fs::rename(&old, to);
        }
        return Err(format!("couldn't move {} into place: {e}", to.display()));
    }
    let _ = fs::remove_dir_all(&old).or_else(|_| fs::remove_file(&old));
    Ok(())
}

/// Point `bin/<name>` at `target` (a new link is made beside it and renamed over the old one).
fn link_bin(dirs: &Dirs, name: &str, target: &Path) -> Result<(), String> {
    let bin = dirs.bin();
    fs::create_dir_all(&bin).map_err(|e| format!("couldn't make {}: {e}", bin.display()))?;
    let link = bin.join(name);
    let tmp = bin.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp).map_err(|e| format!("couldn't link {}: {e}", link.display()))?;
    fs::rename(&tmp, &link).map_err(|e| format!("couldn't link {}: {e}", link.display()))
}

fn install_tool(dirs: &Dirs, t: &ToolPin, platform: Platform, work: &Path) -> Result<Done, String> {
    let key = format!("tool:{}", t.id);
    if let Some(p) = yours(dirs, t) {
        link_bin(dirs, &t.id, &p)?;
        // A download from an earlier run is no longer needed.
        let _ = fs::remove_dir_all(dirs.tools().join(&t.id));
        return Ok(Done {
            key,
            record: json!({"source": "yours", "path": p.to_string_lossy()}),
            fetched: false,
            yours: Some(format!("{} ({})", t.id, p.display())),
        });
    }
    let (_, d) = t.assets.iter().find(|(p, _)| *p == platform).ok_or_else(|| format!("{}: no download for this computer", t.id))?;
    let file = fetch(dirs, d, work).map_err(|e| format!("{}: {e}", t.id))?;
    let unpacked = work.join("unpacked");
    fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    if t.single_gz {
        let out = fs::File::create(unpacked.join(&t.exe)).map_err(|e| e.to_string())?;
        let st = Command::new("gzip").arg("-dc").arg(&file).stdin(Stdio::null()).stdout(out).stderr(Stdio::null()).status();
        if !st.is_ok_and(|s| s.success()) {
            return Err(format!("{}: couldn't unpack the download", t.id));
        }
        fs::set_permissions(unpacked.join(&t.exe), fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    } else {
        untar(&file, &unpacked, true).map_err(|e| format!("{}: {e}", t.id))?;
    }
    if !runnable(&unpacked.join(&t.exe)) {
        return Err(format!("{}: the download didn't hold {}", t.id, t.exe));
    }
    let dest = dirs.tools().join(&t.id);
    move_into_place(&unpacked, &dest, work)?;
    link_bin(dirs, &t.id, &dest.join(&t.exe))?;
    Ok(Done { key, record: json!({"source": "download", "version": t.version, "sha256": d.sha256}), fetched: true, yours: None })
}

fn install_plugin(dirs: &Dirs, p: &PluginPin, work: &Path) -> Result<Done, String> {
    let d = Download { url: p.url.clone(), sha256: p.sha256.clone(), size: p.size };
    let file = fetch(dirs, &d, work).map_err(|e| format!("{}: {e}", p.name))?;
    let unpacked = work.join("unpacked");
    fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    untar(&file, &unpacked, true).map_err(|e| format!("{}: {e}", p.name))?;
    move_into_place(&unpacked, &dirs.plugins().join(&p.name), work)?;
    Ok(Done { key: format!("plugin:{}", p.name), record: json!({"rev": p.rev, "sha256": p.sha256}), fetched: true, yours: None })
}

/// Build one parser the way `tree-sitter build` does (parser.c, and scanner.c when there is one, into one shared
/// library), then lay it out as nvim-treesitter does: `parser/<lang>.so`, `parser-info/<lang>.revision`, and its
/// queries linked from the plugin.
fn install_parser(dirs: &Dirs, p: &ParserPin, work: &Path, cc: &Path) -> Result<Done, String> {
    let d = Download { url: p.url.clone(), sha256: p.sha256.clone(), size: p.size };
    let file = fetch(dirs, &d, work)?;
    let src_root = work.join("src");
    fs::create_dir_all(&src_root).map_err(|e| e.to_string())?;
    untar(&file, &src_root, true)?;
    let grammar = if p.location.is_empty() { src_root.clone() } else { src_root.join(&p.location) };
    let src = grammar.join("src");
    if !src.join("parser.c").is_file() {
        return Err("the download has no src/parser.c".into());
    }
    let out = work.join("parser.so");
    let mut cmd = Command::new(cc);
    cmd.args(["-std=c11", "-O2", "-fPIC", "-shared"]).arg("-I").arg(&src).arg(src.join("parser.c"));
    if src.join("scanner.c").is_file() {
        cmd.arg(src.join("scanner.c"));
    }
    cmd.arg("-o").arg(&out).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let o = cmd.output().map_err(|e| format!("couldn't run the C compiler: {e}"))?;
    if !o.status.success() || !out.is_file() {
        let err = String::from_utf8_lossy(&o.stderr);
        return Err(format!("the C compiler failed: {}", err.lines().find(|l| l.contains("error")).unwrap_or("").trim()));
    }
    let site = dirs.site();
    let parser_dir = site.join("parser");
    fs::create_dir_all(&parser_dir).map_err(|e| e.to_string())?;
    let target = parser_dir.join(format!("{}.so", p.lang));
    // Renamed over the old one: an nvim that has the old one loaded keeps using it until it restarts.
    let staged = parser_dir.join(format!(".{}.so.{}.tmp", p.lang, std::process::id()));
    fs::copy(&out, &staged).map_err(|e| e.to_string())?;
    fs::rename(&staged, &target).map_err(|e| e.to_string())?;
    let info = site.join("parser-info");
    fs::create_dir_all(&info).map_err(|e| e.to_string())?;
    fs::write(info.join(format!("{}.revision", p.lang)), &p.rev).map_err(|e| e.to_string())?;
    for lang in std::iter::once(&p.lang).chain(p.requires.iter()) {
        link_queries(dirs, lang)?;
    }
    Ok(Done { key: format!("parser:{}", p.lang), record: json!({"rev": p.rev, "sha256": p.sha256}), fetched: true, yours: None })
}

/// `site/queries/<lang>` -> the plugin's `runtime/queries/<lang>`, relative, as nvim-treesitter links them.
fn link_queries(dirs: &Dirs, lang: &str) -> Result<(), String> {
    let q = dirs.site().join("queries");
    fs::create_dir_all(&q).map_err(|e| e.to_string())?;
    let link = q.join(lang);
    let target = PathBuf::from("../pack/platoon/start/nvim-treesitter/runtime/queries").join(lang);
    if fs::read_link(&link).is_ok_and(|t| t == target) {
        return Ok(());
    }
    let tmp = q.join(format!(".{lang}.{}.{:?}.tmp", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(&target, &tmp).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &link).map_err(|e| e.to_string())
}

fn install_font(dirs: &Dirs, f: &FontPin, work: &Path) -> Result<Done, String> {
    let file = fetch(dirs, &f.download, work).map_err(|e| format!("font: {e}"))?;
    let unpacked = work.join("unpacked");
    fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let st = Command::new("tar")
        .arg("-xf")
        .arg(&file)
        .arg("-C")
        .arg(&unpacked)
        .args(&f.files)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !st.is_ok_and(|s| s.success()) || f.files.iter().any(|n| !unpacked.join(n).is_file()) {
        return Err("font: couldn't unpack the download".into());
    }
    fs::create_dir_all(&dirs.fonts).map_err(|e| format!("couldn't make {}: {e}", dirs.fonts.display()))?;
    // Each file is copied beside its place and renamed over it, so a font file is never half there.
    for n in &f.files {
        let staged = dirs.fonts.join(format!(".{n}.platoon.tmp"));
        fs::copy(unpacked.join(n), &staged).map_err(|e| format!("couldn't write {}: {e}", dirs.fonts.display()))?;
        fs::rename(&staged, dirs.fonts.join(n)).map_err(|e| format!("couldn't write {}: {e}", dirs.fonts.display()))?;
    }
    if dirs.refresh_fonts {
        // Only platoon's own folder, so the terminal's font list has the new family.
        let _ = Command::new("fc-cache").arg("-f").arg(&dirs.fonts).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    Ok(Done {
        key: "font".into(),
        record: json!({"version": f.version, "sha256": f.download.sha256, "dir": dirs.fonts.to_string_lossy(), "files": f.files}),
        fetched: true,
        yours: None,
    })
}

/// Download (or copy from the mirror) into `work`, and check its sha256. Returns the file.
fn fetch(dirs: &Dirs, d: &Download, work: &Path) -> Result<PathBuf, String> {
    let base = d.url.rsplit('/').next().unwrap_or("download");
    let file = work.join(format!("download-{base}"));
    match &dirs.mirror {
        Some(m) => {
            fs::copy(m.join(base), &file).map_err(|e| format!("{base} isn't in the mirror folder {}: {e}", m.display()))?;
        }
        None => {
            let o = Command::new("curl")
                .args(["-fsSL", "--proto", "=https", "--proto-redir", "=https", "--retry", "2", "--connect-timeout", "20", "--speed-limit", "1024", "--speed-time", "30"])
                // A download bigger than its pin says is stopped rather than fetched in full.
                .arg("--max-filesize")
                .arg(d.size.to_string())
                .arg("-o")
                .arg(&file)
                .arg(&d.url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .map_err(|e| format!("couldn't run curl: {e}"))?;
            if !o.status.success() {
                let why = String::from_utf8_lossy(&o.stderr).trim().to_string();
                return Err(format!("the download failed ({})", if why.is_empty() { d.url.clone() } else { why }));
            }
        }
    }
    let len = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    if len != d.size {
        let _ = fs::remove_file(&file);
        return Err(format!("the download of {base} isn't the file platoon expects (it is {len} bytes, not {}), so it wasn't used", d.size));
    }
    let got = sha256_file(&file).map_err(|e| e.to_string())?;
    if got != d.sha256 {
        let _ = fs::remove_file(&file);
        return Err(format!("the download of {base} isn't the file platoon expects (its checksum differs), so it wasn't used"));
    }
    Ok(file)
}

/// Unpack a tar archive (gzip or xz, told apart by tar itself) into `into`, dropping the one top folder.
fn untar(file: &Path, into: &Path, strip_top: bool) -> Result<(), String> {
    let mut cmd = Command::new("tar");
    cmd.arg("-xf").arg(file).arg("-C").arg(into);
    if strip_top {
        cmd.args(["--strip-components", "1"]);
    }
    let o = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| format!("couldn't run tar: {e}"))?;
    if !o.status.success() {
        return Err(format!("couldn't unpack the download ({})", String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(())
}

fn size_of(p: &Path) -> u64 {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => fs::read_dir(p).map(|rd| rd.flatten().map(|e| size_of(&e.path())).sum()).unwrap_or(0),
        Ok(m) if m.is_file() => m.len(),
        _ => 0,
    }
}

fn font_bytes(dirs: &Dirs) -> u64 {
    let rec = load_record(dirs);
    let Some(r) = rec.get("font") else { return 0 };
    let dir = PathBuf::from(str_of(r, "dir"));
    r.get("files").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|f| size_of(&dir.join(f))).sum()).unwrap_or(0)
}

// ---- remove ---------------------------------------------------------------------------------------------------------

/// What `remove` took out.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Removed {
    pub editor: bool,
    pub font_files: usize,
    pub bytes: u64,
}

/// Delete platoon's editor folder and the font files platoon installed, and nothing else: never the user's own nvim,
/// rg, fd or fonts (the links to them in `bin/` go, the programs stay).
pub fn remove(dirs: &Dirs) -> Result<Removed, String> {
    if !dirs.editor.exists() {
        return Ok(Removed::default());
    }
    let lock = lock(dirs)?;
    let rec = load_record(dirs);
    let mut out = Removed { editor: true, bytes: size_of(&dirs.editor) + font_bytes(dirs), ..Removed::default() };
    // The record goes first: a removal that stops half way never leaves a folder that looks installed.
    let _ = fs::remove_file(dirs.record_path());
    if let Some(r) = rec.get("font") {
        let dir = PathBuf::from(str_of(r, "dir"));
        for f in r.get("files").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            // Only a plain file name platoon wrote, never a path that could reach outside the folder.
            if f.contains('/') || f.starts_with('.') {
                continue;
            }
            if fs::remove_file(dir.join(f)).is_ok() {
                out.font_files += 1;
            }
        }
        // Platoon's own font folder goes when nothing else is in it (never ~/Library/Fonts: it holds other fonts).
        let _ = fs::remove_dir(&dir);
    }
    drop(lock);
    fs::remove_dir_all(&dirs.editor).map_err(|e| format!("couldn't remove {}: {e}", dirs.editor.display()))?;
    Ok(out)
}

// ---- words ----------------------------------------------------------------------------------------------------------

pub fn megabytes(bytes: u64) -> String {
    format!("{} MB", (bytes + 500_000) / 1_000_000)
}

/// How to pick the font in the terminal, plainly. Nothing in any terminal's settings is changed.
pub fn font_advice(family: &str, mac: bool) -> String {
    let mut lines = vec![format!("To see the editor's icons, pick \"{family}\" as your terminal's font:")];
    if mac {
        lines.push("  Terminal: Settings > Profiles > Text > Font > Change".into());
        lines.push("  iTerm2: Settings > Profiles > Text > Font".into());
    } else {
        lines.push("  Xfce Terminal: Edit > Preferences > Appearance > Font".into());
    }
    lines.push(format!("  Ghostty: add the line  font-family = \"{family}\"  to its config file"));
    lines.join("\n")
}

/// The closing lines after an install: time, size, the user's own programs used, notes.
pub fn render_report(r: &Report, dirs: &Dirs) -> String {
    let mut lines = vec![format!(
        "The code editor is ready: {} in {}, took {} s.",
        megabytes(r.disk_bytes),
        dirs.editor.display(),
        r.took.as_secs_f32().round() as u64
    )];
    if !r.yours.is_empty() {
        lines.push(format!("Used your own: {}.", r.yours.join(", ")));
    }
    lines.extend(r.notes.iter().cloned());
    if r.font {
        lines.push(font_advice(&r.font_family, dirs.platform.is_some_and(Platform::is_mac)));
    }
    lines.join("\n")
}

/// Progress for `platoon plugin add editor`: in a terminal one line redrawn in place ("Neovim ✓  Plugins 9/11  ..."), else a
/// line as each part finishes.
pub struct Printer {
    tty: bool,
    quiet: bool,
    state: Mutex<BTreeMap<Part, Progress>>,
}

impl Printer {
    pub fn new(tty: bool) -> Printer {
        Printer { tty, quiet: false, state: Mutex::new(BTreeMap::new()) }
    }

    /// Prints nothing (tests).
    #[cfg(test)]
    pub fn quiet() -> Printer {
        Printer { tty: false, quiet: true, state: Mutex::new(BTreeMap::new()) }
    }

    fn cell(p: &Progress) -> String {
        if p.done == p.total && p.failed == 0 {
            if p.total > 1 { format!("{} {}/{} ✓", p.part.label(), p.done, p.total) } else { format!("{} ✓", p.part.label()) }
        } else if p.done + p.failed == p.total {
            format!("{} {}/{} ({} failed)", p.part.label(), p.done, p.total, p.failed)
        } else {
            format!("{} {}/{}", p.part.label(), p.done, p.total)
        }
    }

    pub fn update(&self, p: Progress) {
        use std::io::Write;
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let finished_now = p.done + p.failed == p.total && st.get(&p.part).is_none_or(|old| old.done + old.failed != old.total || old.total == 0);
        st.insert(p.part, p);
        if self.quiet {
            return;
        }
        let mut out = std::io::stdout().lock();
        if self.tty {
            let line = st.values().map(Printer::cell).collect::<Vec<_>>().join("   ");
            // Cut to the window's width: a line that wraps can't be redrawn in place.
            let line: String = line.chars().take(terminal_width().saturating_sub(1)).collect();
            let _ = write!(out, "\r\x1b[K{line}");
        } else if finished_now {
            let _ = writeln!(out, "{}", Printer::cell(&p));
        }
        let _ = out.flush();
    }

    /// Ends the redrawn line.
    pub fn finish(&self) {
        if self.tty && !self.quiet && !self.state.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            println!();
        }
    }
}

/// Columns in the terminal stdout is, 80 when it can't tell.
fn terminal_width() -> usize {
    // SAFETY: TIOCGWINSZ fills the zeroed struct it is given, or fails and leaves it zero.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0;
    if ok && ws.ws_col > 0 { usize::from(ws.ws_col) } else { 80 }
}

// ---- sha256 ---------------------------------------------------------------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

struct Sha256 {
    h: [u32; 8],
    buf: Vec<u8>,
    len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Sha256 { h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19], buf: Vec::with_capacity(64), len: 0 }
    }

    fn block(&mut self, b: &[u8]) {
        let mut w = [0u32; 64];
        for (i, c) in b.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b2, mut c, mut d, mut e, mut f, mut g, mut h] = self.h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b2) ^ (a & c) ^ (b2 & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b2;
            b2 = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in self.h.iter_mut().zip([a, b2, c, d, e, f, g, h]) {
            *x = x.wrapping_add(y);
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if !self.buf.is_empty() {
            let take = (64 - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == 64 {
                let b = std::mem::take(&mut self.buf);
                self.block(&b);
                self.buf = b;
                self.buf.clear();
            }
        }
        let mut chunks = data.chunks_exact(64);
        for c in &mut chunks {
            self.block(c);
        }
        self.buf.extend_from_slice(chunks.remainder());
    }

    fn hex(mut self) -> String {
        let bits = self.len.wrapping_mul(8);
        let mut tail = std::mem::take(&mut self.buf);
        tail.push(0x80);
        while tail.len() % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_be_bytes());
        for c in tail.chunks_exact(64) {
            self.block(c);
        }
        self.h.iter().map(|x| format!("{x:08x}")).collect()
    }
}

/// The sha256 of some bytes, as lowercase hex (also used for plugins' files).
pub fn sha256_hex(data: &[u8]) -> String {
    let mut s = Sha256::new();
    s.update(data);
    s.hex()
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    use std::io::Read;
    let mut f = fs::File::open(path)?;
    let mut s = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        s.update(&buf[..n]);
    }
    Ok(s.hex())
}

#[cfg(test)]
pub mod testing {
    //! A mirror of tiny fake downloads that match a manifest of their own, for tests here and in `setup`.
    use super::*;

    pub fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-editor-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    pub fn write_exe(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A folder `top` holding `files` (path, text; an executable when the text starts with `#!`), packed as
    /// `<mirror>/<name>`; returns its sha256.
    pub fn pack(mirror: &Path, name: &str, top: &str, files: &[(&str, &str)]) -> String {
        let stage = mirror.join(format!(".stage-{name}"));
        let _ = fs::remove_dir_all(&stage);
        for (p, text) in files {
            let path = stage.join(top).join(p);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            if text.starts_with("#!") {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let out = mirror.join(name);
        let st = Command::new("tar").arg("-czf").arg(&out).arg("-C").arg(&stage).arg(top).status().unwrap();
        assert!(st.success());
        fs::remove_dir_all(&stage).unwrap();
        sha256_file(&out).unwrap()
    }

    /// A manifest whose every download is in `mirror`: three tools, two plugins, two parsers (one with queries it
    /// needs from another language), a font.
    pub fn fake(mirror: &Path) -> Manifest {
        fs::create_dir_all(mirror).unwrap();
        let mut tools = Vec::new();
        for (id, min, names, exe) in [("nvim", Some([0, 12, 0]), vec!["nvim"], "bin/nvim"), ("rg", None, vec!["rg"], "rg"), ("fd", None, vec!["fd", "fdfind"], "fd")] {
            let name = format!("{id}-fake.tar.gz");
            let sha = pack(mirror, &name, &format!("{id}-dist"), &[(exe, "#!/bin/sh\necho fake\n"), ("README", "x")]);
            let size = fs::metadata(mirror.join(&name)).unwrap().len();
            let assets = Platform::ALL.iter().map(|p| (*p, Download { url: format!("https://example.invalid/{name}"), sha256: sha.clone(), size })).collect();
            tools.push(ToolPin { id: id.into(), version: "1.0.0".into(), min_yours: min, names: names.into_iter().map(String::from).collect(), exe: exe.into(), single_gz: false, assets });
        }
        // The fourth, a single gzipped program like tree-sitter's.
        let gz = mirror.join("ts-fake.gz");
        let raw = mirror.join("ts-fake");
        fs::write(&raw, "#!/bin/sh\necho 0.27.0\n").unwrap();
        let st = Command::new("sh").arg("-c").arg(format!("gzip -c '{}' > '{}'", raw.display(), gz.display())).status().unwrap();
        assert!(st.success());
        fs::remove_file(&raw).unwrap();
        let sha = sha256_file(&gz).unwrap();
        let size = fs::metadata(&gz).unwrap().len();
        tools.push(ToolPin {
            id: "tree-sitter".into(),
            version: "0.27.0".into(),
            min_yours: Some([0, 26, 1]),
            names: vec!["tree-sitter".into()],
            exe: "tree-sitter".into(),
            single_gz: true,
            assets: Platform::ALL.iter().map(|p| (*p, Download { url: "https://example.invalid/ts-fake.gz".into(), sha256: sha.clone(), size })).collect(),
        });
        let mut plugins = Vec::new();
        for name in ["alpha.nvim", "nvim-treesitter"] {
            let rev = format!("rev-{name}");
            let sha = pack(mirror, &rev, &format!("{name}-{rev}"), &[("lua/x.lua", "return 1"), ("runtime/queries/lang1/highlights.scm", "; q"), ("runtime/queries/dep/highlights.scm", "; d")]);
            let size = fs::metadata(mirror.join(&rev)).unwrap().len();
            plugins.push(PluginPin { name: name.into(), repo: format!("o/{name}"), url: format!("https://example.invalid/{rev}"), rev, sha256: sha, size });
        }
        let mut parsers = Vec::new();
        for (lang, requires) in [("lang1", vec!["dep".to_string()]), ("lang2", vec![])] {
            let rev = format!("prev-{lang}");
            let sha = pack(mirror, &rev, &format!("tree-sitter-{lang}"), &[("src/parser.c", "int x;"), ("src/scanner.c", "int y;")]);
            let size = fs::metadata(mirror.join(&rev)).unwrap().len();
            parsers.push(ParserPin { lang: lang.into(), repo: format!("o/{lang}"), url: format!("https://example.invalid/{rev}"), rev, location: String::new(), sha256: sha, size, requires });
        }
        let files = vec!["FakeNerdFont-Regular.ttf".to_string(), "OFL.txt".to_string()];
        let fstage = mirror.join(".font");
        fs::create_dir_all(&fstage).unwrap();
        for f in &files {
            fs::write(fstage.join(f), "font").unwrap();
        }
        fs::write(fstage.join("FakeNerdFontMono-Regular.ttf"), "not taken").unwrap();
        let st = Command::new("tar").arg("-czf").arg(mirror.join("Font.tar.xz")).arg("-C").arg(&fstage).args(["FakeNerdFont-Regular.ttf", "OFL.txt", "FakeNerdFontMono-Regular.ttf"]).status().unwrap();
        assert!(st.success());
        fs::remove_dir_all(&fstage).unwrap();
        let font_file = mirror.join("Font.tar.xz");
        let font = FontPin {
            family: "Fake Nerd Font".into(),
            version: "9".into(),
            download: Download { url: "https://example.invalid/Font.tar.xz".into(), sha256: sha256_file(&font_file).unwrap(), size: fs::metadata(&font_file).unwrap().len() },
            files,
        };
        Manifest { tools, plugins, parsers, font }
    }

    /// A fake C compiler that writes what it was asked to build into the `-o` file, and logs each call.
    pub fn fake_cc(dir: &Path) -> PathBuf {
        let bin = dir.join("ccbin");
        write_exe(&bin.join("cc"), &format!("out=''; prev=''; for a in \"$@\"; do [ \"$prev\" = -o ] && out=$a; prev=$a; done; echo \"$*\" >> '{}'; echo built > \"$out\"", dir.join("cc.log").display()));
        bin
    }

    pub fn dirs(base: &Path, search_path: Vec<PathBuf>) -> Dirs {
        Dirs {
            editor: base.join("data/editor"),
            fonts: base.join("fonts/platoon"),
            mirror: Some(base.join("mirror")),
            search_path,
            refresh_fonts: false,
            platform: Some(Platform::LinuxX64),
        }
    }

    pub fn quiet(_: Progress) {}
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn sha256_matches_known_values() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(sha256_hex(long), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        // Fed in uneven pieces, the same answer as all at once.
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut s = Sha256::new();
        for c in data.chunks(37) {
            s.update(c);
        }
        assert_eq!(s.hex(), sha256_hex(&data));
        let one_million_a = vec![b'a'; 1_000_000];
        assert_eq!(sha256_hex(&one_million_a), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn the_editor_plugins_table_is_complete_for_every_computer() {
        let m = crate::editorplugin::bundled_table();
        assert!(table_problems(&m).is_empty(), "{:?}", table_problems(&m));
        let hex = |s: &str| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
        let ids: Vec<&str> = m.tools.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["nvim", "rg", "fd", "tree-sitter"]);
        for t in &m.tools {
            for p in Platform::ALL {
                let d = &t.assets.iter().find(|(q, _)| *q == p).unwrap_or_else(|| panic!("{} has no download for {p:?}", t.id)).1;
                assert!(d.url.starts_with("https://github.com/") && d.url.contains(&t.version), "{}", d.url);
                assert!(hex(&d.sha256) && d.size > 100_000, "{}: {} {}", t.id, d.sha256, d.size);
            }
            assert_eq!(t.assets.len(), 4, "{}", t.id);
        }
        // nvim-treesitter's `main` branch needs Neovim 0.12, and its build step tree-sitter 0.26.1.
        assert_eq!(m.tools[0].min_yours, Some([0, 12, 0]));
        assert_eq!(m.tools[3].min_yours, Some([0, 26, 1]));
        let names: Vec<&str> = m.plugins.iter().map(|p| p.name.as_str()).collect();
        for want in ["neo-tree.nvim", "plenary.nvim", "nui.nvim", "diffview.nvim", "gitsigns.nvim", "snacks.nvim", "nvim-treesitter", "which-key.nvim", "lualine.nvim", "blink.cmp", "mini.nvim"] {
            assert!(names.contains(&want), "{want}");
        }
        for p in &m.plugins {
            assert!(p.rev.len() == 40 && p.rev.chars().all(|c| c.is_ascii_hexdigit()), "{}: a commit, not a branch: {}", p.name, p.rev);
            assert!(hex(&p.sha256), "{}", p.name);
            assert!(p.repo.contains('/'), "{}", p.repo);
            assert_eq!(p.url, format!("https://codeload.github.com/{}/tar.gz/{}", p.repo, p.rev), "{}", p.name);
            assert!(p.size > 0, "{}", p.name);
        }
        let langs: Vec<&str> = m.parsers.iter().map(|p| p.lang.as_str()).collect();
        for want in ["lua", "vim", "vimdoc", "query", "markdown", "markdown_inline", "rust", "python", "javascript", "typescript", "tsx", "json", "yaml", "toml", "bash", "html", "css", "go", "c"] {
            assert!(langs.contains(&want), "{want}");
        }
        for p in &m.parsers {
            assert!(hex(&p.sha256) && !p.rev.is_empty() && p.repo.contains('/') && p.size > 0, "{}", p.lang);
            assert_eq!(p.url, format!("https://codeload.github.com/{}/tar.gz/{}", p.repo, p.rev), "{}", p.lang);
        }
        assert_eq!(m.font.family, "JetBrainsMono Nerd Font");
        // What it fetches on this machine's kind of computer: tens of MB, not hundreds.
        let mb = download_bytes(&m, Platform::LinuxX64) / 1_000_000;
        assert!((40..120).contains(&mb), "{mb} MB");
        assert!(m.font.download.url.ends_with("JetBrainsMono.tar.xz") && hex(&m.font.download.sha256));
        assert!(m.font.files.contains(&"JetBrainsMonoNerdFont-Regular.ttf".to_string()) && m.font.files.contains(&"OFL.txt".to_string()));
        // Every download has its own last part, which is what the mirror folder is keyed by.
        let mut bases: Vec<String> = m.tools.iter().flat_map(|t| t.assets.iter().map(|(_, d)| d.url.rsplit('/').next().unwrap().to_string())).collect();
        bases.sort();
        let n = bases.len();
        bases.dedup();
        assert_eq!(bases.len(), n);
    }

    #[test]
    fn versions_are_read_from_what_programs_print() {
        assert_eq!(parse_version("NVIM v0.12.5\nBuild type: Release"), Some([0, 12, 5]));
        assert_eq!(parse_version("NVIM v0.11.4-dev+123"), Some([0, 11, 4]));
        assert_eq!(parse_version("tree-sitter 0.27.0 (abcdef)"), Some([0, 27, 0]));
        assert_eq!(parse_version("ripgrep 15.2.0"), Some([15, 2, 0]));
        assert_eq!(parse_version("nothing here"), None);
    }

    #[test]
    fn data_and_font_folders_follow_their_knobs() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(a, _)| *a == k).map(|(_, v)| v.to_string());
        assert_eq!(data_dir_from(&env(&[("PLATOON_DATA_DIR", "/d"), ("XDG_DATA_HOME", "/x"), ("HOME", "/h")]), false), PathBuf::from("/d"));
        assert_eq!(data_dir_from(&env(&[("XDG_DATA_HOME", "/x"), ("HOME", "/h")]), false), PathBuf::from("/x/platoon"));
        assert_eq!(data_dir_from(&env(&[("HOME", "/h")]), false), PathBuf::from("/h/.local/share/platoon"));
        assert_eq!(data_dir_from(&env(&[("HOME", "/h")]), true), PathBuf::from("/h/Library/Application Support/platoon"));
        assert_eq!(font_dir_from(&env(&[("PLATOON_FONT_DIR", "/f"), ("HOME", "/h")]), false), PathBuf::from("/f"));
        assert_eq!(font_dir_from(&env(&[("HOME", "/h")]), false), PathBuf::from("/h/.local/share/fonts/platoon"));
        assert_eq!(font_dir_from(&env(&[("HOME", "/h")]), true), PathBuf::from("/h/Library/Fonts"));
    }

    fn record(d: &Dirs) -> Value {
        serde_json::from_str(&fs::read_to_string(d.record_path()).unwrap()).unwrap()
    }

    #[test]
    fn an_install_from_the_mirror_lays_the_folder_out_and_records_it_and_a_second_run_fetches_nothing() {
        let base = tmp("install");
        let m = fake(&base.join("mirror"));
        let d = dirs(&base, vec![fake_cc(&base)]);
        assert!(status(&d, &m).nothing());
        let seen = Mutex::new(Vec::new());
        let r = install(&d, &m, &|p| seen.lock().unwrap().push(p)).unwrap();
        assert_eq!(r.fetched, 4 + 2 + 2 + 1, "every tool, plugin, parser and the font");
        assert!(r.yours.is_empty() && r.notes.is_empty() && r.font, "{r:?}");
        assert!(r.disk_bytes > 0);
        // The layout phase 3 relies on.
        let e = &d.editor;
        for (bin, target) in [("nvim", "tools/nvim/bin/nvim"), ("rg", "tools/rg/rg"), ("fd", "tools/fd/fd"), ("tree-sitter", "tools/tree-sitter/tree-sitter")] {
            assert_eq!(fs::read_link(e.join("bin").join(bin)).unwrap(), e.join(target), "{bin}");
            assert!(runnable(&e.join("bin").join(bin)), "{bin}");
        }
        assert_eq!(d.nvim(), Some(e.join("bin/nvim")));
        assert!(e.join("tools/nvim/README").is_file(), "the whole release, not just the program");
        assert_eq!(fs::read_to_string(e.join("tools/tree-sitter/tree-sitter")).unwrap(), "#!/bin/sh\necho 0.27.0\n");
        assert!(e.join("site/pack/platoon/start/alpha.nvim/lua/x.lua").is_file());
        assert!(e.join("site/pack/platoon/start/nvim-treesitter/runtime/queries/lang1").is_dir());
        assert_eq!(fs::read_to_string(e.join("site/parser/lang1.so")).unwrap(), "built\n");
        assert_eq!(fs::read_to_string(e.join("site/parser-info/lang1.revision")).unwrap(), "prev-lang1");
        assert_eq!(fs::read_link(e.join("site/queries/lang1")).unwrap(), PathBuf::from("../pack/platoon/start/nvim-treesitter/runtime/queries/lang1"));
        assert!(e.join("site/queries/lang1/highlights.scm").is_file(), "the link resolves");
        assert!(e.join("site/queries/dep/highlights.scm").is_file(), "the queries it builds on are linked too");
        let log = fs::read_to_string(base.join("cc.log")).unwrap();
        assert!(log.contains("-shared") && log.contains("parser.c") && log.contains("scanner.c"), "{log}");
        assert!(d.fonts.join("FakeNerdFont-Regular.ttf").is_file() && d.fonts.join("OFL.txt").is_file());
        assert!(!d.fonts.join("FakeNerdFontMono-Regular.ttf").exists(), "only the family platoon names");
        assert!(!e.join("tmp").exists(), "nothing left over");
        let v = record(&d);
        assert_eq!(v["items"]["tool:nvim"]["source"], "download");
        assert_eq!(v["items"]["tool:nvim"]["sha256"], m.tools[0].assets[0].1.sha256.as_str());
        assert_eq!(v["items"]["plugin:alpha.nvim"]["rev"], "rev-alpha.nvim");
        assert_eq!(v["items"]["parser:lang2"]["rev"], "prev-lang2");
        assert_eq!(v["items"]["font"]["files"], json!(["FakeNerdFont-Regular.ttf", "OFL.txt"]));
        assert!(v.get("no_compiler").is_none());
        let st = status(&d, &m);
        assert!(st.complete() && st.needs().is_empty(), "{:?}", st.needs());
        assert!(nerd_font_present_with(&d, &|| false));
        // Progress: each part reached its total.
        let seen = seen.into_inner().unwrap();
        for (part, total) in [(Part::Neovim, 1), (Part::Tools, 3), (Part::Plugins, 2), (Part::Highlighting, 2), (Part::Font, 1)] {
            assert!(seen.iter().any(|p| p.part == part && p.done == total && p.total == total), "{part:?}: {seen:?}");
        }
        // Again: nothing fetched, nothing rebuilt, the same record.
        let before = fs::read_to_string(d.record_path()).unwrap();
        let log_before = fs::read_to_string(base.join("cc.log")).unwrap();
        fs::remove_dir_all(base.join("mirror")).unwrap();
        let r = install(&d, &m, &quiet).unwrap();
        assert_eq!(r.fetched, 0);
        assert_eq!(fs::read_to_string(d.record_path()).unwrap(), before);
        assert_eq!(fs::read_to_string(base.join("cc.log")).unwrap(), log_before);
    }

    #[test]
    fn a_download_with_the_wrong_checksum_is_refused_and_nothing_of_it_looks_installed() {
        let base = tmp("badhash");
        let good = fake(&base.join("mirror"));
        let mut m = good.clone();
        m.plugins[0].sha256 = "0".repeat(64);
        m.tools[0].assets.iter_mut().for_each(|(_, d)| d.sha256 = "1".repeat(64));
        let d = dirs(&base, vec![fake_cc(&base)]);
        let err = install(&d, &m, &quiet).unwrap_err();
        assert!(err.contains("Neovim: nvim:") && err.contains("checksum differs") && err.contains("Plugins: alpha.nvim"), "{err}");
        assert!(err.contains("what did install is kept"), "{err}");
        assert!(!d.plugins().join("alpha.nvim").exists());
        assert!(!d.bin().join("nvim").exists() && !d.editor.join("tools/nvim").exists());
        assert!(!d.editor.join("tmp").exists(), "the refused download is gone too");
        let st = status(&d, &m);
        let state = |k: &str| st.items.iter().find(|i| i.key == k).unwrap().state.clone();
        assert_eq!(state("tool:nvim"), ItemState::Missing);
        assert_eq!(state("plugin:alpha.nvim"), ItemState::Missing);
        assert_eq!(state("tool:rg"), ItemState::Ok, "what was fine is kept");
        assert!(!st.complete());
        assert!(record(&d)["items"].get("plugin:alpha.nvim").is_none());
        // Fixed pins: the next run fetches only those two.
        let r = install(&d, &good, &quiet).unwrap();
        assert_eq!(r.fetched, 2);
        assert!(status(&d, &good).complete());
    }

    #[test]
    fn a_changed_pin_is_out_of_date_and_only_it_is_fetched_again() {
        let base = tmp("update");
        let mirror = base.join("mirror");
        let mut m = fake(&mirror);
        let d = dirs(&base, vec![fake_cc(&base)]);
        install(&d, &m, &quiet).unwrap();
        // A new commit of one plugin.
        let sha = pack(&mirror, "rev-alpha-2", "alpha.nvim-2", &[("lua/x.lua", "return 2")]);
        m.plugins[0].rev = "rev-alpha-2".into();
        m.plugins[0].url = "https://example.invalid/rev-alpha-2".into();
        m.plugins[0].size = fs::metadata(mirror.join("rev-alpha-2")).unwrap().len();
        m.plugins[0].sha256 = sha;
        let st = status(&d, &m);
        assert_eq!(st.needs(), ["alpha.nvim"]);
        assert_eq!(install(&d, &m, &quiet).unwrap().fetched, 1);
        assert_eq!(fs::read_to_string(d.plugins().join("alpha.nvim/lua/x.lua")).unwrap(), "return 2");
        // A plugin and a parser platoon no longer pins are taken out.
        m.plugins.remove(0);
        m.parsers.remove(1);
        install(&d, &m, &quiet).unwrap();
        assert!(!d.plugins().join("alpha.nvim").exists() && !d.site().join("parser/lang2.so").exists());
        assert!(record(&d)["items"].get("plugin:alpha.nvim").is_none() && record(&d)["items"].get("parser:lang2").is_none());
        assert!(d.site().join("queries/lang1").exists() && d.site().join("queries/dep").exists());
    }

    #[test]
    fn the_users_own_programs_are_used_when_new_enough() {
        let base = tmp("yours");
        let m = fake(&base.join("mirror"));
        let own = base.join("own-bin");
        write_exe(&own.join("nvim"), "echo 'NVIM v0.12.1'");
        write_exe(&own.join("rg"), "echo 'ripgrep 14.0.0'");
        write_exe(&own.join("fdfind"), "echo 'fdfind 9.0.0'");
        write_exe(&own.join("tree-sitter"), "echo 'tree-sitter 0.25.3'");
        let d = dirs(&base, vec![own.clone(), fake_cc(&base)]);
        let r = install(&d, &m, &quiet).unwrap();
        assert_eq!(fs::read_link(d.bin().join("nvim")).unwrap(), own.join("nvim"));
        assert_eq!(fs::read_link(d.bin().join("rg")).unwrap(), own.join("rg"));
        assert_eq!(fs::read_link(d.bin().join("fd")).unwrap(), own.join("fdfind"), "Debian's name for fd");
        // Too old for nvim-treesitter's build step: platoon's own.
        assert_eq!(fs::read_link(d.bin().join("tree-sitter")).unwrap(), d.editor.join("tools/tree-sitter/tree-sitter"));
        assert!(!d.editor.join("tools/nvim").exists() && !d.editor.join("tools/rg").exists());
        assert_eq!(r.yours, [format!("fd ({})", own.join("fdfind").display()), format!("nvim ({})", own.join("nvim").display()), format!("rg ({})", own.join("rg").display())]);
        assert_eq!(record(&d)["items"]["tool:nvim"], json!({"source": "yours", "path": own.join("nvim").to_string_lossy()}));
        assert!(status(&d, &m).complete());
        // Their nvim goes away: the link is broken, so it is missing, and the next run downloads one.
        fs::remove_file(own.join("nvim")).unwrap();
        assert_eq!(status(&d, &m).needs(), ["nvim"]);
        install(&d, &m, &quiet).unwrap();
        assert_eq!(fs::read_link(d.bin().join("nvim")).unwrap(), d.editor.join("tools/nvim/bin/nvim"));
    }

    #[test]
    fn an_older_nvim_of_the_users_is_not_used() {
        let base = tmp("old-nvim");
        let m = fake(&base.join("mirror"));
        let own = base.join("own-bin");
        write_exe(&own.join("nvim"), "echo 'NVIM v0.11.4'");
        let d = dirs(&base, vec![own, fake_cc(&base)]);
        let r = install(&d, &m, &quiet).unwrap();
        assert!(r.yours.is_empty(), "{:?}", r.yours);
        assert_eq!(fs::read_link(d.bin().join("nvim")).unwrap(), d.editor.join("tools/nvim/bin/nvim"));
        // platoon's own bin folder on PATH isn't taken for the user's.
        let d2 = Dirs { search_path: vec![d.bin(), fake_cc(&base)], ..d.clone() };
        assert!(find_on_path(&d2, &["rg".into()]).is_none());
    }

    #[test]
    fn without_a_c_compiler_it_still_installs_and_says_highlighting_is_limited() {
        let base = tmp("nocc");
        let m = fake(&base.join("mirror"));
        let d = dirs(&base, vec![]);
        let seen = Mutex::new(Vec::new());
        let r = install(&d, &m, &|p| seen.lock().unwrap().push(p)).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("No C compiler") && n.contains("limited")), "{:?}", r.notes);
        assert!(!d.site().join("parser").exists() && !d.site().join("queries").exists(), "no queries without their parsers");
        let st = status(&d, &m);
        assert!(st.no_compiler && st.complete(), "{:?}", st.needs());
        assert!(!seen.into_inner().unwrap().iter().any(|p| p.part == Part::Highlighting));
        // A compiler later: the parsers are built.
        let d = Dirs { search_path: vec![fake_cc(&base)], ..d };
        install(&d, &m, &quiet).unwrap();
        let st = status(&d, &m);
        assert!(!st.no_compiler && st.complete() && d.site().join("parser/lang1.so").is_file());
    }

    #[test]
    fn a_parser_that_does_not_build_is_a_note_not_a_failure() {
        let base = tmp("ccfail");
        let m = fake(&base.join("mirror"));
        let bin = base.join("badcc");
        write_exe(&bin.join("cc"), "echo 'parser.c:1: error: boom' >&2; exit 1");
        let d = dirs(&base, vec![bin]);
        let r = install(&d, &m, &quiet).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("lang1") && n.contains("plainer colors")), "{:?}", r.notes);
        assert!(!d.site().join("parser/lang1.so").exists());
        assert!(!status(&d, &m).complete());
    }

    #[test]
    fn remove_takes_out_platoons_folder_and_font_and_nothing_else() {
        let base = tmp("remove");
        let m = fake(&base.join("mirror"));
        let own = base.join("own-bin");
        write_exe(&own.join("rg"), "echo 'ripgrep 14.0.0'");
        let d = dirs(&base, vec![own.clone(), fake_cc(&base)]);
        install(&d, &m, &quiet).unwrap();
        // Things that aren't platoon's, beside what is.
        fs::write(d.fonts.join("MyOwnFont.ttf"), "mine").unwrap();
        let other_font = base.join("fonts/Other.ttf");
        fs::write(&other_font, "mine").unwrap();
        fs::create_dir_all(base.join("data/other")).unwrap();
        let r = remove(&d).unwrap();
        assert!(r.editor && r.font_files == 2 && r.bytes > 0, "{r:?}");
        assert!(!d.editor.exists());
        assert!(own.join("rg").is_file(), "the user's own rg stays");
        assert!(d.fonts.join("MyOwnFont.ttf").is_file() && other_font.is_file(), "only the files platoon put there");
        assert!(!d.fonts.join("FakeNerdFont-Regular.ttf").exists());
        assert!(base.join("data/other").is_dir());
        assert!(status(&d, &m).nothing());
        assert!(!nerd_font_present_with(&d, &|| false));
        assert_eq!(remove(&d).unwrap(), Removed::default(), "nothing the second time");
        // With nothing else in it, platoon's font folder goes too.
        install(&d, &m, &quiet).unwrap();
        fs::remove_file(d.fonts.join("MyOwnFont.ttf")).unwrap();
        remove(&d).unwrap();
        assert!(!d.fonts.exists() && base.join("fonts").is_dir());
    }

    #[test]
    fn a_second_install_at_the_same_time_is_refused() {
        let base = tmp("lock");
        let m = fake(&base.join("mirror"));
        let d = dirs(&base, vec![]);
        let held = lock(&d).unwrap();
        assert!(install(&d, &m, &quiet).unwrap_err().contains("another platoon"));
        drop(held);
        assert!(install(&d, &m, &quiet).is_ok());
    }

    #[test]
    fn an_interrupted_install_leaves_nothing_that_looks_installed() {
        let base = tmp("interrupted");
        let m = fake(&base.join("mirror"));
        let d = dirs(&base, vec![fake_cc(&base)]);
        // What a killed run leaves: a half-unpacked plugin in tmp/, and no record.
        fs::create_dir_all(d.tmp().join("w0-1/unpacked/lua")).unwrap();
        assert!(status(&d, &m).nothing());
        install(&d, &m, &quiet).unwrap();
        assert!(!d.tmp().exists() && status(&d, &m).complete());
    }

    #[test]
    fn unsupported_computers_are_told_plainly() {
        let base = tmp("unsupported");
        let m = fake(&base.join("mirror"));
        let d = Dirs { platform: None, ..dirs(&base, vec![]) };
        assert!(install(&d, &m, &quiet).unwrap_err().contains("can't be installed on this computer"));
        assert!(!status(&d, &m).complete());
    }

    #[test]
    fn the_words_are_plain() {
        let d = dirs(Path::new("/x"), vec![]);
        let r = Report { took: Duration::from_millis(41_600), disk_bytes: 151_400_000, fetched: 3, yours: vec!["rg (/usr/bin/rg)".into()], notes: vec![], font: true, font_family: "JetBrainsMono Nerd Font".into() };
        let text = render_report(&r, &d);
        assert!(text.starts_with("The code editor is ready: 151 MB in /x/data/editor, took 42 s."), "{text}");
        assert!(text.contains("Used your own: rg (/usr/bin/rg).") && text.contains("Xfce Terminal: Edit > Preferences > Appearance > Font"), "{text}");
        assert!(text.contains("font-family = \"JetBrainsMono Nerd Font\""), "{text}");
        assert!(font_advice("F", true).contains("iTerm2") && !font_advice("F", true).contains("Xfce"));
        let p = |done, total, failed| Printer::cell(&Progress { part: Part::Plugins, done, total, failed });
        assert_eq!(p(9, 11, 0), "Plugins 9/11");
        assert_eq!(p(11, 11, 0), "Plugins 11/11 ✓");
        assert_eq!(p(10, 11, 1), "Plugins 10/11 (1 failed)");
        assert_eq!(Printer::cell(&Progress { part: Part::Neovim, done: 1, total: 1, failed: 0 }), "Neovim ✓");
    }

    #[test]
    fn a_table_reads_back_what_it_writes_and_its_problems_are_said_plainly() {
        let base = tmp("table");
        let m = fake(&base.join("mirror"));
        let text = serde_json::to_string_pretty(&table_json(&m)).unwrap();
        let back = parse_table(&text).unwrap();
        assert_eq!(back, m);
        // The fake's addresses are https and pinned: nothing wrong.
        assert!(table_problems(&m).is_empty(), "{:?}", table_problems(&m));
        let problems = |f: &dyn Fn(&mut Manifest)| {
            let mut t = m.clone();
            f(&mut t);
            table_problems(&t).join("\n")
        };
        assert!(problems(&|t| t.plugins[0].url = "http://example.invalid/x".into()).contains("plugin alpha.nvim: its address must be an https:// link"));
        assert!(problems(&|t| t.tools[1].assets[0].1.sha256 = String::new()).contains("tool rg for linux-x64: it needs its sha256"));
        assert!(problems(&|t| t.font.download.size = 0).contains("the font: it needs its size in bytes"));
        assert!(problems(&|t| t.parsers[0].size = 400_000_000).contains("more than one download may be"));
        assert!(problems(&|t| {
            for p in &mut t.plugins {
                p.size = 250_000_000;
                p.sha256 = sha256_hex(p.name.as_bytes());
            }
            t.parsers.iter_mut().for_each(|p| p.size = 250_000_000);
        })
        .contains("more than the"));
        assert!(problems(&|t| t.plugins[0].name = "../escape".into()).contains("isn't a plain name"));
        assert!(problems(&|t| t.tools[0].exe = "../../bin/sh".into()).contains("\"exe\" must be a path inside its download"));
        assert!(problems(&|t| t.font.files[0] = "../x.ttf".into()).contains("plain file names"));
        assert!(problems(&|t| t.tools.retain(|x| x.id != "nvim")).contains("no tool \"nvim\""));
        assert!(problems(&|t| {
            let again = t.plugins[0].clone();
            t.plugins.push(again);
        })
        .contains("plugin alpha.nvim is listed twice"));
        // The shape: plain words too.
        assert!(parse_table("[]").unwrap_err().contains("must be a JSON object"));
        assert!(parse_table(r#"{"format": 2}"#).unwrap_err().contains("format this platoon doesn't know"));
        let mut v = table_json(&m);
        v.get_mut("plugins").and_then(|p| p.get_mut(0)).and_then(Value::as_object_mut).unwrap().remove("size");
        assert!(parse_table(&v.to_string()).unwrap_err().contains("plugin alpha.nvim has no \"size\""));
    }

    #[test]
    fn a_download_of_another_size_is_refused() {
        let base = tmp("size");
        let d = dirs(&base, vec![]);
        let mut m = fake(d.mirror.as_ref().unwrap());
        m.font.download.size += 1;
        let err = install(&d, &m, &quiet).unwrap_err();
        assert!(err.contains("Font: font: the download of Font.tar.xz isn't the file platoon expects (it is"), "{err}");
        assert!(!status(&d, &m).items.iter().any(|i| i.key == "font" && i.state == ItemState::Ok));
    }
}
