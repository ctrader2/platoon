//! `platoon uninstall` and `scripts/uninstall.sh`, end to end, each in a made-up world: HOME, every folder variable and
//! PATH point into a temp folder, with stand-in `brew` and `cargo` that only record what they are asked and change
//! files inside that world. `PLATOON_UNINSTALL_TEST_HOME` makes the script and the command refuse to run anywhere else.
//! The script runs under `sh`, `bash` and (when there is one) `dash`.

mod common;

use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
};

const BIN: &str = env!("CARGO_BIN_EXE_platoon");
const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/uninstall.sh");

struct World {
    base: PathBuf,
    home: PathBuf,
    /// Stand-in programs, first on PATH.
    stubs: PathBuf,
}

impl World {
    fn new(name: &str) -> Self {
        let base = common::tmp(&format!("uninstall-{name}"));
        let (home, stubs) = (base.join("home"), base.join("stubs"));
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(&stubs).unwrap();
        Self { base, home, stubs }
    }

    fn claude(&self) -> PathBuf {
        self.home.join(".claude")
    }
    fn config(&self) -> PathBuf {
        self.home.join(".config/platoon")
    }
    fn cache(&self) -> PathBuf {
        self.home.join(".cache/platoon")
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.base.join("calls")).unwrap_or_default()
    }

    /// A command with nothing of the real environment: only this world's folders, and `answers` (None: no terminal).
    fn command(&self, program: &str, extra_path: &[&Path], answers: Option<&str>) -> Command {
        let mut path: Vec<String> = extra_path.iter().map(|p| p.display().to_string()).collect();
        path.push(self.stubs.display().to_string());
        path.push("/usr/bin:/bin".into());
        let tty = self.base.join("answers");
        match answers {
            Some(a) => fs::write(&tty, a).unwrap(),
            None => {
                let _ = fs::remove_file(&tty);
            }
        }
        let mut c = Command::new(program);
        c.env_clear()
            .env("HOME", &self.home)
            .env("PATH", path.join(":"))
            .env("LANG", "C")
            .env("PLATOON_UNINSTALL_TEST_HOME", &self.base)
            .env("PLATOON_UNINSTALL_TTY", &tty)
            .env("PLATOON_NO_UPDATE_CHECK", "1")
            // Never the real Codex (its folder here is this world's `~/.codex`).
            .env("PLATOON_CODEX_BIN", common::NO_REAL_CODEX[1].1)
            // Claude Code installed, whatever this machine's path has: setup does its steps.
            .env(common::NO_REAL_CLAUDE.0, common::NO_REAL_CLAUDE.1)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        c
    }

    /// `platoon setup` with the real binary, as a user would, on top of `user`'s settings.
    fn set_up(&self, user: &Value) {
        fs::write(self.claude().join("settings.json"), serde_json::to_string_pretty(user).unwrap()).unwrap();
        let out = self.command(BIN, &[], None).arg("setup").output().unwrap();
        assert!(out.status.success(), "{}", text(&out));
        fs::create_dir_all(self.config().join("projects")).unwrap();
        fs::create_dir_all(self.cache().join("status")).unwrap();
    }

    fn stub(&self, name: &str, body: &str) {
        let p = self.stubs.join(name);
        fs::write(&p, format!("#!/bin/sh\necho \"{name} $*\" >> '{}'\n{body}\n", self.base.join("calls").display())).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn settings(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.claude().join("settings.json")).unwrap()).unwrap()
    }
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// The shells the script must work in: `sh`, `bash`, and `dash` where it is installed.
fn shells() -> Vec<&'static str> {
    let mut v = vec!["sh", "bash"];
    if Path::new("/usr/bin/dash").exists() || Path::new("/bin/dash").exists() {
        v.push("dash");
    }
    v
}

fn user_settings() -> Value {
    json!({
        "model": "opus",
        "statusLine": null,
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}]},
        "permissions": {"allow": ["Bash(ls)"]}
    })
}

