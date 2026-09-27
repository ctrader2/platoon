//! `platoon plugin ...`: the catalog and the installed plugins from the command line. `list`, `add <id|folder>`,
//! `remove <id>`, `update [id]`, `show <id|folder>`, and `check <folder>` for someone making a plugin. The work is in
//! `plugins.rs`; for the code editor plugin also its downloads (`editorplugin.rs`): adding or updating it fetches what
//! the editor folder lacks, removing it takes the folder and the font off. `platoon setup --add/--remove editor` come
//! here too.
//!
//! A plugin that adds instructions Claude follows (skills, squads, agents) is installed, or updated when its
//! instructions change, only after they are shown: the summary is printed (`pluginreview::summary`, the whole text is
//! `show`) and y/N asked in a terminal; where there is no terminal to ask in, only with `--yes`.

use crate::editor_install::{self, Dirs, Printer};
use crate::editorplugin;
use crate::pluginreview;
use crate::plugins::{self, Catalog, Origin, Outcome, Source, Store, Updated};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

/// What a `platoon plugin` command works on.
pub struct Ctx<'a> {
    pub store: &'a Store,
    pub source: &'a Source,
    /// The code editor's folders (its plugin's downloads go there).
    pub dirs: &'a Dirs,
    pub home: &'a str,
    /// Shows the editor's download progress.
    pub printer: &'a Printer,
    /// platoon's config folder (the user's own squads, for what removing a plugin's agents would leave missing).
    pub config: &'a Path,
    /// Ask the user a yes/no question in the terminal: None when there is no terminal to ask in.
    pub ask: &'a dyn Fn(&str) -> Option<bool>,
}

pub const USAGE: &str = "usage: platoon plugin list | add <id or folder> [--yes] | remove <id> | update [id] [--yes] | show <id or folder> | check <folder>
  list             the plugins in platoon's catalog, and the ones installed
  add <id>         install a plugin from the catalog
  add <folder>     install a plugin from a folder on this computer (for making one; say ./name for a folder here)
                   A plugin with skills, squads or agents (instructions Claude follows) is shown first and
                   asked about; --yes installs it without asking (read it first with show)
  remove <id>      take a plugin off again
  update [id]      bring the installed plugins (or one) up to the catalog's version (changed instructions are
                   shown and asked about again; --yes updates without asking)
  show <id>        everything a plugin adds that Claude follows, in full: each skill, squad and agent
  check <folder>   check a plugin folder before sending it: its plugin.json, its files, what it adds";

/// Runs `platoon plugin <args>`; the exit code: 0 done, 1 it didn't work, 2 the command was wrong.
pub fn cli(args: &[String], c: &Ctx, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let yes = args.iter().any(|a| a == "--yes" || a == "-y");
    let args: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "--yes" && *a != "-y").collect();
    let arg = |i: usize| args.get(i).copied();
    if !matches!(arg(0), Some("check" | "show") | None) {
        take_over_editor(c, out, err);
    }
    match (arg(0), arg(1), args.len()) {
        (Some("list"), None, 1) => list(c, out, err),
        (Some("add"), Some(what), 2) => add(c, what, yes, out, err),
        (Some("remove"), Some(id), 2) => remove(c, id, out, err),
        (Some("update"), only, 1 | 2) => update(c, only, yes, out, err),
        (Some("show"), Some(what), 2) => show(c, what, out, err),
        (Some("check"), Some(dir), 2) => check(Path::new(dir), out),
        _ => {
            let _ = writeln!(err, "{USAGE}");
            2
        }
    }
}

/// An editor installed before it was a plugin becomes the editor plugin (no network, nothing downloaded), said once.
fn take_over_editor(c: &Ctx, out: &mut dyn Write, err: &mut dyn Write) {
    match editorplugin::adopt(c.store, c.dirs) {
        Some(Ok(p)) => {
            let _ = writeln!(out, "{}", editorplugin::adopted_words(&p));
        }
        Some(Err(e)) => {
            let _ = writeln!(err, "Your code editor couldn't be made a plugin yet ({e}); it keeps working.");
        }
        None => {}
    }
}

/// `platoon setup --add editor` / `--remove editor`: the code editor is a plugin now; said, then done as
/// `platoon plugin add editor` / `remove editor`.
pub fn setup_alias(flag: &str, c: &Ctx, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let verb = if flag == "--remove" { "remove" } else { "add" };
    let _ = writeln!(out, "The code editor is a plugin now: this is `platoon plugin {verb} {}`.", editorplugin::ID);
    cli(&[verb.to_string(), editorplugin::ID.to_string()], c, out, err)
}

