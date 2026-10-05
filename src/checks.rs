//! The test check platoon runs itself. A project may name a **test command** (`test_command` in `projects.json`, see
//! `projects.rs`): when a squad's leader stops at the end of the work, platoon runs it where the squad works and tells
//! the squad when it fails, so a squad's "done" is checked by something other than the squad.
//!
//! Here, the part that needs no app: the suggestion on the project's screen (the first command of the "Build and test"
//! block in the `CLAUDE.md` of the project's first git folder; nothing is ever run from a suggestion).

use std::path::{Path, PathBuf};

// ---- the suggestion -----------------------------------------------------------------------------------------------

/// The words of the heading whose block the suggestion comes from (any level, any case).
const SECTION: &str = "build and test";

/// Lines of a block that set the shell up rather than test anything: passed over for the first command.
const SETUP_WORDS: [&str; 8] = ["export", "cd", "source", ".", "set", "unset", "alias", "pushd"];

/// The `CLAUDE.md` that speaks for `folder`: the folder's own, else the one at the top of its git repository (the first
/// folder above it with a `.git`). Looked up on disk only: no process.
pub fn claude_md_for(folder: &Path) -> Option<PathBuf> {
    let own = folder.join("CLAUDE.md");
    if own.is_file() {
        return Some(own);
    }
    let top = folder.ancestors().find(|d| d.join(".git").exists())?;
    Some(top.join("CLAUDE.md")).filter(|p| p.is_file())
}

/// The heading level of a Markdown heading line (`## Build and test` is 2), with its text.
fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    let level = t.chars().take_while(|c| *c == '#').count();
    let rest = &t[level..];
    ((1..=6).contains(&level) && (rest.is_empty() || rest.starts_with(' '))).then(|| (level, rest.trim()))
}

/// A line that opens or closes a fenced block: its fence (the run of three or more backticks or tildes).
fn fence(line: &str) -> Option<String> {
    let t = line.trim_start();
    let c = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let run: String = t.chars().take_while(|x| *x == c).collect();
    (run.chars().count() >= 3).then_some(run)
}

/// `line` without a shell comment at its end (`cargo test   # ~40s` is `cargo test`): a `#` after a space, outside
/// quotes.
fn without_comment(line: &str) -> &str {
    let (mut single, mut double, mut prev_space) = (false, false, true);
    for (i, c) in line.char_indices() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '#' if !single && !double && prev_space => return line[..i].trim_end(),
            _ => {}
        }
        prev_space = c.is_whitespace();
    }
    line
}

/// The command a project's `CLAUDE.md` gives for testing: in the first section whose heading says "Build and test",
/// the first fenced block's first command (a `$ ` prompt taken off, comments and lines that only set the shell up,
/// such as `export PATH=...` or `cd`, passed over). None when there is no such section, block or command.
pub fn suggested_command(text: &str) -> Option<String> {
    let mut lines = text.lines();
    // The section.
    let level = loop {
        let line = lines.next()?;
        if let Some((level, title)) = heading(line)
            && title.to_lowercase().contains(SECTION)
        {
            break level;
        }
    };
    // Its first fenced block (a heading of the same level or above ends the section first).
    let open = loop {
        let line = lines.next()?;
        if let Some(f) = fence(line) {
            break f;
        }
        if heading(line).is_some_and(|(l, _)| l <= level) {
            return None;
        }
    };
    for line in lines {
        if fence(line).is_some_and(|f| f.starts_with(&open[..1]) && f.len() >= open.len()) {
            return None;
        }
        let cmd = without_comment(line.trim().trim_start_matches("$ ").trim());
        if cmd.is_empty() || cmd.starts_with('#') {
            continue;
        }
        let first = cmd.split_whitespace().next().unwrap_or("");
        if SETUP_WORDS.contains(&first) || (first.contains('=') && !first.starts_with('=') && cmd.split_whitespace().count() == 1) {
            continue;
        }
        let cmd = crate::projects::clean_command(cmd);
        return (!cmd.is_empty()).then_some(cmd);
    }
    None
}

/// The suggestion for a project whose folders are `folders` (most recently used first): from the `CLAUDE.md` of the
/// first one in git, as `suggested_command` reads it.
pub fn suggestion_for(folders: &[String]) -> Option<String> {
    let first = folders.iter().map(Path::new).find(|f| crate::squads::in_git(f))?;
    let text = std::fs::read_to_string(claude_md_for(first)?).ok()?;
    suggested_command(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_suggestion_is_the_first_command_of_the_build_and_test_block() {
        // Shaped like a real project's: a line that sets the shell up first, comments after the commands.
        let md = "# A project\n\nSome words.\n\n## Build and test\n\n```\nexport PATH=\"$HOME/.cargo/bin:$PATH\"     # each shell call starts fresh\ncargo test                               # ~40s\ncargo build --release\n```\n\n## Next\n\n```\nmake\n```\n";
        assert_eq!(suggested_command(md).as_deref(), Some("cargo test"));
        // Any level, any case; a `$ ` prompt and a `sh` fence; spaces inside quotes stay.
        let md = "### BUILD AND TEST\n\n~~~sh\n$ npm test -- --grep \"two  words\"\n~~~\n";
        assert_eq!(suggested_command(md).as_deref(), Some("npm test -- --grep \"two  words\""));
        // A `#` inside quotes is not a comment.
        let md = "## Build and test\n```\npytest -k 'not #slow'  # quick\n```\n";
        assert_eq!(suggested_command(md).as_deref(), Some("pytest -k 'not #slow'"));
    }

    #[test]
    fn no_section_no_block_or_an_empty_block_suggests_nothing() {
        assert_eq!(suggested_command("# Project\n\n```\ncargo test\n```\n"), None, "no Build and test section");
        assert_eq!(
            suggested_command("## Build and test\n\nRun the tests with cargo.\n\n## Next\n\n```\nmake\n```\n"),
            None,
            "no block in it"
        );
        assert_eq!(suggested_command("## Build and test\n\n```\n# only a comment\nexport X=1\n```\n"), None, "nothing to run in it");
        assert_eq!(suggested_command(""), None);
    }

    #[test]
    fn the_claude_md_is_the_folders_own_else_the_repositorys() {
        let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-ut-checks-md-{}", std::process::id())));
        let _ = fs::remove_dir_all(&base);
        let repo = base.join("shop");
        let sub = repo.join("web");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(claude_md_for(&sub), None, "none yet");
        fs::write(repo.join("CLAUDE.md"), "## Build and test\n\n```\nmake check\n```\n").unwrap();
        assert_eq!(claude_md_for(&sub), Some(repo.join("CLAUDE.md")), "the repository's");
        assert_eq!(suggestion_for(&[sub.to_string_lossy().into_owned()]).as_deref(), Some("make check"));
        fs::write(sub.join("CLAUDE.md"), "## Build and test\n\n```\nnpm test\n```\n").unwrap();
        assert_eq!(claude_md_for(&sub), Some(sub.join("CLAUDE.md")), "the folder's own first");
        // Only a folder in git is asked: the first one there is.
        let loose = base.join("notes");
        fs::create_dir_all(&loose).unwrap();
        fs::write(loose.join("CLAUDE.md"), "## Build and test\n\n```\nrm -rf /\n```\n").unwrap();
        let folders = [loose.to_string_lossy().into_owned(), sub.to_string_lossy().into_owned()];
        assert_eq!(suggestion_for(&folders).as_deref(), Some("npm test"));
        assert_eq!(suggestion_for(&folders[..1]), None, "a folder outside git suggests nothing");
        let _ = fs::remove_dir_all(&base);
    }
}
