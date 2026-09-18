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

## How to call the tools

The `lxh_*` tools are served by a background daemon. Depending on your agent's
capability you use one of these transports:

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
2. Launch the target app with `lxh_app_launch`.
3. Wait briefly if the app needs time to start.
4. Interact: screenshot, click, type, read window state, etc.
5. Terminate the app with `lxh_app_terminate`.
6. Destroy the display with `lxh_display_destroy`.

Displays are headless Xvfb servers. A read-only preview window opens by default
so the user can watch what the agent is doing. Closing it does not affect the
display. The user can double-click a preview to zoom it.

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

### Input

- `lxh_input_click` — click at `(x, y)`.
- `lxh_input_move` — move the cursor.
- `lxh_input_type` — type text.
- `lxh_input_key` — press a key, with optional modifiers.
- `lxh_input_scroll` — scroll.
- `lxh_input_drag` — drag from `(x1, y1)` to `(x2, y2)`.

### State and capture

- `lxh_capture_screenshot` — full display screenshot as base64 PNG.
- `lxh_get_desktop_overview` — list processes and windows.
- `lxh_get_window_state` — detailed window info plus optional AT-SPI tree and screenshot.
- `lxh_input_get_cursor_position` — current mouse position.

### Accessibility

- `lxh_click_element` — click an AT-SPI element by `pid` and `index`.
- `lxh_set_value` — set the value of an editable AT-SPI element.

## Common workflow

```text
lxh_display_create -> lxh_app_launch -> lxh_wait -> lxh_capture_screenshot -> ... -> lxh_app_terminate -> lxh_display_destroy
```

## Tips

- Always destroy the display when done to free resources.
- Click inside a window before typing if it needs focus.
- Use `lxh_get_desktop_overview` to find window IDs and pids.
- Use `lxh_get_window_state` with `include_tree: true` to inspect UI elements.
