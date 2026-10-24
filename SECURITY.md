# Security policy

## Supported versions

Only the **latest release** of platoon gets security fixes. If you run an older version, or a build from `main`, please
check that the problem is still there in the latest release before reporting it.

## Reporting a vulnerability

**Please don't open a public issue, pull request or Discussion for a security problem.**

Report it privately on GitHub: open the repository's [Security tab](https://github.com/ctrader2/platoon/security) and
press **Report a vulnerability**. Only the maintainer (@ctrader2) sees it. GitHub's guide:
[Privately reporting a security vulnerability](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability).

Say what you found, how to make it happen, which version of platoon and of Claude Code or Codex
(whichever is involved), and your operating system.

platoon has one maintainer, working on it in their own time. You can expect:

- a first reply **within a week**;
- a fix in a new release as soon as one can be made, and a GitHub security advisory crediting you (unless you'd rather
  not be named);
- that you are kept up to date in the report until it is closed.

Please give us a reasonable time to release a fix before you talk about the problem publicly.

## What counts as a security issue

platoon runs with your permissions against your real Claude Code and Codex setups. It edits Claude Code's
`settings.json` (a status line and hooks) and Codex's `hooks.json`, has Codex trust its hooks and tasks' worktrees,
starts Claude Code and Codex sessions and keeps them running in the background, runs tasks in Claude Code's "accept
edits" mode or Codex's `workspace-write` sandbox, commits a Codex task's work for it, and reads your conversation
transcripts. [What platoon changes](guide/what-platoon-changes.md)
describes all of it. Problems in those areas are what we most want to hear about, for example:

- `platoon setup` or a hook writing something to Claude Code's settings or Codex's hooks and trust records that it says
  it won't, or that runs something other than platoon;
- another user on the same computer being able to reach your sessions: the session hosts' sockets and files in
  `~/.cache/platoon/hosts`, or anything else platoon writes;
- text from a transcript, a ticket, a project's notes, a branch name or a file name making platoon run a command, or
  escaping out of the terminal (escape sequences that do more than draw);
- a task's session getting past the publish gate (`git push`, `gh pr create` and the other commands the
  [guide](guide/what-platoon-changes.md#tasks) lists, on Claude Code and on Codex) through a way of writing a command that
  the gate means to catch;
- platoon starting a task's session with more permissions or tools than the guide says it gives one;
- platoon making network requests other than the ones the guide describes, or sending conversation text anywhere the
  guide doesn't say (for example more, in the token features' calls, than [Features that use tokens](guide/tokens.md)
  describes);
- platoon removing or overwriting files it shouldn't: worktrees with uncommitted changes, your branches, your own
  settings or hooks.

## What doesn't

- **Claude Code or Codex itself**, and what the model decides to do inside a session. Report those to Anthropic or
  OpenAI. A task's session edits files without asking by design, and platoon is not a sandbox around it.
- **Commands the publish gate doesn't know.** The gate is a list of known commands, not a sandbox, and the guide says so.
  A tool it doesn't list (or an MCP server that publishes) is a feature request, not a vulnerability.
- **Someone who can already run commands as you**, or edit your `~/.claude`, `~/.codex`, `~/.config/platoon`
  or `~/.cache/platoon`: they can do anything platoon can.
- **Bugs in the dependencies** with no way to reach them through platoon. Dependabot and `cargo deny` watch for
  published advisories.
- **Crashes or garbled screens** with no security effect: open an ordinary
  [bug report](https://github.com/ctrader2/platoon/issues/new/choose).
