//! `scripts/check-versions.sh`, the CI check that README.md and the guide name the Claude Code and Codex versions in
//! `checked-with/`: run on a copy of the docs in a temp folder, as they are (it passes) and changed (it fails, naming
//! the page). Nothing outside the temp folder is written.

mod common;
use common::*;
use std::{fs, path::Path, process::Command};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// A copy of what the script reads: itself, `checked-with/`, README.md and the guide.
fn copy_docs(name: &str) -> std::path::PathBuf {
    let d = tmp(name);
    for dir in ["scripts", "checked-with", "guide"] {
        fs::create_dir_all(d.join(dir)).unwrap();
    }
    fs::copy(Path::new(ROOT).join("scripts/check-versions.sh"), d.join("scripts/check-versions.sh")).unwrap();
    fs::copy(Path::new(ROOT).join("README.md"), d.join("README.md")).unwrap();
    for dir in ["checked-with", "guide"] {
        for f in fs::read_dir(Path::new(ROOT).join(dir)).unwrap().flatten() {
            fs::copy(f.path(), d.join(dir).join(f.file_name())).unwrap();
        }
    }
    d
}

fn check(d: &Path) -> (bool, String) {
    let out = Command::new("sh").arg(d.join("scripts/check-versions.sh")).output().unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn version(d: &Path, tool: &str) -> String {
    fs::read_to_string(d.join("checked-with").join(tool)).unwrap().trim().to_string()
}

#[test]
fn the_docs_name_the_versions_in_checked_with() {
    let d = copy_docs("versions-as-is");
    let (ok, out) = check(&d);
    assert!(ok, "{out}");
    assert!(out.contains(&format!("last checked with Codex {}", version(&d, "codex"))), "{out}");
    assert!(out.contains(&format!("last checked with Claude Code {}", version(&d, "claude-code"))), "{out}");
}

#[test]
fn a_page_that_names_another_codex_version_fails_the_check() {
    let d = copy_docs("versions-codex-differs");
    let codex = version(&d, "codex");
    let page = d.join("guide/troubleshooting.md");
    let text = fs::read_to_string(&page).unwrap();
    let said = format!("Codex {codex}");
    assert!(text.contains(&said), "troubleshooting.md names Codex's version");
    fs::write(&page, text.replace(&said, "Codex 0.1.0")).unwrap();
    let (ok, out) = check(&d);
    assert!(!ok, "{out}");
    assert!(
        out.contains(&format!("guide/troubleshooting.md says last checked with Codex 0.1.0, but checked-with/codex says {codex}")),
        "{out}"
    );
    // Claude Code's line is still right, and said so.
    assert!(out.contains("ok: README.md and the guide say last checked with Claude Code"), "{out}");
}

#[test]
fn a_new_version_in_checked_with_fails_until_the_docs_say_it_too() {
    let d = copy_docs("versions-file-moved");
    fs::write(d.join("checked-with/codex"), "9.9.9\n").unwrap();
    let (ok, out) = check(&d);
    assert!(!ok, "{out}");
    assert!(out.contains("README.md says last checked with Codex"), "{out}");
    // And a README that no longer says it at all.
    let d = copy_docs("versions-line-gone");
    let readme = d.join("README.md");
    let text = fs::read_to_string(&readme).unwrap().replace(&format!(" and Codex {}", version(&d, "codex")), "");
    fs::write(&readme, text).unwrap();
    let (ok, out) = check(&d);
    assert!(!ok, "{out}");
    assert!(out.contains("error: README.md no longer says it was last checked with Codex"), "{out}");
}
