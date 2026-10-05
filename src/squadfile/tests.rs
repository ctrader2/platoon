//! The briefs' store, the built-ins as Markdown, the rules, the one-time conversion of the old JSON files and the
//! drafting of a brief.

use super::*;

fn dir(name: &str) -> PathBuf {
    let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-squadfile-{name}-{}", std::process::id())));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn coding() -> Squad {
    builtin("coding").unwrap()
}

fn agents() -> Vec<Agent> {
    agentfile::builtins()
}

fn step(agent: &str, does: &str, back: Option<Back>) -> Step {
    Step { agent: agent.into(), does: does.into(), back, line: 0 }
}

/// The built-in squads as the code had them before they were briefs: what the Markdown must mean.
fn before_briefs() -> Vec<(&'static str, &'static str, &'static str, Vec<Step>)> {
    use crate::squads::{CODING_CAN_DO, DOCS_CAN_DO, RESEARCH_CAN_DO, SUMMARIZE_CAN_DO, SUMMARIZE_DOES};
    vec![
        (
            "coding",
            "Coding",
            CODING_CAN_DO,
            vec![
                step(
                    "planner",
                    "Reads the task and the code it touches and writes a short plan that restates the acceptance checks.",
                    None,
                ),
                step("builder", "Makes the change the plan describes in the task's folder and runs the tests.", None),
                step(
                    "reviewer",
                    "Reads the change against the task and the plan, and checks that each acceptance check could have failed before the change.",
                    Some(Back { to: 1, times: 2 }),
                ),
                step(
                    "verifier",
                    "Runs the tests and the build and reports plainly what passed and what failed.",
                    Some(Back { to: 1, times: 1 }),
                ),
            ],
        ),
        (
            "research",
            "Research",
            RESEARCH_CAN_DO,
            vec![
                step("questioner", "Turns the task into a short list of specific questions to answer.", None),
                step("researcher", "Answers each question with its sources, from the task's folders and the web.", None),
                step(
                    "report-writer",
                    "Writes report.md in the task's own folder (never in the project's folders): the answers, the sources, and what is still unknown.",
                    None,
                ),
                step("checker", "Checks each claim in the report against its source.", Some(Back { to: 1, times: 1 })),
            ],
        ),
        (
            "docs",
            "Docs",
            DOCS_CAN_DO,
            vec![
                step("outliner", "Reads the task, the existing docs and the code they describe, and proposes what to write where.", None),
                step("tech-writer", "Writes or updates the Markdown files the outline names, in the task's folder.", None),
                step(
                    "editor",
                    "Checks the files for accuracy against the code and the task, clarity, and the project's style.",
                    Some(Back { to: 1, times: 2 }),
                ),
            ],
        ),
        ("summarize", "Summarize", SUMMARIZE_CAN_DO, vec![step("summarizer", SUMMARIZE_DOES, None)]),
    ]
}

