//! Idle sessions stop on their own, and old conversations archive themselves, checked with the real binary.
//!
//! Time is shrunk for the host: PLATOON_IDLE_SECS_PER_MINUTE=2 makes "1 minute" of the idle-stop setting last
//! 2 seconds (and the host checks every 200ms). A real claude can't be used (it stops at a trust prompt in new
//! directories), so stand-ins play its part: `sleep` for a quiet claude, small scripts for the rest, and a
//! hand-written registry file supplies the status a real claude would report. Every directory is a temp dir.

mod common;
use common::*;
use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// One "minute" of the idle-stop setting, as the hosts under test see it.
const MINUTE: Duration = Duration::from_secs(2);
/// Comfortably longer than the threshold (one minute = 2s), to show something did NOT happen.
const STAY: Duration = Duration::from_secs(5);

struct Rig {
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    host: Option<Child>,
    /// Processes this test started (directly or through its own stand-in claude), killed at the end.
    pids: Vec<u32>,
}

impl Rig {
    fn new(name: &str, idle_stop_minutes: u64) -> Self {
        let base = tmp(name);
        let r = Self {
            claude: base.join("claude"),
            cache: base.join("cache"),
            config: base.join("config"),
            work: base.join("work"),
            host: None,
            pids: Vec::new(),
        };
        for d in [&r.claude.join("sessions"), &r.cache, &r.config, &r.work] {
            fs::create_dir_all(d).unwrap();
        }
        r.settings(idle_stop_minutes);
        r
    }

