//! platoon's code editor (Neovim) as a plugin: the `editor` kind of `plugin.json`, and what the core does with it.
//!
//! The split: the CORE has the code (the install engine, `editor_install.rs`; handing the terminal over, the folders,
//! the colors and the session file, `editor.rs`; `v`, `app/editor_glue.rs`). The PLUGIN (`plugins/editor` in the
//! catalog) has the data: its download table (`downloads.json`: which Neovim, tools, Neovim plugins, parsers and font,
//! each with an https address, sha256 and size) and the editor's settings files (`config/init.lua`, ...), which platoon
//! copies into the editor's config folder each time it opens the editor.
//!
//! ```json
//! "editor": { "downloads": "downloads.json", "config": ["config/init.lua", "config/lua/platoon/tree.lua", "config/shell.sh"] }
//! ```
//!
//! `config` files are under `config/` and land at the same path under `<editor>/config/` (so `config/init.lua` is the
//! file nvim starts with, `config/lua/platoon/tree.lua` is `require('platoon.tree')`, `config/shell.sh` is the
//! editor's 'shell'). They are text; the table must pass `editor_install::table_problems`.
//!
//! Installing the plugin puts its files in `<data>/plugins/editor` (as any plugin); its downloads are a second step,
//! run by whoever installed it (the settings' Plugins tab, `platoon plugin add editor`, the `v` offer): the engine fetches only
//! what `<data>/editor` doesn't already have with the same checksum. Removing it removes `<data>/editor` and the font.
//!
//! An editor installed before it was a plugin (`<data>/editor` complete, no editor plugin) is taken over quietly: the
//! copy of the plugin compiled into this platoon (`BUNDLED`, the same files as `plugins/editor`) is installed as the
//! editor plugin, which needs no network, and its table's checksums match what is there, so nothing is downloaded.
//! With no editor plugin at all, the editor's settings files are that copy too, so `v` never breaks.

use crate::editor_install::{self, Dirs, Manifest as Table, Platform, Progress, Removed, Report};
use crate::plugins::{self, Installed, Origin, Store};
use serde_json::Value;
use std::{fs, path::PathBuf};

/// The kind's key in `plugin.json`.
pub const KIND: &str = "editor";
/// The catalog's editor plugin.
pub const ID: &str = "editor";
/// The download table at most.
const MAX_TABLE_FILE: u64 = 256 * 1024;
/// Where the settings files are in the plugin, and under the editor folder.
const CONFIG: &str = "config/";
/// The file nvim is started with.
pub const INIT: &str = "config/init.lua";

/// The editor plugin as it was when this platoon was built: the catalog's `plugins/editor`, file for file (a test keeps
/// the list whole).
pub const BUNDLED: &[(&str, &str)] = &[
    ("plugin.json", include_str!("../plugins/editor/plugin.json")),
    ("downloads.json", include_str!("../plugins/editor/downloads.json")),
    ("config/init.lua", include_str!("../plugins/editor/config/init.lua")),
    ("config/lua/platoon/tree.lua", include_str!("../plugins/editor/config/lua/platoon/tree.lua")),
    ("config/shell.sh", include_str!("../plugins/editor/config/shell.sh")),
];

/// What `"editor"` in `plugin.json` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The download table, a path in the plugin.
    pub downloads: String,
    /// The settings files, paths in the plugin under `config/`.
    pub config: Vec<String>,
}

impl Spec {
    pub fn files(&self) -> Vec<String> {
        std::iter::once(self.downloads.clone()).chain(self.config.iter().cloned()).collect()
    }
}

pub fn parse_spec(v: &Value) -> Result<Spec, String> {
    let obj = v.as_object().ok_or("\"editor\" must be an object with \"downloads\" and \"config\"")?;
    let downloads = obj.get("downloads").and_then(Value::as_str).ok_or("\"editor\" needs \"downloads\": the file with its download table")?.to_string();
    let config = match obj.get("config") {
        Some(Value::Array(a)) => a.iter().map(|x| x.as_str().map(String::from).ok_or("\"editor\"'s \"config\" must be a list of file names")).collect::<Result<Vec<_>, _>>()?,
        _ => return Err("\"editor\" needs \"config\": the list of the editor's settings files".into()),
    };
    Ok(Spec { downloads, config })
}

/// The computer whose download size is shown: this one, or Linux x86_64 where there are no downloads for this one.
fn here() -> Platform {
    Platform::current().unwrap_or(Platform::LinuxX64)
}

