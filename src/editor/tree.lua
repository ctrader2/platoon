-- platoon's two views in the editor's side panel (neo-tree sources), written next to init.lua by platoon each time it
-- opens the editor (compiled into platoon from src/editor/tree.lua; changes here are overwritten).
--
--   Files          every folder of the ticket or session as a top-level entry, each with git's marks
--   Changed files  what each repository changed: for a ticket since its worktree was made (committed and not yet
--                  committed), for a session what is not yet committed; one group per repository
--
-- neo-tree's own "filesystem" source shows one folder per window, so these are sources of platoon's own built from
-- neo-tree's parts (items, renderer, git status per worktree, the common commands).

local M = {}

local cc = require('neo-tree.sources.common.commands')
local common_components = require('neo-tree.sources.common.components')
local file_items = require('neo-tree.sources.common.file-items')
local renderer = require('neo-tree.ui.renderer')
local manager = require('neo-tree.sources.manager')
local events = require('neo-tree.events')
local git = require('neo-tree.git')
local utils = require('neo-tree.utils')

--- The roots, bases and names platoon passed (set by init.lua before neo-tree starts).
M.session = { roots = {} }

--- git's idea of each repository's base: { [repository root] = commit }, for the marks of committed changes.
local function bases()
  local out = {}
  for _, r in ipairs(M.session.roots) do
    if r.repo and r.base then
      out[r.repo] = r.base
    end
  end
  return out
end

local renderers = {
  directory = {
    { 'indent' },
    { 'icon' },
    {
      'container',
      content = {
        { 'name', zindex = 10 },
        { 'clipboard', zindex = 10 },
        { 'git_status', zindex = 10, align = 'right', hide_when_expanded = true },
      },
    },
  },
  file = {
    { 'indent' },
    { 'icon' },
    {
      'container',
      content = {
        { 'name', zindex = 10 },
        { 'clipboard', zindex = 10 },
        { 'modified', zindex = 20, align = 'right' },
        { 'git_status', zindex = 10, align = 'right' },
      },
    },
  },
  message = {
    { 'indent', with_markers = false },
    { 'name', highlight = 'NeoTreeMessage' },
  },
}

-- ---- Files ----------------------------------------------------------------------------------------------------------

local files = {
  name = 'platoon_files',
  display_name = ' Files ',
  components = common_components,
  default_config = { renderers = renderers, window = { mappings = {} } },
}

--- The entries of folder `dir` (an item in `context`), one level: `.git` left out, the rest as neo-tree items.
local function scan(context, dir)
  local fd = vim.uv.fs_scandir(dir.path)
  while fd do
    local name, typ = vim.uv.fs_scandir_next(fd)
    if not name then
      break
    end
    if name ~= '.git' then
      -- (a file that went between the listing and the look is left out)
      pcall(file_items.create_item, context, dir.path .. '/' .. name, typ ~= 'link' and typ or nil)
    end
  end
  dir.loaded = true
  file_items.advanced_sort(dir.children, context.state)
end

--- Folders the user opened are opened again when the tree is built again (a refresh, a switch back to Files).
local function reopen(context, dir, open)
  for _, child in ipairs(dir.children or {}) do
    if child.type == 'directory' and open[child.path] then
      scan(context, child)
      reopen(context, child, open)
    end
  end
end

