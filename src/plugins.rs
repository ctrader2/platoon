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
//!   installed.json                what is installed: version, where from (the catalog or a folder), sha256
//!   .lock, .tmp/                  one change at a time; installs are staged in .tmp
//! ```
//!
//! A plugin counts as installed only when `installed.json` lists it and its folder is there: an install copies into
//! `.tmp`, checks the copy, moves it into place and only then records it, so one that fails or is interrupted never
//! looks installed. Removing un-records first, then deletes.
//!
//! The catalog is BUILT INTO platoon: `build.rs` puts every file of each plugin `plugins/index.json` lists into the
//! binary (`builtin_files`), so installing one needs no network and a user gets exactly what was reviewed and released
//! with their platoon. Showing or installing one unpacks its files into a private temporary folder for as long as it
//! takes (`Entry::unpack`), and `check` runs on them there as on any folder. A newer platoon brings newer plugins: at
//! start `auto_plan` says which installed ones to update quietly (themes, the code editor) and which to ask about
//! first (skills, squads and agents: instructions Claude follows).
//!
//! Knob: `PLATOON_PLUGIN_CATALOG`, a folder laid out like `plugins/` used instead of the built-in catalog (tests).

use crate::editor_install::{sha256_file, sha256_hex};
use crate::theme::{contrast, delta_e, luminance, Theme, MIN_DELTA_E, MIN_ROLE, MIN_TEXT};
use crate::usertheme;
use serde_json::{json, Map, Value};
use std::{
    ffi::OsStr,
    fs, io,
    os::unix::{fs::PermissionsExt, io::AsRawFd},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

/// The catalog's folder in platoon's repository.
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
    /// The catalog that came with platoon (before the catalog was built in: fetched with git; the same to update).
    Catalog,
    /// A folder on this computer (someone making a plugin).
    Folder { path: String },
    /// The copy of it that came with this platoon (the code editor, taken over from an install made before it was a
    /// plugin). Updated from the catalog like one from there.
    Bundled { version: String },
}

impl Origin {
    /// "the catalog", "a folder: ~/my-plugin".
    pub fn words(&self, home: &str) -> String {
        match self {
            Origin::Catalog => "the catalog".into(),
            Origin::Folder { path } => format!("a folder: {}", crate::dirs::tilde(path, home)),
            Origin::Bundled { version } => format!("the copy that came with platoon {version}"),
        }
    }
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
    /// The catalog's version the user said no to updating to when platoon asked at start (`Store::decline_update`):
    /// not asked again for that version.
    pub declined: Option<String>,
}

impl Installed {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// It came from the catalog (or the copy of it that came with platoon), so the catalog updates it.
    pub fn from_catalog(&self) -> bool {
        matches!(self.origin, Origin::Catalog | Origin::Bundled { .. })
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

/// The key of a plugin's record in `installed.json` that holds the version the user said no to (older platoons leave it
/// alone, as any key they don't know).
const DECLINED: &str = "declined_update";

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
                    _ => Origin::Catalog,
                };
                let skills = v
                    .get("skills")
                    .and_then(Value::as_object)
                    .map(|o| o.iter().filter_map(|(name, p)| crate::skillplugin::Placed::from_json(name, p)).collect())
                    .unwrap_or_default();
                let declined = v.get(DECLINED).and_then(Value::as_str).map(String::from);
                Some(Installed { manifest, origin, sha256: s("sha256"), installed_at: v.get("installed_at").and_then(Value::as_u64).unwrap_or(0), skills, declined })
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
            Origin::Catalog => json!({"origin": "catalog"}),
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
            // (`source`, `ref` and `commit`: where a catalog fetched with git came from, before it was built in.)
            for key in ["origin", "source", "ref", "commit", "path", "platoon", "skills", DECLINED] {
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

mod built_in {
    include!(concat!(env!("OUT_DIR"), "/catalog.rs"));
}

/// Every file of the catalog built into this platoon: (its path in `plugins/`, its bytes). `index.json`, and the
/// files of each plugin it lists.
pub fn builtin_files() -> &'static [(&'static str, &'static [u8])] {
    built_in::FILES
}

/// The files of the built-in catalog's plugin `id`, as (path inside the plugin, bytes).
pub fn builtin_plugin(id: &str) -> Vec<(&'static str, &'static [u8])> {
    let prefix = format!("{id}/");
    builtin_files().iter().filter_map(|(rel, bytes)| rel.strip_prefix(prefix.as_str()).map(|r| (r, *bytes))).collect()
}

/// Where the catalog comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogFrom {
    /// Built into this platoon: the repository's `plugins/` as it was when this platoon was built.
    BuiltIn,
    /// A folder laid out like `plugins/` (`PLATOON_PLUGIN_CATALOG`: tests). One that isn't there is an empty catalog.
    Folder(PathBuf),
}

