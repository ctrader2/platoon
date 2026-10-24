//! Which shell commands of a task's session wait for the user: pushing, pull requests, releases and publishing. There is
//! one table of them, Claude Code's `ask` rules (`agents::claude::ASK`): Claude Code applies it itself to a Claude
//! task's commands (its permission prompt). Codex has no such rules, so platoon's hook applies the same table here, to
//! a Codex task's command line before it runs (`waits`).
//!
//! Matching here reads the command line as a shell would, so more spellings are caught than by Claude Code's own
//! matching: every command of a compound line (`&&`, `||`, `;`, `|`, a new line, `( )`), the words inside quotes as
//! words (`git 'push'`), what runs inside `$( )`, backticks, `sh -c '...'`/`bash -lc "..."` and `eval`, past leading
//! `VAR=value`s and wrappers (`env`, `command`, `exec`, `nohup`, `time`, `sudo`, `nice`, `timeout`), a program named
//! by its path (`/usr/bin/git`), and git's own options before its command (`git -C dir push`, `git --no-pager -c k=v
//! push`). Each command so read is matched against the table the way Claude Code matches a rule: `Bash(x:*)` is `x`
//! alone or `x` followed by more words, and a `*` is any text.
//!
//! Decided on purpose: a word that only mentions a command doesn't count (`echo "git push"`, `git commit -m "push
//! it"`, `grep -r "npm publish" .`), and `git push --help` does (it is `git push`; asking costs one yes), as does all
//! of `gh release` (the table's own rule, reading included). Not caught:
//! a script or program that pushes for you (`./release.sh`, `python -c ...`, `make deploy`), a git alias, `xargs` or
//! `find -exec` running one, and an MCP tool that publishes.

use crate::agents::claude::ASK;

/// How deep `sh -c`, `eval` and `$( )` inside each other are followed. A line nested deeper waits (it can't be read).
const MAX_DEPTH: usize = 6;

/// What a script nested deeper than `MAX_DEPTH` reads as: a command that waits.
const TOO_DEEP: &str = "(commands nested too deep to read)";

/// The first command in the shell line `line` that waits for the user (as it was read, its words joined by spaces);
/// None when none does.
pub fn waits(line: &str) -> Option<String> {
    commands(line, 0).into_iter().find(|words| matches_table(words)).map(|words| words.join(" "))
}

/// Does the shell line `line` run one of `programs` (by name, read as `waits` reads a line)?
pub fn runs_any(line: &str, programs: &[&str]) -> bool {
    commands(line, 0).iter().any(|words| words.first().is_some_and(|p| programs.contains(&p.as_str()) || p == TOO_DEEP))
}

/// Does this one command (its words, normalized) match a rule of the table?
fn matches_table(words: &[String]) -> bool {
    if words.is_empty() {
        return false;
    }
    if words[0] == TOO_DEEP {
        return true;
    }
    let line = words.join(" ");
    ASK.iter().filter_map(|r| r.strip_prefix("Bash(")?.strip_suffix(')')).any(|rule| rule_matches(rule, &line))
}

/// One rule against one command line, as Claude Code reads a rule: `x:*` is `x` alone or `x` and more words; else a
/// `*` is any text, and a rule ending in ` *` also takes the command without anything there.
fn rule_matches(rule: &str, line: &str) -> bool {
    if let Some(prefix) = rule.strip_suffix(":*") {
        return line == prefix || line.strip_prefix(prefix).is_some_and(|rest| rest.starts_with(' '));
    }
    let parts: Vec<&str> = rule.split('*').collect();
    if rule.ends_with(" *") && parts.len() == 2 && line == parts[0].trim_end() {
        return true;
    }
    glob(&parts, line)
}

