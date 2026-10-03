#!/usr/bin/env bash
# Builds the made-up world the README animation is recorded in, inside the folder given as the only argument
# (it must not exist yet, or be empty). Nothing outside that folder is read or written: it has its own home, its own
# Claude Code folder, platoon's config, cache and data folders, a stand-in `claude` and a stand-in `code`.
#
#   demo/make-world.sh /tmp/platoon-demo
#   . /tmp/platoon-demo/env.sh          # points HOME, CLAUDE_CONFIG_DIR, PLATOON_* and XDG_* into the world
#   /tmp/platoon-demo/live.sh start     # two stand-in "running sessions" (plain shell loops), see below
#   /tmp/platoon-demo/live.sh stop      # stops exactly what live.sh and platoon started inside this world
#
# Every conversation, project, ticket and name here is invented. The files are written in the shapes platoon reads
# (see src/claude.rs, src/projects.rs, src/tickets.rs, src/names.rs, src/summaries.rs, src/archive.rs, src/usage.rs).
# Times are relative to now, so the list always reads "2h ago", "3d ago". Needs bash and GNU date.
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
mkdir -p "$CL/projects" "$CL/sessions" "$CFG" "$CACHE/status" "$H/.local/share/platoon" "$H/.local/state" \
    "$W/bin" "$W/editors" "$W/replay" "$W/run"

NOW=$(date +%s)
NOW_MS=$((NOW * 1000))
iso() { date -u -d "@$(($1 / 1000))" +%Y-%m-%dT%H:%M:%S.000Z; }
esc() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'; }

# ---- the code folders --------------------------------------------------------------------------------------------
for d in shop shop-api tidy docs-site dotfiles; do mkdir -p "$H/code/$d"; done
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
    local folder=$CL/projects/$(printf '%s' "$dir" | tr '/.' '--')
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
  "$S16": "Windows paths"
}}
EOF
cat > "$CFG/summaries.json" <<EOF
{"version": 1, "entries": {
  "$S3": {"name": "Fast product search", "summary": "Product search was slow because of a full scan. A trigram index and a search column brought it from 2 s to 18 ms; next is ranking by relevance.", "covers": 4, "model": "haiku", "at": $((NOW - 80000))},
  "$S4": {"name": "Order confirmation email", "summary": "Order emails now go out when an order is paid, with items, total and delivery date. The template still needs the shop's logo.", "covers": 3, "model": "haiku", "at": $((NOW - 340000))},
  "$S7": {"name": "Dry-run flag", "summary": "tidy --dry-run prints every planned move without touching anything; two tests cover it.", "covers": 2, "model": "haiku", "at": $((NOW - 490000))},
  "$S13": {"name": "Offline docs search", "summary": "The docs have a search box that works offline from an index built with the site; next is ranking headings above body text.", "covers": 3, "model": "haiku", "at": $((NOW - 90000))},
  "$S15": {"name": "Faster product images", "summary": "Product images are served as WebP and AVIF in four sizes and load lazily below the fold; mobile load time dropped from 4.1 s to 1.6 s.", "covers": 4, "model": "haiku", "at": $((NOW - 440000))},
  "$S8": {"name": "Docs dark mode", "summary": "Dark mode for the docs follows the system setting, with a toggle in the header that remembers the choice.", "covers": 3, "model": "haiku", "at": $((NOW - 18000))}
}}
EOF
cat > "$CFG/archive.json" <<EOF
{"archived": {"$S10": $((NOW - 700000)), "$S11": $((NOW - 900000)), "$S12": $((NOW - 800000))}, "touched": {}}
EOF

# ---- settings ----------------------------------------------------------------------------------------------------
# Catppuccin Mocha; no automatic archive or idle stop during the recording; `v` asks which editor. The project notes
# feature is on (so the notes pane reads as it does in daily use) but nothing can call it: no Stop hook runs here and
# the notes helper is off. Every other feature that would ask Claude is off, and the stand-in claude refuses `-p`.
cat > "$CFG/settings.json" <<'EOF'
{"theme": "catppuccin-mocha", "auto_archive_days": 0, "idle_stop_minutes": 0, "files_editor_ask": true,
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
               "$S5": "$PT", "$S6": "$PT", "$S7": "$PT", "$S16": "$PT"}
}
EOF
d() { date -u -d "@$((NOW - $1 * 86400))" +%Y-%m-%d; }
mkdir -p "$CFG/projects/$PW/tickets" "$CFG/projects/$PT"
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

