//! The verdict: an independent check, before a task is Done, that its work does what the task asked.
//!
//! When a task's session stops, the `task-stop` hook (`taskrun.rs`) asks a fresh one-shot call on the squad's
//! `verdict_model` to judge the work (`claude -p`, or `codex exec` for an OpenAI model). It runs in the task's folder
//! with read-only tools (Claude Code's `Read`, `Grep`, `Glob`: no shell, no edits; Codex's read-only sandbox), outside the token gate (`ai::PART_OF_A_TASK`), and it is given (`prompt`): the task's text with
//! its "Done when" boxes, the session's last message (what it says it did) and, in git, the diff of the task's branch
//! from where it started (`gather`) and what is not committed yet. Its answer follows a JSON Schema (`SCHEMA`): `done`,
//! each box `met` with its evidence, and its findings, each with the exact text at the line it names. platoon looks
//! for every finding's quote in that file near that line and drops one it can't find (`judge`): a check that says "not
//! done" with nothing left that holds up counts as done, with a note. platoon runs no tests: the session is told to run
//! the repo's own and name them (`taskrun::TESTS_RULE`), and for a change to code a last message that names no test
//! run is not done, with the unmet box "Tests run and reported" (`SYSTEM`).
//!
//! Pure but for `gather` (git) and `run` (the call).

use crate::ai::{AiError, Request, Runner};
use crate::tickets::{BoxCheck, Finding, Ticket, Verdict, WHY_MAX, clean};
use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::Duration,
};

/// The ledger's name for the check's calls.
pub const FEATURE: &str = "verdict";

/// The longest the check may take.
pub const TIMEOUT: Duration = Duration::from_secs(600);

/// The tools it may use: reading only.
pub const TOOLS: crate::agents::Tools = crate::agents::Tools::ReadOnly;

/// How far from the line it names a finding's quote may be, in lines.
pub const QUOTE_SLACK: u64 = 3;

/// The most of the diff that goes in the prompt, in bytes; the check reads the files for the rest.
const DIFF_MAX: usize = 150_000;

/// The most findings and boxes kept on the task.
const KEPT: usize = 20;

/// What the check is told it is for. Short: Claude's own instructions are left out (`--system-prompt`).
pub const SYSTEM: &str = "You check whether a piece of work does what its task asked. Someone else did the work; you \
judge it. You can read the files of the folder you are in (Read, Grep, Glob); you can't run or change anything.

- Judge only what the task asks, and each of its \"Done when\" boxes. Not style, naming, or what you would have done \
differently.
- Don't take the worker's word for it: look in the code for the evidence.
- For a change to code, the worker's last message must name the test command it ran and its result (or say the \
repository has no tests and how it checked the change instead). If it names no test run, done is false, and add the \
box \"Tests run and reported\" with met false.
- A finding is something the task asks for that is missing or wrong. Name the file (relative to this folder), the \
line, and copy the exact text of that line as `quote`; say the problem in one sentence.
- done is true when the work does what the task asks and every box is met.
- Answer only with the JSON asked for.";

/// The answer's JSON Schema (`--json-schema`).
pub const SCHEMA: &str = r#"{"type":"object","properties":{"done":{"type":"boolean"},"boxes":{"type":"array","items":{"type":"object","properties":{"text":{"type":"string"},"met":{"type":"boolean"},"evidence":{"type":"string"}},"required":["text","met","evidence"]}},"findings":{"type":"array","items":{"type":"object","properties":{"file":{"type":"string"},"line":{"type":"integer"},"quote":{"type":"string"},"problem":{"type":"string"}},"required":["file","line","quote","problem"]}}},"required":["done","boxes","findings"]}"#;

/// What the check is given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Material {
    /// "#N title" and the task's text.
    pub task: String,
    /// The session's last message: what it says it did.
    pub claim: String,
    /// The folder it works in.
    pub folder: String,
    /// In git: the diff of the task's branch from where it started ("" for none). None: the folder is not in git.
    pub diff: Option<String>,
    /// `git status --short` of the folder: what is not committed yet.
    pub uncommitted: String,
}