impl CatalogFrom {
    /// The built-in catalog, unless `PLATOON_PLUGIN_CATALOG` names a folder.
    pub fn from_env() -> CatalogFrom {
        match std::env::var_os("PLATOON_PLUGIN_CATALOG").filter(|v| !v.is_empty()) {
            Some(dir) => CatalogFrom::Folder(PathBuf::from(dir)),
            None => CatalogFrom::BuiltIn,
        }
    }
}

/// A plugin the catalog offers.
#[derive(Clone, Debug)]
pub struct Entry {
    pub manifest: Manifest,
    pub sha256: String,
    /// Why it can't be installed here (it needs a newer platoon, ...); empty when it can.
    pub problems: Vec<String>,
    /// What installing the code editor it adds downloads on this computer.
    pub download_bytes: Option<u64>,
    /// Its files, (path inside it, bytes): what `unpack` writes out.
    files: Arc<Vec<(String, Vec<u8>)>>,
}

/// A catalog plugin's files written out into a private temporary folder, for as long as this lives (it is deleted
/// when dropped): what showing and installing it read.
pub struct Unpacked {
    root: PathBuf,
    dir: PathBuf,
}

impl Unpacked {
    /// The plugin's folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Unpacked {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A new folder only this user can enter, in the system's temporary folder: made fresh (never one that was already
/// there, whoever made it).
fn scratch() -> io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let mut last = io::Error::other("no free name");
    for _ in 0..16 {
        let p = std::env::temp_dir().join(format!("platoon-plugin-{}-{nanos}", unique()));
        match fs::DirBuilder::new().mode(0o700).create(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

/// Write `files` (path inside `name`, bytes) into a fresh scratch folder, under `name`.
fn unpack_files<'a>(name: &str, files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Result<Unpacked, String> {
    let say = |e: io::Error| format!("couldn't unpack the plugin catalog into a temporary folder: {e}");
    let root = scratch().map_err(say)?;
    let u = Unpacked { dir: root.join(name), root };
    fs::create_dir_all(&u.dir).map_err(say)?;
    for (rel, bytes) in files {
        if !safe_rel(rel) {
            return Err(format!("the plugin catalog has a file that can't be written out ({})", clean(rel)));
        }
        let to = u.dir.join(rel);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(say)?;
        }
        fs::write(&to, bytes).map_err(say)?;
    }
    Ok(u)
}

impl Entry {
    /// Its files, written out into a private temporary folder: what `check`, the review and `Store::install` read.
    pub fn unpack(&self) -> Result<Unpacked, String> {
        unpack_files(&self.manifest.id, self.files.iter().map(|(rel, b)| (rel.as_str(), b.as_slice())))
    }
}

#[derive(Clone, Debug)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    /// Listed in the index but not usable, and why: "foo: plugin.json has no \"version\"".
    pub skipped: Vec<String>,
}

impl Catalog {
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.manifest.id == id)
    }
}

