//! Skills as a kind of plugin: Claude Code skills (`skills/<name>/SKILL.md` and any text files beside it) that
//! installing the plugin writes into Claude Code's own skills folder, `<claude config dir>/skills/<name>/`, the same
//! place `platoon setup` writes the `/to-tickets` skill (`totickets.rs`).
//!
//! ```json
//! "skills": ["skills/changelog-entry"]
//! ```
//!
//! A skill is instructions Claude follows with the user's permissions, so the rules are strict:
//! - Each entry is a folder of the plugin holding `SKILL.md` (at most `MAX_SKILL_MD`) and text files only (at most
//!   `MAX_SKILL_FILES` files, `MAX_SKILL_BYTES` in all), so everything in it can be read before it is installed.
//! - `SKILL.md` starts with front matter: `name` (the folder's name) and `description` (at most `MAX_DESCRIPTION`
//!   characters). No `hooks`: a skill's hooks run commands, and plugins hold no code.
//! - **Names are namespaced by the plugin**: a skill is called by the plugin's id, or starts with it and a dash
//!   (plugin `changelog`: `changelog`, `changelog-entry`). So two plugins of the catalog never clash, and the user
//!   can tell which plugin a skill in Claude Code came from. Never `to-tickets`, which is platoon's own.
//! - It is written as it is, under its own name, only where that name is free: a folder platoon didn't put there is
//!   never written over (the install is refused, naming it).
//!
//! What platoon wrote is recorded in the plugin's entry of `installed.json` (`Placed`: the folder and each file's
//! sha256). Removing the plugin deletes a skill folder only when it is exactly what platoon wrote; one the user
//! changed is kept, and said. An update replaces an untouched copy and keeps a changed one.

use crate::editor_install::{sha256_file, sha256_hex};
use serde_json::{Map, Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// The kind's key in `plugin.json`.
pub const KIND: &str = "skills";
/// A skill's `SKILL.md` at most.
pub const MAX_SKILL_MD: u64 = 64 * 1024;
/// The files of one skill's folder, and their size in all.
pub const MAX_SKILL_FILES: usize = 20;
pub const MAX_SKILL_BYTES: u64 = 256 * 1024;
/// The longest description (Claude Code's own limit).
pub const MAX_DESCRIPTION: usize = 1024;
/// The longest skill name (Claude Code's own limit).
pub const NAME_MAX: usize = 64;
/// Skills platoon has of its own: no plugin may bring one of these names.
pub const BUILT_IN: &[&str] = &[crate::totickets::NAME];

/// What a skill's front matter says.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FrontMatter {
    pub name: String,
    pub description: String,
    /// Tools it may use without asking while it runs (`allowed-tools`).
    pub allowed_tools: Vec<String>,
    /// Claude may use it by itself when it fits (no `disable-model-invocation: true`).
    pub model_may_use: bool,
    /// The user can type `/name` (no `user-invocable: false`).
    pub user_may_type: bool,
    /// Every key, in order.
    pub keys: Vec<String>,
    /// The keys besides those above, with their values as written: what else it asks of Claude Code (a model, say).
    pub others: Vec<(String, String)>,
}

/// A skill of a plugin, as checked: its name, its folder in the plugin, its front matter, its `SKILL.md` and the other
/// files of its folder (path inside the folder, text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub folder: String,
    pub front: FrontMatter,
    pub text: String,
    pub others: Vec<(String, String)>,
}

/// Can this be a skill's name: lowercase letters, digits and single dashes, starting with a letter, at most `NAME_MAX`.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= NAME_MAX
        && n.starts_with(|c: char| c.is_ascii_lowercase())
        && !n.ends_with('-')
        && !n.contains("--")
        && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The skill names plugin `id` may bring: `id` itself, or `id-...`.
pub fn namespaced(plugin: &str, name: &str) -> bool {
    name == plugin || name.strip_prefix(plugin).is_some_and(|rest| rest.starts_with('-') && rest.len() > 1)
}

// ---- front matter ---------------------------------------------------------------------------------------------------

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return v[1..v.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\");
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        return v[1..v.len() - 1].replace("''", "'");
    }
    v.to_string()
}

