//! The two version notices. platoon knows the Claude Code version it was last checked with
//! (`checked-with/claude-code`, compiled in): when the user's Claude Code is newer, it says so once per new version, in
//! the app and in `platoon setup`. And once a day at most it asks GitHub for its latest release, platoon's only network
//! request: when a newer platoon is out, it says so once per version, with the update command for how this platoon was
//! installed. Both run off the UI thread when the app starts, and any failure (no `claude`, no `curl`, offline, the
//! repository private, a rate limit) means no notice and nothing said.
//!
//! The setting `update_check` and `PLATOON_NO_UPDATE_CHECK` switch the GitHub request off; the environment variable
//! switches off the Claude Code check too (nothing is run at all), and every test sets it.
//!
//! What is remembered lives in `<cache>/version-check.json`: when GitHub was last asked, what it said, and which
//! versions were already announced.

use serde_json::{Map, Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant},
};

/// The Claude Code version this platoon was last checked with.
pub fn checked_with() -> &'static str {
    include_str!("../checked-with/claude-code").trim()
}

/// This platoon's own version.
pub fn running() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Where the latest release is asked for.
pub const RELEASES_API: &str = "https://api.github.com/repos/ctrader2/platoon/releases/latest";
/// Where a person finds the releases.
pub const RELEASES_PAGE: &str = "https://github.com/ctrader2/platoon/releases";
/// The install script of the latest release (dist names it after the package).
pub const INSTALLER_URL: &str = "https://github.com/ctrader2/platoon/releases/latest/download/platoon-tui-installer.sh";
/// GitHub is asked at most this often.
pub const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;
/// The environment variable that switches every check off.
pub const ENV_OFF: &str = "PLATOON_NO_UPDATE_CHECK";
const STATE_FILE: &str = "version-check.json";
/// How long `claude --version` and the request may take before they are given up on.
const CLAUDE_WAIT: Duration = Duration::from_secs(10);
const CURL_WAIT: Duration = Duration::from_secs(10);
/// What curl itself allows the request (connecting included).
const CURL_MAX_SECS: &str = "5";

/// A version as three numbers. Anything after them (`-rc.1`) is ignored.
pub type Version = (u64, u64, u64);

/// `1.2.3`, `v1.2.3`, `1.2.3-rc.1` -> (1, 2, 3).
pub fn parse(text: &str) -> Option<Version> {
    let t = text.trim();
    let t = t.strip_prefix('v').unwrap_or(t);
    let mut parts = t.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let rest = parts.next()?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let patch = digits.parse().ok()?;
    Some((major, minor, patch))
}

/// `a` is a later version than `b` (false when either can't be read).
pub fn newer(a: &str, b: &str) -> bool {
    matches!((parse(a), parse(b)), (Some(a), Some(b)) if a > b)
}

/// The version in what `claude --version` printed (`2.1.288 (Claude Code)`). None for anything that doesn't say it is
/// Claude Code: a stand-in or a wrapper that prints something else is no reason for a notice.
pub fn claude_version_in(output: &str) -> Option<String> {
    let line = output.lines().find(|l| l.contains("Claude Code"))?;
    line.split_whitespace().find(|w| parse(w).is_some() && w.chars().next().is_some_and(|c| c.is_ascii_digit())).map(str::to_string)
}

/// The version in GitHub's answer about the latest release (`"tag_name": "v1.1.0"`), without the `v`.
pub fn latest_in(json_text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_text).ok()?;
    let tag = v.get("tag_name")?.as_str()?;
    let (a, b, c) = parse(tag)?;
    Some(format!("{a}.{b}.{c}"))
}

/// `PLATOON_NO_UPDATE_CHECK` as it is set: anything but empty or `0` switches the checks off.
pub fn off_by(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

/// The checks are switched off for this process.
pub fn off_by_env() -> bool {
    off_by(std::env::var_os(ENV_OFF).as_deref())
}

/// How this platoon was installed, as far as the folder it runs from says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    Homebrew,
    /// `cargo install`: in cargo's own folder of programs.
    Cargo,
    /// The install script of a release (it leaves a receipt naming the folder it installed to).
    Script,
    /// Anything else: from source, Nix, copied by hand.
    Other,
}

