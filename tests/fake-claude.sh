#!/bin/sh
dir='@DIR@'
k=$(( $(ls "$dir/calls" | grep -c '\.stdin$') + 1 ))
c="$dir/calls/$k"
for a in "$@"; do printf '%s\n' "$a"; done > "$c.argv"
pwd > "$c.cwd"
env > "$c.env"
cat > "$c.stdin"
if [ -e "$dir/hang" ]; then
  echo $$ > "$dir/pid"
  sleep 300 &
  echo $! > "$dir/sleep-pid"
  wait
fi
[ -e "$dir/delay" ] && sleep "$(cat "$dir/delay")"
usage='{"input_tokens":400,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":50}'
[ -e "$dir/usage" ] && usage=$(cat "$dir/usage")
if [ -e "$dir/fail" ]; then
  printf '{"type":"result","subtype":"error","is_error":true,"result":"%s","usage":%s}' "$(cat "$dir/fail")" "$usage"
  exit 1
fi
if [ -e "$dir/reply.$k" ]; then
  text=$(cat "$dir/reply.$k")
elif [ -e "$dir/reply.default" ]; then
  text=$(cat "$dir/reply.default")
elif grep -q '^Description:' "$c.stdin"; then
  n=$(grep -c '^\[[0-9]*\]$' "$c.stdin")
  text='['
  i=1
  while [ "$i" -le 3 ] && [ "$i" -le "$n" ]; do
    [ "$i" -gt 1 ] && text="$text,"
    text="$text{\"n\":$i,\"reason\":\"Reason $i\"}"
    i=$((i+1))
  done
  text="$text]"
else
  n=$(grep -c '^\[[0-9]*\]$' "$c.stdin")
  text='['
  i=1
  while [ "$i" -le "$n" ]; do
    [ "$i" -gt 1 ] && text="$text,"
    text="$text{\"n\":$i,\"name\":\"Name $k.$i\",\"summary\":\"Summary $k.$i\"}"
    i=$((i+1))
  done
  text="$text]"
fi
# The reply as a JSON string, as claude -p writes it: line breaks kept (as \n), quotes and backslashes escaped.
esc=$(printf '%s' "$text" | sed 's/\\/\\\\/g; s/"/\\"/g' | awk 'NR > 1 { printf "\\n" } { printf "%s", $0 }')
printf '{"type":"result","subtype":"success","is_error":false,"result":"%s","total_cost_usd":0.001,"usage":%s}' "$esc" "$usage"
