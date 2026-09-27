---
name: changelog-entry
description: Write a changelog entry for the changes on the current branch or since the last release, in the style of the project's CHANGELOG.md. Use when the user asks for a changelog entry, release notes, or what changed written up for the people who use the project.
---

# Writing a changelog entry

Work out what changed, then write it for the people who use the project, not for the people who wrote it.

## Find what changed

1. Find the last release: the newest version heading in `CHANGELOG.md`, else the newest git tag
   (`git describe --tags --abbrev=0`). With neither, compare the current branch with the main branch.
2. Read the commits since then (`git log --oneline <last release>..HEAD`), and the diff
   (`git diff <last release>..HEAD`) where a commit message doesn't say enough.
3. Keep what a user would notice: new features, changed behaviour, fixes, removals, and anything they must do when they
   upgrade. Leave out refactoring, tests and build changes that change nothing they see.

## Write it

- Follow the file's own style. When there is no file yet, use the Keep a Changelog layout: a `## [version] - YYYY-MM-DD`
  heading (or `## [Unreleased]`), then `### Added`, `### Changed`, `### Fixed` and `### Removed`, leaving out empty ones.
- One line per change, starting with what the user sees: "Search finds words in file names", not "Refactor the search
  index".
- Say plainly what someone upgrading must do (a setting renamed, a command removed) under the change that needs it.
- Put the new entry at the top, under the file's title. Change nothing else in the file.

## Before you finish

Show the user the entry and ask before writing it to `CHANGELOG.md`, unless they already asked you to write it. Never
commit or push it yourself.
