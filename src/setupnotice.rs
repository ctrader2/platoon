//! The notice that `platoon setup` has something to add or update outside the start-up gate. The gate
//! (`require_setup` in `main.rs`) asks only for what platoon can't start without; what setup puts in beside it (today
//! the `/hand-to-squad` skill, `skill.rs`) would never reach someone who updated platoon without running setup again. So
//! the app says so, once, where the version notice shows (`App::news_showing`).
//!
//! Each thing setup has to do there has a reason (`Due`), and a reason is said once: remembered in
//! `<cache>/setup-notice.json` once it has shown, per step. A new reason (platoon's copy out of date after "missing"
//! was said, or a later platoon's skill) is said once again. Nothing is said for a program that isn't installed or is
//! switched off, nor about a file platoon didn't write (keeping it is the user's choice). A later step outside the gate
//! is one more `Due`.

use crate::skill;
use serde_json::{Map, Value, json};
use std::path::Path;

const STATE_FILE: &str = "setup-notice.json";

/// What setup has to do outside the gate, each named after its step first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum Due {
    /// The `/hand-to-squad` skill isn't there.
    SkillNew,
    /// platoon's copy is there only under an old name of the skill (`skill::OLD_COMMANDS`; the newest such one):
    /// setup renames it.
    SkillRenamed(&'static str),
    /// platoon's own copy of the skill is not the one this platoon writes (a later text, or another platoon's path).
    SkillUpdate,
}

/// A notice to give: what is due, and the reason as it is remembered once said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub due: Due,
    reason: String,
    text: String,
}

impl Notice {
    fn new(due: Due, reason: String) -> Notice {
        let text = match due {
            Due::SkillNew => format!("New: {} in Claude Code sessions — run `platoon setup` to add it", skill::COMMAND),
            Due::SkillRenamed(old) => format!("{old} is now called {} — run `platoon setup` to rename it", skill::COMMAND),
            Due::SkillUpdate => format!("platoon's {} skill has an update — run `platoon setup`", skill::COMMAND),
        };
        Notice { due, reason, text }
    }

    /// What the notice says.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The step it is about: the reason remembered for it is the one said last. (The skill's step keeps the key it had
    /// under its first name, so what was said before is remembered across the renames; each reason names the skill's
    /// text, and a rename's its old name too, so a reason said by an earlier platoon is never this one's.)
    fn step(&self) -> &'static str {
        match self.due {
            Due::SkillNew | Due::SkillRenamed(_) | Due::SkillUpdate => "to-tickets",
        }
    }
}

/// What setup has to do outside the gate, from whether Claude Code is in use here (installed and not switched off),
/// what is at the skill's place, the old name platoon's own copy is under when there is one (`old`:
/// `skill::platoons_old`), and the copy this platoon's setup would write there (`want`). Missing is said once per text
/// of the skill (`skill::version`), and so is the rename of platoon's copy under each old name; out of date once per
/// copy setup would write. An old copy that isn't platoon's (one the user changed) is no rename: setup keeps it, so the
/// skill is just missing.
pub fn due(claude: bool, skill: &skill::State, old: Option<&'static str>, want: &str) -> Option<Notice> {
    if !claude {
        return None;
    }
    match (skill, old) {
        (skill::State::Missing, Some(old)) => Some(Notice::new(Due::SkillRenamed(old), format!("renamed {old} {}", skill::version()))),
        (skill::State::Missing, None) => Some(Notice::new(Due::SkillNew, format!("missing {}", skill::version()))),
        (skill::State::Ours(have), _) if have != want => {
            Some(Notice::new(Due::SkillUpdate, format!("update {}", skill::cksum(want.as_bytes()))))
        }
        (skill::State::Ours(_) | skill::State::NotOurs, _) => None,
    }
}

/// `due` for this computer: Claude Code's folder `claude_root` (only read when `claude`), and the platoon at `exe`.
/// A skill's place that can't be read is no reason for a notice; an old place that can't be read is taken as empty.
pub fn look(claude: bool, claude_root: &Path, exe: &str) -> Option<Notice> {
    if !claude {
        return None;
    }
    due(true, &skill::state(claude_root).ok()?, skill::platoons_old(claude_root), &skill::text(exe))
}

