# Contributing to platoon

Thank you for wanting to help. platoon is a small project with one maintainer, so this page keeps things short. The
deeper guide, for people and for AI coding agents alike, is [CLAUDE.md](CLAUDE.md): how the code is laid out, the
safety rules and the test knobs.

Found a security problem? Don't open an issue: see [SECURITY.md](SECURITY.md).

## Before you start

- **Questions** go to [Discussions](https://github.com/ctrader2/platoon/discussions). **Issues** are for bugs and
  feature requests.
- **Small fixes** (a typo, a clear bug with an obvious fix): open a pull request straight away.
- **Anything bigger**: open an issue or a Discussion first and say what you have in mind. The project works by "discuss
  before building": agreeing on the plan first saves you from writing something that can't be merged.

## Set up and build

You need Linux or macOS, Rust 1.88 or newer, and git with a `user.name` and `user.email` set (the tests make
repositories and commits in temp folders). You don't need Claude Code to build or test platoon.

```
git clone https://github.com/ctrader2/platoon
cd platoon
cargo test                                   # about 40 s
cargo build --release                        # must finish without warnings
cargo fmt --check                            # run `cargo fmt` to fix
cargo clippy --all-targets -- -D warnings    # no clippy warnings, tests included
```

Run your build as `target/release/platoon`.

### Your own Claude Code setup is not a test bed

platoon runs against your real Claude Code setup: it reads `~/.claude`, writes hooks into Claude Code's
`settings.json`, and keeps its own files in `~/.config/platoon` and `~/.cache/platoon`. So:

- **The tests are hermetic.** Every test uses temp folders (`CLAUDE_CONFIG_DIR`, `PLATOON_CONFIG_DIR`,
  `PLATOON_CACHE_DIR`) and a stand-in for `claude` (`tests/fake-claude.sh`). A test must never read or write your real
  files, start a real `claude` or editor, or touch your clipboard. New tests follow the same rule: CLAUDE.md lists the
  environment variables that make this possible.
- **Don't run `platoon setup` on your real config while developing.** It points Claude Code's hooks at the binary that
  ran it, so a setup run from `target/` would leave your Claude Code depending on a build folder. To try setup, point
  the three variables above at temp folders first.
- **Don't replace an installed `platoon`** with `cargo install` from a work-in-progress checkout: Claude Code's settings
  may point at it.
- **Never kill processes you didn't start.** Sessions platoon runs keep running in the background
  (`platoon host ...`).

## A good pull request

- **One change per pull request.** A bug fix and a refactor are two pull requests.
- **Tests** for what you changed. Run `cargo test` twice: a test that fails once and passes the next time is a race to
  fix, not to retry.
- **All four checks above pass**: CI runs the same ones, with warnings as errors.
- **Users' docs follow the code.** A change to what the user sees or does updates the matching page in
  [`guide/`](guide/README.md) in the same pull request (and the README if it mentions it).
- **The file table in CLAUDE.md** stays current when a source file is added or its job changes.
- **A changelog line**: if the repository has a `CHANGELOG.md`, add a line for a user-visible change under
  "Unreleased".

## Commit messages

Say **why**, not only what: the diff already shows what changed. A short first line, then as much explanation as the
change needs.

## AI-assisted contributions

Welcome. Many of platoon's own commits were written with Claude Code, and the repository ships a
[CLAUDE.md](CLAUDE.md) that coding agents read. Two conditions:

- **You are responsible for every line**, as if you had typed it: you have read it, you understand it, and you have run
  the tests.
- **Say in the pull request that AI helped, and how much**, for example "written with Claude Code, reviewed and tested
  by me" or "Copilot completions only".

## Licence

platoon is under the [MIT licence](LICENSE). By contributing you agree that your contribution is under the same licence
(inbound = outbound); there is no CLA and no sign-off to add.

## Where to ask

Anything not covered here: start a [Discussion](https://github.com/ctrader2/platoon/discussions).