/// The prompt: the task, the worker's last message, and the change.
pub fn prompt(m: &Material) -> String {
    let mut out =
        format!("The folder you are in: {}\n\n# The task\n\n{}\n\n# What the worker said at the end\n\n", m.folder, m.task.trim());
    let claim = m.claim.trim();
    out.push_str(if claim.is_empty() { "(nothing)" } else { claim });
    out.push_str("\n\n# The change\n\n");
    match &m.diff {
        None => out.push_str("The folder is not a git repository: judge the files in it as they are.\n"),
        Some(d) if d.trim().is_empty() => out.push_str("Nothing was committed on the task's branch.\n"),
        Some(d) => {
            let (shown, cut) = cut_at(d, DIFF_MAX);
            out.push_str("The diff of the task's branch from where it started:\n\n```diff\n");
            out.push_str(shown.trim_end());
            out.push_str("\n```\n");
            if cut {
                out.push_str("(The diff is cut here: read the files for the rest.)\n");
            }
        }
    }
    if !m.uncommitted.trim().is_empty() {
        out.push_str(&format!("\nNot committed yet (`git status --short`):\n\n```\n{}\n```\n", m.uncommitted.trim_end()));
    }
    out
}

/// `s` cut to at most `max` bytes at a line's end, and whether it was cut.
fn cut_at(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let end = s[..end].rfind('\n').map_or(end, |i| i + 1);
    (&s[..end], true)
}

/// Read what the check is given from the task and its folder. `program` is git; `body` the task's text; `claim` the
/// session's last message.
pub fn gather(program: &str, t: &Ticket, body: &str, claim: &str) -> Material {
    let folder = t.work_folder().unwrap_or_default();
    let mut m = Material {
        task: format!("#{} {}\n\n{}", t.number, t.title, body.trim()),
        claim: claim.to_string(),
        folder: folder.clone(),
        ..Default::default()
    };
    let at = Path::new(&folder);
    let git = |args: &[&str]| crate::worktrees::git(program, at, args);
    if git(&["rev-parse", "--is-inside-work-tree"]).is_err() {
        return m;
    }
    let branch = t.branch();
    let base = t.worktree.as_ref().and_then(|w| w.base.clone()).or_else(|| {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let default = crate::gitfacts::default_branch(program, at, deadline)?;
        git(&["merge-base", &default, &branch]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    });
    m.diff = Some(match base {
        Some(base) => git(&["diff", "--no-color", "--no-ext-diff", &format!("{base}...{branch}")]).unwrap_or_default(),
        None => String::new(),
    });
    m.uncommitted = git(&["status", "--short"]).unwrap_or_default();
    m
}

/// One box as the check answered it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxAnswer {
    pub text: String,
    pub met: bool,
    pub evidence: String,
}

/// One finding as the check answered it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingAnswer {
    pub file: String,
    pub line: u64,
    pub quote: String,
    pub problem: String,
}

/// The check's answer, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub done: bool,
    pub boxes: Vec<BoxAnswer>,
    pub findings: Vec<FindingAnswer>,
}

/// Read the check's answer. `done` must be there and be true or false; a box or a finding that isn't shaped as asked
/// is left out.
pub fn parse(text: &str) -> Result<Answer, String> {
    let v = crate::ai::extract_json(text).ok_or("its answer isn't JSON")?;
    let done = v.get("done").and_then(Value::as_bool).ok_or("its answer has no done: true or false")?;
    let list = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    let s = |x: &Value, k: &str| x.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let boxes = list("boxes")
        .iter()
        .filter_map(|b| Some(BoxAnswer { text: s(b, "text"), met: b.get("met")?.as_bool()?, evidence: s(b, "evidence") }))
        .filter(|b| !b.text.is_empty())
        .collect();
    let findings = list("findings")
        .iter()
        .filter_map(|f| {
            let line = f.get("line").and_then(|l| l.as_u64().or_else(|| l.as_str()?.trim().parse().ok()))?;
            Some(FindingAnswer { file: s(f, "file"), line, quote: f.get("quote")?.as_str()?.to_string(), problem: s(f, "problem") })
        })
        .collect();
    Ok(Answer { done, boxes, findings })
}

/// Whitespace runs as one space, trimmed.
fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where a finding's file is, when it is inside `folder` (a relative path, or an absolute one under it).
fn inside(folder: &Path, file: &str) -> Option<PathBuf> {
    let p = Path::new(file.trim());
    if file.trim().is_empty() || p.components().any(|c| c == Component::ParentDir) {
        return None;
    }
    let full = if p.is_absolute() { p.to_path_buf() } else { folder.join(p) };
    let (root, real) = (fs::canonicalize(folder).ok()?, fs::canonicalize(&full).ok()?);
    real.starts_with(&root).then_some(real)
}