#[test]
fn the_built_in_briefs_mean_what_the_code_said_before() {
    let all = builtins();
    assert_eq!(all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["coding", "research", "docs", "summarize"]);
    for ((id, name, can_do, steps), s) in before_briefs().into_iter().zip(&all) {
        assert!(s.shape.is_empty(), "{id}: {:?}", s.shape);
        assert_eq!((s.id.as_str(), s.name.as_str(), s.can_do.as_str()), (id, name, can_do), "same words for what it does");
        let plain: Vec<Step> = s.workflow.iter().map(|st| Step { line: 0, ..st.clone() }).collect();
        assert_eq!(plain, steps, "{id}: the same steps and send-backs");
        assert!(problems(s, &agents()).is_empty(), "{id}: {:?}", problems(s, &agents()));
        assert!(!s.asks_you.is_empty(), "{id} says when it asks you");
        // Written by platoon and read again: the same.
        assert!(brief::parse(&brief::write(s), id).same_as(s), "{id}");
    }
    let kinds = |id: &str| builtin(id).unwrap().check_kinds().into_iter().map(|(_, k)| k).collect::<Vec<_>>();
    assert_eq!(kinds("coding"), [CheckKind::Tests, CheckKind::Read("reviewer".into()), CheckKind::Acceptance, CheckKind::Gate]);
    assert_eq!(kinds("research"), [CheckKind::Read("checker".into()), CheckKind::Prose, CheckKind::Gate]);
    assert_eq!(kinds("docs"), [CheckKind::Tests, CheckKind::Read("editor".into()), CheckKind::Gate]);
    assert_eq!(kinds("summarize"), [CheckKind::Gate]);
    assert!(is_builtin("coding") && is_builtin("research") && !is_builtin("coda"));
    assert_eq!(builtin_text("docs").unwrap().lines().next(), Some("---"));
    let crew = Crew::of(&coding(), &agents());
    let m: Vec<(&str, &str, bool, bool, bool)> =
        crew.agents.iter().map(|m| (m.id.as_str(), m.model.as_str(), m.files, m.commands, m.web)).collect();
    assert_eq!(
        m,
        [
            ("planner", "opus", false, true, false),
            ("builder", "sonnet", true, true, false),
            ("reviewer", "opus", false, true, false),
            ("verifier", "haiku", false, true, false)
        ]
    );
    assert_eq!(steps_words(&[2]), "step 2");
    assert_eq!(steps_words(&[1, 2, 4]), "steps 1, 2 and 4");
}

#[test]
fn a_brief_round_trips_keeping_what_platoon_doesnt_know() {
    let c = dir("round");
    let s = copy_of(&c, &coding(), "Sentry triage", &[]);
    assert_eq!(s.id, "sentry-triage");
    save(&c, &s).unwrap();
    let p = path(&c, "sentry-triage");
    assert!(p.ends_with("briefs/sentry-triage.md"));
    // Someone adds a front matter line and a section of their own.
    let text = fs::read_to_string(&p).unwrap().replacen("---\n# What", "colour: red\n---\n# What", 1) + "\n# Notes\nOnly on weekdays.\n";
    fs::write(&p, &text).unwrap();
    let back = load(&c, "sentry-triage").unwrap().unwrap();
    assert!(back.shape.is_empty(), "{:?}", back.shape);
    assert_eq!(back.meta, [("colour".to_string(), "red".to_string())]);
    assert_eq!(back.other, [("Notes".to_string(), "Only on weekdays.".to_string())]);
    // A copy of it keeps them.
    let copy = copy_of(&c, &back, "Sentry triage two", &[]);
    save(&c, &copy).unwrap();
    let again = load(&c, &copy.id).unwrap().unwrap();
    assert_eq!((again.meta.clone(), again.other.clone()), (back.meta.clone(), back.other.clone()));
    let listed = load_all(&c);
    assert_eq!(
        listed.iter().map(|l| (l.id.as_str(), l.kind)).collect::<Vec<_>>(),
        [
            ("coding", Kind::BuiltIn),
            ("research", Kind::BuiltIn),
            ("docs", Kind::BuiltIn),
            ("summarize", Kind::BuiltIn),
            ("sentry-triage", Kind::Own),
            ("sentry-triage-two", Kind::Own)
        ]
    );
    assert!(!old_dir(&c).exists(), "nothing written where the old files were");
}

