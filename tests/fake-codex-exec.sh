#!/bin/sh
# A stand-in for `codex exec --json`, for tests only (the real one spends the user's Codex allowance). It records every
# call (`calls/N.argv`, one argument per line, `.cwd`, `.env`, `.stdin`, and `.schema`: the file `--output-schema`
# named, copied while it was there; N from 1) and answers with Codex's JSON events: `thread.started`, `turn.started`,
# one `agent_message` item with the model's words, and `turn.completed` with the usage. The words come from `reply.N`
# (for call N), else `reply.default`, else a naming answer built from the numbers it finds in the request (`[1]`,
# `[2]`, ...), as fake-claude.sh does. `usage` holds `turn.completed`'s usage object.
#
# The ways a call goes wrong: `out.N` (or `out.default`) is printed instead, as it is, and the call exits with the
# code in `exit` (0 when there is none); `hang` makes it sleep until killed (its pid is written to `pid`).
dir='@DIR@'
# The call's number, claimed (noclobber) so that calls running at the same time never share one.
k=1
while ! (set -C; : > "$dir/calls/$k.argv") 2>/dev/null; do k=$((k + 1)); done
c="$dir/calls/$k"
for a in "$@"; do printf '%s\n' "$a"; done > "$c.argv"
pwd > "$c.cwd"
env > "$c.env"
schema=''
prev=''
for a in "$@"; do
  if [ "$prev" = "--output-schema" ]; then schema=$a; fi
  prev=$a
done
if [ -n "$schema" ] && [ -e "$schema" ]; then cp "$schema" "$c.schema"; fi
# Whole before it is counted: a call is counted by its .stdin file, and a test reads it once counted.
cat > "$c.stdin.part" && mv "$c.stdin.part" "$c.stdin"
if [ -e "$dir/hang" ]; then
  echo $$ > "$dir/pid"
  sleep 300 &
  echo $! > "$dir/sleep-pid"
  wait
fi
code=0
[ -e "$dir/exit" ] && code=$(cat "$dir/exit")
if [ -e "$dir/out.$k" ]; then
  cat "$dir/out.$k"
  exit "$code"
elif [ -e "$dir/out.default" ]; then
  cat "$dir/out.default"
  exit "$code"
fi
usage='{"input_tokens":21000,"cached_input_tokens":12800,"cache_write_input_tokens":0,"output_tokens":40,"reasoning_output_tokens":0}'
[ -e "$dir/usage" ] && usage=$(cat "$dir/usage")
if [ -e "$dir/reply.$k" ]; then
  text=$(cat "$dir/reply.$k")
elif [ -e "$dir/reply.default" ]; then
  text=$(cat "$dir/reply.default")
else
  n=$(grep -c '^\[[0-9]*\]$' "$c.stdin")
  text='['
  i=1
  while [ "$i" -le "$n" ]; do
    [ "$i" -gt 1 ] && text="$text,"
    text="$text{\"n\":$i,\"name\":\"Codex name $k.$i\",\"summary\":\"Codex summary $k.$i\"}"
    i=$((i+1))
  done
  text="$text]"
fi
# The words as a JSON string: line breaks kept (as \n), quotes and backslashes escaped.
esc=$(printf '%s' "$text" | sed 's/\\/\\\\/g; s/"/\\"/g' | awk 'NR > 1 { printf "\\n" } { printf "%s", $0 }')
printf '{"type":"thread.started","thread_id":"01a15174-fa71-79a1-9294-3d8358aa5f45"}\n'
printf '{"type":"turn.started"}\n'
printf '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"%s"}}\n' "$esc"
printf '{"type":"turn.completed","usage":%s}\n' "$usage"
exit "$code"
