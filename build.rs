//! Builds platoon's plugin catalog into the binary: every file of each plugin `plugins/index.json` lists (and the index
//! itself), nothing else of the repository. `plugins.rs` includes the list this writes (`$OUT_DIR/catalog.rs`) and
//! unpacks it when a plugin is shown or installed, so installing a plugin needs no network.
//!
//! Hidden files and folders are not part of a plugin and are left out (as `plugins::check` leaves them out). A link is
//! refused: a plugin holds plain files only. The whole `plugins/` folder is watched, so adding, changing or removing a
//! file there builds the catalog again.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=plugins");
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let catalog = root.join("plugins");
    let index_path = catalog.join("index.json");
    let text = fs::read_to_string(&index_path).unwrap_or_else(|e| panic!("{} can't be read: {e}", index_path.display()));
    let index: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("plugins/index.json isn't valid JSON: {e}"));
    let ids: Vec<String> = index
        .get("plugins")
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(|p| p.get("id").and_then(serde_json::Value::as_str)).map(String::from).collect())
        .unwrap_or_default();
    let mut files: Vec<(String, PathBuf)> = vec![("index.json".into(), index_path)];
    for id in ids {
        // Only a plain id names a folder here (platoon checks it again when it reads the catalog).
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            continue;
        }
        let dir = catalog.join(&id);
        if dir.is_dir() {
            walk(&catalog, &dir, &mut files);
        }
    }
    let mut out = String::from("/// Every file of the plugin catalog built into platoon: (its path in `plugins/`, its bytes). Written by build.rs.\npub static FILES: &[(&str, &[u8])] = &[\n");
    for (rel, path) in &files {
        let abs = path.to_str().unwrap_or_else(|| panic!("{} isn't a UTF-8 path", path.display()));
        out.push_str(&format!("    ({rel:?}, include_bytes!({abs:?})),\n"));
    }
    out.push_str("];\n");
    let dest = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("catalog.rs");
    fs::write(&dest, out).unwrap_or_else(|e| panic!("couldn't write {}: {e}", dest.display()));
}

/// Every plain file under `dir`, as (path inside the catalog with `/`, where it is), in name order.
fn walk(catalog: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap_or_else(|e| panic!("{} can't be read: {e}", dir.display())).flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        let name = name.to_str().unwrap_or_else(|| panic!("{} has a name that isn't UTF-8", e.path().display()));
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        let meta = fs::symlink_metadata(&path).unwrap_or_else(|err| panic!("{} can't be read: {err}", path.display()));
        if meta.file_type().is_symlink() {
            panic!("{} is a link; a plugin holds plain files only", path.display());
        } else if meta.is_dir() {
            walk(catalog, &path, out);
        } else if meta.is_file() {
            let rel = path.strip_prefix(catalog).expect("inside the catalog").components().map(|c| c.as_os_str().to_str().expect("UTF-8")).collect::<Vec<_>>().join("/");
            out.push((rel, path));
        }
    }
}
