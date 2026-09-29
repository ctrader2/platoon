-- platoon's code editor: Neovim with platoon's own settings, opened with `v` on a ticket, a session or a folder.
--
-- platoon writes this file (from its editor plugin, plugins/editor/config/init.lua) into its editor folder every time it
-- opens the editor, so changes made here are overwritten. What changes from one opening to the next - the folders, the
-- commits a ticket's changes are counted from, the colors of platoon's theme, whether the font has icons - is in the
-- session file named by $PLATOON_EDITOR_SESSION (JSON written by platoon), and so is the user's back key (`keys.back`, from
-- platoon's keys.json, in Neovim's notation; Ctrl+Space when the session doesn't say).
--
-- Nothing here reads or writes ~/.config/nvim or nvim's usual data folders: platoon starts nvim with its XDG folders
-- inside its own editor folder, and gives programs started from the editor (the terminal, `:!`) the user's own
-- environment back through the `shell.sh` script beside this file.

-- ---- the session ---------------------------------------------------------------------------------------------------

local function read_json(path)
  local f = path and io.open(path, 'r')
  if not f then
    return nil
  end
  local text = f:read('*a')
  f:close()
  local ok, v = pcall(vim.json.decode, text, { luanil = { object = true, array = true } })
  return ok and v or nil
end

local S = read_json(os.getenv('PLATOON_EDITOR_SESSION')) or {}
S.roots = S.roots or {}
S.colors = S.colors or { mode = 'terminal' }
local icons = S.icons == true
-- The back key: back to platoon from anywhere in the editor, as `q` is in normal mode.
local BACK = type(S.keys) == 'table' and type(S.keys.back) == 'string' and S.keys.back ~= '' and S.keys.back or '<C-Space>'
_G.PlatoonEditor = { session = S, back_key = BACK }

--- Whether two keys in Neovim's notation are the same key (`<C-space>` and `<C-Space>`).
local function same_key(a, b)
  return vim.api.nvim_replace_termcodes(a, true, true, true) == vim.api.nvim_replace_termcodes(b, true, true, true)
end

vim.opt.packpath:prepend(S.site)
vim.opt.runtimepath:prepend(S.site)
vim.opt.runtimepath:prepend(S.config_dir)

vim.g.mapleader = ' '
vim.g.maplocalleader = ' '
vim.g.loaded_netrw = 1
vim.g.loaded_netrwPlugin = 1

-- ---- plain settings ------------------------------------------------------------------------------------------------

local o = vim.o
o.number = true
o.signcolumn = 'yes'
o.cursorline = true
o.mouse = 'a'
o.splitright = true
o.splitbelow = true
o.ignorecase = true
o.smartcase = true
o.undofile = true
o.updatetime = 300
o.timeoutlen = 400
o.scrolloff = 4
o.showmode = false
o.laststatus = 3
o.expandtab = true
o.shiftwidth = 4
o.tabstop = 4
-- Copying: see "copying" below (a yank also goes to the system clipboard; deleting and pasting stay in the editor).
o.clipboard = ''
-- The squad (or anything else) may change files while they are open here: read them again when they change on disk
-- and nothing was changed here; Vim's own warning (W12) stays for a file changed both here and on disk.
o.autoread = true
-- Programs started from the editor get the user's own environment back (see the top of this file).
if S.shell and vim.uv.fs_stat(S.shell) then
  o.shell = S.shell
end

