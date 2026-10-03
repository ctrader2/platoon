//! The Squads tab end to end, in a real terminal with the real binary: a squad described in the text box and drafted
//! by a stand-in `claude`, saved, handed a ticket with `S` (the leader starts with that squad's own `--agents`), then
//! changed on the tab (and the leaders' model in the settings) while its leader has stopped: continued, the leader runs
//! with the squad as it was when it started. The squad leaders' system prompt and model, set in the settings, reaching
//! a leader. An agent given a skill and an MCP server from what is installed (in a temp Claude config folder).
//!
//! The "claude" is a stand-in script: `claude -p` answers the drafting call and the commander's check from files,
//! and a session writes down its arguments and echoes what is typed. A real `claude` or editor is never run, every
//! directory is a temp dir (the git repository too), and the only processes signalled are the ones these tests
//! started.

mod common;
use common::*;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const FAKE: &str = r#"#!/bin/sh
if [ "$1" = "-p" ]; then
  mkdir -p "$FAKE_DIR/calls"
  k=$(( $(ls "$FAKE_DIR/calls" | grep -c '\.stdin$') + 1 ))
  for a in "$@"; do printf '%s\n' "$a"; done > "$FAKE_DIR/calls/$k.argv"
  cat > "$FAKE_DIR/calls/$k.stdin"
  if grep -q 'The squad the user wants' "$FAKE_DIR/calls/$k.stdin"; then
    text=$(cat "$FAKE_DIR/draft")
  else
    text='{"fits":true,"reason":"Coding work."}'
  fi
  esc=$(printf '%s' "$text" | tr '\n' ' ' | sed 's/\\/\\\\/g; s/"/\\"/g')
  printf '{"type":"result","subtype":"success","is_error":false,"result":"%s","total_cost_usd":0.001,"usage":{"input_tokens":300,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":20}}' "$esc"
  exit 0
fi
mkdir -p "$FAKE_DIR/started"
n=$(( $(ls "$FAKE_DIR/started" | wc -l) + 1 ))
d="$FAKE_DIR/started/$n"
mkdir -p "$d"
i=0
for a in "$@"; do i=$((i+1)); printf '%s' "$a" > "$d/arg$i"; done
echo "$i" > "$d/argc"
echo $$ > "$d/pid"
sid=""
prev=""
for a in "$@"; do
  [ "$prev" = "--session-id" ] && sid="$a"
  [ "$prev" = "--resume" ] && sid="$a"
  prev="$a"
done
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","kind":"interactive","startedAt":%s,"status":"idle"}' $$ "$sid" "$(pwd)" "$(date +%s%3N)" > "$CLAUDE_CONFIG_DIR/sessions/$$.json"
echo "FAKE LEADER sid:$sid"
while IFS= read -r line; do
  echo "GOT:$line"
done
"#;

/// Stands in for nano: says which file it has, then adds the line it is given to the end of it.
const FAKE_NANO: &str = r#"#!/bin/sh
f=""
for a in "$@"; do case "$a" in -*) ;; *) f="$a" ;; esac; done
echo "FAKE NANO EDITING $f"
read line
printf '%s\n' "$line" >> "$f"
"#;

/// A drafted squad that uses the built-in planner and brings a builder of its own.
const DRAFT: &str = r#"{"name":"Careful coding","can_do":"Makes small, careful changes to code in a git repository.",
 "new_agents":[
  {"name":"quick-builder","model":"haiku","can_change_files":true,"can_run_commands":true,"can_use_web":false,"guidance":"Make the change."}],
 "workflow":[
  {"agent":"planner","does":"Writes a short plan.","back_to":null,"times":0},
  {"agent":"quick-builder","does":"Makes the change and runs the tests.","back_to":null,"times":0},
  {"agent":"planner","does":"Checks the change against the plan.","back_to":2,"times":2}]}"#;

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    config: PathBuf,
    work: PathBuf,
    shop: PathBuf,
    fake: PathBuf,
    bin: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