/// Tool names from one line: `Read, Grep`, `[Read, Grep]` or `Bash(git add:*) Bash(git status:*)`; commas and spaces
/// inside parentheses stay.
fn tools_of(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')).unwrap_or(line);
    let mut out = Vec::new();
    let (mut cur, mut depth) = (String::new(), 0i32);
    for c in line.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' | ' ' | '\t' if depth <= 0 => {
                if !cur.trim().is_empty() {
                    out.push(unquote(&cur));
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(unquote(&cur));
    }
    out.retain(|t| !t.is_empty());
    out
}

/// A `SKILL.md`'s front matter (the simple YAML between two `---` lines at its top) and the text after it.
pub fn front_matter(text: &str) -> Result<(FrontMatter, String), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Err("SKILL.md must start with its front matter: a line \"---\", then name: and description:, then \"---\"".into());
    }
    let mut entries: Vec<(String, String, Vec<String>)> = Vec::new();
    let mut closed = false;
    let mut consumed = first.len();
    for line in lines.by_ref() {
        consumed += line.len();
        let l = line.trim_end_matches(['\n', '\r']);
        if l.trim() == "---" {
            closed = true;
            break;
        }
        if l.trim().is_empty() || l.trim_start().starts_with('#') {
            continue;
        }
        let top = !l.starts_with([' ', '\t', '-']);
        match l.split_once(':') {
            Some((k, v)) if top => entries.push((k.trim().to_string(), v.trim().to_string(), Vec::new())),
            _ => match entries.last_mut() {
                Some(e) => e.2.push(l.trim().to_string()),
                None => return Err(format!("its front matter isn't simple \"key: value\" lines (\"{}\")", l.trim())),
            },
        }
    }
    if !closed {
        return Err("its front matter has no closing \"---\" line".into());
    }
    let body = text[consumed.min(text.len())..].to_string();
    let scalar = |v: &str, more: &[String]| -> String {
        if matches!(v, "|" | ">" | "|-" | ">-" | "|+" | ">+") || v.is_empty() {
            return more.iter().map(|m| unquote(m)).collect::<Vec<_>>().join(" ").trim().to_string();
        }
        let mut s = unquote(v);
        for m in more {
            s.push(' ');
            s.push_str(m.trim());
        }
        s.trim().to_string()
    };
    let mut f = FrontMatter { model_may_use: true, user_may_type: true, ..Default::default() };
    for (k, v, more) in &entries {
        f.keys.push(k.clone());
        match k.as_str() {
            "name" => f.name = scalar(v, more),
            "description" => f.description = scalar(v, more),
            "allowed-tools" => {
                let mut t = tools_of(v);
                for m in more {
                    t.extend(tools_of(m.trim_start_matches('-')));
                }
                f.allowed_tools = t;
            }
            "disable-model-invocation" => f.model_may_use = scalar(v, more) != "true",
            "user-invocable" => f.user_may_type = scalar(v, more) != "false",
            _ => f.others.push((k.clone(), scalar(v, more))),
        }
    }
    Ok((f, body))
}

// ---- checking ------------------------------------------------------------------------------------------------------

fn is_text(bytes: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(bytes).ok()?;
    (!s.contains('\0')).then(|| s.to_string())
}

