//! Plugins: optional things people add to platoon, contributed to the official catalog (the `plugins/` folder of
//! platoon's repository) by pull request. A plugin holds FILES, never code: a folder with a `plugin.json` (who made
//! it, its version, the oldest platoon it works with, and what it adds) and the files it names.
//!
//! What a plugin can add is a list of kinds (`KINDS`), each a key of `plugin.json` naming files: `"themes":
//! ["themes/Harbor Night"]`; platoon's code editor, `"editor": {"downloads": "downloads.json", "config": [...]}`
//! (its download table and the editor's settings files; `editorplugin.rs`); Claude Code skills, `"skills":
//! ["skills/<name>"]` (written into Claude Code's skills folder on install; `skillplugin.rs`); and squads and agents,
//! `"squads": ["squads/<id>.json"]`, `"agents": ["agents/<id>.json"]` (shown on the Squads tab; `squadplugin.rs`).
//! Skills and agents are instructions Claude follows with the user's permissions: installing a plugin that has any
//! shows them to be read first (`pluginreview.rs`). `plugin.json` keys this platoon doesn't know are left alone (a
//! plugin made for a newer platoon says so with `platoon`).
//!
//! On this computer:
//!
//! ```text
//! <data>/plugins/                 `PLATOON_DATA_DIR`, as the code editor uses
//!   <id>/...                      an installed plugin: a copy of its folder
//!   installed.json                what is installed: version, where from (catalog commit or folder), sha256
//!   .catalog/, .catalog.json      the catalog as last fetched, and from where (source, ref, commit)
//!   .lock, .tmp/, .fetch/         one change at a time; installs are staged in .tmp, fetches cloned in .fetch
//! ```
//!
//! A plugin counts as installed only when `installed.json` lists it and its folder is there: an install copies into
//! `.tmp`, checks the copy, moves it into place and only then records it, so one that fails or is interrupted never
//! looks installed. Removing un-records first, then deletes.
//!
//! The catalog is fetched with `git` (a shallow, sparse, blob-filtered clone of `plugins/` at a ref), so it works
//! with the user's own git login while the repository is private and without one once it is public.
//! `GIT_TERMINAL_PROMPT=0`: a missing login fails at once with a plain message rather than waiting for a password.
//!
//! Knobs: `PLATOON_PLUGIN_SOURCE` (a repository URL or a local path instead of platoon's own; tests use a temp git
//! repository, never the network), `PLATOON_PLUGIN_REF` (a branch or tag instead of `main`).

use crate::editor_install::{sha256_file, sha256_hex};
use crate::theme::{contrast, delta_e, luminance, Theme, MIN_DELTA_E, MIN_ROLE, MIN_TEXT};
use crate::usertheme;
use serde_json::{json, Map, Value};
use std::{
    ffi::OsStr,
    fs, io,
    io::Read,
    os::unix::{fs::PermissionsExt, io::AsRawFd},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

/// Where the official catalog lives: platoon's own repository, its `plugins/` folder.
pub const DEFAULT_SOURCE: &str = "https://github.com/ctrader2/platoon.git";
pub const DEFAULT_REF: &str = "main";
/// The catalog's folder in the repository.
pub const CATALOG_FOLDER: &str = "plugins";
pub const MANIFEST: &str = "plugin.json";
pub const INDEX: &str = "index.json";
/// The id prefix of every plugin theme: `plugin:<plugin id>/<theme>`, which no built-in or user theme can have.
pub const THEME_PREFIX: &str = "plugin:";

/// A whole plugin, every file together. Themes are under 1 KiB each: this is room for hundreds.
pub const MAX_PLUGIN_BYTES: u64 = 1024 * 1024;
/// Files in one plugin folder.
pub const MAX_FILES: usize = 200;
/// How deep its folders go.
const MAX_DEPTH: usize = 4;
/// The manifest itself.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_ID: usize = 40;
const MAX_NAME: usize = 40;
const MAX_DESCRIPTION: usize = 400;
const MAX_AUTHOR: usize = 80;
const MAX_LICENSE: usize = 60;
const MAX_PATH: usize = 200;
/// A fetch of the catalog that takes longer than this is given up (a network that hangs).
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// A kind of thing a plugin can add: its key in `plugin.json`, and how one and many of them are called.
pub struct KindInfo {
    pub key: &'static str,
    pub one: &'static str,
    pub many: &'static str,
    /// A list of files (`"themes": [...]`); otherwise one thing described by an object (`"editor": {...}`), said as
    /// `one` whatever its files.
    pub list: bool,
}

/// What plugins can add in this version.
pub const KINDS: &[KindInfo] = &[
    KindInfo { key: "themes", one: "theme", many: "themes", list: true },
    KindInfo { key: crate::editorplugin::KIND, one: "the code editor", many: "the code editor", list: false },
    KindInfo { key: crate::skillplugin::KIND, one: "skill", many: "skills", list: true },
    KindInfo { key: crate::squadplugin::SQUADS, one: "squad", many: "squads", list: true },
    KindInfo { key: crate::squadplugin::AGENTS, one: "agent", many: "agents", list: true },
];

/// The keys of `plugin.json` that aren't kinds.
const FIELDS: &[&str] = &["id", "name", "description", "version", "author", "license", "platoon"];

/// This platoon's version, as plugins' `platoon` field is compared with.
pub fn platoon_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ---- plugin.json --------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub license: Option<String>,
    /// The oldest platoon it works with.
    pub platoon: String,
    /// What it adds: (kind key, the files named), in `KINDS` order, only the kinds it has.
    pub adds: Vec<(&'static str, Vec<String>)>,
    /// platoon's code editor, when it adds that: its download table and settings files (also in `adds`).
    pub editor: Option<crate::editorplugin::Spec>,
    /// Keys this platoon doesn't know (a later kind, a note): left alone.
    pub unknown: Vec<String>,
}

impl Manifest {
    pub fn files(&self, kind: &str) -> &[String] {
        self.adds.iter().find(|(k, _)| *k == kind).map_or(&[], |(_, f)| f.as_slice())
    }

    pub fn themes(&self) -> &[String] {
        self.files("themes")
    }

    /// The names its themes are shown by (their file names).
    pub fn theme_names(&self) -> Vec<String> {
        self.themes().iter().map(|f| theme_name(f)).collect()
    }

    /// "2 themes", "1 theme" (joined with commas when there are several kinds); "nothing" when empty.
    pub fn adds_words(&self) -> String {
        let parts: Vec<String> = KINDS
            .iter()
            .filter_map(|k| {
                let n = self.files(k.key).len();
                if !k.list {
                    return (n > 0).then(|| k.one.to_string());
                }
                (n > 0).then(|| format!("{n} {}", if n == 1 { k.one } else { k.many }))
            })
            .collect();
        if parts.is_empty() { "nothing".into() } else { parts.join(", ") }
    }

    /// It adds instructions Claude follows with the user's permissions (skills, squads or agents): installing it, or
    /// an update that changes them, shows them to be read first.
    pub fn has_instructions(&self) -> bool {
        [crate::skillplugin::KIND, crate::squadplugin::SQUADS, crate::squadplugin::AGENTS].iter().any(|k| !self.files(k).is_empty())
    }

    /// Why this platoon can't use it, if it can't: it needs a newer one.
    pub fn too_new(&self) -> Option<String> {
        too_new_for(&self.platoon, platoon_version())
    }
}

fn too_new_for(needs: &str, current: &str) -> Option<String> {
    match (parse_version(needs), parse_version(current)) {
        (Some(n), Some(c)) if n > c => Some(format!("it needs platoon {needs} or newer (this is {current})")),
        _ => None,
    }
}

/// `1`, `1.2` or `1.2.3` (numbers only), as three numbers to compare.
pub fn parse_version(s: &str) -> Option<[u64; 3]> {
    let parts: Vec<&str> = s.trim().split('.').collect();
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let mut out = [0u64; 3];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p.parse().ok()?;
    }
    Some(out)
}

/// A plugin's id: lowercase letters, digits and dashes, starting with a letter, at most 40 characters. It is the
/// plugin's folder name here and in the catalog, so it can't be anything a file system or a shell finds odd.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && !id.ends_with('-')
        && !id.contains("--")
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Text from a manifest, safe to print: control characters and the invisible ones that reorder text become spaces,
/// runs of space are folded.
fn clean(s: &str) -> String {
    let odd = |c: char| c.is_control() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}');
    s.chars().map(|c| if odd(c) { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("plugin.json isn't valid JSON ({e})"))?;
    let obj = v.as_object().ok_or("plugin.json must be a JSON object")?;
    let text = |key: &str, max: usize, required: bool| -> Result<Option<String>, String> {
        match obj.get(key) {
            None | Some(Value::Null) if !required => Ok(None),
            None | Some(Value::Null) => Err(format!("plugin.json has no \"{key}\"")),
            Some(Value::String(s)) => {
                let s = clean(s);
                if s.is_empty() && required {
                    Err(format!("plugin.json's \"{key}\" is empty"))
                } else if s.chars().count() > max {
                    Err(format!("plugin.json's \"{key}\" is longer than {max} characters"))
                } else {
                    Ok(Some(s).filter(|s| !s.is_empty()))
                }
            }
            Some(_) => Err(format!("plugin.json's \"{key}\" must be text")),
        }
    };
    let need = |key: &str, max: usize| text(key, max, true).map(Option::unwrap_or_default);
    let id = need("id", MAX_ID)?;
    if !valid_id(&id) {
        return Err(format!("the id \"{id}\" isn't allowed: use lowercase letters, digits and dashes, starting with a letter"));
    }
    let name = need("name", MAX_NAME)?;
    let description = need("description", MAX_DESCRIPTION)?;
    let version = need("version", 20)?;
    if parse_version(&version).is_none() {
        return Err(format!("the version \"{version}\" isn't a version like 1.0.0"));
    }
    let author = need("author", MAX_AUTHOR)?;
    let license = text("license", MAX_LICENSE, false)?;
    let platoon = need("platoon", 20)?;
    if parse_version(&platoon).is_none() {
        return Err(format!("\"platoon\" should be the oldest platoon version it works with, like {}; it is \"{platoon}\"", platoon_version()));
    }
    let mut adds = Vec::new();
    let mut editor = None;
    for kind in KINDS {
        match obj.get(kind.key) {
            None => {}
            Some(v) if !kind.list => {
                let spec = crate::editorplugin::parse_spec(v)?;
                adds.push((kind.key, spec.files()));
                editor = Some(spec);
            }
            Some(Value::Array(items)) => {
                let mut files = Vec::new();
                for item in items {
                    let Some(s) = item.as_str() else { return Err(format!("\"{}\" must be a list of file names", kind.key)) };
                    files.push(s.to_string());
                }
                if !files.is_empty() {
                    adds.push((kind.key, files));
                }
            }
            Some(_) => return Err(format!("\"{}\" must be a list of file names", kind.key)),
        }
    }
    let unknown = obj.keys().filter(|k| !FIELDS.contains(&k.as_str()) && !KINDS.iter().any(|kind| kind.key == k.as_str())).map(|k| clean(k)).collect();
    Ok(Manifest { id, name, description, version, author, license, platoon, adds, editor, unknown })
}

fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let path = dir.join(MANIFEST);
    let meta = fs::symlink_metadata(&path).map_err(|_| "there is no plugin.json".to_string())?;
    if !meta.is_file() {
        return Err("plugin.json isn't a plain file".into());
    }
    if meta.len() > MAX_MANIFEST_BYTES {
        return Err(format!("plugin.json is bigger than {} KiB", MAX_MANIFEST_BYTES / 1024));
    }
    let text = fs::read_to_string(&path).map_err(|_| "plugin.json couldn't be read as text".to_string())?;
    parse_manifest(&text)
}

// ---- the files of a plugin ----------------------------------------------------------------------------------------

/// A file of a plugin: its path inside the folder (with `/`), where it is, its size.
#[derive(Clone, Debug)]
struct FileEntry {
    rel: String,
    path: PathBuf,
    len: u64,
}

/// Every plain file of `root`, by path. Hidden files and folders (`.git`, `.DS_Store`) are not part of a plugin and
/// are left out; a link or anything else that isn't a plain file or a folder is a problem (a plugin is plain files).
fn walk(root: &Path, problems: &mut Vec<String>) -> Vec<FileEntry> {
    fn go(root: &Path, dir: &Path, depth: usize, out: &mut Vec<FileEntry>, problems: &mut Vec<String>, seen: &mut usize) {
        let Ok(entries) = fs::read_dir(dir) else {
            problems.push(format!("{} couldn't be read", rel_of(root, dir)));
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            *seen += 1;
            if *seen > MAX_FILES * 2 {
                return;
            }
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = e.path();
            let rel = rel_of(root, &path);
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            if meta.file_type().is_symlink() {
                problems.push(format!("{rel} is a link; a plugin holds plain files only"));
            } else if meta.is_dir() {
                if depth >= MAX_DEPTH {
                    problems.push(format!("{rel} is nested too deep (at most {MAX_DEPTH} folders)"));
                } else {
                    go(root, &path, depth + 1, out, problems, seen);
                }
            } else if meta.is_file() {
                out.push(FileEntry { rel, path, len: meta.len() });
            } else {
                problems.push(format!("{rel} isn't a plain file"));
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = 0;
    go(root, root, 0, &mut out, problems, &mut seen);
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

fn rel_of(root: &Path, p: &Path) -> String {
    let r = p.strip_prefix(root).unwrap_or(p).to_string_lossy().into_owned();
    if r.is_empty() { ".".into() } else { r }
}

/// One sha256 for a plugin's files: of each file's path and sha256, in path order. Two folders with the same files
/// have the same one, wherever they are.
fn tree_hash(files: &[FileEntry]) -> io::Result<String> {
    let mut list = String::new();
    for f in files {
        list.push_str(&format!("{}\0{}\n", f.rel, sha256_file(&f.path)?));
    }
    Ok(sha256_hex(list.as_bytes()))
}

/// A file name from `plugin.json` that can only mean a file inside the plugin: relative, no `..`, no hidden parts,
/// forward slashes.
fn safe_rel(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= MAX_PATH
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.chars().any(char::is_control)
        && p.split('/').all(|part| !part.is_empty() && !part.starts_with('.'))
}

// ---- checking a plugin --------------------------------------------------------------------------------------------

/// What `check` found in a plugin folder.
#[derive(Clone, Debug, Default)]
pub struct CheckReport {
    pub manifest: Option<Manifest>,
    /// Each is a reason it can't be installed, in plain words.
    pub problems: Vec<String>,
    /// Worth knowing, but fine.
    pub notes: Vec<String>,
    /// The names of the themes it adds, in its order.
    pub themes: Vec<String>,
    /// What installing the code editor it adds downloads on this computer, in bytes.
    pub download_bytes: Option<u64>,
    /// Its skills, squads and agents that passed (squads and agents by the short ids of their files).
    pub skills: Vec<crate::skillplugin::Skill>,
    pub squads: Vec<crate::squadfile::Squad>,
    pub agents: Vec<crate::agentfile::Agent>,
    pub files: usize,
    pub bytes: u64,
}

impl CheckReport {
    pub fn ok(&self) -> bool {
        self.problems.is_empty() && self.manifest.is_some()
    }
}

/// Everything a plugin folder must be to be installed (and to be accepted into the catalog): a readable manifest,
/// the files it names there, plain files only, not too big, made for this platoon, and every theme parses and reads
/// well: text and the five colors that carry meaning at platoon's contrast floors, those five told apart. (A user's
/// own theme file is lifted until it reads; a plugin's is held to the bar the built-ins meet as they are.)
pub fn check(dir: &Path) -> CheckReport {
    let mut r = CheckReport::default();
    if !dir.is_dir() {
        r.problems.push(format!("there is no folder at {}", dir.display()));
        return r;
    }
    let files = walk(dir, &mut r.problems);
    r.files = files.len();
    r.bytes = files.iter().map(|f| f.len).sum();
    if r.files > MAX_FILES {
        r.problems.push(format!("it has {} files; a plugin has at most {MAX_FILES}", r.files));
    }
    if r.bytes > MAX_PLUGIN_BYTES {
        r.problems.push(format!("it is {} KiB; a plugin is at most {} KiB", r.bytes.div_ceil(1024), MAX_PLUGIN_BYTES / 1024));
    }
    let m = match read_manifest(dir) {
        Ok(m) => m,
        Err(e) => {
            r.problems.push(e);
            return r;
        }
    };
    if let Some(why) = m.too_new() {
        r.problems.push(format!("{} can't be used here: {why}", m.name));
    }
    if m.adds.is_empty() {
        r.problems.push("it adds nothing: list what it adds in plugin.json (\"themes\", \"skills\", \"squads\", \"agents\", or the editor's files under \"editor\")".into());
    }
    let rel: Vec<(String, PathBuf, u64)> = files.iter().map(|f| (f.rel.clone(), f.path.clone(), f.len)).collect();
    if let Some(spec) = &m.editor {
        let (problems, bytes) = crate::editorplugin::check_spec(spec, &rel, safe_rel);
        r.problems.extend(problems);
        r.download_bytes = bytes;
    }
    let (problems, skills) = crate::skillplugin::check(&m.id, m.files(crate::skillplugin::KIND), &rel, safe_rel);
    r.problems.extend(problems);
    r.skills = skills;
    let (problems, squads, agents) = crate::squadplugin::check(m.files(crate::squadplugin::SQUADS), m.files(crate::squadplugin::AGENTS), &rel, safe_rel);
    r.problems.extend(problems);
    r.squads = squads;
    r.agents = agents;
    for key in &m.unknown {
        r.notes.push(format!("\"{key}\" in plugin.json isn't something this platoon knows; it is left alone"));
    }
    let mut names: Vec<String> = Vec::new();
    for file in m.themes() {
        if !safe_rel(file) {
            r.problems.push(format!("\"{}\" isn't a file name inside the plugin (no leading /, no .., no hidden parts)", clean(file)));
            continue;
        }
        let Some(entry) = files.iter().find(|f| f.rel == *file) else {
            r.problems.push(format!("{file} is listed in plugin.json but isn't in the folder"));
            continue;
        };
        let name = theme_name(file);
        if name.is_empty() {
            r.problems.push(format!("{file}: the file name has nothing to show as the theme's name"));
            continue;
        }
        if let Some(first) = names.iter().find(|n| n.to_lowercase() == name.to_lowercase()) {
            r.problems.push(format!("{file}: the name {name} is already used by another theme of this plugin ({first})"));
            continue;
        }
        let parsed = usertheme::read_theme_text(&entry.path, entry.len).and_then(|t| usertheme::parse(&t));
        let problems = match &parsed {
            Ok(p) => theme_problems(p),
            Err(e) => vec![e.clone()],
        };
        if problems.is_empty() {
            if let Err(e) = parsed.and_then(|p| usertheme::theme_from(String::new(), name.clone(), &p)) {
                r.problems.push(format!("{file}: {e}"));
                continue;
            }
            names.push(name.clone());
            r.themes.push(name);
        } else {
            r.problems.extend(problems.into_iter().map(|p| format!("{file}: {p}")));
        }
    }
    r.manifest = Some(m);
    r
}

/// A theme's name: its file's name, as for the user's own theme files.
fn theme_name(file: &str) -> String {
    usertheme::display_name(OsStr::new(file.rsplit('/').next().unwrap_or(file)))
}

/// Why a theme file doesn't read well enough to be offered, in plain words (empty: it does). The same floors the
/// built-ins are held to, on the file's own colors, before any lifting.
pub fn theme_problems(p: &usertheme::Parsed) -> Vec<String> {
    let mut out = Vec::new();
    let (bg, fg) = (p.background, p.foreground);
    if luminance(bg) >= 0.2 {
        out.push("it is a light theme; platoon's themes are dark".into());
        return out;
    }
    let ratio = |c| contrast(c, bg);
    if ratio(fg) < MIN_TEXT {
        out.push(format!("the text is hard to read on the background ({:.1}:1; it needs {MIN_TEXT}:1)", ratio(fg)));
    }
    let roles = [("accent", 12, 4), ("busy", 13, 5), ("ok", 10, 2), ("warning", 11, 3), ("error", 9, 1)];
    let mut picked = Vec::new();
    for (role, hi, lo) in roles {
        match p.palette[hi].or(p.palette[lo]) {
            None => out.push(format!("palette {hi} (or {lo}) is missing: it is the {role} color")),
            Some(c) => {
                if ratio(c) < MIN_ROLE {
                    out.push(format!("the {role} color is hard to read on the background ({:.1}:1; it needs {MIN_ROLE}:1)", ratio(c)));
                }
                picked.push((role, c));
            }
        }
    }
    for (i, (a, ca)) in picked.iter().enumerate() {
        for (b, cb) in &picked[i + 1..] {
            if delta_e(*ca, *cb) < MIN_DELTA_E {
                out.push(format!("the {a} and {b} colors look too alike"));
            }
        }
    }
    out
}

// ---- installed plugins --------------------------------------------------------------------------------------------

/// Where an installed plugin came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The official catalog: the repository it was fetched from, the ref and the commit.
    Catalog { source: String, rf: String, commit: String },
    /// A folder on this computer (someone making a plugin).
    Folder { path: String },
    /// The copy of it that came with this platoon (the code editor, taken over from an install made before it was a
    /// plugin). Updated from the catalog like one from there.
    Bundled { version: String },
}

impl Origin {
    /// "the catalog (commit 1a2b3c4)", "a folder: ~/my-plugin".
    pub fn words(&self, home: &str) -> String {
        match self {
            Origin::Catalog { commit, .. } => format!("the catalog (commit {})", short(commit)),
            Origin::Folder { path } => format!("a folder: {}", crate::dirs::tilde(path, home)),
            Origin::Bundled { version } => format!("the copy that came with platoon {version}"),
        }
    }
}

pub fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub manifest: Manifest,
    pub origin: Origin,
    /// Of its files (`tree_hash`), as installed.
    pub sha256: String,
    /// Seconds since the epoch.
    pub installed_at: u64,
    /// The skills platoon wrote into Claude Code's skills folder for it (and the changed copies it kept).
    pub skills: Vec<crate::skillplugin::Placed>,
}

impl Installed {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }
}

/// What an install did.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// It wasn't there before.
    New(Installed),
    /// It replaced this version.
    Replaced { now: Installed, was: String },
    /// The same files from the same place were already installed: nothing changed.
    Same(Installed),
}

