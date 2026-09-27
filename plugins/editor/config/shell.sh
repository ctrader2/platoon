#!/bin/sh
# platoon's code editor runs Neovim with its own XDG folders and its own programs first on PATH, so that nothing of
# the user's own Neovim is read or written. This is the editor's 'shell': before starting the user's shell ($SHELL)
# for the terminal, `:!` and `:terminal`, it gives back the user's own environment, as it was when platoon opened the
# editor. platoon writes this file (from its editor plugin, plugins/editor/config/shell.sh) each time it opens the editor.
#
# For each variable below, PLATOON_KEEP_<name> holds the user's value when they had one; without it the variable was
# not set, and it is taken away again. PLATOON_EDITOR_ENV says there is something to give back (a shell started from
# a shell started here has it all already).
if [ -n "${PLATOON_EDITOR_ENV:-}" ]; then
  for v in PATH XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME XDG_CONFIG_DIRS XDG_DATA_DIRS \
      NVIM_APPNAME NVIM_LOG_FILE VIMINIT VIMRUNTIME VIM MYVIMRC; do
    eval "had=\${PLATOON_KEEP_$v+set}"
    if [ "$had" = set ]; then
      eval "$v=\$PLATOON_KEEP_$v"
      export "$v"
    else
      unset "$v"
    fi
    unset "PLATOON_KEEP_$v"
  done
  unset had v PLATOON_EDITOR_ENV PLATOON_EDITOR_SESSION
fi
exec "${SHELL:-/bin/sh}" "$@"
