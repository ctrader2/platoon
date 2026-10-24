//! A task whose squad's model is one of Codex's, at the level of the app: `s` and enter make its worktree, have Codex
//! itself trust that folder (a stand-in app-server records what it was sent), and start the session in Codex with the
//! task in its environment; its session id waits for Codex to say it; Codex unreachable still starts the task and says
//! so; the worktree's removal after the work takes Codex's trust away again. Real git in temp repositories; nothing is
//! started (a unit test's session is only noted), and the real `codex` is never run.

use crate::agents::{CodexAt, Providers};
use crate::app::App;
use crate::testkit::{CodexServerFake, World};
use crate::tickets::{self, NewTicket, State, Tickets};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn draw(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A project "Web shop" in the temp repository `shop`, its task #1 suggesting the user's own squad "Mine" (on
/// GPT-6-Luna, planning first), and an app with Claude Code and Codex, Codex's program a stand-in app-server that
/// answers config reads and writes.
struct Lab {
    w: World,
    app: App,
    project: String,
    shop: PathBuf,
    fake: CodexServerFake,
}

fn lab(name: &str) -> Lab {
    let w = World::new(name);
    let shop = w.base.join("repos/shop");
    fs::create_dir_all(&shop).unwrap();
    fs::write(shop.join("README"), "hi\n").unwrap();
    git(&shop, &["init", "-q"]);
    git(&shop, &["add", "."]);
    git(&shop, &["commit", "-q", "-m", "first"]);
    fs::create_dir_all(w.config.join("squads")).unwrap();
    fs::write(
        w.config.join("squads/mine.md"),
        "---\nname: Mine\ndescription: Ours\nmodel: gpt-6-luna\nverdict_model: gpt-5.6-terra\nplan_first: true\n---\n\n1. Ask the user, then do it.\n",
    )
    .unwrap();
    let mut app = w.app();
    let project = app.projects.create("Web shop").unwrap();
    let mut store = Tickets::open(&w.config, &project);
    store
        .add(NewTicket {
            title: "Add a basket".into(),
            folder: Some(shop.to_string_lossy().into_owned()),
            squad: "mine".into(),
            ..Default::default()
        })
        .unwrap();
    let codex = crate::agents::tests::codex_with_models(name);
    let fake = CodexServerFake::new(name, &codex.root);
    fake.result(
        "config/read",
        None,
        json!({"config": {}, "origins": {}, "layers": [{"name": {"type": "user"}, "version": "sha256:v1", "config": {}}]}),
    );
    fake.result("config/batchWrite", None, json!({"status": "ok", "version": "sha256:v2"}));
    app.set_providers_for_test(Providers {
        claude: Some("/nonexistent/platoon-unit-test/claude".into()),
        codex: Some(CodexAt { program: fake.program.clone(), root: codex.root }),
    });
    app.reload_for_test();
    key(&mut app, KeyCode::Char('3'));
    Lab { w, app, project, shop, fake }
}

impl Lab {
    fn start(&mut self) {
        key(&mut self.app, KeyCode::Char('s'));
        assert_eq!(self.app.sq.pick[self.app.sq.pick_selected].id, "mine");
        key(&mut self.app, KeyCode::Enter);
        self.app.refresh_until_for_test(20, "the session started", |a| a.sq.launched.contains(&1));
    }

    fn ticket(&self) -> crate::tickets::Ticket {
        Tickets::open(&self.w.config, &self.project).get(1).cloned().unwrap()
    }
}

#[test]
fn a_task_on_a_codex_model_starts_in_codex_in_its_trusted_worktree_with_the_task_in_its_environment() {
    let mut l = lab("codex-task-start");
    l.start();
    let t = l.ticket();
    assert_eq!(t.state, State::Working);
    // Its session's id is Codex's to say: until then the task names it by its tag.
    let tag = tickets::task_tag(&l.project, 1);
    assert_eq!(t.session.as_deref(), Some(format!("pending:{tag}").as_str()));
    assert_eq!(t.session_id(), None);
    let w = t.worktree.clone().unwrap();
    let root = crate::worktrees::root_of(&w, 1);
    assert!(root.ends_with("shop-worktrees/task-1") && root.is_dir(), "{root:?}");
    // Codex was asked to trust the worktree's folder, exactly as its own window would, and the task says so.
    let folder_key = root.canonicalize().unwrap().to_string_lossy().into_owned();
    assert_eq!(w.codex_trust.as_deref(), Some(folder_key.as_str()));
    assert_eq!(l.fake.methods(), ["initialize", "initialized", "config/read", "config/batchWrite"]);
    assert_eq!(
        l.fake.requests()[3]["params"],
        json!({"edits": [{"keyPath": format!("projects.\"{folder_key}\".trust_level"), "value": "trusted", "mergeStrategy": "replace"}], "reloadUserConfig": true})
    );
    // What it was started with: Codex's sandbox, the network, the main repository's git folder, the rules and steps.
    let (dir, args) = &l.app.sq.launched_with[0];
    assert_eq!(dir, &w.path);
    assert_eq!(&args[..7], ["--no-daemon", "-m", "gpt-6-luna", "-s", "workspace-write", "-a", "on-request"]);
    let git_dir = l.shop.join(".git").canonicalize().unwrap();
    let set = |k: &str| args.iter().find_map(|a| a.strip_prefix(&format!("{k}="))).map(String::from);
    assert_eq!(set("check_for_update_on_startup").as_deref(), Some("false"));
    assert_eq!(set("sandbox_workspace_write.network_access").as_deref(), Some("true"));
    assert_eq!(set("sandbox_workspace_write.writable_roots"), Some(format!("[\"{}\"]", git_dir.display())));
    let instructions: toml::Table = format!("x={}", set("developer_instructions").unwrap()).parse().unwrap();
    let instructions = instructions["x"].as_str().unwrap();
    assert!(
        instructions.contains("request_user_input") && instructions.contains("Plan first.") && instructions.contains("# The squad: Mine")
    );
    assert!(!instructions.contains("AskUserQuestion"));
    assert!(args.last().unwrap().starts_with("Task #1: Add a basket"), "the first message: {args:?}");
    assert!(!args.iter().any(|a| a == "--session-id" || a == "-n" || a == "resume"), "{args:?}");
    let env = &l.app.sq.launched_env[0];
    assert!(env.contains(&("PLATOON_TASK".to_string(), tag.clone())) && env.contains(&("PLATOON_PROJECT".to_string(), l.project.clone())));
    let task_dir = tickets::task_dir(&l.w.config, &l.project, 1);
    assert!(task_dir.join(tickets::PLAN_PENDING_FILE).exists(), "its edits wait for the plan");
    // The board and the details read as for any task, naming the program it runs in.
    let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();
    let shown = draw(&mut term, &l.app);
    assert!(shown.contains("Add a basket") && shown.contains("Mine · in Codex"), "{shown}");
    // Its host is found by the task it was started for, before (and after) Codex says the session's id.
    l.app.hosts.push(crate::host::HostInfo {
        agent: crate::agents::Agent::Codex,
        claude_pid: 4_000_000,
        host_pid: 4_000_001,
        dir: w.path.clone(),
        args: args.clone(),
        sock: l.w.base.join("none.sock"),
        task: Some(tag.clone()),
    });
    assert_eq!(l.app.task_host(&l.project, &t).map(|h| h.claude_pid), Some(4_000_000));
    assert!(l.app.task_running(&l.project, &t));
    l.app.hosts.clear();
    // enter before Codex said its id (nothing runs it in a unit test): it starts afresh, never `resume` of nothing.
    key(&mut l.app, KeyCode::Enter);
    l.app.refresh_until_for_test(10, "started again", |a| a.sq.launched.len() == 2);
    let (_, again) = &l.app.sq.launched_with[1];
    assert!(again[0] == "--no-daemon" && again.last().unwrap().starts_with("Task #1"), "{again:?}");
    // Once Codex said it (its first hook), enter continues that conversation.
    Tickets::open(&l.w.config, &l.project).record_session(1, "01a1aaaa-0000-7000-8000-000000000001").unwrap();
    l.app.reload_for_test();
    key(&mut l.app, KeyCode::Enter);
    l.app.refresh_until_for_test(10, "continued", |a| a.sq.launched.len() == 3);
    let (_, resumed) = &l.app.sq.launched_with[2];
    assert_eq!(&resumed[..3], ["resume", "01a1aaaa-0000-7000-8000-000000000001", "--no-daemon"]);
}

#[test]
fn when_codex_cant_be_asked_the_task_starts_all_the_same_and_codex_asks_in_the_window() {
    let mut l = lab("codex-task-unreachable");
    fs::write(l.fake.dir.join("exit-at-once"), "").unwrap();
    l.start();
    let t = l.ticket();
    assert_eq!(t.state, State::Working);
    assert_eq!(t.worktree.as_ref().unwrap().codex_trust, None, "nothing of platoon's in Codex's settings");
    let said = l.app.active_notice().unwrap_or_default().to_string();
    assert!(
        said.starts_with("#1 started, but platoon couldn't ask Codex to trust ")
            && said.contains("shop-worktrees/task-1")
            && said.ends_with("Codex asks you in the window whether you trust it."),
        "{said}"
    );
}

#[test]
fn removing_a_done_tasks_worktree_takes_codexs_trust_in_it_away() {
    let mut l = lab("codex-task-remove");
    l.start();
    let key_was = l.ticket().worktree.unwrap().codex_trust.unwrap();
    // Done, its session stopped (none runs in a unit test): the worktree goes, and so does the trust.
    let mut store = Tickets::open(&l.w.config, &l.project);
    store.record_session(1, "01a1aaaa-0000-7000-8000-000000000001").unwrap();
    store
        .update(1, |t| {
            t.state = State::Done;
            true
        })
        .unwrap();
    let projects = json!({(key_was.clone()): {"trust_level": "trusted"}, "/home/u/mine": {"trust_level": "trusted"}});
    l.fake.result("config/read", None, json!({"config": {}, "origins": {}, "layers": [{"name": {"type": "user"}, "version": "sha256:v9", "config": {"projects": projects}}]}));
    l.app.reload_for_test();
    let (config, project) = (l.w.config.clone(), l.project.clone());
    l.app.refresh_until_for_test(20, "the worktree removed and its trust gone", |_| {
        Tickets::open(&config, &project)
            .get(1)
            .and_then(|t| t.worktree.clone())
            .is_some_and(|w| w.removed.is_some() && w.codex_trust.is_none())
    });
    let sent: Vec<Value> = l.fake.requests().into_iter().filter(|r| r["method"] == "config/batchWrite").collect();
    assert_eq!(
        sent.last().unwrap()["params"],
        json!({"edits": [{"keyPath": format!("projects.\"{key_was}\""), "value": null, "mergeStrategy": "replace"}],
               "reloadUserConfig": true, "expectedVersion": "sha256:v9"}),
        "only platoon's entry, only if the file is as it was read"
    );
}