/// What `plugins::check` adds for the skills a plugin names: the problems, each in plain words, and the skills that
/// passed. `files` are the plugin's (path inside it, where it is, size); `safe` says whether a name from `plugin.json`
/// can only mean something inside it.
pub fn check(plugin: &str, entries: &[String], files: &[(String, PathBuf, u64)], safe: fn(&str) -> bool) -> (Vec<String>, Vec<Skill>) {
    let mut problems = Vec::new();
    let mut out: Vec<Skill> = Vec::new();
    for entry in entries {
        let folder = entry.trim_end_matches('/').to_string();
        if !safe(&folder) {
            problems.push(format!("\"{folder}\" isn't a folder name inside the plugin (no leading /, no .., no hidden parts)"));
            continue;
        }
        let inside: Vec<&(String, PathBuf, u64)> =
            files.iter().filter(|(r, ..)| r.strip_prefix(&folder).is_some_and(|rest| rest.starts_with('/'))).collect();
        let Some((_, md_path, md_len)) = inside.iter().find(|(r, ..)| *r == format!("{folder}/SKILL.md")).copied() else {
            problems.push(format!("{folder}: there is no SKILL.md in it (a skill is a folder with its SKILL.md)"));
            continue;
        };
        let name = folder.rsplit('/').next().unwrap_or(&folder).to_string();
        let say = |p: String| format!("skill {name}: {p}");
        let mut bad = Vec::new();
        if *md_len > MAX_SKILL_MD {
            bad.push(format!("SKILL.md is bigger than {} KiB", MAX_SKILL_MD / 1024));
        }
        if inside.len() > MAX_SKILL_FILES {
            bad.push(format!("its folder has {} files; a skill has at most {MAX_SKILL_FILES}", inside.len()));
        }
        let total: u64 = inside.iter().map(|(_, _, l)| *l).sum();
        if total > MAX_SKILL_BYTES {
            bad.push(format!("its folder is {} KiB; a skill is at most {} KiB", total.div_ceil(1024), MAX_SKILL_BYTES / 1024));
        }
        if !bad.is_empty() {
            problems.extend(bad.into_iter().map(say));
            continue;
        }
        let mut others = Vec::new();
        let mut unreadable = false;
        for (rel, path, _) in &inside {
            let short = rel[folder.len() + 1..].to_string();
            match fs::read(path).ok().and_then(|b| is_text(&b)) {
                Some(t) if short != "SKILL.md" => others.push((short, t)),
                Some(_) => {}
                None => {
                    problems.push(say(format!(
                        "{short} isn't plain text: everything in a skill is text, so it can be read before it is installed"
                    )));
                    unreadable = true;
                }
            }
        }
        let text = match fs::read(md_path).ok().and_then(|b| is_text(&b)) {
            Some(t) => t,
            None => continue,
        };
        if unreadable {
            continue;
        }
        let front = match front_matter(&text) {
            Ok((f, _)) => f,
            Err(e) => {
                problems.push(say(e));
                continue;
            }
        };
        let before = problems.len();
        if !valid_name(&name) {
            problems.push(say(format!("the folder's name \"{name}\" can't be a skill's name: use lowercase letters, digits and dashes, starting with a letter, at most {NAME_MAX} characters")));
        } else if !namespaced(plugin, &name) {
            problems.push(say(format!("a plugin's skills are named after it: call it {plugin} or {plugin}-something")));
        }
        if BUILT_IN.contains(&name.as_str()) {
            problems.push(say("that is the name of platoon's own skill".into()));
        }
        if front.name.is_empty() {
            problems.push(say("its front matter has no name:".into()));
        } else if front.name != name {
            problems.push(say(format!("its front matter's name is \"{}\"; it must be the folder's name, {name}", front.name)));
        }
        if front.description.is_empty() {
            problems.push(say("its front matter has no description: (Claude reads it to know when the skill fits)".into()));
        } else if front.description.chars().count() > MAX_DESCRIPTION {
            problems.push(say(format!("its description is longer than {MAX_DESCRIPTION} characters")));
        }
        if front.keys.iter().any(|k| k == "hooks") {
            problems.push(say("it has hooks, which run commands on the user's computer; a plugin holds no code".into()));
        }
        if out.iter().any(|s| s.name == name) {
            problems.push(say("the plugin names two skills with this name".into()));
        }
        if problems.len() == before {
            out.push(Skill { name, folder, front, text, others });
        }
    }
    (problems, out)
}

// ---- in Claude Code's skills folder -------------------------------------------------------------------------------

/// A skill platoon wrote into Claude Code's skills folder for a plugin: its name, its folder there, and each file's
/// path in it and sha256 as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub name: String,
    pub dir: PathBuf,
    pub files: Vec<(String, String)>,
}

/// How a skill platoon wrote is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Now {
    /// Exactly as platoon wrote it.
    AsWritten,
    /// Someone changed it (a file changed, went or was added).
    Changed,
    /// Its folder isn't there any more.
    Gone,
}

impl Placed {
    pub fn to_json(&self) -> Value {
        let files: Map<String, Value> = self.files.iter().map(|(r, h)| (r.clone(), json!(h))).collect();
        json!({"path": self.dir.to_string_lossy(), "files": files})
    }

    pub fn from_json(name: &str, v: &Value) -> Option<Placed> {
        let dir = PathBuf::from(v.get("path")?.as_str()?);
        let files = v.get("files")?.as_object()?.iter().filter_map(|(r, h)| Some((r.clone(), h.as_str()?.to_string()))).collect();
        Some(Placed { name: name.to_string(), dir, files })
    }

