#!/bin/sh
# A stand-in for `codex app-server --listen stdio://`, for tests only (the real one would read and write the user's
# ~/.codex). It records how it was started (`args`, `codex-home`) and every line it is sent (`requests`), and answers a
# request from a file the test wrote: `<method>.<n>` for the n-th request of that method, else `<method>`, with `/` in
# the method written `-` (`hooks-list.2`, `config-batchWrite`) and `@ID@` replaced by the request's id. A file may hold
# several lines (a notification first, garbage, an error) or nothing; no file: no answer. `exit-at-once` makes it stop
# before reading anything; `hang` makes it never read nor end by itself (its pid is in `pid`). Otherwise it ends when
# its input does, as Codex does.
dir='@DIR@'
printf '%s\n' "$@" >"$dir/args"
printf '%s\n' "${CODEX_HOME:-}" >"$dir/codex-home"
echo $$ >"$dir/pid"
if [ -e "$dir/exit-at-once" ]; then
    exit 1
fi
if [ -e "$dir/hang" ]; then
    exec sleep 300
fi
while IFS= read -r line; do
    printf '%s\n' "$line" >>"$dir/requests"
    id=$(printf '%s\n' "$line" | sed -n 's/^{"id":\([0-9][0-9]*\),.*/\1/p')
    if [ -z "$id" ]; then
        continue
    fi
    method=$(printf '%s\n' "$line" | sed -n 's/^{"id":[0-9]*,"method":"\([^"]*\)".*/\1/p')
    n=$(grep -c "^{\"id\":[0-9]*,\"method\":\"$method\"" "$dir/requests")
    name=$(printf '%s' "$method" | tr '/' '-')
    if [ -e "$dir/$name.$n" ]; then
        sed "s/@ID@/$id/g" "$dir/$name.$n"
    elif [ -e "$dir/$name" ]; then
        sed "s/@ID@/$id/g" "$dir/$name"
    fi
done
