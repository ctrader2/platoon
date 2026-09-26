//! A project's notes file: the template a new one starts from, the copies kept before each change, undoing the
//! last change and the before/after view of it.
//!
//! The notes are `<config>/projects/<id>/project.md` (see `projects.rs`). Before every change platoon did not make
//! itself - an editor it started, and later the automatic updates - the version that was there is copied into
//! `<config>/projects/<id>/history/`, newest first, at most `MAX_VERSIONS` of them; the oldest goes.
//!
//! Nothing here ever loses what is on disk: a copy is made before a write, a write that can't be made leaves the
//! file as it was, and a history file that can't be read simply means there is nothing to undo.

use crate::projects::write_atomic;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// How many earlier versions of one project's notes are kept.
pub const MAX_VERSIONS: usize = 20;

/// The line at the top of the notes, saying who writes what. platoon replaces this one line (and only this one)
/// in a file that still carries the one it wrote before the shape of the notes became fluid.
pub const PREAMBLE: &str = "\
platoon keeps these up to date: the summary at the top, the sections in between, and dated entries in Decisions. Your own words are always kept.

";

/// What a new `project.md` starts from: that line, then the two headings that are always there. The sections in
/// between are added as the work needs them (`notesupdate.rs`), and anyone may add their own with `e`.
pub const TEMPLATE: &str = "\
platoon keeps these up to date: the summary at the top, the sections in between, and dated entries in Decisions. Your own words are always kept.

# Summary

# Decisions

";

/// Where the earlier versions of a project's notes are kept.
pub fn history_dir(config: &Path, id: &str) -> PathBuf {
    config.join("projects").join(id).join("history")
}

/// The earlier versions, newest first. (Their names start with the time, zero-padded, so name order is time order.)
pub fn versions(config: &Path, id: &str) -> Vec<PathBuf> {
    let mut names: Vec<String> = fs::read_dir(history_dir(config, id))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".md"))
        .collect();
    names.sort_by(|a, b| b.cmp(a));
    names.into_iter().map(|n| history_dir(config, id).join(n)).collect()
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Keep what is in `project.md` now as the newest earlier version, and drop the oldest ones past `MAX_VERSIONS`.
/// False (and nothing kept) when there is no file yet, or when it is the same as the newest version already kept:
/// a copy of a copy says nothing.
pub fn keep_version(config: &Path, id: &str) -> io::Result<bool> {
    let notes = crate::projects::notes_path(config, id);
    let Ok(text) = fs::read_to_string(&notes) else { return Ok(false) };
    let kept = versions(config, id);
    if kept.first().and_then(|p| fs::read_to_string(p).ok()).as_deref() == Some(text.as_str()) {
        return Ok(false);
    }
    let dir = history_dir(config, id);
    fs::create_dir_all(&dir)?;
    // One file per change; a second change in the same millisecond gets the next number.
    let (ms, mut n) = (now_ms(), 0u32);
    let path = loop {
        let p = dir.join(format!("{ms:013}-{n:03}.md"));
        if !p.exists() {
            break p;
        }
        n += 1;
        if n > 999 {
            break dir.join(format!("{ms:013}-999.md"));
        }
    };
    write_atomic(&path, &text)?;
    for old in versions(config, id).into_iter().skip(MAX_VERSIONS) {
        let _ = fs::remove_file(old);
    }
    Ok(true)
}

/// Drop the newest copy when it is the same as the notes themselves: an editor that was opened and changed
/// nothing must not bury the change before it. True when one was dropped.
pub fn forget_unchanged_version(config: &Path, id: &str) -> bool {
    let Some(latest) = versions(config, id).into_iter().next() else { return false };
    let same = fs::read_to_string(&latest).ok() == fs::read_to_string(crate::projects::notes_path(config, id)).ok();
    same && fs::remove_file(&latest).is_ok()
}

/// The last change to the notes: what was there before (the newest version kept) and what is there now.
/// None when nothing has been kept yet, or when the file or that version can't be read.
pub fn last_change(config: &Path, id: &str) -> Option<(String, String)> {
    let before = fs::read_to_string(versions(config, id).first()?).ok()?;
    let after = fs::read_to_string(crate::projects::notes_path(config, id)).unwrap_or_default();
    Some((before, after))
}

