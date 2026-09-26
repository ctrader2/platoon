//! The `/to-tickets` skill: a Claude Code skill that turns a conversation (or a plan the user points at) into tickets in
//! the session's project, after the user approves the list. Its text is platoon's own (`SKILL`, from
//! `src/skills/to-tickets/SKILL.md`); `platoon setup` writes it to `<claude config dir>/skills/to-tickets/SKILL.md`,
//! updates its own untouched copy when platoon's changes, leaves a copy the user changed alone (`--replace` puts
//! platoon's in, keeping theirs), and `platoon setup --remove to-tickets` takes it out and remembers that (`--add`
//! brings it back). What setup remembers is in `<platoon config>/setup.json`.
//!
//! Also `platoon squads --kinds`: each squad's name, what it can do and whether it needs a folder, which the skill
//! reads to shape the tickets (it never picks a squad).

use serde_json::{json, Map, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// The skill as platoon writes it.
pub const SKILL: &str = include_str!("skills/to-tickets/SKILL.md");

/// Its name: the folder under `skills/`, and what the user types after `/`.
pub const NAME: &str = "to-tickets";

/// Where the skill goes in Claude Code's config folder (`CLAUDE_CONFIG_DIR`, else `~/.claude`).
pub fn skill_path(root: &Path) -> PathBuf {
    root.join("skills").join(NAME).join("SKILL.md")
}

/// Where a copy that setup replaced is kept, in case the user had changed it.
pub fn backup_path(root: &Path) -> PathBuf {
    root.join("skills").join(NAME).join("SKILL.md.platoon-bak")
}

/// FNV-1a of the exact text: what setup remembers about the copy it last wrote.
pub fn hash(text: &str) -> String {
    let h = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{h:016x}")
}

/// Hashes of the copies earlier platoons wrote before setup kept a record (S4, `1623a0e`): a file that is one of these
/// word for word is platoon's own and is updated quietly, not taken for the user's.
const EARLIER: &[&str] = &["95bb7f31c8b4d66f"];

// ---- what setup remembers: <platoon config>/setup.json ----------------------------------------------------------------
//
// `{"to-tickets": {"written": "<hash of the copy platoon last wrote>", "removed": true}}`. Every other key, at either
// level, is kept as it is.

fn choices_path(config: &Path) -> PathBuf {
    config.join("setup.json")
}

/// The file's object, or None when there is one that can't be read (it is then left alone, never overwritten).
fn load_choices(config: &Path) -> Option<Map<String, Value>> {
    match fs::read_to_string(choices_path(config)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Some(Map::new()),
        Err(_) => None,
        Ok(text) => match serde_json::from_str(&text) {
            Ok(Value::Object(o)) => Some(o),
            _ => None,
        },
    }
}

fn record(config: &Path) -> Map<String, Value> {
    load_choices(config).and_then(|mut o| o.remove(NAME)).and_then(|v| match v {
        Value::Object(o) => Some(o),
        _ => None,
    }).unwrap_or_default()
}

/// Change this skill's record (`None` removes a key). Nothing is written when nothing changes.
fn change_record(config: &Path, changes: &[(&str, Option<Value>)]) -> Result<(), String> {
    let path = choices_path(config);
    let mut all = load_choices(config).ok_or_else(|| format!("{} can't be read, so platoon left it alone", path.display()))?;
    let entry = all.entry(NAME.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    let Some(rec) = entry.as_object_mut() else { return Ok(()) };
    let before = rec.clone();
    for (key, value) in changes {
        match value {
            Some(v) => {
                rec.insert(key.to_string(), v.clone());
            }
            None => {
                rec.remove(*key);
            }
        }
    }
    if *rec == before {
        return Ok(());
    }
    if rec.is_empty() {
        all.remove(NAME);
    }
    let text = serde_json::to_string_pretty(&Value::Object(all)).unwrap_or_default() + "\n";
    crate::projects::write_atomic(&path, &text).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

/// The user took the skill out with `platoon setup --remove to-tickets`, and hasn't asked for it back.
pub fn removed_by_user(config: &Path) -> bool {
    record(config).get("removed").and_then(Value::as_bool) == Some(true)
}

fn written_hash(config: &Path) -> Option<String> {
    record(config).get("written").and_then(Value::as_str).map(str::to_string)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    No,
    /// Not there because the user removed it; setup leaves it out until `--add`.
    Removed,
    Yes,
    /// The copy platoon last wrote, untouched, but platoon's own copy has changed since: setup updates it quietly.
    OutOfDate,
    /// Not the copy platoon last wrote (the user changed it, or it can't be read): setup leaves it alone.
    Changed,
}

fn missing(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
}

pub fn installed(root: &Path, config: &Path) -> Installed {
    match fs::read_to_string(skill_path(root)) {
        Ok(text) if text == SKILL => Installed::Yes,
        Ok(text) => {
            let h = hash(&text);
            let ours = match written_hash(config) {
                Some(w) => w == h,
                None => EARLIER.contains(&h.as_str()),
            };
            if ours { Installed::OutOfDate } else { Installed::Changed }
        }
        // Not there (a file where a folder on the way should be counts as not there: writing it will say why).
        Err(e) if missing(&e) && removed_by_user(config) => Installed::Removed,
        Err(e) if missing(&e) => Installed::No,
        // Something is there that can't be read (a folder, no permission): not platoon's to overwrite.
        Err(_) => Installed::Changed,
    }
}

/// Write platoon's copy and remember it. The caller decides whether it may (see `installed`).
fn write(root: &Path, config: &Path) -> Result<(), String> {
    let path = skill_path(root);
    let dir = path.parent().unwrap_or(root);
    fs::create_dir_all(dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    crate::projects::write_atomic(&path, SKILL).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    // The record is what tells a later setup that this copy is platoon's; a failure to keep it only means a later
    // change of platoon's copy will ask instead of updating quietly.
    let _ = change_record(config, &[("written", Some(Value::String(hash(SKILL)))), ("removed", None)]);
    Ok(())
}

/// What `platoon setup` does for the skill: write it when it's missing, update platoon's own untouched copy, and leave
/// a removed or changed one alone. Returns what the file was before.
pub fn setup(root: &Path, config: &Path) -> Result<Installed, String> {
    let state = installed(root, config);
    match state {
        Installed::No | Installed::OutOfDate => write(root, config)?,
        // Already right: make sure the record says so (a copy written before setup kept records).
        Installed::Yes => {
            if written_hash(config).as_deref() != Some(hash(SKILL).as_str()) {
                let _ = change_record(config, &[("written", Some(Value::String(hash(SKILL))))]);
            }
        }
        Installed::Removed | Installed::Changed => {}
    }
    Ok(state)
}

/// `platoon setup --replace to-tickets`: platoon's copy in place of whatever is there, the user's kept as
/// `SKILL.md.platoon-bak`. Returns whether a different copy was kept.
pub fn replace(root: &Path, config: &Path) -> Result<bool, String> {
    let path = skill_path(root);
    let kept = match fs::read_to_string(&path) {
        Ok(text) if text == SKILL => false,
        Ok(_) => {
            fs::copy(&path, backup_path(root)).map_err(|e| format!("couldn't keep your copy of {}: {e}", path.display()))?;
            true
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(format!("couldn't read {}: {e}", path.display())),
    };
    write(root, config)?;
    Ok(kept)
}

/// `platoon setup --add to-tickets`: forget a removal, then do what setup does.
pub fn add(root: &Path, config: &Path) -> Result<Installed, String> {
    change_record(config, &[("removed", None)])?;
    setup(root, config)
}

/// Take the skill out: its `SKILL.md`, and its folder when nothing else is left in it (a backup setup made goes too),
/// and remember that the user doesn't want it, so setup doesn't put it back. True when it was there.
pub fn remove(root: &Path, config: &Path) -> Result<bool, String> {
    let path = skill_path(root);
    let was = match fs::remove_file(&path) {
        Ok(()) => true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(format!("couldn't remove {}: {e}", path.display())),
    };
    if was {
        let _ = fs::remove_file(backup_path(root));
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir(dir); // only when empty: anything else the user put there stays
        }
    }
    change_record(config, &[("removed", Some(Value::Bool(true))), ("written", None)])?;
    Ok(was)
}

// ---- platoon squads --kinds ---------------------------------------------------------------------------------------

const SQUADS_USAGE: &str = "usage: platoon squads --kinds [--json]
  --kinds   each squad's name, what it can do, and whether it needs a folder to work in
            (the /to-tickets skill reads this to shape tickets; it never picks a squad)";

/// `platoon squads ...`. Returns the exit code.
pub fn squads_cli(args: &[String], config: &Path, out: &mut dyn io::Write, err: &mut dyn io::Write) -> i32 {
    let (mut kinds, mut as_json) = (false, false);
    for a in args {
        match a.as_str() {
            "--kinds" => kinds = true,
            "--json" => as_json = true,
            other => {
                let _ = writeln!(err, "platoon squads: don't know \"{other}\"\n{SQUADS_USAGE}");
                return 2;
            }
        }
    }
    if !kinds {
        let _ = writeln!(err, "{SQUADS_USAGE}");
        return 2;
    }
    let _ = write!(out, "{}", kinds_text(config, as_json));
    0
}

/// Every squad the user has now (built-ins first, then their own), with a readable file: name, what it can do and
/// whether it needs a folder. A squad whose file can't be read is left out.
pub fn kinds_text(config: &Path, as_json: bool) -> String {
    let squads: Vec<crate::squadfile::Squad> = crate::squadfile::load_all(config).into_iter().filter_map(|l| l.squad).collect();
    if as_json {
        let rows: Vec<Value> = squads.iter().map(|s| json!({"id": s.id, "name": s.name, "can_do": s.can_do, "needs_folder": s.needs_folder})).collect();
        return serde_json::to_string_pretty(&Value::Array(rows)).unwrap_or_default() + "\n";
    }
    if squads.is_empty() {
        return "No squads yet.\n".into();
    }
    let mut text = String::new();
    for s in &squads {
        let folder = if s.needs_folder { "needs a folder" } else { "no folder needed" };
        text.push_str(&format!("{} ({folder})\n", s.name));
        let can_do = s.can_do.split_whitespace().collect::<Vec<_>>().join(" ");
        if !can_do.is_empty() {
            text.push_str(&format!("  {can_do}\n"));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-totickets-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_skill_is_typed_only_and_asks_platoon_about_the_project_before_anything_else() {
        let (front, body) = SKILL.strip_prefix("---\n").and_then(|s| s.split_once("\n---\n")).expect("frontmatter");
        assert!(front.lines().any(|l| l == "name: to-tickets"), "{front}");
        assert!(front.lines().any(|l| l == "disable-model-invocation: true"), "Claude never runs it by itself: {front}");
        assert!(front.lines().any(|l| l.starts_with("description: ")), "{front}");
        // Its own platoon commands need no permission prompt (Claude Code's `allowed-tools`, as a YAML list), and
        // nothing else is pre-allowed.
        assert!(front.ends_with("allowed-tools:\n  - \"Bash(platoon ticket:*)\"\n  - \"Bash(platoon squads:*)\""), "{front}");
        assert_eq!(front.matches("Bash(").count(), 2, "{front}");
        // Every command in it is a `platoon ticket` or `platoon squads` one, so those two rules cover them all.
        for block in body.split("```sh\n").skip(1) {
            let cmd = block.lines().next().unwrap();
            assert!(cmd.starts_with("platoon ticket ") || cmd.starts_with("platoon squads "), "{cmd}");
        }
        // The first command is the project check, and a failure stops it.
        let first = body.find("```sh\n").unwrap() + 6;
        assert!(body[first..].starts_with("platoon ticket where --session \"$CLAUDE_CODE_SESSION_ID\"\n"), "{}", &body[first..first + 80]);
        assert!(body.find("## 1.").unwrap() < first && first < body.find("## 2.").unwrap());
        let step1 = &body[body.find("## 1.").unwrap()..body.find("## 2.").unwrap()];
        assert!(step1.contains("If it fails, tell the user the line it printed and stop. Do nothing else"), "{step1}");
        assert!(step1.contains("press m"), "{step1}");
        assert!(body.contains("do not guess a project"), "never guesses a project");
    }

    #[test]
    fn the_skill_shapes_tickets_by_kind_of_work_quizzes_the_user_and_writes_only_through_platoon() {
        let body = SKILL;
        // Kinds of work, never a squad.
        assert!(body.contains("platoon squads --kinds") && body.contains("Never pick a squad"), "squads");
        assert!(body.contains("either should be able to take the ticket"), "two squads of one kind");
        // The shape.
        for want in ["fits in one fresh session", "not a list of file changes", "- [ ] ", "must fail before the work", "No file paths", "How far: local changes only.", "Preparation first", "waits for"] {
            assert!(body.contains(want), "{want:?}");
        }
        // The quiz, before anything is written.
        let quiz = body.find("## 4.").unwrap();
        let write = body.find("## 5.").unwrap();
        assert!(quiz < write);
        let q = &body[quiz..write];
        for want in ["numbered list", "right size", "merged or split", "\"waits for\" links real", "clearly approves", "write\nnothing to platoon"] {
            assert!(q.contains(want), "{want:?}: {q}");
        }
        // Writing: through platoon only, blockers first, with the numbers it printed, the session's folders.
        let w = &body[write..];
        for want in [
            "platoon ticket add --from-session \"$CLAUDE_CODE_SESSION_ID\"",
            "--waits-for \"3\"",
            "each ticket after the tickets it waits for",
            "use those printed numbers",
            "every working folder this session has",
            "--folder",
            "--no-folder",
            "no temporary file",
        ] {
            assert!(w.contains(want), "{want:?}");
        }
        // One `platoon` command per ticket, the body on stdin through a quoted heredoc, so the pre-allowed
        // `platoon ticket` rule covers it: no temporary file, no other program.
        let block = &w[w.find("```sh\n").unwrap() + 6..];
        let block = &block[..block.find("```").unwrap()];
        let lines: Vec<&str> = block.lines().collect();
        assert!(lines[0].starts_with("platoon ticket add --from-session \"$CLAUDE_CODE_SESSION_ID\" ") && lines[0].ends_with(" <<'PLATOON_TICKET_BODY'"), "{block}");
        assert_eq!(lines.last(), Some(&"PLATOON_TICKET_BODY"), "{block}");
        assert_eq!(lines.iter().filter(|l| l.contains("platoon")).count(), 1, "one platoon command: {block}");
        for bad in ["mktemp", "rm -f", "cat >", "--body-file", "$(", "&&", "; ", "|"] {
            assert!(!block.contains(bad), "{bad:?} in {block}");
        }
        for bad in ["mktemp", "rm -f", "cat >"] {
            assert!(!body.contains(bad), "{bad:?} anywhere in the skill");
        }
        assert!(!w.contains("--project \""), "never names a project itself");
        // Every argument quoted: each value after a flag in the example is in double quotes.
        let line = w.lines().find(|l| l.starts_with("platoon ticket add")).unwrap();
        let words: Vec<&str> = line.split(' ').collect();
        for (i, word) in words.iter().enumerate() {
            if word.starts_with("--") && i + 1 < words.len() && !words[i + 1].starts_with("--") {
                assert!(words[i + 1].starts_with('"'), "{word} {}", words[i + 1]);
            }
        }
        // Stops on an error, and never touches platoon's files.
        assert!(body.contains("If any `platoon` command fails, stop there, show the user what it printed"));
        assert!(body.contains("Never write, edit or\ndelete platoon's own files"));
        for bad in [".config/platoon", "tickets.json\"", "Edit the", "edit tickets.json", "Write tool"] {
            assert!(!body.contains(bad), "{bad:?}");
        }
        // The end: the numbers, where they are, and that a squad is picked there.
        assert!(body.contains("number platoon printed") && body.contains("press a"));
    }

    #[test]
    fn setup_writes_platoons_copy_updates_its_own_leaves_a_changed_one_and_remembers_a_removal() {
        let root = dir("install");
        let config = root.join("platoon-config");
        let rec = || -> Value { serde_json::from_str(&fs::read_to_string(config.join("setup.json")).unwrap()).unwrap() };
        assert_eq!(installed(&root, &config), Installed::No);
        assert_eq!(setup(&root, &config).unwrap(), Installed::No);
        assert_eq!(fs::read_to_string(root.join("skills/to-tickets/SKILL.md")).unwrap(), SKILL);
        assert_eq!(installed(&root, &config), Installed::Yes);
        assert_eq!(rec()["to-tickets"]["written"], hash(SKILL), "what it wrote is remembered");
        // platoon's copy moves on (here: the record says platoon wrote something else): its own untouched copy is
        // updated quietly, without a backup.
        fs::write(skill_path(&root), "an older skill").unwrap();
        change_record(&config, &[("written", Some(json!(hash("an older skill"))))]).unwrap();
        assert_eq!(installed(&root, &config), Installed::OutOfDate);
        assert_eq!(setup(&root, &config).unwrap(), Installed::OutOfDate);
        assert_eq!(installed(&root, &config), Installed::Yes);
        assert!(!backup_path(&root).exists());
        // The user changed it: left alone, every time.
        fs::write(skill_path(&root), "mine now").unwrap();
        assert_eq!(installed(&root, &config), Installed::Changed);
        assert_eq!(setup(&root, &config).unwrap(), Installed::Changed);
        assert_eq!(fs::read_to_string(skill_path(&root)).unwrap(), "mine now");
        // Until they ask: theirs is kept, platoon's goes in.
        assert!(replace(&root, &config).unwrap());
        assert_eq!(fs::read_to_string(backup_path(&root)).unwrap(), "mine now");
        assert_eq!(installed(&root, &config), Installed::Yes);
        assert!(!replace(&root, &config).unwrap(), "nothing different to keep the second time");
        // Removed: the file, the backup and the empty folder go; `skills/` and other skills stay; setup leaves it out.
        fs::write(config.join("setup.json"), serde_json::to_string(&json!({"theirs": 1, "to-tickets": {"written": hash(SKILL), "extra": "kept"}})).unwrap()).unwrap();
        fs::create_dir_all(root.join("skills/mine")).unwrap();
        assert!(remove(&root, &config).unwrap());
        assert!(!root.join("skills/to-tickets").exists() && root.join("skills/mine").is_dir());
        assert_eq!(installed(&root, &config), Installed::Removed);
        assert_eq!(rec(), json!({"theirs": 1, "to-tickets": {"extra": "kept", "removed": true}}), "unknown keys kept");
        assert_eq!(setup(&root, &config).unwrap(), Installed::Removed);
        assert!(!skill_path(&root).exists(), "not put back");
        assert!(!remove(&root, &config).unwrap(), "nothing to remove the second time");
        // Asked for again: written, and the mark is gone.
        assert_eq!(add(&root, &config).unwrap(), Installed::No);
        assert_eq!(installed(&root, &config), Installed::Yes);
        assert_eq!(rec(), json!({"theirs": 1, "to-tickets": {"extra": "kept", "written": hash(SKILL)}}));
        // Something of the user's own in the folder stays, and so does the folder.
        fs::write(root.join("skills/to-tickets/notes.txt"), "mine").unwrap();
        assert!(remove(&root, &config).unwrap());
        assert!(root.join("skills/to-tickets/notes.txt").is_file() && !skill_path(&root).exists());
        // A setup.json that can't be read is left alone; removing then says why it can't remember.
        fs::write(config.join("setup.json"), "{ nope").unwrap();
        assert!(remove(&root, &config).unwrap_err().contains("can't be read"));
        assert_eq!(fs::read_to_string(config.join("setup.json")).unwrap(), "{ nope");
    }

    #[test]
    fn a_copy_an_earlier_platoon_wrote_before_records_were_kept_counts_as_platoons_own() {
        let root = dir("earlier");
        let config = root.join("platoon-config");
        let old = include_str!("../tests/fixtures/to-tickets-skill-1623a0e.md");
        assert_ne!(old, SKILL, "platoon's copy has moved on since");
        fs::create_dir_all(skill_path(&root).parent().unwrap()).unwrap();
        fs::write(skill_path(&root), old).unwrap();
        assert_eq!(installed(&root, &config), Installed::OutOfDate);
        // With no record, anything else is the user's.
        fs::write(skill_path(&root), format!("{old}\nmy line\n")).unwrap();
        assert_eq!(installed(&root, &config), Installed::Changed);
        // A folder where the file should be can't be read: left alone too.
        fs::remove_file(skill_path(&root)).unwrap();
        fs::create_dir(skill_path(&root)).unwrap();
        assert_eq!(installed(&root, &config), Installed::Changed);
    }

    fn run(config: &Path, args: &[&str]) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = squads_cli(&args, config, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn squads_kinds_lists_what_each_squad_can_do_and_whether_it_needs_a_folder() {
        let config = dir("kinds");
        let (code, out, err) = run(&config, &["--kinds"]);
        assert_eq!((code, err.as_str()), (0, ""));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Coding (needs a folder)", "{out}");
        assert_eq!(lines[1], format!("  {}", crate::squads::CODING_CAN_DO));
        assert!(lines.contains(&"Research (no folder needed)") && lines.contains(&"Docs (needs a folder)") && lines.contains(&"Summarize (no folder needed)"), "{out}");
        // A squad of the user's own is there too, with its own words.
        let mut own = crate::squadfile::builtin("docs").unwrap();
        own.id = "coda-docs".into();
        own.name = "Coda docs".into();
        own.can_do = "Writes pages in Coda.".into();
        own.needs_folder = false;
        crate::squadfile::save(&config, &own).unwrap();
        let (_, out, _) = run(&config, &["--kinds"]);
        assert!(out.ends_with("Coda docs (no folder needed)\n  Writes pages in Coda.\n"), "{out}");
        let (code, out, _) = run(&config, &["--kinds", "--json"]);
        assert_eq!(code, 0);
        let v: Value = serde_json::from_str(&out).unwrap();
        let a = v.as_array().unwrap();
        assert_eq!(a.len(), 5);
        assert_eq!((a[0]["name"].as_str(), a[0]["needs_folder"].as_bool()), (Some("Coding"), Some(true)));
        assert_eq!(a[4], json!({"id": "coda-docs", "name": "Coda docs", "can_do": "Writes pages in Coda.", "needs_folder": false}));
        // A file that can't be read is left out, not shown as a squad.
        fs::write(crate::squadfile::path(&config, "broken"), "{ nope").unwrap();
        let (_, out, _) = run(&config, &["--kinds", "--json"]);
        assert_eq!(serde_json::from_str::<Value>(&out).unwrap().as_array().unwrap().len(), 5);
        // Without --kinds, or with something it doesn't know: how to use it.
        let (code, out, err) = run(&config, &[]);
        assert!(code == 2 && out.is_empty() && err.contains("--kinds"), "{err}");
        assert_eq!(run(&config, &["--kinds", "--nope"]).0, 2);
    }
}
