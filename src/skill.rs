//! The `/hand-to-squad` skill platoon ships for Claude Code: typed in a session, it turns the work agreed there into
//! platoon tasks for a squad (`src/skills/hand-to-squad/SKILL.md`, compiled in). `platoon setup` writes it to
//! `<claude root>/skills/hand-to-squad/SKILL.md` (`install`), never as part of the start-up gate, and `platoon
//! uninstall` takes it out again (`remove_at`).
//!
//! It had other names before (`OLD_COMMANDS`): `/to-tasks` in platoon 1.4, `/to-tickets` through 1.3, each in its own
//! folder (`old_path`). Setup renames platoon's copy: once platoon's `/hand-to-squad` is in place, platoon's unchanged
//! copy under each old name is deleted (`retire_old`); an old copy the user changed is kept. Until setup runs, an old
//! one keeps working, so whatever tells the user to type the skill names the one that is there (`command`), and any of
//! the names counts as installed (`present`).
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
const TEMPLATE: &str = include_str!("skills/hand-to-squad/SKILL.md");

/// Where the template names platoon.
const PROGRAM: &str = "{{platoon}}";

/// How the last line of every copy platoon writes starts; it ends ` cksum <N> -->`. `scripts/uninstall.sh` reads the
/// same line: change both together.
const TRAILER_START: &str = "<!-- platoon setup wrote this file and keeps it up to date; once you change it, platoon leaves it alone.";

/// The command the user types for the skill.
pub const COMMAND: &str = "/hand-to-squad";

/// What the skill was called before, newest first: `/to-tasks` in platoon 1.4, `/to-tickets` through 1.3. Still what is
/// typed where setup hasn't run since.
pub const OLD_COMMANDS: [&str; 2] = ["/to-tasks", "/to-tickets"];

/// The skill's file under Claude Code's folder `root`.
pub fn path(root: &Path) -> PathBuf {
    place(root, COMMAND)
}

/// Where an earlier platoon wrote the skill under its old name `old` (one of `OLD_COMMANDS`).
pub fn old_path(root: &Path, old: &str) -> PathBuf {
    place(root, old)
}

