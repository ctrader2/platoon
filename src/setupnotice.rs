//! The notice that `platoon setup` has something to add or update outside the start-up gate. The gate
//! (`require_setup` in `main.rs`) asks only for what platoon can't start without; what setup puts in beside it (today
//! the `/to-tickets` skill, `skill.rs`) would never reach someone who updated platoon without running setup again. So
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

/// What setup has to do outside the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    /// The `/to-tickets` skill isn't there.
    SkillNew,
    /// platoon's own copy of the skill is not the one this platoon writes (a later text, or another platoon's path).
    SkillUpdate,
}

/// A notice to give: what is due, and the reason as it is remembered once said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub due: Due,
    reason: String,
}

impl Notice {
    /// What the notice says.
    pub fn text(&self) -> &'static str {
        match self.due {
            Due::SkillNew => "New: /to-tickets in Claude Code sessions — run `platoon setup` to add it",
            Due::SkillUpdate => "platoon's /to-tickets skill has an update — run `platoon setup`",
        }
    }

    /// The step it is about: the reason remembered for it is the one said last.
    fn step(&self) -> &'static str {
        match self.due {
            Due::SkillNew | Due::SkillUpdate => "to-tickets",
        }
    }
}

/// What setup has to do outside the gate, from whether Claude Code is in use here (installed and not switched off),
/// what is at the skill's place, and the copy this platoon's setup would write there (`want`). Missing is said once per
/// text of the skill (`skill::version`), out of date once per copy setup would write.
pub fn due(claude: bool, skill: &skill::State, want: &str) -> Option<Notice> {
    if !claude {
        return None;
    }
    match skill {
        skill::State::Missing => Some(Notice { due: Due::SkillNew, reason: format!("missing {}", skill::version()) }),
        skill::State::Ours(have) if have != want => {
            Some(Notice { due: Due::SkillUpdate, reason: format!("update {}", skill::cksum(want.as_bytes())) })
        }
        skill::State::Ours(_) | skill::State::NotOurs => None,
    }
}

/// `due` for this computer: Claude Code's folder `claude_root` (only read when `claude`), and the platoon at `exe`.
/// A skill's place that can't be read is no reason for a notice.
pub fn look(claude: bool, claude_root: &Path, exe: &str) -> Option<Notice> {
    if !claude {
        return None;
    }
    due(true, &skill::state(claude_root).ok()?, &skill::text(exe))
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

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-ut-setupnotice-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_notice_only_when_claude_code_is_in_use_and_the_skill_is_missing_or_platoons_older_copy() {
        let want = skill::text("/usr/bin/platoon");
        let new = due(true, &skill::State::Missing, &want).unwrap();
        assert_eq!(new.due, Due::SkillNew);
        assert_eq!(new.text(), "New: /to-tickets in Claude Code sessions — run `platoon setup` to add it");
        let older = due(true, &skill::State::Ours(skill::text("/opt/old/platoon")), &want).unwrap();
        assert_eq!(older.due, Due::SkillUpdate);
        assert_eq!(older.text(), "platoon's /to-tickets skill has an update — run `platoon setup`");
        assert_eq!(due(true, &skill::State::Ours(want.clone()), &want), None, "up to date");
        assert_eq!(due(true, &skill::State::NotOurs, &want), None, "the user's own: their choice");
        for state in [skill::State::Missing, skill::State::Ours(skill::text("/opt/old/platoon"))] {
            assert_eq!(due(false, &state, &want), None, "no Claude Code here, or switched off");
        }
    }

    #[test]
    fn missing_is_one_reason_per_skill_text_and_an_update_one_per_copy_setup_would_write() {
        let (a, b) = (skill::text("/usr/bin/platoon"), skill::text("/opt/platoon"));
        // Missing doesn't depend on where platoon runs from: the same reason wherever it is.
        assert_eq!(due(true, &skill::State::Missing, &a), due(true, &skill::State::Missing, &b));
        let old = skill::State::Ours(skill::text("/old/platoon"));
        assert_ne!(due(true, &old, &a), due(true, &old, &b), "another copy to write: another reason");
        assert_ne!(due(true, &old, &a).unwrap().reason, due(true, &skill::State::Missing, &a).unwrap().reason);
    }

    #[test]
    fn look_reads_the_skills_place_and_only_where_claude_code_is_in_use() {
        let root = tmp("look");
        assert_eq!(look(true, &root, "/usr/bin/platoon").map(|n| n.due), Some(Due::SkillNew));
        assert_eq!(look(false, &root, "/usr/bin/platoon"), None);
        skill::install(&root, "/usr/bin/platoon");
        assert_eq!(look(true, &root, "/usr/bin/platoon"), None);
        assert_eq!(look(true, &root, "/opt/platoon").map(|n| n.due), Some(Due::SkillUpdate));
    }

    #[test]
    fn a_reason_is_remembered_per_step_and_other_keys_are_kept() {
        let cache = tmp("said");
        let want = skill::text("/usr/bin/platoon");
        let new = due(true, &skill::State::Missing, &want).unwrap();
        let update = due(true, &skill::State::Ours(skill::text("/old")), &want).unwrap();
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
