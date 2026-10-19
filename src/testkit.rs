//! Fixtures for unit tests: a throwaway Claude config dir, platoon cache dir and platoon config dir, so
//! no test ever reads or writes the real `~/.claude`, `~/.cache/platoon` or `~/.config/platoon`.

use crate::agents::claude::Reader;
use crate::app::App;
use crate::usage;
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command},
    sync::atomic::{AtomicUsize, Ordering},
    sync::{Mutex, Once},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Directories to remove when the test binary exits.
static AT_EXIT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static REGISTER: Once = Once::new();

/// A test's temp directory, removed when the test binary exits - whether its tests passed or failed, and even when
/// a thread a test left running wrote into it after the test's own clean-up. Returns `path` as it is.
pub fn removed_at_exit(path: PathBuf) -> PathBuf {
    REGISTER.call_once(|| {
        // SAFETY: registers a plain `extern "C" fn` with the C library, which calls it once as the process exits.
        unsafe { libc::atexit(remove_at_exit) };
    });
    AT_EXIT.lock().unwrap_or_else(|e| e.into_inner()).push(path.clone());
    path
}

extern "C" fn remove_at_exit() {
    // Never wait at exit: a lock still held means a test thread is mid-registration; its dirs are left alone.
    let dirs = match AT_EXIT.try_lock() {
        Ok(mut dirs) => std::mem::take(&mut *dirs),
        Err(std::sync::TryLockError::Poisoned(e)) => std::mem::take(&mut *e.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    for d in dirs {
        let _ = fs::remove_dir_all(d);
    }
}

pub struct World {
    pub base: PathBuf,
    pub root: PathBuf,
    pub cache: PathBuf,
    pub config: PathBuf,
    procs: Vec<Child>,
}

/// Milliseconds since the epoch, as Claude Code writes them: `2026-09-19T03:53:40.091Z`.
pub fn iso(ms: u64) -> String {
    let (secs, frac) = ((ms / 1000) as i64, ms % 1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{frac:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn now_ms() -> u64 {
    usage::now() * 1000
}

impl World {
    pub fn new(name: &str) -> Self {
        let base = removed_at_exit(std::env::temp_dir().join(format!(
            "platoon-ut-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        )));
        let _ = fs::remove_dir_all(&base);
        let (root, cache, config) = (base.join("claude"), base.join("cache"), base.join("config"));
        for d in [root.join("sessions"), root.join("projects"), cache.clone()] {
            fs::create_dir_all(d).unwrap();
        }
        let w = Self { base, root, cache, config, procs: Vec::new() };
        // Most tests use conversations days old to check the list, not auto-archive: switch it off unless a test asks.
        w.settings(120, 0);
        w
    }

    /// Write `settings.json` (0 turns a feature off).
    pub fn settings(&self, idle_stop_minutes: u64, auto_archive_days: u64) {
        crate::settings::save(&self.config, &crate::settings::Settings { idle_stop_minutes, auto_archive_days, ..Default::default() })
            .unwrap();
    }

    pub fn app(&self) -> App {
        App::with_dirs(Reader::with_root(self.root.clone()), self.cache.clone(), self.config.clone())
    }

    /// A saved conversation: a real prompt, an assistant reply using `tokens`, last active `mins_ago` minutes ago.
    pub fn transcript(&self, id: &str, cwd: &str, prompt: &str, mins_ago: u64) -> PathBuf {
        let at = now_ms() - mins_ago * 60_000;
        let lines = vec![
            json!({"type":"mode","mode":"normal"}).to_string(),
            json!({"type":"user","cwd":cwd,"gitBranch":"main","timestamp":iso(at - 5000),"message":{"role":"user","content":prompt}}).to_string(),
            json!({"type":"assistant","cwd":cwd,"gitBranch":"main","timestamp":iso(at),"message":{"model":"claude-sonnet-5","usage":{"input_tokens":50_000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}).to_string(),
        ];
        self.transcript_lines(id, &lines)
    }

    pub fn transcript_lines(&self, id: &str, lines: &[String]) -> PathBuf {
        let dir = self.root.join("projects").join("-some-project");
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{id}.jsonl"));
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        p
    }

    /// Write a registry file for a process (`pid` needn't be ours, but it must be alive to count).
    pub fn register(&self, pid: u32, id: &str, cwd: &str, started: u64, status: &str) {
        let v = json!({"pid":pid,"sessionId":id,"cwd":cwd,"kind":"interactive","startedAt":started,"status":status});
        fs::write(self.root.join("sessions").join(format!("{pid}.json")), v.to_string()).unwrap();
    }

    /// Start a real process (`sleep`) standing in for a claude, and register it. Killed on drop.
    pub fn live(&mut self, id: &str, cwd: &str, started: u64, status: &str) -> u32 {
        let child = Command::new("sleep").arg("60").spawn().unwrap();
        let pid = child.id();
        self.procs.push(child);
        self.register(pid, id, cwd, started, status);
        pid
    }

    /// Start a shell script standing in for a claude that behaves in a particular way when told to stop, and
    /// register it. Killed on drop. The pid is the shell's own, and the script only ever ends through its trap.
    /// Returns once the trap is in place: a SIGTERM before that would simply end it.
    fn live_script(&mut self, id: &str, cwd: &str, started: u64, status: &str, trap: &str) -> u32 {
        let ready = self.base.join(format!("ready-{}", NEXT.fetch_add(1, Ordering::SeqCst)));
        let script = format!("trap '{trap}' TERM; : > '{}'; while :; do sleep 0.05; done", ready.display());
        let child = Command::new("sh").arg("-c").arg(script).spawn().unwrap();
        let pid = child.id();
        self.procs.push(child);
        let start = std::time::Instant::now();
        while !ready.exists() {
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "the stand-in process never got ready");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        self.register(pid, id, cwd, started, status);
        pid
    }

    /// Like `live`, but the process takes `secs` seconds to end after SIGTERM (as Claude does, cleaning up).
    pub fn live_slow_exit(&mut self, id: &str, cwd: &str, started: u64, status: &str, secs: f32) -> u32 {
        self.live_script(id, cwd, started, status, &format!("sleep {secs}; exit 0"))
    }

    /// Like `live`, but the process ignores SIGTERM (only SIGKILL, on drop, ends it).
    pub fn live_deaf(&mut self, id: &str, cwd: &str, started: u64, status: &str) -> u32 {
        self.live_script(id, cwd, started, status, "")
    }

    /// Write the registry file of a background session (`"kind":"bg"`), shaped like Claude's own: its `jobId` is
    /// what `claude attach` takes. `spare` makes it one of the service's idle pre-started spares.
    pub fn register_bg(&self, pid: u32, id: &str, cwd: &str, started: u64, job_id: &str, spare: bool) {
        let mut v = json!({"pid":pid,"sessionId":id,"cwd":cwd,"kind":"bg","startedAt":started,"status":"busy","jobId":job_id,"nameSource":"auto","name":"A title Claude made"});
        if spare {
            v.as_object_mut().unwrap().insert("spare".into(), json!(true));
        }
        fs::write(self.root.join("sessions").join(format!("{pid}.json")), v.to_string()).unwrap();
    }

    /// Write the registry file of an interactive session that handed its conversation to the background job
    /// `job_id`, shaped like Claude Code 2.1.283's: `parkedJobId`, and the `busy` status it never sets back.
    pub fn register_parked(&self, pid: u32, id: &str, cwd: &str, started: u64, job_id: &str) {
        let v = json!({"pid":pid,"sessionId":id,"cwd":cwd,"kind":"interactive","startedAt":started,"status":"busy","parkedJobId":job_id,"name":"platoon-b4","nameSource":"derived"});
        fs::write(self.root.join("sessions").join(format!("{pid}.json")), v.to_string()).unwrap();
    }

    /// Like `live`, for a parked interactive session (see `register_parked`).
    pub fn live_parked(&mut self, id: &str, cwd: &str, started: u64, job_id: &str) -> u32 {
        let child = Command::new("sleep").arg("60").spawn().unwrap();
        let pid = child.id();
        self.procs.push(child);
        self.register_parked(pid, id, cwd, started, job_id);
        pid
    }

    /// Like `live`, for a background session whose attach id is the first 8 characters of `id`.
    pub fn live_bg(&mut self, id: &str, cwd: &str, started: u64) -> u32 {
        let child = Command::new("sleep").arg("60").spawn().unwrap();
        let pid = child.id();
        self.procs.push(child);
        self.register_bg(pid, id, cwd, started, &id[..8], false);
        pid
    }

    /// The archive file, as parsed JSON (None if it doesn't exist).
    pub fn archive_file(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&fs::read_to_string(self.config.join("archive.json")).ok()?).ok()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        for c in &mut self.procs {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = fs::remove_dir_all(&self.base);
    }
}

pub fn alive(pid: u32) -> bool {
    crate::agents::claude::pid_alive(pid)
}

/// Wait for `pid` (a process of the test's own) to have ended, for as long as a loaded machine could need, and fail
/// the test rather than wait for ever.
pub fn wait_ended(pid: u32) {
    let since = std::time::Instant::now();
    while alive(pid) {
        assert!(since.elapsed() < std::time::Duration::from_secs(60), "pid {pid} never ended");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// A stand-in for `claude -p` that records every call (`calls/N.argv`, `.cwd`, `.env`, `.stdin`, N from 1) and
/// answers from files the test writes: `reply.N` (the text the model says, for call N) or else `reply.default`, or,
/// when neither exists, a naming answer built from the numbers it finds in the request (`[1]`, `[2]`, ...) or a
/// ranking answer for a "Description:" request. `usage` holds the four token numbers it reports; `hang` makes it
/// sleep until killed (its pid is written to `pid`); `fail` makes it print an error result and exit 1.
pub struct AiFake {
    pub dir: PathBuf,
    pub script: String,
}

impl AiFake {
    pub fn new(name: &str) -> AiFake {
        let dir = removed_at_exit(std::env::temp_dir().join(format!(
            "platoon-aifake-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        )));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("calls")).unwrap();
        let d = dir.display();
        let body = include_str!("../tests/fake-claude.sh").replace("@DIR@", &d.to_string());
        let script = dir.join("fake-claude");
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        AiFake { script: script.display().to_string(), dir }
    }

    pub fn calls(&self) -> usize {
        fs::read_dir(self.dir.join("calls"))
            .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".stdin")).count())
            .unwrap_or(0)
    }

    fn call_file(&self, k: usize, what: &str) -> String {
        fs::read_to_string(self.dir.join("calls").join(format!("{k}.{what}"))).unwrap_or_default()
    }

    /// What call number `k` (from 1) was sent on its standard input.
    pub fn stdin(&self, k: usize) -> String {
        self.call_file(k, "stdin")
    }

    pub fn argv(&self, k: usize) -> Vec<String> {
        self.call_file(k, "argv").lines().map(String::from).collect()
    }

    /// Whether anything call `k` was given as arguments contains `text`.
    pub fn call_text_has(&self, k: usize, text: &str) -> bool {
        self.call_file(k, "argv").contains(text)
    }

    /// Whether call `k` was given exactly this argument (one that may run over several lines, as a system prompt does).
    pub fn has_arg(&self, k: usize, arg: &str) -> bool {
        format!("\n{}", self.call_file(k, "argv")).contains(&format!("\n{arg}\n"))
    }

    pub fn cwd(&self, k: usize) -> String {
        self.call_file(k, "cwd").trim().to_string()
    }

    pub fn env(&self, k: usize) -> String {
        self.call_file(k, "env")
    }

    /// Say `text` (the model's words, not the envelope) to call `k`.
    pub fn reply(&self, k: usize, text: &str) {
        fs::write(self.dir.join(format!("reply.{k}")), text).unwrap();
    }

    pub fn reply_default(&self, text: &str) {
        fs::write(self.dir.join("reply.default"), text).unwrap();
    }

    pub fn usage(&self, input: u64, output: u64) {
        fs::write(
            self.dir.join("usage"),
            format!(r#"{{"input_tokens":{input},"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":{output}}}"#),
        )
        .unwrap();
    }

    pub fn hang(&self, on: bool) {
        let p = self.dir.join("hang");
        if on {
            fs::write(p, "").unwrap();
        } else {
            let _ = fs::remove_file(p);
        }
    }

    /// Every call takes this many seconds before it answers.
    pub fn slow(&self, secs: f32) {
        fs::write(self.dir.join("delay"), secs.to_string()).unwrap();
    }

    pub fn fail(&self, message: Option<&str>) {
        let p = self.dir.join("fail");
        match message {
            Some(m) => fs::write(p, m).unwrap(),
            None => {
                let _ = fs::remove_file(p);
            }
        }
    }

    /// The pids of the hanging fake and its `sleep`, if it ran.
    pub fn pids(&self) -> Vec<u32> {
        ["pid", "sleep-pid"].iter().filter_map(|f| fs::read_to_string(self.dir.join(f)).ok()?.trim().parse().ok()).collect()
    }
}

impl Drop for AiFake {
    fn drop(&mut self) {
        // Anything still hanging is ours: kill exactly the pids it wrote down.
        for pid in self.pids() {
            if pid > 1 && alive(pid) {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            }
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}
