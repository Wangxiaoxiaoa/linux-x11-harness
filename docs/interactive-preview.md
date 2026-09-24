# Interactive Preview (Expando Window)

Double-clicking a preview cell opens an interactive expanded window that
forwards mouse and keyboard input into the harness display.

## UX flow

```
Default:  preview panel = read-only grid (unchanged)

Double-click a cell
  -> open a WM-managed toplevel showing that display's live capture
  -> mouse clicks / motion / wheel + keyboard are forwarded (XTEST)

Exit: Esc key, WM close button, or window destroy
  -> expando closes; panel keeps running as before
```

- Only one expando at a time. Double-clicking the same cell toggles it
  closed; double-clicking another cell replaces the open expando.
- The small thumbnail keeps rendering in the panel (layout is untouched).
- The expando is a normal toplevel (not override-redirect): the WM decorates
  it (title bar + close button) and gives it focus on click, which is exactly
  what keyboard forwarding needs.

## Geometry

- Height = 50% of the user's screen height (width capped at 50% screen width),
  aspect ratio preserved; docked to the top-left corner (the panel docks
  right). Example: 800x600 display on 2560x1600 -> 1067x800 window.
- Content fills the whole client area; input mapping is
  `display = window / scale` over that area.

## Event forwarding

| User event | Forwarded action |
|---|---|
| ButtonPress 1/2/3 | coords / scale -> warp + press/release (click, drag support) |
| Button 4/5 (wheel) | inject wheel buttons 4/5 at the mapped position |
| MotionNotify | move_mouse (hover feedback; with a button held = drag) |
| KeyPress/KeyRelease | user keycode+state -> keysym -> target keycode (level-1 adds Shift) |
| Escape | close the expando |

Keyboard translation keeps a per-press map
`user_keycode -> (target_keycode, shift_injected)` so a release is translated
with the same shift decision made at press time (the user's shift state may
change between press and release).

## Architecture

- New module `lxh/preview/src/expando.rs`; `scale()` moves out of `cell.rs`
  into a shared helper.
- `PreviewCell` reports `CellEvent::Expanded { display_id }` on double-click;
  the panel owns at most one `ExpandoHandle` and tears it down on exit.
- XTEST injection: the blocking core of `lxh-action` is exposed as public
  `click_blocking` / `move_blocking` / `key_blocking` helpers; the async
  driver methods wrap them. `lxh-preview` depends on `lxh-action`
  (preview -> action -> core, no cycle); the expando thread uses the
  blocking helpers directly, no tokio runtime.
- No daemon/MCP changes: the whole feature lives inside the preview crate.
