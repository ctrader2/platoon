//! The `/to-tasks` skill platoon ships for Claude Code: typed in a session, it turns the work agreed there into
//! platoon tasks for a squad (`src/skills/to-tasks/SKILL.md`, compiled in). `platoon setup` writes it to
//! `<claude root>/skills/to-tasks/SKILL.md` (`install`), never as part of the start-up gate, and `platoon uninstall`
//! takes it out again (`remove_at`).
//!
//! Through platoon 1.3 it was called `/to-tickets` (`<claude root>/skills/to-tickets/`, `old_path`). Setup renames
//! platoon's copy: once platoon's `/to-tasks` is in place, platoon's unchanged old copy is deleted (`retire_old`); an
//! old copy the user changed is kept. Until setup runs, the old one keeps working, so whatever tells the user to type
//! the skill names the one that is there (`command`), and either counts as installed (`present`).
//!
//! Only a file platoon wrote is ever replaced or removed. Each copy setup writes ends in a line that says so and carries
//! the POSIX `cksum` of everything above it (`trailer`): a copy is platoon's while that line is there and matches
//! (`state`). The user's own skill of that name, or platoon's copy once they edit it, is left as it is. No record is kept
//! anywhere else, so this holds whichever platoon wrote the copy, from wherever, and `scripts/uninstall.sh` can tell
//! the same with `cksum` alone.
//!
//! The skill names platoon by the full path of the platoon that wrote it (`{{platoon}}` in the template), so it works
//! whatever `platoon` resolves to in the session's shell; a copy written by a platoon at another path is rewritten.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// The skill as shipped, with `PROGRAM` where platoon's path goes.
const TEMPLATE: &str = include_str!("skills/to-tasks/SKILL.md");

/// Where the template names platoon.
const PROGRAM: &str = "{{platoon}}";

/// How the last line of every copy platoon writes starts; it ends ` cksum <N> -->`. `scripts/uninstall.sh` reads the
/// same line: change both together.
const TRAILER_START: &str = "<!-- platoon setup wrote this file and keeps it up to date; once you change it, platoon leaves it alone.";

/// The command the user types for the skill.
pub const COMMAND: &str = "/to-tasks";

/// What the skill was called through platoon 1.3: still what is typed where setup hasn't run since.
pub const OLD_COMMAND: &str = "/to-tickets";

/// The skill's file under Claude Code's folder `root`.
pub fn path(root: &Path) -> PathBuf {
    root.join("skills").join("to-tasks").join("SKILL.md")
}

/// Where platoon 1.3 and before wrote the skill, as `/to-tickets`.
pub fn old_path(root: &Path) -> PathBuf {
    root.join("skills").join("to-tickets").join("SKILL.md")
}

/// The file setup writes for the platoon at `exe`: the template with `exe` in it (as a shell word; in the front
/// matter also escaped for YAML's double quotes), then the trailer.
pub fn text(exe: &str) -> String {
    let word = crate::agents::claude::shell_word(exe);
    let yaml = word.replace('\\', "\\\\").replace('"', "\\\"");
    let end = TEMPLATE[3..].find("\n---\n").map_or(0, |i| i + 3 + 5);
    let body = format!("{}{}", TEMPLATE[..end].replace(PROGRAM, &yaml), TEMPLATE[end..].replace(PROGRAM, &word));
    let trailer = trailer(&body);
    body + &trailer
}

/// The last line of a copy platoon writes whose lines above are `body`.
fn trailer(body: &str) -> String {
    format!("{TRAILER_START} cksum {} -->\n", cksum(body.as_bytes()))
}

/// Which text of the skill this platoon ships, whatever path a copy names: it changes when a later platoon changes the
/// skill.
pub fn version() -> u32 {
    cksum(TEMPLATE.as_bytes())
}

/// The CRC the POSIX `cksum` command prints for `data` (the same on Linux and macOS).
pub fn cksum(data: &[u8]) -> u32 {
    fn step(crc: u32, byte: u8) -> u32 {
        let mut c = crc ^ (u32::from(byte) << 24);
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
        }
        c
    }
    let mut crc = data.iter().fold(0, |c, &b| step(c, b));
    let mut n = data.len();
    while n > 0 {
        crc = step(crc, (n & 0xff) as u8);
        n >>= 8;
    }
    !crc
}

/// What is at the skill's place.
#[derive(Debug, PartialEq, Eq)]
pub enum State {
    Missing,
    /// A copy platoon wrote, unchanged since: its text.
    Ours(String),
    /// Something platoon didn't write (or a copy changed since, or a link): never touched.
    NotOurs,
}