/// Does `text` match the rule cut at its `*`s (`parts`): the first part at the start, the last at the end, the others
/// in order between?
fn glob(parts: &[&str], text: &str) -> bool {
    let Some((first, rest)) = parts.split_first() else { return text.is_empty() };
    let Some(mut left) = text.strip_prefix(first) else { return false };
    let Some((last, middle)) = rest.split_last() else { return left.is_empty() };
    for p in middle {
        match left.find(p) {
            Some(i) => left = &left[i + p.len()..],
            None => return false,
        }
    }
    left.ends_with(last)
}

// ---- reading a command line ------------------------------------------------------------------------------------------

/// Every simple command in `script`, each as its words (normalized: `normalize`), those run by the commands inside it
/// too (`$( )`, backticks, `sh -c`, `eval`).
fn commands(script: &str, depth: usize) -> Vec<Vec<String>> {
    if depth > MAX_DEPTH {
        return vec![vec![TOO_DEEP.to_string()]];
    }
    let mut lexed = Lexer::new(script, depth);
    lexed.run();
    let mut out = lexed.nested;
    for words in lexed.commands {
        out.extend(expand(words, depth));
    }
    out
}

/// One simple command's words, normalized, or what it runs when it is a shell given a script (`sh -c`) or `eval`.
fn expand(words: Vec<String>, depth: usize) -> Vec<Vec<String>> {
    let words = normalize(words);
    let Some(program) = words.first() else { return Vec::new() };
    if matches!(program.as_str(), "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh" | "ash" | "fish") {
        // `bash -lc 'script'`, `sh -e -c 'script'`: the first word that isn't an option after a `c` option.
        let mut it = words.iter().skip(1);
        let mut takes_script = false;
        for w in it.by_ref() {
            if w == "--" {
                break;
            }
            if let Some(flags) = w.strip_prefix('-').filter(|f| !f.starts_with('-')) {
                takes_script |= flags.contains('c');
                continue;
            }
            if w.starts_with("--") {
                continue;
            }
            if takes_script {
                return commands(w, depth + 1);
            }
            break;
        }
        if takes_script && let Some(w) = it.next() {
            return commands(w, depth + 1);
        }
        return vec![words];
    }
    if program == "eval" {
        return commands(&words[1..].join(" "), depth + 1);
    }
    vec![words]
}

/// Words that may come before a command and aren't it: the shell's own keywords.
const KEYWORDS: [&str; 12] = ["!", "{", "}", "then", "do", "else", "elif", "if", "while", "until", "in", "time"];

/// One command's words as the program sees them: no leading `VAR=value`s, keywords or wrappers (`env`, `command`,
/// `exec`, `nohup`, `sudo`, `nice`, `timeout`, ... with their own options), the program by its name (`/usr/bin/git` is
/// `git`), and for git, its options before its command left out (`git -C dir push` is `git push`).
fn normalize(mut words: Vec<String>) -> Vec<String> {
    loop {
        let Some(first) = words.first() else { return words };
        let assignment = first.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        });
        if assignment || KEYWORDS.contains(&first.as_str()) {
            words.remove(0);
            continue;
        }
        let name = program_name(first);
        let skip = match name.as_str() {
            "command" | "builtin" | "exec" | "nohup" | "stdbuf" => options_then(&words[1..], &[]),
            "env" => {
                // Its options, then any VAR=value, then the command.
                let mut n = options_then(&words[1..], &["-u", "--unset", "-C", "--chdir", "-S", "--split-string"]);
                while words.get(1 + n).is_some_and(|w| w.contains('=') && !w.starts_with('-')) {
                    n += 1;
                }
                n
            }
            "sudo" | "doas" => {
                options_then(&words[1..], &["-u", "--user", "-g", "--group", "-C", "-D", "--chdir", "-h", "--host", "-p", "--prompt"])
            }
            "nice" => options_then(&words[1..], &["-n", "--adjustment"]),
            "timeout" => {
                let n = options_then(&words[1..], &["-k", "--kill-after", "-s", "--signal"]);
                n + 1 // the duration
            }
            _ => {
                words[0] = name;
                break;
            }
        };
        words.drain(..(1 + skip).min(words.len()));
    }
    if words[0] == "git" {
        let mut i = 1;
        while let Some(w) = words.get(i) {
            if !w.starts_with('-') {
                break;
            }
            let takes_value = matches!(
                w.as_str(),
                "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env" | "--exec-path" | "--super-prefix"
            );
            i += if takes_value { 2 } else { 1 };
        }
        words.drain(1..i.min(words.len()));
    }
    words
}