impl Lab {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let lab = Self {
            claude: base.join("claude"),
            config: base.join("config"),
            work: base.join("work"),
            shop: base.join("shop"),
            fake: base.join("fake"),
            bin: base.join("bin"),
            base,
        };
        for d in [
            &lab.claude.join("sessions"),
            &lab.claude.join("projects"),
            &lab.base.join("cache"),
            &lab.config,
            &lab.work,
            &lab.shop,
            &lab.fake,
            &lab.bin,
        ] {
            fs::create_dir_all(d).unwrap();
        }
        for (path, text) in [(lab.base.join("fake-claude.sh"), FAKE), (lab.bin.join("nano"), FAKE_NANO)] {
            fs::write(&path, text).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(lab.fake.join("draft"), DRAFT).unwrap();
        fs::write(lab.config.join("settings.json"), r#"{"auto_archive_days":0,"tokens_enabled":true,"tokens_asked":true}"#).unwrap();
        fs::write(lab.shop.join("README"), "a shop\n").unwrap();
        git(&lab.shop, &["init", "-q"]);
        git(&lab.shop, &["add", "."]);
        git(&lab.shop, &["commit", "-q", "-m", "first"]);
        let v = json!({"version": 1, "projects": {"pweb": {"name": "Web", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []}}, "sessions": {}});
        fs::write(lab.config.join("projects.json"), v.to_string()).unwrap();
        fs::create_dir_all(lab.config.join("projects/pweb/tickets")).unwrap();
        let t = json!({"number": 1, "title": "Basket", "file": "01-basket.md", "state": "new", "folders": [lab.shop.to_string_lossy()], "blocked_by": [], "created": 1, "updated": 1});
        fs::write(lab.config.join("projects/pweb/tickets/01-basket.md"), "A basket page.\n").unwrap();
        fs::write(lab.config.join("projects/pweb/tickets.json"), json!({"next": 2, "tickets": [t]}).to_string()).unwrap();
        lab
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.claude.to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.base.join("cache").to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.config.to_string_lossy().into()),
            ("PLATOON_CLAUDE_BIN", self.base.join("fake-claude.sh").to_string_lossy().into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            ("PLATOON_NOTES_HELPER", "off".into()),
            ("PLATOON_EDITOR_PATH", self.bin.to_string_lossy().into()),
            ("EDITOR", String::new()),
            ("VISUAL", String::new()),
            ("FAKE_DIR", self.fake.to_string_lossy().into()),
        ]
    }

    fn squad_file(&self, id: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(self.config.join("squads").join(format!("{id}.json"))).unwrap()).unwrap()
    }

    fn ticket_now(&self) -> Value {
        let v: Value = serde_json::from_str(&fs::read_to_string(self.config.join("projects/pweb/tickets.json")).unwrap()).unwrap();
        v["tickets"][0].clone()
    }

    fn started(&self) -> usize {
        // Only starts that have finished recording: the stand-in writes `pid` after `argc`.
        fs::read_dir(self.fake.join("started")).map(|d| d.flatten().filter(|e| e.path().join("pid").exists()).count()).unwrap_or(0)
    }

    fn args(&self, k: usize) -> Vec<String> {
        let d = self.fake.join("started").join(k.to_string());
        let n: usize = fs::read_to_string(d.join("argc")).unwrap().trim().parse().unwrap();
        (1..=n).map(|i| fs::read_to_string(d.join(format!("arg{i}"))).unwrap()).collect()
    }

    fn pid(&self, k: usize) -> u32 {
        fs::read_to_string(self.fake.join("started").join(k.to_string()).join("pid")).unwrap().trim().parse().unwrap()
    }

    fn call(&self, k: usize, what: &str) -> String {
        fs::read_to_string(self.fake.join("calls").join(format!("{k}.{what}"))).unwrap_or_default()
    }

    /// End the fake claudes the hosts run, by the exact pids they published.
    fn end_hosts(&self) {
        for e in fs::read_dir(self.base.join("cache/hosts")).into_iter().flatten().flatten() {
            if e.path().extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(v) = fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
            if let Some(pid) = v["claude_pid"].as_u64().map(|p| p as u32).filter(|p| *p > 1) {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                until("the fake claude to exit", || !alive(pid));
            }
        }
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        self.end_hosts();
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn value_after<'a>(args: &'a [String], flag: &str) -> &'a str {
    let at = args.iter().position(|a| a == flag).unwrap_or_else(|| panic!("no {flag} in {args:?}"));
    &args[at + 1]
}