    /// Is it a folder called by its name inside a `skills` folder? (A record that says otherwise is never acted on.)
    fn plausible(&self) -> bool {
        valid_name(&self.name)
            && self.dir.is_absolute()
            && self.dir.file_name().is_some_and(|n| n == self.name.as_str())
            && self.dir.parent().and_then(Path::file_name).is_some_and(|n| n == "skills")
    }

    pub fn now(&self) -> Now {
        match fs::symlink_metadata(&self.dir) {
            Err(_) => Now::Gone,
            Ok(m) if !m.is_dir() => Now::Changed,
            Ok(_) => match on_disk(&self.dir) {
                Some(mut files) => {
                    let mut want = self.files.clone();
                    files.sort();
                    want.sort();
                    if files == want { Now::AsWritten } else { Now::Changed }
                }
                None => Now::Changed,
            },
        }
    }
}

/// Every file under `dir` (hidden ones too), with its sha256; None when something there isn't a plain file or a folder,
/// or can't be read.
fn on_disk(dir: &Path) -> Option<Vec<(String, String)>> {
    fn go(root: &Path, dir: &Path, out: &mut Vec<(String, String)>, depth: usize) -> Option<()> {
        if depth > 6 || out.len() > 500 {
            return None;
        }
        for e in fs::read_dir(dir).ok()?.flatten() {
            let path = e.path();
            let meta = fs::symlink_metadata(&path).ok()?;
            if meta.is_dir() {
                go(root, &path, out, depth + 1)?;
            } else if meta.is_file() {
                let rel = path.strip_prefix(root).ok()?.to_string_lossy().into_owned();
                out.push((rel, sha256_file(&path).ok()?));
            } else {
                return None;
            }
        }
        Some(())
    }
    let mut out = Vec::new();
    go(dir, dir, &mut out, 0)?;
    out.sort();
    Some(out)
}

/// What installing (or updating) a plugin does in Claude Code's skills folder, worked out before anything is touched.
#[derive(Debug, Default)]
pub struct Plan {
    /// Skills to write: the skill, where, and whether it replaces platoon's own untouched copy there.
    write: Vec<(Skill, PathBuf, bool)>,
    /// platoon's copies the user changed: kept as they are (and their record with them).
    keep: Vec<Placed>,
    /// platoon's untouched copies of skills the new version no longer has: removed.
    drop: Vec<Placed>,
    /// Worth saying: a changed copy kept, a skill it no longer has.
    pub notes: Vec<String>,
}

/// Where a skill goes: `<claude config dir>/skills/<name>`.
pub fn target(claude: &Path, name: &str) -> PathBuf {
    claude.join("skills").join(name)
}

/// Work out what installing `skills` does, given what platoon wrote for this plugin before (`before`) and for the
/// other plugins installed (`others`: each plugin's name and its skills). Err, in plain words, when a skill's name is
/// taken by something platoon didn't write for this plugin: nothing is ever written over.
pub fn plan(claude: &Path, skills: &[Skill], before: &[Placed], others: &[(String, Vec<Placed>)]) -> Result<Plan, String> {
    let mut p = Plan::default();
    for s in skills {
        let to = target(claude, &s.name);
        if let Some((plugin, _)) = others.iter().find(|(_, placed)| placed.iter().any(|x| x.name == s.name)) {
            return Err(format!("the skill {} comes with the plugin {} already", s.name, plugin));
        }
        let mine = before.iter().find(|x| x.name == s.name && x.dir == to);
        match mine.map(Placed::now) {
            Some(Now::AsWritten) => p.write.push((s.clone(), to, true)),
            Some(Now::Changed) => {
                p.notes.push(format!(
                    "Your changed copy of the skill {} is kept as it is (in {}); the plugin's new version of it wasn't written.",
                    s.name,
                    to.display()
                ));
                p.keep.push(mine.cloned().unwrap_or_else(|| Placed { name: s.name.clone(), dir: to.clone(), files: Vec::new() }));
            }
            Some(Now::Gone) | None => {
                if fs::symlink_metadata(&to).is_ok() {
                    return Err(format!(
                        "Claude Code already has a skill called {} (in {}) that platoon didn't put there. platoon leaves it alone, so the plugin isn't installed: rename or remove that skill first",
                        s.name,
                        to.display()
                    ));
                }
                p.write.push((s.clone(), to, false));
            }
        }
    }
    for old in before.iter().filter(|o| !skills.iter().any(|s| s.name == o.name)) {
        match old.now() {
            Now::AsWritten if old.plausible() => p.drop.push(old.clone()),
            Now::Changed => p.notes.push(format!(
                "The skill {} isn't in the new version; your changed copy of it is kept (in {}).",
                old.name,
                old.dir.display()
            )),
            _ => {}
        }
    }
    Ok(p)
}