/// The catalog: the one built into platoon (unpacked for a moment to be read and checked), or a folder's.
pub fn load_catalog(from: &CatalogFrom) -> Catalog {
    let (entries, skipped) = match from {
        CatalogFrom::Folder(dir) => read_catalog(dir),
        CatalogFrom::BuiltIn => match unpack_files(CATALOG_FOLDER, builtin_files().iter().copied()) {
            Ok(u) => read_catalog(u.dir()),
            Err(e) => (Vec::new(), vec![e]),
        },
    };
    Catalog { entries, skipped }
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
        let found = walk(&folder, &mut ignored);
        let sha256 = tree_hash(&found).unwrap_or_default();
        // Its files are kept to be written out again when it is shown or installed (a plugin too big to install
        // keeps none: it can't be installed anyway).
        let files: Vec<(String, Vec<u8>)> = if report.bytes <= MAX_PLUGIN_BYTES { found.iter().filter_map(|f| fs::read(&f.path).ok().map(|b| (f.rel.clone(), b))).collect() } else { Vec::new() };
        entries.push(Entry { manifest: m, sha256, problems: report.problems, download_bytes: report.download_bytes, files: Arc::new(files) });
    }
    (entries, skipped)
}

/// The catalog's version is newer than the one installed.
pub fn newer(p: &Installed, e: &Entry) -> bool {
    matches!((parse_version(&e.manifest.version), parse_version(&p.manifest.version)), (Some(a), Some(b)) if a > b)
}

/// The catalog has a newer version of an installed plugin that came from it, one that can be installed here.
pub fn update_available(p: &Installed, catalog: &Catalog) -> bool {
    p.from_catalog() && catalog.get(p.id()).is_some_and(|e| e.problems.is_empty() && newer(p, e))
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

/// Bring installed plugins up to the catalog: each (or the one named) that came from the catalog and whose version
/// there is newer is installed again. One another platoon updated a moment before is up to date.
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
        if !newer(&p, e) {
            out.push(Updated::UpToDate { name });
            continue;
        }
        if !e.problems.is_empty() {
            out.push(Updated::Failed { name, why: e.problems.join("; ") });
            continue;
        }
        let done = e.unpack().and_then(|u| store.install_noting(u.dir(), Origin::Catalog));
        match done {
            Ok((Outcome::Same(_), _)) => out.push(Updated::UpToDate { name }),
            Ok((o, notes)) => out.push(Updated::Done { name, from: p.manifest.version.clone(), to: o.plugin().manifest.version.clone(), notes }),
            Err(why) => out.push(Updated::Failed { name, why }),
        }
    }
    out
}

/// An update of an installed plugin to ask the user about first (it has instructions Claude follows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub id: String,
    pub name: String,
    pub from: String,
    pub to: String,
}

/// What a newer platoon does with the plugins installed from its catalog when it starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutoPlan {
    /// Updated quietly, then said in one line: themes and the code editor (ids).
    pub quiet: Vec<String>,
    /// Asked about first, with the review of what changed: skills, squads and agents. One the user said no to for
    /// that same version isn't here.
    pub ask: Vec<Ask>,
}

/// Which installed plugins a newer catalog updates: only those installed from the catalog whose version there is
/// newer (one installed from a folder is never touched; one the catalog no longer has stays as it is).
pub fn auto_plan(store: &Store, catalog: &Catalog) -> AutoPlan {
    let mut plan = AutoPlan::default();
    for p in store.installed() {
        if !update_available(&p, catalog) {
            continue;
        }
        let Some(e) = catalog.get(p.id()) else { continue };
        if e.manifest.has_instructions() || p.manifest.has_instructions() {
            if p.declined.as_deref() != Some(e.manifest.version.as_str()) {
                plan.ask.push(Ask { id: p.id().to_string(), name: e.manifest.name.clone(), from: p.manifest.version.clone(), to: e.manifest.version.clone() });
            }
        } else {
            plan.quiet.push(p.id().to_string());
        }
    }
    plan
}

