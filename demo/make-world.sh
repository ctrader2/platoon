#!/usr/bin/env bash
# Builds the made-up world the README animation is recorded in, inside the folder given as the only argument
# (it must not exist yet, or be empty). Nothing outside that folder is read or written: it has its own home, its own
# Claude Code folder, its own Codex folder, platoon's config, cache and data folders, and stand-ins for `claude`,
# `codex` and `code`.
#
#   demo/make-world.sh /tmp/platoon-demo
#   . /tmp/platoon-demo/env.sh          # points HOME, CLAUDE_CONFIG_DIR, CODEX_HOME, PLATOON_* and XDG_* into the world
#   /tmp/platoon-demo/live.sh start     # stand-in "running sessions" (shell loops and held locks), see below
#   /tmp/platoon-demo/live.sh stop      # stops exactly what live.sh and platoon started inside this world
#
# Every conversation, project, task and name here is invented. The files are written in the shapes platoon reads
# (see src/agents/claude.rs, src/agents/codex.rs and src/testdata/codex/ for Codex 0.163's, src/projects.rs,
# src/tickets.rs, src/squads.rs, src/summaries.rs, src/archive.rs, src/usage.rs). Times are relative to now, so the
# list always reads "2h ago", "3d ago". Needs bash, GNU date and flock.
set -euo pipefail

W=${1:?usage: make-world.sh <empty folder>}
mkdir -p "$W"
W=$(cd "$W" && pwd)
if [ -n "$(ls -A "$W")" ]; then
    echo "make-world.sh: $W is not empty" >&2
    exit 1
fi

H=$W/home                      # the made-up user's home: paths read ~/code/shop
CL=$H/.claude                  # CLAUDE_CONFIG_DIR
CFG=$H/.config/platoon         # PLATOON_CONFIG_DIR
CACHE=$H/.cache/platoon        # PLATOON_CACHE_DIR
CX=$H/.codex                   # CODEX_HOME
mkdir -p "$CL/projects" "$CL/sessions" "$CFG" "$CACHE/status" "$H/.local/share/platoon" "$H/.local/state" \
    "$CX/sessions" "$CX/thread-writer-locks" "$W/bin" "$W/editors" "$W/replay" "$W/run"

NOW=$(date +%s)
NOW_MS=$((NOW * 1000))
iso() { date -u -d "@$(($1 / 1000))" +%Y-%m-%dT%H:%M:%S.000Z; }
esc() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'; }

# ---- the code folders --------------------------------------------------------------------------------------------
for d in shop shop-api tidy docs-site dotfiles shop-worktrees/task-2 shop-worktrees/task-3; do mkdir -p "$H/code/$d"; done
printf '# shop\n\nThe storefront: Next.js, Stripe, Postgres.\n' > "$H/code/shop/README.md"
printf '# shop-api\n\nThe API behind the shop.\n' > "$H/code/shop-api/README.md"
printf '# tidy\n\nA small CLI that sorts a messy folder into dated folders.\n' > "$H/code/tidy/README.md"
printf '# docs\n\nThe documentation site.\n' > "$H/code/docs-site/README.md"