#[test]
fn a_file_that_cant_be_read_is_shown_as_such_and_never_written_over() {
    let c = dir("corrupt");
    fs::create_dir_all(super::dir(&c)).unwrap();
    fs::write(path(&c, "mine"), [0xff, 0xfe, 0x00]).unwrap();
    fs::write(path(&c, "coding"), [0xc3, 0x28]).unwrap();
    let listed = load_all(&c);
    assert_eq!(listed.len(), 5);
    assert!(listed[0].squad.is_none() && listed[0].corrupt.as_deref() == Some("it isn't text (UTF-8)"), "{listed:?}");
    assert_eq!(listed[4].corrupt.as_deref(), Some("it isn't text (UTF-8)"));
    assert!(get(&c, "coding").is_none(), "a changed built-in that can't be read isn't quietly the built-in");
    let mut s = coding();
    s.name = "Mine".into();
    let err = save(&c, &s).unwrap_err().to_string();
    assert!(err.contains("can't be read, so platoon leaves it alone"), "{err}");
    assert_eq!(fs::read(path(&c, "coding")).unwrap(), [0xc3, 0x28]);
    // A file that is text but not a brief is read, and says what is wrong where.
    fs::write(path(&c, "mine"), "just some words\n").unwrap();
    let s = get(&c, "mine").unwrap();
    assert_eq!(
        problems(&s, &agents()),
        [
            "The brief has no front matter: start it with a \"---\" line, \"name: ...\" and another \"---\".",
            "The brief has no \"# What it does\" section.",
            "The brief has no \"# Steps\" section.",
            "Front matter: the brief has no name; add a line \"name: ...\"."
        ]
    );
}

#[test]
fn a_changed_built_in_says_so_and_restore_puts_the_file_aside() {
    let c = dir("restore");
    let mut s = coding();
    s.can_do = "Makes careful changes.".into();
    save(&c, &s).unwrap();
    let text = fs::read_to_string(path(&c, "coding")).unwrap();
    assert!(text.starts_with("---\nid: coding\nname: Coding\n---\n# What it does\nMakes careful changes.\n"), "{text}");
    assert_eq!(load_all(&c)[0].kind, Kind::Modified);
    assert_eq!(get(&c, "coding").unwrap().can_do, "Makes careful changes.");
    remove(&c, "coding").unwrap();
    assert!(!path(&c, "coding").exists() && path(&c, "coding").with_extension("md.bak").exists());
    assert_eq!(load_all(&c)[0].kind, Kind::BuiltIn);
    assert!(get(&c, "coding").unwrap().same_as(&coding()));
    // Saved as platoon has it: no file.
    save(&c, &s).unwrap();
    save(&c, &coding()).unwrap();
    assert!(!path(&c, "coding").exists());
    assert_eq!(load_all(&c)[0].kind, Kind::BuiltIn);
    // The built-in's own text, written for the user's editor, reads as the built-in.
    save_text(&c, "coding", builtin_text("coding").unwrap()).unwrap();
    assert!(get(&c, "coding").unwrap().same_as(&coding()));
}