files.navigate = function(state, path, path_to_reveal, callback)
  state.dirty = false
  state.platoon_open = state.platoon_open or {}
  state.git_base_by_worktree = bases()
  local roots = M.session.roots
  state.path = roots[1] and roots[1].path or vim.fn.getcwd()
  local nodes, expanded = {}, {}
  for _, r in ipairs(roots) do
    -- each root in a context of its own, so a root inside another stays a root of its own
    local context = file_items.create_context()
    context.state = state
    local ok, root = pcall(file_items.create_item, context, r.path, 'directory')
    if ok and root then
      root.name = r.name
      root.extra = { platoon_root = r }
      context.folders[root.path] = root
      scan(context, root)
      reopen(context, root, state.platoon_open)
      table.insert(nodes, root)
      table.insert(expanded, root.id)
      for p, _ in pairs(context.folders) do
        if state.platoon_open[p] then
          table.insert(expanded, p)
        end
      end
      -- git's marks: asked here and now (neo-tree keeps the answer per repository, and the marks read it)
      pcall(git.status, r.path, state.git_base_by_worktree, false, { untracked_files = 'all' })
    end
  end
  if #nodes == 0 then
    nodes = { { id = 'platoon-none', name = 'None of the folders is there any more.', type = 'message' } }
  end
  state.default_expanded_nodes = expanded
  if path_to_reveal then
    renderer.position.set(state, path_to_reveal)
  end
  renderer.show_nodes(nodes, state)
  if type(callback) == 'function' then
    vim.schedule(callback)
  end
end

--- Open or close a folder; its entries are read the first time it opens.
files.toggle_directory = function(state, node)
  node = node or state.tree:get_node()
  if not node or node.type ~= 'directory' then
    return
  end
  state.platoon_open = state.platoon_open or {}
  if node.loaded == false then
    local context = file_items.create_context()
    context.state = state
    local dir = { id = node:get_id(), path = node.path, type = 'directory', children = {} }
    context.folders[dir.path] = dir
    scan(context, dir)
    node.loaded = true
    state.platoon_open[node.path] = true
    renderer.show_nodes(dir.children, state, node:get_id())
  elseif node:is_expanded() then
    node:collapse()
    state.platoon_open[node.path] = nil
    renderer.redraw(state)
  else
    node:expand()
    state.platoon_open[node.path] = true
    renderer.redraw(state)
  end
end

files.refresh = function()
  manager.refresh(files.name)
end

-- ---- Changed files --------------------------------------------------------------------------------------------------

local changes = {
  name = 'platoon_changes',
  display_name = ' Changed files ',
  components = common_components,
  default_config = { renderers = renderers, window = { mappings = {} } },
}

--- One entry per repository among the roots (two folders of one repository are one group), in the roots' order.
function M.repos()
  local out, seen = {}, {}
  for _, r in ipairs(M.session.roots) do
    if r.repo and not seen[r.repo] then
      seen[r.repo] = true
      table.insert(out, { repo = r.repo, base = r.base, name = r.repo_name or r.name })
    end
  end
  return out
end

