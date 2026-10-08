# Clipboard Sharing Between the User Desktop and Harness Displays

## Problem

Harness displays are separate X servers: their CLIPBOARD selections are
independent of the user's desktop (`:0`). Today `lxh_clipboard_get/set`
operate on whatever X display the daemon process inherited (`$DISPLAY`,
normally `:0`) via arboard, regardless of the `display_id` argument — an
accidental bridge that works but is undocumented, fragile, and half of
what sharing requires (sandbox-side clipboards are unreachable).

## Goal

A shared clipboard with **direction policies**, so sandbox activity can
never silently overwrite the user's desktop clipboard:

```
clipboard_sync: "off" | "to_sandbox" (default) | "bidirectional"
```

| Policy | desktop → sandbox | sandbox → desktop |
|---|---|---|
| `to_sandbox` (default) | automatic | only via explicit `lxh_clipboard_set` |
| `bidirectional` | automatic | automatic (VM-style; opt-in) |
| `off` | never | never |

Rationale: sandbox-side changes (agent-driven Ctrl+C inside web pages,
apps) must not clobber a clipboard the user is actively using. Desktop →
sandbox inflow is read-only from the user's point of view and is the
high-frequency need (paste desktop content into the sandbox, by agent or
by the user working in an interactive expando window).

## Architecture: canonical hub + spokes

The daemon holds the canonical clipboard value in memory. Every display —
the user's desktop and each harness display — syncs against it:

```
              canonical { text, fingerprint, version }   (daemon memory)
               ↑ arboard persistent instance  ↑ x11rb selection owner
        :0 user bridge                 sandbox spoke (per display)
        (poll 500ms + push)            (SelectionClear events + poll 1s)
```

- **User bridge** (one thread): holds a persistent `arboard::Clipboard`
  on `:0` (it serves selection requests while alive). Polls `:0` every
  500 ms; on change updates canonical and bumps the version. Writes
  canonical changes to `:0` via arboard `set_text`.
- **Sandbox spoke** (one thread per display with sync != off): x11rb
  connection to the harness display; a never-mapped owner window.
  - Push: on canonical version change, `SetSelectionOwner` and serve
    `SelectionRequest` (TARGETS, UTF8_STRING, STRING).
  - Detect: `SelectionClear` (an app took ownership — near-instant) plus
    a 1 s fingerprint poll as a safety net.
  - Policy gate: local changes are only forwarded to canonical under
    `bidirectional`.
- **Hub**: `Mutex<ClipboardState>` + `Condvar` so spokes wake instantly
  on version bumps instead of pure polling.

### Loop prevention

A spoke only acts when *its local fingerprint* differs from canonical;
after propagation both are equal, so the cycle terminates. The user
bridge is symmetrical (it only acts when `:0`'s fingerprint differs).
Concurrent changes on both sides: canonical wins ties by version; last
propagation wins, deterministic enough for text.

### `lxh_clipboard_get/set` semantics after the refactor

- `set(text)`: hub writes canonical + arboard-set on `:0` + notifies
  spokes → user's desktop clipboard updated (explicit act) and sandbox
  apps can paste immediately.
- `get()`: fresh arboard read of `:0` (the user's clipboard, always
  current) — unchanged observable behavior for agents.
- The `display_id` argument stays in the schema for interface stability
  but is documented as ignored: there is exactly one shared clipboard.

## Scope limits (documented, deliberate)

- Text only (UTF8_STRING / STRING); images and the INCR protocol are out
  of scope. Content larger than 1 MiB is truncated with a log line.
- `CLIPBOARD` selection only; `PRIMARY` is untouched.
- Latency: desktop → sandbox ~10-50 ms (SelectionClear-driven on the
  sandbox side happens when the source is another spoke; `:0` side is
  poll-driven at 500 ms), sandbox-local copies detected near-instantly
  via `SelectionClear`.
- Rust SDK's `Display::driver().clipboard_*` keeps the old environment-
  clipboard semantics (documented); the daemon no longer routes through
  it.

## Lifecycle

- Spoke threads start in `create_display` when policy != off and stop on
  `destroy_display` / session cleanup / daemon shutdown via a stop
  channel (thread closes its window and exits).
- Headless daemon (no `:0`): the user bridge does not start; sandbox
  spokes still run — canonical changes then come only from
  `lxh_clipboard_set`, and sandboxes share among themselves.
- The `:0` bridge starts lazily on the first display that needs it.

## Files

- `lxh/daemon/src/clipboard.rs` (new): hub, user bridge, sandbox spoke
  (~280 lines).
- `lxh/daemon/src/handlers.rs`: create/destroy/cleanup wiring;
  `clipboard_get/set` rerouted through the hub (~30 lines).
- `lxh/daemon/src/tools.rs`: `DisplayCreateArgs.clipboard_sync`; updated
  descriptions for the three affected tools (~20 lines).
- Docs: `usage.md` section, `ARCHITECTURE.md` row updates.

Total ~330 lines plus tests.

## Tests

- Unit: fingerprint stability; policy parsing; hub version monotonicity.
- Integration (DISPLAY=:0 available): create display with default
  policy; set `:0` clipboard via `xclip -i`; assert the harness display
  serves the same content (`xclip -selection clipboard -o` with
  `DISPLAY=<harness>`); bidirectional display: copy inside the harness
  via `xclip -i`, assert `:0` reflects it; `off` display: assert no
  propagation.
- Manual: expando paste flow.
