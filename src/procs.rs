//! Looking at other processes: who a process's parent is, and what is running under it. Linux reads
//! `/proc`; anywhere else (macOS) it asks `ps`, and on macOS the kernel itself for a process's arguments and
//! whether it is a zombie. All are best effort, so callers get None when the system couldn't say and must
//! treat that as "unknown".

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    path::Path,
    process::Command,
    time::Duration,
};

/// One row of the process table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub pid: u32,
    pub ppid: u32,
    /// Has exited but not been reaped yet: still listed, but running nothing.
    pub zombie: bool,
    /// When the process started, in a unit that only means something next to another reading of the same
    /// process (Linux: clock ticks after boot; elsewhere a hash of `ps`'s start time). 0 = unknown. A pid that
    /// was reused by a new process has a different one, which is how "still the same process" is checked.
    pub start: u64,
    /// How long it has been running, when the system said (Linux: its start against the time since boot; macOS: `ps`'s
    /// elapsed time).
    pub age: Option<Duration>,
}

/// "pid (comm) S ppid ...": comm may contain spaces or parens, so split after the last ')'.
fn parse_stat(pid: u32, stat: &str) -> Option<Row> {
    let mut rest = stat.rsplit_once(')')?.1.split_whitespace();
    let state = rest.next()?;
    let ppid = rest.next()?.parse().ok()?;
    // Field 22 of the line (fields 3 and 4, state and ppid, are already taken).
    let start = rest.nth(17).and_then(|t| t.parse().ok()).unwrap_or(0);
    Some(Row { pid, ppid, zombie: state.starts_with('Z'), start, age: None })
}

/// How long a process whose start (clock ticks after boot, `Row.start` on Linux) is `start` has been running, given the
/// time since boot (`/proc/uptime`'s first figure) and the clock's ticks per second. None when either can't be read, or
/// they disagree (a start after now): an age is only ever what was measured.
fn age_of(start: u64, uptime: Option<f64>, hz: u64) -> Option<Duration> {
    let up = uptime.filter(|u| u.is_finite())?;
    if start == 0 || hz == 0 {
        return None;
    }
    let secs = up - start as f64 / hz as f64;
    (secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

/// `/proc/uptime`'s first figure: seconds since boot.
fn uptime() -> Option<f64> {
    fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()
}

/// `ps`'s elapsed time, `[[dd-]hh:]mm:ss`.
fn parse_etime(text: &str) -> Option<Duration> {
    let (days, rest) = match text.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, text),
    };
    let mut secs = 0u64;
    let parts: Vec<&str> = rest.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    for p in parts {
        secs = secs * 60 + p.parse::<u64>().ok()?;
    }
    Some(Duration::from_secs(days * 86_400 + secs))
}

pub fn ppid(pid: u32) -> Option<u32> {
    if let Ok(s) = fs::read_to_string(format!("/proc/{pid}/stat")) {
        return parse_stat(pid, &s).map(|r| r.ppid);
    }
    let out = Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Every process on the machine, or None if the table can't be read at all.
pub fn table() -> Option<Vec<Row>> {
    if Path::new("/proc/self/stat").exists() {
        let (up, hz) = (uptime(), u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).unwrap_or(0));
        let mut rows = Vec::new();
        for e in fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            // A process that exits while we look simply isn't listed.
            if let Some(mut row) = fs::read_to_string(e.path().join("stat")).ok().and_then(|s| parse_stat(pid, &s)) {
                row.age = age_of(row.start, up, hz);
                rows.push(row);
            }
        }
        return Some(rows);
    }
    let out = Command::new("ps").args(["-A", "-o", "pid=,ppid=,stat=,etime=,lstart="]).output().ok().filter(|o| o.status.success())?;
    Some(parse_ps(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_ps(text: &str) -> Vec<Row> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (pid, ppid, stat) = (f.next()?.parse().ok()?, f.next()?.parse().ok()?, f.next()?);
            let age = f.next().and_then(parse_etime);
            // The start time is several words ("Sat Sep 19 11:44:41 2026"); only equality matters.
            let when = f.collect::<Vec<_>>().join(" ");
            let start = if when.is_empty() {
                0
            } else {
                let mut h = DefaultHasher::new();
                when.hash(&mut h);
                h.finish() | 1
            };
            Some(Row { pid, ppid, zombie: stat.starts_with('Z'), start, age })
        })
        .collect()
}