/// A stand-in "claude" this test started (never anyone else's), reaped as soon as it ends so it doesn't linger as a
/// zombie that still looks alive.
fn sleeper() -> (u32, std::thread::JoinHandle<std::process::ExitStatus>) {
    let mut c: Child = Command::new("sleep").arg("60").stdin(Stdio::null()).spawn().unwrap();
    let pid = c.id();
    (pid, std::thread::spawn(move || c.wait().unwrap()))
}

fn end(pid: u32, waiter: std::thread::JoinHandle<std::process::ExitStatus>) -> Option<i32> {
    if common::alive(pid) {
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
    use std::os::unix::process::ExitStatusExt;
    waiter.join().unwrap().signal()
}

fn host_entry(w: &World, pid: u32) {
    let hosts = w.cache().join("hosts");
    fs::create_dir_all(&hosts).unwrap();
    fs::write(
        hosts.join(format!("{pid}.json")),
        json!({"claude_pid": pid, "host_pid": pid, "dir": "/work/app", "args": ["--resume", "sess-42"]}).to_string(),
    )
    .unwrap();
}

// ---- platoon uninstall, the real binary --------------------------------------------------------------------------

#[test]
fn uninstall_takes_out_exactly_what_the_real_setup_added_and_a_second_run_finds_nothing() {
    let w = World::new("roundtrip");
    let mut user = user_settings();
    user.as_object_mut().unwrap().remove("statusLine");
    w.set_up(&user);
    let set_up = fs::read_to_string(w.claude().join("settings.json")).unwrap();
    assert!(set_up.contains("hook guard") && set_up.contains("hook session-start") && set_up.contains(" statusline"), "{set_up}");
    let skill = w.claude().join("skills/to-tickets/SKILL.md");
    assert!(fs::read_to_string(&skill).unwrap().contains(&format!("{BIN} task add")), "setup wrote the skill, naming itself");

    let out = w.command(BIN, &[], Some("n\n")).arg("uninstall").output().unwrap();
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert_eq!(w.settings(), user, "{said}");
    assert_eq!(fs::read_to_string(w.claude().join("settings.json.platoon-bak")).unwrap(), set_up);
    assert!(!w.cache().exists() && w.config().exists(), "{said}");
    for want in [
        "Also delete your projects, notes, tasks and settings? [y/N]",
        "Removed:",
        "the status line, the session guard hook, the project notes hook, the notes updates hook",
        &format!("the /to-tickets skill ({})", skill.display()),
        "platoon's cache folder",
        "Kept:",
        "To delete them later: rm -rf",
        "your conversations: they are Claude Code's",
        "Last step, remove the program: rm ",
    ] {
        assert!(said.contains(want), "{want}: {said}");
    }

    let again = w.command(BIN, &[], None).arg("uninstall").output().unwrap();
    let said = text(&again);
    assert_eq!(again.status.code(), Some(0), "{said}");
    assert!(said.contains("Nothing of platoon's was left to remove.") && !said.contains("Removed:"), "{said}");
    assert!(said.contains("there was no terminal to ask in; --all deletes it"), "{said}");
    assert_eq!(w.settings(), user);
    assert!(!skill.parent().unwrap().exists() && w.claude().join("skills").is_dir(), "only the skill's own folder went");
}

#[test]
fn uninstall_and_the_script_keep_a_to_tickets_skill_platoon_did_not_write() {
    for (sh, with_platoon) in [("sh", true)].into_iter().chain(shells().into_iter().map(|s| (s, false))) {
        let w = World::new(&format!("own-skill-{sh}-{with_platoon}"));
        w.set_up(&json!({}));
        let skill = w.claude().join("skills/to-tickets/SKILL.md");
        let mine = fs::read_to_string(&skill).unwrap().replace("Fewer is better", "Fewer is nicer");
        fs::write(&skill, &mine).unwrap();
        let out = if with_platoon {
            w.command(BIN, &[], Some("n\n")).arg("uninstall").output().unwrap()
        } else {
            w.command(sh, &[], Some("n\n")).arg(SCRIPT).output().unwrap()
        };
        let said = text(&out);
        assert_eq!(out.status.code(), Some(0), "{sh}: {said}");
        assert_eq!(fs::read_to_string(&skill).unwrap(), mine, "{sh}: {said}");
        let line = format!(
            "the /to-tickets skill at {}: platoon didn't write it (or it was changed since), so it is yours and stays",
            skill.display()
        );
        assert!(said.contains(&line), "{sh}: {said}");
        assert!(!said.contains("the /to-tickets skill ("), "{sh}: {said}");
    }
}

#[test]
fn uninstall_refuses_to_run_outside_the_test_home_and_rejects_unknown_flags() {
    let w = World::new("guard");
    w.set_up(&json!({}));
    let before = fs::read_to_string(w.claude().join("settings.json")).unwrap();
    let out = w.command(BIN, &[], None).arg("uninstall").env("PLATOON_CACHE_DIR", "/var/tmp/not-here").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out).contains("refusing to run"), "{}", text(&out));
    let out = w.command(BIN, &[], None).args(["uninstall", "--yes"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out).contains("don't know \"--yes\"") && text(&out).contains("--stop-sessions"), "{}", text(&out));
    assert_eq!(fs::read_to_string(w.claude().join("settings.json")).unwrap(), before, "nothing changed");
    let help = w.command(BIN, &[], None).args(["uninstall", "--help"]).output().unwrap();
    assert!(help.status.success() && text(&help).contains("--all"));
}