/// Before a plugin with instructions is installed (or updated with changed ones): the summary, then y/N in a
/// terminal, or `--yes`. Ok: go on. Err: the exit code, nothing changed (0 when the person answered no, 1 otherwise).
fn confirmed(c: &Ctx, dir: &Path, installed: Option<PathBuf>, update: Option<(&str, &str)>, yes: bool, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), i32> {
    let (m, review) = match pluginreview::review(dir, installed.as_deref()) {
        Ok(x) => x,
        Err(e) => {
            let _ = writeln!(err, "{e}");
            return Err(1);
        }
    };
    let needs = m.has_instructions() && (installed.is_none() || review.changed());
    if !needs {
        return Ok(());
    }
    let _ = write!(out, "{}", pluginreview::summary(&m, &review, update));
    if yes {
        return Ok(());
    }
    let verb = if update.is_some() { "Update" } else { "Install" };
    let _ = out.flush();
    match (c.ask)(&format!("{verb} it? [y/N] ")) {
        Some(true) => Ok(()),
        Some(false) => {
            // The person said no: that is an answer, not a failure, so the exit code is 0.
            let _ = writeln!(out, "Not {}: nothing was changed.", if update.is_some() { "updated" } else { "installed" });
            Err(0)
        }
        None => {
            let cmd = if update.is_some() { format!("platoon plugin update {} --yes", m.id) } else { format!("platoon plugin add {} --yes", m.id) };
            let _ = writeln!(
                err,
                "Not {}: it has instructions Claude follows with your permissions, so platoon asks first, and there is no terminal here to ask in. Read them (platoon plugin show {}), then run `{cmd}`.",
                if update.is_some() { "updated" } else { "installed" },
                m.id
            );
            Err(1)
        }
    }
}

/// Fetch what the installed editor plugin's table has and the editor folder doesn't, with progress; says how it went.
/// True when it finished.
fn editor_downloads(c: &Ctx, out: &mut dyn Write, err: &mut dyn Write) -> bool {
    if editorplugin::complete(c.store, c.dirs) {
        return true;
    }
    let _ = writeln!(out, "Downloading the code editor's parts ({})…", editorplugin::size_words(editorplugin::download_bytes(c.store)));
    let _ = out.flush();
    let done = editorplugin::install_downloads(c.store, c.dirs, &|p| c.printer.update(p));
    c.printer.finish();
    match done {
        Ok(r) => {
            let _ = writeln!(out, "{}", editor_install::render_report(&r, c.dirs));
            true
        }
        Err(e) => {
            let _ = writeln!(err, "The code editor plugin is installed, but its downloads didn't finish.\n{e}\n`platoon plugin add {}` tries again.", editorplugin::ID);
            false
        }
    }
}
fn fetch(store: &Store, source: &Source, out: &mut dyn Write) -> Result<Catalog, String> {
    let _ = writeln!(out, "Fetching the plugin catalog…");
    let _ = out.flush();
    plugins::fetch(store, source)
}

fn list(c: &Ctx, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (store, source, home) = (c.store, c.source, c.home);
    let installed = store.installed();
    let catalog = match fetch(store, source, out) {
        Ok(c) => Some(c),
        Err(e) => {
            let _ = writeln!(err, "{e}");
            let old = plugins::cached(store, source);
            if old.is_some() {
                let _ = writeln!(err, "Showing the catalog as it was when it was last fetched.");
            }
            old
        }
    };
    let code = i32::from(catalog.is_none());
    if let Some(c) = &catalog {
        let _ = writeln!(out, "\nIn the catalog (commit {}):", plugins::short(&c.commit));
        if c.entries.is_empty() {
            let _ = writeln!(out, "  nothing yet");
        }
        let w = c.entries.iter().map(|e| e.manifest.id.len()).max().unwrap_or(0);
        for e in &c.entries {
            let m = &e.manifest;
            let state = match installed.iter().find(|p| p.id() == m.id) {
                Some(p) if plugins::update_available(p, c) => format!("  [installed {}, update available]", p.manifest.version),
                Some(_) => "  [installed]".to_string(),
                None if !e.problems.is_empty() => format!("  [can't be installed: {}]", e.problems.join("; ")),
                None => String::new(),
            };
            let adds = match e.download_bytes {
                Some(b) => format!("{}, downloads {}", m.adds_words(), editorplugin::size_words(b)),
                None => m.adds_words(),
            };
            let _ = writeln!(out, "  {:<w$}  {:<8} {} ({adds}){state}", m.id, m.version, m.name);
            let _ = writeln!(out, "  {:<w$}  {}", "", m.description);
        }
        for s in &c.skipped {
            let _ = writeln!(out, "  (left out: {s})");
        }
    }
    let _ = writeln!(out, "\nInstalled:");
    if installed.is_empty() {
        let _ = writeln!(out, "  none");
    }
    let w = installed.iter().map(|p| p.id().len()).max().unwrap_or(0);
    for p in &installed {
        let m = &p.manifest;
        let _ = writeln!(out, "  {:<w$}  {:<8} {} ({}), from {}", m.id, m.version, m.name, m.adds_words(), p.origin.words(home));
        if m.editor.is_some() && !editorplugin::complete(store, c.dirs) {
            let _ = writeln!(out, "  {:<w$}  its downloads aren't complete: `platoon plugin add {}` finishes them", "", m.id);
        }
    }
    code
}