# ---- conversations -----------------------------------------------------------------------------------------------
# conv ID DIR BRANCH MODEL MINUTES_AGO CONTEXT_TOKENS PROMPT REPLY [PROMPT REPLY ...]
# writes ~/.claude/projects/<dir with / as ->/<ID>.jsonl the way Claude Code does, and a plain replay the stand-in
# claude shows when the session is opened.
conv() {
    local id=$1 dir=$H/code/$2 branch=$3 model=$4 ago=$5 tokens=$6
    shift 6
    local folder
    folder=$CL/projects/$(printf '%s' "$dir" | tr '/.' '--')
    mkdir -p "$folder"
    local file=$folder/$id.jsonl end=$((NOW_MS - ago * 60000))
    local n=$(($# / 2)) i=0 at
    printf '{"type":"mode","mode":"normal","sessionId":"%s"}\n' "$id" > "$file"
    : > "$W/replay/$id.txt"
    while [ $# -ge 2 ]; do
        at=$((end - (n - i) * 240000))
        printf '{"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":"%s"},"uuid":"u%s-%d","timestamp":"%s","cwd":"%s","sessionId":"%s","gitBranch":"%s"}\n' \
            "$(esc "$1")" "${id:0:8}" "$i" "$(iso $((at + 1000)))" "$dir" "$id" "$branch" >> "$file"
        printf '{"parentUuid":"u%s-%d","isSidechain":false,"type":"assistant","message":{"id":"msg_%s%d","role":"assistant","model":"%s","content":[{"type":"text","text":"%s"}],"usage":{"input_tokens":%d,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":900}},"requestId":"req_%s%d","uuid":"a%s-%d","timestamp":"%s","cwd":"%s","sessionId":"%s","gitBranch":"%s"}\n' \
            "${id:0:8}" "$i" "${id:0:8}" "$i" "$model" "$(esc "$2")" $((tokens * (i + 1) / n)) "${id:0:8}" "$i" "${id:0:8}" "$i" \
            "$(iso $((at + 200000)))" "$dir" "$id" "$branch" >> "$file"
        printf '\n  > %s\n\n  %s\n' "$1" "$2" | fold -s -w 100 | sed 's/^\([^ ]\)/    \1/' >> "$W/replay/$id.txt"
        i=$((i + 1))
        shift 2
    done
}

OPUS=claude-opus-5
SONNET=claude-sonnet-5
HAIKU=claude-haiku-4-5-20251001

S1=5f0c2a1e-8b3d-4e7a-9c21-3d6f0b8e4a17   # web shop: Stripe checkout (running, working)
S2=a41e7c90-2d5b-4f18-b6e3-7c0d9a2f5e61   # web shop: basket rounding
S3=c7d2f8a4-61e9-4b0c-8f3a-2e5b7d1c9a03   # shop-api: search speed
S4=0b9e4d17-3c6a-4e2f-a8d5-91f7c2b6e840   # shop-api: order emails
S5=e3a8b6c1-7f42-4d9e-b051-6c2d8f4a3e97   # tidy: config file (running, finishes during the tour)
S6=9d4f1b2e-a6c3-4870-9e5d-f2b8c7a1d064   # tidy: release workflow
S7=6e1c9a3d-4b8f-4f27-8d60-a5e3b9c2f718   # tidy: dry run
S8=f2b7e4a9-0c5d-4a1b-9e83-d6c1f7a2b450   # docs: dark mode
S9=48c3e6f1-9a2b-4d7e-b1f4-0e8a5c3d6b29   # docs: link check
S10=b8a5d2c7-e14f-4c93-a6b0-3f9e7d1c2a85  # dotfiles: keymaps (archived)
S11=7a2e9f4b-1d8c-4b65-93e7-c0f5a8d4b316  # shop: README (archived)
S12=d16f3a8e-5c2b-4e09-87a4-b9e1d6f0c3a2  # shop-api: Postgres upgrade (archived)
S13=2c8e5b1f-6a9d-4f3c-b7e2-8d0a4c6f1e59  # docs: search box
S14=91f6d3a2-b4e7-4c18-a5d9-2e7b0f8c4a63  # dotfiles: tmux status line
S15=5b3a7e2d-c9f1-4a64-8b0e-f4d2a6c8e917  # shop: product images
S16=e7c4a1f9-2d6b-4e85-9a3c-b1f8d5e2c740  # tidy: Windows paths

conv $S1 shop feat/checkout $OPUS 2 118000 \
    "Add Stripe checkout to the basket page, with a success page and a cancel page that brings the basket back." \
    "Plan: create a Checkout Session on the server from the basket, redirect to Stripe, and handle the return on /checkout/success and /checkout/cancel. I will start with the API route." \
    "Use the test keys from .env.local and keep the basket until the webhook says paid." \
    "Done so far: the API route and both pages. Now wiring the checkout.session.completed webhook so the basket is only cleared once payment is confirmed."
conv $S2 shop fix/rounding $SONNET 190 64000 \
    "The basket total is off by a cent when there are three or more items with a discount. Find out why and fix it." \
    "Found it: the discount is rounded once per basket instead of once per line, so three lines at 0.333 lose a cent. Rounding per line now, with a test for the three-item case. All 48 tests pass."
conv $S3 shop-api main $SONNET 1450 151000 \
    "Search on /products takes two seconds with 5,000 products. Profile it and make it fast." \
    "The query does a full scan with ILIKE on name and description. Added a trigram index and a tsvector column: search now takes 18 ms at 5,000 products and 40 ms at 50,000."
conv $S4 shop-api feat/order-email $HAIKU 5800 22000 \
    "Send an order confirmation email with the items, the total and the delivery date." \
    "Added the email template and send it from the order-paid handler. The delivery date comes from the warehouse lead time plus two working days."
conv $S5 tidy main $SONNET 1 87000 \
    "tidy should read its options from tidy.toml in the repo root, and flags on the command line should win over it." \
    "Added a Config struct loaded from tidy.toml, merged under the parsed flags. Writing tests for a missing file, a bad value and a flag that overrides the file."
conv $S6 tidy ci/release $OPUS 2900 46000 \
    "Set up a GitHub Actions workflow that builds release binaries for Linux and macOS when I push a tag." \
    "Added release.yml: a matrix over x86_64 and aarch64 for Linux and macOS, stripped binaries, checksums, and a draft release with all of them attached."
conv $S7 tidy feat/dry-run $HAIKU 8300 15000 \
    "Add --dry-run that prints what tidy would move, without moving anything." \
    "Added --dry-run: every move is printed as from -> to and nothing is touched. Covered by two new tests."
conv $S8 docs-site main $SONNET 310 39000 \
    "The docs site needs a dark mode that follows the system setting, with a toggle in the header." \
    "Added a prefers-color-scheme stylesheet and a three-way toggle (system, light, dark) that remembers the choice. Code blocks use a matching dark palette."
conv $S9 docs-site ci/links $HAIKU 4300 18000 \
    "Write a script that checks every internal link in the docs and fails CI on a broken one." \
    "scripts/check-links.mjs crawls the built site and reports each broken link with the page it is on. Found and fixed 3 broken links; CI runs it on every pull request."
conv $S10 dotfiles main $SONNET 13000 31000 \
    "Clean up my Neovim keymaps: there are duplicates across three files." \
    "Merged them into lua/keymaps.lua and removed 11 duplicates. Two clashed (leader f and leader g); kept the newer ones."
conv $S11 shop docs/readme $HAIKU 16500 12000 \
    "Rewrite the README so a new developer can run the shop locally in five minutes." \
    "Rewrote it around one path: clone, copy .env.example, docker compose up, open localhost:3000. Moved the deployment notes to docs/deploy.md."
conv $S12 shop-api chore/pg16 $SONNET 15200 58000 \
    "Upgrade the local Postgres from 15 to 16 without losing the seed data." \
    "Dumped with pg_dumpall, bumped the compose image to postgres:16, restored, and checked the row counts match."

conv $S13 docs-site feat/search $SONNET 1580 52000 \
    "Add a search box to the docs that works offline, without a hosted search service." \
    "Built a search index at build time with a small client-side library; results show as you type, with the matching heading. The index is 140 KB gzipped."
conv $S14 dotfiles main $HAIKU 9100 9000 \
    "Make my tmux status line show the git branch and battery, and nothing else." \
    "Replaced the status line with two segments, the git branch of the active pane and the battery percentage, refreshed every 15 seconds."
conv $S15 shop perf/images $OPUS 7400 97000 \
    "Product pages are slow on mobile. Serve the product images as WebP in the right sizes and lazy-load the ones below the fold." \
    "Images now go through the image component with WebP and AVIF at four widths; below-the-fold ones load lazily. Largest contentful paint on a mid-range phone went from 4.1 s to 1.6 s."
conv $S16 tidy fix/windows-paths $SONNET 12400 34000 \
    "tidy fails on Windows paths with spaces. Make it handle them and add tests." \
    "Paths are now built with PathBuf everywhere instead of joined strings, and quoted when printed. Added tests for spaces and for UNC paths."
# Task #3's session (Claude Code, in the task's worktree), stopped at a question for the user.
S17=3e9a6c2b-d84f-4a17-b5c0-71f2e8d4a96c
conv $S17 shop-worktrees/task-3 task/3 $SONNET 25 41000 \
    "Work on task #3: Show the delivery date on the order page." \
    "The order page can show the date from the warehouse lead time, as the email does. Before I change it: should the date be in the buyer's time zone or the shop's?"

# ---- Codex conversations -----------------------------------------------------------------------------------------
# cxconv ID DIR BRANCH MODEL MINUTES_AGO CONTEXT_TOKENS STATE NAME PROMPT REPLY [PROMPT REPLY ...]   (STATE: finished or working)
# writes ~/.codex/sessions/YYYY/MM/DD/rollout-<time>-<ID>.jsonl in Codex 0.163's record shapes (src/testdata/codex/),
# the name Codex gives it in session_index.jsonl, and a plain replay. STATE `working` leaves the last turn unfinished.
cxconv() {
    local id=$1 dir=$H/code/$2 branch=$3 model=$4 ago=$5 tokens=$6 state=$7 name=$8
    shift 8
    local end=$((NOW_MS - ago * 60000)) n=$(($# / 2)) i=0 o=0 at
    local start=$((end - n * 240000))
    local day file
    day=$CX/sessions/$(date -u -d "@$((start / 1000))" +%Y/%m/%d)
    mkdir -p "$day"
    file=$day/rollout-$(date -u -d "@$((start / 1000))" +%Y-%m-%dT%H-%M-%S)-$id.jsonl
    rec() { printf '{"timestamp":"%s","ordinal":%d,"type":"%s","payload":%s}\n' "$(iso "$1")" "$o" "$2" "$3" >> "$file"; o=$((o + 1)); }
    : > "$file"
    : > "$W/replay/$id.txt"
    rec "$start" session_meta "{\"session_id\":\"$id\",\"id\":\"$id\",\"timestamp\":\"$(iso "$start")\",\"cwd\":\"$dir\",\"originator\":\"codex-tui\",\"cli_version\":\"0.163.0\",\"source\":\"cli\",\"thread_source\":\"user\",\"model_provider\":\"openai\",\"base_instructions\":{\"text\":\"(left out)\"},\"history_mode\":\"paginated\",\"git\":{\"branch\":\"$branch\"}}"
    while [ $# -ge 2 ]; do
        at=$((end - (n - i) * 240000))
        local used=$((tokens * (i + 1) / n)) last=$((i + 1 == n))
        rec $((at + 100)) event_msg "{\"type\":\"task_started\",\"turn_id\":\"turn-$i\",\"started_at\":$((at / 1000)),\"model_context_window\":258400,\"collaboration_mode_kind\":\"default\"}"
        rec $((at + 100)) turn_context "{\"turn_id\":\"turn-$i\",\"cwd\":\"$dir\",\"approval_policy\":\"on-request\",\"sandbox_policy\":{\"type\":\"workspace-write\",\"network_access\":false},\"model\":\"$model\",\"effort\":\"medium\",\"summary\":\"none\"}"
        rec $((at + 1000)) response_item "{\"type\":\"message\",\"id\":\"msg_u$i\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"$(esc "$1")\"}]}"
        rec $((at + 1000)) event_msg "{\"type\":\"item_completed\",\"thread_id\":\"$id\",\"turn_id\":\"turn-$i\",\"item\":{\"type\":\"UserMessage\",\"id\":\"u$i\",\"content\":[{\"type\":\"text\",\"text\":\"$(esc "$1")\"}]}}"
        printf '\n  > %s\n' "$1" | fold -s -w 100 | sed 's/^\([^ ]\)/    \1/' >> "$W/replay/$id.txt"
        if [ "$state" = working ] && [ "$last" = 1 ]; then
            # Still at it: the turn has started and nothing has finished it.
            rec $((at + 60000)) event_msg "{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":$((used * 3)),\"cached_input_tokens\":$((used * 2)),\"cache_write_input_tokens\":0,\"output_tokens\":$((used / 40)),\"reasoning_output_tokens\":0,\"total_tokens\":$((used * 3))},\"last_token_usage\":{\"input_tokens\":$used,\"cached_input_tokens\":0,\"cache_write_input_tokens\":0,\"output_tokens\":10,\"reasoning_output_tokens\":0,\"total_tokens\":$used},\"model_context_window\":258400},$LIMITS}"
            break
        fi
        rec $((at + 200000)) event_msg "{\"type\":\"item_completed\",\"thread_id\":\"$id\",\"turn_id\":\"turn-$i\",\"item\":{\"type\":\"AgentMessage\",\"id\":\"a$i\",\"content\":[{\"type\":\"Text\",\"text\":\"$(esc "$2")\"}],\"phase\":\"final_answer\"}}"
        rec $((at + 200000)) response_item "{\"type\":\"message\",\"status\":\"completed\",\"id\":\"msg_a$i\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"$(esc "$2")\",\"annotations\":[],\"logprobs\":[]}],\"phase\":\"final_answer\"}"
        rec $((at + 200100)) event_msg "{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":$((used * 3)),\"cached_input_tokens\":$((used * 2)),\"cache_write_input_tokens\":0,\"output_tokens\":$((used / 40)),\"reasoning_output_tokens\":0,\"total_tokens\":$((used * 3))},\"last_token_usage\":{\"input_tokens\":$used,\"cached_input_tokens\":0,\"cache_write_input_tokens\":0,\"output_tokens\":10,\"reasoning_output_tokens\":0,\"total_tokens\":$used},\"model_context_window\":258400},$LIMITS}"
        rec $((at + 200200)) event_msg "{\"type\":\"task_complete\",\"turn_id\":\"turn-$i\",\"last_agent_message\":\"$(esc "$2")\",\"started_at\":$((at / 1000)),\"completed_at\":$((at / 1000 + 200))}"
        printf '\n  %s\n' "$2" | fold -s -w 100 | sed 's/^\([^ ]\)/    \1/' >> "$W/replay/$id.txt"
        i=$((i + 1))
        shift 2
    done
    printf '{"id":"%s","thread_name":"%s","updated_at":"%s"}\n' "$id" "$(esc "$name")" "$(iso "$end")" >> "$CX/session_index.jsonl"
    touch -d "@$((end / 1000))" "$file"
}

# Your ChatGPT plan's Codex limits, as Codex writes them after each reply: 5 hours and a week.
LIMITS="\"rate_limits\":{\"limit_id\":\"codex\",\"limit_name\":null,\"primary\":{\"used_percent\":17.0,\"window_minutes\":300,\"resets_at\":$((NOW + 3 * 3600))},\"secondary\":{\"used_percent\":9.0,\"window_minutes\":10080,\"resets_at\":$((NOW + 5 * 86400))},\"credits\":{\"has_credits\":false,\"unlimited\":false,\"balance\":null},\"individual_limit\":null,\"spend_control_reached\":null,\"plan_type\":\"plus\",\"rate_limit_reached_type\":null}"

C1=01a3f2c4-7b18-7e42-9d06-5c81a0e3b7f2   # shop-api: login rate limit (Codex, open and waiting)
C2=01a3e9b0-2c55-7a19-8f3d-e4b7092c6a15   # tidy: --since flag (Codex)
C3=01a3f3d8-91a6-7c07-b2e4-38d5f1a9c0e6   # task #2's session (Codex, working)
C4=01a3c1e7-58d2-7b90-a4f1-6e0d3b8c2f47   # docs: changelog page (Codex)

cxconv $C1 shop-api feat/rate-limit gpt-6-luna 6 96000 finished "Rate limit the login endpoint" \
    "Rate limit POST /login: five tries a minute per IP and per email, with a clear message when it trips." \
    "Added a token bucket per IP and per email in front of the login handler: five tries a minute, then 429 with a Retry-After header and the message 'Too many attempts, try again in a minute.' Tests cover both limits and the reset."
cxconv $C2 tidy feat/since gpt-5.6-terra 300 54000 finished "Add a --since flag to tidy" \
    "Add --since 2026-01-01 so tidy only moves files modified after that date." \
    "Added --since, parsed as a date or a relative age like 30d. Files older than it are left alone and listed as skipped with --verbose. Three new tests."
cxconv $C4 docs-site main gpt-6-luna 2700 31000 finished "Generate the changelog page" \
    "Build a changelog page for the docs from CHANGELOG.md at build time." \
    "The docs build now renders CHANGELOG.md into /changelog, one section per release with anchors, and links it from the footer."
cxconv $C3 shop-worktrees/task-2 task/2 gpt-6-luna 1 118000 working "Discount codes at checkout" \
    "Work on task #2: Discount codes at checkout." \
    ""

# ---- names, generated names and summaries, the archive -----------------------------------------------------------
cat > "$CFG/names.json" <<EOF
{"version": 1, "names": {
  "$S1": "Stripe checkout",
  "$S2": "Basket rounding fix",
  "$S5": "Config file support",
  "$S6": "Release workflow",
  "$S9": "Link checker",
  "$S10": "Neovim keymaps",
  "$S11": "README rewrite",
  "$S12": "Postgres 16 upgrade",
  "$S14": "tmux status line",
  "$S16": "Windows paths",
  "$C2": "Since flag",
  "$C3": "Discount codes",
  "$C4": "Changelog page",
  "$S17": "Delivery date"
}}
EOF
cat > "$CFG/summaries.json" <<EOF
{"version": 1, "entries": {
  "$S3": {"name": "Fast product search", "summary": "Product search was slow because of a full scan. A trigram index and a search column brought it from 2 s to 18 ms; next is ranking by relevance.", "covers": 4, "model": "haiku", "at": $((NOW - 80000))},
  "$S4": {"name": "Order confirmation email", "summary": "Order emails now go out when an order is paid, with items, total and delivery date. The template still needs the shop's logo.", "covers": 3, "model": "haiku", "at": $((NOW - 340000))},
  "$S7": {"name": "Dry-run flag", "summary": "tidy --dry-run prints every planned move without touching anything; two tests cover it.", "covers": 2, "model": "haiku", "at": $((NOW - 490000))},
  "$S13": {"name": "Offline docs search", "summary": "The docs have a search box that works offline from an index built with the site; next is ranking headings above body text.", "covers": 3, "model": "haiku", "at": $((NOW - 90000))},
  "$S15": {"name": "Faster product images", "summary": "Product images are served as WebP and AVIF in four sizes and load lazily below the fold; mobile load time dropped from 4.1 s to 1.6 s.", "covers": 4, "model": "haiku", "at": $((NOW - 440000))},
  "$C1": {"name": "Login rate limit", "summary": "POST /login allows five tries a minute per IP and per email, then answers 429 with a Retry-After header and a plain message.", "covers": 1, "model": "haiku", "at": $((NOW - 200))},
  "$S8": {"name": "Docs dark mode", "summary": "Dark mode for the docs follows the system setting, with a toggle in the header that remembers the choice.", "covers": 3, "model": "haiku", "at": $((NOW - 18000))}
}}
EOF
cat > "$CFG/archive.json" <<EOF
{"archived": {"$S10": $((NOW - 700000)), "$S11": $((NOW - 900000)), "$S12": $((NOW - 800000))}, "touched": {}}
EOF

# ---- settings ----------------------------------------------------------------------------------------------------
# Dracula; no automatic archive or idle stop during the recording; `v` asks which editor. The project notes
# feature is on (so the notes pane reads as it does in daily use) but nothing can call it: no Stop hook runs here and
# the notes helper is off. Every other feature that would ask Claude is off, and the stand-in claude refuses `-p`.
cat > "$CFG/settings.json" <<'EOF'
{"theme": "dracula", "auto_archive_days": 0, "idle_stop_minutes": 0, "files_editor_ask": true,
 "tokens_enabled": true, "tokens_asked": true,
 "token_features": {"names": false, "find": false, "project_notes": true, "project_description": false}}
EOF

# ---- projects and their notes ------------------------------------------------------------------------------------
PW=p3c9e1a7b4d20
PT=p8f14b6c2e905
cat > "$CFG/projects.json" <<EOF
{
  "version": 1,
  "projects": {
    "$PW": {"name": "Web shop", "color": "green", "badge": "WS", "archived": false, "created": $((NOW - 1200000)),
            "archived_sessions": [],
            "description": "An online shop on Next.js and Stripe: checkout, basket, search and order emails.",
            "description_at": $((NOW - 3600))},
    "$PT": {"name": "tidy CLI", "color": "blue", "badge": "TC", "archived": false, "created": $((NOW - 900000)),
            "archived_sessions": [],
            "description": "A small command-line tool that sorts a messy folder into dated folders.",
            "description_at": $((NOW - 7200))}
  },
  "sessions": {"$S1": "$PW", "$S2": "$PW", "$S3": "$PW", "$S4": "$PW", "$S11": "$PW", "$S12": "$PW", "$S15": "$PW",
               "$C1": "$PW", "$C3": "$PW", "$S17": "$PW",
               "$S5": "$PT", "$S6": "$PT", "$S7": "$PT", "$S16": "$PT", "$C2": "$PT"}
}
EOF
d() { date -u -d "@$((NOW - $1 * 86400))" +%Y-%m-%d; }
mkdir -p "$CFG/projects/$PW/tasks" "$CFG/projects/$PT"
cat > "$CFG/projects/$PW/project.md" <<EOF
# Summary

An online shop for a small coffee roaster: a Next.js storefront, a Node API and Postgres. Checkout with Stripe is being built now; search and order emails are done. Launch is planned once checkout and discount codes work.

# Open questions

## Checkout

- Do we need guest checkout at launch, or can buyers sign in with an email link?
- Which countries do we ship to in the first month?

# Status

- Stripe checkout: API route and pages done, webhook in progress.
- Basket rounding fixed: discounts are rounded per line.
- Product search: 18 ms at 5,000 products.

# Decisions

- $(d 9): Postgres 16 for local development and production.
- $(d 6): Prices are stored in cents, never as decimals.
- $(d 3): Discounts are rounded per line, not per basket.
- $(d 1): The basket is cleared only when Stripe's webhook says paid.
EOF
cat > "$CFG/projects/$PT/project.md" <<EOF
# Summary

tidy sorts a messy folder (Downloads, a camera card) into dated folders. It is a single Rust binary that needs no config; a config file is being added for people who want their own rules.

# Requirements

- Never overwrite a file: a clash gets a numbered name.
- --dry-run shows every move first.

# Decisions

- $(d 8): Release binaries for Linux and macOS, x86_64 and aarch64.
- $(d 1): Command-line flags win over tidy.toml.
EOF

# Tasks on the web shop: one done, one running in Codex (its squad's model is one of Codex's), one waiting for an
# answer in Claude Code, one not started yet.
T=$((NOW - 3 * 86400))
WT=$H/code/shop-worktrees
cat > "$CFG/projects/$PW/tasks.json" <<EOF
{
  "next": 5,
  "tasks": [
    {"number": 1, "title": "Round discounts per line in the basket", "file": "01-round-discounts-per-line-in-the-basket.md",
     "state": "done", "folder": "$H/code/shop", "squad": "bug-fix", "session": "$S2", "started": $((NOW - 9000)),
     "worktree": {"path": "$WT/task-1", "branch": "task/1", "removed": $((NOW - 7000))},
     "created": $((T - 86400)), "updated": $((NOW - 7200))},
    {"number": 2, "title": "Discount codes at checkout", "file": "02-discount-codes-at-checkout.md",
     "state": "working", "folder": "$H/code/shop", "squad": "small-change", "session": "$C3", "started": $((NOW - 600)),
     "worktree": {"path": "$WT/task-2", "branch": "task/2"},
     "created": $T, "updated": $((NOW - 60)), "from": {"session": "$S1", "project": "$PW"}},
    {"number": 3, "title": "Show the delivery date on the order page", "file": "03-show-the-delivery-date-on-the-order-page.md",
     "state": "needs_you", "need": "question",
     "why": "Should the date be in the buyer's time zone or the shop's?",
     "folder": "$H/code/shop", "squad": "feature", "session": "$S17", "started": $((NOW - 2400)),
     "worktree": {"path": "$WT/task-3", "branch": "task/3"},
     "created": $T, "updated": $((NOW - 1500))},
    {"number": 4, "title": "Gift wrapping option", "file": "04-gift-wrapping-option.md",
     "state": "not_started", "folder": "$H/code/shop", "squad": "feature", "created": $T, "updated": $((T + 3600))}
  ]
}
EOF
cat > "$CFG/projects/$PW/tasks/01-round-discounts-per-line-in-the-basket.md" <<'EOF'
## What it should do

A basket with three or more discounted items shows the same total as the order email and the Stripe charge.

## Done when

- [x] Three items at 0.333 off each add up to the right total
- [x] The basket, the email and Stripe agree to the cent
EOF
cat > "$CFG/projects/$PW/tasks/02-discount-codes-at-checkout.md" <<'EOF'
## What it should do

A buyer can type a discount code on the basket page and see the new total before paying. A code can be a percentage or a fixed amount, can expire, and can be limited to a number of uses. Stripe charges the discounted total, and the order email shows the code and what it saved.

## Done when

- [ ] An expired or used-up code is refused with a plain message
- [ ] The basket, the Stripe charge and the email show the same discounted total
- [ ] Removing the code puts the full price back
- [ ] Tests run and reported
EOF
cat > "$CFG/projects/$PW/tasks/03-show-the-delivery-date-on-the-order-page.md" <<'EOF'
## What it should do

The order page shows the expected delivery date, the same one the confirmation email gives.

## Done when

- [ ] The order page and the email show the same date
- [ ] Tests run and reported
EOF
cat > "$CFG/projects/$PW/tasks/04-gift-wrapping-option.md" <<'EOF'
## What it should do

A buyer can ask for gift wrapping at checkout, for a small fee, with a message on a card.

## Done when

- [ ] Gift wrapping and its message show on the order and in the email
- [ ] Tests run and reported
EOF

# A squad of the user's own that runs in Codex: its model is one of Codex's.
mkdir -p "$CFG/squads"
cat > "$CFG/squads/small-change.md" <<'EOF'
---
name: Small change
description: A small, well-understood change, done in Codex and checked by Claude.
model: gpt-6-luna
verdict_model: sonnet
plan_first: false
---
1. Read the task and the code it touches. Keep the change as small as the task allows.
2. Make the change, with a test that shows it works.
3. Find how this repo runs its tests and run them all; fix what fails.
4. Reply with what changed, the test command you ran and its result.
EOF

# Codex's list of models, as Codex itself caches it (platoon reads the names and windows from here).
cat > "$CX/models_cache.json" <<'EOF'
{
 "fetched_at": "2026-10-24T01:57:35.594489377Z",
 "client_version": "0.163.0",
 "models": [
  {"slug": "gpt-6-luna", "display_name": "GPT-6-Luna", "visibility": "list", "context_window": 272000,
   "default_reasoning_level": "medium", "supported_in_api": true, "priority": 5},
  {"slug": "gpt-5.6-terra", "display_name": "GPT-5.6-Terra", "visibility": "list", "context_window": 272000,
   "default_reasoning_level": "medium", "supported_in_api": true, "priority": 8}
 ]
}
EOF

# ---- plan usage, as `platoon statusline` saves it ---------------------------------------------------------------
for id in $S1 $S5; do
    printf '{"ts":%d,"context_pct":%d,"five_hour":{"pct":38,"resets_at":%d},"seven_day":{"pct":21,"resets_at":%d}}\n' \
        "$NOW" "$([ "$id" = "$S1" ] && echo 61 || echo 44)" $((NOW + 2 * 3600 + 900)) $((NOW + 4 * 86400)) > "$CACHE/status/$id.json"
done

# ---- the stand-ins -------------------------------------------------------------------------------------------------
# `codex`: never the real one. It says its version, and opened from platoon (`codex resume <id> --no-daemon`) it shows
# that conversation's messages as plain text. It does nothing else: no model, no network.
cat > "$W/bin/codex" <<EOF
#!/usr/bin/env bash
# Stand-in for codex in platoon's README demo. Shows a saved conversation as plain text; runs nothing.
W='$W'
EOF
cat >> "$W/bin/codex" <<'EOF'
case ${1:-} in
    --version) echo "codex-cli 0.163.0"; exit 0 ;;
    exec | app-server) echo "demo stand-in: no model here" >&2; exit 1 ;;
esac
id=
[ "${1:-}" = resume ] && id=${2:-}
trap 'exit 0' TERM HUP INT
printf '\033[2J\033[H\n  \033[2m(demo world: a stand-in for Codex shows the saved conversation)\033[0m\n'
[ -n "$id" ] && [ -f "$W/replay/$id.txt" ] && cat "$W/replay/$id.txt"
printf '\n  \033[2m────────────────────────────────────────────────────────────────────────────────\033[0m\n  > '
while :; do sleep 1; done
EOF
chmod +x "$W/bin/codex"

# `claude`: never the real one. Opened from platoon (`claude --resume <id>`) it registers itself the way Claude Code
# does, so platoon sees it running, and shows that conversation's messages as plain text. It does nothing else.
cat > "$W/bin/claude" <<EOF
#!/usr/bin/env bash
# Stand-in for claude in platoon's README demo. Shows a saved conversation as plain text; runs nothing.
W='$W'
EOF
cat >> "$W/bin/claude" <<'EOF'
id=
while [ $# -gt 0 ]; do
    case $1 in
        --resume) id=$2; shift ;;
        -p|--print) echo "$*" >> "$W/p-calls"; echo "demo stand-in: no model here" >&2; exit 1 ;;
    esac
    shift
done
reg=$CLAUDE_CONFIG_DIR/sessions/$$.json
if [ -n "$id" ]; then
    printf '{"pid":%d,"sessionId":"%s","cwd":"%s","kind":"interactive","startedAt":%d,"status":"idle"}\n' \
        $$ "$id" "$PWD" "$(($(date +%s) * 1000))" > "$reg"
fi
trap 'rm -f "$reg"; exit 0' TERM HUP INT EXIT
printf '\033[2J\033[H\n  \033[2m(demo world: a stand-in for Claude Code shows the saved conversation)\033[0m\n'
[ -n "$id" ] && [ -f "$W/replay/$id.txt" ] && cat "$W/replay/$id.txt"
printf '\n  \033[2m────────────────────────────────────────────────────────────────────────────────\033[0m\n  > '
while :; do sleep 1; done
EOF
chmod +x "$W/bin/claude"

# `code`: the only editor `v` can find here. It does nothing.
printf '#!/bin/sh\n# Stand-in for VS Code in platoon'"'"'s README demo: does nothing.\nexit 0\n' > "$W/editors/code"
printf '#!/bin/sh\n# Stand-in for Zed in platoon'"'"'s README demo: does nothing.\nexit 0\n' > "$W/editors/zed"
chmod +x "$W/editors/code" "$W/editors/zed"

# ---- the environment -----------------------------------------------------------------------------------------------
cat > "$W/env.sh" <<EOF
# Source this: everything platoon and Claude Code would read or write points inside the demo world.
export HOME='$H'
export CLAUDE_CONFIG_DIR='$CL'
export PLATOON_CONFIG_DIR='$CFG'
export PLATOON_CACHE_DIR='$CACHE'
export PLATOON_DATA_DIR='$H/.local/share/platoon'
export XDG_CONFIG_HOME='$H/.config' XDG_CACHE_HOME='$H/.cache' XDG_DATA_HOME='$H/.local/share'
export XDG_STATE_HOME='$H/.local/state' XDG_RUNTIME_DIR='$W/run'
export PLATOON_SKIP_SETUP_CHECK=1
export PLATOON_CLAUDE_BIN='$W/bin/claude' PLATOON_FORBID_REAL_CLAUDE=1
export CODEX_HOME='$CX' PLATOON_CODEX_BIN='$W/bin/codex'
export PLATOON_NO_UPDATE_CHECK=1
export PLATOON_FORBID_REAL_EDITOR=1 PLATOON_EDITOR_PATH='$W/editors'
export PLATOON_CLIPBOARD=off
export PLATOON_NOTES_HELPER=off
export PATH='$W/bin':/usr/local/bin:/usr/bin:/bin
unset PLATOON_PROJECT CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
cd "\$HOME"
EOF

# live.sh: "running" conversations. Claude Code's are shell loops that write a registry entry the way Claude Code does
# (~/.claude/sessions/<pid>.json): S1 keeps working; S5 works for a while and then finishes, so the list shows it
# turn from working to done. Codex's are `flock` holding the conversation's writer lock, as a Codex window does: C1
# waits for you, C3 (task #2's session) is in the middle of a reply (its rollout says so). `stop` ends those, and any
# session host platoon started inside this world.
cat > "$W/live.sh" <<EOF
#!/usr/bin/env bash
W='$W' CL='$CL' CX='$CX' S1='$S1' S5='$S5' C1='$C1' C3='$C3' H='$H'
EOF
cat >> "$W/live.sh" <<'EOF'
reg() { printf '{"pid":%d,"sessionId":"%s","cwd":"%s","kind":"interactive","startedAt":%d,"status":"%s"}\n' \
    "$1" "$2" "$3" "$(($(date +%s) * 1000 - 600000))" "$4" > "$CL/sessions/$1.json"; }
case $1 in
start)
    for spec in "$S1 $H/code/shop 100000" "$S5 $H/code/tidy ${FINISH_AFTER:-12}"; do
        set -- $spec
        (
            trap 'rm -f "$CL/sessions/$BASHPID.json"; exit 0' TERM
            reg $BASHPID "$1" "$2" busy
            ticks=$(awk "BEGIN { print int($3 * 10) }")
            for ((i = 0; i < ticks; i++)); do sleep 0.1; done
            reg $BASHPID "$1" "$2" idle
            while :; do sleep 0.1; done
        ) > /dev/null 2>&1 &
        echo $! >> "$W/live.pids"
    done
    for id in $C1 $C3; do
        lock=$CX/thread-writer-locks/$id.lock
        : > "$lock"
        flock -x "$lock" sleep 100000 > /dev/null 2>&1 &
        echo $! >> "$W/live.pids"
    done
    ;;
stop)
    # Each pid here is live.sh's own: a loop, or a flock with its sleep (ended first, so flock lets go and exits).
    [ -f "$W/live.pids" ] && while read -r p; do
        for c in $(ps -o pid= --ppid "$p" 2> /dev/null); do kill "$c" 2> /dev/null; done
        kill "$p" 2> /dev/null
    done < "$W/live.pids"
    rm -f "$W/live.pids"
    # Session hosts platoon started here: their command line holds this world's path, which nothing else has.
    ps -eo pid=,args= | while read -r p args; do
        case $args in *"platoon host"*"$W"* | "bash $W/bin/claude"* | "bash $W/bin/codex"*) kill "$p" 2> /dev/null ;; esac
    done
    ;;
esac
EOF
chmod +x "$W/live.sh"
echo "demo world ready in $W"
