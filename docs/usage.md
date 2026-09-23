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

Displays created on this machine automatically open a read-only preview window on your desktop, scaled to a fraction of your screen (aspect ratio preserved). The preview is rendered by the daemon and never touches the display's lifecycle.

- Double-click the preview to zoom it; double-click again to restore.
- Closing the preview window (or calling `lxh_preview_close`) does not affect the display.
- Reopen it later with `lxh_preview_open`.
- The preview requires a running desktop session (`DISPLAY`). On headless hosts no preview is created and everything else works as usual.

## Example MCP session

```json
{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lxh_display_create","arguments":{}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lxh_app_launch","arguments":{"display_id":"d-99","command":"xterm","args":[]}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"lxh_capture_screenshot","arguments":{"display_id":"d-99"}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"lxh_display_destroy","arguments":{"display_id":"d-99"}}}
```

## Tool categories

| Category | Tools |
|---|---|
| Display | `lxh_display_create` `lxh_display_destroy` `lxh_display_attach` `lxh_display_detach` `lxh_display_info` |
| Apps | `lxh_app_launch` `lxh_app_terminate` `lxh_list_apps` |
| Input | `lxh_input_click` `lxh_input_move` `lxh_input_type` `lxh_input_key` `lxh_input_scroll` `lxh_input_drag` `lxh_input_get_cursor_position` |
| Capture | `lxh_capture_screenshot` `lxh_capture_window` `lxh_zoom` |
| State | `lxh_get_desktop_overview` `lxh_get_window_state` |
| AT-SPI | `lxh_set_value` `lxh_click_element` `lxh_invoke_menu` `lxh_verify_state` |
| Window | `lxh_window_focus` `lxh_window_set_frame` `lxh_window_close` |
| Clipboard | `lxh_clipboard_get` `lxh_clipboard_set` |
| Preview | `lxh_preview_open` `lxh_preview_close` |
| Wait | `lxh_wait` |

Notable parameters:

- `lxh_input_click` / `lxh_input_move` / `lxh_input_drag` accept `from_zoom: true` to interpret coordinates in the last `lxh_zoom` image instead of display coordinates.
- `lxh_input_type` accepts an optional `pid` to enable AT-SPI text writing when the focused widget is not editable.
- `lxh_click_element` / `lxh_set_value` accept an optional `identity` string (from `lxh_get_window_state` elements) for drift-proof targeting.
- `lxh_get_desktop_overview` accepts `pid` and `on_screen_only` filters.

## Run tests

```bash
cargo test -- --test-threads=1
```

Integration tests live in `lxh/daemon/tests/integration.rs`. They exercise the daemon end-to-end and must run sequentially because each test allocates X11 displays.