#[test]
fn a_described_squad_is_drafted_saved_handed_a_ticket_and_continued_with_the_squad_it_started_with() {
    let lab = Lab::new("sqtab-draft");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    t.wait_for("enter open");
    t.send(b"3");
    t.wait_for_all(&["╭ [Squads]", "╭ Coding", "n describe a new squad"]);
    t.send(b"n");
    t.wait_for_all(&["What to describe for Claude to draft:", "A new squad", "A new agent"]);
    t.send(b"\r");
    t.wait_for_all(&["describe a new squad", "enter ask Claude to draft it"]);
    t.send(b"a squad that codes carefully: a planner and a builder\r");
    t.wait_for_all(&["Draft, not saved", "Careful coding", "quick-builder", "Makes the change and runs the tests."]);
    let asked = lab.call(1, "stdin");
    assert!(asked.contains("a squad that codes carefully: a planner and a builder"), "{asked}");
    assert!(asked.contains("- planner (Opus; runs commands): "), "told the agents there are: {asked}");
    assert!(lab.call(1, "argv").contains("\n--model\nsonnet\n"), "drafted on Sonnet: {}", lab.call(1, "argv"));
    assert!(!lab.config.join("squads/careful-coding.json").exists(), "not saved until the user says so");
    t.send(b"\x13");
    t.wait_for_flat("Saved the squad \"Careful coding\" and its new agent.");
    let v = lab.squad_file("careful-coding");
    assert_eq!((v["name"].as_str(), v["workflow"][1]["agent"].as_str()), (Some("Careful coding"), Some("quick-builder")));
    let a: Value = serde_json::from_str(&fs::read_to_string(lab.config.join("agents/quick-builder.json")).unwrap()).unwrap();
    assert_eq!(a["model"], "haiku");
    t.wait_for_all(&["Your own"]);

    // Hand the ticket to it: the picker lists every squad by name.
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for_all(&["╭ Sessions", "╭ Tickets", "╭ Notes"]);
    t.send(b"\t");
    t.wait_for_flat("S hand to a squad");
    t.send(b"S");
    t.wait_for_all(&["hand to a squad", "Coding", "Careful coding", "Makes small, careful changes"]);
    // Coding, Research, Docs and Summarize come first.
    t.send(b"\x1b[B\x1b[B\x1b[B\x1b[B\r");
    t.wait_for_all(&["FAKE LEADER sid:", "ctrl+space back to the project"]);
    assert!(
        lab.call(2, "stdin").starts_with("The squad: Careful coding\nWhat it can do: Makes small, careful changes"),
        "{}",
        lab.call(2, "stdin")
    );
    let first = lab.args(1);
    assert_eq!(value_after(&first, "--agent"), "careful-coding-leader");
    assert_eq!(value_after(&first, "--model"), "sonnet");
    assert_eq!(value_after(&first, "-n"), "Careful coding squad · #1 Basket");
    let agents: Value = serde_json::from_str(value_after(&first, "--agents")).unwrap();
    assert_eq!(
        agents.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(),
        ["careful-coding-leader", "careful-coding-planner", "careful-coding-quick-builder"]
    );
    assert_eq!(agents["careful-coding-quick-builder"]["model"], "haiku");
    assert_eq!(agents["careful-coding-quick-builder"]["tools"], json!(["Read", "Grep", "Glob", "Edit", "Write", "Bash"]));
    assert_eq!(agents["careful-coding-planner"]["model"], "opus", "the built-in planner");
    assert_eq!(agents["careful-coding-planner"]["tools"], json!(["Read", "Grep", "Glob", "Bash"]));
    let prompt = agents["careful-coding-leader"]["prompt"].as_str().unwrap();
    assert!(prompt.starts_with("You are given one ticket: a piece of work in a project."), "the leaders' shared system prompt: {prompt}");
    assert!(prompt.contains("You lead the Careful coding squad. Your members are careful-coding-planner and careful-coding-quick-builder.\nWhat the squad can do: Makes small, careful changes"), "{prompt}");
    assert!(prompt.contains("3. careful-coding-planner: Checks the change against the plan. It may send the work back: then give its reasons to careful-coding-quick-builder and go on again from step 2. At most twice;"), "{prompt}");
    assert_eq!(lab.ticket_now()["leaders"][0]["squad"], "careful-coding");

    // The squad changes on the tab (what it can do), and every leader runs on Opus from now on (the settings file).
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(b"3");
    t.wait_for_all(&["╭ [Squads]", "Careful coding"]);
    // To the list (shift+left), on the new squad, then into its details from the top, down to what it can do.
    t.send(b"\x1b[1;2D");
    t.wait_for_all(&["enter details", "╭ Careful coding"]);
    t.send(b"\rg\x1b[B\r");
    t.wait_for("what it can do");
    t.send(b"\x15Makes big, bold changes.\r");
    t.wait_for_flat("Saved what the squad can do.");
    assert_eq!(lab.squad_file("careful-coding")["can_do"], "Makes big, bold changes.");
    let mut settings: Value = serde_json::from_str(&fs::read_to_string(lab.config.join("settings.json")).unwrap()).unwrap();
    settings.as_object_mut().unwrap().insert("squad_leader_model".into(), json!("opus"));
    fs::write(lab.config.join("settings.json"), settings.to_string()).unwrap();

    // Its session stops; continued, it runs with the squad as it was when it started.
    let pid = lab.pid(1);
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    until("the fake leader to exit", || !alive(pid));
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for("╭ Tickets");
    t.send(b"\t");
    t.wait_for_flat("c cancel");
    t.send(b"\r");
    t.wait_for_all(&["The squad's session isn't running. Enter to continue it.", "Careful coding squad"]);
    t.send(b"\r");
    t.wait_for_all(&["FAKE LEADER sid:", "ctrl+space back to the project"]);
    until("the second start", || lab.started() == 2);
    let again = lab.args(2);
    assert_eq!(value_after(&again, "--resume"), value_after(&first, "--session-id"));
    assert_eq!(value_after(&again, "--model"), "sonnet", "not the Opus the settings say since");
    assert_eq!(value_after(&again, "--agents"), value_after(&first, "--agents"), "the very same squad");
    assert_eq!(value_after(&again, "--agent"), "careful-coding-leader");
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
    t.wait_exit();
}