    /// Written the way platoon writes it (temp file + rename), so the host never reads half a file.
    fn settings(&self, idle_stop_minutes: u64) {
        let tmp = self.config.join("settings.json.tmp");
        fs::write(&tmp, format!(r#"{{"idle_stop_minutes":{idle_stop_minutes},"auto_archive_days":0}}"#)).unwrap();
        fs::rename(&tmp, self.config.join("settings.json")).unwrap();
    }

    /// The settings file with the MCP choice too (`None`: left out, as an older file would be).
    fn settings_mcp(&self, idle_stop_minutes: u64, ignores_mcp: Option<bool>) {
        let extra = ignores_mcp.map(|b| format!(r#","idle_stop_ignores_mcp":{b}"#)).unwrap_or_default();
        let tmp = self.config.join("settings.json.tmp");
        fs::write(&tmp, format!(r#"{{"idle_stop_minutes":{idle_stop_minutes},"auto_archive_days":0{extra}}}"#)).unwrap();
        fs::rename(&tmp, self.config.join("settings.json")).unwrap();
    }

    /// Claude Code's user config, where `claude mcp add` keeps servers (inside CLAUDE_CONFIG_DIR, which is set).
    fn mcp_servers(&self, servers: serde_json::Value) {
        fs::write(self.claude.join(".claude.json"), serde_json::json!({ "mcpServers": servers }).to_string()).unwrap();
    }

    /// A pid a stand-in wrote to `name` in the cache dir. Remembered so it is killed at the end if still there.
    fn pid_file(&mut self, name: &str) -> u32 {
        let file = self.cache.join(name);
        until(&format!("{name} to be written"), || fs::read_to_string(&file).is_ok_and(|s| s.trim().parse::<u32>().is_ok()));
        let pid = fs::read_to_string(&file).unwrap().trim().parse().unwrap();
        self.pids.push(pid);
        pid
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.claude.to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.cache.to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.config.to_string_lossy().into()),
            ("PLATOON_IDLE_SECS_PER_MINUTE", MINUTE.as_secs().to_string()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
        ]
    }

    /// Start a host running `program args` and return claude's pid (the program's own pid).
    fn start(&mut self, program: &str, args: &[&str]) -> u32 {
        let mut cmd = Command::new(BIN);
        cmd.env("PLATOON_NO_UPDATE_CHECK", "1"); // no version checks: never the network, never `claude --version`
        cmd.args(["host", "--program", program, "--dir"])
            .arg(&self.work)
            .args(["--rows", "24", "--cols", "80", "--"])
            .args(args)
            .envs(self.env())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        self.host = Some(cmd.spawn().unwrap());
        until("host to publish itself", || find_host(&self.cache).is_some());
        let (pid, _) = find_host(&self.cache).unwrap();
        self.pids.push(pid);
        pid
    }

    fn sock(&self) -> PathBuf {
        find_host(&self.cache).unwrap().1
    }

    /// What a real claude's registry entry says about its process. Missing until this is called.
    fn register(&self, pid: u32, status: &str) {
        let tmp = self.claude.join("sessions").join(format!("{pid}.tmp"));
        fs::write(
            &tmp,
            format!(r#"{{"pid":{pid},"sessionId":"s-{pid}","cwd":"/tmp","kind":"interactive","startedAt":1,"status":"{status}"}}"#),
        )
        .unwrap();
        fs::rename(&tmp, self.claude.join("sessions").join(format!("{pid}.json"))).unwrap();
    }

    /// The host has ended the session: claude is gone, and the host removed its files and exited cleanly.
    fn stopped(&mut self, pid: u32) -> bool {
        !alive(pid) && find_host(&self.cache).is_none() && self.host.as_mut().is_some_and(|h| matches!(h.try_wait(), Ok(Some(_))))
    }

    /// Assert the session is still up for `how_long`, checking throughout.
    fn stays_up(&self, pid: u32, how_long: Duration) {
        let start = Instant::now();
        while start.elapsed() < how_long {
            assert!(alive(pid) && find_host(&self.cache).is_some(), "the session was stopped after {:?}", start.elapsed());
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn wait_stopped(&mut self, pid: u32) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if self.stopped(pid) {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("the session was never stopped");
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // Only pids this test started: its host, claude, and whatever its stand-in claude started.
        for &pid in &self.pids {
            if alive(pid) {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            }
        }
        if let Some(h) = &mut self.host {
            let _ = h.kill();
            let _ = h.wait();
        }
    }
}

#[test]
fn a_quiet_idle_session_nobody_watches_is_stopped_after_the_threshold_and_only_that_one() {
    let mut r = Rig::new("idle-happy", 1);
    let mut bystander = Command::new("sleep").arg("60").spawn().unwrap(); // not the host's: must never be signalled
    let pid = r.start("sleep", &["600"]);
    r.register(pid, "idle");
    let started = Instant::now();

    r.stays_up(pid, Duration::from_millis(1000)); // not at once: the threshold is 2s
    r.wait_stopped(pid);
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(1500), "stopped after only {took:?}, before the threshold");
    assert!(took < Duration::from_secs(8), "took {took:?}");
    assert!(r.host.as_mut().unwrap().wait().unwrap().success(), "the host exits cleanly like it does when claude ends by itself");
    assert!(!r.cache.join("hosts").join(format!("{pid}.sock")).exists());
    assert!(bystander.try_wait().unwrap().is_none(), "a process the host doesn't own was touched");
    let _ = bystander.kill();
    let _ = bystander.wait();
}

#[test]
fn a_window_attached_keeps_the_session_up_and_leaving_starts_the_clock() {
    let mut r = Rig::new("idle-viewer", 1);
    let pid = r.start("sleep", &["600"]);
    r.register(pid, "idle");
    let viewer = Viewer::connect(&r.sock(), 24, 80);
    r.stays_up(pid, STAY); // way past the 2s threshold, but somebody is looking

    viewer.close();
    let left = Instant::now();
    r.stays_up(pid, Duration::from_millis(1000)); // the quiet period starts when the window leaves, not before
    r.wait_stopped(pid);
    assert!(left.elapsed() >= Duration::from_millis(1500), "stopped {:?} after the window left", left.elapsed());
}

#[test]
fn a_working_session_is_never_stopped_until_it_has_been_idle_for_the_whole_threshold() {
    let mut r = Rig::new("idle-busy", 1);
    let pid = r.start("sleep", &["600"]);
    r.register(pid, "busy");
    r.stays_up(pid, STAY);

    r.register(pid, "idle");
    let idle_since = Instant::now();
    r.stays_up(pid, Duration::from_millis(1000));
    r.wait_stopped(pid);
    assert!(idle_since.elapsed() >= Duration::from_millis(1500), "counted busy time as idle time");
}

#[test]
fn a_status_that_is_missing_unreadable_or_unknown_never_counts_as_idle() {
    let mut r = Rig::new("idle-unknown", 1);
    let pid = r.start("sleep", &["600"]);
    // No registry entry at all (a real claude writes none until it is past its startup prompts).
    r.stays_up(pid, STAY);
    fs::write(r.claude.join("sessions").join(format!("{pid}.json")), "{ not json").unwrap();
    r.stays_up(pid, Duration::from_secs(3));
    r.register(pid, "waiting");
    r.stays_up(pid, Duration::from_secs(3));
    // Only a real "idle" lets it go.
    r.register(pid, "idle");
    r.wait_stopped(pid);
}

#[test]
fn a_session_with_a_live_child_process_is_left_alone_until_the_child_is_gone() {
    let mut r = Rig::new("idle-child", 1);
    let child_file = r.cache.join("child.pid");
    // A claude that started a background job (think dev server) and now waits for input.
    let script = format!("sleep 600 & echo $! > {}; read x", child_file.display());
    let pid = r.start("sh", &["-c", &script]);
    r.register(pid, "idle");
    until("the child to start", || fs::read_to_string(&child_file).is_ok_and(|s| s.trim().parse::<u32>().is_ok()));
    let child: u32 = fs::read_to_string(&child_file).unwrap().trim().parse().unwrap();
    r.pids.push(child);

    r.stays_up(pid, STAY);
    assert!(alive(child), "the background job must survive");

    unsafe { libc::kill(child as i32, libc::SIGKILL) }; // our own job: now nothing runs under claude
    r.wait_stopped(pid);
}

#[test]
fn output_from_claude_counts_as_activity() {
    let mut r = Rig::new("idle-output", 1);
    // Prints a line a second for five seconds, then goes quiet. bash's `read -t` is a builtin: no child process.
    let pid = r.start("bash", &["-c", "for i in 1 2 3 4 5; do echo tick$i; read -t 1 x; done; read x"]);
    r.register(pid, "idle");
    let start = Instant::now();
    // Idle, unwatched and past 2s since the start, yet still alive at 3.5s: only the output explains it.
    while start.elapsed() < Duration::from_millis(3500) {
        assert!(alive(pid), "stopped at {:?} while it was still printing", start.elapsed());
        thread::sleep(Duration::from_millis(100));
    }
    r.wait_stopped(pid);
    assert!(
        start.elapsed() >= Duration::from_millis(5500),
        "stopped {:?} in, less than the threshold after the last output",
        start.elapsed()
    );
}

#[test]
fn zero_turns_it_off_and_changing_the_setting_applies_without_restarting_the_host() {
    let mut r = Rig::new("idle-off", 0);
    let pid = r.start("sleep", &["600"]);
    r.register(pid, "idle");
    r.stays_up(pid, STAY);

    r.settings(1); // edited while the host runs
    r.wait_stopped(pid);
}

// --- through the real UI -------------------------------------------------------------------------------

/// A stand-in claude that registers itself as idle (as the real one does), prints how it was started, and then
/// sits there quietly. `exec` keeps the pid, so the registry entry belongs to the process the host spawned.
fn quiet_claude(dir: &Path) -> PathBuf {
    let p = dir.join("quiet-claude.sh");
    let script = r#"#!/bin/sh
echo "fake-claude args:$*"
f="$CLAUDE_CONFIG_DIR/sessions/$$"
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","kind":"interactive","startedAt":1,"status":"idle"}' "$$" "$2" "$(pwd)" > "$f.tmp"
mv "$f.tmp" "$f.json"
exec sleep 600
"#;
    fs::write(&p, script).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// A conversation last active in 2020 (so certainly old) or "now-ish" if `old` is false.
fn save(r: &Rig, id: &str, prompt: &str, old: bool) {
    let dir = r.claude.join("projects").join("-work");
    fs::create_dir_all(&dir).unwrap();
    let ts = if old { "2020-01-01T00:00:00.000Z".to_string() } else { "2099-01-01T00:00:00.000Z".to_string() };
    let line = format!(
        r#"{{"type":"user","cwd":"{}","gitBranch":"main","timestamp":"{ts}","message":{{"role":"user","content":"{prompt}"}}}}"#,
        r.work.display()
    );
    fs::write(dir.join(format!("{id}.jsonl")), line + "\n").unwrap();
}

#[test]
fn a_stopped_session_is_still_listed_and_enter_resumes_it_from_its_saved_history() {
    let mut r = Rig::new("idle-resume", 1);
    save(&r, "beef-0001-aaaa", "refactor the widget", false);
    let claude = quiet_claude(&r.claude);
    let mut env = r.env();
    env.push(("PLATOON_CLAUDE_BIN", claude.to_string_lossy().into()));
    let mut t = Term::spawn(&[], &env, &r.work, 30, 110);
    t.wait_for("refactor the widget");
    t.send(b"\r");
    t.wait_for("fake-claude args:--resume beef-0001-aaaa");
    let (first, _) = find_host(&r.cache).expect("resumed under a host");
    r.pids.push(first);

    t.send(&[0]); // ctrl+space: leave it. With nobody attached, the idle clock starts.
    t.wait_for("enter open");
    until("the idle session to be stopped by its host", || !alive(first) && find_host(&r.cache).is_none());

    thread::sleep(Duration::from_millis(1500)); // platoon looks again once a second
    // Nothing was lost and nothing changed in the list: same row, same tab, no extra state or column.
    until("the list to show the row after the stop", || {
        let listed = t.text();
        let flat = t.flat();
        listed.contains("refactor the widget") && flat.contains("Active 1") && flat.contains("Archived 0")
    });
    // What must *not* be on the screen is read from one that has settled: half a frame would agree too easily.
    let listed = t.stable();
    for banned in ["stopped", "Session ended", "not open", "asleep", "paused"] {
        assert!(!listed.contains(banned), "{banned:?} must not appear:\n{listed}");
    }

    t.send(b"\r"); // Enter resumes it, as for any conversation nothing is running
    t.wait_for("fake-claude args:--resume beef-0001-aaaa");
    let (second, _) = find_host(&r.cache).expect("a new host after Enter");
    assert_ne!(second, first, "a fresh process must have been started");
    r.pids.push(second);
    assert!(alive(second));
    assert!(r.claude.join("projects/-work/beef-0001-aaaa.jsonl").exists());

    t.send(&[0]);
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn old_conversations_are_archived_on_first_run_with_a_notice_and_restoring_one_sticks() {
    let mut r = Rig::new("auto-archive", 0);
    fs::remove_file(r.config.join("settings.json")).unwrap(); // no settings file: the defaults apply
    save(&r, "dead-0001-aaaa", "an old conversation", true);
    save(&r, "live-0002-bbbb", "a current conversation", false);
    let claude = quiet_claude(&r.claude);
    let mut env = r.env();
    env.push(("PLATOON_CLAUDE_BIN", claude.to_string_lossy().into()));
    let mut t = Term::spawn(&[], &env, &r.work, 30, 110);

    t.wait_for("Auto-archived 1 session idle for 2+ days. Find it in Archived.");
    assert!(t.flat().contains("Active 1") && t.flat().contains("Archived 1"), "{}", t.text());
    let file: serde_json::Value = serde_json::from_str(&fs::read_to_string(r.config.join("archive.json")).unwrap()).unwrap();
    assert!(file["archived"].get("dead-0001-aaaa").is_some() && file["archived"].get("live-0002-bbbb").is_none(), "{file}");
    assert!(r.claude.join("projects/-work/dead-0001-aaaa.jsonl").exists(), "the conversation itself is untouched");

    // Open it from the Archived tab: it is restored and resumed, and stays restored.
    t.send(b"\x1b[C"); // right arrow: Archived
    t.wait_for("a restore");
    t.send(b"\r");
    t.wait_for("fake-claude args:--resume dead-0001-aaaa");
    if let Some((pid, _)) = find_host(&r.cache) {
        r.pids.push(pid);
    }
    t.send(&[0]);
    t.wait_for_flat("Archived 0");
    thread::sleep(Duration::from_millis(2500)); // several refreshes
    let text = t.stable().split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(text.contains("Archived 0") && text.contains("Active 2"), "{text}");
    t.send(b"q");
    t.wait_exit();
}

// --- MCP servers ---------------------------------------------------------------------------------------

/// Gone, or a zombie waiting to be reaped: either way it is not running.
fn gone(pid: u32) -> bool {
    !running(pid)
}

fn wait_gone(what: &str, pid: u32) {
    until(what, || gone(pid));
}

#[test]
fn mcp_servers_do_not_keep_a_session_alive_and_the_ones_that_outlive_claude_are_cleaned_up_but_nothing_else() {
    let mut r = Rig::new("mcp-happy", 1);
    let (a, w) = (r.cache.join("mcp-a.pid"), r.cache.join("mcp-wrapped.pid"));
    // A wrapper-style server: `sh -c '...'` that starts the real one and waits for it.
    let wrapper = format!("sleep 612 & echo $! > {}; wait", w.display());
    r.mcp_servers(serde_json::json!({
        "plain": {"command": "sleep", "args": ["611"]},
        "wrapped": {"type": "stdio", "command": "sh", "args": ["-c", wrapper]},
        "remote": {"type": "http", "url": "https://example.invalid/mcp"},
        "broken": {"command": 5},
    }));
    // Not under claude, and with exactly the configured server's command line: none of platoon's business.
    let mut bystander = Command::new("sleep").arg("611").spawn().unwrap();
    // Servers that ignore the hang-up a closing terminal sends (as anything that detaches itself does).
    let script = format!(
        "trap '' HUP; sleep 611 & echo $! > {}; sh -c '{wrapper}' & echo $! > {}; read x",
        a.display(),
        r.cache.join("mcp-wrapper.pid").display()
    );
    let pid = r.start("sh", &["-c", &script]);
    r.register(pid, "idle");
    let (plain, sh, deep) = (r.pid_file("mcp-a.pid"), r.pid_file("mcp-wrapper.pid"), r.pid_file("mcp-wrapped.pid"));
    let started = Instant::now();

    r.stays_up(pid, Duration::from_millis(1000)); // the threshold still applies
    r.wait_stopped(pid);
    assert!(started.elapsed() >= Duration::from_millis(1500), "stopped after {:?}", started.elapsed());
    // claude was ended; they don't end with it (nothing tells them to), so the host asked them to, after a moment.
    for (what, p) in [("the plain server", plain), ("the wrapper", sh), ("what the wrapper started", deep)] {
        wait_gone(&format!("{what} to be ended"), p);
    }
    assert!(r.host.as_mut().unwrap().wait().unwrap().success());
    assert!(bystander.try_wait().unwrap().is_none(), "a look-alike outside the session was signalled");
    let _ = bystander.kill();
    let _ = bystander.wait();
}

#[test]
fn a_server_that_is_not_configured_keeps_the_session_alive_even_beside_configured_ones() {
    let mut r = Rig::new("mcp-other", 1);
    r.mcp_servers(serde_json::json!({"plain": {"command": "sleep", "args": ["611"]}}));
    let script = format!(
        "trap '' HUP; sleep 611 & echo $! > {}; sleep 612 & echo $! > {}; read x",
        r.cache.join("mcp.pid").display(),
        r.cache.join("job.pid").display()
    );
    let pid = r.start("sh", &["-c", &script]);
    r.register(pid, "idle");
    let (_mcp, job) = (r.pid_file("mcp.pid"), r.pid_file("job.pid")); // job: a dev server, say
    r.stays_up(pid, STAY);
    assert!(alive(job));
    unsafe { libc::kill(job as i32, libc::SIGKILL) };
    r.wait_stopped(pid);
}

#[test]
fn the_same_command_line_below_a_direct_child_is_work_not_an_mcp_server() {
    let mut r = Rig::new("mcp-deep", 1);
    r.mcp_servers(serde_json::json!({"plain": {"command": "sleep", "args": ["611"]}}));
    // claude -> sh (think: a tool call) -> sleep 611. The only direct child is not a configured server.
    let inner = format!("sleep 611 & echo $! > {}; wait", r.cache.join("deep.pid").display());
    let script = format!("sh -c '{inner}' & echo $! > {}; read x", r.cache.join("tool.pid").display());
    let pid = r.start("sh", &["-c", &script]);
    r.register(pid, "idle");
    let (tool, deep) = (r.pid_file("tool.pid"), r.pid_file("deep.pid"));
    r.stays_up(pid, STAY);
    unsafe {
        libc::kill(tool as i32, libc::SIGKILL);
        libc::kill(deep as i32, libc::SIGKILL);
    }
    r.wait_stopped(pid);
}

#[test]
fn counting_mcp_servers_keeps_such_a_session_alive_and_changing_it_applies_to_the_running_host() {
    let mut r = Rig::new("mcp-setting", 1);
    r.settings_mcp(1, Some(false)); // "Count MCP servers"
    r.mcp_servers(serde_json::json!({"plain": {"command": "sleep", "args": ["611"]}}));
    let script = format!("trap '' HUP; sleep 611 & echo $! > {}; read x", r.cache.join("mcp.pid").display());
    let pid = r.start("sh", &["-c", &script]);
    r.register(pid, "idle");
    let mcp = r.pid_file("mcp.pid");
    r.stays_up(pid, STAY);
    assert!(alive(mcp));

    r.settings_mcp(1, Some(true)); // the default choice, set while the host runs
    r.wait_stopped(pid);
    wait_gone("the server to be cleaned up", mcp);
}

#[test]
fn an_older_settings_file_without_the_choice_ignores_mcp_servers() {
    let mut r = Rig::new("mcp-default", 1);
    r.mcp_servers(serde_json::json!({"plain": {"command": "sleep", "args": ["611"]}}));
    let script = format!("trap '' HUP; sleep 611 & echo $! > {}; read x", r.cache.join("mcp.pid").display());
    let pid = r.start("sh", &["-c", &script]);
    r.register(pid, "idle");
    let mcp = r.pid_file("mcp.pid");
    r.wait_stopped(pid);
    wait_gone("the server to be cleaned up", mcp);
}

#[test]
fn a_squads_leader_is_not_stopped_while_its_ticket_is_working_and_is_once_it_waits_on_the_user() {
    let mut r = Rig::new("idle-leader", 1);
    // The ticket whose current leader this session is, Working.
    let tickets = r.config.join("projects/p1/tickets.json");
    fs::create_dir_all(tickets.parent().unwrap()).unwrap();
    let write = |state: &str| {
        let t = serde_json::json!({"next": 2, "tickets": [{"number": 1, "title": "Basket", "file": "01-basket.md", "state": state, "how_far": "Local commit",
            "folders": ["/tmp"], "blocked_by": [], "created": 1, "updated": 1, "leaders": [{"squad": "coding", "session": "lead-idle-1", "started": 1}]}]});
        let tmp = tickets.with_extension("tmp");
        fs::write(&tmp, t.to_string()).unwrap();
        fs::rename(&tmp, &tickets).unwrap();
    };
    write("working");
    // A stand-in leader: started with the leader's flags (its session id among them), it only sleeps.
    let script = r.work.join("leader.sh");
    fs::write(&script, "#!/bin/sh\nexec sleep 600\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let pid =
        r.start(&script.to_string_lossy(), &["--add-dir", "/tmp", "--session-id", "lead-idle-1", "-n", "#1 Basket", "--agent", "leader"]);
    r.register(pid, "idle");
    // Idle, quiet, nobody watching, way past the 2s threshold: kept, because its members may be at work.
    r.stays_up(pid, STAY);
    // Waiting on you: an idle session like any other.
    write("waiting");
    let since = Instant::now();
    r.wait_stopped(pid);
    assert!(since.elapsed() < Duration::from_secs(8), "took {:?}", since.elapsed());
}
