//! Talking to Codex's own app-server (`codex app-server --listen stdio://`), the way Codex's window does: one JSON
//! object per line on the child's input and output. `platoon setup` asks it to trust the hooks setup wrote, and
//! `platoon uninstall` to take those records out again, so Codex works out each hook's hash and writes its own
//! `config.toml`: platoon never does either. The protocol is Codex's (0.163), not a promise: every answer is read
//! defensively, the whole exchange is time-boxed, and only the child started here is ever killed.

use serde_json::{Map, Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// How long one whole exchange may take, from starting Codex to its last answer.
pub const TIME_BOX: Duration = Duration::from_secs(20);
/// How long Codex is given to end once its input is closed, within the time box, before it is killed.
const EXIT_GRACE: Duration = Duration::from_secs(5);

/// What Codex says about a hook: it runs only once trusted (or when an administrator's settings put it there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustStatus {
    Untrusted,
    /// Trusted once, but its command has changed since.
    Modified,
    Trusted,
    Managed,
    Unknown,
}

/// One command hook as Codex lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    /// Codex's name for it in `hooks.state`: `<hooks.json path>:<event>:<group>:<handler>`.
    pub key: String,
    /// Codex's spelling of the event (`sessionStart`).
    pub event: String,
    pub command: String,
    /// The file it is in.
    pub source: PathBuf,
    /// What Codex records as trusted.
    pub hash: String,
    pub trust: TrustStatus,
}

impl Hook {
    /// Is it for `event` as `hooks.json` spells it (`SessionStart` is Codex's `sessionStart`)?
    pub fn is_event(&self, event: &str) -> bool {
        let plain = |s: &str| s.chars().filter(|c| *c != '_').collect::<String>().to_lowercase();
        plain(&self.event) == plain(event)
    }

    /// Is it in the file `path` (both sides as the file system resolves them)?
    pub fn is_in(&self, path: &Path) -> bool {
        let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        real(&self.source) == real(path)
    }
}

/// Codex's `hooks.state` table in the user's `config.toml`, and the version of that file Codex read it from.
pub struct HookState {
    pub records: Map<String, Value>,
    pub version: Option<String>,
}

/// A running `codex app-server` this platoon started, initialized. Dropping it closes its input, gives it a moment to
/// end, then kills it (only it) if it hasn't.
pub struct Server {
    child: Child,
    input: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    deadline: Instant,
    timeout: Duration,
    next_id: u64,
}

fn unexpected() -> String {
    "Codex answered something platoon doesn't understand".into()
}