#[test]
fn the_squad_leaders_system_prompt_and_model_are_set_in_the_settings_and_reach_every_leader() {
    let lab = Lab::new("sqtab-prompt");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    t.wait_for("enter open");
    // A squad has no leader of its own on the Squads tab any more.
    t.send(b"3");
    t.wait_for_all(&["╭ [Squads]", "Built in", "Every squad has the same leader"]);
    let screen = t.stable();
    assert!(!screen.contains("Needs a folder") && !screen.contains("Use a worktree"), "{screen}");
    t.send(b"\t");
    t.wait_for("esc back to the list");
    t.send(b"\x1b");
    t.wait_gone("esc back to the list");
    // Settings, the Projects tab (before Editors and Plugins, the last): the leaders' model and system prompt, just after
    // the commander's.
    t.send(b"s");
    t.wait_for("Settings file:");
    t.send(b"\x1b[Z");
    t.wait_for("No plugins installed, and none from the catalog to show.");
    t.send(b"\x1b[Z");
    t.wait_for("Editing files");
    t.send(b"\x1b[Z");
    t.wait_for_all(&[
        "System prompt for the squad commander",
        "System prompt for squad leaders",
        "Model for squad leaders",
        "System prompt for drafting a squad",
    ]);
    t.send(b"G");
    t.wait_for("How platoon asks Claude to draft one agent");
    t.send(b"\x1b[A\x1b[A");
    t.wait_for("The model every squad's leader runs on.");
    t.send(b"\x1b[D");
    until("the leaders' model to be saved", || {
        fs::read_to_string(lab.config.join("settings.json")).unwrap_or_default().contains("\"squad_leader_model\": \"opus\"")
    });
    t.send(b"\x1b[A");
    t.wait_for_all(&["How platoon asks Claude to lead a squad through a ticket", "enter edit"]);
    t.send(b"\r");
    t.wait_for("╭ edit the system prompt with");
    t.send(b"\r");
    t.wait_for_all(&["FAKE NANO EDITING", "squad-leader.md"]);
    t.send(b"Keep the reports to five lines.\r");
    t.wait_for_all(&["Saved: platoon uses your system prompt for squad leaders", "Settings file:"]);
    let g = fs::read_to_string(lab.config.join("prompts/squad-leader.md")).unwrap();
    assert!(
        g.starts_with("You are given one ticket: a piece of work in a project.") && g.ends_with("Keep the reports to five lines.\n"),
        "{g}"
    );
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    // Handed a ticket, Coding's leader has both.
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for_all(&["╭ Sessions", "╭ Tickets", "╭ Notes"]);
    t.send(b"\t");
    t.wait_for_flat("S hand to a squad");
    t.send(b"S");
    t.wait_for_all(&["hand to a squad", "Coding", "enter hand it over"]);
    t.send(b"\r");
    t.wait_for_all(&["FAKE LEADER sid:", "ctrl+space back to the project"]);
    let args = lab.args(1);
    assert_eq!((value_after(&args, "--agent"), value_after(&args, "--model")), ("coding-leader", "opus"));
    let agents: Value = serde_json::from_str(value_after(&args, "--agents")).unwrap();
    assert_eq!(agents["coding-leader"]["model"], "opus");
    assert_eq!(agents["coding-builder"]["model"], "sonnet", "the agents keep their own");
    let prompt = agents["coding-leader"]["prompt"].as_str().unwrap();
    assert!(prompt.starts_with(g.trim()), "{prompt}");
    assert!(prompt.contains("\n\nYou lead the Coding squad.") && prompt.contains("SQUAD: done"), "{prompt}");
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
    t.wait_exit();
}