#[test]
fn uninstall_stops_running_sessions_only_after_a_yes() {
    let w = World::new("sessions");
    w.set_up(&json!({}));
    let (pid, waiter) = sleeper();
    host_entry(&w, pid);
    // No, then no terminal at all: nothing changes, the session runs on.
    for answers in [Some("n\n"), None] {
        let out = w.command(BIN, &[], answers).args(["uninstall", "--all"]).output().unwrap();
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{said}");
        assert!(said.contains(&format!("in /work/app (pid {pid}): `claude --resume sess-42` opens it again")), "{said}");
        assert!(said.contains("Nothing was changed") || said.contains("changed nothing"), "{said}");
        assert!(common::alive(pid) && w.config().exists() && w.settings().to_string().contains("platoon"));
    }
    let out = w.command(BIN, &[], Some("y\n")).args(["uninstall", "--all"]).output().unwrap();
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("Stop it and go on? [y/N]") && said.contains("stopped (their conversations are saved)"), "{said}");
    assert!(!said.contains("Also delete"), "--all doesn't ask: {said}");
    assert_eq!(end(pid, waiter), Some(libc::SIGTERM), "stopped the gentle way");
    assert!(!w.config().exists() && !w.cache().exists());
}

// ---- the script -----------------------------------------------------------------------------------------------------

#[test]
fn the_script_runs_platoon_uninstall_then_removes_a_cargo_install() {
    for sh in shells() {
        let w = World::new(&format!("cargo-{sh}"));
        w.set_up(&json!({"theme": "dark"}));
        let cargo_bin = w.home.join(".cargo/bin");
        fs::create_dir_all(&cargo_bin).unwrap();
        symlink(BIN, cargo_bin.join("platoon")).unwrap();
        w.stub(
            "cargo",
            r#"case "$1" in
  install) [ -e "$HOME/.cargo/bin/platoon" ] && printf 'platoon-tui v1.0.0:\n    platoon\n'; exit 0 ;;
  uninstall) [ "$2" = platoon-tui ] && rm -f "$HOME/.cargo/bin/platoon"; exit 0 ;;
esac"#,
        );
        let out = w.command(sh, &[&cargo_bin], None).arg(SCRIPT).output().unwrap();
        let said = text(&out);
        assert_eq!(out.status.code(), Some(0), "{sh}: {said}");
        assert_eq!(w.settings(), json!({"theme": "dark"}), "{sh}: {said}");
        assert!(!cargo_bin.join("platoon").exists(), "{sh}: {said}");
        assert!(w.calls().contains("cargo uninstall platoon-tui"), "{sh}: {}", w.calls());
        assert!(!w.cache().exists() && w.config().exists(), "no terminal: the config is kept: {said}");
        assert!(said.contains("the program, with cargo (cargo uninstall platoon-tui)"), "{sh}: {said}");
        assert!(said.contains("there was no terminal to ask in; --all deletes it"), "{sh}: {said}");
        assert!(!said.contains("Last step"), "the script removes the program itself: {said}");
        assert!(said.trim_end().ends_with("platoon is uninstalled."), "{sh}: {said}");
    }
}