#[test]
fn every_problem_is_one_line_that_says_what_and_where() {
    let text = "---\nid: mine\nname: Mine\n---\n# What it does\nIt does things.\n\n# Steps\n1. planner: Plans.\n2. builder: Builds.\n3. verifer: Checks. May send the work back to step 3, at most twice.\n4. reviewer:\n\n# Checks\n- reviewr read the change\n- the tests pass\n";
    let s = brief::parse(text, "mine");
    assert_eq!(
        problems(&s, &agents()),
        [
            "Steps, line 11: no agent called verifer; there are planner, builder, reviewer and verifier.",
            "Steps, line 11: it sends the work back to step 3, which isn't before it.",
            "Steps, line 12: say what step 4 does after \"reviewer:\".",
            "Checks, line 15: no step is done by reviewr; the steps' agents are planner, builder, verifer and reviewer."
        ]
    );
    let ok = coding();
    let with = |f: &dyn Fn(&mut Squad)| {
        let mut s = ok.clone();
        f(&mut s);
        problems(&s, &agents())
    };
    assert_eq!(
        with(&|s| s.id = "Bad Id".into()),
        ["The file name Bad Id.md can't be a brief's: use lowercase letters, digits and dashes, starting with a letter."]
    );
    assert_eq!(with(&|s| s.name = " ".into()), ["Front matter: the brief has no name; add a line \"name: ...\"."]);
    assert_eq!(with(&|s| s.name = "x".repeat(41)), ["Front matter, line 3: the name is longer than 40 characters."]);
    assert_eq!(with(&|s| s.can_do = String::new()), ["What it does, line 5: say in a sentence or two what the squad does."]);
    assert_eq!(
        with(&|s| s.workflow[2].back = Some(Back { to: 0, times: 9 })),
        ["Steps, line 12: it may send the work back 1 to 5 times, not 9."]
    );
    assert_eq!(
        with(&|s| s.workflow.clear()),
        [
            "Steps, line 9: there are no steps; write one a line, as \"1. builder: what it does\".",
            "Checks, line 19: no step is done by reviewer; the steps' agents are none."
        ]
    );
    // An agent with problems of its own keeps the squads that use it from starting, said once.
    let mut blank = agents();
    blank.iter_mut().find(|a| a.id == "reviewer").unwrap().guidance = String::new();
    let mut twice = ok.clone();
    twice.workflow.push(step("reviewer", "Again.", None));
    assert_eq!(problems(&twice, &blank), ["Steps, line 12: its agent reviewer isn't ready: reviewer has no system prompt yet."]);
    // At most 8 different agents and 12 steps.
    let mut many = ok.clone();
    let mut lib = agents();
    for n in 0..9 {
        let mut a = agentfile::builtin("planner").unwrap();
        a.id = format!("helper-{n}");
        lib.push(a);
        many.workflow.push(step(&format!("helper-{n}"), "Helps.", None));
    }
    assert_eq!(
        problems(&many, &lib),
        ["Step 13: step 13 is one too many; a brief has at most 12 steps.", "Steps, line 9: they name 13 different agents; at most 8."]
    );
    // A name nowhere near any agent: the brief's own agents are named.
    let mut far = ok.clone();
    far.workflow[0].agent = "zzzzzzzz".into();
    assert_eq!(problems(&far, &agents())[0], "Steps, line 10: no agent called zzzzzzzz; there are builder, reviewer and verifier.");
}

#[test]
fn edited_old_coding_prompts_are_carried_over_once_into_the_built_in_agents_but_not_the_leader() {
    let c = dir("migrate");
    fs::create_dir_all(c.join("prompts")).unwrap();
    fs::write(c.join("prompts/coding-builder.md"), "Build it in small commits.\n").unwrap();
    fs::write(c.join("prompts/coding-planner.md"), format!("{}\n", crate::squads::PLANNER_GUIDANCE)).unwrap();
    fs::write(c.join("prompts/coding-leader.md"), "Lead calmly.").unwrap();
    let listed = load_all(&c);
    assert_eq!(listed[0].kind, Kind::BuiltIn, "the old leader's text isn't carried anywhere: every squad shares one leader now");
    assert!(!path(&c, "coding").exists());
    assert!(!crate::instructions::path(&c, crate::instructions::SQUAD_LEADER).exists());
    assert_eq!(agentfile::get(&c, "builder").unwrap().guidance, "Build it in small commits.");
    assert!(!agentfile::path(&c, "planner").exists(), "unchanged stays the built-in");
    assert!(old_dir(&c).join(MIGRATED_MARK).exists(), "the mark stays where it always was");
    agentfile::remove(&c, "builder").unwrap();
    assert!(!migrate_prompts(&c), "restored: not carried over again");
    let c2 = dir("migrate-none");
    fs::create_dir_all(c2.join("prompts")).unwrap();
    fs::write(c2.join("prompts/coding-verifier.md"), crate::squads::VERIFIER_GUIDANCE).unwrap();
    assert_eq!(load_all(&c2)[0].kind, Kind::BuiltIn);
    assert!(!old_dir(&c2).exists() && !super::dir(&c2).exists() && !agentfile::dir(&c2).exists(), "no folders for nothing");
}

/// An old JSON squad file of the user's own.
fn write_old(c: &Path, id: &str, v: &Value) {
    fs::create_dir_all(old_dir(c)).unwrap();
    fs::write(old_path(c, id), v.to_string()).unwrap();
}