static NEXT: AtomicU64 = AtomicU64::new(0);

fn unique() -> String {
    format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst))
}

/// What `apply` changed, to keep (`commit`) or undo (`rollback`).
#[derive(Debug, Default)]
pub struct Txn {
    /// Folders put in place.
    placed: Vec<PathBuf>,
    /// Folders moved aside (where to, where from): deleted on commit, put back on rollback.
    aside: Vec<(PathBuf, PathBuf)>,
    staging: Option<PathBuf>,
}

impl Txn {
    pub fn commit(self) {
        for (moved, _) in &self.aside {
            let _ = fs::remove_dir_all(moved);
        }
        if let Some(s) = &self.staging {
            let _ = fs::remove_dir_all(s);
        }
    }

    pub fn rollback(self) {
        for p in self.placed.iter().rev() {
            let _ = fs::remove_dir_all(p);
        }
        for (moved, from) in self.aside.iter().rev() {
            let _ = fs::rename(moved, from);
        }
        if let Some(s) = &self.staging {
            let _ = fs::remove_dir_all(s);
        }
    }
}

/// Carry out a plan: each skill's files, read from the installed plugin's folder `plugin_dir`, go into a staging folder
/// inside Claude Code's skills folder, then into place (an untouched copy of platoon's moved aside first). Returns
/// what is recorded for the plugin now (the skills written and the changed copies kept) and what to commit. On an
/// error everything done so far is undone.
pub fn apply(claude: &Path, plan: &Plan, plugin_dir: &Path) -> Result<(Vec<Placed>, Txn), String> {
    let mut txn = Txn::default();
    let mut placed: Vec<Placed> = plan.keep.clone();
    if plan.write.is_empty() && plan.drop.is_empty() {
        return Ok((placed, txn));
    }
    let skills = claude.join("skills");
    let result = (|| -> Result<(), String> {
        fs::create_dir_all(&skills).map_err(|e| format!("couldn't make {}: {e}", skills.display()))?;
        let stage = skills.join(format!(".platoon-staging-{}", unique()));
        txn.staging = Some(stage.clone());
        for (s, to, replace) in &plan.write {
            let from = plugin_dir.join(&s.folder);
            let tmp = stage.join(&s.name);
            let mut files = Vec::new();
            let mut names: Vec<String> = std::iter::once("SKILL.md".to_string()).chain(s.others.iter().map(|(r, _)| r.clone())).collect();
            names.sort();
            for rel in names {
                let dst = tmp.join(&rel);
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent).map_err(|e| format!("couldn't write the skill {}: {e}", s.name))?;
                }
                let bytes = fs::read(from.join(&rel)).map_err(|e| format!("couldn't read the skill {}'s {rel}: {e}", s.name))?;
                fs::write(&dst, &bytes).map_err(|e| format!("couldn't write the skill {}: {e}", s.name))?;
                // Text, never programs: nothing is made runnable.
                fs::set_permissions(&dst, fs::Permissions::from_mode(0o644))
                    .map_err(|e| format!("couldn't write the skill {}: {e}", s.name))?;
                files.push((rel, sha256_hex(&bytes)));
            }
            if *replace {
                let moved = skills.join(format!(".platoon-old-{}-{}", s.name, unique()));
                fs::rename(to, &moved).map_err(|e| format!("couldn't move the old copy of the skill {} aside: {e}", s.name))?;
                txn.aside.push((moved, to.clone()));
            }
            fs::rename(&tmp, to).map_err(|e| format!("couldn't put the skill {} in place: {e}", s.name))?;
            txn.placed.push(to.clone());
            placed.push(Placed { name: s.name.clone(), dir: to.clone(), files });
        }
        for old in &plan.drop {
            let moved = skills.join(format!(".platoon-old-{}-{}", old.name, unique()));
            fs::rename(&old.dir, &moved).map_err(|e| format!("couldn't take the skill {} away: {e}", old.name))?;
            txn.aside.push((moved, old.dir.clone()));
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            placed.sort_by(|a, b| a.name.cmp(&b.name));
            Ok((placed, txn))
        }
        Err(e) => {
            txn.rollback();
            Err(e)
        }
    }
}