/// How many of `words` are a wrapper's own options (those in `with_value` take the next word too), up to the command.
fn options_then(words: &[String], with_value: &[&str]) -> usize {
    let mut i = 0;
    while let Some(w) = words.get(i) {
        if w == "--" {
            return i + 1;
        }
        if !w.starts_with('-') || w == "-" {
            break;
        }
        i += if with_value.contains(&w.as_str()) { 2 } else { 1 };
    }
    i.min(words.len())
}

/// A program's name without its folder: `/usr/bin/git` is `git`.
fn program_name(word: &str) -> String {
    word.rsplit('/').next().unwrap_or(word).to_string()
}

/// A small shell reader: words (quotes taken off), the simple commands they make (cut at `;`, `&`, `|`, a new line,
/// `(` and `)`), redirections left out, comments skipped, and the scripts inside `$( )` and backticks read too
/// (`nested`). Not a whole shell: enough to find the commands a line runs.
struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    depth: usize,
    word: String,
    /// The word has begun (so `''` is an empty word, not none).
    in_word: bool,
    current: Vec<String>,
    commands: Vec<Vec<String>>,
    nested: Vec<Vec<String>>,
    /// The next word is a redirection's target, not a word of the command.
    skip_next: bool,
}

impl<'a> Lexer<'a> {
    fn new(script: &'a str, depth: usize) -> Self {
        Lexer {
            chars: script.chars().peekable(),
            depth,
            word: String::new(),
            in_word: false,
            current: Vec::new(),
            commands: Vec::new(),
            nested: Vec::new(),
            skip_next: false,
        }
    }

    fn end_word(&mut self) {
        if self.in_word {
            let w = std::mem::take(&mut self.word);
            if self.skip_next {
                self.skip_next = false;
            } else {
                self.current.push(w);
            }
            self.in_word = false;
        }
    }

    fn end_command(&mut self) {
        self.end_word();
        self.skip_next = false;
        if !self.current.is_empty() {
            self.commands.push(std::mem::take(&mut self.current));
        }
    }

