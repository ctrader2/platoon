//! Looking at other processes: who a process's parent is, and what is running under it. Linux reads
//! `/proc`; anywhere else (macOS) it asks `ps`. Both are best effort, so callers get None when the
//! system couldn't say and must treat that as "unknown".

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    path::Path,
    process::Command,
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
}

/// "pid (comm) S ppid ...": comm may contain spaces or parens, so split after the last ')'.
fn parse_stat(pid: u32, stat: &str) -> Option<Row> {
    let mut rest = stat.rsplit_once(')')?.1.split_whitespace();
    let state = rest.next()?;
    let ppid = rest.next()?.parse().ok()?;
    // Field 22 of the line (fields 3 and 4, state and ppid, are already taken).
    let start = rest.nth(17).and_then(|t| t.parse().ok()).unwrap_or(0);
    Some(Row { pid, ppid, zombie: state.starts_with('Z'), start })
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
        let mut rows = Vec::new();
        for e in fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            // A process that exits while we look simply isn't listed.
            if let Some(row) = fs::read_to_string(e.path().join("stat")).ok().and_then(|s| parse_stat(pid, &s)) {
                rows.push(row);
            }
        }
        return Some(rows);
    }
    let out = Command::new("ps").args(["-A", "-o", "pid=,ppid=,stat=,lstart="]).output().ok().filter(|o| o.status.success())?;
    Some(parse_ps(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_ps(text: &str) -> Vec<Row> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (pid, ppid, stat) = (f.next()?.parse().ok()?, f.next()?.parse().ok()?, f.next()?);
            // The start time is several words ("Sat Sep 19 11:44:41 2026"); only equality matters.
            let when = f.collect::<Vec<_>>().join(" ");
            let start = if when.is_empty() {
                0
            } else {
                let mut h = DefaultHasher::new();
                when.hash(&mut h);
                h.finish() | 1
            };
            Some(Row { pid, ppid, zombie: stat.starts_with('Z'), start })
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
    if let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) {
        let args: Vec<String> = raw.split(|&b| b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        let args = match args.split_last() {
            Some((last, rest)) if last.is_empty() => rest.to_vec(),
            _ => args,
        };
        return (!args.is_empty()).then_some(args);
    }
    if Path::new("/proc/self/stat").exists() {
        return None; // Linux, and this process is gone
    }
    let out = Command::new("ps").args(["-ww", "-o", "command=", "-p", &pid.to_string()]).output().ok().filter(|o| o.status.success())?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!line.is_empty()).then(|| vec![line])
}

/// What is running under `pid` right now (None: couldn't tell).
#[cfg(test)]
pub fn live_descendants(pid: u32) -> Option<Vec<u32>> {
    descendants(&table()?, pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, ppid: u32, zombie: bool) -> Row {
        Row { pid, ppid, zombie, start: 0 }
    }

    #[test]
    fn stat_lines_parse_even_when_the_name_has_spaces_and_parens() {
        assert_eq!(parse_stat(7, "7 (my (weird) name) S 42 7 7 0 -1"), Some(row(7, 42, false)));
        // Field 22 is the start time, here 987654.
        let full = "7 (a b) S 42 7 7 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 987654 1000 100 18446744073709551615";
        assert_eq!(parse_stat(7, full), Some(Row { pid: 7, ppid: 42, zombie: false, start: 987654 }));
        assert_eq!(parse_stat(8, "8 (sh) Z 1 8 8 0 -1"), Some(row(8, 1, true)));
        assert_eq!(parse_stat(9, "garbage"), None);
    }

    #[test]
    fn ps_output_parses() {
        assert_eq!(parse_ps("  1     0 Ss\n 20     1 Z+\nbad line\n"), vec![row(1, 0, false), row(20, 1, true)]);
        let rows = parse_ps(
            "  5     1 S    Sat Sep 19 11:44:41 2026\n  6     1 S    Sat Sep 19 11:44:42 2026\n  7     1 S    Sat Sep 19 11:44:41 2026\n",
        );
        assert!(rows.iter().all(|r| r.start != 0));
        assert_eq!((rows[0].start == rows[2].start, rows[0].start == rows[1].start), (true, false), "same start time = same reading");
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
        assert_eq!(mine.ppid, std::process::id());
    }
}