/// Is `text` a copy platoon wrote and nobody changed: its last line is the trailer, with the cksum of the rest?
fn is_ours(text: &str) -> bool {
    let Some(body_end) = text.strip_suffix('\n').and_then(|t| t.rfind('\n')).map(|i| i + 1) else { return false };
    let (body, last) = text.split_at(body_end);
    last.starts_with(TRAILER_START) && last == trailer(body)
}

/// What is at the skill's place under `root`. Err: it is there but couldn't be read.
pub fn state(root: &Path) -> Result<State, String> {
    state_at(&path(root))
}

/// What is at the old `/to-tickets` skill's place under `root`, judged as `state` judges the current one (platoon's
/// copies have always ended in the same trailer).
pub fn old_state(root: &Path) -> Result<State, String> {
    state_at(&old_path(root))
}

fn state_at(p: &Path) -> Result<State, String> {
    match fs::symlink_metadata(p) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(State::Missing),
        Err(e) => return Err(format!("couldn't read {}: {e}", p.display())),
        // A link (a dotfiles manager's, say) is the user's arrangement, whatever it points at.
        Ok(m) if !m.is_file() => return Ok(State::NotOurs),
        Ok(_) => {}
    }
    match fs::read(p) {
        Ok(bytes) => Ok(String::from_utf8(bytes).ok().filter(|t| is_ours(t)).map_or(State::NotOurs, State::Ours)),
        Err(e) => Err(format!("couldn't read {}: {e}", p.display())),
    }
}

/// Is something at that place for Claude Code to run, platoon's copy or the user's own? (One that can't be read counts
/// as not there.)
fn there(state: &Result<State, String>) -> bool {
    matches!(state, Ok(State::Ours(_) | State::NotOurs))
}

/// Is the skill there for Claude Code to run, under either name, platoon's copy or the user's own?
pub fn present(root: &Path) -> bool {
    there(&state(root)) || there(&old_state(root))
}

/// The command to tell the user to type: `/to-tasks`, or `/to-tickets` while only that is there (platoon updated, setup
/// not run since). With neither, `/to-tasks`, the one setup adds.
pub fn command(root: &Path) -> &'static str {
    if !there(&state(root)) && there(&old_state(root)) { OLD_COMMAND } else { COMMAND }
}

/// What `install` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    Added,
    /// platoon's older copy (another text, or another platoon's path) replaced.
    Updated,
    AlreadyDone,
    /// A file platoon didn't write is there: left as it is.
    NotOurs(PathBuf),
    /// Couldn't be read or written: why.
    Failed(String),
}

/// Put the skill for the platoon at `exe` in Claude Code's folder `root`: written when missing, rewritten when it is
/// platoon's copy and differs from this one, left alone when it isn't platoon's.
pub fn install(root: &Path, exe: &str) -> Installed {
    let p = path(root);
    let want = text(exe);
    let outcome = match state(root) {
        Err(why) => return Installed::Failed(why),
        Ok(State::NotOurs) => return Installed::NotOurs(p),
        Ok(State::Ours(have)) if have == want => return Installed::AlreadyDone,
        Ok(State::Ours(_)) => Installed::Updated,
        Ok(State::Missing) => Installed::Added,
    };
    match crate::projects::write_atomic(&p, &want) {
        Ok(()) => outcome,
        Err(e) => Installed::Failed(format!("couldn't write {}: {e}", p.display())),
    }
}

/// What `remove` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Removal {
    /// platoon's copy deleted (and its folder, when that left it empty).
    Removed(PathBuf),
    NotThere,
    /// A file platoon didn't write is there: kept.
    NotOurs(PathBuf),
    Failed(String),
}

/// Take platoon's copy of the skill at `p` (`path` or `old_path` of Claude Code's folder) out, and its folder when
/// nothing else is in it.
pub fn remove_at(p: &Path) -> Removal {
    let p = p.to_path_buf();
    match state_at(&p) {
        Err(why) => Removal::Failed(why),
        Ok(State::Missing) => Removal::NotThere,
        Ok(State::NotOurs) => Removal::NotOurs(p),
        Ok(State::Ours(_)) => match fs::remove_file(&p) {
            Ok(()) => {
                if let Some(dir) = p.parent() {
                    let _ = fs::remove_dir(dir);
                }
                Removal::Removed(p)
            }
            Err(e) => Removal::Failed(format!("couldn't delete {}: {e}", p.display())),
        },
    }
}

/// What `retire_old` did with the old `/to-tickets` copy.
#[derive(Debug, PartialEq, Eq)]
pub enum Retired {
    NotThere,
    /// platoon's copy, unchanged, deleted now that platoon's `/to-tasks` is in place (and its folder, when empty).
    Removed(PathBuf),
    /// Changed since platoon wrote it, or never platoon's: kept.
    NotOurs(PathBuf),
    /// platoon's copy, kept because `/to-tasks` isn't platoon's copy: it is still the skill platoon's tasks come from.
    Kept(PathBuf),
    Failed(String),
}

