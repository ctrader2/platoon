//! Fixtures for unit tests: a throwaway Claude config dir, platoon cache dir and platoon config dir, so
//! no test ever reads or writes the real `~/.claude`, `~/.cache/platoon` or `~/.config/platoon`.

use crate::app::App;
use crate::claude::Reader;
use crate::usage;
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

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
        let base = std::env::temp_dir().join(format!("platoon-ut-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
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
        crate::settings::save(&self.config, &crate::settings::Settings { idle_stop_minutes, auto_archive_days, ..Default::default() }).unwrap();
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

    /// Write the registry file of a background session (`"kind":"bg"`), shaped like Claude's own: its `jobId` is
    /// what `claude attach` takes. `spare` makes it one of the service's idle pre-started spares.
    pub fn register_bg(&self, pid: u32, id: &str, cwd: &str, started: u64, job_id: &str, spare: bool) {
        let mut v = json!({"pid":pid,"sessionId":id,"cwd":cwd,"kind":"bg","startedAt":started,"status":"busy","jobId":job_id,"nameSource":"auto","name":"A title Claude made"});
        if spare {
            v.as_object_mut().unwrap().insert("spare".into(), json!(true));
        }
        fs::write(self.root.join("sessions").join(format!("{pid}.json")), v.to_string()).unwrap();
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
    crate::claude::pid_alive(pid)
}