#[test]
fn the_script_removes_a_homebrew_install_and_all_deletes_the_config_without_asking() {
    for sh in shells() {
        let w = World::new(&format!("brew-{sh}"));
        w.set_up(&json!({}));
        let cellar = w.base.join("brew/Cellar/platoon/1.0.0/bin");
        let brew_bin = w.base.join("brew/bin");
        fs::create_dir_all(&cellar).unwrap();
        fs::create_dir_all(&brew_bin).unwrap();
        symlink(BIN, cellar.join("platoon")).unwrap();
        symlink(cellar.join("platoon"), brew_bin.join("platoon")).unwrap();
        w.stub(
            "brew",
            &format!(
                r#"case "$1" in
  list) [ -e '{c}' ] ;;
  uninstall) rm -rf '{c}' '{b}'; exit 0 ;;
esac"#,
                c = cellar.join("platoon").display(),
                b = brew_bin.join("platoon").display()
            ),
        );
        let out = w.command(sh, &[&brew_bin], Some("")).args([SCRIPT, "--all"]).output().unwrap();
        let said = text(&out);
        assert_eq!(out.status.code(), Some(0), "{sh}: {said}");
        assert!(w.calls().contains("brew uninstall platoon"), "{sh}: {}", w.calls());
        assert!(!w.config().exists() && !w.cache().exists() && !brew_bin.join("platoon").exists(), "{sh}: {said}");
        assert!(!said.contains("Also delete"), "{sh}: {said}");
        assert!(said.contains("the program, with Homebrew (brew uninstall platoon)"), "{sh}: {said}");
        assert_eq!(w.settings(), json!({}), "{sh}");
    }
}

#[test]
fn the_script_removes_an_install_script_install_and_only_its_line_in_the_startup_files() {
    for sh in shells() {
        let w = World::new(&format!("receipt-{sh}"));
        w.set_up(&json!({}));
        let bin = w.home.join(".local/bin");
        let receipt_dir = w.home.join(".config/platoon-tui");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&receipt_dir).unwrap();
        symlink(BIN, bin.join("platoon")).unwrap();
        fs::write(bin.join("uv"), "another tool").unwrap();
        let receipt = json!({"binaries": ["platoon"], "binary_aliases": {}, "install_layout": "flat",
            "install_prefix": bin, "modify_path": true, "provider": {"source": "cargo-dist", "version": "0.33.0"}});
        fs::write(receipt_dir.join("platoon-tui-receipt.json"), receipt.to_string()).unwrap();
        fs::write(receipt_dir.join("env.sh"), "export PATH=...\n").unwrap();
        let profile = "export EDITOR=vi\n\n. \"$HOME/.config/platoon-tui/env.sh\"\n. \"$HOME/.local/bin/env\"\n";
        fs::write(w.home.join(".profile"), profile).unwrap();
        fs::write(w.home.join(".bashrc"), "alias ll='ls -l'\n").unwrap();
        let zdot = w.home.join("zsh");
        fs::create_dir_all(&zdot).unwrap();
        fs::write(zdot.join(".zshrc"), "\n. \"$HOME/.config/platoon-tui/env.sh\"\n").unwrap();
        let fish = w.home.join(".config/fish/conf.d");
        fs::create_dir_all(&fish).unwrap();
        fs::write(fish.join("platoon-tui.env.fish"), "source \"$HOME/.config/platoon-tui/env.fish\"\n").unwrap();
        let out = w.command(sh, &[&bin], Some("y\n")).env("ZDOTDIR", &zdot).arg(SCRIPT).output().unwrap();
        let said = text(&out);
        assert_eq!(out.status.code(), Some(0), "{sh}: {said}");
        assert!(!bin.join("platoon").exists() && bin.join("uv").exists(), "{sh}: {said}");
        assert!(!receipt_dir.exists() && !fish.join("platoon-tui.env.fish").exists(), "{sh}: {said}");
        assert_eq!(
            fs::read_to_string(w.home.join(".profile")).unwrap(),
            "export EDITOR=vi\n\n. \"$HOME/.local/bin/env\"\n",
            "only platoon's line goes: another tool's stays"
        );
        assert_eq!(fs::read_to_string(w.home.join(".bashrc")).unwrap(), "alias ll='ls -l'\n");
        assert_eq!(fs::read_to_string(zdot.join(".zshrc")).unwrap(), "\n");
        assert!(said.contains("Also delete your projects, notes, tasks and settings? [y/N]"), "{sh}: {said}");
        assert!(!w.config().exists(), "answered yes: {said}");
        assert!(said.contains("the program, put there by the install script"), "{sh}: {said}");
    }
}