/// Live processes anywhere below `root` (children, their children, ...), not counting `root` itself.
/// None if `root` isn't in the table: then nothing can be said about what runs under it.
pub fn descendants(rows: &[Row], root: u32) -> Option<Vec<u32>> {
    if !rows.iter().any(|r| r.pid == root) {
        return None;
    }
    let mut kids: HashMap<u32, Vec<&Row>> = HashMap::new();
    for r in rows {
        kids.entry(r.ppid).or_default().push(r);
    }
    let (mut out, mut todo) = (Vec::new(), vec![root]);
    while let Some(p) = todo.pop() {
        for r in kids.get(&p).into_iter().flatten() {
            if r.pid != root && !out.contains(&r.pid) {
                if !r.zombie {
                    out.push(r.pid);
                }
                todo.push(r.pid);
            }
        }
    }
    Some(out)
}

/// The command line of `pid` as separate arguments, or None if it can't be read (gone, or not ours). Where
/// only `ps` can say (macOS) the arguments come back as one string, since `ps` doesn't keep them apart.
pub fn cmdline(pid: u32) -> Option<Vec<String>> {
    if let Some(args) = cmdline_quick(pid) {
        return Some(args);
    }
    if Path::new("/proc/self/stat").exists() {
        return None; // Linux, and this process is gone
    }
    // The kernel wouldn't say (another user's process, say): `ps` can still show the line, as one string.
    let out = Command::new("ps").args(["-ww", "-o", "command=", "-p", &pid.to_string()]).output().ok().filter(|o| o.status.success())?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!line.is_empty()).then(|| vec![line])
}

/// `cmdline` without ever starting `ps`, for reading many processes at once: None also where only `ps` could say
/// (another user's process on macOS).
pub fn cmdline_quick(pid: u32) -> Option<Vec<String>> {
    if let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) {
        let args: Vec<String> = raw.split(|&b| b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        let args = match args.split_last() {
            Some((last, rest)) if last.is_empty() => rest.to_vec(),
            _ => args,
        };
        return (!args.is_empty()).then_some(args);
    }
    #[cfg(target_os = "macos")]
    return procargs(pid);
    #[cfg(not(target_os = "macos"))]
    None
}

/// macOS: the arguments of `pid`, kept apart, from the kernel (`sysctl` `KERN_PROCARGS2`, what `ps` itself reads).
/// None if it won't say: gone, a zombie, or not ours.
#[cfg(target_os = "macos")]
fn procargs(pid: u32) -> Option<Vec<String>> {
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let ok = unsafe { libc::sysctl(mib.as_mut_ptr(), 2, (&mut argmax as *mut libc::c_int).cast(), &mut size, std::ptr::null_mut(), 0) };
    if ok != 0 || argmax <= 0 {
        return None;
    }
    let mut buf = vec![0u8; argmax as usize];
    let mut size = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let ok = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) };
    if ok != 0 {
        return None;
    }
    parse_procargs2(&buf[..size.min(buf.len())])
}

