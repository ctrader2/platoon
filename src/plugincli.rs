//! `platoon plugin ...`: the catalog and the installed plugins from the command line. `list`, `add <id|folder>`,
//! `remove <id>`, `update [id]`, and `check <folder>` for someone making a plugin. The work is in `plugins.rs`; for
//! the code editor plugin also its downloads (`editorplugin.rs`): adding or updating it fetches what the editor folder
//! lacks, removing it takes the folder and the font off. `platoon setup --add/--remove editor` come here too.

use crate::editor_install::{self, Dirs, Printer};
use crate::editorplugin;
use crate::plugins::{self, Catalog, Origin, Outcome, Source, Store, Updated};
use std::{io::Write, path::Path};

/// What a `platoon plugin` command works on.
pub struct Ctx<'a> {
    pub store: &'a Store,
    pub source: &'a Source,
    /// The code editor's folders (its plugin's downloads go there).
    pub dirs: &'a Dirs,
    pub home: &'a str,
    /// Shows the editor's download progress.
    pub printer: &'a Printer,
}

pub const USAGE: &str = "usage: platoon plugin list | add <id or folder> | remove <id> | update [id] | check <folder>
  list             the plugins in platoon's catalog, and the ones installed
  add <id>         install a plugin from the catalog
  add <folder>     install a plugin from a folder on this computer (for making one; say ./name for a folder here)
  remove <id>      take a plugin off again
  update [id]      bring the installed plugins (or one) up to the catalog's version
  check <folder>   check a plugin folder before sending it: its plugin.json, its files, its themes";

/// Runs `platoon plugin <args>`; the exit code: 0 done, 1 it didn't work, 2 the command was wrong.
pub fn cli(args: &[String], c: &Ctx, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let arg = |i: usize| args.get(i).map(String::as_str);
    if !matches!(arg(0), Some("check") | None) {
        take_over_editor(c, out, err);
    }
    match (arg(0), arg(1), args.len()) {
        (Some("list"), None, 1) => list(c, out, err),
        (Some("add"), Some(what), 2) => add(c, what, out, err),
        (Some("remove"), Some(id), 2) => remove(c, id, out, err),
        (Some("update"), only, 1 | 2) => update(c, only, out, err),
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
    let next = if m.editor.is_some() { "" } else { " Choose a theme in platoon: s for settings, then Theme." };
    let _ = match o {
        Outcome::New(_) => writeln!(out, "Installed {} {} ({}).{next}", m.name, m.version, m.adds_words()),
        Outcome::Replaced { was, .. } if *was == m.version => writeln!(out, "Installed {} {} from the catalog, in place of the copy there was.", m.name, m.version),
        Outcome::Replaced { was, .. } => writeln!(out, "Updated {} from {was} to {}.", m.name, m.version),
        Outcome::Same(_) => writeln!(out, "{} {} is already installed, with the same files.", m.name, m.version),
    };
}

/// After a plugin is installed: the code editor's downloads, when it adds the editor. The exit code.
fn after_install(c: &Ctx, o: &Outcome, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    say_outcome(o, out);
    if o.plugin().manifest.editor.is_some() && !editor_downloads(c, out, err) {
        return 1;
    }
    0
}

fn add(c: &Ctx, what: &str, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (store, source) = (c.store, c.source);
    if is_folder(what) {
        let home = std::env::var("HOME").unwrap_or_default();
        let expanded = match what.strip_prefix("~/") {
            Some(rest) if !home.is_empty() => Path::new(&home).join(rest),
            _ => Path::new(what).to_path_buf(),
        };
        let Ok(dir) = expanded.canonicalize() else {
            let _ = writeln!(err, "There is no folder at {what}.");
            return 1;
        };
        return match store.install(&dir, Origin::Folder { path: dir.to_string_lossy().into_owned() }) {
            Ok(o) => after_install(c, &o, out, err),
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
    let origin = Origin::Catalog { source: catalog.source.clone(), rf: catalog.rf.clone(), commit: catalog.commit.clone() };
    match store.install(&entry.dir, origin) {
        Ok(o) => after_install(c, &o, out, err),
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
    match store.remove(id) {
        Ok(Some(p)) => {
            let _ = writeln!(out, "Removed {} ({} no longer offered). A theme of it that was in use gives way to the default.", p.manifest.name, p.manifest.adds_words());
            0
        }
        Ok(None) if id == editorplugin::ID => {
            let _ = writeln!(out, "The code editor plugin isn't installed; there is nothing to remove.");
            0
        }
        Ok(None) => {
            let _ = writeln!(err, "No plugin called \"{id}\" is installed. `platoon plugin list` shows what is.");
            1
        }
        Err(e) => {
            let _ = writeln!(err, "{e}");
            1
        }
    }
}

fn update(c: &Ctx, only: Option<&str>, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
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
    for u in plugins::update(store, &catalog, only) {
        let _ = match u {
            Updated::Done { name, from, to } if from == to => writeln!(out, "Updated {name} ({to}): its files changed."),
            Updated::Done { name, from, to } => writeln!(out, "Updated {name} from {from} to {to}."),
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

fn check(dir: &Path, out: &mut dyn Write) -> i32 {
    let r = plugins::check(dir);
    let _ = writeln!(out, "Checking {}", dir.display());
    if let Some(m) = &r.manifest {
        let license = m.license.as_deref().map_or(String::new(), |l| format!(", license {l}"));
        let _ = writeln!(out, "  plugin.json: {} ({}) {} by {}{license}, for platoon {} or newer", m.name, m.id, m.version, m.author, m.platoon);
        let _ = writeln!(out, "  adds: {}", if r.themes.is_empty() { m.adds_words() } else { format!("{} theme{}: {}", r.themes.len(), if r.themes.len() == 1 { "" } else { "s" }, r.themes.join(", ")) });
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

    fn run(args: &[&str], store: &Store, source: &Source) -> (i32, String, String) {
        // The editor's folders beside the store's (a test of the editor gives its own, with a mirror).
        let base = store.dir.parent().unwrap().parent().unwrap();
        run_with(args, store, source, &crate::editor_install::testing::dirs(base, vec![]))
    }

    fn run_with(args: &[&str], store: &Store, source: &Source, dirs: &Dirs) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let printer = Printer::quiet();
        let c = Ctx { store, source, dirs, home: "/home/u", printer: &printer };
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
        let c = Ctx { store: &store, source: &src, dirs: &dirs, home: "/home/u", printer: &printer };
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
