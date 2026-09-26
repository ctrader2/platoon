//! The Squads tab end to end, in a real terminal with the real binary: a squad described in the text box and drafted
//! by a stand-in `claude`, saved, handed a ticket with `a` (the leader starts with that squad's own `--agents`), then
//! changed on the tab while its leader has stopped: continued, the leader runs with the squad as it was when it
//! started. And a squad's system prompt edited in a stand-in editor.
//!
//! The "claude" is a stand-in script: `claude -p` answers the drafting call and the commander's check from files,
//! and a session writes down its arguments and echoes what is typed. A real `claude` or editor is never run, every
//! directory is a temp dir (the git repository too), and the only processes signalled are the ones these tests
//! started.

mod common;
use common::*;
use serde_json::{json, Value};
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

const DRAFT: &str = r#"{"name":"Careful coding","can_do":"Makes small, careful changes to code in a git repository.","needs_folder":true,"worktree":"always",
 "leader":{"model":"sonnet","guidance":"Lead calmly and ask when unsure."},
 "members":[
  {"name":"planner","model":"opus","can_change_files":false,"can_run_commands":true,"can_use_web":false,"guidance":"Plan the change."},
  {"name":"builder","model":"haiku","can_change_files":true,"can_run_commands":true,"can_use_web":false,"guidance":"Make the change."}],
 "workflow":[
  {"member":"planner","does":"Writes a short plan.","back_to":null,"times":0},
  {"member":"builder","does":"Makes the change and runs the tests.","back_to":null,"times":0},
  {"member":"planner","does":"Checks the change against the plan.","back_to":2,"times":2}]}"#;

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
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects"), &lab.base.join("cache"), &lab.config, &lab.work, &lab.shop, &lab.fake, &lab.bin] {
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
        fs::read_dir(self.fake.join("started")).map(|d| d.count()).unwrap_or(0)
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
    t.wait_for_all(&["╭ Squads", "╭ Coding", "n describe a new squad"]);
    t.send(b"n");
    t.wait_for_all(&["describe a new squad", "enter ask Claude to draft it"]);
    t.send(b"a squad that codes carefully: a planner and a builder\r");
    t.wait_for_all(&["Draft, not saved", "Careful coding", "Lead calmly and ask when unsure."]);
    let asked = lab.call(1, "stdin");
    assert!(asked.contains("a squad that codes carefully: a planner and a builder"), "{asked}");
    assert!(lab.call(1, "argv").contains("\n--model\nsonnet\n"), "drafted on Sonnet: {}", lab.call(1, "argv"));
    assert!(!lab.config.join("squads/careful-coding.json").exists(), "not saved until the user says so");
    t.send(b"\x13");
    t.wait_for_flat("Saved the squad \"Careful coding\".");
    let v = lab.squad_file("careful-coding");
    assert_eq!((v["name"].as_str(), v["members"][1]["model"].as_str()), (Some("Careful coding"), Some("haiku")));
    t.wait_for_all(&["Your own"]);

    // Hand the ticket to it: the picker lists every squad by name.
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for_all(&["╭ Sessions", "╭ Tickets", "╭ Notes"]);
    t.send(b"\t");
    t.wait_for_flat("a hand to a squad");
    t.send(b"a");
    t.wait_for_all(&["hand to a squad", "Coding", "Careful coding", "Makes small, careful changes"]);
    // Coding, Research, Docs and Summarize come first.
    t.send(b"\x1b[B\x1b[B\x1b[B\x1b[B\r");
    t.wait_for_all(&["FAKE LEADER sid:", "ctrl+space back to the project"]);
    assert!(lab.call(2, "stdin").starts_with("The squad: Careful coding\nWhat it can do: Makes small, careful changes"), "{}", lab.call(2, "stdin"));
    let first = lab.args(1);
    assert_eq!(value_after(&first, "--agent"), "careful-coding-leader");
    assert_eq!(value_after(&first, "--model"), "sonnet");
    assert_eq!(value_after(&first, "-n"), "Careful coding squad · #1 Basket");
    let agents: Value = serde_json::from_str(value_after(&first, "--agents")).unwrap();
    assert_eq!(agents.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), ["careful-coding-leader", "careful-coding-planner", "careful-coding-builder"]);
    assert_eq!(agents["careful-coding-builder"]["model"], "haiku");
    assert_eq!(agents["careful-coding-builder"]["tools"], json!(["Read", "Grep", "Glob", "Edit", "Write", "Bash"]));
    assert_eq!(agents["careful-coding-planner"]["tools"], json!(["Read", "Grep", "Glob", "Bash"]));
    let prompt = agents["careful-coding-leader"]["prompt"].as_str().unwrap();
    assert!(prompt.starts_with("Lead calmly and ask when unsure.\n\nYou lead the Careful coding squad."), "{prompt}");
    assert!(prompt.contains("3. careful-coding-planner: Checks the change against the plan. It may send the work back: then give its reasons to careful-coding-builder and go on again from step 2. At most twice;"), "{prompt}");
    assert_eq!(lab.ticket_now()["leaders"][0]["squad"], "careful-coding");

    // The squad changes on the tab: its leader now runs on Opus.
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(b"3");
    t.wait_for_all(&["╭ Squads", "Careful coding"]);
    // To the list (shift+left), on the new squad, then into its details from the top, down to the leader's model
    // (past its name, what it can do, whether it needs a folder and its worktree).
    t.send(b"\x1b[1;2D");
    t.wait_for_all(&["enter details", "╭ Careful coding"]);
    t.send(b"\rg\x1b[B\x1b[B\x1b[B\x1b[B\r");
    t.wait_for_all(&["Model for the leader:", "Opus"]);
    t.send(b"\x1b[A\r");
    t.wait_for_flat("The leader runs on Opus now.");
    assert_eq!(lab.squad_file("careful-coding")["leader"]["model"], "opus");

    // Its session stops; continued, it runs with the squad as it was when it started.
    let pid = lab.pid(1);
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    until("the fake leader to exit", || !alive(pid));
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for("╭ Tickets");
    t.send(b"\t");
    t.wait_for_flat("s state");
    t.send(b"\r");
    t.wait_for_all(&["The squad's session isn't running. Enter to continue it.", "Careful coding squad"]);
    t.send(b"\r");
    t.wait_for_all(&["FAKE LEADER sid:", "ctrl+space back to the project"]);
    until("the second start", || lab.started() == 2);
    let again = lab.args(2);
    assert_eq!(value_after(&again, "--resume"), value_after(&first, "--session-id"));
    assert_eq!(value_after(&again, "--model"), "sonnet", "not the Opus it was changed to since");
    assert_eq!(value_after(&again, "--agents"), value_after(&first, "--agents"), "the very same squad");
    assert_eq!(value_after(&again, "--agent"), "careful-coding-leader");
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn a_squads_system_prompt_is_edited_in_the_editor_and_the_squad_says_user_modified() {
    let lab = Lab::new("sqtab-prompt");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    t.wait_for("enter open");
    t.send(b"3");
    t.wait_for_all(&["╭ Squads", "Built in"]);
    // Into the details, down to the leader's system prompt.
    t.send(b"\t\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B");
    t.wait_for_all(&["enter edit", "esc back to the list"]);
    t.send(b"\r");
    t.wait_for("nano");
    t.send(b"\r");
    t.wait_for("FAKE NANO EDITING");
    t.send(b"Keep the reports to five lines.\r");
    t.wait_for_flat("Saved the leader's system prompt.");
    t.wait_for_all(&["User Modified"]);
    let v = lab.squad_file("coding");
    let g = v["leader"]["guidance"].as_str().unwrap();
    assert!(g.starts_with("You are given one ticket") && g.ends_with("Keep the reports to five lines."), "{g}");
    assert_eq!(v["builtin"], "coding");
    // Settings keeps the commander's system prompt and the drafting one, and no Coding entries.
    t.send(b"s");
    t.wait_for("Settings file:");
    t.send(b"\x1b[Z");
    t.wait_for_all(&["System prompt for the squad commander", "System prompt for drafting a squad"]);
    let screen = t.stable();
    assert!(!screen.contains("Coding squad's"), "{screen}");
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    t.wait_exit();
}