#[test]
fn each_old_json_file_becomes_a_brief_once_and_is_kept_as_bak() {
    let c = dir("convert");
    // One of the user's own, with keys platoon never knew and the retired ones.
    let mut own = to_value(&copy_of(&c, &coding(), "Scripts", &[]), None);
    let o = own.as_object_mut().unwrap();
    o.insert("leader".into(), json!({"model": "opus", "guidance": "Lead."}));
    o.insert("colour".into(), json!("teal"));
    write_old(&c, "scripts", &own);
    // A changed built-in: renamed, and its reviewer step gone.
    let mut changed = coding();
    changed.name = "Coding Agent".into();
    changed.workflow.remove(2);
    changed.workflow[2].back = None;
    write_old(&c, "coding", &to_value(&changed, Some("coding")));
    // A changed built-in that is the built-in after all (only retired keys).
    let mut same = to_value(&builtin("docs").unwrap(), Some("docs"));
    same.as_object_mut().unwrap().insert("use_worktree".into(), json!(true));
    write_old(&c, "docs", &same);
    // One whose brief is there already: the brief wins.
    let mut mine = copy_of(&c, &coding(), "Mine", &[]);
    mine.can_do = "The brief's words.".into();
    save(&c, &mine).unwrap();
    let mut old_mine = mine.clone();
    old_mine.can_do = "The old file's words.".into();
    write_old(&c, "mine", &to_value(&old_mine, None));
    // One that can't be read.
    fs::write(old_path(&c, "broken"), "{ nope").unwrap();

    let listed = load_all(&c);
    let by = |id: &str| listed.iter().find(|l| l.id == id).unwrap();
    let scripts = by("scripts").squad.clone().unwrap();
    assert!(scripts.shape.is_empty(), "{:?}", scripts.shape);
    assert_eq!((scripts.name.as_str(), scripts.workflow.len()), ("Scripts", 4));
    assert_eq!(scripts.checks, DEFAULT_CHECKS, "an own squad gets the tests and the gate, as every squad had before");
    assert_eq!(scripts.asks_you, DEFAULT_ASKS_YOU);
    assert!(old_dir(&c).join("scripts.json.bak").exists() && !old_path(&c, "scripts").exists());
    let bak: Value = serde_json::from_str(&fs::read_to_string(old_dir(&c).join("scripts.json.bak")).unwrap()).unwrap();
    assert_eq!(bak["colour"], "teal", "the old file is kept as it was");
    let coding_now = by("coding");
    assert_eq!(coding_now.kind, Kind::Modified);
    let s = coding_now.squad.clone().unwrap();
    assert_eq!((s.name.as_str(), s.agent_ids()), ("Coding Agent", vec!["planner".to_string(), "builder".into(), "verifier".into()]));
    assert_eq!(
        s.checks,
        ["the tests pass", "every acceptance box is ticked, with how", "nothing others see without your yes"],
        "the built-in's checks, less the one naming an agent its steps no longer have"
    );
    assert!(problems(&s, &agents()).is_empty(), "{:?}", problems(&s, &agents()));
    assert_eq!(by("docs").kind, Kind::BuiltIn, "no brief is needed for the built-in as it is");
    assert!(!path(&c, "docs").exists() && old_dir(&c).join("docs.json.bak").exists());
    assert_eq!(get(&c, "mine").unwrap().can_do, "The brief's words.", "a brief that is there wins");
    assert!(old_dir(&c).join("mine.json.bak").exists() && !old_path(&c, "mine").exists());
    let broken = by("broken");
    assert!(broken.squad.is_none() && broken.corrupt.as_deref().is_some_and(|w| w.contains("it isn't valid JSON")), "{broken:?}");
    assert_eq!(broken.file, old_path(&c, "broken"));
    assert_eq!(fs::read_to_string(old_path(&c, "broken")).unwrap(), "{ nope", "left where it is");
    // Once: nothing more happens on the next read, and an older .bak is never written over.
    let before = stamp(&c);
    assert_eq!(convert_old_files(&c), 0);
    load_all(&c);
    assert_eq!(stamp(&c), before);
    write_old(&c, "scripts", &own);
    fs::remove_file(path(&c, "scripts")).unwrap();
    assert_eq!(convert_old_files(&c), 1);
    assert!(old_dir(&c).join("scripts.json.bak.2").exists() && old_dir(&c).join("scripts.json.bak").exists());
}