/// Something installed came from the catalog: only then is there anything for a new catalog to update.
pub fn any_from_catalog(store: &Store) -> bool {
    store.installed().iter().any(Installed::from_catalog)
}

/// Update the plugins of `plan.quiet` from the catalog: what was updated, as (name, version, it is the code editor),
/// and what couldn't be, in plain words.
pub fn auto_update(store: &Store, catalog: &Catalog, plan: &AutoPlan) -> (Vec<(String, String, bool)>, Vec<String>) {
    let (mut done, mut failed) = (Vec::new(), Vec::new());
    for id in &plan.quiet {
        for u in update(store, catalog, Some(id)) {
            match u {
                Updated::Done { name, to, .. } => {
                    let editor = catalog.get(id).is_some_and(|e| e.manifest.editor.is_some());
                    done.push((name, to, editor));
                }
                Updated::Failed { name, why } => failed.push(format!("{name} couldn't be updated: {why}")),
                _ => {}
            }
        }
    }
    (done, failed)
}

/// The one line that says what was updated with platoon: "Updated with platoon: Code editor (Neovim) 1.2.0, Theme
/// pack 1.1.0." Empty when nothing was.
pub fn updated_words(done: &[(String, String, bool)]) -> String {
    if done.is_empty() {
        return String::new();
    }
    let list: Vec<String> = done.iter().map(|(name, version, _)| format!("{name} {version}")).collect();
    format!("Updated with platoon: {}.", list.join(", "))
}