#[test]
fn the_script_stops_with_nothing_changed_when_platoon_uninstall_is_told_no() {
    for sh in shells() {
        let w = World::new(&format!("no-{sh}"));
        w.set_up(&json!({}));
        let cargo_bin = w.home.join(".cargo/bin");
        fs::create_dir_all(&cargo_bin).unwrap();
        symlink(BIN, cargo_bin.join("platoon")).unwrap();
        w.stub("cargo", "exit 0");
        let (pid, waiter) = sleeper();
        host_entry(&w, pid);
        let before = fs::read_to_string(w.claude().join("settings.json")).unwrap();
        let out = w.command(sh, &[&cargo_bin], Some("n\n")).arg(SCRIPT).output().unwrap();
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{sh}: {said}");
        assert!(said.contains("Left them running. Nothing was changed.") && said.contains("the program was left where it is"), "{said}");
        assert!(common::alive(pid) && cargo_bin.join("platoon").exists() && w.cache().exists());
        assert_eq!(fs::read_to_string(w.claude().join("settings.json")).unwrap(), before);
        assert!(!w.calls().contains("cargo"), "{}", w.calls());
        end(pid, waiter);
    }
}

#[test]
fn without_platoon_the_script_asks_the_same_questions_cleans_the_folders_and_says_what_to_do_by_hand() {
    for sh in shells() {
        let w = World::new(&format!("none-{sh}"));
        w.set_up(&json!({}));
        fs::create_dir_all(w.home.join(".local/share/platoon/editor")).unwrap();
        // A task's worktree, made the way platoon makes one.
        let repo = w.base.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let ok = Command::new("git").arg("-C").arg(dir).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
            assert!(ok.success(), "git {args:?}");
        };
        git(&repo, &["init", "-q"]);
        fs::write(repo.join("a"), "a").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "a"]);
        let wt = w.base.join("repo-worktrees/task-4");
        git(&repo, &["worktree", "add", "-q", "-b", "task/4", wt.to_str().unwrap()]);
        let project = w.config().join("projects/p1");
        fs::create_dir_all(&project).unwrap();
        let tasks = json!({"next": 5, "tasks": [{"number": 4, "title": "t", "file": "04.md", "state": "done", "folder": repo,
            "worktree": {"path": wt, "branch": "task/4"}}]});
        fs::write(project.join("tasks.json"), serde_json::to_string_pretty(&tasks).unwrap()).unwrap();
        let (pid, waiter) = sleeper();
        host_entry(&w, pid);

        // No platoon anywhere. A "no" to the sessions: nothing changes.
        let out = w.command(sh, &[], Some("n\n")).arg(SCRIPT).output().unwrap();
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{sh}: {said}");
        assert!(said.contains("No platoon program was found") && said.contains("Left them running. Nothing was changed."), "{said}");
        assert!(common::alive(pid) && w.cache().exists() && w.config().exists());
        let skill = w.claude().join("skills/to-tickets/SKILL.md");
        assert!(skill.is_file(), "{sh}: untouched after a no");

        // Yes and yes: the session stops, the folders go, the worktree is listed and kept.
        let out = w.command(sh, &[], Some("y\ny\n")).arg(SCRIPT).output().unwrap();
        let said = text(&out);
        assert_eq!(out.status.code(), Some(0), "{sh}: {said}");
        assert!(said.contains(&format!("in /work/app (pid {pid}): `claude --resume sess-42` opens it again")), "{said}");
        assert!(said.contains("Also delete your projects, notes, tasks and settings? [y/N]"), "{said}");
        assert_eq!(end(pid, waiter), Some(libc::SIGTERM), "{sh}");
        assert!(!w.cache().exists() && !w.config().exists() && !w.home.join(".local/share/platoon").exists(), "{said}");
        let repo_s = repo.display().to_string();
        assert!(said.contains(&format!("git -C {repo_s} worktree remove {}", wt.display())), "{sh}: {said}");
        assert!(said.contains(&format!("git -C {repo_s} branch -D task/4")), "{sh}: {said}");
        assert!(wt.join("a").exists(), "never deleted");
        // The skill setup wrote is told apart with `cksum` alone, and goes with its folder.
        assert!(said.contains(&format!("the /to-tickets skill ({})", skill.display())), "{sh}: {said}");
        assert!(!skill.parent().unwrap().exists(), "{sh}");
        // Claude Code's settings can't be taken out without platoon: said, with how.
        assert!(said.contains("Claude Code's settings still mention platoon") && said.contains("platoon hook guard"), "{said}");
        assert!(w.settings().to_string().contains("hook guard"));
        assert!(said.trim_end().ends_with("platoon is uninstalled, but for what is above to do by hand."), "{said}");
        assert!(said.contains("platoon is running 1 session:") && said.contains("Stop it and go on? [y/N]"), "{said}");
    }
}