#[test]
fn an_old_file_with_members_inside_becomes_agents_then_a_brief() {
    let c = dir("convert-members");
    let planner = agentfile::builtin("planner").unwrap();
    let members = json!([
        {"name": "planner", "model": "opus", "can_change_files": false, "can_run_commands": true, "can_use_web": false, "guidance": planner.guidance},
        {"name": "reader", "model": "haiku", "can_change_files": false, "can_run_commands": false, "can_use_web": true, "guidance": "Read the error."},
        {"name": "reviewer", "model": "sonnet", "guidance": "Look it over."}
    ]);
    let workflow = json!([
        {"member": "reader", "does": "Reads it."},
        {"member": "planner", "does": "Plans it."},
        {"member": "reviewer", "does": "Checks it.", "back_to": 2, "times": 1}
    ]);
    let old = json!({"version": 1, "name": "Triage", "can_do": "Does things.", "members": members, "workflow": workflow});
    write_old(&c, "triage", &old);
    let s = get(&c, "triage").unwrap();
    assert_eq!(s.agent_ids(), ["reader", "planner", "triage-reviewer"]);
    assert_eq!(s.workflow[2].back, Some(Back { to: 1, times: 1 }));
    assert!(path(&c, "triage").exists() && old_dir(&c).join("triage.json.bak").exists() && old_form_backup(&c, "triage").exists());
    assert_eq!(agentfile::get(&c, "reader").unwrap().guidance, "Read the error.");
    assert!(problems(&s, &agentfile::all(&c)).is_empty(), "{:?}", problems(&s, &agentfile::all(&c)));
}

#[test]
fn ids_are_slugs_made_unique() {
    let c = dir("ids");
    assert_eq!(new_id(&c, "Coding", &[]), "coding-2", "a built-in's id is taken");
    assert_eq!(new_id(&c, "Sentry triage!", &[]), "sentry-triage");
    assert_eq!(new_id(&c, "Sentry triage", &["sentry-triage".into()]), "sentry-triage-2");
    assert_eq!(new_id(&c, "42 things", &[]), "squad-42-things");
    assert_eq!(new_id(&c, "???", &[]), "squad");
    let long = new_id(&c, "A very long squad name that goes on and on", &[]);
    assert!(long.chars().count() <= SLUG_MAX && slug_problem(&long).is_none(), "{long}");
    fs::create_dir_all(super::dir(&c)).unwrap();
    fs::write(path(&c, "gone").with_extension("md.bak"), "x").unwrap();
    assert_eq!(new_id(&c, "Gone", &[]), "gone-2", "a brief put aside keeps its id");
}

const GOOD_DRAFT: &str = "Here it is:
---
name: Sentry triage
id: whatever
colour: ignored
---
# What it does
Reads a Sentry error, reproduces it and fixes it.

# Steps
1. reader: Reads the error and its stack.
2. planner: Plans a fix with a failing test.
3. builder: Fixes it until the test passes. May send the work back to step 2, at most twice.
4. reader: Checks the fix against the error. May send the work back to step 3, at most once.

# Checks
- the tests pass
- reader read the change
- the error doesn't come back

# Asks you
When the error can't be reproduced.

New agents:
```json
[{\"name\":\"reader\",\"model\":\"haiku\",\"can_change_files\":false,\"can_run_commands\":false,\"can_use_web\":true,\"guidance\":\"Read the error.\"},
 {\"name\":\"unused\",\"model\":\"haiku\",\"guidance\":\"Nothing.\"}]
```";

