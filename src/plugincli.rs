//! `platoon plugin ...`: the catalog and the installed plugins from the command line. `list`, `add <id|folder>`,
//! `remove <id>`, `update [id]`, and `check <folder>` for someone making a plugin. The work is in `plugins.rs`.

use crate::plugins::{self, Catalog, Origin, Outcome, Source, Store, Updated};
use std::{io::Write, path::Path};

pub const USAGE: &str = "usage: platoon plugin list | add <id or folder> | remove <id> | update [id] | check <folder>
  list             the plugins in platoon's catalog, and the ones installed
  add <id>         install a plugin from the catalog
  add <folder>     install a plugin from a folder on this computer (for making one; say ./name for a folder here)
  remove <id>      take a plugin off again
  update [id]      bring the installed plugins (or one) up to the catalog's version
  check <folder>   check a plugin folder before sending it: its plugin.json, its files, its themes";

/// Runs `platoon plugin <args>`; the exit code: 0 done, 1 it didn't work, 2 the command was wrong.
pub fn cli(args: &[String], store: &Store, source: &Source, home: &str, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let arg = |i: usize| args.get(i).map(String::as_str);
    match (arg(0), arg(1), args.len()) {
        (Some("list"), None, 1) => list(store, source, home, out, err),
        (Some("add"), Some(what), 2) => add(store, source, what, out, err),
        (Some("remove"), Some(id), 2) => remove(store, id, out, err),
        (Some("update"), only, 1 | 2) => update(store, source, only, out, err),
        (Some("check"), Some(dir), 2) => check(Path::new(dir), out),
        _ => {
            let _ = writeln!(err, "{USAGE}");
            2
        }
    }
}

fn fetch(store: &Store, source: &Source, out: &mut dyn Write) -> Result<Catalog, String> {
    let _ = writeln!(out, "Fetching the plugin catalog…");
    let _ = out.flush();
    plugins::fetch(store, source)
}

fn list(store: &Store, source: &Source, home: &str, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
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
            let _ = writeln!(out, "  {:<w$}  {:<8} {} ({}){state}", m.id, m.version, m.name, m.adds_words());
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
    }
    code
}

/// A folder rather than an id: it has a slash, starts with a dot or a tilde, or is a folder here and not an id.
fn is_folder(what: &str) -> bool {
    what.contains('/') || what.starts_with('.') || what.starts_with('~') || !plugins::valid_id(what)
}

fn say_outcome(o: &Outcome, out: &mut dyn Write) {
    let m = &o.plugin().manifest;
    let _ = match o {
        Outcome::New(_) => writeln!(out, "Installed {} {} ({}). Choose a theme in platoon: s for settings, then Theme.", m.name, m.version, m.adds_words()),
        Outcome::Replaced { was, .. } => writeln!(out, "Updated {} from {was} to {}.", m.name, m.version),
        Outcome::Same(_) => writeln!(out, "{} {} is already installed, with the same files.", m.name, m.version),
    };
}

fn add(store: &Store, source: &Source, what: &str, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
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
            Ok(o) => {
                say_outcome(&o, out);
                0
            }
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
        Ok(o) => {
            say_outcome(&o, out);
            0
        }
        Err(e) => {
            let _ = writeln!(err, "{e}");
            1
        }
    }
}

fn remove(store: &Store, id: &str, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match store.remove(id) {
        Ok(Some(p)) => {
            let _ = writeln!(out, "Removed {} ({} no longer offered). A theme of it that was in use gives way to the default.", p.manifest.name, p.manifest.adds_words());
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

fn update(store: &Store, source: &Source, only: Option<&str>, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
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
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = cli(&args, store, source, "/home/u", &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
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
