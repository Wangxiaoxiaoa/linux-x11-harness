# VNC Streaming (feat/vnc-streaming branch)

Replaces the self-built interactive expando window with x11vnc per
display. The user opens the sandbox in any VNC client instead of the
custom expando window.

## Why

The self-built expando (512 lines) hand-implements what VNC servers
provide out of the box: frame streaming, keyboard translation,
pointer forwarding, clipboard extension. The VNC path removes ~500
lines of protocol code in exchange for one system dependency
(x11vnc) and per-display child-process lifecycle management.

## Design

### Launch

`lxh_display_create` spawns, alongside Xvfb and openbox:

```bash
x11vnc -display :<N> -rfbport <5900+K> -nopw -shared -forever
```

- `-shared`: multiple clients allowed
- `-forever`: keep serving after client disconnect
- `-nopw`: local-only MVP; the sandbox X server itself is disposable
- Port: 5900+K where K avoids collisions (checked against listening
  ports; the hub records the assignment per display)

### User access

Any VNC client: `vncviewer localhost:5900+K`. The preview panel's
double-click action opens the system VNC client (`xdg-open` style
launch) instead of the expando window.

### Clipboard

x11vnc syncs the VNC client clipboard with the sandbox display
natively (CutText extension). The desktop spoke (:0 -> hub) stays:
desktop-to-sandbox clipboard flow still goes through the hub. The
sandbox->desktop direction for VNC users is handled by x11vnc; for
agents it goes through lxh_clipboard_set as before.

### Policy

The `clipboard_sync` parameter is dropped (x11vnc owns the sandbox
side of the clipboard). The desktop spoke keeps its origin filter
(sandbox-origin updates never overwrite the user's desktop clipboard
unless the user pastes them deliberately in a desktop app via the
agent).

## Scope

- Delete: `lxh/preview/src/expando.rs` (512 lines), the panel's
  expando wiring, the SelectionClear/SelectionRequest serving in the
  sandbox spoke (x11vnc serves the sandbox clipboard now).
- Keep: the desktop spoke (origin filter, hub -> :0 read path), the
  read-only preview panel, all MCP tools.
- Add: x11vnc lifecycle in the daemon (spawn on display create, kill
  on destroy), port allocation, double-click -> launch VNC client.
- CI: add x11vnc to the apt package list.

## Rejected alternatives

- noVNC (web canvas): no extra client install, but needs websockify
  plus a browser embedding point in the panel — significantly more
  moving parts for MVP.
- Self-hosted RFB protocol in Rust: no mature crate; reimplementing
  the RFB handshake/encoding is exactly the complexity we are removing.