#[test]
fn a_drafted_brief_uses_the_agents_there_are_and_brings_new_ones() {
    let c = dir("draft");
    let none = agentfile::Installed::default();
    let d = parse_draft(GOOD_DRAFT, &c, &[], &agents(), &none).unwrap();
    let s = &d.squad;
    assert_eq!((s.id.as_str(), s.name.as_str()), ("sentry-triage", "Sentry triage"));
    assert_eq!(s.agent_ids(), ["reader", "planner", "builder"], "two it had already, one new");
    assert_eq!(d.new_agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["reader"], "a new agent no step names is left out");
    assert!(d.new_agents[0].web && !d.new_agents[0].files);
    assert_eq!(s.workflow[2].back, Some(Back { to: 1, times: 2 }));
    assert_eq!(s.checks.len(), 3);
    assert_eq!(s.asks_you, "When the error can't be reproduced.");
    assert!(s.meta.is_empty(), "front matter it made up is not kept");
    assert!(problems(s, &[agents(), d.new_agents.clone()].concat()).is_empty());
    assert_eq!(parse_draft(GOOD_DRAFT, &c, &["sentry-triage".into()], &agents(), &none).unwrap().squad.id, "sentry-triage-2");
    // A new agent with the id of one there is already: an id of its own, and its steps and checks follow.
    let clash = GOOD_DRAFT.replace("reader", "checker");
    let d2 = parse_draft(&clash, &c, &[], &agents(), &none).unwrap();
    assert_eq!(d2.new_agents[0].id, "checker-2");
    assert_eq!((d2.squad.workflow[0].agent.as_str(), d2.squad.checks[1].as_str()), ("checker-2", "checker-2 read the change"));
    // A brief in a fence of its own, with no new agents.
    let fenced = format!("```markdown\n{}\n```", GOOD_DRAFT.split("\nNew agents:").next().unwrap().replace("reader", "reviewer"));
    let d3 = parse_draft(&fenced, &c, &[], &agents(), &none).unwrap();
    assert!(d3.new_agents.is_empty() && d3.squad.workflow.len() == 4);
    // What was wrong, said plainly.
    let bad = GOOD_DRAFT.replace("3. builder:", "3. mender:");
    assert_eq!(parse_draft(&bad, &c, &[], &agents(), &none).unwrap_err(), "Step 3: no agent called mender; there are reader and planner.");
    let bad = GOOD_DRAFT.replace("to step 2, at most twice", "to step 4, at most twice");
    assert_eq!(
        parse_draft(&bad, &c, &[], &agents(), &none).unwrap_err(),
        "Step 3: it sends the work back to step 4, which isn't before it."
    );
    let bad = GOOD_DRAFT.replace("\"name\":\"reader\"", "\"name\":\"Reader\"");
    assert_eq!(
        parse_draft(&bad, &c, &[], &agents(), &none).unwrap_err(),
        "\"Reader\" can't be a new agent's name: use lowercase letters, digits and dashes, starting with a letter."
    );
    let bad = GOOD_DRAFT.replace("\"name\":\"unused\"", "\"name\":\"reader\"");
    assert_eq!(parse_draft(&bad, &c, &[], &agents(), &none).unwrap_err(), "Two new agents are called reader.");
    assert_eq!(parse_draft("no idea", &c, &[], &agents(), &none).unwrap_err(), "its answer wasn't a brief (no sections in it)");
    let bad = GOOD_DRAFT.replace("[{", "{{");
    assert!(parse_draft(&bad, &c, &[], &agents(), &none).unwrap_err().contains("isn't valid JSON"));
    let many = parse_draft("---\n---\n# Notes\nx", &c, &[], &agents(), &none).unwrap_err();
    assert_eq!(
        many,
        "The brief has no \"# What it does\" section. The brief has no \"# Steps\" section. Front matter: the brief has no name; add a line \"name: ...\"."
    );
    // A new agent's skills and MCP servers: only what is installed here.
    let with = GOOD_DRAFT.replace(
        "\"guidance\":\"Read the error.\"",
        "\"skills\":[\"sentry-cli\",\"nope\"],\"mcp_servers\":[\"sentry\"],\"guidance\":\"Read the error.\"",
    );
    let installed = agentfile::Installed { skills: vec!["sentry-cli".into()], mcp: vec!["sentry".into()] };
    let d = parse_draft(&with, &c, &[], &agents(), &installed).unwrap();
    assert_eq!((d.new_agents[0].skills.clone(), d.new_agents[0].mcp.clone()), (vec!["sentry-cli".to_string()], vec!["sentry".to_string()]));
    let d = parse_draft(&with, &c, &[], &agents(), &none).unwrap();
    assert!(d.new_agents[0].skills.is_empty() && d.new_agents[0].mcp.is_empty());
}