/// A folder rather than an id: it has a slash, starts with a dot or a tilde, or is a folder here and not an id.
fn is_folder(what: &str) -> bool {
    what.contains('/') || what.starts_with('.') || what.starts_with('~') || !plugins::valid_id(what)
}

fn say_outcome(o: &Outcome, out: &mut dyn Write) {
    let m = &o.plugin().manifest;
    let next = match plugins::next_words(m) {
        n if n.is_empty() => String::new(),
        n => format!(" {n}"),
    };
    let _ = match o {
        Outcome::New(_) => writeln!(out, "Installed {} {} ({}).{next}", m.name, m.version, m.adds_words()),
        Outcome::Replaced { was, .. } if *was == m.version => writeln!(out, "Installed {} {} from the catalog, in place of the copy there was.", m.name, m.version),
        Outcome::Replaced { was, .. } => writeln!(out, "Updated {} from {was} to {}.", m.name, m.version),
        Outcome::Same(_) => writeln!(out, "{} {} is already installed, with the same files.", m.name, m.version),
    };
}

/// After a plugin is installed: what is worth saying, and the code editor's downloads, when it adds the editor. The
/// exit code.
fn after_install(c: &Ctx, o: &Outcome, notes: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    say_outcome(o, out);
    for n in notes {
        let _ = writeln!(out, "{n}");
    }
    if o.plugin().manifest.editor.is_some() && !editor_downloads(c, out, err) {
        return 1;
    }
    0
}

/// A folder given on the command line: `~/` means the home folder. None: there is none there.
fn folder_of(what: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = match what.strip_prefix("~/") {
        Some(rest) if !home.is_empty() => Path::new(&home).join(rest),
        _ => Path::new(what).to_path_buf(),
    };
    expanded.canonicalize().ok()
}

/// The installed copy of plugin `id`, when there is one: what an update is compared with.
fn installed_dir(store: &Store, id: &str) -> Option<PathBuf> {
    store.get(id).map(|p| store.plugin_dir(p.id()))
}

/// The id a plugin folder's manifest gives (for comparing with the installed copy); None when it can't be read.
fn id_in(dir: &Path) -> Option<String> {
    plugins::check(dir).manifest.map(|m| m.id)
}

fn add(c: &Ctx, what: &str, yes: bool, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (store, source) = (c.store, c.source);
    if is_folder(what) {
        let Some(dir) = folder_of(what) else {
            let _ = writeln!(err, "There is no folder at {what}.");
            return 1;
        };
        let was = id_in(&dir).and_then(|id| store.get(&id));
        let update = was.as_ref().map(|p| p.manifest.version.clone());
        let to = plugins::check(&dir).manifest.map(|m| m.version).unwrap_or_default();
        if let Err(code) = confirmed(c, &dir, was.as_ref().map(|p| store.plugin_dir(p.id())), update.as_deref().map(|f| (f, to.as_str())), yes, out, err) {
            return code;
        }
        return match store.install_noting(&dir, Origin::Folder { path: dir.to_string_lossy().into_owned() }) {
            Ok((o, notes)) => after_install(c, &o, &notes, out, err),
            Err(e) => {
                let _ = writeln!(err, "{e}");
                1
            }
        };
    }
    let catalog = match fetch(store, source, out) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(err, "{e}");
            return 1;
        }
    };
    let Some(entry) = catalog.get(what) else {
        let _ = writeln!(err, "There is no plugin called \"{what}\" in the catalog. `platoon plugin list` shows what there is.");
        return 1;
    };
    if !entry.problems.is_empty() {
        let _ = writeln!(err, "{} can't be installed: {}", entry.manifest.name, entry.problems.join("; "));
        return 1;
    }
    let was = store.get(what);
    if !was.as_ref().is_some_and(|p| p.sha256 == entry.sha256) {
        let update = was.as_ref().map(|p| (p.manifest.version.clone(), entry.manifest.version.clone()));
        if let Err(code) = confirmed(c, &entry.dir, installed_dir(store, what), update.as_ref().map(|(a, b)| (a.as_str(), b.as_str())), yes, out, err) {
            return code;
        }
    }
    let origin = Origin::Catalog { source: catalog.source.clone(), rf: catalog.rf.clone(), commit: catalog.commit.clone() };
    match store.install_noting(&entry.dir, origin) {
        Ok((o, notes)) => after_install(c, &o, &notes, out, err),
        Err(e) => {
            let _ = writeln!(err, "{e}");
            1
        }
    }
}