#[test]
fn an_agent_is_given_a_skill_and_an_mcp_server_from_what_is_installed_and_a_leader_starts_it_with_them() {
    let lab = Lab::new("sqtab-tools");
    // What is installed in the (temp) Claude config folder: one skill, one MCP server whose settings hold a token.
    fs::create_dir_all(lab.claude.join("skills/pdf-tools")).unwrap();
    fs::write(lab.claude.join("skills/pdf-tools/SKILL.md"), "---\nname: pdf\ndescription: Reads PDFs.\n---\nUse pdftotext.\n").unwrap();
    let claude_json = r#"{"mcpServers":{"sentry":{"command":"npx","env":{"SENTRY_TOKEN":"tok-123-secret"}}},"numStartups":3}"#;
    fs::write(lab.claude.join(".claude.json"), claude_json).unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    t.wait_for("enter open");
    t.send(b"3");
    t.wait_for_all(&["╭ [Squads]", "← → agents"]);
    t.send(b"\x1b[C");
    t.wait_for_all(&["[Agents]", "▎ planner"]);
    t.send(b"\r");
    t.wait_for_all(&["esc back to the list", "Skills", "None: enter picks from what is installed"]);
    // Down to Skills: past its name, model and three switches.
    t.send(b"\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B");
    t.wait_for("enter pick");
    t.send(b"\r");
    t.wait_for_all(&["╭ skills", "[ ] pdf", "Type the name of a skill…"]);
    t.send(b"\r");
    t.wait_for_all(&["[x] pdf", "planner has the skill pdf now"]);
    t.send(b"\x1b");
    t.wait_gone("Type the name of a skill…");
    // MCP servers: the one installed, then one typed that isn't installed here.
    t.send(b"\x1b[B");
    t.wait_for("enter pick");
    t.send(b"\r");
    t.wait_for_all(&["╭ MCP servers", "[ ] sentry", "Type the name of an MCP server…"]);
    t.send(b"\r");
    t.wait_for("[x] sentry");
    t.send(b"\x1b[B\r");
    t.wait_for_all(&["an MCP server", "as it is configured"]);
    t.send(b"linear\r");
    t.wait_for_flat("planner has the MCP server linear now (not found here");
    t.wait_for_all(&["sentry, linear (not found here)"]);
    let screen = t.stable();
    assert!(!screen.contains("tok-123"), "what a server is, is never shown: {screen}");
    let a: Value = serde_json::from_str(&fs::read_to_string(lab.config.join("agents/planner.json")).unwrap()).unwrap();
    assert_eq!(
        (a["skills"].clone(), a["mcp_servers"].clone(), a["builtin"].as_str()),
        (json!(["pdf"]), json!(["sentry", "linear"]), Some("planner"))
    );
    // Coding's leader starts with them: preloaded skills, servers by name, and the tools that let the planner use them.
    t.send(b"\x1b");
    t.wait_gone("esc back to the list");
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for_all(&["╭ Sessions", "╭ Tickets", "╭ Notes"]);
    t.send(b"\t");
    t.wait_for_flat("S hand to a squad");
    t.send(b"S");
    t.wait_for_all(&["hand to a squad", "Coding", "enter hand it over"]);
    t.send(b"\r");
    t.wait_for_all(&["FAKE LEADER sid:", "ctrl+space back to the project"]);
    let agents: Value = serde_json::from_str(value_after(&lab.args(1), "--agents")).unwrap();
    let p = &agents["coding-planner"];
    assert_eq!((p["skills"].clone(), p["mcpServers"].clone()), (json!(["pdf"]), json!(["sentry", "linear"])));
    assert_eq!(p["tools"], json!(["Read", "Grep", "Glob", "Bash", "Skill", "mcp__sentry", "mcp__linear"]));
    assert!(agents["coding-builder"].get("skills").is_none() && agents["coding-builder"].get("mcpServers").is_none());
    assert!(!value_after(&lab.args(1), "--agents").contains("tok-123"), "only names are passed on");
    assert_eq!(fs::read_to_string(lab.claude.join(".claude.json")).unwrap(), claude_json, "Claude's own file is only read");
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
    t.wait_exit();
}

