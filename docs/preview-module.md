# Preview Panel Module Design

## Goal

Rework the preview feature into a loosely-coupled panel module: all
previews live inside a single override-redirect container window on the
user's desktop, laid out by an orchestration layer. Cell and container
sizes are derived from the user's screen resolution; when a display
disappears the remaining cells re-layout automatically; previews beyond
one page are paginated instead of filling the screen.

## Hard requirements (user-confirmed)

1. Preview cells and the orchestration logic live in **separate files**.
2. The container shrinks/grows with the cell count: one preview means a
   one-cell container; the grid caps at **2 columns x 4 rows**.
3. More than 8 cells (2x4) are paginated; a page bar (`◀ 1/2 ▶`) appears
   at the bottom only while more than one page exists.
4. When a display's X server dies, its cell is removed automatically and
   the remaining cells re-layout; the container shrinks with them.
5. Every size (cell, gap, margin, container) is derived from the screen
   resolution — no hard-coded pixel values.
6. Cells show a hover title bar (title + close button) that overlays only
   their own top strip; neighbouring cells are never covered.
7. No `unsafe` code (x11rb end to end).

## Module layout (one responsibility per file)

```
lxh/preview/src/
├── lib.rs        Public facade: pub use PreviewPanel
├── geometry.rs   Pure math: ScreenMetrics (cell/gap/margin derived from
│                 the screen resolution), aspect-ratio fitting. No X11.
├── layout.rs     Pure math: grid and pagination. Inputs (cell count,
│                 page, metrics) -> visible cell positions and container
│                 size. No X11.
├── container.rs  Container window: OR window creation, pager bar
│                 painting and clicks, always-on-top restacking.
│                 Contains no layout math.
├── cell.rs       One preview cell: render loop, hover title bar + close,
│                 display-disconnect detection. Knows nothing about the
│                 panel, the container's layout, or other cells.
└── panel.rs      Orchestration: owns the container and all cells,
                  handles open/close, consumes events, applies layout
                  from layout.rs. The only file with global knowledge.
└── x11.rs        Small X11 helpers shared by the above.
```

### Coupling rules

- `geometry.rs` / `layout.rs`: pure functions and data, zero X11.
- `cell.rs`: talks to the panel only through an
  `mpsc::Sender<CellEvent>` (`Closed` / `DisplayGone`) and a
  `PreviewCellHandle` (set_rect / set_mapped / is_alive / stop).
- `container.rs`: paints only itself (pager bar) and reports
  `ContainerEvent` (PagePrev / PageNext).
- `panel.rs`: the only file that references both cells and the
  container; all geometry decisions come from `layout.rs`.

## Layout algorithm (layout.rs)

- Grid: 2 columns; rows = ceil(visible / 2), capped at 4.
- Page capacity = 8; total pages = ceil(cells / 8).
- Container width = 2*margin + cols*cell_w + (cols-1)*gap
- Container height = 2*margin + rows*cell_h + (rows-1)*gap
  + (pages > 1 ? pager_h : 0)
- The panel docks to the right screen edge (margins from metrics).

## Size algorithm (geometry.rs)

- cell_w = screen_w / 8, cell_h = screen_h / 8
- gap = cell_h / 16, clamped to 8..=16 (12 on a 2560x1600 screen)
- margin = gap
- strip_h = cell_h / 6, clamped to 28..=32 (cell hover title bar)
- pager_h = strip_h
- Each cell's rendered size = fit_inside(target resolution, cell box),
  preserving the target display's aspect ratio.

All derived in `ScreenMetrics::from_screen(screen_w, screen_h)`.

## Events and threading

```
cell threads xN ──CellEvent──────┐
container thread ─ContainerEvent─┤→ panel event loop → re-layout
daemon threads ──open()/close()──┘   (single mutex)
```

- A cell's `GetImage` failing at the connection level (as opposed to
  protocol errors such as BadMatch) means the display is gone: the cell
  reports `DisplayGone` and exits.
- The panel removes the cell and re-lays out on `DisplayGone`/`Closed`.
- Cells not on the current page are unmapped and skip rendering.

## Daemon integration

- `PreviewManager` became `PreviewPanel`; `open(display_id, display,
  title)` and `close(display_id)` signatures are unchanged — the daemon
  only updated the type name.
- Display destroy and session cleanup keep calling `close`.