impl Store {
    /// The user said no to updating plugin `id` to `version` when platoon asked at start: kept in its record, so the
    /// same version isn't asked about again (a newer one is). Nothing else changes.
    pub fn decline_update(&self, id: &str, version: &str) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut rec = self.read_record()?;
        let Some(entry) = rec.get_mut("plugins").and_then(Value::as_object_mut).and_then(|p| p.get_mut(id)).and_then(Value::as_object_mut) else {
            return Ok(());
        };
        entry.insert(DECLINED.into(), json!(version));
        self.write_record(&rec)
    }

    /// The copy of the catalog an older platoon fetched with git (`.catalog/`, and `.catalog.json` saying from where):
    /// platoon's own, unused now, so it goes. Nothing else in the folder is touched.
    pub fn forget_fetched_catalog(&self) {
        let old = self.dir.join(".catalog");
        if fs::symlink_metadata(&old).is_ok_and(|m| m.is_dir()) {
            let _ = fs::remove_dir_all(&old);
        }
        let record = self.dir.join(".catalog.json");
        if fs::symlink_metadata(&record).is_ok_and(|m| m.is_file()) {
            let _ = fs::remove_file(&record);
        }
    }
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
    //! Plugin folders and catalog folders for tests (here, in the app's tests and in `plugincli`).
    use super::*;

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

    /// A catalog folder laid out like `plugins/` in `<base>/catalog`, with these plugins (id, version, themes).
    pub fn catalog_dir(base: &Path, plugins: &[(&str, &str, &[(&str, &str)])]) -> PathBuf {
        let cat = base.join("catalog");
        write_catalog(&cat, plugins);
        cat
    }

    /// Replace the catalog in `cat` with these plugins (what a newer platoon brings).
    pub fn write_catalog(cat: &Path, plugins: &[(&str, &str, &[(&str, &str)])]) {
        let _ = fs::remove_dir_all(cat);
        fs::create_dir_all(cat).unwrap();
        let mut index = Vec::new();
        for (id, version, themes) in plugins {
            plugin(cat, id, version, themes);
            index.push(json!({"id": id, "version": version}));
        }
        fs::write(cat.join(INDEX), serde_json::to_string_pretty(&json!({"plugins": index})).unwrap()).unwrap();
    }

    /// Change the version of plugin `id` in catalog `cat` (its plugin.json and the index) to `to`.
    pub fn bump(cat: &Path, id: &str, to: &str) {
        let mut m: Value = serde_json::from_str(&fs::read_to_string(cat.join(id).join(MANIFEST)).unwrap()).unwrap();
        m.as_object_mut().unwrap().insert("version".into(), json!(to));
        fs::write(cat.join(id).join(MANIFEST), serde_json::to_string_pretty(&m).unwrap()).unwrap();
        let mut index: Value = serde_json::from_str(&fs::read_to_string(cat.join(INDEX)).unwrap()).unwrap();
        for p in index.get_mut("plugins").and_then(Value::as_array_mut).unwrap() {
            if p.get("id").and_then(Value::as_str) == Some(id) {
                p.as_object_mut().unwrap().insert("version".into(), json!(to));
            }
        }
        fs::write(cat.join(INDEX), index.to_string()).unwrap();
    }

    /// Add plugin folder `dir` (already in `cat`) to the catalog's index.
    pub fn list_in_index(cat: &Path, id: &str, version: &str) {
        let path = cat.join(INDEX);
        let mut index: Value = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({"plugins": []}));
        index.get_mut("plugins").and_then(Value::as_array_mut).unwrap().push(json!({"id": id, "version": version}));
        fs::write(path, index.to_string()).unwrap();
    }

    pub fn from(cat: &Path) -> CatalogFrom {
        CatalogFrom::Folder(cat.to_path_buf())
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
    fn the_catalog_is_read_unpacked_to_install_and_updates_only_to_a_newer_version() {
        let base = tmp("catalog");
        let cat = catalog_dir(&base, &[("sea-themes", "1.0.0", &[("Deep", GOOD_THEME)]), ("dusk", "0.3.0", &[("Dusk", GOOD_THEME)])]);
        let s = store(&base);
        let c = load_catalog(&from(&cat));
        let ids: Vec<&str> = c.entries.iter().map(|e| e.manifest.id.as_str()).collect();
        assert_eq!(ids, ["sea-themes", "dusk"], "in the index's order");
        assert!(c.skipped.is_empty(), "{:?}", c.skipped);
        // Unpacked into a private folder of its own, gone again when done with.
        let e = c.get("dusk").unwrap();
        let u = e.unpack().unwrap();
        let root = u.dir().parent().unwrap().to_path_buf();
        assert!(u.dir().join("themes/Dusk").is_file() && u.dir().ends_with("dusk"));
        assert_eq!(fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700, "only this user can look in");
        let o = s.install(u.dir(), Origin::Catalog).unwrap();
        drop(u);
        assert!(!root.exists(), "the unpacked copy is gone");
        assert_eq!(o.plugin().origin, Origin::Catalog);
        assert_eq!(o.plugin().sha256, e.sha256);
        // Nothing newer: up to date. The same version with other files is not an update (a new version raises it).
        assert_eq!(update(&s, &c, None), [Updated::UpToDate { name: "dusk name".into() }]);
        write_catalog(&cat, &[("dusk", "0.3.0", &[("Dusk", &GOOD_THEME.replace("#14181f", "#101010"))])]);
        let c2 = load_catalog(&from(&cat));
        assert!(!update_available(&s.get("dusk").unwrap(), &c2));
        // A newer version: updated.
        bump(&cat, "dusk", "0.4.0");
        let c3 = load_catalog(&from(&cat));
        assert!(update_available(&s.get("dusk").unwrap(), &c3));
        assert_eq!(update(&s, &c3, Some("dusk")), [Updated::Done { name: "dusk name".into(), from: "0.3.0".into(), to: "0.4.0".into(), notes: Vec::new() }]);
        assert_eq!(s.get("dusk").unwrap().manifest.version, "0.4.0");
        // An older one (an older platoon) is no update either.
        bump(&cat, "dusk", "0.2.0");
        assert_eq!(update(&s, &load_catalog(&from(&cat)), None), [Updated::UpToDate { name: "dusk name".into() }]);
        // One the catalog no longer has: kept.
        write_catalog(&cat, &[("sea-themes", "1.0.0", &[("Deep", GOOD_THEME)])]);
        assert_eq!(update(&s, &load_catalog(&from(&cat)), None), [Updated::Gone { name: "dusk name".into() }]);
        assert!(s.get("dusk").is_some());
        // A catalog folder that isn't there is an empty catalog, and says why.
        let none = load_catalog(&from(&base.join("no-such-catalog")));
        assert!(none.entries.is_empty() && none.skipped == ["the catalog has no readable index.json"]);
    }

    #[test]
    fn a_record_from_when_the_catalog_was_fetched_with_git_reads_as_from_the_catalog() {
        let base = tmp("old-record");
        let cat = catalog_dir(&base, &[("dusk", "0.3.0", &[("Dusk", GOOD_THEME)])]);
        let s = store(&base);
        let c = load_catalog(&from(&cat));
        let u = c.get("dusk").unwrap().unpack().unwrap();
        s.install(u.dir(), Origin::Catalog).unwrap();
        // As an older platoon wrote it: where the catalog was fetched from, and a key it wrote that this one doesn't know.
        let mut rec: Value = serde_json::from_str(&fs::read_to_string(s.record_path()).unwrap()).unwrap();
        let p = rec.get_mut("plugins").and_then(|p| p.get_mut("dusk")).and_then(Value::as_object_mut).unwrap();
        p.insert("source".into(), json!("https://example.invalid/platoon.git"));
        p.insert("ref".into(), json!("main"));
        p.insert("commit".into(), json!("0123456789abcdef"));
        p.insert("later".into(), json!(true));
        fs::write(s.record_path(), rec.to_string()).unwrap();
        let p = s.get("dusk").unwrap();
        assert!(p.from_catalog() && p.origin == Origin::Catalog);
        assert_eq!(p.origin.words("/home/u"), "the catalog");
        // The same files again: nothing changes. A newer version: its record loses the git keys, keeps the unknown one.
        assert!(matches!(s.install(u.dir(), Origin::Catalog).unwrap(), Outcome::Same(_)));
        bump(&cat, "dusk", "0.4.0");
        update(&s, &load_catalog(&from(&cat)), None);
        let rec: Value = serde_json::from_str(&fs::read_to_string(s.record_path()).unwrap()).unwrap();
        let p = &rec["plugins"]["dusk"];
        assert!(p.get("source").is_none() && p.get("commit").is_none() && p.get("ref").is_none(), "{p}");
        assert_eq!((p["origin"].as_str(), p["later"].as_bool()), (Some("catalog"), Some(true)));
    }

    #[test]
    fn a_newer_catalog_updates_themes_quietly_asks_about_instructions_and_remembers_a_no_for_that_version_only() {
        let base = tmp("auto");
        let claude = base.join("claude");
        let s = Store::with_claude(base.join("data/plugins"), claude);
        let cat = catalog_dir(&base, &[("sea-themes", "1.0.0", &[("Deep", GOOD_THEME)]), ("dusk", "0.3.0", &[("Dusk", GOOD_THEME)])]);
        crate::squadplugin::testing::plugin(&cat, "kit", "1.0.0");
        list_in_index(&cat, "kit", "1.0.0");
        let mine = plugin(&base.join("mine"), "moss", "1.0.0", &[("Moss", GOOD_THEME)]);
        // Nothing installed: nothing to do (and the catalog needn't even be read).
        assert!(!any_from_catalog(&s));
        let c = load_catalog(&from(&cat));
        for id in ["sea-themes", "dusk", "kit"] {
            let u = c.get(id).unwrap().unpack().unwrap();
            s.install(u.dir(), Origin::Catalog).unwrap();
        }
        s.install(&mine, folder(&mine)).unwrap();
        assert!(any_from_catalog(&s));
        assert_eq!(auto_plan(&s, &c), AutoPlan::default(), "nothing newer: nothing to do");
        // A newer platoon: sea-themes and kit are newer, dusk isn't; moss (from a folder) is in the catalog too, newer.
        bump(&cat, "sea-themes", "1.1.0");
        bump(&cat, "kit", "1.1.0");
        plugin(&cat, "moss", "9.0.0", &[("Moss", GOOD_THEME)]);
        list_in_index(&cat, "moss", "9.0.0");
        let c = load_catalog(&from(&cat));
        let plan = auto_plan(&s, &c);
        assert_eq!(plan.quiet, ["sea-themes"]);
        assert_eq!(plan.ask, [Ask { id: "kit".into(), name: "kit name".into(), from: "1.0.0".into(), to: "1.1.0".into() }]);
        let (done, failed) = auto_update(&s, &c, &plan);
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(done, [("sea-themes name".to_string(), "1.1.0".to_string(), false)]);
        assert_eq!(updated_words(&done), "Updated with platoon: sea-themes name 1.1.0.");
        assert_eq!(s.get("sea-themes").unwrap().manifest.version, "1.1.0");
        assert_eq!(s.get("kit").unwrap().manifest.version, "1.0.0", "not updated without asking");
        assert_eq!(s.get("moss").unwrap().manifest.version, "1.0.0", "a folder's plugin is never touched");
        // Again: only the question is left; the same run twice changes nothing more.
        let plan = auto_plan(&s, &c);
        assert!(plan.quiet.is_empty() && plan.ask.len() == 1);
        assert!(auto_update(&s, &c, &plan).0.is_empty());
        assert_eq!(updated_words(&[]), "");
        // No to 1.1.0: not asked again for it; its state is still "update ready".
        s.decline_update("kit", "1.1.0").unwrap();
        assert_eq!(s.get("kit").unwrap().declined.as_deref(), Some("1.1.0"));
        assert_eq!(auto_plan(&s, &c), AutoPlan::default());
        assert!(update_available(&s.get("kit").unwrap(), &c));
        // A still newer one is asked about again.
        bump(&cat, "kit", "1.2.0");
        let c = load_catalog(&from(&cat));
        assert_eq!(auto_plan(&s, &c).ask.iter().map(|a| a.to.as_str()).collect::<Vec<_>>(), ["1.2.0"]);
        // Updating it (the user said yes) forgets the no.
        let u = c.get("kit").unwrap().unpack().unwrap();
        s.install(u.dir(), Origin::Catalog).unwrap();
        assert_eq!(s.get("kit").unwrap().declined, None);
        // Declining a plugin that isn't installed changes nothing.
        s.decline_update("nothing", "1.0.0").unwrap();
        assert!(s.get("nothing").is_none());
    }

    #[test]
    fn two_platoons_starting_at_once_update_a_plugin_once_and_only_one_says_so() {
        let base = tmp("auto-twice");
        let cat = catalog_dir(&base, &[("sea-themes", "1.0.0", &[("Deep", GOOD_THEME)]), ("dusk", "0.3.0", &[("Dusk", GOOD_THEME)])]);
        let s = store(&base);
        let c = load_catalog(&from(&cat));
        for id in ["sea-themes", "dusk"] {
            s.install(c.get(id).unwrap().unpack().unwrap().dir(), Origin::Catalog).unwrap();
        }
        bump(&cat, "sea-themes", "1.1.0");
        bump(&cat, "dusk", "0.4.0");
        let c = load_catalog(&from(&cat));
        // Both plan before either updates (as two windows opened together), then both update.
        let plans = [auto_plan(&s, &c), auto_plan(&s, &c)];
        let runs: Vec<(Vec<(String, String, bool)>, Vec<String>)> = thread::scope(|sc| {
            let hs: Vec<_> = plans.iter().map(|plan| sc.spawn(|| auto_update(&s, &c, plan))).collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (_, failed) in &runs {
            assert!(failed.is_empty(), "{failed:?}");
        }
        let mut said: Vec<String> = runs.iter().flat_map(|(d, _)| d.iter().map(|(n, ..)| n.clone())).collect();
        said.sort();
        assert_eq!(said, ["dusk name", "sea-themes name"], "each update said once, by whichever did it");
        assert_eq!((s.get("sea-themes").unwrap().manifest.version.as_str(), s.get("dusk").unwrap().manifest.version.as_str()), ("1.1.0", "0.4.0"));
        assert!(!s.tmp().exists());
    }

    #[test]
    fn the_catalog_an_older_platoon_fetched_is_deleted_and_nothing_else() {
        let base = tmp("old-catalog");
        let s = store(&base);
        fs::create_dir_all(s.dir.join(".catalog/editor")).unwrap();
        fs::write(s.dir.join(".catalog/index.json"), "{}").unwrap();
        fs::write(s.dir.join(".catalog.json"), "{}").unwrap();
        fs::create_dir_all(s.dir.join("editor")).unwrap();
        fs::write(s.dir.join("installed.json"), "{}").unwrap();
        fs::write(s.dir.join(".lock"), "").unwrap();
        s.forget_fetched_catalog();
        let mut left: Vec<String> = fs::read_dir(&s.dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, [".lock", "editor", "installed.json"]);
        s.forget_fetched_catalog();
        store(&base.join("nothing-here")).forget_fetched_catalog();
        assert!(!base.join("nothing-here").exists(), "nothing is made either");
    }

    #[test]
    fn the_built_in_catalog_is_the_repositorys_plugins_folder_file_for_file() {
        let cat = Path::new(env!("CARGO_MANIFEST_DIR")).join(CATALOG_FOLDER);
        let (on_disk, skipped) = read_catalog(&cat);
        assert!(skipped.is_empty(), "{skipped:?}");
        // Every file of every listed plugin, and the index, byte for byte; nothing else of the repository.
        let mut want: Vec<String> = vec![INDEX.to_string()];
        for e in &on_disk {
            let mut ignored = Vec::new();
            want.extend(walk(&cat.join(&e.manifest.id), &mut ignored).into_iter().map(|f| format!("{}/{}", e.manifest.id, f.rel)));
        }
        want.sort();
        let mut got: Vec<String> = builtin_files().iter().map(|(rel, _)| rel.to_string()).collect();
        got.sort();
        assert_eq!(got, want);
        for (rel, bytes) in builtin_files() {
            assert_eq!(fs::read(cat.join(rel)).unwrap(), *bytes, "{rel}");
        }
        // Read from inside platoon: the same plugins, the same files.
        let built = load_catalog(&CatalogFrom::BuiltIn);
        assert!(built.skipped.is_empty(), "{:?}", built.skipped);
        let ids = |es: &[Entry]| es.iter().map(|e| (e.manifest.id.clone(), e.manifest.version.clone(), e.sha256.clone())).collect::<Vec<_>>();
        assert_eq!(ids(&built.entries), ids(&on_disk));
        assert!(built.entries.iter().all(|e| e.problems.is_empty()));
        assert!(builtin_plugin(crate::editorplugin::ID).iter().any(|(rel, _)| *rel == crate::editorplugin::INIT));
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
            let r = check(&cat.join(&e.manifest.id));
            assert!(r.ok() && r.notes.is_empty(), "{}: {:?} {:?}", e.manifest.id, r.problems, r.notes);
        }
        // Every folder with a plugin.json is in the index: nothing sits in the catalog unoffered.
        for d in fs::read_dir(&cat).unwrap().flatten().filter(|d| d.path().join(MANIFEST).exists()) {
            let id = d.file_name().to_string_lossy().into_owned();
            assert!(entries.iter().any(|e| e.manifest.id == id), "{id} is missing from plugins/index.json");
        }
    }
}
