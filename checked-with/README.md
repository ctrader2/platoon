# Checked with

The version of each coding agent platoon was last checked with, one file per program, holding just the version.

- `claude-code`: Claude Code.
- `codex`: OpenAI's Codex CLI.

README.md and the guide say the same versions ("It was last checked with Claude Code X and Codex Y");
`scripts/check-versions.sh` fails CI when they don't. The daily watch (`.github/workflows/version-watch.yml`, logic in
`scripts/version-watch.sh`) opens an issue when npm has a newer Claude Code or Codex. Both files are compiled into
platoon (`src/versions.rs`): when a user's Claude Code or Codex is newer, the app says so once and `platoon setup` each
time.

To move to a new version: check it (the watch's issue has the list), then change the file and the docs' line in one
commit.