/// Is the finding's quote in its file, within `QUOTE_SLACK` lines of the line it names (whitespace aside)?
pub fn quote_found(folder: &Path, f: &FindingAnswer) -> bool {
    let quote = squeeze(&f.quote);
    if quote.is_empty() || f.line == 0 {
        return false;
    }
    let Some(path) = inside(folder, &f.file) else { return false };
    let mut text = String::new();
    let read = fs::File::open(&path).and_then(|file| file.take(4 << 20).read_to_string(&mut text));
    if read.is_err() {
        return false;
    }
    let lines: Vec<&str> = text.lines().collect();
    let span = f.quote.trim().lines().count().max(1) as u64;
    let from = f.line.saturating_sub(1 + QUOTE_SLACK) as usize;
    let to = ((f.line - 1 + QUOTE_SLACK + span) as usize).min(lines.len());
    from < to && squeeze(&lines[from..to].join("\n")).contains(&quote)
}

/// What platoon makes of the answer: the findings whose quotes are where they say, the boxes, and done.
pub fn judge(a: &Answer, folder: &Path, model: &str, tokens: u64, at: u64) -> Verdict {
    let (found, lost): (Vec<&FindingAnswer>, Vec<&FindingAnswer>) = a.findings.iter().partition(|f| quote_found(folder, f));
    let boxes: Vec<BoxCheck> = a.boxes.iter().take(KEPT).map(|b| BoxCheck { text: clean(&b.text, WHY_MAX), met: b.met }).collect();
    let findings: Vec<Finding> = found
        .iter()
        .take(KEPT)
        .map(|f| Finding {
            file: clean(f.file.trim().trim_start_matches("./"), 300),
            line: f.line,
            problem: clean(if f.problem.is_empty() { "(no problem given)" } else { &f.problem }, WHY_MAX),
        })
        .collect();
    let nothing_holds = findings.is_empty() && boxes.iter().all(|b| b.met);
    let note = match (a.done, nothing_holds, lost.is_empty()) {
        (false, true, false) => "the check's findings could not be found in the code",
        (false, true, true) => "the check said not done but gave no reason",
        _ => "",
    };
    Verdict {
        at,
        model: model.to_string(),
        failed: None,
        done: a.done || nothing_holds,
        boxes,
        findings,
        dropped: u32::try_from(lost.len()).unwrap_or(u32::MAX),
        note: note.to_string(),
        tokens,
    }
}

/// A check that could not run, and why.
pub fn failed(model: &str, why: &str, tokens: u64, at: u64) -> Verdict {
    Verdict { at, model: model.to_string(), failed: Some(clean(why, WHY_MAX)), tokens, ..Default::default() }
}

/// What the check found, short, for the task's "Needs you" line: "2 problems: src/mail.rs:42: ..." (file:line first).
pub fn short(v: &Verdict) -> String {
    let n = v.problems();
    let first = v.findings.first().map(Finding::line_text).or_else(|| v.unmet().next().map(|b| format!("not met: {}", b.text)));
    let count = if n == 1 { "1 problem".to_string() } else { format!("{n} problems") };
    clean(
        &match first {
            Some(f) => format!("{count}: {f}"),
            None => count,
        },
        WHY_MAX,
    )
}

/// What the session is told when its stop is refused: the findings (file:line first) and the unmet boxes.
pub fn refusal(v: &Verdict) -> String {
    let mut out = format!(
        "platoon's check of your work (a fresh {} session that read the task, your last message and the change) says it \
         isn't done yet:\n\n",
        v.model
    );
    for f in &v.findings {
        out.push_str(&format!("- {}\n", f.line_text()));
    }
    for b in v.unmet() {
        out.push_str(&format!("- Done when \"{}\": not met\n", b.text));
    }
    out.push_str(
        "\nFix these, then finish again: platoon checks again when you stop. If a finding is wrong, \
         say why in your last message.",
    );
    out
}

/// The answer as it came, kept as `verdict.json` in the task's own folder (pretty, when it is JSON).
pub fn keep(file: &Path, raw: &str) {
    let text = crate::ai::extract_json(raw).and_then(|v| serde_json::to_string_pretty(&v).ok()).unwrap_or_else(|| raw.to_string());
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = crate::projects::write_atomic(file, &text);
}

