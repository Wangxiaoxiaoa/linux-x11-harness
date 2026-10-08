# Usage

## Build

```bash
cargo build --release
```

The MCP server binary is `target/release/linux-x11-harness`.

## Run the MCP server

The MCP server is a long-lived daemon plus a per-agent stdio proxy.

```bash
./target/release/linux-x11-harness serve  # start daemon
./target/release/linux-x11-harness mcp    # stdio proxy (auto-starts daemon)
./target/release/linux-x11-harness status # check daemon
./target/release/linux-x11-harness stop   # stop daemon
```

## Multiple isolated instances

By default, all agents share one daemon and each MCP connection gets its own session. Displays created by one session are cleaned up when that session disconnects.

To run a fully separate daemon process, use a unique socket:

```bash
./target/release/linux-x11-harness serve --socket /tmp/lxh-agent-1.sock
./target/release/linux-x11-harness mcp --socket /tmp/lxh-agent-1.sock
./target/release/linux-x11-harness stop --socket /tmp/lxh-agent-1.sock
```

`--socket` overrides the default socket path (`$XDG_RUNTIME_DIR/linux-x11-harness.sock`, falling back to `/tmp/linux-x11-harness.sock`) and the `LXH_SOCKET_PATH` environment variable.

## Display preview

Displays created on this machine automatically open a live preview panel on your desktop, scaled to a fraction of your screen (aspect ratio preserved). The preview is rendered by the daemon and never touches the display's lifecycle.

- Previews are **read-only by default** — safe to keep on screen while the agent works.
- **Double-click a preview** to open an expanded interactive window (~50% screen height, docked top-left): your mouse clicks, motion, wheel and keyboard are forwarded into the sandboxed app via XTEST, and the view refreshes ~100ms. Exit with `Esc` or the window's close button; the small thumbnail keeps rendering in the panel.
- Only one expanded window exists at a time; double-clicking the same cell toggles it closed, another cell replaces it.
- Closing the preview panel (or calling `lxh_preview_close`) does not affect the display.
- Reopen it later with `lxh_preview_open`.
- The preview requires a running desktop session (`DISPLAY`). On headless hosts no preview is created and everything else works as usual.

## Routing: check the user's desktop first

For any GUI app request, call `lxh_list_user_windows` (optionally filtered by `name`) before creating a display: if the app already runs on the user's desktop (`:0`), focus it with `wmctrl -a` instead of creating a sandbox; if not, create one with `lxh_display_create` + `lxh_app_launch`.

## Reading screen text without vision

If the model cannot view images, or the app exposes no accessibility tree, capture with `save_to` and read with `lxh_ocr`:

```json
{"method":"tools/call","params":{"name":"lxh_capture_window","arguments":{"display_id":"d-1","window_id":123,"save_to":"/tmp/shot.png"}}}
{"method":"tools/call","params":{"name":"lxh_ocr","arguments":{"image_path":"/tmp/shot.png"}}}
```

`save_to` writes the PNG and returns its path instead of base64 data (also available on `lxh_zoom`). `lxh_ocr` probes engines in order: tesseract, then the python package `rapidocr_onnxruntime`.

## Example MCP session

```json
{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lxh_display_create","arguments":{}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lxh_app_launch","arguments":{"display_id":"d-99","command":"xterm","args":[]}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"lxh_get_desktop_overview","arguments":{"display_id":"d-99"}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"lxh_capture_window","arguments":{"display_id":"d-99","window_id":4194314,"save_to":"/tmp/shot.png"}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"lxh_ocr","arguments":{"image_path":"/tmp/shot.png"}}}
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"lxh_display_destroy","arguments":{"display_id":"d-99"}}}
```

## Tool categories

| Category | Tools |
|---|---|
| Display | `lxh_display_create` `lxh_display_destroy` `lxh_display_attach` `lxh_display_detach` `lxh_display_info` |
| Apps | `lxh_app_launch` `lxh_app_terminate` `lxh_list_apps` |
| User desktop | `lxh_list_user_windows` |
| Input | `lxh_input_click` `lxh_input_move` `lxh_input_type` `lxh_input_key` `lxh_input_scroll` `lxh_input_drag` `lxh_input_get_cursor_position` `lxh_hover` |
| Capture | `lxh_capture_window` `lxh_zoom` |
| OCR | `lxh_ocr` |
| State | `lxh_get_desktop_overview` `lxh_get_window_state` `lxh_verify_state` |
| AT-SPI | `lxh_set_value` `lxh_click_element` `lxh_invoke_menu` |
| Window | `lxh_window_focus` `lxh_window_set_frame` `lxh_window_close` |
| Clipboard | `lxh_clipboard_get` `lxh_clipboard_set` |
| Preview | `lxh_preview_open` `lxh_preview_close` |
| Wait | `lxh_wait` |

Notable parameters:

- `lxh_input_click` / `lxh_input_move` / `lxh_input_drag` accept `from_zoom: true` to interpret coordinates in the last `lxh_zoom` image instead of display coordinates.
- `lxh_input_type` accepts an optional `pid` to enable AT-SPI text writing when the focused widget is not editable.
- `lxh_click_element` / `lxh_set_value` accept an optional `identity` string (from `lxh_get_window_state` elements) for drift-proof targeting.
- `lxh_get_desktop_overview` accepts `pid` and `on_screen_only` filters.

## Example scenarios

### Desktop app functional test

```json
{"method":"tools/call","params":{"name":"lxh_display_create","arguments":{}}}
{"method":"tools/call","params":{"name":"lxh_app_launch","arguments":{"display_id":"d-1","command":"gedit"}}}
{"method":"tools/call","params":{"name":"lxh_input_type","arguments":{"display_id":"d-1","text":"Hello World"}}}
{"method":"tools/call","params":{"name":"lxh_input_key","arguments":{"display_id":"d-1","key":"ctrl+s"}}}
{"method":"tools/call","params":{"name":"lxh_verify_state","arguments":{"display_id":"d-1","pid":1234,"expect":[{"window":{"exists":true,"title_contains":"Hello World"}}]}}}
{"method":"tools/call","params":{"name":"lxh_display_destroy","arguments":{"display_id":"d-1"}}}
```

### Browser automation (Chromium/Electron)

The daemon advertises accessibility on the session bus at startup, so Chromium and Electron apps render full AT-SPI trees. Launch with a URL, wait for the page to load, then use `lxh_get_window_state` to read links, buttons, and form fields from the tree.

### Menu invocation

```json
{"method":"tools/call","params":{"name":"lxh_invoke_menu","arguments":{"display_id":"d-1","pid":1234,"path":["File","Export as PDF"]}}}
```

### UI exploration with hover

When the agent encounters an unfamiliar button or icon, `lxh_hover` moves the mouse there, waits for tooltips to appear, and returns a screenshot for the agent to read.

### Zoom into fine detail

`lxh_zoom` captures a cropped, scaled region of a window. The response includes `display_origin` and `scale`; pass `from_zoom: true` to input tools to translate coordinates from the zoom image back to display space.

## Run tests

```bash
cargo test -- --test-threads=1
```

Integration tests live in `lxh/daemon/tests/integration.rs`. They exercise the daemon end-to-end and must run sequentially because each test allocates X11 displays.