impl Outcome {
    pub fn plugin(&self) -> &Installed {
        match self {
            Outcome::New(p) | Outcome::Same(p) | Outcome::Replaced { now: p, .. } => p,
        }
    }
}

/// The installed plugins, in `<data>/plugins`.
#[derive(Clone, Debug)]
pub struct Store {
    pub dir: PathBuf,
    /// Claude Code's config folder, where a plugin's skills are written (`<it>/skills/<name>`). None: a plugin with
    /// skills can't be installed (a test's store that isn't given one never reaches the user's).
    pub claude: Option<PathBuf>,
    /// Make the record fail to write after the files are in place (tests of a failed install).
    #[cfg(test)]
    pub fail_record: bool,
}

/// One change at a time: an exclusive lock on `.lock`, let go when dropped.
struct Lock(#[allow(dead_code)] fs::File);

static NEXT: AtomicU64 = AtomicU64::new(0);

fn unique() -> String {
    format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst))
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Write `text` to `path` in one step: a temp file beside it, then a rename.
fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let tmp = path.with_file_name(format!(".{}.{}.tmp", path.file_name().and_then(OsStr::to_str).unwrap_or("file"), unique()));
    let done = fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, path));
    if done.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    done
}

impl Store {
    pub fn new(dir: PathBuf) -> Store {
        Store {
            dir,
            claude: None,
            #[cfg(test)]
            fail_record: false,
        }
    }

    /// The same, writing skills into Claude Code's config folder `claude`.
    pub fn with_claude(dir: PathBuf, claude: PathBuf) -> Store {
        Store { claude: Some(claude), ..Store::new(dir) }
    }

    /// `<data>/plugins`: `PLATOON_DATA_DIR`, else the platform's data folder (as the code editor), with Claude Code's
    /// config folder (`CLAUDE_CONFIG_DIR`, else `~/.claude`) for skills.
    pub fn from_env() -> Store {
        Store::with_claude(crate::editor_install::data_dir().join("plugins"), crate::claude::claude_root())
    }

    pub fn record_path(&self) -> PathBuf {
        self.dir.join("installed.json")
    }

    fn tmp(&self) -> PathBuf {
        self.dir.join(".tmp")
    }

    fn catalog_dir(&self) -> PathBuf {
        self.dir.join(".catalog")
    }

    fn catalog_record(&self) -> PathBuf {
        self.dir.join(".catalog.json")
    }