/// Run the check: a one-shot call on `model` in `folder`, read-only, with the schema. The answer's text (JSON) and the
/// tokens it used, or why it could not run with the tokens used anyway.
pub fn run(runner: &Runner, model: &str, folder: &Path, prompt: String, timeout: Duration) -> Result<(String, u64), (String, u64)> {
    let mut req = Request::new(FEATURE, SYSTEM, prompt);
    req.model = Some(model.to_string());
    req.tools = TOOLS;
    req.cwd = Some(folder.to_path_buf());
    req.json_schema = Some(SCHEMA.to_string());
    req.effort = "medium";
    req.timeout = timeout;
    req.user_initiated = true;
    let settings = crate::settings::Settings::default();
    match runner.run(&settings, &req) {
        Ok(r) => Ok((r.text, r.usage.total())),
        Err(AiError::Failed { detail, .. }) => Err((detail, 0)),
        Err(e) => Err((e.plain(), 0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-verdict-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("src")).unwrap();
        d
    }

    fn finding(file: &str, line: u64, quote: &str) -> FindingAnswer {
        FindingAnswer { file: file.into(), line, quote: quote.into(), problem: "it's wrong".into() }
    }

    #[test]
    fn a_findings_quote_counts_only_in_its_file_near_its_line() {
        let d = dir("quotes");
        let lines: Vec<String> = (1..=20).map(|i| format!("    let x{i} = {i};")).collect();
        fs::write(d.join("src/mail.rs"), lines.join("\n")).unwrap();
        fs::write(d.parent().unwrap().join("platoon-verdict-outside.rs"), "    let x10 = 10;").unwrap();
        assert!(quote_found(&d, &finding("src/mail.rs", 10, "let x10 = 10;")), "on its line, whitespace aside");
        assert!(quote_found(&d, &finding("./src/mail.rs", 13, "let   x10 = 10;")), "three lines off");
        assert!(quote_found(&d, &finding(&d.join("src/mail.rs").to_string_lossy(), 7, "let x10 = 10;")), "an absolute path inside");
        assert!(quote_found(&d, &finding("src/mail.rs", 10, "let x10 = 10;\n    let x11 = 11;")), "over two lines");
        assert!(!quote_found(&d, &finding("src/mail.rs", 14, "let x10 = 10;")), "four lines off");
        assert!(!quote_found(&d, &finding("src/mail.rs", 10, "let y = 1;")), "not there");
        assert!(!quote_found(&d, &finding("src/mail.rs", 10, "  ")), "an empty quote");
        assert!(!quote_found(&d, &finding("src/mail.rs", 0, "let x1 = 1;")), "line 0");
        assert!(!quote_found(&d, &finding("src/other.rs", 10, "let x10 = 10;")), "no such file");
        assert!(!quote_found(&d, &finding("../platoon-verdict-outside.rs", 1, "let x10 = 10;")), "outside the folder");
        let _ = fs::remove_file(d.parent().unwrap().join("platoon-verdict-outside.rs"));
    }

    #[test]
    fn the_answer_is_read_and_judged_findings_without_their_quote_dropped() {
        let d = dir("judge");
        fs::write(d.join("src/mail.rs"), "fn send() {\n    html_only();\n}\n").unwrap();
        let text = r#"{"done": false,
            "boxes": [{"text": "A text-only client gets the order", "met": false, "evidence": "no text part"},
                      {"text": "Docs say so", "met": true, "evidence": "guide/mail.md:3"}, {"met": true}],
            "findings": [{"file": "src/mail.rs", "line": "2", "quote": "html_only();", "problem": "Only HTML is sent."},
                         {"file": "src/mail.rs", "line": 40, "quote": "text_part();", "problem": "Made up."},
                         {"file": "src/mail.rs"}]}"#;
        let a = parse(text).unwrap();
        assert_eq!((a.done, a.boxes.len(), a.findings.len()), (false, 2, 2), "the ones not shaped as asked are left out");
        let v = judge(&a, &d, "fable", 1234, 99);
        assert!(!v.done);
        assert_eq!(v.findings, vec![Finding { file: "src/mail.rs".into(), line: 2, problem: "Only HTML is sent.".into() }]);
        assert_eq!((v.dropped, v.tokens, v.model.as_str(), v.at), (1, 1234, "fable", 99));
        assert_eq!(v.problems(), 2, "the finding and the unmet box");
        assert_eq!(short(&v), "2 problems: src/mail.rs:2: Only HTML is sent.");
        let r = refusal(&v);
        assert!(r.contains("- src/mail.rs:2: Only HTML is sent.\n- Done when \"A text-only client gets the order\": not met"), "{r}");
        assert!(!r.contains("Made up"), "{r}");
        // Not done, but nothing it found holds up: done, with a note.
        let made_up = Answer { done: false, boxes: vec![], findings: vec![finding("src/mail.rs", 40, "text_part();")] };
        let v = judge(&made_up, &d, "fable", 0, 0);
        assert!(v.done && v.dropped == 1 && v.note == "the check's findings could not be found in the code", "{v:?}");
        let v = judge(&Answer { done: false, boxes: vec![], findings: vec![] }, &d, "fable", 0, 0);
        assert!(v.done && v.note == "the check said not done but gave no reason", "{v:?}");
        // No test run named: the box the check adds is unmet, so it is not done, and the session is told so.
        let untested = Answer {
            done: false,
            boxes: vec![BoxAnswer { text: "Tests run and reported".into(), met: false, evidence: "no test command named".into() }],
            findings: vec![],
        };
        let v = judge(&untested, &d, "fable", 0, 0);
        assert!(!v.done && v.note.is_empty(), "{v:?}");
        assert_eq!(short(&v), "1 problem: not met: Tests run and reported");
        assert!(refusal(&v).contains("- Done when \"Tests run and reported\": not met"), "{}", refusal(&v));
        assert!(!refusal(&v).contains("platoon runs the tests"), "platoon runs none");
        // Done.
        let v = judge(&Answer { done: true, boxes: a.boxes[1..].to_vec(), findings: vec![] }, &d, "fable", 0, 0);
        assert!(v.done && v.note.is_empty() && v.problems() == 0);
        // Not an answer.
        assert!(parse("I think it's fine").is_err());
        assert!(parse(r#"{"boxes": []}"#).unwrap_err().contains("done"));
        assert!(parse("```json\n{\"done\": true}\n```").unwrap().done, "fenced, and the lists may be missing");
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn in_git_the_check_gets_the_branchs_diff_from_where_it_started_and_what_is_not_committed() {
        let d = dir("gather");
        let repo = d.join("shop");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("mail.rs"), "fn send() {}\n").unwrap();
        fs::write(repo.join("notes.txt"), "old\n").unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "first"]);
        let wt = crate::worktrees::prepare("git", &repo.to_string_lossy(), 5, None).unwrap().unwrap();
        let at = Path::new(&wt.path);
        fs::write(at.join("mail.rs"), "fn send() {}\nfn send_text() {}\n").unwrap();
        git(at, &["commit", "-q", "-am", "text part"]);
        fs::write(at.join("notes.txt"), "changed\n").unwrap();
        // The default branch moving on later isn't the task's change.
        fs::write(repo.join("other.rs"), "x\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "elsewhere"]);
        let mut t = Ticket { number: 5, title: "Text mails".into(), worktree: Some(wt.clone()), ..Default::default() };
        for base in [wt.base.clone(), None] {
            t.worktree.as_mut().unwrap().base = base.clone();
            let m = gather("git", &t, "## Done when\n\n- [ ] Text part", "Added it.");
            let diff = m.diff.clone().unwrap();
            assert!(diff.contains("+fn send_text() {}"), "base {base:?}: {diff}");
            assert!(!diff.contains("other.rs") && !diff.contains("changed"), "base {base:?}: {diff}");
            assert!(m.uncommitted.contains("notes.txt"), "{}", m.uncommitted);
            assert_eq!((m.task.as_str(), m.claim.as_str()), ("#5 Text mails\n\n## Done when\n\n- [ ] Text part", "Added it."));
        }
        // Not in git: no diff.
        let plain = d.join("plain");
        fs::create_dir_all(&plain).unwrap();
        let t = Ticket { number: 6, folder: Some(plain.to_string_lossy().into_owned()), ..Default::default() };
        assert_eq!(gather("git", &t, "", "").diff, None);
    }

    #[test]
    fn the_prompt_has_the_task_the_claim_and_the_change() {
        let m = Material {
            task: "#5 Login\n\n## Done when\n\n- [ ] It logs in".into(),
            claim: "I added the login form.".into(),
            folder: "/w/shop-worktrees/task-5".into(),
            diff: Some("diff --git a/x b/x\n+login\n".into()),
            uncommitted: " M src/x.rs\n".into(),
        };
        let p = prompt(&m);
        for want in ["/w/shop-worktrees/task-5", "#5 Login", "- [ ] It logs in", "I added the login form.", "+login", "M src/x.rs"] {
            assert!(p.contains(want), "{want}:\n{p}");
        }
        assert!(prompt(&Material { diff: None, ..m.clone() }).contains("not a git repository"));
        assert!(prompt(&Material { diff: Some(String::new()), uncommitted: String::new(), ..m.clone() }).contains("Nothing was committed"));
        let big = "+a line of the diff\n".repeat(20_000);
        let p = prompt(&Material { diff: Some(big), ..m });
        assert!(p.len() < DIFF_MAX + 2000 && p.contains("The diff is cut here"), "{}", p.len());
        // A change to code is done only with its tests run and named, by the worker: platoon runs none.
        assert!(SYSTEM.contains("name the test command it ran and its result"), "{SYSTEM}");
        assert!(SYSTEM.contains("If it names no test run, done is false, and add the box \"Tests run and reported\""), "{SYSTEM}");
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(schema["required"], serde_json::json!(["done", "boxes", "findings"]));
    }
}
