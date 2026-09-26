//! The `/to-tickets` skill: a Claude Code skill that turns a conversation (or a plan the user points at) into tickets in
//! the session's project, after the user approves the list. Its text is platoon's own (`SKILL`, from
//! `src/skills/to-tickets/SKILL.md`); `platoon setup` writes it to `<claude config dir>/skills/to-tickets/SKILL.md`,
//! says when the copy there is out of date, and `platoon setup --remove to-tickets` takes it out again.
//!
//! Also `platoon squads --kinds`: each squad's name, what it can do and whether it needs a folder, which the skill
//! reads to shape the tickets (it never picks a squad).

use serde_json::{json, Value};
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

#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    No,
    Yes,
    /// There, but not what this platoon would write (platoon's copy changed, or the file was edited).
    OutOfDate,
}

pub fn installed(root: &Path) -> Installed {
    match fs::read_to_string(skill_path(root)) {
        Ok(text) if text == SKILL => Installed::Yes,
        Ok(_) => Installed::OutOfDate,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Installed::No,
        // Something is there that can't be read (a folder, no permission): not what platoon wrote.
        Err(_) => Installed::OutOfDate,
    }
}

/// Write platoon's copy. A different copy that was there is kept beside it as `SKILL.md.platoon-bak` first.
pub fn install(root: &Path) -> Result<(), String> {
    let path = skill_path(root);
    let dir = path.parent().unwrap_or(root);
    fs::create_dir_all(dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    if installed(root) == Installed::OutOfDate && path.is_file() {
        fs::copy(&path, backup_path(root)).map_err(|e| format!("couldn't back up {}: {e}", path.display()))?;
    }
    crate::projects::write_atomic(&path, SKILL).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

/// Take the skill out: its `SKILL.md`, and its folder when nothing else is left in it (a backup setup made goes too).
/// True when it was there.
pub fn remove(root: &Path) -> Result<bool, String> {
    let path = skill_path(root);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("couldn't remove {}: {e}", path.display())),
    }
    let _ = fs::remove_file(backup_path(root));
    if let Some(dir) = path.parent() {
        let _ = fs::remove_dir(dir); // only when empty: anything else the user put there stays
    }
    Ok(true)
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
            "--body-file \"$body\"",
            "--waits-for \"3\"",
            "each ticket after the tickets it waits for",
            "use those printed numbers",
            "every working folder this session has",
            "--folder",
            "--no-folder",
            "mktemp",
            "rm -f \"$body\"",
        ] {
            assert!(w.contains(want), "{want:?}");
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
    fn installing_writes_platoons_copy_under_the_claude_config_folder_and_tells_an_out_of_date_one() {
        let root = dir("install");
        assert_eq!(installed(&root), Installed::No);
        install(&root).unwrap();
        assert_eq!(fs::read_to_string(root.join("skills/to-tickets/SKILL.md")).unwrap(), SKILL);
        assert_eq!(installed(&root), Installed::Yes);
        // Changed since (platoon's copy moved on, or someone edited it): out of date, and setup puts it right,
        // keeping the one it replaced.
        fs::write(skill_path(&root), "an older skill").unwrap();
        assert_eq!(installed(&root), Installed::OutOfDate);
        install(&root).unwrap();
        assert_eq!(installed(&root), Installed::Yes);
        assert_eq!(fs::read_to_string(backup_path(&root)).unwrap(), "an older skill");
        // Removed: the file, the backup and the empty folder go; `skills/` and other skills stay.
        fs::create_dir_all(root.join("skills/mine")).unwrap();
        assert!(remove(&root).unwrap());
        assert!(!root.join("skills/to-tickets").exists() && root.join("skills/mine").is_dir());
        assert_eq!(installed(&root), Installed::No);
        assert!(!remove(&root).unwrap(), "nothing to remove the second time");
        // Something of the user's own in the folder stays, and so does the folder.
        install(&root).unwrap();
        fs::write(root.join("skills/to-tickets/notes.txt"), "mine").unwrap();
        assert!(remove(&root).unwrap());
        assert!(root.join("skills/to-tickets/notes.txt").is_file() && !skill_path(&root).exists());
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