/// Put the previous version back. The version being replaced is kept first, so an undo can itself be undone.
/// Nothing is written unless the previous version could be read whole, so a damaged history file never costs the
/// notes that are there now.
pub fn undo(config: &Path, id: &str) -> Result<(), String> {
    let Some(latest) = versions(config, id).into_iter().next() else {
        return Err("There is nothing to undo: platoon hasn't kept an earlier version of these notes yet.".into());
    };
    let before = fs::read_to_string(&latest).map_err(|_| "The earlier version couldn't be read, so nothing was changed.".to_string())?;
    let notes = crate::projects::notes_path(config, id);
    let now = fs::read_to_string(&notes).unwrap_or_default();
    if now == before {
        return Err("There is nothing to undo: the notes are the same as the version before them.".into());
    }
    keep_version(config, id).map_err(|e| format!("Couldn't keep a copy of the notes, so nothing was changed: {e}"))?;
    write_atomic(&notes, &before).map_err(|e| format!("Couldn't write the notes: {e}"))
}

/// Write `text` as the notes, keeping what is there now first. Used for a change platoon makes itself.
pub fn write(config: &Path, id: &str, text: &str) -> io::Result<()> {
    keep_version(config, id)?;
    write_atomic(&crate::projects::notes_path(config, id), text)
}

/// When the file was last changed and how big it is: what "changed on disk" is decided by.
pub type Stamp = Option<(SystemTime, u64)>;

pub fn stamp(config: &Path, id: &str) -> Stamp {
    let m = fs::metadata(crate::projects::notes_path(config, id)).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The file on disk is not the one platoon last looked at. What is on disk wins: platoon re-reads it rather than
/// writing over it.
pub fn changed_on_disk(config: &Path, id: &str, since: Stamp) -> bool {
    stamp(config, id) != since
}

/// One line of the before/after view.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    /// Unchanged, shown for context.
    Same,
    /// It was there before and is gone now.
    Removed,
    /// It is there now and was not before.
    Added,
    /// A stretch of unchanged lines that is left out.
    Gap,
}

impl Mark {
    /// The character in front of the line, so the view reads without any color.
    pub fn sign(self) -> &'static str {
        match self {
            Mark::Same => "  ",
            Mark::Removed => "- ",
            Mark::Added => "+ ",
            Mark::Gap => "  ",
        }
    }
}

/// How many unchanged lines are shown around a change.
const CONTEXT: usize = 2;
/// Past this many changed lines on either side, the middle is shown as "all this went, all that came" rather than
/// line by line: the exact matching is O(n*m) and nobody reads a thousand-line diff anyway.
const LCS_MAX: usize = 400;

/// The lines of `before` and `after` marked as removed, added or unchanged, with long unchanged stretches left out.
pub fn diff(before: &str, after: &str) -> Vec<(Mark, String)> {
    condense(&marked(before, after))
}

fn marked(before: &str, after: &str) -> Vec<(Mark, String)> {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut out: Vec<(Mark, String)> = a[..head].iter().map(|l| (Mark::Same, (*l).to_string())).collect();
    if am.len() <= LCS_MAX && bm.len() <= LCS_MAX {
        out.extend(lcs_diff(am, bm));
    } else {
        out.extend(am.iter().map(|l| (Mark::Removed, (*l).to_string())));
        out.extend(bm.iter().map(|l| (Mark::Added, (*l).to_string())));
    }
    out.extend(a[a.len() - tail..].iter().map(|l| (Mark::Same, (*l).to_string())));
    out
}