    /// An installed plugin's folder.
    pub fn plugin_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    fn lock(&self) -> Result<Lock, String> {
        fs::create_dir_all(&self.dir).map_err(|e| format!("couldn't make {}: {e}", self.dir.display()))?;
        let f = fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.dir.join(".lock")).map_err(|e| e.to_string())?;
        let start = Instant::now();
        loop {
            // SAFETY: flock on a descriptor this function owns; the lock goes with the file when `Lock` is dropped.
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Lock(f));
            }
            if start.elapsed() > Duration::from_secs(3) {
                return Err("Another platoon is installing or removing a plugin right now; try again when it has finished.".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// `installed.json` as it is: an empty record when there is none; an error when it can't be read (then nothing
    /// is written over it).
    fn read_record(&self) -> Result<Map<String, Value>, String> {
        let path = self.record_path();
        match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(o)) => Ok(o),
                _ => Err(format!("{} can't be read, so platoon won't change it; fix it or remove it", path.display())),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
            Err(e) => Err(format!("{} can't be read ({e})", path.display())),
        }
    }

    fn write_record(&self, rec: &Map<String, Value>) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_record {
            return Err("the record couldn't be written (a test said so)".into());
        }
        let text = serde_json::to_string_pretty(&Value::Object(rec.clone())).unwrap_or_default();
        write_atomic(&self.record_path(), &text).map_err(|e| format!("couldn't write {}: {e}", self.record_path().display()))
    }

    /// The installed plugins, by name. One whose folder or manifest is missing, or that `installed.json` doesn't list
    /// (an install that never finished), is not installed.
    pub fn installed(&self) -> Vec<Installed> {
        let Ok(rec) = self.read_record() else { return Vec::new() };
        let Some(plugins) = rec.get("plugins").and_then(Value::as_object) else { return Vec::new() };
        let mut out: Vec<Installed> = plugins
            .iter()
            .filter(|(id, _)| valid_id(id))
            .filter_map(|(id, v)| {
                let manifest = read_manifest(&self.plugin_dir(id)).ok().filter(|m| m.id == *id)?;
                let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
                let origin = match v.get("origin").and_then(Value::as_str) {
                    Some("folder") => Origin::Folder { path: s("path") },
                    Some("bundled") => Origin::Bundled { version: s("platoon") },
                    _ => Origin::Catalog { source: s("source"), rf: s("ref"), commit: s("commit") },
                };
                let skills = v
                    .get("skills")
                    .and_then(Value::as_object)
                    .map(|o| o.iter().filter_map(|(name, p)| crate::skillplugin::Placed::from_json(name, p)).collect())
                    .unwrap_or_default();
                Some(Installed { manifest, origin, sha256: s("sha256"), installed_at: v.get("installed_at").and_then(Value::as_u64).unwrap_or(0), skills })
            })
            .collect();
        out.sort_by(|a, b| a.manifest.name.to_lowercase().cmp(&b.manifest.name.to_lowercase()).then(a.id().cmp(b.id())));
        out
    }

    pub fn get(&self, id: &str) -> Option<Installed> {
        self.installed().into_iter().find(|p| p.id() == id)
    }

    /// Install the plugin in folder `src` (or put this copy in place of the one installed): checked first, then
    /// copied into `.tmp`, the copy checked against the original, moved into place, and only then recorded. Any
    /// failure puts back what was there before and leaves nothing that looks installed.
    pub fn install(&self, src: &Path, origin: Origin) -> Result<Outcome, String> {
        self.install_noting(src, origin).map(|(o, _)| o)
    }

    /// `install`, and what is worth saying besides (a skill the user changed that was kept as it is).
    ///
    /// A plugin's skills are written into Claude Code's skills folder once its files are in place, before it is
    /// recorded; a skill's name taken by something platoon didn't write for this plugin refuses the whole install
    /// before anything is touched (`skillplugin::plan`).
    pub fn install_noting(&self, src: &Path, origin: Origin) -> Result<(Outcome, Vec<String>), String> {
        let report = check(src);
        let Some(m) = report.manifest.clone().filter(|_| report.problems.is_empty()) else {
            let what = report.manifest.as_ref().map_or_else(|| src.display().to_string(), |m| m.name.clone());
            return Err(format!("{what} can't be installed: {}", report.problems.join("; ")));
        };
        let mut ignored = Vec::new();
        let files = walk(src, &mut ignored);
        let hash = tree_hash(&files).map_err(|e| format!("couldn't read the plugin's files: {e}"))?;
        let _lock = self.lock()?;
        let mut rec = self.read_record()?;
        if m.editor.is_some() {
            if let Some(other) = self.installed().into_iter().find(|p| p.manifest.editor.is_some() && p.id() != m.id) {
                return Err(format!("{} can't be installed while {} is: platoon has one code editor. Remove {} first.", m.name, other.manifest.name, other.id()));
            }
        }
        let before = self.get(&m.id);
        if let Some(b) = &before {
            if b.sha256 == hash && b.origin == origin {
                return Ok((Outcome::Same(b.clone()), Vec::new()));
            }
        }
        // Where its skills go, worked out before anything is touched.
        let old_skills = before.as_ref().map(|b| b.skills.clone()).unwrap_or_default();
        let others: Vec<(String, Vec<crate::skillplugin::Placed>)> = self.installed().into_iter().filter(|p| p.id() != m.id).map(|p| (p.manifest.name.clone(), p.skills)).collect();
        let skills_plan = if report.skills.is_empty() && old_skills.is_empty() {
            None
        } else {
            let Some(claude) = self.claude.clone() else {
                return Err(format!("{} can't be installed: it has skills, and platoon doesn't know where Claude Code's folder is", m.name));
            };
            let plan = crate::skillplugin::plan(&claude, &report.skills, &old_skills, &others).map_err(|e| format!("{} can't be installed: {e}", m.name))?;
            Some((claude, plan))
        };
        // Whatever an interrupted install left half done goes; nothing in it was ever recorded.
        let _ = fs::remove_dir_all(self.tmp());
        fs::create_dir_all(self.tmp()).map_err(|e| format!("couldn't make {}: {e}", self.tmp().display()))?;
        let stage = self.tmp().join(format!("{}-{}", m.id, unique()));
        let staged = (|| -> io::Result<String> {
            for f in &files {
                let to = stage.join(&f.rel);
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&f.path, &to)?;
                // Files, never programs: nothing in a plugin is ever made runnable.
                fs::set_permissions(&to, fs::Permissions::from_mode(0o644))?;
            }
            fs::create_dir_all(&stage)?;
            let mut ignored = Vec::new();
            tree_hash(&walk(&stage, &mut ignored))
        })();
        let fail = |why: String| {
            let _ = fs::remove_dir_all(self.tmp());
            Err(why)
        };
        match staged {
            Ok(h) if h == hash => {}
            Ok(_) => return fail(format!("{}'s files changed while they were being copied; try again", m.name)),
            Err(e) => return fail(format!("couldn't copy {}'s files: {e}", m.name)),
        }
        let dest = self.plugin_dir(&m.id);
        let old = self.tmp().join(format!("{}-old-{}", m.id, unique()));
        let had_old = fs::symlink_metadata(&dest).is_ok();
        if had_old {
            if let Err(e) = fs::rename(&dest, &old) {
                return fail(format!("couldn't move the old copy of {} aside: {e}", m.name));
            }
        }
        let put_back = || {
            let _ = fs::remove_dir_all(&dest);
            if had_old {
                let _ = fs::rename(&old, &dest);
            }
        };
        if let Err(e) = fs::rename(&stage, &dest) {
            put_back();
            return fail(format!("couldn't move {} into place: {e}", m.name));
        }
        let mut notes = Vec::new();
        let (placed, txn) = match &skills_plan {
            None => (Vec::new(), None),
            Some((claude, plan)) => match crate::skillplugin::apply(claude, plan, &dest) {
                Ok((placed, txn)) => {
                    notes.extend(plan.notes.clone());
                    (placed, Some(txn))
                }
                Err(e) => {
                    put_back();
                    return fail(format!("{} couldn't be installed: {e}", m.name));
                }
            },
        };
        let entry = match &origin {
            Origin::Catalog { source, rf, commit } => json!({"origin": "catalog", "source": source, "ref": rf, "commit": commit}),
            Origin::Folder { path } => json!({"origin": "folder", "path": path}),
            Origin::Bundled { version } => json!({"origin": "bundled", "platoon": version}),
        };
        let mut entry = entry.as_object().cloned().unwrap_or_default();
        entry.insert("version".into(), json!(m.version));
        entry.insert("sha256".into(), json!(hash));
        entry.insert("files".into(), json!(files.len()));
        entry.insert("installed_at".into(), json!(now()));
        if !placed.is_empty() {
            let skills: Map<String, Value> = placed.iter().map(|p| (p.name.clone(), p.to_json())).collect();
            entry.insert("skills".into(), Value::Object(skills));
        }
        if !rec.get("plugins").is_some_and(Value::is_object) {
            rec.insert("plugins".into(), json!({}));
        }
        if let Some(plugins) = rec.get_mut("plugins").and_then(Value::as_object_mut) {
            // Keys a later platoon added to this plugin's record are kept.
            let mut merged = plugins.get(&m.id).and_then(Value::as_object).cloned().unwrap_or_default();
            for key in ["origin", "source", "ref", "commit", "path", "platoon", "skills"] {
                merged.remove(key);
            }
            merged.extend(entry);
            plugins.insert(m.id.clone(), Value::Object(merged));
        }
        if let Err(e) = self.write_record(&rec) {
            if let Some(t) = txn {
                t.rollback();
            }
            put_back();
            return fail(e);
        }
        if let Some(t) = txn {
            t.commit();
        }
        let _ = fs::remove_dir_all(self.tmp());
        let now = self.get(&m.id).ok_or_else(|| format!("{} was installed but can't be read back", m.name))?;
        let outcome = match before {
            Some(b) => Outcome::Replaced { now, was: b.manifest.version },
            None => Outcome::New(now),
        };
        Ok((outcome, notes))
    }

    /// Take a plugin off: un-recorded first, then its folder goes. `None` when it wasn't installed.
    pub fn remove(&self, id: &str) -> Result<Option<Installed>, String> {
        self.remove_noting(id).map(|(p, _)| p)
    }

    /// `remove`, and what is worth saying besides: its skills leave Claude Code's skills folder too, but one the user
    /// changed is kept, and said.
    pub fn remove_noting(&self, id: &str) -> Result<(Option<Installed>, Vec<String>), String> {
        if !valid_id(id) {
            return Ok((None, Vec::new()));
        }
        let _lock = self.lock()?;
        let was = self.get(id);
        let mut rec = self.read_record()?;
        let listed = rec.get("plugins").and_then(Value::as_object).is_some_and(|p| p.contains_key(id));
        let dest = self.plugin_dir(id);
        if !listed && fs::symlink_metadata(&dest).is_err() {
            return Ok((None, Vec::new()));
        }
        if listed {
            if let Some(plugins) = rec.get_mut("plugins").and_then(Value::as_object_mut) {
                plugins.remove(id);
            }
            self.write_record(&rec)?;
        }
        if fs::symlink_metadata(&dest).is_ok() {
            let gone = self.dir.join(format!(".gone-{id}-{}", unique()));
            match fs::rename(&dest, &gone) {
                Ok(()) => {
                    let _ = fs::remove_dir_all(&gone);
                }
                Err(_) => {
                    let _ = fs::remove_dir_all(&dest);
                }
            }
        }
        let notes = was.as_ref().map(|p| crate::skillplugin::take_away(&p.skills)).unwrap_or_default();
        Ok((was, notes))
    }
}

// ---- the catalog --------------------------------------------------------------------------------------------------

/// Where the catalog is fetched from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// A git URL, or a folder on this computer (a repository).
    pub url: String,
    /// A branch or a tag.
    pub rf: String,
}

impl Source {
    /// platoon's own repository at `main`, unless `PLATOON_PLUGIN_SOURCE` / `PLATOON_PLUGIN_REF` say otherwise.
    pub fn from_env() -> Source {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Source { url: env("PLATOON_PLUGIN_SOURCE").unwrap_or_else(|| DEFAULT_SOURCE.into()), rf: env("PLATOON_PLUGIN_REF").unwrap_or_else(|| DEFAULT_REF.into()) }
    }

    /// What git is given: a folder as a `file://` URL (so a shallow clone is shallow there too).
    fn git_url(&self) -> String {
        let u = self.url.trim();
        if u.contains("://") || (u.contains('@') && u.contains(':')) {
            return u.to_string();
        }
        let p = Path::new(u);
        let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(p) };
        format!("file://{}", abs.display())
    }
}

/// A plugin the catalog offers.
#[derive(Clone, Debug)]
pub struct Entry {
    pub manifest: Manifest,
    /// Its folder in the fetched catalog.
    pub dir: PathBuf,
    pub sha256: String,
    /// Why it can't be installed here (it needs a newer platoon, ...); empty when it can.
    pub problems: Vec<String>,
    /// What installing the code editor it adds downloads on this computer.
    pub download_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Catalog {
    pub source: String,
    pub rf: String,
    pub commit: String,
    pub entries: Vec<Entry>,
    /// Listed in the index but not usable, and why: "foo: plugin.json has no \"version\"".
    pub skipped: Vec<String>,
}

impl Catalog {
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.manifest.id == id)
    }
}

/// Run git by `deadline`, with no way to ask for a password. Its output, or what it said went wrong.
fn git(dir: &Path, args: &[&str], deadline: Instant) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err("git isn't installed".into()),
        Err(e) => return Err(format!("git couldn't start ({e})")),
    };
    let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
    let read_out = thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.take(1 << 20).read_to_end(&mut s);
        }
        s
    });
    let read_err = thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(e) = err.as_mut() {
            let _ = e.take(1 << 16).read_to_end(&mut s);
        }
        s
    });
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = String::from_utf8_lossy(&read_out.join().unwrap_or_default()).into_owned();
                let err = String::from_utf8_lossy(&read_err.join().unwrap_or_default()).into_owned();
                return if status.success() { Ok(out) } else { Err(git_error(&err)) };
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                // Out of time: this is our own child, and only it is ended.
                let _ = child.kill();
                let _ = child.wait();
                return Err("it took too long (no answer from the network?)".into());
            }
        }
    }
}

/// What git said went wrong, in a few plain words.
fn git_error(stderr: &str) -> String {
    let low = stderr.to_lowercase();
    if low.contains("could not read username") || low.contains("terminal prompts disabled") || low.contains("authentication failed") {
        return "git needs a login for it and none is saved (the catalog is private for now)".into();
    }
    if low.contains("could not resolve host") || low.contains("unable to access") && low.contains("resolve") {
        return "no network connection (the address couldn't be found)".into();
    }
    if low.contains("remote branch") && low.contains("not found") {
        return "the catalog's branch or tag wasn't found".into();
    }
    if low.contains("does not appear to be a git repository") || low.contains("repository") && low.contains("not found") {
        return "there is no repository there".into();
    }
    let line = stderr.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("warning:") && !l.starts_with("hint:")).last().unwrap_or("git failed");
    let line = line.strip_prefix("fatal: ").unwrap_or(line);
    clean(line).chars().take(160).collect()
}