/// Take a removed plugin's skills out of Claude Code's skills folder: each one exactly as platoon wrote it goes; one
/// the user changed stays. Returns what is worth saying (each skill kept, and why).
pub fn take_away(placed: &[Placed]) -> Vec<String> {
    let mut notes = Vec::new();
    for p in placed {
        match p.now() {
            Now::Gone => {}
            Now::AsWritten if p.plausible() => {
                let moved = p.dir.with_file_name(format!(".platoon-gone-{}-{}", p.name, unique()));
                let done = fs::rename(&p.dir, &moved).and_then(|()| fs::remove_dir_all(&moved));
                if let Err(e) = done {
                    notes.push(format!("The skill {} couldn't be removed ({e}); it is still in {}.", p.name, p.dir.display()));
                }
            }
            Now::AsWritten => {}
            Now::Changed => notes.push(format!(
                "The skill {} was changed after it was installed, so it is kept (in {}): remove it yourself if you don't want it.",
                p.name,
                p.dir.display()
            )),
        }
    }
    notes
}

/// Are these skill files (path, text) what a plugin's skill says now? (For telling whether an update changes a skill's
/// instructions.)
pub fn fingerprint(s: &Skill) -> String {
    let mut all = format!("SKILL.md\0{}\n", s.text);
    let mut others = s.others.clone();
    others.sort();
    for (r, t) in others {
        all.push_str(&format!("{r}\0{t}\n"));
    }
    sha256_hex(all.as_bytes())
}

#[cfg(test)]
pub mod testing {
    //! A skill's files for tests.
    use std::{fs, path::Path};

    pub fn skill_md(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\nDo the thing carefully, then say what you did.\n")
    }