/// The file of the skill typed as `command` (its folder is the name without the slash).
fn place(root: &Path, command: &str) -> PathBuf {
    root.join("skills").join(command.trim_start_matches('/')).join("SKILL.md")
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

/// What is at the skill's place under its old name `old` under `root`, judged as `state` judges the current one
/// (platoon's copies have always ended in the same trailer).
pub fn old_state(root: &Path, old: &str) -> Result<State, String> {
    state_at(&old_path(root, old))
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

/// Is the skill there for Claude Code to run, under any of its names, platoon's copy or the user's own?
pub fn present(root: &Path) -> bool {
    there(&state(root)) || OLD_COMMANDS.iter().any(|old| there(&old_state(root, old)))
}

/// The command to tell the user to type: `/hand-to-squad`, or while only old names are there (platoon updated, setup
/// not run since), the newest of those. With none, `/hand-to-squad`, the one setup adds.
pub fn command(root: &Path) -> &'static str {
    if there(&state(root)) {
        return COMMAND;
    }
    OLD_COMMANDS.into_iter().find(|old| there(&old_state(root, old))).unwrap_or(COMMAND)
}

/// The newest old name platoon's own unchanged copy is under (one setup would rename), if any. One that can't be read
/// counts as not there.
pub fn platoons_old(root: &Path) -> Option<&'static str> {
    OLD_COMMANDS.into_iter().find(|old| matches!(old_state(root, old), Ok(State::Ours(_))))
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

/// Take platoon's copy of the skill at `p` (`path` or an `old_path` of Claude Code's folder) out, and its folder when
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

/// What `retire_old` did with the copy under one old name.
#[derive(Debug, PartialEq, Eq)]
pub enum Retired {
    NotThere,
    /// platoon's copy, unchanged, deleted now that platoon's `/hand-to-squad` is in place (and its folder, when empty).
    Removed(PathBuf),
    /// Changed since platoon wrote it, or never platoon's: kept.
    NotOurs(PathBuf),
    /// platoon's copy, kept because `/hand-to-squad` isn't platoon's copy: it is still the skill platoon's tasks come
    /// from.
    Kept(PathBuf),
    Failed(String),
}

/// After `install`: take out platoon's unchanged copy under each old name (`OLD_COMMANDS`, in that order) once
/// platoon's `/hand-to-squad` copy is in place, each on its own. An old copy the user changed is theirs and stays; so
/// does platoon's while `/hand-to-squad` isn't platoon's (the user's own, or not written), so the skill never goes away
/// altogether.
pub fn retire_old(root: &Path) -> Vec<(&'static str, Retired)> {
    let new_is_ours = matches!(state(root), Ok(State::Ours(_)));
    OLD_COMMANDS.into_iter().map(|old| (old, retire(root, old, new_is_ours))).collect()
}

fn retire(root: &Path, old: &str, new_is_ours: bool) -> Retired {
    let p = old_path(root, old);
    match old_state(root, old) {
        Err(why) => return Retired::Failed(why),
        Ok(State::Missing) => return Retired::NotThere,
        Ok(State::NotOurs) => return Retired::NotOurs(p),
        Ok(State::Ours(_)) => {}
    }
    if !new_is_ours {
        return Retired::Kept(p);
    }
    match remove_at(&p) {
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
        assert!(t.starts_with("---\nname: hand-to-squad\n"));
        assert!(t.contains("  - \"Bash('/opt/my tools/platoon' task add:*)\"\n"), "{t}");
        assert!(t.contains("'/opt/my tools/platoon' task squads\n"));
        assert!(t.contains("'/opt/my tools/platoon' task add --folder "));
        assert!(t.contains("$ARGUMENTS"));
        // The description: the user types it; a suggestion at most once, never an interruption.
        let description = t.lines().find(|l| l.starts_with("description: ")).unwrap();
        for want in ["typing /hand-to-squad", "suggest it once", "Never interrupt", "/hand-to-squad sends this work to a platoon squad."] {
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

    /// An earlier platoon's copy of the skill under its old name `old`: the same trailer, its own text.
    fn old_copy(root: &Path, old: &str) -> String {
        let name = old.trim_start_matches('/');
        let body = TEMPLATE.replace(COMMAND, old).replace("name: hand-to-squad", &format!("name: {name}")).replace(PROGRAM, "/old/platoon");
        let text = format!("{body}{}", trailer(&body));
        fs::create_dir_all(old_path(root, old).parent().unwrap()).unwrap();
        fs::write(old_path(root, old), &text).unwrap();
        text
    }

    #[test]
    fn the_old_names_are_the_14_and_13_ones_newest_first_each_in_its_own_folder() {
        let root = Path::new("/home/u/.claude");
        assert_eq!(OLD_COMMANDS, ["/to-tasks", "/to-tickets"]);
        assert_eq!(path(root), Path::new("/home/u/.claude/skills/hand-to-squad/SKILL.md"));
        assert_eq!(old_path(root, "/to-tasks"), Path::new("/home/u/.claude/skills/to-tasks/SKILL.md"));
        assert_eq!(old_path(root, "/to-tickets"), Path::new("/home/u/.claude/skills/to-tickets/SKILL.md"));
    }

    #[test]
    fn retire_old_takes_out_platoons_old_copies_only_once_platoons_new_one_is_in() {
        let root = tmp("retire");
        let (tasks, tickets) = (old_path(&root, "/to-tasks"), old_path(&root, "/to-tickets"));
        assert_eq!(retire_old(&root), [("/to-tasks", Retired::NotThere), ("/to-tickets", Retired::NotThere)]);
        // A 1.4 world (platoon's /to-tasks) and a 1.3 one's leftover (platoon's /to-tickets), both at once.
        old_copy(&root, "/to-tasks");
        old_copy(&root, "/to-tickets");
        assert_eq!(old_state(&root, "/to-tasks").map(|s| matches!(s, State::Ours(_))), Ok(true), "the old trailer is platoon's");
        assert_eq!(platoons_old(&root), Some("/to-tasks"), "the newest");
        // Nothing at /hand-to-squad yet: the old copies are the only skill, kept.
        assert_eq!(retire_old(&root), [("/to-tasks", Retired::Kept(tasks.clone())), ("/to-tickets", Retired::Kept(tickets.clone()))]);
        assert!(tasks.is_file() && tickets.is_file());
        // The user's own /hand-to-squad: the old copies stay too.
        fs::create_dir_all(path(&root).parent().unwrap()).unwrap();
        fs::write(path(&root), "---\nname: hand-to-squad\n---\nMine.\n").unwrap();
        assert_eq!(retire_old(&root), [("/to-tasks", Retired::Kept(tasks.clone())), ("/to-tickets", Retired::Kept(tickets.clone()))]);
        fs::remove_file(path(&root)).unwrap();
        // platoon's /hand-to-squad in: both old copies go, with their folders; the skills folder stays.
        assert_eq!(install(&root, "/usr/bin/platoon"), Installed::Added);
        assert_eq!(retire_old(&root), [("/to-tasks", Retired::Removed(tasks.clone())), ("/to-tickets", Retired::Removed(tickets.clone()))]);
        assert!(!tasks.parent().unwrap().exists() && !tickets.parent().unwrap().exists() && path(&root).is_file());
        assert_eq!(retire_old(&root), [("/to-tasks", Retired::NotThere), ("/to-tickets", Retired::NotThere)]);
        assert_eq!(platoons_old(&root), None);
        // Each on its own: an old copy the user changed is theirs, kept whatever is at /hand-to-squad; platoon's other goes.
        let mine = old_copy(&root, "/to-tickets").replace("Fewer is better", "Fewer is nicer");
        fs::write(&tickets, &mine).unwrap();
        old_copy(&root, "/to-tasks");
        assert_eq!(platoons_old(&root), Some("/to-tasks"));
        assert_eq!(retire_old(&root), [("/to-tasks", Retired::Removed(tasks.clone())), ("/to-tickets", Retired::NotOurs(tickets.clone()))]);
        assert_eq!(fs::read_to_string(&tickets).unwrap(), mine);
        assert_eq!(platoons_old(&root), None, "the user's copy is no rename");
        // And the other way round.
        fs::remove_file(&tickets).unwrap();
        let mine = old_copy(&root, "/to-tasks").replace("Fewer is better", "Fewer is nicer");
        fs::write(&tasks, &mine).unwrap();
        old_copy(&root, "/to-tickets");
        assert_eq!(platoons_old(&root), Some("/to-tickets"), "platoon's is under the older name only");
        assert_eq!(retire_old(&root), [("/to-tasks", Retired::NotOurs(tasks.clone())), ("/to-tickets", Retired::Removed(tickets.clone()))]);
        assert_eq!(fs::read_to_string(&tasks).unwrap(), mine);
    }

    #[test]
    fn the_command_named_is_the_one_there_and_any_name_counts_as_present() {
        let root = tmp("command");
        assert!(!present(&root));
        assert_eq!(command(&root), "/hand-to-squad", "none: the one setup adds");
        old_copy(&root, "/to-tickets");
        assert!(present(&root));
        assert_eq!(command(&root), "/to-tickets", "only the 1.3 one: the one that works");
        old_copy(&root, "/to-tasks");
        assert_eq!(command(&root), "/to-tasks", "both old ones: the newer");
        fs::remove_file(old_path(&root, "/to-tickets")).unwrap();
        assert!(present(&root));
        assert_eq!(command(&root), "/to-tasks", "only the 1.4 one");
        install(&root, "/usr/bin/platoon");
        assert_eq!(command(&root), "/hand-to-squad");
        fs::remove_file(old_path(&root, "/to-tasks")).unwrap();
        assert!(present(&root));
        assert_eq!(command(&root), "/hand-to-squad");
        // The user's own old ones count as much as platoon's.
        fs::remove_file(path(&root)).unwrap();
        assert!(!present(&root));
        for old in OLD_COMMANDS {
            fs::create_dir_all(old_path(&root, old).parent().unwrap()).unwrap();
            fs::write(old_path(&root, old), "---\nname: mine\n---\nMine.\n").unwrap();
            assert!(present(&root));
            assert_eq!(command(&root), old);
            assert_eq!(platoons_old(&root), None, "not platoon's: no rename");
            fs::remove_file(old_path(&root, old)).unwrap();
        }
    }
}
