---
name: linux-x11-harness
description: Run and automate Linux GUI applications in isolated X11 displays.
license: MIT
compatibility: Linux with xvfb and openbox installed.
---

# linux-x11-harness

Use this skill when you need to control Linux GUI applications without touching the user's real desktop.

## When to use

- Launch a GUI app in a clean, isolated X11 display.
- Take screenshots to verify UI state.
- Send mouse, keyboard, or clipboard input to a GUI app.
- Read the AT-SPI accessibility tree or click UI elements programmatically.
- Discover installed/running apps and invoke their menus by path.
- Assert UI state after actions without reading screenshots manually.

## Decision flow for any GUI app request

Before touching any GUI app, check whether it already runs on the user's
default display (`:0`) with `lxh_list_user_windows` (optionally filtered
by `name`):

- **App already open on the user's desktop** → do NOT create a display.
  Focus it with `wmctrl -a <title>` and interact via shell.
- **App NOT running** → use this skill: `lxh_display_create` +
  `lxh_app_launch`. NEVER launch GUI apps with shell commands on the
  user's desktop — a fresh GUI app request always goes through a harness
  display, so the agent gets a live preview and safe automation surface.
- **User says "automate", "test", "in a sandbox", "don't touch my
  desktop", or the task needs repeated programmatic interaction** →
  always use this skill, even if the app happens to run on `:0`.

Shell commands are for non-GUI work (files, processes, curl) and for
focusing existing windows — never for launching GUI apps.

## How to call the tools

The `lxh_*` tools are served by a background daemon. Depending on your
agent's capability you use one of these transports:

- **MCP agents**: the server is registered as `linux-x11-harness`; call the
  tools directly as MCP tools.
- **Agents without MCP support** (skill only): call the daemon socket from
  your shell with the bundled helper next to this file:

  ```bash
  python3 <this-skill-directory>/lxh.py lxh_display_create '{"persistent": true}'
  python3 <this-skill-directory>/lxh.py lxh_app_launch \
      '{"display_id": "d-...", "command": "xterm"}'
  ```

  The helper speaks JSON-RPC over the Unix socket (default
  `$XDG_RUNTIME_DIR/linux-x11-harness.sock`, override with `LXH_SOCKET_PATH`)
  and prints the tool result as JSON. Never start your own Xvfb — always go
  through the daemon, or you lose isolation and the preview.

## Lifecycle

Always follow this order:

1. Create a display with `lxh_display_create`.
2. Find the app to launch with `lxh_list_apps` (or use a known path).
3. Launch the target app with `lxh_app_launch`.
4. Wait briefly if the app needs time to start.
5. Interact: screenshot, click, type, read window state, etc.
6. Verify the result with `lxh_verify_state` or `lxh_get_window_state`.
7. Terminate the app with `lxh_app_terminate`.
8. Destroy the display with `lxh_display_destroy`.

Displays are headless Xvfb servers. A read-only preview window opens by default
so the user can watch what the agent is doing. Closing it does not affect the
display.

## Key tools

### Display lifecycle

- `lxh_display_create` — create an isolated display. Returns `display_id` and `display`.
- `lxh_display_destroy` — destroy a display and everything inside it.
- `lxh_display_info` — get resolution and app count.
- `lxh_preview_open` — open (or reopen) the preview window for a display.
- `lxh_preview_close` — close the preview window without touching the display.

### Apps

- `lxh_app_launch` — launch a command inside a display. Returns `pid`.
- `lxh_app_terminate` — kill an app by `pid`.
- `lxh_list_apps` — list running and installed apps. Returns `launch_path` ready for `lxh_app_launch`, so you can start apps by name without knowing the binary path.

### Input

- `lxh_input_click` — click at `(x, y)`. Accepts `from_zoom: true` to interpret coordinates in the last `lxh_zoom` image.
- `lxh_input_move` — move the cursor. Accepts `from_zoom: true`.
- `lxh_input_type` — type text. Accepts optional `pid` for AT-SPI write fallback (works even when no editable widget has keyboard focus).
- `lxh_input_key` — press a key, with optional modifiers.
- `lxh_input_scroll` — scroll.
- `lxh_input_drag` — drag from `(x1, y1)` to `(x2, y2)`. Accepts `from_zoom: true`.

