//! The search dialog (`/`) and renaming (`r`) end to end, in a real terminal with the real binary. Every directory
//! platoon touches is a temp dir, and no session is started (a real claude would stop at its trust prompt).

mod common;
use common::*;
use std::{fs, path::{Path, PathBuf}};

struct World {
    base: PathBuf,
}

impl World {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        for d in ["claude/sessions", "cache", "config"] {
            fs::create_dir_all(base.join(d)).unwrap();
        }
        // The conversations carry fixed dates that will be old one day: this file is not about auto-archive.
        fs::write(base.join("config/settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        Self { base }
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        let dir = |d: &str| self.base.join(d).to_string_lossy().into_owned();
        vec![
            ("CLAUDE_CONFIG_DIR", dir("claude")),
            ("PLATOON_CACHE_DIR", dir("cache")),
            ("PLATOON_CONFIG_DIR", dir("config")),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
        ]
    }

    /// A saved conversation run in a directory called `dir`, whose user typed `prompts`.
    fn save(&self, id: &str, dir: &str, prompts: &[&str], hour: u64) {
        let cwd = self.base.join("work").join(dir);
        fs::create_dir_all(&cwd).unwrap();
        let folder = self.base.join("claude/projects/-work");
        fs::create_dir_all(&folder).unwrap();
        let mut text = String::new();
        for (i, p) in prompts.iter().enumerate() {
            let ts = format!("2026-09-19T{hour:02}:{i:02}:00.000Z");
            text.push_str(&format!(
                r#"{{"type":"user","cwd":"{}","gitBranch":"main","timestamp":"{ts}","message":{{"role":"user","content":"{p}"}}}}"#,
                cwd.display()
            ));
            text.push('\n');
            text.push_str(&format!(
                r#"{{"type":"user","cwd":"{}","timestamp":"{ts}","message":{{"role":"user","content":[{{"tool_use_id":"t","type":"tool_result","content":"output about zebra"}}]}}}}"#,
                cwd.display()
            ));
            text.push('\n');
        }
        fs::write(folder.join(format!("{id}.jsonl")), text).unwrap();
    }

    fn names(&self) -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(self.base.join("config/names.json")).unwrap()).unwrap()["names"].clone()
    }
}

fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