    /// The text up to the `)` that closes a `$(` just read (nested ones counted, quotes respected).
    fn substitution(&mut self) -> String {
        let (mut text, mut level, mut quote) = (String::new(), 1usize, None::<char>);
        while let Some(c) = self.chars.next() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some('"'), '\\') => {
                    text.push(c);
                    if let Some(n) = self.chars.next() {
                        text.push(n);
                    }
                    continue;
                }
                (Some(_), _) => {}
                (None, '\'' | '"') => quote = Some(c),
                (None, '\\') => {
                    text.push(c);
                    if let Some(n) = self.chars.next() {
                        text.push(n);
                    }
                    continue;
                }
                (None, '(') => level += 1,
                (None, ')') => {
                    level -= 1;
                    if level == 0 {
                        return text;
                    }
                }
                _ => {}
            }
            text.push(c);
        }
        text
    }

    /// The text up to the closing backtick.
    fn backticks(&mut self) -> String {
        let mut text = String::new();
        while let Some(c) = self.chars.next() {
            match c {
                '`' => break,
                '\\' => {
                    if let Some(n) = self.chars.next() {
                        text.push(n);
                    }
                }
                c => text.push(c),
            }
        }
        text
    }

    fn read_nested(&mut self, script: &str) {
        let found = commands(script, self.depth + 1);
        self.nested.extend(found);
    }

    fn run(&mut self) {
        while let Some(c) = self.chars.next() {
            match c {
                ' ' | '\t' => self.end_word(),
                '\n' | ';' | '&' | '|' | '(' | ')' => self.end_command(),
                '#' if !self.in_word => {
                    for n in self.chars.by_ref() {
                        if n == '\n' {
                            break;
                        }
                    }
                    self.end_command();
                }
                '<' | '>' => {
                    // `2>&1`, `>out`, `<<EOF`, `&>x`: the number before it is no word; the target after it isn't one.
                    if self.in_word && self.word.chars().all(|c| c.is_ascii_digit()) {
                        self.word.clear();
                        self.in_word = false;
                    }
                    self.end_word();
                    while self.chars.peek().is_some_and(|n| matches!(n, '<' | '>' | '|')) {
                        self.chars.next();
                    }
                    if self.chars.peek() == Some(&'&') {
                        self.chars.next();
                        while self.chars.peek().is_some_and(|n| n.is_ascii_digit() || *n == '-') {
                            self.chars.next();
                        }
                    } else {
                        self.skip_next = true;
                    }
                }
                '\'' => {
                    self.in_word = true;
                    for n in self.chars.by_ref() {
                        if n == '\'' {
                            break;
                        }
                        self.word.push(n);
                    }
                }
                '"' => {
                    self.in_word = true;
                    while let Some(n) = self.chars.next() {
                        match n {
                            '"' => break,
                            '\\' => match self.chars.next() {
                                Some(e @ ('"' | '\\' | '$' | '`')) => self.word.push(e),
                                Some('\n') | None => {}
                                Some(e) => {
                                    self.word.push('\\');
                                    self.word.push(e);
                                }
                            },
                            '$' if self.chars.peek() == Some(&'(') => {
                                self.chars.next();
                                let inner = self.substitution();
                                self.read_nested(&inner);
                            }
                            '`' => {
                                let inner = self.backticks();
                                self.read_nested(&inner);
                            }
                            n => self.word.push(n),
                        }
                    }
                }
                '\\' => match self.chars.next() {
                    Some('\n') | None => {}
                    Some(n) => {
                        self.in_word = true;
                        self.word.push(n);
                    }
                },
                '$' if self.chars.peek() == Some(&'(') => {
                    self.chars.next();
                    let inner = self.substitution();
                    self.read_nested(&inner);
                    self.in_word = true;
                }
                '`' => {
                    let inner = self.backticks();
                    self.read_nested(&inner);
                    self.in_word = true;
                }
                c => {
                    self.in_word = true;
                    self.word.push(c);
                }
            }
        }
        self.end_command();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushing_publishing_and_pull_requests_wait_in_every_spelling_a_shell_runs() {
        for line in [
            "git push",
            "git push origin main",
            "git push -u origin task/5",
            "git push --force-with-lease",
            "git push --help",
            "git -C /w/shop push",
            "git -C '/w/my shop' push origin",
            "git -c push.default=current push",
            "git --git-dir=/w/.git push origin",
            "git --git-dir /w/.git --work-tree /w push",
            "git --no-pager push",
            "git 'push'",
            "\"git\" push",
            "/usr/bin/git push",
            "GIT_SSH_COMMAND='ssh -i k' git push",
            "env GIT_TRACE=1 git push",
            "env -u HOME git push",
            "command git push",
            "exec git push",
            "nohup git push &",
            "time git push",
            "sudo -u me git push",
            "timeout 30 git push",
            "nice -n 5 git push",
            "cd /w/shop && git push",
            "git add . && git commit -m done && git push",
            "make test; git push",
            "false || git push",
            "git status | cat; git push 2>&1 | tail -n 3",
            "(cd /w && git push)",
            "{ git push; }",
            "if true; then git push; fi",
            "bash -lc 'git push origin main'",
            "bash -lc \"cd /w && git push\"",
            "sh -c 'git push'",
            "sh -e -c 'git push'",
            "/bin/bash --login -c 'git push'",
            "zsh -c \"sh -c 'git push'\"",
            "eval git push",
            "eval 'git push'",
            "echo $(git push)",
            "echo \"$(git push)\"",
            "echo `git push`",
            "x=$(git -C /w push 2>&1)",
            "git push \\\n  origin main",
            "gh pr create --fill",
            "gh pr create --title 'Login' --body x",
            "gh pr merge 12 --squash",
            "gh release create v1.0",
            "gh release list",
            "gh api -X POST repos/o/r/issues",
            "gh api repos/o/r/pulls -f title=x -f head=task/5",
            "gh api repos/o/r/pulls --method POST",
            "gh api graphql -F query=@q.graphql",
            "gh api repos/o/r/releases --input body.json",
            "glab mr create",
            "glab",
            "cargo publish",
            "cargo publish --dry-run",
            "npm publish",
            "npm publish --access public",
        ] {
            assert!(waits(line).is_some(), "should wait: {line}");
        }
    }

    #[test]
    fn commands_that_only_mention_publishing_or_only_read_go_through() {
        for line in [
            "git status",
            "git log --oneline -5",
            "git -C /w/shop commit -m done",
            "git -C /w commit -m 'push the fix'",
            "git commit -m \"git push later\"",
            "git commit -m push",
            "git stash push",
            "git stash push -m wip",
            "git pushx",
            "git fetch origin",
            "git ls-remote origin",
            "echo git push",
            "echo \"git push\"",
            "echo 'cargo publish'",
            "printf '%s\\n' 'npm publish'",
            "grep -r \"git push\" .",
            "rg 'gh pr create' docs",
            "# git push\ngit status",
            "cat <<'EOF'\nnothing here\nEOF",
            "gh pr view 12",
            "gh pr list",
            "gh pr checkout 12",
            "gh api repos/o/r/pulls",
            "gh api repos/o/r --jq .name",
            "cargo build --release",
            "cargo test",
            "npm test",
            "npm run publish-docs",
            "cargo install --path .",
            "",
            "   ",
            "ls -la > out.txt",
        ] {
            assert_eq!(waits(line), None, "should go through: {line}");
        }
    }

    #[test]
    fn what_matched_is_said_as_it_was_read() {
        assert_eq!(waits("cd /w && git -C /w push -u origin task/5").as_deref(), Some("git push -u origin task/5"));
        assert_eq!(waits("bash -lc 'gh pr create --fill'").as_deref(), Some("gh pr create --fill"));
    }

    #[test]
    fn a_program_run_anywhere_in_a_line_is_found_by_its_name() {
        let patch = ["apply_patch", "applypatch"];
        for line in ["apply_patch <<'EOF'\n*** Begin Patch\nEOF", "cd /w && apply_patch < p.diff", "bash -lc 'applypatch x'"] {
            assert!(runs_any(line, &patch), "{line}");
        }
        for line in ["cat apply_patch.md", "echo apply_patch", "git apply p.diff"] {
            assert!(!runs_any(line, &patch), "{line}");
        }
    }

    #[test]
    fn the_table_is_claude_codes_own_ask_list() {
        // Every rule of the table is matched by the command it names (the rule with its `*`s taken as one word).
        for rule in ASK {
            let inner = rule.strip_prefix("Bash(").and_then(|r| r.strip_suffix(')')).unwrap();
            let example = inner.trim_end_matches(":*").replace('*', "x");
            assert!(matches_table(&example.split(' ').map(String::from).collect::<Vec<_>>()), "{rule}: {example}");
        }
    }

    #[test]
    fn a_line_nested_too_deep_or_unclosed_is_read_without_trouble() {
        let mut deep = "git push".to_string();
        for _ in 0..(MAX_DEPTH + 2) {
            deep = format!("sh -c {}", crate::agents::claude::shell_word(&deep));
        }
        assert!(waits(&deep).is_some(), "too deep to read: it waits");
        assert_eq!(waits(&"(".repeat(50)), None);
        assert!(waits("echo \"unclosed $(git push").is_some());
        assert_eq!(waits("echo 'unclosed"), None);
        assert!(waits("git push 'unclosed").is_some());
    }
}