/// `KERN_PROCARGS2`'s layout: argc (a native-endian int), the executable's path, NULs up to the arguments, then
/// argc NUL-terminated arguments (the environment follows; it is not read).
#[cfg(any(target_os = "macos", test))]
fn parse_procargs2(buf: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    if argc <= 0 {
        return None;
    }
    let rest = &buf[4..];
    let rest = &rest[rest.iter().position(|&b| b == 0)?..]; // past the executable's path
    let rest = &rest[rest.iter().position(|&b| b != 0)?..]; // and the NULs after it
    let mut args = Vec::new();
    let mut parts = rest.split(|&b| b == 0);
    for _ in 0..argc {
        args.push(String::from_utf8_lossy(parts.next()?).into_owned());
    }
    // The last argument must have ended inside what was read, or it was cut short.
    parts.next()?;
    Some(args)
}

/// macOS: whether `pid` is a zombie (it has exited and waits for its parent to reap it), from the kernel's
/// `proc_pidinfo`. Err with the errno when it won't say: `ESRCH` when there is no such process.
#[cfg(target_os = "macos")]
pub fn zombie(pid: u32) -> Result<bool, i32> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let n =
        unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
    if n == size { Ok(info.pbi_status == libc::SZOMB) } else { Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)) }
}

/// What is running under `pid` right now (None: couldn't tell).
#[cfg(test)]
pub fn live_descendants(pid: u32) -> Option<Vec<u32>> {
    descendants(&table()?, pid)
}

/// The folder process `pid` works in, or None when the system won't say (gone, another user's). Linux reads
/// `/proc/<pid>/cwd`; elsewhere (macOS) `lsof` is asked, which is slow enough that callers keep the answer.
pub fn cwd(pid: u32) -> Option<std::path::PathBuf> {
    if Path::new("/proc/self/stat").exists() {
        return fs::read_link(format!("/proc/{pid}/cwd")).ok();
    }
    let out = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    lsof_cwd(&String::from_utf8_lossy(&out.stdout))
}

/// The folder in `lsof -d cwd -Fn`'s answer: its `n` line.
fn lsof_cwd(text: &str) -> Option<std::path::PathBuf> {
    text.lines().find_map(|l| l.strip_prefix('n')).filter(|p| p.starts_with('/')).map(std::path::PathBuf::from)
}

pub fn pid_alive(pid: u32) -> bool {
    // Signal 0 checks existence only. EPERM means it exists but belongs to someone else.
    let exists = unsafe { libc::kill(pid as i32, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) };
    alive_verdict(exists, read_proc_stat(pid))
}

/// What reading `/proc/<pid>/stat` came to.
#[derive(Debug, PartialEq)]
#[allow(dead_code)] // each platform makes only some of these (macOS only `Zombie`, `Vanished`, `Unreadable`)
enum StatRead {
    /// No `/proc` on this platform (macOS), or it is not mounted: the file says nothing either way.
    NoProc,
    /// The process vanished under us: the file is gone (NotFound), or the kernel refused the read because
    /// the process was ending (ESRCH, "No such process": it shows up as an unclassified error, not NotFound).
    Vanished,
    /// Some other trouble reading (permission, ...).
    Unreadable,
    Text(String),
    /// No `/proc`, but the kernel said whether it is a zombie (macOS: `procs::zombie`).
    Zombie(bool),
}

#[cfg(target_os = "macos")]
fn read_proc_stat(pid: u32) -> StatRead {
    match crate::procs::zombie(pid) {
        Ok(z) => StatRead::Zombie(z),
        Err(libc::ESRCH) => StatRead::Vanished,
        Err(_) => StatRead::Unreadable,
    }
}

#[cfg(not(target_os = "macos"))]
fn read_proc_stat(pid: u32) -> StatRead {
    if !cfg!(target_os = "linux") {
        return StatRead::NoProc;
    }
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => StatRead::Text(s),
        Err(e) => classify_read_error(&e, std::path::Path::new("/proc/self/stat").exists()),
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn classify_read_error(e: &std::io::Error, proc_is_there: bool) -> StatRead {
    if e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH) {
        // Every pid looks "not found" when /proc is not mounted at all: then we cannot tell.
        if proc_is_there { StatRead::Vanished } else { StatRead::NoProc }
    } else {
        StatRead::Unreadable
    }
}