/// The program that is Claude Code here: `PLATOON_CLAUDE_BIN` (a wrapper, or a test's stand-in) or `claude`. None when
/// the real one is forbidden (`PLATOON_FORBID_REAL_CLAUDE`, set by tests) and no stand-in is named.
pub fn claude_program() -> Option<String> {
    match std::env::var("PLATOON_CLAUDE_BIN") {
        Ok(p) => Some(p),
        Err(_) if std::env::var_os("PLATOON_FORBID_REAL_CLAUDE").is_some() => None,
        Err(_) => Some("claude".into()),
    }
}

/// How an install is told: the program's real path (links followed), cargo's folder of programs, and the folder the
/// install script's receipt names, if there is one.
pub fn install_kind(exe: &Path, cargo_bin: &Path, receipt_prefix: Option<&Path>) -> Install {
    if exe.components().any(|c| c.as_os_str() == "Cellar") {
        return Install::Homebrew;
    }
    let dir = exe.parent();
    let same = |p: &Path| dir.is_some_and(|d| d == p || std::fs::canonicalize(p).is_ok_and(|c| c == d));
    if receipt_prefix.is_some_and(same) {
        return Install::Script;
    }
    if same(cargo_bin) {
        return Install::Cargo;
    }
    Install::Other
}

/// How the platoon running now was installed (real paths and the environment; not for tests).
pub fn install_here() -> Install {
    let Some(exe) = std::env::current_exe().ok().and_then(|e| std::fs::canonicalize(e).ok()) else { return Install::Other };
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let cargo_bin = std::env::var_os("CARGO_HOME").map_or_else(|| home.join(".cargo"), PathBuf::from).join("bin");
    let config = std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    let receipt = std::fs::read_to_string(config.join("platoon-tui/platoon-tui-receipt.json")).ok();
    let prefix = receipt
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("install_prefix").and_then(Value::as_str).map(PathBuf::from));
    install_kind(&exe, &cargo_bin, prefix.as_deref())
}

/// What a newer platoon's notice says, with the update command for this install.
pub fn platoon_line(latest: &str, install: &Install) -> String {
    match install {
        Install::Homebrew => format!("platoon {latest} is out. To update: brew upgrade platoon"),
        Install::Cargo => format!("platoon {latest} is out. To update: cargo install platoon-tui"),
        Install::Script => format!("platoon {latest} is out. To update, run the install script again: curl -LsSf {INSTALLER_URL} | sh"),
        Install::Other => format!("platoon {latest} is out: {RELEASES_PAGE}"),
    }
}

/// What the Claude Code notice says, when the user's version is newer than the one platoon was checked with.
pub fn claude_line(user: &str, checked: &str) -> Option<String> {
    newer(user, checked).then(|| {
        format!("Claude Code {user} is newer than this platoon was checked with ({checked}). If something looks wrong, update platoon.")
    })
}

/// What is remembered between runs (`<cache>/version-check.json`). Keys platoon doesn't know are kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// When GitHub was last asked (seconds since the epoch), whatever came of it. 0: never.
    pub checked_at: u64,
    /// The latest release GitHub named, the last time it answered.
    pub latest: Option<String>,
    /// The newest platoon and Claude Code versions already announced.
    pub announced_platoon: Option<String>,
    pub announced_claude: Option<String>,
    other: Map<String, Value>,
}