#[test]
fn slash_opens_the_dialog_the_quick_filter_narrows_enter_keeps_it_and_esc_clears_it() {
    let w = World::new("search-quick");
    w.save("aaaaaaaa-1111", "alpha", &["fix the docker build"], 20);
    w.save("bbbbbbbb-2222", "beta", &["write the docs"], 19);
    let mut t = Term::spawn(&[], &w.env(), &w.base, 30, 100);
    t.wait_for("alpha");
    t.wait_for("beta");
    t.wait_for_flat("/ search"); // (the footer of a frame can arrive after its rows)
    t.wait_for_flat("r rename");

    t.send(b"/");
    t.wait_for("╭ Search");
    t.wait_for("Narrows the list by name, folder, branch and description.");
    // Typing right away narrows the list at once (the words are ones that are keys elsewhere, too).
    t.send(b"alp");
    t.wait_for_flat("2 shown"); // both are in the dialog's list until the words narrow it: alpha alone soon
    t.wait_for_flat("1 shown");
    t.wait_for("❯ alp");
    t.send(b"\r");
    t.wait_gone("╭ Search");
    t.wait_for("filter: alp");
    assert!(!t.text().contains("write the docs"), "kept: only alpha is listed\n{}", t.text());
    // Esc in the list clears the kept filter.
    t.send(b"\x1b");
    t.wait_for("write the docs");
    // Esc in the dialog clears it and closes.
    t.send(b"/");
    t.wait_for("╭ Search");
    t.send(b"zzz");
    t.wait_for("No sessions match the filter");
    t.send(b"\x1b");
    t.wait_gone("╭ Search");
    t.wait_for("write the docs");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn tab_finds_conversations_by_the_words_typed_in_them_and_enter_opens_the_archived_tab_for_one_put_away() {
    let w = World::new("search-full");
    w.save("aaaaaaaa-1111", "alpha", &["fix the docker build", "then add a migration for users"], 20);
    w.save("bbbbbbbb-2222", "beta", &["write the docs"], 19);
    w.save("cccccccc-3333", "gamma", &["set up the kubernetes cluster for staging"], 18);
    fs::write(w.base.join("config/archive.json"), r#"{"version":1,"archived":{"cccccccc-3333":1}}"#).unwrap();
    let mut t = Term::spawn(&[], &w.env(), &w.base, 30, 100);
    t.wait_for("alpha");
    t.wait_for_flat("2 Archived 1".replace("2 Archived 1", "Archived 1").as_str());

    t.send(b"/\t");
    t.wait_for("Type words you asked Claude about, in any order.");
    // Any order, any case; the words are lit in the message shown under the name.
    t.send(b"MIGRATION docker");
    t.wait_for("1 found");
    t.wait_for("then add a migration for users");
    assert!(!t.text().contains("Nothing found"));
    // Words that are only in tool output are not found.
    t.send(b"\x15zebra"); // ctrl+u clears the box
    t.wait_for("Nothing found");
    // A conversation on the Archived tab is found too, and Enter opens that tab with it selected.
    t.send(b"\x15KUBE staging");
    t.wait_for("set up the kubernetes cluster for staging");
    t.wait_for("archived");
    t.send(b"\r");
    t.wait_gone("╭ Search");
    t.wait_for("gamma");
    let screen = t.text();
    assert!(!screen.contains("alpha") && !screen.contains("beta"), "the Archived tab holds only gamma:\n{screen}");
    // The index is kept in its own file in the cache directory, next to the small conversation index.
    let cache = listing(&w.base.join("cache"));
    assert!(cache.contains(&"prompts.jsonl".to_string()) && cache.contains(&"index.json".to_string()), "{cache:?}");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn a_prompt_added_after_the_first_run_is_found_by_the_next_and_a_corrupt_cache_file_is_rebuilt() {
    let w = World::new("search-warm");
    w.save("aaaaaaaa-1111", "alpha", &["fix the docker build"], 20);
    {
        let mut t = Term::spawn(&[], &w.env(), &w.base, 30, 100);
        t.wait_for("alpha");
        t.send(b"/\tdocker");
        t.wait_for("1 found");
        let cache = w.base.join("cache/prompts.jsonl");
        until("the prompt index to be saved", || cache.exists());
        t.send(b"\x1b");
        t.wait_gone("╭ Search");
        t.send(b"q");
        t.wait_exit();
    }
    // The transcript grows while platoon is not running; the saved index only reads the new part.
    use std::io::Write;
    let mut f = fs::OpenOptions::new().append(true).open(w.base.join("claude/projects/-work/aaaaaaaa-1111.jsonl")).unwrap();
    writeln!(f, r#"{{"type":"user","cwd":"/x","timestamp":"2026-09-19T21:00:00.000Z","message":{{"role":"user","content":"and deploy with terraform"}}}}"#).unwrap();
    drop(f);
    {
        let mut t = Term::spawn(&[], &w.env(), &w.base, 30, 100);
        t.wait_for("alpha");
        t.send(b"/\tterraform");
        t.wait_for("1 found");
        t.wait_for("and deploy with terraform");
        t.send(b"\x1b");
        t.wait_gone("╭ Search");
        t.send(b"q");
        t.wait_exit();
    }
    // A damaged file is never an error: the index is simply read again.
    fs::write(w.base.join("cache/prompts.jsonl"), "{ not json at all\n\u{0}\u{0}").unwrap();
    let mut t = Term::spawn(&[], &w.env(), &w.base, 30, 100);
    t.wait_for("alpha");
    t.send(b"/\tdocker");
    t.wait_for("1 found"); // (the description alone is on the list behind the dialog too)
    t.wait_for("fix the docker build");
    t.send(b"\x1b");
    t.wait_gone("╭ Search");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn r_renames_a_conversation_and_the_name_is_kept_in_platoons_own_file_and_found_by_search() {
    let w = World::new("search-rename");
    w.save("aaaaaaaa-1111", "alpha", &["fix the docker build"], 20);
    w.save("bbbbbbbb-2222", "beta", &["write the docs"], 19);
    let transcript = w.base.join("claude/projects/-work/aaaaaaaa-1111.jsonl");
    let before = fs::read(&transcript).unwrap();
    let mut t = Term::spawn(&[], &w.env(), &w.base, 30, 100);
    t.wait_for("alpha");
    t.wait_for("beta");

    // Esc cancels: nothing is written.
    t.send(b"r");
    t.wait_for("rename conversation");
    t.wait_for("❯ alpha"); // the newest is first
    t.send(b"zzz");
    t.wait_for("❯ alphazzz");
    t.send(b"\x1b");
    t.wait_gone("rename conversation");
    assert!(!w.base.join("config/names.json").exists());

    // Enter saves: the box starts with the current name; ctrl+u empties it.
    t.send(b"r");
    t.wait_for("❯ alpha");
    t.send(b"\x15Payments work");
    t.wait_for("❯ Payments work");
    t.send(b"\r");
    t.wait_gone("rename conversation");
    t.wait_for("Payments work");
    until("names.json", || w.base.join("config/names.json").exists());
    assert_eq!(w.names(), serde_json::json!({"aaaaaaaa-1111": "Payments work"}));
    assert_eq!(fs::read(&transcript).unwrap(), before, "Claude's own files are never written");

    // The quick filter and the word search both find it by the name.
    t.send(b"/payments");
    t.wait_for("1 shown");
    t.send(b"\t");
    t.wait_for("1 found");
    t.send(b"\x1b");
    t.wait_gone("╭ Search");

    // An empty name gives the automatic one back.
    t.send(b"r");
    t.wait_for("❯ Payments work");
    t.send(b"\x15\r");
    t.wait_gone("rename conversation");
    t.wait_for("Name cleared");
    until("the name to leave the file", || w.names() == serde_json::json!({}));
    t.send(b"q");
    t.wait_exit();
    assert!(!w.base.join("claude/names.json").exists());
}