/// The steps' reasons said before, as `<cache>/setup-notice.json` has them (`{"said": {step: reason}}`).
fn load(cache: &Path) -> Map<String, Value> {
    match std::fs::read_to_string(cache.join(STATE_FILE)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(o)) => o,
        _ => Map::new(),
    }
}

/// `n`'s reason was said before.
pub fn said(cache: &Path, n: &Notice) -> bool {
    load(cache).get("said").and_then(|s| s.get(n.step())).and_then(Value::as_str) == Some(n.reason.as_str())
}

/// Remember `n`'s reason as said. Other keys in the file are kept.
pub fn remember(cache: &Path, n: &Notice) -> std::io::Result<()> {
    let mut o = load(cache);
    let mut said = match o.remove("said") {
        Some(Value::Object(s)) => s,
        _ => Map::new(),
    };
    said.insert(n.step().into(), json!(n.reason));
    o.insert("said".into(), Value::Object(said));
    crate::projects::write_atomic(&cache.join(STATE_FILE), &serde_json::to_string_pretty(&Value::Object(o)).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    /// platoon's copy under none of the skill's old names.
    const NONE: Option<&str> = None;

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-ut-setupnotice-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_notice_only_when_claude_code_is_in_use_and_the_skill_is_missing_or_platoons_older_copy() {
        let want = skill::text("/usr/bin/platoon");
        let new = due(true, &skill::State::Missing, NONE, &want).unwrap();
        assert_eq!(new.due, Due::SkillNew);
        assert_eq!(new.text(), "New: /hand-to-squad in Claude Code sessions — run `platoon setup` to add it");
        let older = due(true, &skill::State::Ours(skill::text("/opt/old/platoon")), NONE, &want).unwrap();
        assert_eq!(older.due, Due::SkillUpdate);
        assert_eq!(older.text(), "platoon's /hand-to-squad skill has an update — run `platoon setup`");
        assert_eq!(due(true, &skill::State::Ours(want.clone()), NONE, &want), None, "up to date");
        assert_eq!(due(true, &skill::State::NotOurs, NONE, &want), None, "the user's own: their choice");
        for state in [skill::State::Missing, skill::State::Ours(skill::text("/opt/old/platoon"))] {
            assert_eq!(due(false, &state, NONE, &want), None, "no Claude Code here, or switched off");
        }
    }

    #[test]
    fn platoons_copy_under_an_old_name_is_a_rename_named_after_it() {
        let want = skill::text("/usr/bin/platoon");
        for old in ["/to-tasks", "/to-tickets"] {
            let renamed = due(true, &skill::State::Missing, Some(old), &want).unwrap();
            assert_eq!(renamed.due, Due::SkillRenamed(old));
            assert_eq!(renamed.text(), format!("{old} is now called /hand-to-squad — run `platoon setup` to rename it"));
            assert_ne!(renamed.reason, due(true, &skill::State::Missing, NONE, &want).unwrap().reason, "said once again after \"new\"");
            // /hand-to-squad in already: whatever is at the old place is no reason.
            assert_eq!(due(true, &skill::State::Ours(want.clone()), Some(old), &want), None);
            assert_eq!(due(true, &skill::State::NotOurs, Some(old), &want), None);
            assert_eq!(due(false, &skill::State::Missing, Some(old), &want), None);
        }
        let reason = |old| due(true, &skill::State::Missing, Some(old), &want).unwrap().reason;
        assert_ne!(reason("/to-tasks"), reason("/to-tickets"), "each old name its own reason");
    }

    #[test]
    fn what_platoon_14_said_never_swallows_this_rename() {
        // As platoon 1.4 remembered its notices: under the same step, a reason naming its own skill's text (the cksum of
        // 1.4's src/skills/to-tasks/SKILL.md).
        const V14: u32 = 1_741_090_795;
        let want = skill::text("/usr/bin/platoon");
        for said_by_14 in [format!("renamed {V14}"), format!("missing {V14}")] {
            let cache = tmp("said-by-14");
            fs::write(cache.join(STATE_FILE), json!({"said": {"to-tickets": said_by_14}}).to_string()).unwrap();
            // A 1.3 user told "/to-tickets is now called /to-tasks" who never ran setup, and a 1.4 user with /to-tasks.
            for old in ["/to-tickets", "/to-tasks"] {
                let n = due(true, &skill::State::Missing, Some(old), &want).unwrap();
                assert!(!said(&cache, &n), "{said_by_14:?}: {}", n.text());
            }
            assert!(!said(&cache, &due(true, &skill::State::Missing, NONE, &want).unwrap()), "{said_by_14:?}: new");
        }
    }

    #[test]
    fn missing_is_one_reason_per_skill_text_and_an_update_one_per_copy_setup_would_write() {
        let (a, b) = (skill::text("/usr/bin/platoon"), skill::text("/opt/platoon"));
        // Missing doesn't depend on where platoon runs from: the same reason wherever it is.
        assert_eq!(due(true, &skill::State::Missing, NONE, &a), due(true, &skill::State::Missing, NONE, &b));
        let old = skill::State::Ours(skill::text("/old/platoon"));
        assert_ne!(due(true, &old, NONE, &a), due(true, &old, NONE, &b), "another copy to write: another reason");
        assert_ne!(due(true, &old, NONE, &a).unwrap().reason, due(true, &skill::State::Missing, NONE, &a).unwrap().reason);
    }

    #[test]
    fn look_reads_the_skills_place_and_only_where_claude_code_is_in_use() {
        let root = tmp("look");
        assert_eq!(look(true, &root, "/usr/bin/platoon").map(|n| n.due), Some(Due::SkillNew));
        assert_eq!(look(false, &root, "/usr/bin/platoon"), None);
        // An earlier platoon's copy, at an old place of the skill: a rename, from the newest old name platoon's copy is
        // under.
        for old in ["/to-tickets", "/to-tasks"] {
            let at = skill::old_path(&root, old);
            fs::create_dir_all(at.parent().unwrap()).unwrap();
            skill::install(&root, "/opt/old/platoon");
            fs::rename(skill::path(&root), &at).unwrap();
            assert_eq!(look(true, &root, "/usr/bin/platoon").map(|n| n.due), Some(Due::SkillRenamed(old)));
        }
        // The newer one the user's own: the older one, platoon's, is the rename.
        fs::write(skill::old_path(&root, "/to-tasks"), "---\nname: to-tasks\n---\nMine.\n").unwrap();
        assert_eq!(look(true, &root, "/usr/bin/platoon").map(|n| n.due), Some(Due::SkillRenamed("/to-tickets")));
        // Both the user's: no rename, setup keeps them, so the skill is just new.
        fs::write(skill::old_path(&root, "/to-tickets"), "---\nname: to-tickets\n---\nMine.\n").unwrap();
        assert_eq!(look(true, &root, "/usr/bin/platoon").map(|n| n.due), Some(Due::SkillNew));
        skill::install(&root, "/usr/bin/platoon");
        assert_eq!(look(true, &root, "/usr/bin/platoon"), None);
        assert_eq!(look(true, &root, "/opt/platoon").map(|n| n.due), Some(Due::SkillUpdate));
    }

    #[test]
    fn a_reason_is_remembered_per_step_and_other_keys_are_kept() {
        let cache = tmp("said");
        let want = skill::text("/usr/bin/platoon");
        let new = due(true, &skill::State::Missing, NONE, &want).unwrap();
        let update = due(true, &skill::State::Ours(skill::text("/old")), NONE, &want).unwrap();
        assert!(!said(&cache, &new));
        fs::write(cache.join(STATE_FILE), r#"{"later": 1}"#).unwrap();
        remember(&cache, &new).unwrap();
        assert!(said(&cache, &new) && !said(&cache, &update));
        remember(&cache, &update).unwrap();
        assert!(said(&cache, &update) && !said(&cache, &new), "the step's newest reason replaces the one before");
        let v: Value = serde_json::from_str(&fs::read_to_string(cache.join(STATE_FILE)).unwrap()).unwrap();
        assert_eq!(v["later"], json!(1));
        // A file that can't be read is taken as nothing said, and replaced.
        fs::write(cache.join(STATE_FILE), "not json").unwrap();
        assert!(!said(&cache, &new));
        remember(&cache, &new).unwrap();
        assert!(said(&cache, &new));
    }
}