changes.navigate = function(state, path, path_to_reveal, callback)
  state.dirty = false
  state.git_base_by_worktree = bases()
  local repos = M.repos()
  state.path = repos[1] and repos[1].repo or (M.session.roots[1] and M.session.roots[1].path) or vim.fn.getcwd()
  local nodes, expanded = {}, {}
  for _, g in ipairs(repos) do
    local context = file_items.create_context()
    context.state = state
    local lookup, top, over_base = git.status(g.repo, state.git_base_by_worktree, false, { untracked_files = 'all' })
    -- (when git's answer is the same as last time neo-tree hands back only the first part: the rest it kept)
    local known = top and git.worktrees[top]
    if not over_base and g.base and known and known.status_diff then
      over_base = known.status_diff[g.base]
    end
    local root = file_items.create_item(context, top or g.repo, 'directory')
    root.loaded = true
    root.extra = { platoon_repo = g }
    context.folders[root.path] = root
    local count = 0
    for _, sl in ipairs({ lookup or {}, over_base or {} }) do
      for p, status in pairs(sl) do
        if type(status) ~= 'table' and status ~= '!' and not context.item_exists[p] and p ~= root.path then
          local ok, item = pcall(file_items.create_item, context, p)
          if ok and item then
            if item.type == 'unknown' then
              item.type = 'file'
            end
            if item.type ~= 'directory' then
              count = count + 1
            end
            item.extra = { git_status = status }
          end
        end
      end
    end
    local since = g.base and ('since ' .. g.base:sub(1, 7)) or 'not committed'
    if count == 0 then
      root.name = ('%s · nothing changed (%s)'):format(g.name, since)
    else
      root.name = ('%s · %d changed (%s)'):format(g.name, count, since)
    end
    for id, _ in pairs(context.folders) do
      table.insert(expanded, id)
    end
    file_items.advanced_sort(root.children, state)
    table.insert(nodes, root)
  end
  if #nodes == 0 then
    nodes = { { id = 'platoon-none', name = 'None of these folders is in git, so there are no changes to show.', type = 'message' } }
  end
  state.default_expanded_nodes = expanded
  renderer.show_nodes(nodes, state)
  if type(callback) == 'function' then
    vim.schedule(callback)
  end
end

changes.refresh = function()
  manager.refresh(changes.name)
end

-- ---- commands (the same for both views) ----------------------------------------------------------------------------

local function commands_for(source)
  local refresh = utils.wrap(manager.refresh, source.name)
  local redraw = utils.wrap(manager.redraw, source.name)
  local toggle = function(state)
    return function(node)
      if source.toggle_directory then
        source.toggle_directory(state, node)
      else
        cc.toggle_node(state)
      end
    end
  end
  local c = {}
  c.open = function(state)
    cc.open(state, toggle(state))
  end
  c.open_split = function(state)
    cc.open_split(state, toggle(state))
  end
  c.open_vsplit = function(state)
    cc.open_vsplit(state, toggle(state))
  end
  c.open_tabnew = function(state)
    cc.open_tabnew(state, toggle(state))
  end
  c.toggle_node = function(state)
    cc.toggle_node(state, toggle(state))
  end
  c.close_node = function(state)
    local node = state.tree:get_node()
    if node and node.type == 'directory' and node:is_expanded() then
      if state.platoon_open then
        state.platoon_open[node.path] = nil
      end
    end
    cc.close_node(state, redraw)
  end
  c.refresh = refresh
  c.add = function(state)
    cc.add(state, refresh)
  end
  c.add_directory = function(state)
    cc.add_directory(state, refresh)
  end
  c.delete = function(state)
    cc.delete(state, refresh)
  end
  c.delete_visual = function(state, selected)
    cc.delete_visual(state, selected, refresh)
  end
  c.rename = function(state)
    cc.rename(state, refresh)
  end
  c.copy = function(state)
    cc.copy(state, refresh)
  end
  c.move = function(state)
    cc.move(state, refresh)
  end
  c.copy_to_clipboard = function(state)
    cc.copy_to_clipboard(state, redraw)
  end
  c.cut_to_clipboard = function(state)
    cc.cut_to_clipboard(state, redraw)
  end
  c.paste_from_clipboard = function(state)
    cc.paste_from_clipboard(state, refresh)
  end
  cc._add_common_commands(c)
  return c
end

files.commands = commands_for(files)
changes.commands = commands_for(changes)

-- ---- keeping them current -------------------------------------------------------------------------------------------

local function subscribe(source)
  -- git's answer for a worktree came in: redraw the marks
  manager.subscribe(source.name, {
    event = events.GIT_STATUS_CHANGED,
    handler = function()
      manager.redraw(source.name)
    end,
  })
  manager.subscribe(source.name, {
    event = events.VIM_BUFFER_CHANGED,
    handler = function(args)
      if utils.is_real_file(args.afile) then
        manager.refresh(source.name)
      end
    end,
  })
  manager.subscribe(source.name, {
    event = events.VIM_BUFFER_MODIFIED_SET,
    handler = utils.wrap(manager.opened_buffers_changed, source.name),
  })
  manager.subscribe(source.name, {
    event = events.GIT_EVENT,
    handler = function()
      manager.refresh(source.name)
    end,
  })
end

files.setup = function()
  subscribe(files)
end
changes.setup = function()
  subscribe(changes)
end

M.files = files
M.changes = changes

--- Register both with Lua's module loader under the names neo-tree is given.
function M.register()
  package.loaded['platoon_files'] = files
  package.loaded['platoon_changes'] = changes
end

return M