/// After `install`: take out platoon's unchanged `/to-tickets` copy once platoon's `/to-tasks` copy is in place. An old
/// copy the user changed is theirs and stays; so does platoon's while `/to-tasks` isn't platoon's (the user's own, or
/// not written), so the skill never goes away altogether.
pub fn retire_old(root: &Path) -> Retired {
    let old = old_path(root);
    match old_state(root) {
        Err(why) => return Retired::Failed(why),
        Ok(State::Missing) => return Retired::NotThere,
        Ok(State::NotOurs) => return Retired::NotOurs(old),
        Ok(State::Ours(_)) => {}
    }
    if !matches!(state(root), Ok(State::Ours(_))) {
        return Retired::Kept(old);
    }
    match remove_at(&old) {
        Removal::Removed(p) => Retired::Removed(p),
        Removal::NotThere => Retired::NotThere,
        Removal::NotOurs(p) => Retired::NotOurs(p),
        Removal::Failed(why) => Retired::Failed(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-skill-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn cksum_is_the_posix_commands() {
        assert_eq!(cksum(b""), 4_294_967_295);
        assert_eq!(cksum(b"123456789"), 930_766_865);
        assert_eq!(cksum(b"a"), 1_220_704_766);
    }

    #[test]
    fn the_skill_names_this_platoon_by_its_full_path_and_is_typed_by_the_user() {
        let t = text("/opt/my tools/platoon");
        assert!(!t.contains(PROGRAM), "{t}");
        assert!(t.starts_with("---\nname: to-tasks\n"));
        assert!(t.contains("  - \"Bash('/opt/my tools/platoon' task add:*)\"\n"), "{t}");
        assert!(t.contains("'/opt/my tools/platoon' task squads\n"));
        assert!(t.contains("'/opt/my tools/platoon' task add --folder "));
        assert!(t.contains("$ARGUMENTS"));
        // The description: the user types it; a suggestion at most once, never an interruption.
        let description = t.lines().find(|l| l.starts_with("description: ")).unwrap();
        for want in ["typing /to-tasks", "suggest it once", "Never interrupt"] {
            assert!(description.contains(want), "{want:?}: {description}");
        }
        assert!(description.len() < 1024, "agents cap a skill's description");
        assert!(!t.contains("disable-model-invocation"), "Claude may suggest it, so it must see it");
        // A path YAML's double quotes would end is escaped there, and only there.
        let odd = text("/a\"b\\c/platoon");
        assert!(odd.contains("\"Bash('/a\\\"b\\\\c/platoon' task add:*)\""), "{odd}");
        assert!(odd.contains("\n'/a\"b\\c/platoon' task squads\n"), "{odd}");
        assert!(is_ours(&t) && is_ours(&odd));
    }

    #[test]
    fn its_commands_are_ones_platoon_has() {
        // Every `task <command> --flag` the skill uses is in `platoon task`'s usage.
        let t = text("platoon");
        let usage = crate::tickets::USAGE;
        for flag in ["--folder", "--squad", "--title", "--project"] {
            assert!(t.contains(flag) && usage.contains(flag), "{flag}");
        }
        assert!(usage.contains("platoon task squads") && usage.contains("platoon task add"));
    }

    #[test]
    fn only_a_copy_platoon_wrote_and_nobody_changed_is_platoons() {
        let t = text("/usr/bin/platoon");
        assert!(is_ours(&t));
        assert!(!is_ours(&t.replace("Fewer is better", "Fewer is nicer")), "edited above the last line");
        assert!(!is_ours(&format!("{t}\nmy note\n")), "something added after it");
        assert!(!is_ours(&t[..t.len() - 1]), "the last newline taken away");
        assert!(!is_ours(""));
        assert!(!is_ours(TEMPLATE), "the template has no trailer");
    }

    #[test]
    fn install_adds_updates_its_own_and_leaves_anything_else_alone() {
        let root = tmp("install");
        let p = path(&root);
        assert_eq!(install(&root, "/usr/bin/platoon"), Installed::Added);
        assert_eq!(fs::read_to_string(&p).unwrap(), text("/usr/bin/platoon"));
        assert_eq!(install(&root, "/usr/bin/platoon"), Installed::AlreadyDone);
        // platoon moved (or its text changed): platoon's own copy is rewritten.
        assert_eq!(install(&root, "/opt/platoon"), Installed::Updated);
        assert_eq!(fs::read_to_string(&p).unwrap(), text("/opt/platoon"));
        assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 1, "no temp file left beside it");
        // Changed by the user: theirs now, never touched.
        let mine = fs::read_to_string(&p).unwrap().replace("Fewer is better", "Fewer is nicer");
        fs::write(&p, &mine).unwrap();
        assert_eq!(install(&root, "/opt/platoon"), Installed::NotOurs(p.clone()));
        assert_eq!(fs::read_to_string(&p).unwrap(), mine);
        assert_eq!(remove_at(&path(&root)), Removal::NotOurs(p.clone()));
        assert_eq!(fs::read_to_string(&p).unwrap(), mine);
        // A link is the user's arrangement, even to platoon's own text.
        let elsewhere = root.join("elsewhere.md");
        fs::write(&elsewhere, text("/opt/platoon")).unwrap();
        fs::remove_file(&p).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &p).unwrap();
        assert_eq!(install(&root, "/opt/platoon"), Installed::NotOurs(p.clone()));
        assert_eq!(remove_at(&path(&root)), Removal::NotOurs(p.clone()));
        assert!(fs::symlink_metadata(&p).unwrap().file_type().is_symlink());
    }

    #[test]
    fn remove_takes_out_its_own_copy_and_its_folder_when_empty() {
        let root = tmp("remove");
        let p = path(&root);
        assert_eq!(remove_at(&path(&root)), Removal::NotThere);
        install(&root, "/usr/bin/platoon");
        assert_eq!(remove_at(&path(&root)), Removal::Removed(p.clone()));
        assert!(!p.parent().unwrap().exists() && root.join("skills").is_dir(), "the skills folder itself stays");
        // A file of the user's beside it keeps the folder.
        install(&root, "/usr/bin/platoon");
        fs::write(p.parent().unwrap().join("notes.md"), "mine").unwrap();
        assert_eq!(remove_at(&path(&root)), Removal::Removed(p.clone()));
        assert!(!p.exists() && p.parent().unwrap().join("notes.md").exists());
    }

    /// platoon 1.3's copy of the skill at its old place: the same trailer, its own text.
    fn old_copy(root: &Path) -> String {
        let body = TEMPLATE.replace(COMMAND, OLD_COMMAND).replace("name: to-tasks", "name: to-tickets").replace(PROGRAM, "/old/platoon");
        let text = format!("{body}{}", trailer(&body));
        fs::create_dir_all(old_path(root).parent().unwrap()).unwrap();
        fs::write(old_path(root), &text).unwrap();
        text
    }

    #[test]
    fn retire_old_takes_out_platoons_old_copy_only_once_platoons_new_one_is_in() {
        let root = tmp("retire");
        assert_eq!(retire_old(&root), Retired::NotThere);
        old_copy(&root);
        assert_eq!(old_state(&root).map(|s| matches!(s, State::Ours(_))), Ok(true), "the old trailer is platoon's");
        // Nothing at /to-tasks yet: the old copy is the only skill, kept.
        assert_eq!(retire_old(&root), Retired::Kept(old_path(&root)));
        assert!(old_path(&root).is_file());
        // The user's own /to-tasks: the old copy stays too.
        fs::create_dir_all(path(&root).parent().unwrap()).unwrap();
        fs::write(path(&root), "---\nname: to-tasks\n---\nMine.\n").unwrap();
        assert_eq!(retire_old(&root), Retired::Kept(old_path(&root)));
        fs::remove_file(path(&root)).unwrap();
        // platoon's /to-tasks in: the old copy goes, with its folder; the skills folder stays.
        assert_eq!(install(&root, "/usr/bin/platoon"), Installed::Added);
        assert_eq!(retire_old(&root), Retired::Removed(old_path(&root)));
        assert!(!old_path(&root).parent().unwrap().exists() && path(&root).is_file());
        assert_eq!(retire_old(&root), Retired::NotThere);
        // An old copy the user changed is theirs: kept, whatever is at /to-tasks.
        let mine = old_copy(&root).replace("Fewer is better", "Fewer is nicer");
        fs::write(old_path(&root), &mine).unwrap();
        assert_eq!(retire_old(&root), Retired::NotOurs(old_path(&root)));
        assert_eq!(fs::read_to_string(old_path(&root)).unwrap(), mine);
    }

    #[test]
    fn the_command_named_is_the_one_there_and_either_counts_as_present() {
        let root = tmp("command");
        assert!(!present(&root));
        assert_eq!(command(&root), "/to-tasks", "neither: the one setup adds");
        old_copy(&root);
        assert!(present(&root));
        assert_eq!(command(&root), "/to-tickets", "only the old one: the one that works");
        install(&root, "/usr/bin/platoon");
        assert_eq!(command(&root), "/to-tasks");
        fs::remove_file(old_path(&root)).unwrap();
        assert!(present(&root));
        assert_eq!(command(&root), "/to-tasks");
        // The user's own old one counts as much as platoon's.
        fs::remove_file(path(&root)).unwrap();
        fs::write(old_path(&root), "---\nname: to-tickets\n---\nMine.\n").unwrap();
        assert!(present(&root));
        assert_eq!(command(&root), "/to-tickets");
    }
}
