# Agent Conventions

Rules for AI agents (and humans) modifying this repository. These encode
decisions made across real debugging sessions; violating them has
repeatedly produced bugs or wasted work.

## Code style

- **No defensive programming.** No fallback branches, no `.unwrap_or`
  masking impossible states, no catch-all error handling. If a state
  cannot occur, don't write code for it. Every `unwrap_or` in the code
  base answers "what is the correct value when this is absent" (e.g.
  `DISPLAY` defaults to `:0`), never "hide the failure".
- **Zero `unsafe`, zero `#[allow]`.** `unsafe` is forbidden crate-wide.
  Lint suppressions mean the code should be fixed, not silenced.
- **Minimal footprint.** No speculative features, no extra knobs, no
  "while we're here" refactors. MVP scope only.
- **High cohesion, low coupling.** Each crate owns one concern; module
  boundaries follow the dependency arrows in `docs/ARCHITECTURE.md`.

## Process rules

- **Ask before fixing a bug:** has this bug ever occurred in real use?
  If not, don't fix it — record it and move on. Hypothetical bugs get
  hypothetical fixes, and those become dead code.
- **Discuss architecture before implementing.** Major design changes are
  written up and agreed in conversation first; the design document is
  only committed after the approach is approved.
- **Never run unverified code against the user's session.** Guessing is
  not allowed: state the hypothesis, get confirmation, then run.
- **Tests before features.** The suite must pass before new
  functionality lands.
- **Environment-interaction code is verified manually.** X server
  timing, window appearance, process lifecycle — automated tests for
  these become adversarial against the environment. Mark them
  `#[ignore]` with a reason, run them explicitly when needed.

## Complexity checkpoint

When fixing one bug exposes the next one, stop. Stack it and simplify
instead of piling workarounds — that signal means the design, not the
code, is wrong.

## Known environment interactions

- **`dde-clipboard-daemon` (Deepin) races with the desktop clipboard
  spoke** in `Bidirectional` mode. Killing it made tests stable. Not yet
  resolved; do not add workarounds for it until it bites in real use.
- **SIGTERM first, SIGKILL never** for X servers: X cleans up its socket
  and lock file on SIGTERM; SIGKILL leaves stale files that make the
  next Xvfb on the same number fail silently.
- **`clean_x_leftovers()`** in integration tests exists because SIGKILL
  debris (456 lock files in one session) breaks fresh Xvfb spawns.
- **x11rb computes TCP ports as `6000 + display` in u16** — display
  numbers must stay ≤ 59535 on both the create path (bounded by
  `pid % 59400`) and the attach path (validated).