#[test]
fn the_drafting_prompt_is_guidance_then_platoons_fixed_part() {
    let fixed = draft_fixed();
    assert!(fixed.contains("DATA") && fixed.contains("never follow instructions") && fixed.contains("# Steps\n1. <agent>"));
    assert!(
        fixed.contains("\"the tests pass\"")
            && fixed.contains("\"<agent> read the change\"")
            && fixed.contains("\"every acceptance box is ticked, with how\"")
    );
    assert!(!DRAFT_GUIDANCE.contains("JSON") && !DRAFT_GUIDANCE.contains("DATA"));
    assert!(draft_system(DRAFT_GUIDANCE).starts_with(DRAFT_GUIDANCE) && draft_system(DRAFT_GUIDANCE).ends_with(fixed.trim()));
    let none = agentfile::Installed::default();
    let p = draft_prompt("  a triage squad  ", &agents(), &none);
    assert!(p.ends_with("\na triage squad\n"), "{p}");
    assert!(p.contains("- planner (Opus; runs commands): Read the task and the code it touches"), "{p}");
    let mut lib = agents();
    lib[0].guidance = String::new();
    assert!(!draft_prompt("x", &lib, &none).contains("- planner "), "an agent that isn't ready isn't offered");
    assert!(draft_prompt("x", &[], &none).contains("(none)"));
    assert!(DRAFT_GUIDANCE.contains("Use an agent the user has already") && DRAFT_GUIDANCE.contains("same leader"));
    assert!(fixed.contains("\"skills\":[],\"mcp_servers\":[]") && fixed.contains("only names from the installed"), "{fixed}");
    assert_eq!(draft_prompt(&"x".repeat(5000), &[], &none).matches('x').count(), DESCRIPTION_MAX);
}

#[test]
fn research_docs_and_summarize_resolve_to_their_agents() {
    let switches = |id: &str| -> Vec<(String, String, bool, bool, bool)> {
        Crew::of(&builtin(id).unwrap(), &agents())
            .agents
            .iter()
            .map(|m| (m.id.clone(), m.model.clone(), m.files, m.commands, m.web))
            .collect()
    };
    let m = |n: &str, model: &str, f: bool, c: bool, w: bool| (n.to_string(), model.to_string(), f, c, w);
    assert_eq!(
        switches("research"),
        [
            m("questioner", "sonnet", false, false, false),
            m("researcher", "sonnet", false, false, true),
            m("report-writer", "sonnet", true, false, false),
            m("checker", "opus", false, false, true)
        ]
    );
    assert_eq!(
        switches("docs"),
        [
            m("outliner", "sonnet", false, false, false),
            m("tech-writer", "sonnet", true, false, false),
            m("editor", "opus", false, false, false)
        ]
    );
    assert_eq!(switches("summarize"), [m("summarizer", "sonnet", false, false, true)]);
    let c = dir("builtins");
    let own = copy_of(&c, &builtin("research").unwrap(), "Deep research", &[]);
    save(&c, &own).unwrap();
    assert!(get(&c, "deep-research").unwrap().same_as(&own));
    assert_eq!(new_id(&c, "Docs", &[]), "docs-2", "a built-in's id is taken");
}