-- ---- copying: to the system clipboard, the way platoon copies -----------------------------------------------------
--
-- A yank (`y`, `yy`, `Y`, `y` on a selection; also "+y) goes where platoon's own copies go, as PLATOON_CLIPBOARD
-- says (the session's `clipboard`):
-- - "system": the system clipboard through platoon itself (`platoon clipboard-hold`, which stays in the background
--   holding the text until something else is copied, the way xclip does; no clipboard tool needs to be installed),
--   and OSC 52, the escape sequence that asks the terminal to set its own clipboard (Ghostty, kitty, WezTerm, iTerm2,
--   and over SSH);
-- - "osc52": only OSC 52; "off": nothing leaves the editor.
-- Deleting (`d`, `x`, `c`) doesn't touch the system clipboard. Pasting (`p`, and "+p) always pastes from the editor's
-- own registers: nothing asks the terminal for its clipboard (many terminals never answer, and Neovim would wait for
-- ten seconds); text copied in another program is pasted with the terminal's own paste (ctrl+shift+v).

local CB = S.clipboard or { target = 'off' }
local last = { lines = { '' }, regtype = 'v' }

--- `lines` as a clipboard provider is given them: a whole-line copy ends with an empty item (its last line break).
local function copy_out(lines, regtype)
  local out = vim.list_extend({}, lines)
  last = { lines = out, regtype = regtype or 'v' }
  if CB.target ~= 'system' and CB.target ~= 'osc52' then
    return
  end
  local text = table.concat(out, '\n')
  if CB.target == 'system' and type(CB.helper) == 'table' and #CB.helper > 0 then
    pcall(vim.system, CB.helper, { stdin = text, detach = true, stdout = false, stderr = false })
  end
  -- A sequence longer than terminals take is printed as garbage by some: bigger texts go by the system route only.
  local max = tonumber(CB.osc52_max) or 100000
  if 4 * math.ceil(#text / 3) <= max then
    pcall(function()
      require('vim.ui.clipboard.osc52').copy('+')(out)
    end)
  end
end

-- "+y and "*y go through this provider; plain `y` (into the unnamed register) is sent out by the autocmd below, so
-- that deletes and pastes never touch it.
vim.g.clipboard = {
  name = 'platoon',
  copy = { ['+'] = copy_out, ['*'] = copy_out },
  paste = {
    ['+'] = function()
      return { last.lines, last.regtype }
    end,
    ['*'] = function()
      return { last.lines, last.regtype }
    end,
  },
  cache_enabled = false,
}

vim.api.nvim_create_autocmd('TextYankPost', {
  group = vim.api.nvim_create_augroup('platoon_copy', { clear = true }),
  callback = function()
    local ev = vim.v.event
    if ev.operator == 'y' and (ev.regname == nil or ev.regname == '') then
      local lines = vim.list_extend({}, ev.regcontents)
      if (ev.regtype or ''):sub(1, 1) == 'V' then
        table.insert(lines, '')
      end
      copy_out(lines, ev.regtype)
    end
  end,
})

-- ---- colors: platoon's theme ---------------------------------------------------------------------------------------

local C = S.colors
local truecolor = C.mode == 'truecolor'
o.termguicolors = truecolor
-- Dark or light, as platoon's theme is (an older platoon doesn't say: its themes were all dark); with the terminal's
-- own colors the terminal says which it is.
if C.mode ~= 'terminal' then
  o.background = C.background == 'light' and 'light' or 'dark'
end

--- A role's color as this terminal takes it: '#rrggbb' in truecolor, a 0-255 number otherwise.
local function pick(c)
  if not c then
    return nil
  end
  if truecolor then
    return c.gui
  end
  return c.cterm
end

--- The base colors: base16 from platoon's theme (it starts with `hi clear`, so it runs once, first), and the
--- terminal's 16 colors.
local function apply_base()
  if C.base16 then
    local ok, base16 = pcall(require, 'mini.base16')
    if ok then
      base16.setup({ palette = C.base16, use_cterm = C.base16_cterm or true, plugins = { default = true } })
    end
  end
  vim.g.colors_name = 'platoon'
  if truecolor and C.terminal then
    for i, hex in ipairs(C.terminal) do
      vim.g['terminal_color_' .. (i - 1)] = hex
    end
  end
end

--- platoon's own groups on top (again once the plugins have defined theirs).
local function apply_colors()
  for _, g in ipairs(C.groups or {}) do
    local spec = {}
    if g.link then
      spec.link = g.link
    else
      for _, k in ipairs({ 'fg', 'bg', 'sp' }) do
        local c = g[k]
        if c then
          if c.gui then
            spec[k] = c.gui
          end
          if c.cterm ~= nil then
            spec['cterm' .. k] = c.cterm
          end
        end
      end
      if g.bold then
        spec.bold = true
        spec.cterm = { bold = true }
      end
      if g.reverse then
        spec.reverse = true
        spec.cterm = vim.tbl_extend('force', spec.cterm or {}, { reverse = true })
      end
    end
    vim.api.nvim_set_hl(0, g.name, spec)
  end
end

-- ---- plugins -------------------------------------------------------------------------------------------------------

-- Everything platoon installed is a native package under site/pack/platoon/start: load them now, so they can be set
-- up below.
vim.cmd('packloadall')
apply_base()
apply_colors()

local function try(what, f)
  local ok, err = pcall(f)
  if not ok then
    vim.schedule(function()
      vim.notify(('platoon: %s could not start: %s'):format(what, err), vim.log.levels.WARN)
    end)
  end
end

--- A mapping with a plain description (which-key shows it).
local function map(mode, lhs, rhs, desc, opts)
  opts = vim.tbl_extend('force', { silent = true, desc = desc }, opts or {})
  vim.keymap.set(mode, lhs, rhs, opts)
end

-- ---- back to platoon: `q`, and the back key ------------------------------------------------------------------------------------------

--- Terminal buffers whose program is still running.
local function running_terminals()
  local n = 0
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(b) and vim.bo[b].buftype == 'terminal' then
      local chan = vim.bo[b].channel
      if chan and chan > 0 and vim.fn.jobwait({ chan }, 0)[1] == -1 then
        n = n + 1
      end
    end
  end
  return n
end

local function unsaved()
  local out = {}
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(b) and vim.bo[b].modified and vim.bo[b].buftype == '' then
      local name = vim.api.nvim_buf_get_name(b)
      table.insert(out, name ~= '' and vim.fn.fnamemodify(name, ':~:.') or 'a new file with no name')
    end
  end
  return out
end

--- Leave the editor and go back to platoon. With changes not saved it asks first: save them, throw them away, or
--- stay. A program still running in the editor's terminal is said too, since leaving stops it.
function PlatoonEditor.back()
  local changed = unsaved()
  if #changed > 0 then
    local what = #changed == 1 and ('%s has changes you have not saved.'):format(changed[1])
      or ('%d files have changes you have not saved.'):format(#changed)
    local answer = vim.fn.confirm(what .. '\nSave them before going back to platoon?', '&Save\n&Discard them\n&Keep editing', 3)
    if answer == 1 then
      pcall(vim.cmd, 'silent! wall')
      local left = unsaved()
      if #left > 0 then
        vim.notify(('Could not save %s. Nothing was closed.'):format(table.concat(left, ', ')), vim.log.levels.WARN)
        return
      end
    elseif answer ~= 2 then
      return
    end
  elseif running_terminals() > 0 then
    local answer = vim.fn.confirm('A program is still running in the terminal. Going back to platoon stops it.', '&Go back\n&Stay here', 2)
    if answer ~= 1 then
      return
    end
  end
  -- A terminal still open goes first (the terminal plugin's own clean-up at exit can otherwise keep the editor from
  -- closing); that ends the program in it, as leaving would anyway.
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_valid(b) and vim.bo[b].buftype == 'terminal' then
      pcall(vim.api.nvim_buf_delete, b, { force = true })
    end
  end
  vim.cmd('qall!')
end

map('n', 'q', PlatoonEditor.back, 'Back to platoon')
-- Recording a macro, which `q` did, moves to `Q`.
map('n', 'Q', 'q', 'Record a macro (what q does in Vim)', { remap = false })
-- The back key, in normal, insert, visual and terminal mode alike. (A key Neovim can't map is left out, quietly:
-- `q` still works.)
local back_modes = { 'n', 'i', 'x', 's', 't' }
pcall(map, back_modes, BACK, PlatoonEditor.back, 'Back to platoon')
-- Ctrl+Space reaches Neovim as a NUL byte, which some terminals and Neovim versions call Ctrl+@.
if same_key(BACK, '<C-Space>') then
  pcall(map, back_modes, '<C-@>', PlatoonEditor.back, 'Back to platoon')
end
-- Ctrl+Z would stop the editor with platoon waiting behind it: `q` goes back instead.
if not same_key(BACK, '<C-z>') then
  map({ 'n', 'v' }, '<C-z>', '<Nop>', 'Nothing (q or the back key goes back to platoon)')
end

-- ---- the roots -----------------------------------------------------------------------------------------------------

local roots = S.roots

--- The root a file is in (the deepest one), or nil.
local function root_of(path)
  local best
  for _, r in ipairs(roots) do
    if path == r.path or vim.startswith(path, r.path .. '/') then
      if not best or #r.path > #best.path then
        best = r
      end
    end
  end
  return best
end
PlatoonEditor.root_of = root_of

--- The folder things start in for the current file: its repository, else its root, else the first root.
local function here()
  local name = vim.api.nvim_buf_get_name(0)
  local r = name ~= '' and root_of(vim.fn.fnamemodify(name, ':p')) or nil
  if r then
    return r.repo or r.path, r
  end
  local first = roots[1]
  return first and (first.repo or first.path) or vim.fn.getcwd(), first
end

local function root_paths()
  local out = {}
  for _, r in ipairs(roots) do
    table.insert(out, r.path)
  end
  return out
end

if roots[1] and vim.uv.fs_stat(roots[1].path) then
  vim.cmd.cd(vim.fn.fnameescape(roots[1].path))
end

-- ---- reading files changed on disk ---------------------------------------------------------------------------------

vim.api.nvim_create_autocmd({ 'FocusGained', 'BufEnter', 'CursorHold', 'CursorHoldI', 'TermLeave' }, {
  group = vim.api.nvim_create_augroup('platoon_autoread', { clear = true }),
  callback = function()
    if vim.fn.mode() ~= 'c' and vim.fn.getcmdwintype() == '' then
      pcall(vim.cmd, 'checktime')
    end
  end,
})

-- ---- syntax highlighting -------------------------------------------------------------------------------------------

try('syntax highlighting', function()
  require('nvim-treesitter').setup({ install_dir = S.site })
end)

-- Languages whose parser was tried this session (built on first open, or found missing), so it is tried once.
local tried = {}

--- Highlight `buf` with its language's parser. One platoon didn't build is built on first open with the installed
--- tree-sitter CLI and the C compiler, quietly; when that can't be done the file just has no highlighting.
local function highlight(buf, ft)
  local lang = vim.treesitter.language.get_lang(ft) or ft
  if pcall(vim.treesitter.start, buf, lang) then
    return
  end
  if not S.compile_parsers or tried[lang] then
    return
  end
  tried[lang] = true
  local ok, ts = pcall(require, 'nvim-treesitter')
  if not ok or not vim.list_contains(ts.get_available(), lang) then
    return
  end
  -- nvim-treesitter says what it is doing on the message line; this runs in the background, so it says nothing.
  local log = require('nvim-treesitter.log')
  local Logger = log.Logger
  local saved = { Logger.info, Logger.warn, Logger.error }
  Logger.info, Logger.warn = function() end, function() end
  Logger.error = function(_, m, ...)
    return m:format(...)
  end
  local ok2, task = pcall(ts.install, { lang })
  local function restore()
    Logger.info, Logger.warn, Logger.error = saved[1], saved[2], saved[3]
  end
  if not ok2 or not task then
    restore()
    return
  end
  task:await(function()
    vim.schedule(function()
      restore()
      if vim.api.nvim_buf_is_valid(buf) then
        pcall(vim.treesitter.start, buf, lang)
      end
    end)
  end)
end

vim.api.nvim_create_autocmd('FileType', {
  group = vim.api.nvim_create_augroup('platoon_highlight', { clear = true }),
  callback = function(ev)
    if vim.bo[ev.buf].buftype == '' then
      highlight(ev.buf, ev.match)
    end
  end,
})

-- ---- icons: Nerd Font glyphs when the font has them, plain ones otherwise ------------------------------------------

try('icons', function()
  if icons then
    require('mini.icons').setup({ style = 'glyph' })
    require('mini.icons').mock_nvim_web_devicons()
  else
    -- without the font, no file-type icons at all (plain folder marks stay): letters in their place read as noise
    require('mini.icons').setup({ style = 'ascii' })
  end
end)

-- ---- the side panel: Files and Changed files -----------------------------------------------------------------------

local tree = require('platoon.tree')
tree.session = S
tree.register()

local function neotree(source, action)
  require('neo-tree.command').execute({ source = source, position = 'left', action = action or 'focus' })
end

--- Diff of one repository: a ticket's worktree against the commit it was made from (committed and not yet committed
--- changes), anything else against its last commit.
local function diff_repo(g)
  local args = {}
  if g.base then
    table.insert(args, g.base)
  end
  table.insert(args, '-C' .. g.repo)
  vim.cmd('DiffviewOpen ' .. table.concat(vim.tbl_map(vim.fn.fnameescape, args), ' '))
end

--- Which repository: the only one, else the one asked for.
local function with_repo(prompt, f)
  local repos = tree.repos()
  if #repos == 0 then
    vim.notify('None of these folders is in git.', vim.log.levels.INFO)
  elseif #repos == 1 then
    f(repos[1])
  else
    vim.ui.select(repos, {
      prompt = prompt,
      format_item = function(g)
        return g.name .. (g.base and (' · since ' .. g.base:sub(1, 7)) or ' · not committed') .. '  ' .. vim.fn.fnamemodify(g.repo, ':~')
      end,
    }, function(g)
      if g then
        f(g)
      end
    end)
  end
end
PlatoonEditor.diff = function()
  -- the repository of the file in view first; otherwise asked
  local name = vim.api.nvim_buf_get_name(0)
  local r = name ~= '' and root_of(vim.fn.fnamemodify(name, ':p')) or nil
  if r and r.repo then
    for _, g in ipairs(tree.repos()) do
      if g.repo == r.repo then
        return diff_repo(g)
      end
    end
  end
  with_repo('Which repository?', diff_repo)
end
PlatoonEditor.diff_pick = function()
  with_repo('Which repository?', diff_repo)
end

local ascii_folders = { folder_closed = '+', folder_open = '-', folder_empty = '~', default = ' ' }

try('the file tree', function()
  require('neo-tree').setup({
    sources = { 'platoon_files', 'platoon_changes' },
    -- platoon's own views replace neo-tree's filesystem view, which isn't loaded; opening a folder is left to Vim
    filesystem = { hijack_netrw_behavior = 'disabled' },
    default_source = 'platoon_files',
    close_if_last_window = false,
    popup_border_style = 'rounded',
    enable_git_status = true,
    enable_diagnostics = false,
    use_popups_for_input = false,
    log_level = 'warn',
    source_selector = {
      winbar = true,
      statusline = false,
      sources = {
        { source = 'platoon_files', display_name = ' 1 Files ' },
        { source = 'platoon_changes', display_name = ' 2 Changed files ' },
      },
    },
    default_component_configs = {
      indent = { with_expanders = icons, expander_collapsed = icons and '' or '>', expander_expanded = icons and '' or 'v' },
      icon = icons and {} or ascii_folders,
      modified = { symbol = icons and '●' or '*' },
      git_status = {
        symbols = icons and {
          added = '+', modified = '~', deleted = '-', renamed = '>', untracked = '?', ignored = '', unstaged = '', staged = '', conflict = '!',
        } or {
          added = '+', modified = '~', deleted = '-', renamed = '>', untracked = '?', ignored = '', unstaged = '', staged = '', conflict = '!',
        },
      },
      name = { use_git_status_colors = true },
    },
    window = {
      position = 'left',
      width = 36,
      mappings = {
        -- space is the leader key (platoon's keys); neo-tree's own use of it goes
        ['<space>'] = 'none',
        ['q'] = function()
          PlatoonEditor.back()
        end,
        [BACK] = function()
          PlatoonEditor.back()
        end,
        ['<esc>'] = 'close_window',
        ['<cr>'] = 'open',
        ['o'] = 'open',
        ['l'] = 'open',
        ['<right>'] = 'open',
        ['h'] = 'close_node',
        ['<left>'] = 'close_node',
        ['1'] = function()
          neotree('platoon_files')
        end,
        ['2'] = function()
          neotree('platoon_changes')
        end,
        ['<tab>'] = 'next_source',
        ['<s-tab>'] = 'prev_source',
        ['D'] = function(state)
          local node = state.tree:get_node()
          local path = node and node.path
          for _, g in ipairs(tree.repos()) do
            if path and (path == g.repo or vim.startswith(path, g.repo .. '/')) then
              return diff_repo(g)
            end
          end
          PlatoonEditor.diff_pick()
        end,
      },
    },
  })
end)

-- ---- git: marks in files, diffs ------------------------------------------------------------------------------------

try('git marks', function()
  require('gitsigns').setup({
    signs = icons and {} or {
      add = { text = '+' },
      change = { text = '~' },
      delete = { text = '_' },
      topdelete = { text = '‾' },
      changedelete = { text = '~' },
      untracked = { text = '?' },
    },
    on_attach = function(buf)
      local gs = require('gitsigns')
      local function bmap(lhs, rhs, desc)
        vim.keymap.set('n', lhs, rhs, { buffer = buf, silent = true, desc = desc })
      end
      bmap(']h', function()
        gs.nav_hunk('next')
      end, 'Next change')
      bmap('[h', function()
        gs.nav_hunk('prev')
      end, 'Previous change')
      bmap('<leader>gs', gs.stage_hunk, 'Stage this change')
      bmap('<leader>gu', gs.reset_hunk, 'Undo this change')
      bmap('<leader>gp', gs.preview_hunk, 'Show this change')
      bmap('<leader>gb', function()
        gs.blame_line({ full = true })
      end, 'Who changed this line')
    end,
  })
end)

try('diffs', function()
  local close = { 'n', '<esc>', '<cmd>DiffviewClose<cr>', { desc = 'Close the diff' } }
  local back = { { 'n', 'i', 'x' }, BACK, PlatoonEditor.back, { desc = 'Back to platoon' } }
  local off = function(lhs)
    return { 'n', lhs, false }
  end
  local no_leader = { off('<leader>e'), off('<leader>b'), off('<leader>co'), off('<leader>ct'), off('<leader>cb'), off('<leader>ca'), off('<leader>cO'), off('<leader>cT'), off('<leader>cB'), off('<leader>cA') }
  require('diffview').setup({
    use_icons = icons,
    icons = icons and nil or { folder_closed = '+', folder_open = '-' },
    signs = icons and nil or { fold_closed = '>', fold_open = 'v', done = 'x' },
    enhanced_diff_hl = true,
    keymaps = {
      view = vim.list_extend({ close, back }, no_leader),
      file_panel = vim.list_extend({ close, back }, no_leader),
      file_history_panel = vim.list_extend({ close, back }, no_leader),
    },
  })
end)

-- ---- search, the terminal -------------------------------------------------------------------------------------------

try('search and the terminal', function()
  require('snacks').setup({
    picker = {
      enabled = true,
      ui_select = true,
      prompt = icons and ' ' or '> ',
      icons = icons and {} or {
        files = { enabled = false },
        git = { enabled = false },
        ui = { live = '* ', selected = '* ', unselected = '  ' },
        tree = { vertical = '| ', middle = '|-', last = '`-' },
      },
    },
    -- the shell script beside this file (the user's environment back, then their $SHELL), run as it is
    terminal = { enabled = true, shell = S.shell and { S.shell } or nil, win = { style = 'terminal' } },
    -- In the terminal's normal mode (two quick Esc), q goes back to platoon like everywhere else (not "hide"); the back
    -- key does from inside the terminal too.
    styles = {
      terminal = {
        keys = {
          q = { 'q', function() PlatoonEditor.back() end, desc = 'Back to platoon' },
          platoon_back = { BACK, function() PlatoonEditor.back() end, mode = { 'n', 't' }, desc = 'Back to platoon' },
        },
      },
    },
  })
end)

local function find_files()
  Snacks.picker.files({ dirs = root_paths(), hidden = true, title = 'Find a file' })
end
local function search_text()
  Snacks.picker.grep({ dirs = root_paths(), hidden = true, title = 'Search text' })
end
PlatoonEditor.terminal = function()
  local dir = here()
  Snacks.terminal.toggle(nil, { cwd = dir, win = { position = 'bottom', height = 0.35 } })
end

-- ---- key hints, status line, completion ----------------------------------------------------------------------------

try('key hints', function()
  require('which-key').setup({
    preset = 'modern',
    delay = 300,
    icons = icons and { mappings = true } or {
      mappings = false,
      rules = false,
      breadcrumb = '>',
      separator = '->',
      group = '+',
      ellipsis = '...',
      keys = {
        Up = 'up ', Down = 'down ', Left = 'left ', Right = 'right ', C = 'ctrl+', M = 'alt+', D = 'cmd+', S = 'shift+',
        CR = 'enter ', Esc = 'esc ', ScrollWheelDown = 'wheel down ', ScrollWheelUp = 'wheel up ', NL = 'enter ', BS = 'backspace ',
        Space = 'space ', Tab = 'tab ', F1 = 'F1', F2 = 'F2', F3 = 'F3', F4 = 'F4', F5 = 'F5', F6 = 'F6', F7 = 'F7', F8 = 'F8',
        F9 = 'F9', F10 = 'F10', F11 = 'F11', F12 = 'F12',
      },
    },
    spec = {
      { '<leader>', group = 'platoon' },
      { '<leader>g', group = 'This change (git)' },
    },
  })
end)

local function lualine_theme()
  local r = C.roles or {}
  local function sec(fg, bg, bold)
    -- (Terminal colors: the terminal's own text and background have no number; the nearest role stands in)
    return { fg = pick(r[fg]) or pick(r.subtext), bg = pick(r[bg]) or pick(r.surface), gui = bold and 'bold' or nil }
  end
  local function mode(color)
    return { a = sec('on_accent', color, true), b = sec('text', 'border'), c = sec('subtext', 'surface') }
  end
  return {
    normal = mode('accent'),
    insert = mode('ok'),
    visual = mode('busy'),
    replace = mode('err'),
    command = mode('warn'),
    terminal = mode('ok'),
    inactive = { a = sec('dim', 'surface'), b = sec('dim', 'surface'), c = sec('dim', 'surface') },
  }
end

try('the status line', function()
  require('lualine').setup({
    options = {
      theme = lualine_theme(),
      icons_enabled = icons,
      globalstatus = true,
      component_separators = icons and { left = '', right = '' } or { left = '|', right = '|' },
      section_separators = icons and { left = '', right = '' } or { left = '', right = '' },
      disabled_filetypes = { statusline = {} },
    },
    sections = {
      lualine_a = { 'mode' },
      lualine_b = {
        function()
          local _, r = here()
          return r and r.name or ''
        end,
      },
      lualine_c = {
        {
          'filename',
          path = 1,
          fmt = function(name)
            return vim.bo.buftype == 'terminal' and 'terminal' or name
          end,
        },
      },
      lualine_x = {
        function()
          return S.title or ''
        end,
      },
      lualine_y = { 'filetype' },
      lualine_z = { 'location' },
    },
  })
end)

try('completion', function()
  -- Ctrl+Space is platoon's back key, so the menu is opened with Ctrl+N (which then moves down it, as in Vim).
  local keymap = {
    preset = 'none',
    ['<C-e>'] = { 'cancel', 'fallback' },
    ['<CR>'] = { 'accept', 'fallback' },
    ['<Tab>'] = { 'select_next', 'fallback' },
    ['<S-Tab>'] = { 'select_prev', 'fallback' },
    ['<Down>'] = { 'select_next', 'fallback' },
    ['<Up>'] = { 'select_prev', 'fallback' },
    ['<C-n>'] = { 'show', 'select_next', 'fallback_to_mappings' },
    ['<C-p>'] = { 'select_prev', 'fallback_to_mappings' },
  }
  -- The back key wins over the menu's keys.
  for lhs in pairs(vim.deepcopy(keymap)) do
    if lhs ~= 'preset' and same_key(lhs, BACK) then
      keymap[lhs] = nil
    end
  end
  require('blink.cmp').setup({
    fuzzy = { implementation = 'lua' },
    cmdline = { enabled = false },
    sources = { default = { 'buffer', 'path' } },
    appearance = { nerd_font_variant = 'mono' },
    completion = {
      list = { selection = { preselect = false, auto_insert = false } },
      menu = icons and {} or { draw = { columns = { { 'label', 'label_description', gap = 1 }, { 'kind' } } } },
    },
    keymap = keymap,
  })
end)

-- ---- platoon's keys: space, then a letter ---------------------------------------------------------------------------

map('n', '<leader>q', PlatoonEditor.back, 'Back to platoon')
map('n', '<leader>e', function()
  neotree('platoon_files')
end, 'Files')
map('n', '<leader>c', function()
  neotree('platoon_changes')
end, 'Changed files')
map('n', '<leader>f', find_files, 'Find a file')
map('n', '<leader>s', search_text, 'Search text')
map('n', '<leader>b', function()
  Snacks.picker.buffers({ title = 'Open files' })
end, 'Open files')
map('n', '<leader>d', PlatoonEditor.diff, 'Diff: what changed')
map('n', '<leader>D', PlatoonEditor.diff_pick, 'Diff of another repository')
map('n', '<leader>h', '<cmd>DiffviewFileHistory %<cr>', 'History of this file')
map('n', '<leader>t', PlatoonEditor.terminal, 'Terminal')
map('n', '<leader>w', '<cmd>write<cr>', 'Save this file')
map('n', '<leader>?', function()
  require('which-key').show({ global = true })
end, 'All keys')
-- Ctrl+/ opens and hides the terminal, from a file or from inside the terminal (terminals send it as Ctrl+_).
for _, lhs in ipairs({ '<C-/>', '<C-_>' }) do
  map({ 'n', 't' }, lhs, PlatoonEditor.terminal, 'Terminal')
end

-- Colors again once everything has set its own (plugins define their groups when they start).
apply_colors()

-- ---- opening ---------------------------------------------------------------------------------------------------------

vim.api.nvim_create_autocmd('VimEnter', {
  once = true,
  callback = function()
    -- A ticket opens on what it changed; a session or a folder on its files.
    local start = 'platoon_files'
    for _, r in ipairs(roots) do
      if r.base then
        start = 'platoon_changes'
        break
      end
    end
    pcall(neotree, start, 'show')
  end,
})