/// Fetch the catalog from `source` (a shallow, sparse clone of its `plugins/` folder at the ref), keep it as this
/// computer's copy of the catalog, and read it.
pub fn fetch(store: &Store, source: &Source) -> Result<Catalog, String> {
    let say = |why: String| format!("platoon couldn't reach its plugin catalog: {why}");
    let work = store.dir.join(".fetch").join(unique());
    fs::create_dir_all(&work).map_err(|e| say(format!("couldn't make {}: {e}", work.display())))?;
    let result = (|| -> Result<Catalog, String> {
        let deadline = Instant::now() + FETCH_TIMEOUT;
        let url = source.git_url();
        git(&work, &["clone", "--quiet", "--depth", "1", "--filter=blob:none", "--sparse", "--no-checkout", "--single-branch", "--branch", &source.rf, "--", &url, "repo"], deadline).map_err(say)?;
        let repo = work.join("repo");
        git(&repo, &["sparse-checkout", "set", "--no-cone", &format!("/{CATALOG_FOLDER}/")], deadline).map_err(say)?;
        git(&repo, &["checkout", "--quiet", &source.rf], deadline).map_err(say)?;
        let commit = git(&repo, &["rev-parse", "HEAD"], deadline).map_err(say)?.trim().to_string();
        let got = repo.join(CATALOG_FOLDER);
        if !got.is_dir() {
            return Err(format!("platoon's plugin catalog isn't there yet: its repository has no {CATALOG_FOLDER} folder on {}", source.rf));
        }
        let _lock = store.lock()?;
        let next = store.dir.join(format!(".catalog-new-{}", unique()));
        fs::rename(&got, &next).map_err(|e| format!("couldn't keep the catalog: {e}"))?;
        let old = store.dir.join(format!(".catalog-old-{}", unique()));
        let had = store.catalog_dir().exists();
        if had {
            let _ = fs::rename(store.catalog_dir(), &old);
        }
        if let Err(e) = fs::rename(&next, store.catalog_dir()) {
            if had {
                let _ = fs::rename(&old, store.catalog_dir());
            }
            let _ = fs::remove_dir_all(&next);
            return Err(format!("couldn't keep the catalog: {e}"));
        }
        let _ = fs::remove_dir_all(&old);
        let rec = json!({"source": source.url, "ref": source.rf, "commit": commit, "fetched_at": now()});
        write_atomic(&store.catalog_record(), &serde_json::to_string_pretty(&rec).unwrap_or_default()).map_err(|e| format!("couldn't keep the catalog: {e}"))?;
        drop(_lock);
        cached(store, source).ok_or_else(|| "the catalog couldn't be read after fetching it".to_string())
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

/// The catalog as last fetched from `source`, without going to the network. None if it never was.
pub fn cached(store: &Store, source: &Source) -> Option<Catalog> {
    let rec: Value = serde_json::from_str(&fs::read_to_string(store.catalog_record()).ok()?).ok()?;
    let s = |k: &str| rec.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    if s("source") != source.url || s("ref") != source.rf {
        return None;
    }
    let (entries, skipped) = read_catalog(&store.catalog_dir());
    Some(Catalog { source: s("source"), rf: s("ref"), commit: s("commit"), entries, skipped })
}

/// The plugins a catalog folder offers: those its `index.json` lists (in its order), each read from its folder.
/// One whose folder, manifest or id doesn't match the index is left out, with the reason.
pub fn read_catalog(dir: &Path) -> (Vec<Entry>, Vec<String>) {
    let (mut entries, mut skipped) = (Vec::new(), Vec::new());
    let index: Value = match fs::read_to_string(dir.join(INDEX)).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(v) => v,
        None => return (entries, vec!["the catalog has no readable index.json".into()]),
    };
    let listed = index.get("plugins").and_then(Value::as_array).cloned().unwrap_or_default();
    for item in listed {
        let Some(id) = item.get("id").and_then(Value::as_str) else { continue };
        if !valid_id(id) || entries.iter().any(|e: &Entry| e.manifest.id == id) {
            skipped.push(format!("{}: not a usable id", clean(id)));
            continue;
        }
        let folder = dir.join(id);
        let report = check(&folder);
        let Some(m) = report.manifest.clone() else {
            skipped.push(format!("{id}: {}", report.problems.join("; ")));
            continue;
        };
        if m.id != id {
            skipped.push(format!("{id}: its plugin.json says its id is {}", m.id));
            continue;
        }
        if item.get("version").and_then(Value::as_str).is_some_and(|v| v != m.version) {
            skipped.push(format!("{id}: index.json and plugin.json give different versions"));
            continue;
        }
        let mut ignored = Vec::new();
        let sha256 = tree_hash(&walk(&folder, &mut ignored)).unwrap_or_default();
        entries.push(Entry { manifest: m, dir: folder, sha256, problems: report.problems, download_bytes: report.download_bytes });
    }
    (entries, skipped)
}

/// What `update` did for one plugin.
#[derive(Clone, Debug, PartialEq)]
pub enum Updated {
    /// Replaced with the catalog's: from this version to that (and what is worth saying besides: a skill the user
    /// changed, kept as it is).
    Done { name: String, from: String, to: String, notes: Vec<String> },
    UpToDate { name: String },
    /// Installed from a folder: add it again to update it.
    FromFolder { name: String },
    /// The catalog doesn't have it any more (it stays installed).
    Gone { name: String },
    Failed { name: String, why: String },
}

/// Bring installed plugins up to the catalog: each (or the one named) that came from the catalog and whose files
/// there are different now is installed again.
pub fn update(store: &Store, catalog: &Catalog, only: Option<&str>) -> Vec<Updated> {
    let mut out = Vec::new();
    for p in store.installed().into_iter().filter(|p| only.is_none_or(|id| p.id() == id)) {
        let name = p.manifest.name.clone();
        if matches!(p.origin, Origin::Folder { .. }) {
            out.push(Updated::FromFolder { name });
            continue;
        }
        let Some(e) = catalog.get(p.id()) else {
            out.push(Updated::Gone { name });
            continue;
        };
        if e.sha256 == p.sha256 {
            out.push(Updated::UpToDate { name });
            continue;
        }
        let origin = Origin::Catalog { source: catalog.source.clone(), rf: catalog.rf.clone(), commit: catalog.commit.clone() };
        match store.install_noting(&e.dir, origin) {
            Ok((o, notes)) => out.push(Updated::Done { name, from: p.manifest.version.clone(), to: o.plugin().manifest.version.clone(), notes }),
            Err(why) => out.push(Updated::Failed { name, why }),
        }
    }
    out
}

/// The catalog's version of an installed plugin is different from what is installed.
pub fn update_available(p: &Installed, catalog: &Catalog) -> bool {
    matches!(p.origin, Origin::Catalog { .. } | Origin::Bundled { .. }) && catalog.get(p.id()).is_some_and(|e| e.sha256 != p.sha256 && e.problems.is_empty())
}

// ---- themes from plugins ------------------------------------------------------------------------------------------

/// A plugin's theme: the theme, and the name of the plugin it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginTheme {
    pub theme: Theme,
    pub plugin: String,
}

/// The themes of every installed plugin (by plugin name, then in each plugin's order), ids `plugin:<id>/<slug>`.
/// A name one of `taken` already has (a built-in's, one of the user's) is shown with its plugin's name after it.
/// A file that doesn't load is left out (it was checked when it was installed).
pub fn themes(store: &Store, taken: &[String]) -> Vec<PluginTheme> {
    let mut out: Vec<PluginTheme> = Vec::new();
    for p in store.installed() {
        let dir = store.plugin_dir(p.id());
        for file in p.manifest.themes() {
            if !safe_rel(file) {
                continue;
            }
            let path = dir.join(file);
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            if !meta.is_file() {
                continue;
            }
            let name = theme_name(file);
            let id = format!("{THEME_PREFIX}{}/{}", p.id(), usertheme::slug(&name));
            if name.is_empty() || out.iter().any(|t| t.theme.id == id) {
                continue;
            }
            let lower = name.to_lowercase();
            let clash = taken.iter().any(|t| t.to_lowercase() == lower) || out.iter().any(|t| t.theme.name.to_lowercase() == lower);
            let shown = if clash { format!("{name} ({})", p.manifest.name) } else { name };
            let Ok(theme) = usertheme::read_theme_text(&path, meta.len()).and_then(|t| usertheme::parse(&t)).and_then(|parsed| usertheme::theme_from(id, shown, &parsed)) else {
                continue;
            };
            out.push(PluginTheme { theme, plugin: p.manifest.name.clone() });
        }
    }
    out
}

/// What to do with a plugin once it is installed, in a sentence or two (its themes, squads and agents, skills).
pub fn next_words(m: &Manifest) -> String {
    let mut out: Vec<String> = Vec::new();
    if !m.themes().is_empty() {
        out.push("Pick a theme in the settings (s, then Theme).".into());
    }
    if !m.files(crate::squadplugin::SQUADS).is_empty() {
        out.push("Its squads are on the Squads tab (3), and a ticket can be handed to them (a).".into());
    } else if !m.files(crate::squadplugin::AGENTS).is_empty() {
        out.push("Its agents are on the Squads tab (3), for your squads' steps.".into());
    }
    let skills: Vec<String> = m.files(crate::skillplugin::KIND).iter().map(|f| format!("/{}", f.trim_end_matches('/').rsplit('/').next().unwrap_or(f))).collect();
    match skills.as_slice() {
        [] => {}
        [one] => out.push(format!("Its skill is in Claude Code's skills folder: type {one} in a new Claude session.")),
        _ => out.push(format!("Its skills are in Claude Code's skills folder: {} in a new Claude session.", skills.join(", "))),
    }
    out.join(" ")
}

/// Where platoon keeps plugins, seen from its config folder: the store (as `Store::from_env`). In a unit test, beside
/// the test's config folder (`<its parent>/data/plugins`), never the user's.
pub fn store_for(config: &Path) -> Store {
    #[cfg(test)]
    {
        // A test's config folder straight in the temp folder: its own folder, never a `data` shared by every test.
        let base = config.parent().filter(|p| *p != std::env::temp_dir()).unwrap_or(config);
        Store::with_claude(base.join("data/plugins"), base.join("claude"))
    }
    #[cfg(not(test))]
    {
        let _ = config;
        Store::from_env()
    }
}

/// What is needed to see that the installed plugins changed: `installed.json`'s size and time.
pub fn fingerprint(store: &Store) -> Option<(u64, Option<std::time::SystemTime>)> {
    fs::metadata(store.record_path()).ok().map(|m| (m.len(), m.modified().ok()))
}

#[cfg(test)]
pub mod testing {
    //! Plugin folders and a catalog repository for tests (here, in the app's tests and in `plugincli`).
    use super::*;
    use std::process::Command;

    pub fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-plugins-{name}-{}-{}", std::process::id(), unique())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A dark theme that meets every floor.
    pub const GOOD_THEME: &str = "background = #14181f\nforeground = #dfe3ea\npalette = 1=#ff7a85\npalette = 2=#8bd49c\npalette = 3=#e6c170\npalette = 4=#7ab4ff\npalette = 5=#c89cf5\npalette = 9=#ff7a85\npalette = 10=#8bd49c\npalette = 11=#e6c170\npalette = 12=#7ab4ff\npalette = 13=#c89cf5\n";

    /// A plugin folder `<base>/<id>` with these themes (file name, text).
    pub fn plugin(base: &Path, id: &str, version: &str, themes: &[(&str, &str)]) -> PathBuf {
        let dir = base.join(id);
        fs::create_dir_all(dir.join("themes")).unwrap();
        let files: Vec<String> = themes.iter().map(|(n, _)| format!("themes/{n}")).collect();
        for (n, text) in themes {
            fs::write(dir.join("themes").join(n), text).unwrap();
        }
        let m = json!({"id": id, "name": format!("{id} name"), "description": format!("What {id} is."), "version": version, "author": "Someone", "license": "MIT", "platoon": "0.1.0", "themes": files});
        fs::write(dir.join(MANIFEST), serde_json::to_string_pretty(&m).unwrap()).unwrap();
        dir
    }

    pub fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A git repository shaped like platoon's: some code outside `plugins/`, and a catalog with these plugins
    /// (id, version, themes). Returns it and the commit.
    pub fn catalog_repo(base: &Path, plugins: &[(&str, &str, &[(&str, &str)])]) -> (PathBuf, String) {
        let repo = base.join("source");
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(repo.join("README.md"), "not the catalog\n").unwrap();
        super::testing::git(&repo, &["init", "--quiet"]);
        let commit = commit_catalog(&repo, plugins);
        (repo, commit)
    }

    /// Replace the catalog in `repo` with these plugins and commit.
    pub fn commit_catalog(repo: &Path, plugins: &[(&str, &str, &[(&str, &str)])]) -> String {
        let cat = repo.join(CATALOG_FOLDER);
        let _ = fs::remove_dir_all(&cat);
        fs::create_dir_all(&cat).unwrap();
        let mut index = Vec::new();
        for (id, version, themes) in plugins {
            plugin(&cat, id, version, themes);
            index.push(json!({"id": id, "version": version}));
        }
        fs::write(cat.join(INDEX), serde_json::to_string_pretty(&json!({"plugins": index})).unwrap()).unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "--quiet", "-m", "catalog"]);
        git(repo, &["rev-parse", "HEAD"])
    }

    pub fn source(repo: &Path) -> Source {
        Source { url: repo.to_string_lossy().into_owned(), rf: "main".into() }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn manifest_json(extra: &str) -> String {
        format!(r#"{{"id": "sea-themes", "name": "Sea themes", "description": "Two themes.", "version": "1.2.0", "author": "A. Person", "platoon": "0.1.0", "themes": ["themes/Deep"]{extra}}}"#)
    }

    #[test]
    fn a_manifest_parses_and_keeps_what_it_does_not_know_aside() {
        let m = parse_manifest(&manifest_json(r#", "license": "MIT", "widgets": ["widgets/x"], "homepage": "somewhere""#)).unwrap();
        assert_eq!((m.id.as_str(), m.name.as_str(), m.version.as_str(), m.platoon.as_str()), ("sea-themes", "Sea themes", "1.2.0", "0.1.0"));
        assert_eq!(m.license.as_deref(), Some("MIT"));
        assert_eq!(m.themes(), ["themes/Deep"]);
        assert_eq!(m.unknown, ["widgets", "homepage"], "later kinds and other keys are left alone, not refused");
        assert_eq!(m.adds_words(), "1 theme");
        let m = parse_manifest(&manifest_json("")).unwrap();
        assert_eq!(m.license, None, "the license is optional");
    }

    #[test]
    fn a_manifest_missing_a_field_or_with_a_wrong_one_says_which() {
        for key in ["id", "name", "description", "version", "author", "platoon"] {
            let mut v: Value = serde_json::from_str(&manifest_json("")).unwrap();
            v.as_object_mut().unwrap().remove(key);
            assert_eq!(parse_manifest(&v.to_string()).unwrap_err(), format!("plugin.json has no \"{key}\""));
        }
        assert!(parse_manifest("{").unwrap_err().starts_with("plugin.json isn't valid JSON"));
        assert_eq!(parse_manifest("[]").unwrap_err(), "plugin.json must be a JSON object");
        let bad = |from: &str, to: &str| parse_manifest(&manifest_json("").replace(from, to)).unwrap_err();
        assert!(bad("\"sea-themes\"", "\"Sea Themes\"").contains("isn't allowed"));
        assert!(bad("\"sea-themes\"", "\"../x\"").contains("isn't allowed"));
        assert!(bad("\"1.2.0\"", "\"1.2-beta\"").contains("isn't a version"));
        assert!(bad("\"0.1.0\"", "\"soon\"").contains("oldest platoon version"));
        assert!(bad("[\"themes/Deep\"]", "\"themes/Deep\"").contains("must be a list"));
        assert!(bad("[\"themes/Deep\"]", "[1]").contains("must be a list"));
        assert!(bad("\"Two themes.\"", "5").contains("must be text"));
        assert!(bad("\"Two themes.\"", &format!("\"{}\"", "x".repeat(500))).contains("longer than"));
        // Control characters never reach the screen.
        let m = parse_manifest(&manifest_json("").replace("Sea themes", "Sea\\u001b[31m themes")).unwrap();
        assert!(!m.name.contains('\x1b'));
    }

    #[test]
    fn versions_compare_and_a_plugin_for_a_newer_platoon_is_refused() {
        assert_eq!(parse_version("1"), Some([1, 0, 0]));
        assert_eq!(parse_version("0.12.3"), Some([0, 12, 3]));
        for bad in ["", "1.", "1.2.3.4", "v1", "1.x", "-1"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
        assert!(too_new_for("0.2.0", "0.1.9").unwrap().contains("needs platoon 0.2.0 or newer (this is 0.1.9)"));
        assert_eq!(too_new_for("0.1", "0.1.0"), None);
        assert_eq!(too_new_for("0.0.9", "0.1.0"), None);
        assert!(parse_version(platoon_version()).is_some());
    }

    #[test]
    fn ids_are_plain() {
        for ok in ["a", "sea-themes", "x2", "abc-1-2"] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["", "A", "1abc", "-a", "a-", "a--b", "a_b", "a.b", "a/b", ".x", &"a".repeat(41), "catalog "] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn a_good_plugin_passes_the_check() {
        let base = tmp("check-good");
        let dir = plugin(&base, "sea-themes", "1.0.0", &[("Deep Water", GOOD_THEME), ("Night Shore", GOOD_THEME)]);
        fs::write(dir.join("README.md"), "about it").unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".git/big"), vec![0u8; 2 * MAX_PLUGIN_BYTES as usize]).unwrap();
        let r = check(&dir);
        assert!(r.ok(), "{:?}", r.problems);
        assert_eq!(r.themes, ["Deep Water", "Night Shore"]);
        assert_eq!(r.files, 4, "hidden folders are not part of a plugin");
    }

    #[test]
    fn a_bad_plugin_fails_the_check_with_a_plain_reason() {
        let base = tmp("check-bad");
        let reasons = |dir: &Path| check(dir).problems.join(" | ");
        // No folder, no manifest, a broken manifest.
        assert!(reasons(&base.join("nothing")).contains("there is no folder"));
        fs::create_dir_all(base.join("empty")).unwrap();
        assert!(reasons(&base.join("empty")).contains("there is no plugin.json"));
        let dir = plugin(&base, "broken", "1.0.0", &[("A", GOOD_THEME)]);
        fs::write(dir.join(MANIFEST), "{\"id\": \"broken\"").unwrap();
        assert!(reasons(&dir).contains("isn't valid JSON"));
        // A theme file that is listed but missing, or outside the folder.
        let dir = plugin(&base, "missing", "1.0.0", &[("A", GOOD_THEME)]);
        fs::remove_file(dir.join("themes/A")).unwrap();
        assert!(reasons(&dir).contains("themes/A is listed in plugin.json but isn't in the folder"));
        let dir = plugin(&base, "escape", "1.0.0", &[("A", GOOD_THEME)]);
        fs::write(dir.join(MANIFEST), manifest_json("").replace("sea-themes", "escape").replace("themes/Deep", "../escape/themes/A")).unwrap();
        assert!(reasons(&dir).contains("isn't a file name inside the plugin"));
        // A theme that can't be read, and ones that don't read well.
        let dir = plugin(&base, "unreadable", "1.0.0", &[("A", "background = nope\nforeground = #fff")]);
        assert!(reasons(&dir).contains("themes/A: the background isn't a color"));
        let faint = GOOD_THEME.replace("foreground = #dfe3ea", "foreground = #4a4f58");
        let dir = plugin(&base, "faint", "1.0.0", &[("A", &faint)]);
        assert!(reasons(&dir).contains("themes/A: the text is hard to read on the background"), "{}", reasons(&dir));
        let dark_accent = GOOD_THEME.replace("#7ab4ff", "#23324a");
        let dir = plugin(&base, "dark-accent", "1.0.0", &[("A", &dark_accent)]);
        assert!(reasons(&dir).contains("the accent color is hard to read"), "{}", reasons(&dir));
        let alike = GOOD_THEME.replace("#e6c170", "#8bd49d");
        let dir = plugin(&base, "alike", "1.0.0", &[("A", &alike)]);
        assert!(reasons(&dir).contains("the ok and warning colors look too alike"), "{}", reasons(&dir));
        let light = plugin(&base, "light", "1.0.0", &[("A", "background = #fafafa\nforeground = #111111")]);
        assert!(reasons(&light).contains("it is a light theme"));
        let sparse = plugin(&base, "sparse", "1.0.0", &[("A", "background = #111111\nforeground = #eeeeee")]);
        assert!(reasons(&sparse).contains("palette 12 (or 4) is missing: it is the accent color"));
        // Too big, a link, nothing added, a newer platoon.
        let big = plugin(&base, "big", "1.0.0", &[("A", GOOD_THEME)]);
        fs::write(big.join("blob"), vec![b'x'; MAX_PLUGIN_BYTES as usize + 1]).unwrap();
        assert!(reasons(&big).contains("a plugin is at most 1024 KiB"), "{}", reasons(&big));
        let huge_theme = plugin(&base, "huge", "1.0.0", &[("A", &format!("{GOOD_THEME}#{}", "x".repeat(70_000)))]);
        assert!(reasons(&huge_theme).contains("bigger than 64 KiB"));
        let linked = plugin(&base, "linked", "1.0.0", &[("A", GOOD_THEME)]);
        std::os::unix::fs::symlink("/etc/hostname", linked.join("themes/B")).unwrap();
        assert!(reasons(&linked).contains("themes/B is a link"));
        let none = plugin(&base, "none", "1.0.0", &[]);
        assert!(reasons(&none).contains("it adds nothing"));
        let newer = plugin(&base, "newer", "1.0.0", &[("A", GOOD_THEME)]);
        let text = fs::read_to_string(newer.join(MANIFEST)).unwrap().replace("\"0.1.0\"", "\"99.0.0\"");
        fs::write(newer.join(MANIFEST), text).unwrap();
        assert!(reasons(&newer).contains("it needs platoon 99.0.0 or newer"));
        let twice = plugin(&base, "twice", "1.0.0", &[("A", GOOD_THEME)]);
        fs::write(twice.join(MANIFEST), manifest_json("").replace("sea-themes", "twice").replace("[\"themes/Deep\"]", "[\"themes/A\", \"themes/A\"]")).unwrap();
        assert!(reasons(&twice).contains("already used by another theme"));
    }

    fn store(base: &Path) -> Store {
        Store::new(base.join("data/plugins"))
    }

    fn folder(dir: &Path) -> Origin {
        Origin::Folder { path: dir.to_string_lossy().into_owned() }
    }

    #[test]
    fn install_copies_records_and_is_idempotent_and_remove_takes_it_off() {
        let base = tmp("install");
        let src = plugin(&base, "sea-themes", "1.0.0", &[("Deep", GOOD_THEME)]);
        let s = store(&base);
        assert!(s.installed().is_empty());
        let o = s.install(&src, folder(&src)).unwrap();
        assert!(matches!(o, Outcome::New(_)));
        let p = s.get("sea-themes").unwrap();
        assert_eq!(p.manifest.version, "1.0.0");
        assert_eq!(p.sha256.len(), 64);
        assert!(s.plugin_dir("sea-themes").join("themes/Deep").is_file());
        let mode = fs::metadata(s.plugin_dir("sea-themes").join("themes/Deep")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0, "nothing in a plugin is made runnable");
        // The same again: nothing changes.
        assert!(matches!(s.install(&src, folder(&src)).unwrap(), Outcome::Same(_)));
        // A new version replaces it.
        fs::write(src.join("themes/Deep"), GOOD_THEME.replace("#14181f", "#15181f")).unwrap();
        let text = fs::read_to_string(src.join(MANIFEST)).unwrap().replace("1.0.0", "1.1.0");
        fs::write(src.join(MANIFEST), text).unwrap();
        match s.install(&src, folder(&src)).unwrap() {
            Outcome::Replaced { now, was } => assert_eq!((was.as_str(), now.manifest.version.as_str()), ("1.0.0", "1.1.0")),
            o => panic!("{o:?}"),
        }
        assert!(!s.tmp().exists(), "nothing is left in the staging folder");
        // Keys another platoon wrote are kept.
        let mut rec: Value = serde_json::from_str(&fs::read_to_string(s.record_path()).unwrap()).unwrap();
        rec.as_object_mut().unwrap().insert("later".into(), json!(1));
        fs::write(s.record_path(), rec.to_string()).unwrap();
        assert_eq!(s.remove("sea-themes").unwrap().unwrap().manifest.version, "1.1.0");
        assert!(!s.plugin_dir("sea-themes").exists());
        assert!(s.installed().is_empty());
        let rec: Value = serde_json::from_str(&fs::read_to_string(s.record_path()).unwrap()).unwrap();
        assert_eq!(rec.get("later"), Some(&json!(1)));
        assert_eq!(s.remove("sea-themes").unwrap(), None, "removing twice is fine");
        assert_eq!(s.remove("../etc").unwrap(), None);
    }

    #[test]
    fn a_failed_install_leaves_nothing_that_looks_installed() {
        let base = tmp("install-fail");
        let s = store(&base);
        // A plugin that fails its check never gets as far as a file.
        let bad = plugin(&base, "bad", "1.0.0", &[("A", "background = #fff\nforeground = #000")]);
        assert!(s.install(&bad, folder(&bad)).unwrap_err().contains("can't be installed: themes/A: it is a light theme"));
        assert!(!s.plugin_dir("bad").exists());
        // The record can't be written after the files are in place: they go again.
        let good = plugin(&base, "good", "1.0.0", &[("A", GOOD_THEME)]);
        let mut failing = s.clone();
        failing.fail_record = true;
        assert!(failing.install(&good, folder(&good)).is_err());
        assert!(!s.plugin_dir("good").exists() && s.installed().is_empty());
        assert!(!s.tmp().exists());
        // ... and with an older version installed, that one is put back as it was.
        s.install(&good, folder(&good)).unwrap();
        let text = fs::read_to_string(good.join(MANIFEST)).unwrap().replace("1.0.0", "2.0.0");
        fs::write(good.join(MANIFEST), text).unwrap();
        assert!(failing.install(&good, folder(&good)).is_err());
        assert_eq!(s.get("good").unwrap().manifest.version, "1.0.0");
        // A folder with no record (an install cut off half way) is not installed, and the next install replaces it.
        fs::create_dir_all(s.plugin_dir("stray")).unwrap();
        fs::write(s.plugin_dir("stray").join(MANIFEST), "{}").unwrap();
        assert!(s.get("stray").is_none());
        // A record that can't be read is never written over.
        fs::write(s.record_path(), "{ not json").unwrap();
        assert!(s.install(&good, folder(&good)).unwrap_err().contains("can't be read"));
        assert_eq!(fs::read_to_string(s.record_path()).unwrap(), "{ not json");
        assert!(s.installed().is_empty());
    }

    #[test]
    fn the_catalog_is_fetched_sparsely_from_a_git_repo_and_the_commit_is_recorded() {
        let base = tmp("fetch");
        let (repo, commit) = catalog_repo(&base, &[("sea-themes", "1.0.0", &[("Deep", GOOD_THEME)]), ("dusk", "0.3.0", &[("Dusk", GOOD_THEME)])]);
        let s = store(&base);
        let src = source(&repo);
        assert!(cached(&s, &src).is_none());
        let cat = fetch(&s, &src).unwrap();
        assert_eq!(cat.commit, commit);
        let ids: Vec<&str> = cat.entries.iter().map(|e| e.manifest.id.as_str()).collect();
        assert_eq!(ids, ["sea-themes", "dusk"], "in the index's order");
        assert!(s.catalog_dir().join("sea-themes/themes/Deep").is_file());
        assert!(!s.catalog_dir().join("README.md").exists() && !s.dir.join(".catalog/src").exists(), "only the plugins folder");
        assert!(!s.dir.join(".fetch").read_dir().unwrap().any(|_| true), "the clone is gone");
        // The copy kept is there without the network, for the same source only.
        assert_eq!(cached(&s, &src).unwrap().commit, commit);
        assert!(cached(&s, &Source { url: "elsewhere".into(), rf: "main".into() }).is_none());
        // Installing from it records the commit.
        let e = cat.get("dusk").unwrap();
        let o = s.install(&e.dir, Origin::Catalog { source: cat.source.clone(), rf: cat.rf.clone(), commit: cat.commit.clone() }).unwrap();
        assert!(matches!(&o.plugin().origin, Origin::Catalog { commit: c, .. } if *c == commit));
        assert_eq!(o.plugin().sha256, e.sha256);
        // Nothing new: up to date. A new commit that changes it: updated; one it no longer has: kept.
        assert_eq!(update(&s, &cat, None), [Updated::UpToDate { name: "dusk name".into() }]);
        let newer = commit_catalog(&repo, &[("dusk", "0.4.0", &[("Dusk", &GOOD_THEME.replace("#14181f", "#101010"))])]);
        let cat2 = fetch(&s, &src).unwrap();
        assert_eq!(cat2.commit, newer);
        assert!(update_available(&s.get("dusk").unwrap(), &cat2));
        assert_eq!(update(&s, &cat2, Some("dusk")), [Updated::Done { name: "dusk name".into(), from: "0.3.0".into(), to: "0.4.0".into(), notes: Vec::new() }]);
        assert!(matches!(s.get("dusk").unwrap().origin, Origin::Catalog { commit: c, .. } if c == newer));
        commit_catalog(&repo, &[("sea-themes", "1.0.0", &[("Deep", GOOD_THEME)])]);
        let cat3 = fetch(&s, &src).unwrap();
        assert_eq!(update(&s, &cat3, None), [Updated::Gone { name: "dusk name".into() }]);
        assert!(s.get("dusk").is_some());
    }

    #[test]
    fn a_repository_without_a_catalog_says_so() {
        let base = tmp("no-catalog");
        let repo = base.join("source");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("README.md"), "code only\n").unwrap();
        super::testing::git(&repo, &["init", "--quiet"]);
        super::testing::git(&repo, &["add", "-A"]);
        super::testing::git(&repo, &["commit", "--quiet", "-m", "no catalog"]);
        let e = fetch(&store(&base), &source(&repo)).unwrap_err();
        assert_eq!(e, "platoon's plugin catalog isn't there yet: its repository has no plugins folder on main");
    }

    #[test]
    fn a_catalog_that_cannot_be_reached_says_so_plainly_and_keeps_the_old_copy() {
        let base = tmp("fetch-fail");
        let (repo, _) = catalog_repo(&base, &[("dusk", "0.3.0", &[("Dusk", GOOD_THEME)])]);
        let s = store(&base);
        fetch(&s, &source(&repo)).unwrap();
        let e = fetch(&s, &Source { url: base.join("no-repo").to_string_lossy().into_owned(), rf: "main".into() }).unwrap_err();
        assert!(e.starts_with("platoon couldn't reach its plugin catalog: "), "{e}");
        let e = fetch(&s, &Source { url: repo.to_string_lossy().into_owned(), rf: "no-such-branch".into() }).unwrap_err();
        assert!(e.contains("branch or tag wasn't found"), "{e}");
        assert!(cached(&s, &source(&repo)).is_some(), "the copy fetched before is kept");
        assert!(git_error("fatal: could not read Username for 'https://github.com': terminal prompts disabled").contains("needs a login"));
    }

    #[test]
    fn a_catalog_entry_that_does_not_match_its_index_is_left_out_with_a_reason() {
        let base = tmp("index");
        let cat = base.join("cat");
        plugin(&cat, "good", "1.0.0", &[("A", GOOD_THEME)]);
        plugin(&cat, "other", "1.0.0", &[("A", GOOD_THEME)]);
        fs::rename(cat.join("other"), cat.join("renamed")).unwrap();
        plugin(&cat, "old", "1.0.0", &[("A", GOOD_THEME)]);
        let index = json!({"plugins": [{"id": "good", "version": "1.0.0"}, {"id": "renamed"}, {"id": "old", "version": "0.9.0"}, {"id": "gone"}]});
        fs::write(cat.join(INDEX), index.to_string()).unwrap();
        let (entries, skipped) = read_catalog(&cat);
        assert_eq!(entries.iter().map(|e| e.manifest.id.as_str()).collect::<Vec<_>>(), ["good"]);
        assert_eq!(skipped.len(), 3, "{skipped:?}");
        assert!(skipped[0].contains("says its id is other") && skipped[1].contains("different versions") && skipped[2].contains("no folder"), "{skipped:?}");
    }

    #[test]
    fn plugin_themes_are_namespaced_labelled_and_never_clash() {
        let base = tmp("themes");
        let s = store(&base);
        let a = plugin(&base, "sea-themes", "1.0.0", &[("Nord", GOOD_THEME), ("Deep", GOOD_THEME)]);
        let b = plugin(&base, "more", "1.0.0", &[("Deep", GOOD_THEME)]);
        s.install(&a, folder(&a)).unwrap();
        s.install(&b, folder(&b)).unwrap();
        let taken: Vec<String> = Theme::all().into_iter().map(|t| t.name).collect();
        let got = themes(&s, &taken);
        let seen: Vec<(&str, &str, &str)> = got.iter().map(|t| (t.theme.id.as_str(), t.theme.name.as_str(), t.plugin.as_str())).collect();
        assert_eq!(
            seen,
            [
                ("plugin:more/deep", "Deep", "more name"),
                ("plugin:sea-themes/nord", "Nord (sea-themes name)", "sea-themes name"),
                ("plugin:sea-themes/deep", "Deep (sea-themes name)", "sea-themes name"),
            ],
            "by plugin name; a name already taken carries its plugin's"
        );
        s.remove("more").unwrap();
        assert_eq!(themes(&s, &taken).len(), 2);
    }

    #[test]
    fn a_plugins_skills_go_into_claudes_folder_a_clash_is_refused_and_only_platoons_own_copies_are_taken_away() {
        use crate::squadplugin::testing::plugin as full;
        let base = tmp("skills-store");
        let claude = base.join("claude");
        let src = full(&base.join("src"), "changelog", "1.0.0");
        let skill = claude.join("skills/changelog-entry");
        // A store that doesn't know Claude's folder writes no skill anywhere.
        assert!(store(&base).install(&src, folder(&src)).unwrap_err().contains("platoon doesn't know where Claude Code's folder is"));
        let s = Store::with_claude(base.join("data/plugins"), claude.clone());
        // The user's own skill of that name: refused, nothing installed, theirs untouched.
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "the user's own").unwrap();
        let e = s.install(&src, folder(&src)).unwrap_err();
        assert!(e.starts_with("changelog name can't be installed: Claude Code already has a skill called changelog-entry"), "{e}");
        assert!(s.installed().is_empty() && !s.plugin_dir("changelog").exists());
        assert_eq!(fs::read_to_string(skill.join("SKILL.md")).unwrap(), "the user's own");
        fs::remove_dir_all(&skill).unwrap();
        // Free: written and recorded.
        let (o, notes) = s.install_noting(&src, folder(&src)).unwrap();
        assert!(matches!(o, Outcome::New(_)) && notes.is_empty());
        assert_eq!(fs::read_to_string(skill.join("SKILL.md")).unwrap(), fs::read_to_string(src.join("skills/changelog-entry/SKILL.md")).unwrap());
        let p = s.get("changelog").unwrap();
        assert_eq!(p.skills.len(), 1);
        assert_eq!((p.skills[0].name.as_str(), p.skills[0].dir.as_path()), ("changelog-entry", skill.as_path()));
        // A record that can't be written after the skill is in place: the skill goes again, the old version stays.
        let text = fs::read_to_string(src.join(MANIFEST)).unwrap().replace("1.0.0", "1.1.0");
        fs::write(src.join(MANIFEST), text).unwrap();
        fs::write(src.join("skills/changelog-entry/SKILL.md"), crate::skillplugin::testing::skill_md("changelog-entry", "Write it better.")).unwrap();
        let mut failing = s.clone();
        failing.fail_record = true;
        assert!(failing.install(&src, folder(&src)).is_err());
        assert_eq!(s.get("changelog").unwrap().manifest.version, "1.0.0");
        assert!(!fs::read_to_string(skill.join("SKILL.md")).unwrap().contains("better"), "the old copy is back");
        // An update replaces the untouched copy.
        let (o, _) = s.install_noting(&src, folder(&src)).unwrap();
        assert!(matches!(o, Outcome::Replaced { .. }));
        assert!(fs::read_to_string(skill.join("SKILL.md")).unwrap().contains("better"));
        // The user changes it: the next update keeps theirs and says so; removing keeps it and says so.
        fs::write(skill.join("SKILL.md"), "changed by the user").unwrap();
        let text = fs::read_to_string(src.join(MANIFEST)).unwrap().replace("1.1.0", "1.2.0");
        fs::write(src.join(MANIFEST), text).unwrap();
        let (_, notes) = s.install_noting(&src, folder(&src)).unwrap();
        assert!(notes.iter().any(|n| n.contains("Your changed copy of the skill changelog-entry is kept")), "{notes:?}");
        assert_eq!(fs::read_to_string(skill.join("SKILL.md")).unwrap(), "changed by the user");
        let (was, notes) = s.remove_noting("changelog").unwrap();
        assert!(was.is_some() && notes[0].contains("was changed after it was installed, so it is kept"), "{notes:?}");
        assert!(skill.join("SKILL.md").exists());
        // Untouched: removing the plugin takes it away, and nothing else of Claude's folder.
        fs::remove_dir_all(&skill).unwrap();
        fs::create_dir_all(claude.join("skills/someone-elses")).unwrap();
        fs::write(claude.join("skills/someone-elses/SKILL.md"), "x").unwrap();
        s.install(&src, folder(&src)).unwrap();
        assert!(skill.exists());
        let (_, notes) = s.remove_noting("changelog").unwrap();
        assert!(notes.is_empty() && !skill.exists(), "{notes:?}");
        assert!(claude.join("skills/someone-elses/SKILL.md").exists());
        let left: Vec<String> = fs::read_dir(claude.join("skills")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["someone-elses"], "no staging or old copies left behind");
    }

    #[test]
    fn every_plugin_in_the_repository_catalog_passes_the_check_and_matches_the_index() {
        let cat = Path::new(env!("CARGO_MANIFEST_DIR")).join(CATALOG_FOLDER);
        let (entries, skipped) = read_catalog(&cat);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert!(!entries.is_empty());
        for e in &entries {
            let r = check(&e.dir);
            assert!(r.ok() && r.notes.is_empty(), "{}: {:?} {:?}", e.manifest.id, r.problems, r.notes);
        }
        // Every folder with a plugin.json is in the index: nothing sits in the catalog unoffered.
        for d in fs::read_dir(&cat).unwrap().flatten().filter(|d| d.path().join(MANIFEST).exists()) {
            let id = d.file_name().to_string_lossy().into_owned();
            assert!(entries.iter().any(|e| e.manifest.id == id), "{id} is missing from plugins/index.json");
        }
    }
}