/// What `plugins::check` adds for the editor kind: its problems, and what it downloads here. `files` are the plugin's
/// (path inside it, where it is, size); `safe` says whether a name from `plugin.json` can only mean a file inside it.
pub fn check_spec(spec: &Spec, files: &[(String, PathBuf, u64)], safe: fn(&str) -> bool) -> (Vec<String>, Option<u64>) {
    let mut out = Vec::new();
    let find = |rel: &str| files.iter().find(|(r, ..)| r == rel);
    let mut bytes = None;
    if !safe(&spec.downloads) {
        out.push(format!("\"{}\" isn't a file name inside the plugin (no leading /, no .., no hidden parts)", spec.downloads));
    } else {
        match find(&spec.downloads) {
            None => out.push(format!("{} is listed in plugin.json but isn't in the folder", spec.downloads)),
            Some((_, _, len)) if *len > MAX_TABLE_FILE => out.push(format!("{} is bigger than {} KiB", spec.downloads, MAX_TABLE_FILE / 1024)),
            Some((_, path, _)) => match fs::read_to_string(path).map_err(|_| "it isn't text".to_string()).and_then(|t| editor_install::parse_table(&t)) {
                Err(e) => out.push(format!("{}: {e}", spec.downloads)),
                Ok(t) => {
                    out.extend(editor_install::table_problems(&t).into_iter().map(|p| format!("{}: {p}", spec.downloads)));
                    bytes = Some(editor_install::download_bytes(&t, here()));
                }
            },
        }
    }
    if !spec.config.iter().any(|c| c == INIT) {
        out.push(format!("\"editor\"'s \"config\" must have {INIT}: the file the editor starts with"));
    }
    for c in &spec.config {
        if !safe(c) || !c.starts_with(CONFIG) {
            out.push(format!("\"{c}\" must be a file inside the plugin's config/ folder"));
            continue;
        }
        let Some((_, path, _)) = find(c) else {
            out.push(format!("{c} is listed in plugin.json but isn't in the folder"));
            continue;
        };
        let text = fs::read(path).ok().and_then(|b| String::from_utf8(b).ok());
        if text.is_none_or(|t| t.contains('\0')) {
            out.push(format!("{c} isn't plain text: the editor's settings are text files"));
        }
    }
    (out, bytes)
}

/// "about 58 MB".
pub fn size_words(bytes: u64) -> String {
    format!("about {} MB", (bytes + 500_000) / 1_000_000)
}

/// The table of the copy compiled into this platoon.
pub fn bundled_table() -> Table {
    editor_install::parse_table(bundled("downloads.json")).expect("the bundled editor plugin's table reads (a test checks)")
}

fn bundled(path: &str) -> &'static str {
    BUNDLED.iter().find(|(p, _)| *p == path).map_or("", |(_, t)| t)
}

/// The installed plugin that adds the code editor (at most one can be installed).
pub fn installed(store: &Store) -> Option<Installed> {
    store.installed().into_iter().find(|p| p.manifest.editor.is_some())
}