fn remove(c: &Ctx, id: &str, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let store = c.store;
    if editorplugin::is_editor(store, id) {
        return match editorplugin::remove(store, c.dirs, id) {
            Ok((p, r)) => {
                let name = p.map_or_else(|| "The code editor".to_string(), |p| p.manifest.name);
                let what = if r.editor { format!(" and everything it downloaded ({}, {})", c.dirs.editor.display(), editor_install::megabytes(r.bytes)) } else { String::new() };
                let font = if r.font_files > 0 { ", with the font platoon installed" } else { "" };
                let _ = writeln!(out, "Removed {name}{what}{font}. Your own Neovim and its settings are not touched; `platoon plugin add {id}` puts it back.");
                0
            }
            Err(e) => {
                let _ = writeln!(err, "{e}");
                1
            }
        };
    }
    // Asked before it goes: the user's own squads that use its agents.
    let users = crate::squadplugin::users_of_plugin(c.config, id);
    match store.remove_noting(id) {
        Ok((Some(p), notes)) => {
            let m = &p.manifest;
            let mut said = format!("Removed {} ({} no longer offered).", m.name, m.adds_words());
            if !m.themes().is_empty() {
                said.push_str(" A theme of it that was in use gives way to the default.");
            }
            if !m.files(crate::squadplugin::SQUADS).is_empty() {
                said.push_str(" Tickets its squads already have keep working as they started.");
            }
            let _ = writeln!(out, "{said}");
            if let Some(w) = crate::squadplugin::removal_warning(&users) {
                let _ = writeln!(out, "{}", w.replace(" will show ", " show "));
            }
            for n in notes {
                let _ = writeln!(out, "{n}");
            }
            0
        }
        Ok((None, _)) if id == editorplugin::ID => {
            let _ = writeln!(out, "The code editor plugin isn't installed; there is nothing to remove.");
            0
        }
        Ok((None, _)) => {
            let _ = writeln!(err, "No plugin called \"{id}\" is installed. `platoon plugin list` shows what is.");
            1
        }
        Err(e) => {
            let _ = writeln!(err, "{e}");
            1
        }
    }
}

fn update(c: &Ctx, only: Option<&str>, yes: bool, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (store, source) = (c.store, c.source);
    let installed = store.installed();
    if let Some(id) = only {
        if !installed.iter().any(|p| p.id() == id) {
            let _ = writeln!(err, "No plugin called \"{id}\" is installed.");
            return 1;
        }
    } else if installed.is_empty() {
        let _ = writeln!(out, "No plugins are installed.");
        return 0;
    }
    let catalog = match fetch(store, source, out) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(err, "{e}");
            return 1;
        }
    };
    let mut code = 0;
    let editor_name = editorplugin::installed(store).map(|p| p.manifest.name);
    let mut results = Vec::new();
    for p in installed.iter().filter(|p| only.is_none_or(|id| p.id() == id)) {
        // Changed instructions are shown and asked about again first.
        if let Some(e) = catalog.get(p.id()).filter(|e| plugins::update_available(p, &catalog) && (e.manifest.has_instructions() || p.manifest.has_instructions())) {
            let versions = (p.manifest.version.clone(), e.manifest.version.clone());
            if let Err(c2) = confirmed(c, &e.dir, Some(store.plugin_dir(p.id())), Some((&versions.0, &versions.1)), yes, out, err) {
                code = code.max(c2);
                continue;
            }
        }
        results.extend(plugins::update(store, &catalog, Some(p.id())));
    }
    for u in results {
        let _ = match u {
            Updated::Done { name, from, to, notes } => {
                if from == to {
                    let _ = writeln!(out, "Updated {name} ({to}): its files changed.");
                } else {
                    let _ = writeln!(out, "Updated {name} from {from} to {to}.");
                }
                for n in notes {
                    let _ = writeln!(out, "{n}");
                }
                Ok(())
            }
            Updated::UpToDate { name } => writeln!(out, "{name} is up to date."),
            Updated::FromFolder { name } => writeln!(out, "{name} was installed from a folder: add that folder again to update it."),
            Updated::Gone { name } => writeln!(out, "{name} is no longer in the catalog; it stays installed."),
            Updated::Failed { name, why } => {
                code = 1;
                writeln!(err, "{name} couldn't be updated: {why}")
            }
        };
    }
    // The editor's downloads follow its table (a new one, or an earlier download that didn't finish).
    let editor_here = editorplugin::installed(store).is_some_and(|p| only.is_none_or(|id| p.id() == id));
    if editor_name.is_some() && editor_here && !editor_downloads(c, out, err) {
        code = 1;
    }
    code
}