### State and capture

- `lxh_capture_screenshot` — full display screenshot as base64 PNG.
- `lxh_capture_window` — single window screenshot.
- `lxh_zoom` — capture a cropped, scaled region of a window. Returns `display_origin` and `scale`; combine with `from_zoom: true` on input tools to act on what you see.
- `lxh_get_desktop_overview` — list processes and windows (with `window_id`, `pid`, `title`, `bounds`, `z_index`, `on_screen`). Supports `pid` and `on_screen_only` filters.
- `lxh_get_window_state` — detailed window info plus optional AT-SPI tree (with `actionable`, `value`, `checked`, `enabled`, `selected` per element) and screenshot.
- `lxh_input_get_cursor_position` — current mouse position.

### Accessibility

- `lxh_click_element` — activate an AT-SPI element by `pid` and `index`. Accepts optional `identity` (from a previous `lxh_get_window_state`) for drift-proof targeting. Left-click uses AT-SPI doAction (focus-free, works on background windows); passive elements return `suspected_noop`.
- `lxh_set_value` — set the value of an editable AT-SPI element. Accepts optional `identity`.
- `lxh_invoke_menu` — invoke an application menu item by path (e.g. `["File", "Open"]`). Fails with the exact unmatched segment.
- `lxh_verify_state` — assert window (exists, title_contains) and element (role, label_contains) state predicates. Returns per-predicate outcomes. Use after actions to verify UI changes without reading screenshots.

## Common workflow

```text
lxh_display_create
  -> lxh_list_apps (find the app)
  -> lxh_app_launch
  -> lxh_wait
  -> lxh_capture_screenshot
  -> lxh_get_window_state (find elements)
  -> lxh_click_element / lxh_set_value / lxh_invoke_menu
  -> lxh_verify_state (assert the result)
  -> lxh_app_terminate
  -> lxh_display_destroy
```

## Example scenarios

### Text editor end-to-end
```text
lxh_display_create -> lxh_app_launch("gedit")
  -> lxh_input_type("Hello World") -> lxh_input_key("ctrl+s")
  -> lxh_verify_state(window.title_contains="Hello World")
  -> lxh_app_terminate -> lxh_display_destroy
```

### Browser automation
```text
lxh_display_create -> lxh_app_launch("chromium --no-first-run https://example.com")
  -> lxh_wait(3000) -> lxh_capture_window
  -> lxh_get_window_state(include_tree=true) -- find links/buttons
  -> lxh_click_element(element_index) -> lxh_wait(2000)
  -> lxh_capture_window (verify navigation)
  -> lxh_display_destroy
```

### Hover to discover unknown UI
```text
lxh_display_create -> lxh_app_launch(app)
  -> lxh_capture_window -- see the toolbar
  -> lxh_hover(x=toolbar_icon_x, y=toolbar_icon_y) -- read tooltip
  -> decide: lxh_click_element or lxh_input_click
  -> lxh_verify_state -- confirm the action
  -> lxh_display_destroy
```

### Cross-app clipboard transfer
```text
lxh_display_create -> lxh_app_launch("xterm -e 'ls -la > /tmp/out.txt'")
  -> lxh_wait(2000) -> lxh_app_launch("gedit /tmp/out.txt")
  -> lxh_get_window_state(include_tree=true) -- read file content
  -> lxh_clipboard_set(content) -> lxh_app_launch("other_app")
  -> lxh_input_key("ctrl+v") -> lxh_display_destroy
```

## Tips

- Always destroy the display when done to free resources.
- Click inside a window before typing if it needs focus.
- Use `lxh_list_apps` to discover installed apps and get their launch paths.
- Use `lxh_get_desktop_overview` to find window IDs and pids.
- Use `lxh_get_window_state` with `include_tree: true` to inspect UI elements. Each element carries `actionable` (whether it can be activated), `value`, `checked`, `enabled`, `selected` and `identity`.
- Use `lxh_verify_state` after actions instead of reading screenshots — it returns structured satisfied/unsatisfied per predicate.
- Use `lxh_zoom` to inspect fine UI detail, then `from_zoom: true` on input tools to act on what you see.
- Prefer `lxh_click_element` with `identity` over coordinate clicks when the tree might change between observation and action.
