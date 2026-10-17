//! What a task's session spent, as its transcript says (read by `claude::read_session_log`, off the UI thread, with
//! its subagents' transcripts beside it), said short for a row and in full for the details. Pure.

use crate::claude::Spent;

/// A count of tokens, short: 950, 12K, 1.2M (one decimal below ten).
pub fn short(n: u64) -> String {
    let one = |v: u64, unit: u64, suffix: &str| {
        let tenths = (v * 10 + unit / 2) / unit;
        if tenths < 100 && !tenths.is_multiple_of(10) {
            format!("{}.{}{suffix}", tenths / 10, tenths % 10)
        } else {
            format!("{}{suffix}", (v + unit / 2) / unit)
        }
    };
    match n {
        n if n < 1000 => n.to_string(),
        n if n < 999_500 => one(n, 1000, "K"),
        n => one(n, 1_000_000, "M"),
    }
}

/// What the short totals count: tokens sent to Claude and written by it. Cache reads (the conversation so far, read
/// back each turn) are left out: they are cheap and would make every total look huge. The details give them apart.
pub fn sent_and_written(s: &Spent) -> u64 {
    s.input + s.output
}

/// A row's total: "84K tokens".
pub fn row_total(s: &Spent) -> String {
    format!("{} tokens", short(sent_and_written(s)))
}

/// The details' total: the row's number, said as what it is, then the cache reads beside it: "84K sent and written,
/// plus 1.9M read again from Claude's cache".
pub fn details_total(s: &Spent) -> String {
    let main = format!("{} sent and written", short(sent_and_written(s)));
    if s.cache_read == 0 {
        return main;
    }
    format!("{main}, plus {} read again from Claude's cache", short(s.cache_read))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spent(input: u64, output: u64, cache_read: u64) -> Spent {
        Spent { input, output, cache_read, subagents: false }
    }

    #[test]
    fn counts_are_short_and_say_what_they_include() {
        for (n, want) in [
            (0, "0"),
            (950, "950"),
            (1000, "1K"),
            (1234, "1.2K"),
            (12_345, "12K"),
            (999_499, "999K"),
            (999_500, "1M"),
            (1_250_000, "1.3M"),
            (12_600_000, "13M"),
        ] {
            assert_eq!(short(n), want, "{n}");
        }
        let s = spent(40_000, 44_000, 1_900_000);
        assert_eq!(row_total(&s), "84K tokens");
        assert_eq!(details_total(&s), "84K sent and written, plus 1.9M read again from Claude's cache");
        assert_eq!(details_total(&spent(900, 100, 0)), "1K sent and written");
    }
}