/// `platoon plugin show <id or folder>`: everything a plugin adds that Claude follows, in full. From the catalog (as
/// last fetched, when it can't be reached), else the installed copy; or a folder.
fn show(c: &Ctx, what: &str, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (store, source) = (c.store, c.source);
    let (dir, from) = if is_folder(what) {
        match folder_of(what) {
            Some(d) => (d, "a folder on this computer".to_string()),
            None => {
                let _ = writeln!(err, "There is no folder at {what}.");
                return 1;
            }
        }
    } else {
        let catalog = match plugins::fetch(store, source) {
            Ok(cat) => Some(cat),
            Err(e) => {
                let old = plugins::cached(store, source);
                if old.is_none() && store.get(what).is_none() {
                    let _ = writeln!(err, "{e}");
                    return 1;
                }
                old
            }
        };
        match (catalog.as_ref().and_then(|cat| cat.get(what).map(|e| (e.dir.clone(), cat.commit.clone()))), store.get(what)) {
            (Some((d, commit)), _) => (d, format!("the catalog (commit {})", plugins::short(&commit))),
            (None, Some(p)) => (store.plugin_dir(p.id()), format!("the copy installed here, from {}", p.origin.words(c.home))),
            (None, None) => {
                let _ = writeln!(err, "There is no plugin called \"{what}\" in the catalog or installed. `platoon plugin list` shows what there is.");
                return 1;
            }
        }
    };
    let installed = id_in(&dir).and_then(|id| installed_dir(store, &id)).filter(|d| *d != dir);
    let (m, review) = match pluginreview::review(&dir, installed.as_deref()) {
        Ok(x) => x,
        Err(e) => {
            let _ = writeln!(err, "{e}");
            return 1;
        }
    };
    let license = m.license.as_deref().map_or(String::new(), |l| format!(", license {l}"));
    let _ = writeln!(out, "{} {} by {}{license}\n{}\nFrom {from}. It adds {}.", m.name, m.version, m.author, m.description, m.adds_words());
    if review.items.is_empty() {
        let _ = writeln!(out, "It has nothing Claude follows: no skills, squads or agents.");
        return 0;
    }
    let _ = writeln!(out, "Claude follows what is below with your permissions: read it before you install it.");
    for i in &review.items {
        let mark = match (installed.is_some(), i.change) {
            (true, pluginreview::Change::New) => " (new in this version)",
            (true, pluginreview::Change::Changed) => " (changed in this version)",
            _ => "",
        };
        let _ = writeln!(out, "\n==== {}: {}{mark} ====\n", i.kind.word(), i.name);
        let _ = write!(out, "{}", i.text);
    }
    for g in &review.gone {
        let _ = writeln!(out, "\nNo longer in this version: {g}.");
    }
    0
}

