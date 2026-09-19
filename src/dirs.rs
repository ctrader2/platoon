//! Candidate directories for "new session": recent Claude projects, zoxide's frecency list,
//! and git repos found under common project roots.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const ROOTS: [&str; 7] = ["repos", "src", "code", "projects", "dev", "work", "git"];
const SKIP: [&str; 5] = ["node_modules", "target", "vendor", "snap", "Library"];
const MAX_DEPTH: usize = 3;
const MAX_TOTAL: usize = 5000;

pub fn tilde(path: &str, home: &str) -> String {
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => path.to_string(),
    }
}

pub fn expand(input: &str, home: &str) -> Option<String> {
    if input == "~" {
        Some(home.to_string())
    } else if let Some(rest) = input.strip_prefix("~/") {
        Some(format!("{home}/{rest}"))
    } else if input.starts_with('/') {
        Some(input.to_string())
    } else {
        None
    }
}

/// Ordered, deduplicated absolute paths: recent first, then zoxide, then repos found on disk.
pub fn candidates(claude_root: &Path, home: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |p: String| {
        if !p.is_empty() && Path::new(&p).is_dir() && seen.insert(p.clone()) {
            out.push(p);
        }
    };

    // Recent Claude projects, newest first.
    let mut recent: Vec<(u64, String)> = Vec::new();
    if let Ok(text) = fs::read_to_string(claude_root.join("history.jsonl")) {
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            if let (Some(p), Some(ts)) = (v["project"].as_str(), v["timestamp"].as_u64()) {
                recent.push((ts, p.to_string()));
            }
        }
    }
    recent.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in recent {
        add(p);
    }

    // zoxide, if installed: already ordered by frecency.
    if let Ok(o) = Command::new("zoxide").args(["query", "--list"]).output() {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                add(line.to_string());
            }
        }
    }

    add(home.to_string());
    // Non-hidden folders directly under home, then git repos under the usual project roots.
    let home_path = PathBuf::from(home);
    let mut top: Vec<PathBuf> = fs::read_dir(&home_path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !is_hidden(p) && !is_skipped(p))
        .collect();
    top.sort();
    for p in top {
        add(p.to_string_lossy().into_owned());
    }
    let mut found = Vec::new();
    for r in ROOTS {
        scan(&home_path.join(r), 0, &mut found);
    }
    found.sort();
    for p in found {
        add(p);
    }
    out
}

fn is_hidden(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'))
}

fn is_skipped(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| SKIP.contains(&n))
}

/// Collect directories containing `.git`, without descending into them.
fn scan(dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_DEPTH || out.len() >= MAX_TOTAL {
        return;
    }
    if dir.join(".git").exists() {
        out.push(dir.to_string_lossy().into_owned());
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && !is_hidden(&p) && !is_skipped(&p) {
            scan(&p, depth + 1, out);
        }
    }
}