#[test]
fn the_script_without_a_terminal_and_with_sessions_running_changes_nothing() {
    let w = World::new("none-notty");
    let (pid, waiter) = sleeper();
    host_entry(&w, pid);
    fs::create_dir_all(w.config()).unwrap();
    let out = w.command("sh", &[], None).arg(SCRIPT).output().unwrap();
    let said = text(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(said.contains("no terminal to ask in") && said.contains("--stop-sessions"), "{said}");
    assert!(common::alive(pid) && w.cache().exists() && w.config().exists());
    // --stop-sessions says yes; without a terminal the config folder is still kept.
    let out = w.command("sh", &[], None).args([SCRIPT, "--stop-sessions"]).output().unwrap();
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert_eq!(end(pid, waiter), Some(libc::SIGTERM));
    assert!(!w.cache().exists() && w.config().exists(), "{said}");
}

#[test]
fn an_old_platoon_without_uninstall_is_left_on_the_path_with_how_to_delete_it() {
    let w = World::new("old");
    let old = w.base.join("old-bin");
    fs::create_dir_all(&old).unwrap();
    // An older platoon: no --version, so no `uninstall` either.
    fs::write(old.join("platoon"), "#!/bin/sh\necho usage >&2\nexit 2\n").unwrap();
    fs::set_permissions(old.join("platoon"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(w.cache()).unwrap();
    let out = w.command("sh", &[&old], None).arg(SCRIPT).output().unwrap();
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("is too old to uninstall itself"), "{said}");
    assert!(
        said.contains(&format!("the program at {}/platoon: it wasn't installed with Homebrew, cargo or the install script", old.display())),
        "{said}"
    );
    assert!(said.contains(&format!("rm {}/platoon", old.display())) && old.join("platoon").exists(), "{said}");
    assert!(!w.cache().exists());
}

#[test]
fn the_script_refuses_to_run_outside_its_test_home_and_rejects_unknown_flags() {
    let w = World::new("script-guard");
    fs::create_dir_all(w.cache()).unwrap();
    let out = w.command("sh", &[], None).arg(SCRIPT).env("HOME", "/var/tmp").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out).contains("refusing to run"), "{}", text(&out));
    let out = w.command("sh", &[], None).arg(SCRIPT).env("PLATOON_CONFIG_DIR", "/etc/x").output().unwrap();
    assert!(out.status.code() == Some(1) && text(&out).contains("/etc/x is outside it"), "{}", text(&out));
    let out = w.command("sh", &[], None).args([SCRIPT, "--yes"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out).contains("don't know \"--yes\""));
    assert!(w.cache().exists(), "nothing changed");
}