impl State {
    pub fn load(cache: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(cache.join(STATE_FILE)) else { return Self::default() };
        let Ok(Value::Object(mut o)) = serde_json::from_str::<Value>(&text) else { return Self::default() };
        let mut text_of = |k: &str| o.remove(k).and_then(|v| v.as_str().map(str::to_string));
        let (latest, announced_platoon, announced_claude) = (text_of("latest"), text_of("announced_platoon"), text_of("announced_claude"));
        let checked_at = o.remove("checked_at").and_then(|v| v.as_u64()).unwrap_or(0);
        Self { checked_at, latest, announced_platoon, announced_claude, other: o }
    }

    pub fn save(&self, cache: &Path) -> std::io::Result<()> {
        let mut o = self.other.clone();
        o.insert("checked_at".into(), json!(self.checked_at));
        for (k, v) in
            [("latest", &self.latest), ("announced_platoon", &self.announced_platoon), ("announced_claude", &self.announced_claude)]
        {
            match v {
                Some(v) => o.insert(k.into(), json!(v)),
                None => o.remove(k),
            };
        }
        let text = serde_json::to_string_pretty(&Value::Object(o)).unwrap_or_default();
        crate::projects::write_atomic(&cache.join(STATE_FILE), &text)
    }

    /// GitHub may be asked again: a day has passed (or the clock went back).
    pub fn due(&self, now: u64) -> bool {
        self.checked_at == 0 || now < self.checked_at || now - self.checked_at >= CHECK_EVERY_SECS
    }
}

/// What to say, given the versions found and what was announced before, and the state to remember once it has been
/// said. None: nothing new to say.
pub fn decide(state: &State, claude: Option<&str>, running: &str, checked: &str, install: &Install) -> Option<(String, State)> {
    let mut after = state.clone();
    let platoon = state.latest.as_deref().filter(|l| newer(l, running) && state.announced_platoon.as_deref() != Some(*l)).map(|l| {
        after.announced_platoon = Some(l.to_string());
        platoon_line(l, install)
    });
    let claude = claude.filter(|c| state.announced_claude.as_deref() != Some(*c) && newer(c, checked));
    if let Some(c) = claude {
        after.announced_claude = Some(c.to_string());
    }
    let text = match (claude, platoon) {
        (None, None) => return None,
        (Some(c), None) => claude_line(c, checked)?,
        (None, Some(p)) => p,
        // One notice: the Claude Code part without its advice (the platoon part is the advice), then the update.
        (Some(c), Some(p)) => format!("Claude Code {c} is newer than this platoon was checked with ({checked}). {p}"),
    };
    Some((text, after))
}

/// Run `program args`, wait at most `wait`, and give back what it printed when it succeeded. A program that is
/// missing, fails or takes too long gives nothing (one that took too long is stopped: it is ours).
fn output_of(program: &str, args: &[&str], wait: Duration) -> Option<String> {
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < wait => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut out = String::new();
    child.stdout.take()?.take(1 << 20).read_to_string(&mut out).ok()?;
    status.success().then_some(out)
}

/// The user's Claude Code version: `<program> --version`.
pub fn claude_version(program: &str) -> Option<String> {
    claude_version_in(&output_of(program, &["--version"], CLAUDE_WAIT)?)
}

/// The latest release on GitHub, asked with `curl` (a short timeout, https only, a User-Agent as GitHub requires).
pub fn fetch_latest(curl: &str, url: &str) -> Option<String> {
    let agent = format!("User-Agent: platoon/{}", running());
    let args = ["-fsSL", "--proto", "=https", "--max-time", CURL_MAX_SECS, "-H", "Accept: application/vnd.github+json", "-H", &agent, url];
    latest_in(&output_of(curl, &args, CURL_WAIT)?)
}

/// Everything the checks need, so a test can give its own.
pub struct Checks {
    pub cache: PathBuf,
    /// The program to ask for Claude Code's version; None: don't ask.
    pub claude: Option<String>,
    pub curl: String,
    pub url: String,
    /// The setting: GitHub may be asked.
    pub update_check: bool,
    /// `PLATOON_NO_UPDATE_CHECK`: nothing is run at all.
    pub off: bool,
    pub now: u64,
    pub install: Install,
}

