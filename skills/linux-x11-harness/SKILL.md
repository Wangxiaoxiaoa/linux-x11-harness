---
name: linux-x11-harness
description: Run and automate Linux GUI applications in isolated X11 displays.
license: MIT
compatibility: Linux with xvfb and openbox installed.
---

# linux-x11-harness

Automate Linux GUI apps in isolated X11 displays (Xvfb + openbox) with a
live preview panel on the user's desktop — without touching their real
desktop.

## Decision flow for any GUI app request

Check whether the app already runs on the user's default display (`:0`)
with `lxh_list_user_windows` (optional `name` filter):

- **Already on `:0`** → focus it with `wmctrl -a <title>`; do NOT create
  a display.
- **Not running** → `lxh_display_create` + `lxh_app_launch`. NEVER launch
  GUI apps with shell commands on the user's desktop.
- **User says "automate", "test", "sandbox", "don't touch my desktop"** →
  always use this skill, even if the app runs on `:0`.

Shell is for non-GUI work and focusing existing windows — never for
launching GUI apps.

## Calling the tools

- **MCP agents**: the server is registered as `linux-x11-harness`; call
  `lxh_*` tools directly.
- **No MCP**: use the bundled helper next to this file:

  ```bash
  LXH_SOCKET_PATH=/tmp/lxh-opencode.sock \
    python3 <this-directory>/lxh.py lxh_display_create '{"persistent": true}'
  ```

  Never start your own Xvfb — always go through the daemon.

## Lifecycle

1. `lxh_display_create` — creates the display and the preview panel.
2. `lxh_app_launch` — start the app (returns pid).
3. `lxh_wait` / `lxh_get_desktop_overview` — wait for the window; the
   overview gives `window_id`, bounds and z-order (zombie windows are
   filtered).
4. Automate: `lxh_input_click/type/key`, `lxh_click_element` (AT-SPI
   doAction), `lxh_set_value`, `lxh_invoke_menu`, `lxh_verify_state`.
5. `lxh_capture_window` / `lxh_zoom` — screenshots (display-absolute
   coordinates; `window_id` identifies, not transforms).
6. `lxh_app_terminate` + `lxh_display_destroy` — clean up.

## Reading screen text without vision

If you cannot view images, or the app exposes no accessibility tree:

```bash
lxh_capture_window '{"display_id": "d-...", "window_id": 123,
                     "save_to": "/tmp/shot.png"}'
lxh_ocr '{"image_path": "/tmp/shot.png"}'
```

`save_to` writes the PNG and returns its path instead of base64 data;
`lxh_ocr` returns the recognized text (tesseract, else rapidocr).
`lxh_zoom` accepts the same `save_to` for small regions.

## Gotchas

- AT-SPI works for GTK/Qt/Electron apps; Chromium needs the a11y
  environment broadcast (the daemon sets it). Apps like WeChat expose
  nothing — use the OCR flow above.
- `lxh_click_element` and `lxh_set_value` accept an `identity` from a
  previous `lxh_get_window_state` for drift-proof targeting.
- `lxh_input_type` accepts an optional `pid` for an AT-SPI write fallback
  when the focused widget is not editable.
- `lxh_verify_state` is single-sample; call `lxh_wait` between attempts.

Full tool reference and JSON examples: `docs/usage.md` in the repository.