fn check(dir: &Path, out: &mut dyn Write) -> i32 {
    let r = plugins::check(dir);
    let _ = writeln!(out, "Checking {}", dir.display());
    if let Some(m) = &r.manifest {
        let license = m.license.as_deref().map_or(String::new(), |l| format!(", license {l}"));
        let _ = writeln!(out, "  plugin.json: {} ({}) {} by {}{license}, for platoon {} or newer", m.name, m.id, m.version, m.author, m.platoon);
        let _ = writeln!(out, "  adds: {}", if r.themes.is_empty() { m.adds_words() } else { format!("{} theme{}: {}", r.themes.len(), if r.themes.len() == 1 { "" } else { "s" }, r.themes.join(", ")) });
        let names = |v: Vec<String>| v.join(", ");
        if !r.skills.is_empty() {
            let _ = writeln!(out, "  skills: {} (SKILL.md at most {} KiB; a skill's folder at most {} files, {} KiB, text only)", names(r.skills.iter().map(|s| s.name.clone()).collect()), crate::skillplugin::MAX_SKILL_MD / 1024, crate::skillplugin::MAX_SKILL_FILES, crate::skillplugin::MAX_SKILL_BYTES / 1024);
        }
        if !r.squads.is_empty() {
            let _ = writeln!(out, "  squads: {}", names(r.squads.iter().map(|s| format!("{} ({})", s.name, s.id)).collect()));
        }
        if !r.agents.is_empty() {
            let _ = writeln!(out, "  agents: {}", names(r.agents.iter().map(|a| format!("{} ({})", a.name, a.id)).collect()));
        }
    }
    let _ = writeln!(out, "  {} file{}, {} KiB (at most {} KiB)", r.files, if r.files == 1 { "" } else { "s" }, r.bytes.div_ceil(1024), plugins::MAX_PLUGIN_BYTES / 1024);
    for n in &r.notes {
        let _ = writeln!(out, "  note: {n}");
    }
    if r.ok() {
        let _ = writeln!(out, "Looks good: it can be installed, and sent to the catalog.");
        0
    } else {
        let _ = writeln!(out, "Problems:");
        for p in &r.problems {
            let _ = writeln!(out, "  - {p}");
        }
        let _ = writeln!(out, "Not ready: fix the problems above and check again.");
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::testing::*;
    use std::path::PathBuf;

    fn run(args: &[&str], store: &Store, source: &Source) -> (i32, String, String) {
        // The editor's folders beside the store's (a test of the editor gives its own, with a mirror).
        let base = store.dir.parent().unwrap().parent().unwrap();
        run_with(args, store, source, &crate::editor_install::testing::dirs(base, vec![]))
    }

    fn run_with(args: &[&str], store: &Store, source: &Source, dirs: &Dirs) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let printer = Printer::quiet();
        let config = store.dir.parent().unwrap().parent().unwrap().join("config");
        let c = Ctx { store, source, dirs, home: "/home/u", printer: &printer, config: &config, ask: &|_| None };
        let code = cli(&args, &c, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn the_code_editor_plugin_from_the_command_line_and_the_setup_aliases() {
        use crate::editor_install::testing as fake;
        let base = tmp("cli-editor");
        let dirs = fake::dirs(&base, vec![fake::fake_cc(&base)]);
        let (repo, _) = crate::editorplugin::testing::catalog(&base.join("cat"), dirs.mirror.as_ref().unwrap());
        let store = Store::new(base.join("data/plugins"));
        let src = source(&repo);
        let r = |args: &[&str]| run_with(args, &store, &src, &dirs);
        let (code, out, _) = r(&["list"]);
        assert_eq!(code, 0);
        assert!(out.contains("Code editor (Neovim) (the code editor, downloads about 0 MB)"), "{out}");
        // `platoon setup --add editor`: said, then the plugin and its downloads.
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let printer = Printer::quiet();
        let config = base.join("config");
        let c = Ctx { store: &store, source: &src, dirs: &dirs, home: "/home/u", printer: &printer, config: &config, ask: &|_| None };
        assert_eq!(setup_alias("--add", &c, &mut o, &mut e), 0, "{}", String::from_utf8_lossy(&e));
        let out = String::from_utf8(o).unwrap();
        assert!(out.starts_with("The code editor is a plugin now: this is `platoon plugin add editor`.\n"), "{out}");
        assert!(out.contains("Installed Code editor (Neovim) 1.0.0 (the code editor).") && out.contains("Downloading the code editor's parts") && out.contains("The code editor is ready:"), "{out}");
        assert!(crate::editorplugin::complete(&store, &dirs));
        let (_, out, _) = r(&["list"]);
        assert!(out.contains("[installed]") && !out.contains("aren't complete"), "{out}");
        // A part gone: list says so, update (nothing new in the catalog) fetches it again.
        std::fs::remove_dir_all(dirs.site().join("pack/platoon/start/alpha.nvim")).unwrap();
        let (_, out, _) = r(&["list"]);
        assert!(out.contains("its downloads aren't complete: `platoon plugin add editor` finishes them"), "{out}");
        let (code, out, _) = r(&["update"]);
        assert!(code == 0 && out.contains("Code editor (Neovim) is up to date.") && out.contains("Downloading the code editor's parts"), "{out}");
        assert!(crate::editorplugin::complete(&store, &dirs));
        // Downloads that can't finish: the plugin stays, said plainly, exit 1.
        std::fs::remove_dir_all(dirs.site().join("pack/platoon/start/alpha.nvim")).unwrap();
        let mirror = dirs.mirror.clone().unwrap();
        std::fs::rename(&mirror, base.join("mirror-away")).unwrap();
        let (code, _, err) = r(&["add", "editor"]);
        assert!(code == 1 && err.contains("its downloads didn't finish") && err.contains("`platoon plugin add editor` tries again"), "{err}");
        std::fs::rename(base.join("mirror-away"), &mirror).unwrap();
        // `platoon setup --remove editor`: the plugin, the folder and the font.
        let (mut o, mut e) = (Vec::new(), Vec::new());
        assert_eq!(setup_alias("--remove", &c, &mut o, &mut e), 0);
        let out = String::from_utf8(o).unwrap();
        assert!(out.starts_with("The code editor is a plugin now: this is `platoon plugin remove editor`."), "{out}");
        assert!(out.contains("Removed Code editor (Neovim) and everything it downloaded (") && out.contains("with the font platoon installed"), "{out}");
        assert!(!dirs.editor.exists() && !dirs.fonts.exists() && store.get("editor").is_none());
        let (code, out, _) = r(&["remove", "editor"]);
        assert!(code == 0 && out.contains("isn't installed; there is nothing to remove"), "{out}");
    }

    /// A catalog repository holding the plugin with a skill, a squad and two agents (`squadplugin::testing::plugin`).
    fn instructions_catalog(base: &Path, version: &str) -> PathBuf {
        let repo = base.join("source");
        let cat = repo.join(plugins::CATALOG_FOLDER);
        let _ = std::fs::remove_dir_all(cat.join("changelog"));
        crate::squadplugin::testing::plugin(&cat, "changelog", version);
        std::fs::write(cat.join(plugins::INDEX), serde_json::json!({"plugins": [{"id": "changelog", "version": version}]}).to_string()).unwrap();
        if !repo.join(".git").exists() {
            git(&repo, &["init", "--quiet"]);
        }
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "--quiet", "-m", "catalog"]);
        repo
    }

    fn run_asking(args: &[&str], store: &Store, source: &Source, answer: Option<bool>) -> (i32, String, String) {
        let base = store.dir.parent().unwrap().parent().unwrap();
        let dirs = crate::editor_install::testing::dirs(base, vec![]);
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let printer = Printer::quiet();
        let config = base.join("config");
        let asked = std::cell::Cell::new(0);
        let ask = |_: &str| {
            asked.set(asked.get() + 1);
            answer
        };
        let c = Ctx { store, source, dirs: &dirs, home: "/home/u", printer: &printer, config: &config, ask: &ask };
        let code = cli(&args, &c, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn a_plugin_with_instructions_is_shown_and_asked_about_and_refused_without_a_terminal_unless_yes() {
        let base = tmp("cli-instructions");
        let repo = instructions_catalog(&base, "1.0.0");
        let claude = base.join("claude");
        let store = Store::with_claude(base.join("data/plugins"), claude.clone());
        let src = source(&repo);
        let skill = claude.join("skills/changelog-entry/SKILL.md");
        // show: everything, in full, without installing anything.
        let (code, out, err) = run_asking(&["show", "changelog"], &store, &src, None);
        assert_eq!(code, 0, "{err}");
        assert!(out.starts_with("changelog name 1.0.0 by Someone, license MIT\nWhat changelog is.\nFrom the catalog (commit "), "{out}");
        for want in ["==== Skill: changelog-entry ====", "Do the thing carefully", "==== Squad: changelog squad ====", "2. **changelog-writer** (this plugin)", "==== Agent: change-reader ====", crate::squadplugin::testing::READER, "- Can run commands on this computer"] {
            assert!(out.contains(want), "{want}: {out}");
        }
        assert!(store.installed().is_empty() && !skill.exists());
        // No terminal to ask in: refused, nothing written.
        let (code, out, err) = run_asking(&["add", "changelog"], &store, &src, None);
        assert_eq!(code, 1);
        assert!(out.contains("changelog name 1.0.0 adds instructions Claude follows with your permissions:") && out.contains("Read them in full: platoon plugin show changelog"), "{out}");
        assert!(err.contains("Not installed: it has instructions Claude follows with your permissions, so platoon asks first, and there is no terminal here to ask in.") && err.contains("platoon plugin add changelog --yes"), "{err}");
        assert!(store.installed().is_empty() && !skill.exists());
        // Asked, and no.
        let (code, out, _) = run_asking(&["add", "changelog"], &store, &src, Some(false));
        assert!(code == 0 && out.contains("Not installed: nothing was changed."), "{out}");
        assert!(store.installed().is_empty());
        // Asked, and yes.
        let (code, out, err) = run_asking(&["add", "changelog"], &store, &src, Some(true));
        assert_eq!(code, 0, "{err}");
        assert!(out.contains("Installed changelog name 1.0.0 (1 skill, 1 squad, 2 agents). Its squads are on the Squads tab (3)") && out.contains("type /changelog-entry in a new Claude session"), "{out}");
        assert!(skill.is_file());
        // The same again: nothing to ask.
        let (code, out, _) = run_asking(&["add", "changelog"], &store, &src, None);
        assert!(code == 0 && out.contains("already installed"), "{out}");
        // An update that changes an agent's instructions asks again; not in a terminal it waits for --yes.
        let repo = instructions_catalog(&base, "1.1.0");
        let reader = repo.join("plugins/changelog/agents/change-reader.json");
        std::fs::write(&reader, std::fs::read_to_string(&reader).unwrap().replace("change nothing", "change nothing at all")).unwrap();
        git(&repo, &["commit", "--quiet", "-am", "reader"]);
        let (code, out, err) = run_asking(&["update"], &store, &src, None);
        assert_eq!(code, 1);
        assert!(out.contains("changelog name 1.0.0 -> 1.1.0 changes instructions Claude follows with your permissions. Read them again:") && out.contains("Agent change-reader  [changed]"), "{out}");
        assert!(err.contains("Not updated") && err.contains("platoon plugin update changelog --yes"), "{err}");
        assert_eq!(store.get("changelog").unwrap().manifest.version, "1.0.0");
        let (code, out, err) = run_asking(&["update", "--yes"], &store, &src, None);
        assert_eq!(code, 0, "{err}");
        assert!(out.contains("Updated changelog name from 1.0.0 to 1.1.0."), "{out}");
        // Removing: the user's own squad that uses its agents is named; a skill the user changed is kept, and said.
        let config = base.join("config");
        let squad = crate::squadfile::get(&config, "plugin:changelog/changelog").unwrap();
        crate::squadfile::save(&config, &crate::squadfile::copy_of(&config, &squad, "Release notes", &[])).unwrap();
        std::fs::write(&skill, "my own words").unwrap();
        let (code, out, _) = run_asking(&["remove", "changelog"], &store, &src, None);
        assert_eq!(code, 0);
        assert!(out.starts_with("Removed changelog name (1 skill, 1 squad, 2 agents no longer offered). Tickets its squads already have keep working as they started."), "{out}");
        assert!(out.contains("Your squad Release notes uses its agents: those steps show the agent as missing"), "{out}");
        assert!(out.contains("The skill changelog-entry was changed after it was installed, so it is kept"), "{out}");
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), "my own words");
        // --yes from a folder too; check lists what it adds.
        let dir = crate::squadplugin::testing::plugin(&base.join("mine"), "notes-kit", "0.1.0");
        let (code, out, _) = run_asking(&["check", &dir.to_string_lossy()], &store, &src, None);
        assert!(code == 0 && out.contains("skills: notes-kit-entry (SKILL.md at most 64 KiB") && out.contains("agents: change-reader (change-reader), changelog-writer (changelog-writer)"), "{out}");
        let (code, _, err) = run_asking(&["add", &dir.to_string_lossy()], &store, &src, None);
        assert!(code == 1 && err.contains("platoon plugin add notes-kit --yes"), "{err}");
        let (code, out, err) = run_asking(&["add", &dir.to_string_lossy(), "--yes"], &store, &src, None);
        assert_eq!(code, 0, "{err}");
        assert!(out.contains("Installed notes-kit name 0.1.0"), "{out}");
    }

    #[test]
    fn list_add_update_remove_and_check_from_the_command_line() {
        let base = tmp("cli");
        let (repo, commit) = catalog_repo(&base, &[("sea-themes", "1.0.0", &[("Deep", GOOD_THEME)])]);
        let store = Store::new(base.join("data/plugins"));
        let src = source(&repo);
        let (code, out, _) = run(&["list"], &store, &src);
        assert_eq!(code, 0);
        assert!(out.contains(&format!("In the catalog (commit {}):", &commit[..7])), "{out}");
        assert!(out.contains("sea-themes  1.0.0    sea-themes name (1 theme)\n"), "{out}");
        assert!(out.contains("Installed:\n  none"), "{out}");
        let (code, out, err) = run(&["add", "sea-themes"], &store, &src);
        assert_eq!(code, 0, "{err}");
        assert!(out.contains("Installed sea-themes name 1.0.0 (1 theme)."), "{out}");
        let (_, out, _) = run(&["list"], &store, &src);
        assert!(out.contains("(1 theme)  [installed]") && out.contains(&format!("from the catalog (commit {})", &commit[..7])), "{out}");
        let (code, out, _) = run(&["add", "sea-themes"], &store, &src);
        assert!(code == 0 && out.contains("already installed"), "{out}");
        let (code, out, _) = run(&["update"], &store, &src);
        assert!(code == 0 && out.contains("sea-themes name is up to date."), "{out}");
        commit_catalog(&repo, &[("sea-themes", "1.1.0", &[("Deep", &GOOD_THEME.replace("#14181f", "#121212"))])]);
        let (_, out, _) = run(&["list"], &store, &src);
        assert!(out.contains("[installed 1.0.0, update available]"), "{out}");
        let (code, out, _) = run(&["update", "sea-themes"], &store, &src);
        assert!(code == 0 && out.contains("Updated sea-themes name from 1.0.0 to 1.1.0."), "{out}");
        let (code, _, err) = run(&["add", "nothing-here"], &store, &src);
        assert!(code == 1 && err.contains("no plugin called \"nothing-here\" in the catalog"), "{err}");
        let (code, out, _) = run(&["remove", "sea-themes"], &store, &src);
        assert!(code == 0 && out.starts_with("Removed sea-themes name"), "{out}");
        let (code, _, err) = run(&["remove", "sea-themes"], &store, &src);
        assert!(code == 1 && err.contains("No plugin called \"sea-themes\" is installed"));
        // From a folder, and checked.
        let dir = plugin(&base.join("mine"), "my-themes", "0.1.0", &[("Mine", GOOD_THEME)]);
        let (code, out, _) = run(&["check", &dir.to_string_lossy()], &store, &src);
        assert!(code == 0 && out.contains("adds: 1 theme: Mine") && out.contains("Looks good"), "{out}");
        let (code, out, err) = run(&["add", &dir.to_string_lossy()], &store, &src);
        assert_eq!(code, 0, "{err}");
        assert!(out.contains("Installed my-themes name 0.1.0"));
        let (_, out, _) = run(&["update"], &store, &src);
        assert!(out.contains("was installed from a folder"), "{out}");
        std::fs::write(dir.join("themes/Mine"), "background = #ffffff\nforeground = #000000\n").unwrap();
        let (code, out, _) = run(&["check", &dir.to_string_lossy()], &store, &src);
        assert!(code == 1 && out.contains("  - themes/Mine: it is a light theme") && out.contains("Not ready"), "{out}");
        // A catalog that can't be reached.
        let gone = Source { url: base.join("nowhere").to_string_lossy().into_owned(), rf: "main".into() };
        let (code, _, err) = run(&["add", "sea-themes"], &store, &gone);
        assert!(code == 1 && err.starts_with("platoon couldn't reach its plugin catalog:"), "{err}");
        assert_eq!(run(&["frobnicate"], &store, &src).0, 2);
        assert_eq!(run(&[], &store, &src).0, 2);
    }
}