// ---- a real terminal: `curl ... | sh`, with the answers typed into /dev/tty --------------------------------------

/// Runs `cat <script> | sh -s -- <flags>` in a pseudo-terminal (stdin is the pipe, as with curl; the questions must go
/// through /dev/tty), types `typed` at each `[y/N]`, and returns everything shown and the exit code.
fn piped_into_sh_in_a_terminal(w: &World, extra_path: &[&Path], flags: &str, typed: &[&str]) -> (String, u32) {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::{
        io::{Read, Write},
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    let pair = native_pty_system().openpty(PtySize { rows: 50, cols: 200, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut path: Vec<String> = extra_path.iter().map(|p| p.display().to_string()).collect();
    path.push(w.stubs.display().to_string());
    path.push("/usr/bin:/bin".into());
    let mut cmd = CommandBuilder::new("sh");
    cmd.args(["-c", &format!("cat '{SCRIPT}' | sh -s -- {flags}")]);
    cmd.env_clear();
    for (k, v) in [
        ("HOME", w.home.display().to_string()),
        ("PATH", path.join(":")),
        ("LANG", "C".into()),
        ("PLATOON_UNINSTALL_TEST_HOME", w.base.display().to_string()),
        ("PLATOON_NO_UPDATE_CHECK", "1".into()),
        ("NO_COLOR", "1".into()),
    ] {
        cmd.env(k, v);
    }
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let seen = Arc::new(Mutex::new(String::new()));
    let sink = seen.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    let wait_for = |count: usize| {
        let start = Instant::now();
        while seen.lock().unwrap().matches("[y/N]").count() < count {
            assert!(start.elapsed() < Duration::from_secs(20), "no question {count}: {}", seen.lock().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    for (i, t) in typed.iter().enumerate() {
        wait_for(i + 1);
        writer.write_all(t.as_bytes()).unwrap();
        writer.flush().unwrap();
    }
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "didn't finish: {}", seen.lock().unwrap());
        std::thread::sleep(Duration::from_millis(20));
    };
    std::thread::sleep(Duration::from_millis(100));
    let shown = seen.lock().unwrap().clone();
    (shown, status.exit_code())
}

#[test]
fn piped_into_sh_platoon_still_asks_in_the_terminal() {
    let w = World::new("pty");
    w.set_up(&json!({}));
    let cargo_bin = w.home.join(".cargo/bin");
    fs::create_dir_all(&cargo_bin).unwrap();
    symlink(BIN, cargo_bin.join("platoon")).unwrap();
    w.stub("cargo", "exit 0");
    let (pid, waiter) = sleeper();
    host_entry(&w, pid);
    let (shown, code) = piped_into_sh_in_a_terminal(&w, &[&cargo_bin], "", &["y\n", "y\n"]);
    assert_eq!(code, 0, "{shown}");
    assert!(shown.contains("Stop it and go on? [y/N]") && shown.contains("Also delete your projects"), "{shown}");
    assert_eq!(end(pid, waiter), Some(libc::SIGTERM));
    assert!(!w.config().exists() && !w.cache().exists() && w.settings() == json!({}), "{shown}");
}

#[test]
fn piped_into_sh_without_platoon_the_script_asks_in_the_terminal() {
    let w = World::new("pty-none");
    fs::create_dir_all(w.config()).unwrap();
    fs::create_dir_all(w.cache()).unwrap();
    // "n": the config folder is kept.
    let (shown, code) = piped_into_sh_in_a_terminal(&w, &[], "", &["n\n"]);
    assert_eq!(code, 0, "{shown}");
    assert!(shown.contains("Also delete your projects, notes, tasks and settings? [y/N]"), "{shown}");
    assert!(w.config().exists() && !w.cache().exists(), "{shown}");
    let (shown, code) = piped_into_sh_in_a_terminal(&w, &[], "", &["y\n"]);
    assert_eq!(code, 0, "{shown}");
    assert!(!w.config().exists(), "{shown}");
}
