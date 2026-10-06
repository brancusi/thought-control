-- Real-terminal E2E for thc in WezTerm (writing.md §7 R-tests). Started by
-- scripts/e2e/wezterm.sh with its own config file, class and scratch vault: it never reads
-- ~/.wezterm.lua or touches a real vault. Keys go through WezTerm's own key encoder
-- (`SendKey`), with the kitty keyboard protocol on, as in thc's WezTerm setup.
local wezterm = require 'wezterm'
local act = wezterm.action
local config = wezterm.config_builder()

config.enable_kitty_keyboard = true
config.term = 'xterm-256color'
config.window_close_confirmation = 'NeverPrompt'
config.initial_cols = 100
config.initial_rows = 30
config.default_prog = { os.getenv('THC_BIN'), 'j', '--no-focus' }
config.set_environment_variables = {
  HOME = os.getenv('E2E_HOME'),
  THC_VAULT = os.getenv('E2E_VAULT'),
  THC_CACHE_DIR = os.getenv('E2E_CACHE'),
  THC_TEST = '1',
  THC_TEST_ROOT = os.getenv('E2E_ROOT'),
  THC_NO_UPDATE_CHECK = '1',
}

-- Keys as {key, mods}: what a person types (the shifted character with SHIFT, as macOS
-- delivers a real key press).
local keys = {
  { 'H', 'SHIFT' }, { 'i', '' }, { ' ', '' }, { 'X', 'SHIFT' }, { '?', 'SHIFT' },
  { 'Enter', 'SHIFT' }, { 'o', '' }, { 'k', '' },
}

wezterm.on('gui-startup', function(cmd)
  local _, pane, window = wezterm.mux.spawn_window(cmd or {})
  wezterm.time.call_after(3, function()
    local gw = window:gui_window()
    for _, k in ipairs(keys) do
      gw:perform_action(act.SendKey { key = k[1], mods = k[2] }, pane)
    end
    wezterm.time.call_after(3, function()
      local f = io.open(os.getenv('E2E_OUT'), 'w')
      f:write(pane:get_lines_as_text(30))
      f:close()
      gw:perform_action(act.SendKey { key = 'c', mods = 'CTRL' }, pane)
      gw:perform_action(act.SendKey { key = 'c', mods = 'CTRL' }, pane)
      wezterm.time.call_after(2, function()
        gw:perform_action(act.QuitApplication, pane)
      end)
    end)
  end)
end)

return config