impl Server {
    /// Start `program` as Codex's app-server on the folder `codex_root` and introduce platoon to it. The whole exchange
    /// that follows must end within `timeout`.
    pub fn start(program: &str, codex_root: &Path, timeout: Duration) -> Result<Server, String> {
        // A unit test passes a stand-in by its full path: the real `codex` would read and write the user's ~/.codex.
        #[cfg(test)]
        assert!(program.contains('/'), "a unit test started Codex by name ({program}): pass a stand-in's path");
        let mut cmd = Command::new(program);
        cmd.args(["app-server", "--listen", "stdio://"])
            .env("CODEX_HOME", codex_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = crate::ai::spawn_retrying(&mut cmd).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!("Codex's program ({program}) isn't there"),
            _ => format!("Codex couldn't be started ({e})"),
        })?;
        let (tx, lines) = mpsc::channel();
        if let Some(out) = child.stdout.take() {
            // Left to end by itself: when Codex ends, its output does.
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines() {
                    let Ok(line) = line else { break };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });
        }
        let input = child.stdin.take();
        let mut s = Server { child, input, lines, deadline: Instant::now() + timeout, timeout, next_id: 1 };
        let hello = s.request("initialize", json!({"clientInfo": {"name": "platoon", "version": env!("CARGO_PKG_VERSION")}}))?;
        // The folder Codex says it works on must be the one platoon wrote hooks.json in.
        let home = hello.get("codexHome").and_then(Value::as_str).ok_or_else(unexpected)?;
        let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if real(Path::new(home)) != real(codex_root) {
            return Err(format!("Codex is using another folder ({home})"));
        }
        s.send(&json!({"method": "initialized"}))?;
        Ok(s)
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        let stopped = || "Codex stopped before it answered".to_string();
        let input = self.input.as_mut().ok_or_else(stopped)?;
        writeln!(input, "{message}").and_then(|_| input.flush()).map_err(|_| stopped())
    }

    /// Send one request and wait for its answer, skipping every other line (Codex's notifications).
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"id": id, "method": method, "params": params}))?;
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(format!("Codex didn't answer within {} seconds", self.timeout.as_secs_f32().ceil()));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("Codex stopped before it answered".into()),
            };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v.get("method").is_some() || v.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(e) = v.get("error") {
                let why = e.get("message").and_then(Value::as_str).unwrap_or("no reason given");
                return Err(format!("Codex refused: {why}"));
            }
            return v.get("result").cloned().ok_or_else(unexpected);
        }
    }

    /// Every command hook Codex would run in a session in `cwd`, with whether it is trusted.
    pub fn hooks(&mut self, cwd: &Path) -> Result<Vec<Hook>, String> {
        let answer = self.request("hooks/list", json!({"cwds": [cwd]}))?;
        let entries = answer.get("data").and_then(Value::as_array).ok_or_else(unexpected)?;
        let mut out: Vec<Hook> = Vec::new();
        for entry in entries {
            for h in entry.get("hooks").and_then(Value::as_array).ok_or_else(unexpected)? {
                if let Some(h) = hook(h)
                    && !out.iter().any(|o| o.key == h.key)
                {
                    out.push(h);
                }
            }
        }
        Ok(out)
    }

    /// Trust `hooks` as they are now, exactly as Codex's own "Trust" does: their keys and current hashes merged into
    /// `hooks.state`, and running sessions told.
    pub fn trust(&mut self, hooks: &[Hook]) -> Result<(), String> {
        let records: Map<String, Value> = hooks.iter().map(|h| (h.key.clone(), json!({"trusted_hash": h.hash}))).collect();
        let edit = json!({"keyPath": "hooks.state", "value": records, "mergeStrategy": "upsert"});
        self.request("config/batchWrite", json!({"edits": [edit], "reloadUserConfig": true})).map(|_| ())
    }

    /// A table of the user's own `config.toml` (`path`, its keys; empty when there is none), as Codex reads it, and
    /// that file's version.
    fn user_table(&mut self, path: &[&str]) -> Result<(Map<String, Value>, Option<String>), String> {
        let answer = self.request("config/read", json!({"includeLayers": true}))?;
        let layers = answer.get("layers").and_then(Value::as_array).ok_or_else(unexpected)?;
        let user = layers.iter().find(|l| {
            let name = l.get("name");
            name.and_then(|n| n.get("type")).and_then(Value::as_str) == Some("user")
                && name.and_then(|n| n.get("profile")).is_none_or(Value::is_null)
        });
        let Some(user) = user else { return Ok((Map::new(), None)) };
        let table = path.iter().try_fold(user.get("config"), |v, k| Some(v?.get(*k)));
        let records = match table.flatten() {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => return Err(unexpected()),
        };
        Ok((records, user.get("version").and_then(Value::as_str).map(String::from)))
    }

    /// The `hooks.state` table of the user's own `config.toml` (empty when there is none), as Codex reads it.
    pub fn hook_state(&mut self) -> Result<HookState, String> {
        let (records, version) = self.user_table(&["hooks", "state"])?;
        Ok(HookState { records, version })
    }

    /// The `projects` table of the user's own `config.toml` (each folder's `trust_level`), and that file's version.
    pub fn projects(&mut self) -> Result<(Map<String, Value>, Option<String>), String> {
        self.user_table(&["projects"])
    }

    /// Trust the folder Codex knows as `key`, as Codex's own folder-trust question does when the user says yes
    /// (`tui/src/config_update.rs`: `projects."<key>".trust_level = "trusted"`, replaced).
    pub fn trust_project(&mut self, key: &str) -> Result<(), String> {
        let edit = json!({"keyPath": format!("projects.{}.trust_level", quoted_key(key)), "value": "trusted", "mergeStrategy": "replace"});
        self.request("config/batchWrite", json!({"edits": [edit], "reloadUserConfig": true})).map(|_| ())
    }

    /// Take the folders Codex knows as `keys` out of its `projects` table (each a `null` in its place: Codex's way of
    /// clearing a value, `clear_config_value`), only if the file is still the `version` it was read at.
    pub fn forget_projects(&mut self, keys: &[String], version: Option<&str>) -> Result<(), String> {
        let edits: Vec<Value> = keys
            .iter()
            .map(|k| json!({"keyPath": format!("projects.{}", quoted_key(k)), "value": Value::Null, "mergeStrategy": "replace"}))
            .collect();
        let mut params = json!({"edits": edits, "reloadUserConfig": true});
        if let (Some(v), Some(p)) = (version, params.as_object_mut()) {
            p.insert("expectedVersion".into(), json!(v));
        }
        self.request("config/batchWrite", params).map(|_| ())
    }

    /// Replace `hooks.state` with `records`, only if the file is still the `version` it was read at.
    pub fn replace_hook_state(&mut self, records: Map<String, Value>, version: Option<&str>) -> Result<(), String> {
        let edit = json!({"keyPath": "hooks.state", "value": records, "mergeStrategy": "replace"});
        let mut params = json!({"edits": [edit], "reloadUserConfig": true});
        if let (Some(v), Some(p)) = (version, params.as_object_mut()) {
            p.insert("expectedVersion".into(), json!(v));
        }
        self.request("config/batchWrite", params).map(|_| ())
    }
}