    /// Write `<plugin>/skills/<name>/SKILL.md` (and the other files).
    pub fn write_skill(plugin: &Path, name: &str, text: &str, others: &[(&str, &str)]) {
        let dir = plugin.join("skills").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), text).unwrap();
        for (rel, t) in others {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, t).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        crate::plugins::testing::tmp(&format!("skill-{name}"))
    }

    fn files_of(dir: &Path) -> Vec<(String, PathBuf, u64)> {
        let mut out = Vec::new();
        fn go(root: &Path, d: &Path, out: &mut Vec<(String, PathBuf, u64)>) {
            for e in fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    go(root, &p, out);
                } else {
                    out.push((p.strip_prefix(root).unwrap().to_string_lossy().into_owned(), p.clone(), fs::metadata(&p).unwrap().len()));
                }
            }
        }
        go(dir, dir, &mut out);
        out.sort();
        out
    }

    fn safe(p: &str) -> bool {
        !p.is_empty() && !p.starts_with('/') && p.split('/').all(|x| !x.is_empty() && !x.starts_with('.'))
    }

    #[test]
    fn front_matter_reads_the_simple_yaml_skills_use() {
        let (f, body) = front_matter("---\nname: changelog-entry\ndescription: \"Write it: plainly.\"\nallowed-tools: Bash(git log:*) Bash(git diff:*), Read\n---\n# Hi\n").unwrap();
        assert_eq!((f.name.as_str(), f.description.as_str()), ("changelog-entry", "Write it: plainly."));
        assert_eq!(f.allowed_tools, ["Bash(git log:*)", "Bash(git diff:*)", "Read"]);
        assert!(f.model_may_use && f.user_may_type);
        assert_eq!(body, "# Hi\n");
        let (f, _) = front_matter("---\nname: x\ndescription: >\n  Two lines\n  folded.\nallowed-tools:\n  - \"Bash(platoon ticket:*)\"\n  - Read\ndisable-model-invocation: true\n---\n").unwrap();
        assert_eq!(f.description, "Two lines folded.");
        assert_eq!(f.allowed_tools, ["Bash(platoon ticket:*)", "Read"]);
        assert!(!f.model_may_use);
        // platoon's own /to-tickets skill reads too (its description has ": " inside quotes).
        let (f, _) = front_matter(crate::totickets::SKILL).unwrap();
        assert_eq!(f.name, "to-tickets");
        assert!(f.description.starts_with("Turn this conversation") && f.allowed_tools.len() == 2, "{f:?}");
        assert!(front_matter("# no front matter\n").unwrap_err().contains("must start with its front matter"));
        assert!(front_matter("---\nname: x\n").unwrap_err().contains("no closing"));
    }

    #[test]
    fn a_good_skill_passes_and_each_bad_one_fails_plainly() {
        let base = tmp("check");
        let p = base.join("changelog");
        write_skill(
            &p,
            "changelog-entry",
            &skill_md("changelog-entry", "Write a changelog entry."),
            &[("templates/entry.md", "## [x.y.z]\n")],
        );
        let (problems, skills) = check("changelog", &["skills/changelog-entry".into()], &files_of(&p), safe);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(skills[0].name, "changelog-entry");
        assert_eq!(skills[0].others, [("templates/entry.md".to_string(), "## [x.y.z]\n".to_string())]);
        let reasons = |dir: &Path, entry: &str| check("changelog", &[entry.into()], &files_of(dir), safe).0.join(" | ");
        // Not namespaced, platoon's own, a name that doesn't match, no description, hooks, binary, too big.
        let q = base.join("q");
        write_skill(&q, "review", &skill_md("review", "Review."), &[]);
        assert!(reasons(&q, "skills/review").contains("a plugin's skills are named after it: call it changelog or changelog-something"));
        write_skill(&q, "changelog-x", &skill_md("changelog-y", "X."), &[]);
        assert!(reasons(&q, "skills/changelog-x").contains("it must be the folder's name, changelog-x"));
        write_skill(&q, "changelog-nodesc", "---\nname: changelog-nodesc\n---\nbody\n", &[]);
        assert!(reasons(&q, "skills/changelog-nodesc").contains("has no description"));
        write_skill(&q, "changelog-hook", "---\nname: changelog-hook\ndescription: d\nhooks:\n  PreToolUse: []\n---\n", &[]);
        assert!(reasons(&q, "skills/changelog-hook").contains("it has hooks, which run commands"));
        write_skill(&q, "changelog-bin", &skill_md("changelog-bin", "d"), &[("tool.bin", "a\0b")]);
        assert!(reasons(&q, "skills/changelog-bin").contains("tool.bin isn't plain text"));
        write_skill(&q, "changelog-big", &format!("{}{}", skill_md("changelog-big", "d"), "x".repeat(70_000)), &[]);
        assert!(reasons(&q, "skills/changelog-big").contains("SKILL.md is bigger than 64 KiB"));
        write_skill(&q, "changelog-long", &skill_md("changelog-long", &"d".repeat(1100)), &[]);
        assert!(reasons(&q, "skills/changelog-long").contains("longer than 1024 characters"));
        fs::create_dir_all(q.join("skills/changelog-empty")).unwrap();
        fs::write(q.join("skills/changelog-empty/notes.md"), "x").unwrap();
        assert!(reasons(&q, "skills/changelog-empty").contains("there is no SKILL.md in it"));
        assert!(reasons(&q, "../skills/x").contains("isn't a folder name inside the plugin"));
        let t = base.join("to-tickets");
        write_skill(&t, "to-tickets", &skill_md("to-tickets", "d"), &[]);
        assert!(check("to-tickets", &["skills/to-tickets".into()], &files_of(&t), safe).0.join(" ").contains("platoon's own skill"));
        assert!(
            namespaced("changelog", "changelog")
                && namespaced("changelog", "changelog-entry")
                && !namespaced("changelog", "changelogs")
                && !namespaced("changelog", "changelog-")
        );
    }

    fn skill(dir: &Path, name: &str, text: &str) -> Skill {
        write_skill(dir, name, text, &[]);
        let (f, _) = front_matter(text).unwrap();
        Skill { name: name.into(), folder: format!("skills/{name}"), front: f, text: text.into(), others: Vec::new() }
    }

    #[test]
    fn skills_are_written_only_where_free_kept_when_changed_and_taken_away_only_as_written() {
        let base = tmp("place");
        let claude = base.join("claude");
        let plugin = base.join("plugin");
        let s = skill(&plugin, "changelog-entry", &skill_md("changelog-entry", "Write it."));
        // Free: written, recorded.
        let p = plan(&claude, std::slice::from_ref(&s), &[], &[]).unwrap();
        let (placed, txn) = apply(&claude, &p, &plugin).unwrap();
        txn.commit();
        assert_eq!(placed.len(), 1);
        let dir = claude.join("skills/changelog-entry");
        assert_eq!(fs::read_to_string(dir.join("SKILL.md")).unwrap(), s.text);
        assert_eq!(fs::metadata(dir.join("SKILL.md")).unwrap().permissions().mode() & 0o111, 0);
        assert_eq!(placed[0].now(), Now::AsWritten);
        assert!(
            fs::read_dir(claude.join("skills")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with('.')),
            "no staging left"
        );
        // An untouched copy is replaced by a new version.
        let s2 = skill(&plugin, "changelog-entry", &skill_md("changelog-entry", "Write it better."));
        let p = plan(&claude, std::slice::from_ref(&s2), &placed, &[]).unwrap();
        let (placed, txn) = apply(&claude, &p, &plugin).unwrap();
        txn.commit();
        assert!(fs::read_to_string(dir.join("SKILL.md")).unwrap().contains("better"));
        // Changed by the user: kept on update (said), and kept on removal (said).
        fs::write(dir.join("SKILL.md"), "mine now").unwrap();
        assert_eq!(placed[0].now(), Now::Changed);
        let p = plan(&claude, std::slice::from_ref(&s), &placed, &[]).unwrap();
        assert!(p.notes[0].contains("Your changed copy of the skill changelog-entry is kept"), "{:?}", p.notes);
        let (kept, txn) = apply(&claude, &p, &plugin).unwrap();
        txn.commit();
        assert_eq!(fs::read_to_string(dir.join("SKILL.md")).unwrap(), "mine now");
        assert_eq!(kept, placed, "the record of the kept copy stays");
        let notes = take_away(&kept);
        assert!(notes[0].contains("was changed after it was installed, so it is kept"), "{notes:?}");
        assert!(dir.join("SKILL.md").exists());
        // As written: taken away. A file the user added counts as a change.
        fs::remove_dir_all(&dir).unwrap();
        let p = plan(&claude, std::slice::from_ref(&s), &[], &[]).unwrap();
        let (placed, txn) = apply(&claude, &p, &plugin).unwrap();
        txn.commit();
        fs::write(dir.join("extra.md"), "mine").unwrap();
        assert_eq!(placed[0].now(), Now::Changed);
        fs::remove_file(dir.join("extra.md")).unwrap();
        assert!(take_away(&placed).is_empty());
        assert!(!dir.exists());
        assert!(take_away(&placed).is_empty(), "gone already: nothing to say");
    }

    #[test]
    fn a_skill_folder_platoon_did_not_write_is_never_written_over() {
        let base = tmp("clash");
        let claude = base.join("claude");
        let plugin = base.join("plugin");
        let s = skill(&plugin, "changelog-entry", &skill_md("changelog-entry", "Write it."));
        let theirs = claude.join("skills/changelog-entry");
        fs::create_dir_all(&theirs).unwrap();
        fs::write(theirs.join("SKILL.md"), "the user's own").unwrap();
        let e = plan(&claude, std::slice::from_ref(&s), &[], &[]).unwrap_err();
        assert!(e.contains("Claude Code already has a skill called changelog-entry") && e.contains("platoon didn't put there"), "{e}");
        assert_eq!(fs::read_to_string(theirs.join("SKILL.md")).unwrap(), "the user's own");
        // Another plugin's.
        let other = Placed { name: "changelog-entry".into(), dir: theirs.clone(), files: Vec::new() };
        let e = plan(&claude, std::slice::from_ref(&s), &[], &[("Other".into(), vec![other])]).unwrap_err();
        assert!(e.contains("comes with the plugin Other already"), "{e}");
        // A rollback puts back what was there.
        fs::remove_dir_all(&theirs).unwrap();
        let p = plan(&claude, std::slice::from_ref(&s), &[], &[]).unwrap();
        let (_, txn) = apply(&claude, &p, &plugin).unwrap();
        txn.rollback();
        assert!(!theirs.exists());
        // A record that doesn't point into a skills folder is never acted on.
        let odd = Placed { name: "changelog-entry".into(), dir: base.join("elsewhere/changelog-entry"), files: Vec::new() };
        fs::create_dir_all(&odd.dir).unwrap();
        assert!(take_away(std::slice::from_ref(&odd)).is_empty());
        assert!(odd.dir.exists());
    }
}