#[test]
fn the_list_switches_to_the_agents_g_goes_from_a_step_to_its_agent_and_an_edit_there_reaches_every_squad() {
    let lab = Lab::new("sqtab-agents");
    // A copy of Coding of the user's own: it shares Coding's agents.
    fs::create_dir_all(lab.config.join("squads")).unwrap();
    // Colours as a terminal without NO_COLOR has them (with it set, crossterm drops the colour of bold text).
    let mut env = lab.env();
    env.push(("NO_COLOR", String::new()));
    let mut t = Term::spawn(&[], &env, &lab.work, 40, 130);
    t.wait_for("enter open");
    t.send(b"3");
    t.wait_for_all(&["[Squads]", "╭ Coding", "← → agents"]);
    t.send(b"c");
    t.wait_for("copy squad");
    t.send(b"\x15Careful coding\r");
    t.wait_for_flat("it uses the same agents");
    // The switch: the label of the list it shows in brackets and in another colour than the other.
    t.send(b"\x1b[C");
    t.wait_for_all(&["[Agents]", "+ New agent", "← → squads"]);
    let (r, c) = t.find("[Agents]").unwrap();
    let (r2, c2) = t.find("Squads  [").unwrap();
    // The theme's accent (Catppuccin Mocha lavender) and its dim text.
    let accent = vt100::Color::Rgb(0xb4, 0xbe, 0xfe);
    for i in 0..8 {
        assert_eq!(t.fg_at(r, c + i), accent, "cell {i} of [Agents]");
    }
    assert!(t.attrs_at(r, c + 1).0, "bold");
    assert_ne!(t.fg_at(r2, c2), accent, "the other label is dim");
    let (pr, pc) = t.find("▎ planner").unwrap();
    let (br, bc) = t.find("  builder").unwrap();
    assert_eq!(t.fg_at(pr, pc), accent, "the selection's mark");
    assert_ne!(t.bg_at(pr, pc + 3), t.bg_at(br, bc + 3), "the selected agent has its own background");
    t.wait_for_all(&["Used in", "Coding", "Careful coding", "step 1"]);
    // Back to the squads, into Coding's workflow, and g on the reviewer's step.
    t.send(b"\x1b[D");
    t.wait_for_all(&["[Squads]", "← → agents"]);
    t.send(b"\x1b[H\r");
    t.wait_for("esc back to the list");
    // Down to step 3's agent: past name, what it can do, and steps 1 and 2 (each its agent, then what it does).
    for _ in 0..6 {
        t.send(b"\x1b[B");
    }
    t.wait_for("enter pick the agent");
    t.send(b"g");
    t.wait_for_all(&["[Agents]", "Name                reviewer"]);
    // Its system prompt, in the (stand-in) editor: every squad that uses it has it. (Past its model, its three
    // switches, its skills and its MCP servers.)
    for _ in 0..7 {
        t.send(b"\x1b[B");
    }
    t.wait_for("enter edit");
    t.send(b"\r");
    t.wait_for("nano");
    t.send(b"\r");
    t.wait_for("FAKE NANO EDITING");
    t.send(b"Be strict.\r");
    t.wait_for_flat("Saved reviewer's system prompt, in every squad that uses it (Coding and Careful coding)");
    let a: Value = serde_json::from_str(&fs::read_to_string(lab.config.join("agents/reviewer.json")).unwrap()).unwrap();
    assert_eq!(a["builtin"], "reviewer");
    assert!(a["guidance"].as_str().unwrap().ends_with("Be strict."), "{a}");
    assert!(!lab.config.join("squads/coding.json").exists(), "the squad itself is as it was");
    t.wait_for_all(&["User Modified"]);
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
    t.wait_exit();
}