/// A folder as one segment of a `config/batchWrite` key path: in double quotes, its `\\` and `"` escaped, as Codex's
/// own window writes it (`tui/src/config_update.rs`).
fn quoted_key(key: &str) -> String {
    format!("\"{}\"", key.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The name Codex knows a folder by in its `projects` table: the folder as the file system resolves it
/// (`config/src/loader/mod.rs`: `project_trust_key`, a canonical path).
pub fn project_key(folder: &Path) -> String {
    folder.canonicalize().unwrap_or_else(|_| folder.to_path_buf()).to_string_lossy().into_owned()
}

/// What a folder's entry in Codex's `projects` table says about trusting it, if it has one.
fn trust_level(projects: &Map<String, Value>, key: &str) -> Option<String> {
    Some(projects.get(key)?.get("trust_level")?.as_str()?.to_string())
}

/// Have Codex (`program`, its folder `codex_root`) trust `folder` for sessions started in it, as the user's yes to
/// Codex's own question would (a task's worktree, which is the user's repository: Codex trusts it, and would only ask).
/// `ours`: the name platoon already wrote down for it, when it had Codex trust it before. Ok(Some(name)) when the
/// trust is platoon's (written now, or already there under `ours`); Ok(None) when the folder already had an entry of
/// the user's (trusted or not), which is left exactly as it is and never taken away by platoon. Err when Codex
/// couldn't be asked: it asks the user in the window then. The whole exchange ends within `timeout`.
pub fn trust_folder(
    program: &str,
    codex_root: &Path,
    folder: &Path,
    ours: Option<&str>,
    timeout: Duration,
) -> Result<Option<String>, String> {
    let key = project_key(folder);
    let mut server = Server::start(program, codex_root, timeout)?;
    let (projects, _) = server.projects()?;
    match trust_level(&projects, &key) {
        Some(level) if level == "trusted" && ours == Some(key.as_str()) => Ok(Some(key)),
        Some(_) => Ok(None),
        None => {
            server.trust_project(&key)?;
            Ok(Some(key))
        }
    }
}

/// Is `key` the folder of a task's worktree platoon made: `<repo>-worktrees/task-<N>`? Only such a folder's trust is
/// ever taken away by platoon.
pub fn is_task_worktree(key: &str) -> bool {
    let p = Path::new(key);
    let task = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("task-")).is_some_and(|n| n.parse::<u32>().is_ok());
    let parent = p.parent().and_then(Path::file_name).and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("-worktrees") && n.len() > 10);
    task && parent && p.is_absolute()
}