/// A notice to show, and what to remember once it has shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub text: String,
    pub state: State,
}

/// Run the checks: Claude Code's version, then GitHub if the setting allows and a day has passed (the time asked is
/// saved at once, whatever the answer). Blocks for as long as they take: call it off the UI thread.
pub fn run(c: &Checks) -> Option<Outcome> {
    if c.off {
        return None;
    }
    let mut state = State::load(&c.cache);
    let claude = c.claude.as_deref().and_then(claude_version);
    if c.update_check && state.due(c.now) {
        state.checked_at = c.now;
        if let Some(latest) = fetch_latest(&c.curl, &c.url) {
            state.latest = Some(latest);
        }
        let _ = state.save(&c.cache);
    }
    let (text, state) = decide(&state, claude.as_deref(), running(), checked_with(), &c.install)?;
    Some(Outcome { text, state })
}

/// `run` on a thread of its own; its answer arrives on the receiver.
pub fn spawn(c: Checks) -> Receiver<Option<Outcome>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(run(&c));
    });
    rx
}

/// The line `platoon setup` prints about Claude Code, when the user's is newer than the one platoon was checked with.
/// Said every time setup runs (setup is a report); nothing is run when the checks are switched off.
pub fn setup_line(claude: Option<&str>, off: bool) -> Option<String> {
    if off {
        return None;
    }
    claude_line(&claude_version(claude?)?, checked_with())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-ut-versions-{name}-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A stand-in program: records its arguments in `<dir>/<name>.calls` and prints `out`.
    fn stand_in(dir: &Path, name: &str, out: &str) -> String {
        let p = dir.join(name);
        let calls = dir.join(format!("{name}.calls"));
        std::fs::write(&p, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncat <<'EOF'\n{out}\nEOF\n", calls.display())).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    }

    fn calls(dir: &Path, name: &str) -> Vec<String> {
        std::fs::read_to_string(dir.join(format!("{name}.calls"))).unwrap_or_default().lines().map(str::to_string).collect()
    }

    fn checks(dir: &Path, claude_out: &str, github_out: &str) -> Checks {
        Checks {
            cache: dir.join("cache"),
            claude: Some(stand_in(dir, "claude", claude_out)),
            curl: stand_in(dir, "curl", github_out),
            url: "https://example.invalid/releases/latest".into(),
            update_check: true,
            off: false,
            now: 1_800_000_000,
            install: Install::Cargo,
        }
    }

    #[test]
    fn checked_with_is_the_file_compiled_in_and_running_is_the_crate_version() {
        assert_eq!(checked_with(), include_str!("../checked-with/claude-code").trim());
        assert!(parse(checked_with()).is_some(), "{}", checked_with());
        assert_eq!(running(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn versions_are_read_and_compared_as_numbers() {
        assert_eq!(parse("2.1.288"), Some((2, 1, 288)));
        assert_eq!(parse("v1.10.0"), Some((1, 10, 0)));
        assert_eq!(parse("1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse("1.2"), None);
        assert_eq!(parse("one.two.three"), None);
        assert!(newer("2.2.0", "2.1.288"));
        assert!(newer("2.1.1000", "2.1.288"), "not compared as text");
        assert!(newer("1.10.0", "1.9.9"));
        assert!(!newer("2.1.288", "2.1.288"));
        assert!(!newer("2.1.9", "2.1.288"));
        assert!(!newer("garbage", "1.0.0") && !newer("1.0.0", "garbage"));
    }

    #[test]
    fn only_output_that_says_claude_code_counts() {
        assert_eq!(claude_version_in("2.1.288 (Claude Code)\n").as_deref(), Some("2.1.288"));
        assert_eq!(claude_version_in("2.2.0 (Claude Code)").as_deref(), Some("2.2.0"));
        // `sh --version` and the like, used as stand-ins in tests: no notice.
        assert_eq!(claude_version_in("GNU bash, version 5.2.21(1)-release"), None);
        assert_eq!(claude_version_in(""), None);
    }

    #[test]
    fn githubs_answer_gives_the_tag_without_its_v_and_anything_else_gives_nothing() {
        let canned = r#"{"url":"https://api.github.com/repos/ctrader2/platoon/releases/1","tag_name":"v1.1.0","name":"1.1.0","draft":false,"prerelease":false}"#;
        assert_eq!(latest_in(canned).as_deref(), Some("1.1.0"));
        assert_eq!(latest_in(r#"{"tag_name":"1.2.3"}"#).as_deref(), Some("1.2.3"));
        // A private repository (404), a rate limit, or not JSON at all.
        assert_eq!(latest_in(r#"{"message":"Not Found","documentation_url":"https://docs.github.com/rest","status":"404"}"#), None);
        assert_eq!(latest_in(r#"{"message":"API rate limit exceeded"}"#), None);
        assert_eq!(latest_in("<html>"), None);
        assert_eq!(latest_in(r#"{"tag_name":"nightly"}"#), None);
    }

    #[test]
    fn the_environment_variable_is_off_unless_empty_or_zero() {
        use std::ffi::OsStr;
        assert!(off_by(Some(OsStr::new("1"))));
        assert!(off_by(Some(OsStr::new("yes"))));
        assert!(!off_by(Some(OsStr::new("0"))));
        assert!(!off_by(Some(OsStr::new(""))));
        assert!(!off_by(None));
    }

    #[test]
    fn the_install_is_told_by_the_folder_platoon_runs_from() {
        let cargo_bin = Path::new("/home/u/.cargo/bin");
        let local = Path::new("/home/u/.local/bin");
        let kind = |exe: &str, receipt: Option<&Path>| install_kind(Path::new(exe), cargo_bin, receipt);
        assert_eq!(kind("/opt/homebrew/Cellar/platoon/1.0.0/bin/platoon", None), Install::Homebrew);
        assert_eq!(kind("/home/linuxbrew/.linuxbrew/Cellar/platoon/1.0.0/bin/platoon", None), Install::Homebrew);
        assert_eq!(kind("/home/u/.cargo/bin/platoon", None), Install::Cargo);
        assert_eq!(kind("/home/u/.local/bin/platoon", Some(local)), Install::Script);
        assert_eq!(kind("/home/u/.local/bin/platoon", None), Install::Other, "no receipt: not the script's");
        assert_eq!(kind("/home/u/.cargo/bin/platoon", Some(local)), Install::Cargo, "a receipt for another folder");
        assert_eq!(kind("/nix/store/abc-platoon-1.0.0/bin/platoon", None), Install::Other);
        assert_eq!(kind("/home/u/repos/platoon/target/release/platoon", None), Install::Other);
    }

    #[test]
    fn each_install_gets_its_own_update_command() {
        assert_eq!(platoon_line("1.1.0", &Install::Homebrew), "platoon 1.1.0 is out. To update: brew upgrade platoon");
        assert_eq!(platoon_line("1.1.0", &Install::Cargo), "platoon 1.1.0 is out. To update: cargo install platoon-tui");
        assert_eq!(
            platoon_line("1.1.0", &Install::Script),
            "platoon 1.1.0 is out. To update, run the install script again: curl -LsSf \
             https://github.com/ctrader2/platoon/releases/latest/download/platoon-tui-installer.sh | sh"
        );
        assert_eq!(platoon_line("1.1.0", &Install::Other), "platoon 1.1.0 is out: https://github.com/ctrader2/platoon/releases");
    }

    #[test]
    fn each_new_version_is_announced_once_and_both_together_make_one_notice() {
        let s = State::default();
        // Nothing newer: nothing to say.
        assert_eq!(decide(&s, Some("2.1.288"), "1.0.0", "2.1.288", &Install::Cargo), None);
        assert_eq!(decide(&s, None, "1.0.0", "2.1.288", &Install::Cargo), None);
        // A newer Claude Code.
        let (text, after) = decide(&s, Some("2.2.0"), "1.0.0", "2.1.288", &Install::Cargo).unwrap();
        assert_eq!(
            text,
            "Claude Code 2.2.0 is newer than this platoon was checked with (2.1.288). If something looks wrong, update platoon."
        );
        assert_eq!(after.announced_claude.as_deref(), Some("2.2.0"));
        // Said once: not again for the same version, again for the next one.
        assert_eq!(decide(&after, Some("2.2.0"), "1.0.0", "2.1.288", &Install::Cargo), None);
        assert!(decide(&after, Some("2.2.1"), "1.0.0", "2.1.288", &Install::Cargo).is_some());
        // A newer platoon.
        let s = State { latest: Some("1.1.0".into()), ..State::default() };
        let (text, after) = decide(&s, None, "1.0.0", "2.1.288", &Install::Homebrew).unwrap();
        assert_eq!(text, "platoon 1.1.0 is out. To update: brew upgrade platoon");
        assert_eq!(decide(&after, None, "1.0.0", "2.1.288", &Install::Homebrew), None);
        // The latest is this one, or older (running a build ahead of the release): nothing.
        assert_eq!(decide(&s, None, "1.1.0", "2.1.288", &Install::Cargo), None);
        assert_eq!(decide(&s, None, "1.2.0", "2.1.288", &Install::Cargo), None);
        // Both: one notice, the update last.
        let (text, after) = decide(&s, Some("2.2.0"), "1.0.0", "2.1.288", &Install::Cargo).unwrap();
        assert_eq!(
            text,
            "Claude Code 2.2.0 is newer than this platoon was checked with (2.1.288). platoon 1.1.0 is out. To update: cargo install platoon-tui"
        );
        assert_eq!((after.announced_claude.as_deref(), after.announced_platoon.as_deref()), (Some("2.2.0"), Some("1.1.0")));
    }

    #[test]
    fn the_state_file_round_trips_and_keeps_keys_it_does_not_know() {
        let d = tmp("state");
        assert_eq!(State::load(&d), State::default(), "no file");
        std::fs::write(d.join(STATE_FILE), r#"{"checked_at": 5, "latest": "1.1.0", "someday": [1]}"#).unwrap();
        let mut s = State::load(&d);
        assert_eq!((s.checked_at, s.latest.as_deref()), (5, Some("1.1.0")));
        s.announced_claude = Some("2.2.0".into());
        s.save(&d).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join(STATE_FILE)).unwrap()).unwrap();
        assert_eq!(v["someday"], json!([1]));
        assert_eq!(State::load(&d).announced_claude.as_deref(), Some("2.2.0"));
        std::fs::write(d.join(STATE_FILE), "not json").unwrap();
        assert_eq!(State::load(&d), State::default(), "a damaged file is the same as none");
    }

    #[test]
    fn github_is_asked_at_most_once_a_day() {
        let s = State { checked_at: 1000, ..State::default() };
        assert!(State::default().due(5));
        assert!(!s.due(1000 + CHECK_EVERY_SECS - 1));
        assert!(s.due(1000 + CHECK_EVERY_SECS));
        assert!(s.due(999), "the clock went back");
    }

    #[test]
    fn with_the_environment_variable_set_nothing_is_run_and_nothing_is_written() {
        let d = tmp("off");
        let c = Checks { off: true, ..checks(&d, "9.0.0 (Claude Code)", r#"{"tag_name":"v9.0.0"}"#) };
        assert_eq!(run(&c), None);
        assert!(calls(&d, "claude").is_empty() && calls(&d, "curl").is_empty(), "neither stand-in was started");
        assert!(!c.cache.join(STATE_FILE).exists());
        let claude = stand_in(&d, "claude2", "9.0.0 (Claude Code)");
        assert_eq!(setup_line(Some(&claude), true), None);
        assert!(calls(&d, "claude2").is_empty());
    }

    #[test]
    fn a_run_asks_both_once_a_day_and_says_what_it_found() {
        let d = tmp("run");
        let c = checks(&d, "9.0.0 (Claude Code)", r#"{"tag_name":"v9.0.0"}"#);
        let out = run(&c).expect("a notice");
        assert!(out.text.starts_with("Claude Code 9.0.0 is newer than this platoon was checked with"), "{}", out.text);
        assert!(out.text.ends_with("platoon 9.0.0 is out. To update: cargo install platoon-tui"), "{}", out.text);
        assert_eq!(calls(&d, "claude"), ["--version"]);
        let curl = calls(&d, "curl");
        assert_eq!(curl.len(), 1);
        assert!(curl[0].contains("--max-time 5") && curl[0].contains("--proto =https"), "{curl:?}");
        assert!(
            curl[0].contains(&format!("User-Agent: platoon/{}", running())) && curl[0].ends_with("https://example.invalid/releases/latest")
        );
        // The time asked and the answer are saved at once; what was announced only once it has shown (`out.state`).
        let saved = State::load(&c.cache);
        assert_eq!((saved.checked_at, saved.latest.as_deref(), saved.announced_platoon.as_deref()), (c.now, Some("9.0.0"), None));
        out.state.save(&c.cache).unwrap();
        // An hour later: GitHub is not asked, and nothing new is said.
        assert_eq!(run(&Checks { now: c.now + 3600, ..checks(&d, "9.0.0 (Claude Code)", "") }), None);
        assert_eq!(calls(&d, "curl").len(), 1);
    }

    #[test]
    fn the_setting_off_or_any_failure_says_nothing_and_never_fails() {
        let d = tmp("fail");
        // The setting off: GitHub is never asked.
        let c = Checks { update_check: false, ..checks(&d, "2.1.288 (Claude Code)", r#"{"tag_name":"v9.0.0"}"#) };
        assert_eq!(run(&c), None);
        assert!(calls(&d, "curl").is_empty());
        // A private repository: curl -f fails on the 404 and prints nothing. The day's attempt is still remembered.
        let d = tmp("404");
        let mut c = checks(&d, "2.1.288 (Claude Code)", "");
        let failing = d.join("curl404");
        std::fs::write(&failing, "#!/bin/sh\necho 'curl: (22) The requested URL returned error: 404' >&2\nexit 22\n").unwrap();
        std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
        c.curl = failing.display().to_string();
        assert_eq!(run(&c), None);
        assert_eq!(State::load(&c.cache).checked_at, c.now);
        // No curl and no claude at all.
        let c = Checks {
            curl: d.join("no-curl").display().to_string(),
            claude: Some(d.join("no-claude").display().to_string()),
            now: c.now + CHECK_EVERY_SECS,
            ..checks(&d, "", "")
        };
        assert_eq!(run(&c), None);
    }

    #[test]
    fn setup_says_a_newer_claude_code_every_time_and_nothing_otherwise() {
        let d = tmp("setup");
        let newer_one = stand_in(&d, "claude", "9.0.0 (Claude Code)");
        let line = setup_line(Some(&newer_one), false).expect("a line");
        assert_eq!(
            line,
            format!(
                "Claude Code 9.0.0 is newer than this platoon was checked with ({}). If something looks wrong, update platoon.",
                checked_with()
            )
        );
        assert_eq!(setup_line(Some(&newer_one), false), Some(line), "setup is a report: said each time");
        let same = stand_in(&d, "claude-same", &format!("{} (Claude Code)", checked_with()));
        assert_eq!(setup_line(Some(&same), false), None);
        assert_eq!(setup_line(None, false), None);
    }

    #[test]
    fn a_program_that_hangs_is_given_up_on_and_stopped() {
        let d = tmp("hang");
        let p = d.join("slow");
        std::fs::write(&p, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let start = Instant::now();
        assert_eq!(output_of(&p.display().to_string(), &[], Duration::from_millis(300)), None);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
