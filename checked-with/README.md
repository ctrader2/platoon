# Checked with

The version of each coding agent platoon was last checked with, one file per tool, holding just the version.

- `claude-code`: Claude Code. README.md and the guide say the same version ("It was last checked with Claude Code
  X"); `scripts/check-claude-version.sh` fails CI when they don't. The daily watch
  (`.github/workflows/claude-code-watch.yml`) opens an issue when npm has a newer Claude Code.

Other tools (Codex, Gemini) get a file of their own here when platoon supports them.

To move to a new version: check it (the watch's issue has the list), then change the file and the docs' line in one
commit.