/// The decision behind `pid_alive`, kept pure so the race can be tested without timing.
///
/// A process that has exited stays a zombie (state `Z`) until its parent reaps it, and a zombie still
/// answers signal 0, though it is gone as far as anyone using it is concerned. And if the parent reaps it
/// between our signal 0 and our read of `/proc/<pid>/stat`, the read fails (NotFound or ESRCH): that also
/// means gone (reading it as "not a zombie, so alive" made a dead process look alive for one call). macOS has
/// no `/proc`: there the kernel is asked directly (`StatRead::Zombie`). Any other read error (say, permission),
/// an unparseable line, or no way to ask at all leaves what the signal said: it exists.
fn alive_verdict(exists_per_kill: bool, stat: StatRead) -> bool {
    if !exists_per_kill {
        return false;
    }
    match stat {
        StatRead::NoProc | StatRead::Unreadable => true,
        StatRead::Vanished => false,
        StatRead::Zombie(z) => !z,
        StatRead::Text(s) => {
            let state = s.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_owned));
            !matches!(state.as_deref(), Some("Z" | "X" | "x"))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_process_s_working_folder_is_read_and_lsofs_answer_parsed() {
        let here = std::env::current_dir().unwrap();
        let got = cwd(std::process::id()).expect("our own working folder");
        assert_eq!(fs::canonicalize(got).unwrap(), fs::canonicalize(&here).unwrap());
        assert_eq!(cwd(4_000_000_000), None, "no such process");
        assert_eq!(lsof_cwd("p123\nfcwd\nn/Users/u/shop\n"), Some(std::path::PathBuf::from("/Users/u/shop")));
        assert_eq!(lsof_cwd("p123\n"), None);
    }

    use super::*;

    #[test]
    fn liveness_is_decided_by_the_signal_and_then_the_state_line() {
        use StatRead::{NoProc, Unreadable, Vanished};
        // Not there per the signal: never alive, whatever the state file says.
        assert!(!alive_verdict(false, stat("S")));
        for r in [NoProc, Vanished, Unreadable, StatRead::Text(String::new())] {
            assert!(!alive_verdict(false, r));
        }
        // There, and running, sleeping, waiting: alive (comm with spaces and a ')' is parsed past).
        for st in ["R", "S", "D", "T", "t", "I"] {
            assert!(alive_verdict(true, stat(st)), "{st}");
        }
        // There but a zombie or dead: gone.
        for st in ["Z", "X", "x"] {
            assert!(!alive_verdict(true, stat(st)), "{st}");
        }
        // The race: the signal worked, then the parent reaped it before we read the state (file gone, or
        // the kernel said ESRCH): gone.
        assert!(!alive_verdict(true, Vanished));
        // Other read trouble, or a line we cannot read, or no /proc (macOS): the signal stands.
        assert!(alive_verdict(true, Unreadable));
        // macOS: the kernel's answer about a zombie decides.
        assert!(alive_verdict(true, StatRead::Zombie(false)) && !alive_verdict(true, StatRead::Zombie(true)));
        assert!(!alive_verdict(false, StatRead::Zombie(false)));
        assert!(alive_verdict(true, StatRead::Text("garbage".into())));
        assert!(alive_verdict(true, StatRead::Text(String::new())));
        assert!(alive_verdict(true, NoProc));
    }

    #[test]
    fn a_failed_read_of_the_state_file_is_sorted_into_gone_or_unknown() {
        use std::io::{Error, ErrorKind};
        let esrch = Error::from_raw_os_error(libc::ESRCH);
        assert_eq!(classify_read_error(&Error::from(ErrorKind::NotFound), true), StatRead::Vanished);
        assert_eq!(classify_read_error(&esrch, true), StatRead::Vanished, "the kernel says ESRCH for a process that is ending");
        assert_eq!(classify_read_error(&Error::from(ErrorKind::NotFound), false), StatRead::NoProc, "no /proc: cannot tell");
        assert_eq!(classify_read_error(&Error::from(ErrorKind::PermissionDenied), true), StatRead::Unreadable);
        assert_eq!(classify_read_error(&Error::from_raw_os_error(libc::EIO), true), StatRead::Unreadable);
    }

    #[test]
    fn a_child_that_has_exited_but_was_not_reaped_yet_is_not_alive() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        // Not reaped: a zombie. The kill takes a moment to land, so wait for it (generously: a loaded machine).
        let start = std::time::Instant::now();
        while pid_alive(pid) {
            assert!(start.elapsed() < std::time::Duration::from_secs(30), "a killed child nobody reaped still reads as alive");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 0, "it is a zombie, which still answers a signal: only its state tells");
        child.wait().unwrap();
    }

    #[test]
    fn pid_alive_tells_a_running_process_from_a_reaped_one() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        assert!(pid_alive(pid));
        child.kill().unwrap();
        child.wait().unwrap(); // reaped: gone for good
        assert!(!pid_alive(pid));
        assert!(pid_alive(std::process::id()));
    }

    fn stat(state: &str) -> StatRead {
        StatRead::Text(format!("123 (a b) c) {state} 1 123 123 0 -1"))
    }

    fn row(pid: u32, ppid: u32, zombie: bool) -> Row {
        Row { pid, ppid, zombie, start: 0, age: None }
    }

    #[test]
    fn stat_lines_parse_even_when_the_name_has_spaces_and_parens() {
        assert_eq!(parse_stat(7, "7 (my (weird) name) S 42 7 7 0 -1"), Some(row(7, 42, false)));
        // Field 22 is the start time, here 987654.
        let full = "7 (a b) S 42 7 7 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 987654 1000 100 18446744073709551615";
        assert_eq!(parse_stat(7, full), Some(Row { pid: 7, ppid: 42, zombie: false, start: 987654, age: None }));
        assert_eq!(parse_stat(8, "8 (sh) Z 1 8 8 0 -1"), Some(row(8, 1, true)));
        assert_eq!(parse_stat(9, "garbage"), None);
    }

    #[test]
    fn ps_output_parses() {
        assert_eq!(parse_ps("  1     0 Ss\n 20     1 Z+\nbad line\n"), vec![row(1, 0, false), row(20, 1, true)]);
        let rows = parse_ps(
            "  5     1 S       01:02 Sat Sep 19 11:44:41 2026\n  6     1 S    1-02:03:04 Sat Sep 19 11:44:42 2026\n  7     1 S  00:00 Sat Sep 19 11:44:41 2026\n",
        );
        assert!(rows.iter().all(|r| r.start != 0));
        assert_eq!((rows[0].start == rows[2].start, rows[0].start == rows[1].start), (true, false), "same start time = same reading");
        let ages: Vec<Option<u64>> = rows.iter().map(|r| r.age.map(|a| a.as_secs())).collect();
        assert_eq!(ages, [Some(62), Some(86_400 + 7384), Some(0)]);
    }

    #[test]
    fn a_process_s_age_is_read_from_its_start_and_the_time_since_boot() {
        assert_eq!(parse_etime("05:07"), Some(Duration::from_secs(307)));
        assert_eq!(parse_etime("2:05:07"), Some(Duration::from_secs(7507)));
        assert_eq!(parse_etime("3-00:00:01"), Some(Duration::from_secs(3 * 86_400 + 1)));
        for bad in ["", "7", "a:b", "1:2:3:4", "x-01:02"] {
            assert_eq!(parse_etime(bad), None, "{bad:?}");
        }
        // Started 10 s after boot (100 ticks a second), 70.5 s since boot: running 60.5 s.
        assert_eq!(age_of(1000, Some(70.5), 100), Some(Duration::from_secs_f64(60.5)));
        assert_eq!(age_of(0, Some(70.5), 100), None, "no start time");
        assert_eq!(age_of(1000, None, 100), None);
        assert_eq!(age_of(1000, Some(70.5), 0), None);
        assert_eq!(age_of(9000, Some(70.5), 100), None, "a start after now is no age at all");
    }

    #[test]
    fn macos_argument_lists_parse_and_a_cut_short_one_is_refused() {
        // argc 3, "/bin/sleep", padding, then "sleep", "31", "a b", then the environment.
        let mut buf = 3i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/bin/sleep\0\0\0\0sleep\x0031\0a b\0PATH=/bin\0\0");
        assert_eq!(parse_procargs2(&buf), Some(vec!["sleep".to_string(), "31".to_string(), "a b".to_string()]));
        // An empty argument in the middle is kept.
        let mut buf = 3i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/bin/x\0x\0\0y\0");
        assert_eq!(parse_procargs2(&buf), Some(vec!["x".to_string(), String::new(), "y".to_string()]));
        // Fewer arguments than argc says, or the last one not ended: nothing rather than a wrong list.
        let mut buf = 3i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/bin/sleep\0sleep\x0031\0");
        assert_eq!(parse_procargs2(&buf), None);
        let mut buf = 1i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/bin/sleep\0sle");
        assert_eq!(parse_procargs2(&buf), None);
        assert_eq!(parse_procargs2(&0i32.to_ne_bytes()), None);
        assert_eq!(parse_procargs2(b"ab"), None);
    }

    #[test]
    fn descendants_are_found_at_any_depth_and_zombies_and_strangers_are_left_out() {
        let rows = [row(10, 1, false), row(11, 10, false), row(12, 11, false), row(13, 10, true), row(20, 1, false), row(21, 20, false)];
        let mut d = descendants(&rows, 10).unwrap();
        d.sort();
        assert_eq!(d, [11, 12], "the child's child counts; the zombie and the unrelated tree don't");
        assert_eq!(descendants(&rows, 12).unwrap(), Vec::<u32>::new());
        assert_eq!(descendants(&rows, 999), None, "a process the table doesn't show is unknown, not childless");
    }

    #[test]
    fn a_cycle_in_a_broken_table_cannot_loop_forever() {
        let rows = [row(5, 6, false), row(6, 5, false)];
        assert_eq!(descendants(&rows, 5).unwrap(), [6]);
    }

    #[test]
    fn the_real_process_table_shows_our_own_child() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let me = std::process::id();
        let mine = live_descendants(me).expect("the process table must be readable");
        let _ = child.kill();
        let _ = child.wait();
        assert!(mine.contains(&child.id()), "{mine:?}");
        assert_eq!(ppid(child.id()), None, "a reaped process has no parent");
        assert_eq!(cmdline(child.id()), None, "and no command line");
    }

    #[test]
    fn a_live_process_shows_its_arguments_and_a_start_time_that_does_not_change() {
        let mut child = Command::new("sleep").arg("631").spawn().unwrap();
        let rows = table().unwrap();
        let mine = rows.iter().find(|r| r.pid == child.id()).copied().unwrap();
        // Right after the spawn returns the new program's arguments can be unset for a moment (Linux reads that as
        // no command line): wait for them, generously, so a loaded machine is slow rather than wrong.
        let since = std::time::Instant::now();
        let mut args = cmdline(child.id());
        while args.is_none() && since.elapsed() < std::time::Duration::from_secs(60) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            args = cmdline(child.id());
        }
        let again = table().unwrap().into_iter().find(|r| r.pid == child.id()).unwrap();
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(args, Some(vec!["sleep".to_string(), "631".to_string()]));
        assert!(mine.start != 0 && mine.start == again.start, "{mine:?} {again:?}");
        assert!(mine.age.is_some_and(|a| a < Duration::from_secs(600)), "just started: {mine:?}");
        assert_eq!(mine.ppid, std::process::id());
    }
}