/// An installed editor plugin's download table, checked.
pub fn table_of(store: &Store, p: &Installed) -> Result<Table, String> {
    let spec = p.manifest.editor.as_ref().ok_or("it doesn't add the code editor")?;
    let path = store.plugin_dir(p.id()).join(&spec.downloads);
    let t = fs::read_to_string(&path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    let t = editor_install::parse_table(&t)?;
    let problems = editor_install::table_problems(&t);
    if problems.is_empty() { Ok(t) } else { Err(problems.join("; ")) }
}

/// The table the editor is installed from: the installed editor plugin's.
pub fn table(store: &Store) -> Option<Table> {
    installed(store).and_then(|p| table_of(store, &p).ok())
}

/// What installing (the rest of) the editor downloads here: the installed plugin's table, else the one that came
/// with platoon (the catalog's, when it was built).
pub fn download_bytes(store: &Store) -> u64 {
    editor_install::download_bytes(&table(store).unwrap_or_else(bundled_table), here())
}

/// The editor's settings files, as (path under `config/`, text): the installed editor plugin's, else the copy that
/// came with platoon.
pub fn config_files(store: &Store) -> Vec<(String, String)> {
    if let Some(p) = installed(store) {
        let dir = store.plugin_dir(p.id());
        let spec = p.manifest.editor.clone().unwrap_or(Spec { downloads: String::new(), config: Vec::new() });
        let files: Option<Vec<(String, String)>> = spec
            .config
            .iter()
            .filter_map(|c| c.strip_prefix(CONFIG).map(|rel| (c, rel)))
            .map(|(c, rel)| fs::read_to_string(dir.join(c)).ok().map(|t| (rel.to_string(), t)))
            .collect();
        if let Some(f) = files.filter(|f| !f.is_empty()) {
            return f;
        }
    }
    BUNDLED.iter().filter_map(|(p, t)| p.strip_prefix(CONFIG).map(|rel| (rel.to_string(), t.to_string()))).collect()
}

/// Enough of the editor is installed to open it (`editor::ready`), by the installed plugin's table, or (no plugin
/// yet) by the one that came with platoon.
pub fn ready(store: &Store, dirs: &Dirs) -> bool {
    let t = table(store).unwrap_or_else(bundled_table);
    crate::editor::ready(&editor_install::status(dirs, &t))
}

/// Everything of the installed plugin's table is there and current (false without a plugin).
pub fn complete(store: &Store, dirs: &Dirs) -> bool {
    table(store).is_some_and(|t| editor_install::status(dirs, &t).complete())
}

/// An editor installed before it was a plugin: its folder has a record and can be opened, and no editor plugin is
/// installed.
pub fn from_before(store: &Store, dirs: &Dirs) -> bool {
    installed(store).is_none() && dirs.editor.join("installed.json").is_file() && crate::editor::ready(&editor_install::status(dirs, &bundled_table()))
}

/// Take over an editor installed before it was a plugin: the copy of the plugin that came with platoon is installed
/// as the editor plugin (no network), and nothing is downloaded (the folder is left as it is). `None` when there is
/// nothing to take over.
pub fn adopt(store: &Store, dirs: &Dirs) -> Option<Result<Installed, String>> {
    if !from_before(store, dirs) {
        return None;
    }
    let stage = store.dir.join(format!(".bundled-{}", std::process::id()));
    let _ = fs::remove_dir_all(&stage);
    let written = (|| -> std::io::Result<()> {
        for (rel, text) in BUNDLED {
            let p = stage.join(rel);
            fs::create_dir_all(p.parent().unwrap_or(&stage))?;
            fs::write(p, text)?;
        }
        Ok(())
    })();
    let done = match written {
        Ok(()) => store.install(&stage, Origin::Bundled { version: plugins::platoon_version().to_string() }).map(|o| o.plugin().clone()),
        Err(e) => Err(format!("couldn't write the editor plugin's files: {e}")),
    };
    let _ = fs::remove_dir_all(&stage);
    Some(done)
}

/// The one line said when an editor from before was taken over.
pub fn adopted_words(p: &Installed) -> String {
    format!("Your code editor is now the plugin \"{}\" (nothing was downloaded; the settings show it: s, then Plugins).", p.manifest.name)
}

/// Fetch what the installed editor plugin's table has that the editor folder doesn't (the engine's own idempotence:
/// whatever is there with the same checksum stays).
pub fn install_downloads(store: &Store, dirs: &Dirs, progress: &(dyn Fn(Progress) + Sync)) -> Result<Report, String> {
    let p = installed(store).ok_or("the code editor plugin isn't installed")?;
    let t = table_of(store, &p)?;
    editor_install::install(dirs, &t, progress)
}

/// Take the editor plugin off, with everything it downloaded: the editor folder and the font platoon installed (never
/// the user's own programs, fonts or Neovim settings). The folder goes first, so a removal that stops half way leaves
/// the plugin there to remove again rather than a folder with no plugin.
pub fn remove(store: &Store, dirs: &Dirs, id: &str) -> Result<(Option<Installed>, Removed), String> {
    let removed = editor_install::remove(dirs)?;
    let was = store.remove(id)?;
    Ok((was, removed))
}

/// Whether plugin `id` (installed) adds the code editor.
pub fn is_editor(store: &Store, id: &str) -> bool {
    store.get(id).is_some_and(|p| p.manifest.editor.is_some())
}

#[cfg(test)]
pub mod testing {
    //! An editor plugin whose table is a mirror of tiny fake downloads (`editor_install::testing::fake`), as a folder
    //! and in a catalog repository: for tests here, in `plugincli` and in the app.
    use super::*;
    use crate::editor_install::testing as fake;
    use serde_json::json;
    use std::path::Path;

    /// `<base>/<id>`: the bundled plugin's settings files, with a table for the fake downloads in `mirror`.
    pub fn plugin(base: &Path, id: &str, version: &str, mirror: &Path) -> (PathBuf, Table) {
        let table = fake::fake(mirror);
        let dir = base.join(id);
        for (rel, text) in BUNDLED {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, text).unwrap();
        }
        fs::write(dir.join("downloads.json"), serde_json::to_string_pretty(&editor_install::table_json(&table)).unwrap()).unwrap();
        let mut m: Value = serde_json::from_str(bundled("plugin.json")).unwrap();
        if let Some(o) = m.as_object_mut() {
            o.insert("id".into(), json!(id));
            o.insert("version".into(), json!(version));
        }
        fs::write(dir.join("plugin.json"), serde_json::to_string_pretty(&m).unwrap()).unwrap();
        (dir, table)
    }

    /// An editor folder as a pre-plugin platoon left it after installing from its own pins (which are the bundled
    /// table): every part recorded with the real checksums and there on disk (the programs are stand-ins).
    pub fn folder_from_before(d: &Dirs) {
        let real = bundled_table();
        let mut items = serde_json::Map::new();
        for t in &real.tools {
            let sha = t.assets.iter().find(|(p, _)| *p == Platform::LinuxX64).unwrap().1.sha256.clone();
            items.insert(format!("tool:{}", t.id), serde_json::json!({"source": "download", "version": t.version, "sha256": sha}));
            let exe = d.editor.join("tools").join(&t.id).join(&t.exe);
            fake::write_exe(&exe, "echo old");
            fs::create_dir_all(d.editor.join("bin")).unwrap();
            std::os::unix::fs::symlink(&exe, d.editor.join("bin").join(&t.id)).unwrap();
        }
        for p in &real.plugins {
            items.insert(format!("plugin:{}", p.name), serde_json::json!({"rev": p.rev, "sha256": p.sha256}));
            fs::create_dir_all(d.site().join("pack/platoon/start").join(&p.name)).unwrap();
        }
        for p in &real.parsers {
            items.insert(format!("parser:{}", p.lang), serde_json::json!({"rev": p.rev, "sha256": p.sha256}));
            fs::create_dir_all(d.site().join("parser")).unwrap();
            fs::write(d.site().join("parser").join(format!("{}.so", p.lang)), "built").unwrap();
        }
        fs::create_dir_all(&d.fonts).unwrap();
        for f in &real.font.files {
            fs::write(d.fonts.join(f), "font").unwrap();
        }
        items.insert("font".into(), serde_json::json!({"version": real.font.version, "sha256": real.font.download.sha256, "dir": d.fonts, "files": real.font.files}));
        fs::write(d.editor.join("installed.json"), serde_json::json!({"version": 1, "items": items}).to_string()).unwrap();
        assert!(editor_install::status(d, &real).complete());
    }

    /// A catalog repository with the fake editor plugin (id `editor`) and a theme plugin.
    pub fn catalog(base: &Path, mirror: &Path) -> (PathBuf, Table) {
        let (repo, _) = plugins::testing::catalog_repo(base, &[("sea-themes", "1.0.0", &[("Deep", plugins::testing::GOOD_THEME)])]);
        let cat = repo.join(plugins::CATALOG_FOLDER);
        let (_, table) = plugin(&cat, ID, "1.0.0", mirror);
        let index = json!({"plugins": [{"id": ID, "version": "1.0.0"}, {"id": "sea-themes", "version": "1.0.0"}]});
        fs::write(cat.join(plugins::INDEX), index.to_string()).unwrap();
        plugins::testing::git(&repo, &["add", "-A"]);
        plugins::testing::git(&repo, &["commit", "--quiet", "-m", "editor"]);
        (repo, table)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::editor_install::testing as fake;
    use crate::plugins::Outcome;
    use std::path::Path;
    use crate::plugins::testing::tmp;

    fn store(base: &Path) -> Store {
        Store::new(base.join("data/plugins"))
    }

    #[test]
    fn the_bundled_copy_is_the_catalogs_editor_plugin_file_for_file() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins").join(ID);
        let mut on_disk: Vec<String> = Vec::new();
        fn walk(root: &Path, d: &Path, out: &mut Vec<String>) {
            for e in fs::read_dir(d).unwrap().flatten() {
                if e.path().is_dir() {
                    walk(root, &e.path(), out);
                } else {
                    out.push(e.path().strip_prefix(root).unwrap().to_string_lossy().into_owned());
                }
            }
        }
        walk(&dir, &dir, &mut on_disk);
        on_disk.sort();
        let mut listed: Vec<String> = BUNDLED.iter().map(|(p, _)| p.to_string()).collect();
        listed.sort();
        assert_eq!(on_disk, listed, "BUNDLED must list every file of plugins/editor");
        let r = plugins::check(&dir);
        assert!(r.ok() && r.notes.is_empty(), "{:?} {:?}", r.problems, r.notes);
        let m = r.manifest.unwrap();
        assert_eq!(m.adds_words(), "the code editor");
        assert_eq!(m.editor.as_ref().unwrap().config, ["config/init.lua", "config/lua/platoon/tree.lua", "config/shell.sh"]);
        assert!(r.download_bytes.is_some_and(|b| b > 10_000_000));
        // Its settings files reach the editor at their paths under config/.
        let names: Vec<String> = config_files(&Store::new(tmp("bundled-none").join("p"))).into_iter().map(|(p, _)| p).collect();
        assert_eq!(names, ["init.lua", "lua/platoon/tree.lua", "shell.sh"]);
    }

    #[test]
    fn the_editor_kind_is_checked_plainly() {
        let base = tmp("kind");
        let (dir, _) = plugin(&base.join("p"), "my-editor", "1.0.0", &base.join("mirror"));
        let r = plugins::check(&dir);
        assert!(r.ok(), "{:?}", r.problems);
        let problems = |f: &dyn Fn(&Path)| {
            let (d, _) = plugin(&base.join("q"), "my-editor", "1.0.0", &base.join("mirror"));
            f(&d);
            let p = plugins::check(&d).problems.join("\n");
            fs::remove_dir_all(&d).unwrap();
            p
        };
        let edit_table = |f: fn(&mut Value)| {
            move |d: &Path| {
                let mut v: Value = serde_json::from_str(&fs::read_to_string(d.join("downloads.json")).unwrap()).unwrap();
                f(&mut v);
                fs::write(d.join("downloads.json"), v.to_string()).unwrap();
            }
        };
        let p = problems(&edit_table(|v| {
            let e = v.get_mut("plugins").and_then(|p| p.get_mut(0)).and_then(Value::as_object_mut).unwrap();
            e.insert("url".into(), "http://example.invalid/a".into());
        }));
        assert!(p.contains("downloads.json: plugin alpha.nvim: its address must be an https:// link"), "{p}");
        let p = problems(&edit_table(|v| {
            let e = v.get_mut("font").and_then(Value::as_object_mut).unwrap();
            e.remove("sha256");
        }));
        assert!(p.contains("downloads.json: the font has no \"sha256\""), "{p}");
        let p = problems(&edit_table(|v| {
            for e in v.get_mut("parsers").and_then(Value::as_array_mut).unwrap() {
                e.as_object_mut().unwrap().insert("size".into(), 290_000_000u64.into());
            }
            for (i, e) in v.get_mut("plugins").and_then(Value::as_array_mut).unwrap().iter_mut().enumerate() {
                let o = e.as_object_mut().unwrap();
                o.insert("size".into(), 290_000_000u64.into());
                o.insert("sha256".into(), format!("{:064x}", i + 7).into());
            }
        }));
        assert!(p.contains("more than the 600 MB a plugin may"), "{p}");
        let p = problems(&|d: &Path| {
            let mut m: Value = serde_json::from_str(&fs::read_to_string(d.join("plugin.json")).unwrap()).unwrap();
            let e = m.get_mut("editor").and_then(Value::as_object_mut).unwrap();
            e.insert("config".into(), serde_json::json!(["config/init.lua", "../outside.lua", "notes.lua"]));
            fs::write(d.join("plugin.json"), m.to_string()).unwrap();
        });
        assert!(p.contains("\"../outside.lua\" must be a file inside the plugin's config/ folder") && p.contains("\"notes.lua\" must be a file inside"), "{p}");
        let p = problems(&|d: &Path| fs::write(d.join("config/init.lua"), [0u8, 159, 146, 150]).unwrap());
        assert!(p.contains("config/init.lua isn't plain text"), "{p}");
        let p = problems(&|d: &Path| fs::remove_file(d.join("config/shell.sh")).unwrap());
        assert!(p.contains("config/shell.sh is listed in plugin.json but isn't in the folder"), "{p}");
        let p = problems(&|d: &Path| {
            let mut m: Value = serde_json::from_str(&fs::read_to_string(d.join("plugin.json")).unwrap()).unwrap();
            m.as_object_mut().unwrap().insert("editor".into(), serde_json::json!({"downloads": "downloads.json"}));
            fs::write(d.join("plugin.json"), m.to_string()).unwrap();
        });
        assert!(p.contains("\"editor\" needs \"config\""), "{p}");
    }

    #[test]
    fn installing_the_plugin_drives_the_engine_from_its_table_and_removing_it_takes_everything_off() {
        let base = tmp("drive");
        let s = store(&base);
        let d = fake::dirs(&base, vec![fake::fake_cc(&base)]);
        let (dir, want) = plugin(&base.join("src"), ID, "1.0.0", d.mirror.as_ref().unwrap());
        assert!(!ready(&s, &d) && config_files(&s).len() == 3);
        s.install(&dir, Origin::Folder { path: dir.to_string_lossy().into_owned() }).unwrap();
        assert_eq!(table(&s).unwrap(), want);
        let r = install_downloads(&s, &d, &fake::quiet).unwrap();
        assert!(r.fetched > 0 && r.font && r.font_family == "Fake Nerd Font");
        assert!(ready(&s, &d) && complete(&s, &d));
        assert_eq!(install_downloads(&s, &d, &fake::quiet).unwrap().fetched, 0, "a second run fetches nothing");
        // Its settings files are the plugin's, changed or not.
        fs::write(s.plugin_dir(ID).join("config/init.lua"), "-- changed").unwrap();
        assert!(config_files(&s).iter().any(|(p, t)| p == "init.lua" && t == "-- changed"));
        // A second editor plugin isn't taken while one is installed.
        let (other, _) = plugin(&base.join("src2"), "other-editor", "1.0.0", &base.join("mirror2"));
        let err = s.install(&other, Origin::Folder { path: String::new() }).unwrap_err();
        assert!(err.contains("platoon has one code editor"), "{err}");
        let (was, removed) = remove(&s, &d, ID).unwrap();
        assert!(was.is_some() && removed.editor && removed.font_files > 0);
        assert!(!d.editor.exists() && !d.fonts.exists() && s.get(ID).is_none());
        assert!(adopt(&s, &d).is_none(), "nothing left to take over");
    }

    #[test]
    fn an_editor_from_before_plugins_is_taken_over_and_nothing_is_downloaded() {
        let base = tmp("adopt");
        let s = store(&base);
        // An empty mirror: any download at all would fail (and the network is never used).
        let d = fake::dirs(&base, vec![fake::fake_cc(&base)]);
        fs::create_dir_all(d.mirror.as_ref().unwrap()).unwrap();
        folder_from_before(&d);
        let before = fs::read(d.editor.join("installed.json")).unwrap();
        assert!(from_before(&s, &d) && ready(&s, &d), "it opens before anything is done (the bundled settings)");
        let p = adopt(&s, &d).unwrap().unwrap();
        assert_eq!(p.id(), ID);
        assert_eq!(p.origin, Origin::Bundled { version: plugins::platoon_version().to_string() });
        assert!(adopted_words(&p).contains("nothing was downloaded"));
        assert_eq!(fs::read(d.editor.join("installed.json")).unwrap(), before, "the editor folder is left as it is");
        assert!(adopt(&s, &d).is_none(), "once");
        assert!(complete(&s, &d) && ready(&s, &d));
        // Installing the plugin's downloads now reuses everything: nothing fetched, the empty mirror never asked.
        let r = install_downloads(&s, &d, &fake::quiet).unwrap();
        assert_eq!(r.fetched, 0);
        // The same from the catalog's copy (what `platoon plugin add editor` installs over it): same table, nothing.
        let (repo, _) = plugins::testing::catalog_repo(&base.join("cat"), &[]);
        let cat_dir = repo.join(plugins::CATALOG_FOLDER).join(ID);
        for (rel, text) in BUNDLED {
            fs::create_dir_all(cat_dir.join(rel).parent().unwrap()).unwrap();
            fs::write(cat_dir.join(rel), text).unwrap();
        }
        let o = s.install(&cat_dir, Origin::Catalog { source: "x".into(), rf: "main".into(), commit: "c".into() }).unwrap();
        assert!(matches!(o, Outcome::Replaced { .. }));
        assert_eq!(install_downloads(&s, &d, &fake::quiet).unwrap().fetched, 0);
        // Removed: the folder and the font go, and there is nothing to take over any more.
        remove(&s, &d, ID).unwrap();
        assert!(!d.editor.exists() && installed(&s).is_none() && adopt(&s, &d).is_none());
    }
}