/// Have Codex forget its trust in the folders platoon had it trust (`keys`, as `trust_folder` named them), when their
/// entries still say so and they are task worktrees' (`is_task_worktree`); every other entry stays, and the file is
/// written only if it hasn't changed since it was read. Ok: how many entries went (one already gone, or that the user
/// changed to "untrusted", is left as it is and counts as done).
pub fn forget_folders(program: &str, codex_root: &Path, keys: &[String], timeout: Duration) -> Result<usize, String> {
    let mut server = Server::start(program, codex_root, timeout)?;
    let (projects, version) = server.projects()?;
    let gone: Vec<String> =
        keys.iter().filter(|k| is_task_worktree(k) && trust_level(&projects, k).as_deref() == Some("trusted")).cloned().collect();
    if gone.is_empty() {
        return Ok(0);
    }
    server.forget_projects(&gone, version.as_deref())?;
    Ok(gone.len())
}

impl Drop for Server {
    fn drop(&mut self) {
        // Codex ends when its input does.
        drop(self.input.take());
        let until = self.deadline.min(Instant::now() + EXIT_GRACE);
        while Instant::now() < until {
            match self.child.try_wait() {
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                _ => return,
            }
        }
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// One hook of `hooks/list`'s answer; None for a hook that isn't a command or lacks what trusting it needs.
fn hook(v: &Value) -> Option<Hook> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    if text("handlerType")? != "command" {
        return None;
    }
    let trust = match text("trustStatus").as_deref() {
        Some("untrusted") => TrustStatus::Untrusted,
        Some("modified") => TrustStatus::Modified,
        Some("trusted") => TrustStatus::Trusted,
        Some("managed") => TrustStatus::Managed,
        _ => TrustStatus::Unknown,
    };
    Some(Hook {
        key: text("key")?,
        event: text("eventName")?,
        command: text("command")?,
        source: PathBuf::from(text("sourcePath")?),
        hash: text("currentHash")?,
        trust,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{CodexServerFake, codex_hook};
    use std::fs;

    fn home(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-codexserver-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_codex_that_never_answers_is_given_up_on_in_time_and_only_it_is_killed() {
        let codex = home("hang");
        let fake = CodexServerFake::new("hang", &codex);
        fs::write(fake.dir.join("hang"), "").unwrap();
        let started = Instant::now();
        let err = Server::start(&fake.program, &codex, Duration::from_millis(300)).err().expect("no answer, no server");
        assert!(err.starts_with("Codex didn't answer within"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        if let Ok(pid) = fs::read_to_string(fake.dir.join("pid")) {
            assert!(!crate::procs::pid_alive(pid.trim().parse().unwrap()), "killed and reaped");
        }
    }

    #[test]
    fn hooks_are_read_defensively_and_their_events_match_however_spelled() {
        let codex = home("list");
        let file = codex.join("hooks.json");
        let fake = CodexServerFake::new("list", &codex);
        let good = codex_hook("k1", "sessionStart", "x", &file, "sha256:a", "untrusted");
        let mut no_hash = codex_hook("k2", "stop", "y", &file, "sha256:b", "trusted");
        no_hash.as_object_mut().unwrap().remove("currentHash");
        let not_a_command = json!({"key": "k3", "eventName": "stop", "handlerType": "prompt", "sourcePath": file, "currentHash": "h"});
        let odd = codex_hook("k4", "stop", "z", &file, "sha256:c", "something-new");
        // The same hook listed for two folders comes back once.
        let entry = |hooks: Vec<Value>| json!({"cwd": "/", "hooks": hooks, "warnings": [], "errors": []});
        fake.result("hooks/list", None, json!({"data": [entry(vec![good.clone(), no_hash, not_a_command, odd]), entry(vec![good])]}));
        let mut s = Server::start(&fake.program, &codex, Duration::from_secs(20)).unwrap();
        let hooks = s.hooks(Path::new("/")).unwrap();
        drop(s);
        assert_eq!(
            hooks.iter().map(|h| (h.key.as_str(), h.trust)).collect::<Vec<_>>(),
            [("k1", TrustStatus::Untrusted), ("k4", TrustStatus::Unknown)]
        );
        assert!(hooks[0].is_event("SessionStart") && hooks[0].is_event("session_start") && !hooks[0].is_event("Stop"));
        assert!(hooks[0].is_in(&file) && !hooks[0].is_in(&codex.join("other.json")));
        assert_eq!(fake.methods(), ["initialize", "initialized", "hooks/list"]);
    }

    #[test]
    fn the_users_hook_records_are_read_from_their_own_config_layer() {
        let codex = home("state");
        let fake = CodexServerFake::new("state", &codex);
        let layer = |kind: &str, profile: Value, state: Value| json!({"name": {"type": kind, "file": "/x/config.toml", "profile": profile}, "version": format!("v-{kind}-{profile}"), "config": {"hooks": {"state": state}}});
        fake.result(
            "config/read",
            Some(1),
            json!({"config": {}, "origins": {}, "layers": [
                layer("system", Value::Null, json!({"sys": {}})),
                layer("user", json!("work"), json!({"profile": {}})),
                layer("user", Value::Null, json!({"mine": {"trusted_hash": "h"}})),
            ]}),
        );
        fake.result(
            "config/read",
            Some(2),
            json!({"config": {}, "origins": {}, "layers": [{"name": {"type": "user"}, "version": "v", "config": {}}]}),
        );
        fake.result("config/read", Some(3), json!({"config": {}, "origins": {}}));
        let mut s = Server::start(&fake.program, &codex, Duration::from_secs(20)).unwrap();
        let state = s.hook_state().unwrap();
        assert_eq!((Value::Object(state.records), state.version.as_deref()), (json!({"mine": {"trusted_hash": "h"}}), Some("v-user-null")));
        let empty = s.hook_state().unwrap();
        assert!(empty.records.is_empty() && empty.version.as_deref() == Some("v"), "a user file without hooks.state");
        assert_eq!(s.hook_state().err().unwrap(), "Codex answered something platoon doesn't understand");
        drop(s);
        assert_eq!(fake.requests()[2]["params"], json!({"includeLayers": true}));
    }

    /// Codex's answer to `config/read` with the user's own `projects` table and file version.
    fn user_projects(projects: Value, version: &str) -> Value {
        json!({"config": {}, "origins": {}, "layers": [
            {"name": {"type": "system", "file": "/etc/codex/config.toml"}, "version": "v0", "config": {"projects": {"/sys": {"trust_level": "trusted"}}}},
            {"name": {"type": "user", "file": "/x/config.toml"}, "version": version, "config": {"projects": projects, "model": "gpt-6-luna"}},
        ]})
    }

    #[test]
    fn a_task_worktree_is_trusted_as_codexs_own_question_would_and_the_users_own_say_is_left_alone() {
        let codex = home("trust-folder");
        let base = home("trust-folder-work");
        let folder = base.join("my \"shop\"-worktrees/task-5");
        fs::create_dir_all(&folder).unwrap();
        let key = project_key(&folder);
        assert_eq!(key, folder.canonicalize().unwrap().to_string_lossy(), "the folder as the file system resolves it");
        // Not there yet: written, exactly as Codex's window writes it.
        let fake = CodexServerFake::new("trust-folder", &codex);
        fake.result("config/read", None, user_projects(json!({"/elsewhere": {"trust_level": "trusted"}}), "sha256:v1"));
        fake.result("config/batchWrite", None, json!({"status": "ok", "version": "sha256:v2"}));
        assert_eq!(trust_folder(&fake.program, &codex, &folder, None, TIME_BOX), Ok(Some(key.clone())));
        assert_eq!(fake.methods(), ["initialize", "initialized", "config/read", "config/batchWrite"]);
        let sent = fake.requests();
        assert_eq!(sent[2]["params"], json!({"includeLayers": true}));
        let path = format!("projects.\"{}\".trust_level", key.replace('\\', "\\\\").replace('"', "\\\""));
        assert_eq!(
            sent[3],
            json!({"id": 3, "method": "config/batchWrite", "params": {
                "edits": [{"keyPath": path, "value": "trusted", "mergeStrategy": "replace"}], "reloadUserConfig": true}})
        );
        assert!(path.contains("my \\\"shop\\\"-worktrees"), "{path}");
        // platoon's own entry, there from before: nothing written. The user's own entry (trusted or not): left alone,
        // and never platoon's.
        for (projects, ours, want) in [
            (json!({(key.clone()): {"trust_level": "trusted"}}), Some(key.as_str()), Some(key.clone())),
            (json!({(key.clone()): {"trust_level": "trusted"}}), None, None),
            (json!({(key.clone()): {"trust_level": "untrusted"}}), Some(key.as_str()), None),
        ] {
            let fake = CodexServerFake::new("trust-folder-there", &codex);
            fake.result("config/read", None, user_projects(projects.clone(), "v"));
            assert_eq!(trust_folder(&fake.program, &codex, &folder, ours, TIME_BOX), Ok(want), "{projects}");
            assert!(!fake.methods().contains(&"config/batchWrite".to_string()), "{projects}");
        }
        // Codex can't be asked: said, for the window's own question to take over.
        let gone = trust_folder("/nonexistent/platoon-unit-test/codex", &codex, &folder, None, TIME_BOX);
        assert!(gone.as_ref().is_err_and(|e| e.contains("isn't there")), "{gone:?}");
    }

    #[test]
    fn only_trust_platoon_gave_a_task_worktree_is_taken_away_and_only_if_the_file_is_as_it_was_read() {
        let codex = home("forget-folder");
        let fake = CodexServerFake::new("forget-folder", &codex);
        let ours = "/home/u/shop-worktrees/task-5".to_string();
        let untrusted_since = "/home/u/shop-worktrees/task-6".to_string();
        let gone_already = "/home/u/shop-worktrees/task-7".to_string();
        let not_a_worktree = "/home/u/shop".to_string();
        fake.result(
            "config/read",
            None,
            user_projects(
                json!({(ours.clone()): {"trust_level": "trusted"}, (untrusted_since.clone()): {"trust_level": "untrusted"},
                       (not_a_worktree.clone()): {"trust_level": "trusted"}}),
                "sha256:v7",
            ),
        );
        fake.result("config/batchWrite", None, json!({"status": "ok"}));
        let keys = [ours.clone(), untrusted_since, gone_already, not_a_worktree];
        assert_eq!(forget_folders(&fake.program, &codex, &keys, TIME_BOX), Ok(1));
        let sent = fake.requests();
        assert_eq!(
            sent[3]["params"],
            json!({"edits": [{"keyPath": "projects.\"/home/u/shop-worktrees/task-5\"", "value": null, "mergeStrategy": "replace"}],
                   "reloadUserConfig": true, "expectedVersion": "sha256:v7"})
        );
        // Nothing of platoon's left: nothing written.
        let fake = CodexServerFake::new("forget-folder-none", &codex);
        fake.result("config/read", None, user_projects(json!({}), "v"));
        assert_eq!(forget_folders(&fake.program, &codex, &[ours], TIME_BOX), Ok(0));
        assert_eq!(fake.methods(), ["initialize", "initialized", "config/read"]);
        for (key, is) in [
            ("/r/shop-worktrees/task-5", true),
            ("/r/shop-worktrees/task-5/sub", false),
            ("/r/shop/task-5", false),
            ("/r/-worktrees/task-5", false),
            ("/r/shop-worktrees/task-x", false),
            ("shop-worktrees/task-5", false),
        ] {
            assert_eq!(is_task_worktree(key), is, "{key}");
        }
    }
}
