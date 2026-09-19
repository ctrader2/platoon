//! Looking at other processes: who a process's parent is, and what is running under it. Linux reads
//! `/proc`; anywhere else (macOS) it asks `ps`. Both are best effort, so callers get None when the
//! system couldn't say and must treat that as "unknown".

use std::{collections::HashMap, fs, path::Path, process::Command};

/// One row of the process table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub pid: u32,
    pub ppid: u32,
    /// Has exited but not been reaped yet: still listed, but running nothing.
    pub zombie: bool,
}

/// "pid (comm) S ppid ...": comm may contain spaces or parens, so split after the last ')'.
fn parse_stat(pid: u32, stat: &str) -> Option<Row> {
    let mut rest = stat.rsplit_once(')')?.1.split_whitespace();
    let state = rest.next()?;
    Some(Row { pid, ppid: rest.next()?.parse().ok()?, zombie: state.starts_with('Z') })
}

pub fn ppid(pid: u32) -> Option<u32> {
    if let Ok(s) = fs::read_to_string(format!("/proc/{pid}/stat")) {
        return parse_stat(pid, &s).map(|r| r.ppid);
    }
    let out = Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Every process on the machine, or None if the table can't be read at all.
fn table() -> Option<Vec<Row>> {
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
    let out = Command::new("ps").args(["-A", "-o", "pid=,ppid=,stat="]).output().ok().filter(|o| o.status.success())?;
    Some(parse_ps(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_ps(text: &str) -> Vec<Row> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some(Row { pid: f.next()?.parse().ok()?, ppid: f.next()?.parse().ok()?, zombie: f.next()?.starts_with('Z') })
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

/// What is running under `pid` right now (None: couldn't tell).
pub fn live_descendants(pid: u32) -> Option<Vec<u32>> {
    descendants(&table()?, pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, ppid: u32, zombie: bool) -> Row {
        Row { pid, ppid, zombie }
    }

    #[test]
    fn stat_lines_parse_even_when_the_name_has_spaces_and_parens() {
        assert_eq!(parse_stat(7, "7 (my (weird) name) S 42 7 7 0 -1"), Some(row(7, 42, false)));
        assert_eq!(parse_stat(8, "8 (sh) Z 1 8 8 0 -1"), Some(row(8, 1, true)));
        assert_eq!(parse_stat(9, "garbage"), None);
    }

    #[test]
    fn ps_output_parses() {
        assert_eq!(parse_ps("  1     0 Ss\n 20     1 Z+\nbad line\n"), vec![row(1, 0, false), row(20, 1, true)]);
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
    }
}
