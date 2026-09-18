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

## Run tests

```bash
cargo test -- --test-threads=1
```

Integration tests live in `lxh/daemon/tests/integration.rs`. They exercise the daemon end-to-end and must run sequentially because each test allocates X11 displays.