/// The usual longest-common-subsequence diff over the lines that differ.
fn lcs_diff(a: &[&str], b: &[&str]) -> Vec<(Mark, String)> {
    let (n, m) = (a.len(), b.len());
    // table[i][j] = length of the longest common subsequence of a[i..] and b[j..]
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if a[i] == b[j] { table[i + 1][j + 1] + 1 } else { table[i + 1][j].max(table[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((Mark::Same, a[i].to_string()));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            out.push((Mark::Removed, a[i].to_string()));
            i += 1;
        } else {
            out.push((Mark::Added, b[j].to_string()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| (Mark::Removed, (*l).to_string())));
    out.extend(b[j..].iter().map(|l| (Mark::Added, (*l).to_string())));
    out
}

/// Leave out stretches of unchanged lines more than `CONTEXT` away from a change.
fn condense(lines: &[(Mark, String)]) -> Vec<(Mark, String)> {
    let near: Vec<bool> = (0..lines.len())
        .map(|i| {
            let from = i.saturating_sub(CONTEXT);
            let to = (i + CONTEXT).min(lines.len().saturating_sub(1));
            (from..=to).any(|k| lines[k].0 != Mark::Same)
        })
        .collect();
    let mut out: Vec<(Mark, String)> = Vec::new();
    let mut skipped = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if near[i] {
            if skipped > 0 {
                out.push((Mark::Gap, format!("… {skipped} unchanged line{} …", if skipped == 1 { "" } else { "s" })));
                skipped = 0;
            }
            out.push(line.clone());
        } else {
            skipped += 1;
        }
    }
    if skipped > 0 && !out.is_empty() {
        out.push((Mark::Gap, format!("… {skipped} unchanged line{} …", if skipped == 1 { "" } else { "s" })));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::notes_path;

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-notes-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("projects/p1")).unwrap();
        d
    }

    fn set(config: &Path, text: &str) {
        write_atomic(&notes_path(config, "p1"), text).unwrap();
    }

    fn now(config: &Path) -> String {
        fs::read_to_string(notes_path(config, "p1")).unwrap_or_default()
    }

    #[test]
    fn a_new_file_starts_from_the_template_with_the_two_fixed_headings_and_who_writes_what() {
        let mut at = 0;
        for heading in ["# Summary", "# Decisions"] {
            let found = TEMPLATE[at..].find(heading).unwrap_or_else(|| panic!("{heading} missing from the template:\n{TEMPLATE}"));
            at += found + heading.len();
        }
        assert!(TEMPLATE.starts_with(PREAMBLE), "the template starts with the line that says who writes what");
        let first = TEMPLATE.lines().next().unwrap();
        assert!(first.contains("summary at the top") && first.contains("Decisions") && first.contains("Your own words"), "{first}");
        let lower = TEMPLATE.to_lowercase();
        for jargon in ["markdown", "prompt", "token", "llm", "context window"] {
            assert!(!lower.contains(jargon), "{jargon:?} in the template");
        }
    }

    #[test]
    fn a_copy_is_kept_before_each_change_newest_first_and_only_twenty_of_them() {
        let d = dir("history");
        assert!(!keep_version(&d, "p1").unwrap(), "no notes yet: nothing to keep");
        set(&d, "one\n");
        assert!(keep_version(&d, "p1").unwrap());
        assert!(!keep_version(&d, "p1").unwrap(), "the same text again is not kept twice");
        for n in 2..=25 {
            set(&d, &format!("{n}\n"));
            assert!(keep_version(&d, "p1").unwrap());
        }
        let kept = versions(&d, "p1");
        assert_eq!(kept.len(), MAX_VERSIONS, "the oldest are dropped");
        let texts: Vec<String> = kept.iter().map(|p| fs::read_to_string(p).unwrap()).collect();
        assert_eq!(texts[0], "25\n", "newest first");
        assert_eq!(texts[1], "24\n");
        assert_eq!(texts[MAX_VERSIONS - 1], "6\n");
    }

    #[test]
    fn undo_puts_the_previous_version_back_and_can_itself_be_undone() {
        let d = dir("undo");
        assert!(undo(&d, "p1").unwrap_err().contains("nothing to undo"));
        set(&d, "first\n");
        keep_version(&d, "p1").unwrap();
        set(&d, "second\n"); // as an editor would
        assert_eq!(last_change(&d, "p1"), Some(("first\n".into(), "second\n".into())));
        undo(&d, "p1").unwrap();
        assert_eq!(now(&d), "first\n");
        undo(&d, "p1").unwrap();
        assert_eq!(now(&d), "second\n", "undoing the undo brings it back");
        undo(&d, "p1").unwrap();
        assert_eq!(now(&d), "first\n");
        // Nothing changed since the last copy: there is nothing to undo, and nothing is written.
        set(&d, "first\n");
        keep_version(&d, "p1").unwrap();
        let before = versions(&d, "p1").len();
        assert!(undo(&d, "p1").unwrap_err().contains("nothing to undo"));
        assert_eq!((now(&d).as_str(), versions(&d, "p1").len()), ("first\n", before));
    }

    #[test]
    fn a_history_file_that_cannot_be_read_never_costs_the_notes_that_are_there() {
        let d = dir("corrupt");
        set(&d, "real notes\n");
        keep_version(&d, "p1").unwrap();
        set(&d, "edited notes\n");
        // The newest version is unreadable: a folder where a file should be.
        let latest = versions(&d, "p1").into_iter().next().unwrap();
        fs::remove_file(&latest).unwrap();
        fs::create_dir(&latest).unwrap();
        assert!(undo(&d, "p1").unwrap_err().contains("couldn't be read"));
        assert_eq!(now(&d), "edited notes\n", "the notes are untouched");
        assert_eq!(last_change(&d, "p1"), None, "and the before/after view says there is nothing to show");
        // Bytes that are not text are treated the same way.
        fs::remove_dir(&latest).unwrap();
        fs::write(&latest, [0xff, 0xfe, 0x00]).unwrap();
        assert!(undo(&d, "p1").is_err());
        assert_eq!(now(&d), "edited notes\n");
    }

    #[test]
    fn the_file_on_disk_is_what_counts_a_change_made_elsewhere_is_noticed() {
        let d = dir("stamp");
        assert_eq!(stamp(&d, "p1"), None);
        assert!(changed_on_disk(&d, "p1", Some((SystemTime::UNIX_EPOCH, 0))), "no file where one was expected");
        set(&d, "mine\n");
        let seen = stamp(&d, "p1");
        assert!(seen.is_some() && !changed_on_disk(&d, "p1", seen));
        set(&d, "somebody else's, longer\n");
        assert!(changed_on_disk(&d, "p1", seen));
        // And what is read back is what is on disk, not what platoon wrote last.
        assert_eq!(now(&d), "somebody else's, longer\n");
    }

    #[test]
    fn the_before_and_after_view_marks_what_went_and_what_came_and_reads_without_color() {
        let before = "# Overview\nSell socks.\n# Status\nStarted.\n";
        let after = "# Overview\nSell socks and hats.\n# Status\nStarted.\n";
        let d = diff(before, after);
        let text: Vec<String> = d.iter().map(|(m, l)| format!("{}{l}", m.sign())).collect();
        assert_eq!(
            text,
            ["  # Overview", "- Sell socks.", "+ Sell socks and hats.", "  # Status", "  Started."],
            "removed lines are marked -, added ones +"
        );
        assert_eq!(d.iter().filter(|(m, _)| *m == Mark::Removed).count(), 1);
        assert_eq!(d.iter().filter(|(m, _)| *m == Mark::Added).count(), 1);
        // Lines that only moved apart are not shown as changes, and a long unchanged stretch is left out.
        let big_before: String = (0..50).map(|n| format!("line {n}\n")).collect();
        let big_after = big_before.replace("line 25", "line twenty-five");
        let d = diff(&big_before, &big_after);
        assert!(d.len() < 12, "only the change and a little around it: {d:?}");
        assert!(d.iter().any(|(m, l)| *m == Mark::Gap && l.contains("unchanged lines")), "{d:?}");
        assert!(d.iter().any(|(m, l)| *m == Mark::Removed && l == "line 25"));
        assert!(d.iter().any(|(m, l)| *m == Mark::Added && l == "line twenty-five"));
        // Nothing changed at all.
        assert!(diff("same\n", "same\n").iter().all(|(m, _)| *m != Mark::Removed && *m != Mark::Added));
        // A whole new file, and a file emptied.
        assert!(diff("", "new\n").iter().all(|(m, _)| *m == Mark::Added));
        assert!(diff("gone\n", "").iter().all(|(m, _)| *m == Mark::Removed));
    }

    #[test]
    fn writing_the_notes_ourselves_keeps_the_version_that_was_there() {
        let d = dir("write");
        write(&d, "p1", TEMPLATE).unwrap();
        assert_eq!(now(&d), TEMPLATE);
        assert!(versions(&d, "p1").is_empty(), "there was nothing to keep");
        write(&d, "p1", "changed\n").unwrap();
        assert_eq!(versions(&d, "p1").len(), 1);
        assert_eq!(fs::read_to_string(&versions(&d, "p1")[0]).unwrap(), TEMPLATE);
    }
}