# Tickets on the web shop: one done, two new (one waits for the other), one cancelled.
T=$((NOW - 3 * 86400))
cat > "$CFG/projects/$PW/tickets.json" <<EOF
{
  "next": 5,
  "tickets": [
    {"number": 1, "title": "Round discounts per line in the basket", "file": "01-round-discounts-per-line-in-the-basket.md",
     "state": "done", "folders": ["$H/code/shop"], "blocked_by": [], "created": $((T - 86400)), "updated": $T},
    {"number": 2, "title": "Discount codes at checkout", "file": "02-discount-codes-at-checkout.md",
     "state": "new", "folders": ["$H/code/shop", "$H/code/shop-api"], "blocked_by": [], "created": $T, "updated": $T,
     "from": {"session": "$S1", "project": "$PW"}},
    {"number": 3, "title": "Show the delivery date on the order page", "file": "03-show-the-delivery-date-on-the-order-page.md",
     "state": "new", "folders": ["$H/code/shop"], "blocked_by": [2], "created": $T, "updated": $T},
    {"number": 4, "title": "Gift wrapping option", "file": "04-gift-wrapping-option.md",
     "state": "cancelled", "folders": ["$H/code/shop"], "blocked_by": [], "created": $T, "updated": $((T + 3600))}
  ]
}
EOF
cat > "$CFG/projects/$PW/tickets/01-round-discounts-per-line-in-the-basket.md" <<'EOF'
## What it should do

A basket with three or more discounted items shows the same total as the order email and the Stripe charge.

How far: a local commit.

## Acceptance

- [x] Three items at 0.333 off each add up to the right total
- [x] The basket, the email and Stripe agree to the cent
EOF
cat > "$CFG/projects/$PW/tickets/02-discount-codes-at-checkout.md" <<'EOF'
## What it should do

A buyer can type a discount code on the basket page and see the new total before paying. A code can be a percentage or a fixed amount, can expire, and can be limited to a number of uses. Stripe charges the discounted total, and the order email shows the code and what it saved.

How far: local changes only (say if you want a commit, or a push and merge request).

## Acceptance

- [ ] An expired or used-up code is refused with a plain message
- [ ] The basket, the Stripe charge and the email show the same discounted total
- [ ] Removing the code puts the full price back
EOF
cat > "$CFG/projects/$PW/tickets/03-show-the-delivery-date-on-the-order-page.md" <<'EOF'
## What it should do

The order page shows the expected delivery date, the same one the confirmation email gives.

How far: local changes only (say if you want a commit, or a push and merge request).

## Acceptance

- [ ] The order page and the email show the same date
EOF
cat > "$CFG/projects/$PW/tickets/04-gift-wrapping-option.md" <<'EOF'
## What it should do

A buyer can ask for gift wrapping at checkout.

How far: local changes only (say if you want a commit, or a push and merge request).

## Acceptance

- [ ] Gift wrapping shows on the order and in the email
EOF

# ---- plan usage, as `platoon statusline` saves it ---------------------------------------------------------------
for id in $S1 $S5; do
    printf '{"ts":%d,"context_pct":%d,"five_hour":{"pct":38,"resets_at":%d},"seven_day":{"pct":21,"resets_at":%d}}\n' \
        "$NOW" "$([ "$id" = "$S1" ] && echo 61 || echo 44)" $((NOW + 2 * 3600 + 900)) $((NOW + 4 * 86400)) > "$CACHE/status/$id.json"
done

# ---- the stand-ins -------------------------------------------------------------------------------------------------
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
export PLATOON_FORBID_REAL_EDITOR=1 PLATOON_EDITOR_PATH='$W/editors'
export PLATOON_CLIPBOARD=off
export PLATOON_NOTES_HELPER=off
export PATH='$W/bin':/usr/local/bin:/usr/bin:/bin
unset PLATOON_PROJECT CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
cd "\$HOME"
EOF

# live.sh: "running" conversations. Each is a shell loop that writes a registry entry the way Claude Code does
# (~/.claude/sessions/<pid>.json): S1 keeps working; S5 works for a while and then finishes, so the list shows it
# turn from working to done. `stop` ends those loops and any session host platoon started inside this world.
cat > "$W/live.sh" <<EOF
#!/usr/bin/env bash
W='$W' CL='$CL' S1='$S1' S5='$S5' H='$H'
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
    ;;
stop)
    [ -f "$W/live.pids" ] && while read -r p; do kill "$p" 2> /dev/null; done < "$W/live.pids"
    rm -f "$W/live.pids"
    # Session hosts platoon started here: their command line holds this world's path, which nothing else has.
    ps -eo pid=,args= | while read -r p args; do
        case $args in *"platoon host"*"$W"* | "bash $W/bin/claude"*) kill "$p" 2> /dev/null ;; esac
    done
    ;;
esac
EOF
chmod +x "$W/live.sh"
echo "demo world ready in $W"
