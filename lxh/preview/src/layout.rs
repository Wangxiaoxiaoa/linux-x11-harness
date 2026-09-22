//! Grid and pagination layout for the preview panel.
//!
//! Pure math over [`ScreenMetrics`]: given how many cells exist and which
//! page is shown, produces every visible cell's position and the container
//! size. No X11, no cells, no container — trivially unit-testable.

use crate::geometry::ScreenMetrics;

/// Maximum grid: 2 columns x 4 rows (user-confirmed cap).
pub const MAX_COLS: usize = 2;
pub const MAX_ROWS: usize = 4;
/// Cells per page: 2 x 4.
pub const PAGE_CAPACITY: usize = MAX_COLS * MAX_ROWS;

/// Where one visible cell sits inside the container, relative to the
/// container's content origin (top-left inside the margin).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSlot {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The full geometry of the panel for a given state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelLayout {
    /// Container size (grid only; the hover title strip and pager bar are
    /// added on top by the caller via `hover` / `has_pages`).
    pub width: u32,
    pub height: u32,
    /// Container top-left on the user's screen.
    pub x: i32,
    pub y: i32,
    /// Extra height added while the pager bar is shown.
    pub pager_extra: u32,
}

/// How many cells are visible on `page` when `total` cells exist.
pub fn visible_count(total: usize, page: usize) -> usize {
    if total == 0 {
        return 0;
    }
    let pages = total_pages(total);
    let page = page.min(pages - 1);
    let start = page * PAGE_CAPACITY;
    (total - start).min(PAGE_CAPACITY)
}

/// Total pages needed for `total` cells.
pub fn total_pages(total: usize) -> usize {
    total.div_ceil(PAGE_CAPACITY).max(1)
}

/// Positions of the cells on the current page, in page order.
///
/// `sizes[i]` is the fitted content size of the i-th visible cell; each cell
/// is placed top-left inside its grid slot.
pub fn cell_slots(metrics: &ScreenMetrics, visible: usize, sizes: &[(u32, u32)]) -> Vec<CellSlot> {
    let mut slots = Vec::with_capacity(visible);
    for i in 0..visible {
        let col = i % MAX_COLS;
        let row = i / MAX_COLS;
        let x = metrics.margin + col as u32 * (metrics.cell_w + metrics.gap);
        let y = metrics.margin + row as u32 * (metrics.cell_h + metrics.gap);
        let (w, h) = sizes
            .get(i)
            .copied()
            .unwrap_or((metrics.cell_w, metrics.cell_h));
        slots.push(CellSlot { x, y, w, h });
    }
    slots
}

/// Container geometry for the current state.
///
/// `visible` cells are on the current page; the grid adapts to them (the user
/// requirement: one preview -> a one-cell container, up to 2x4). The pager
/// bar adds height only when more than one page exists.
pub fn panel_layout(metrics: &ScreenMetrics, visible: usize, pages: usize) -> PanelLayout {
    let visible = visible.min(PAGE_CAPACITY);
    let cols = visible.clamp(1, MAX_COLS);
    let rows = visible.div_ceil(MAX_COLS).clamp(1, MAX_ROWS);

    let cols = cols as u32;
    let rows = rows as u32;
    let grid_w = 2 * metrics.margin + cols * metrics.cell_w + (cols - 1) * metrics.gap;
    let grid_h = 2 * metrics.margin + rows * metrics.cell_h + (rows - 1) * metrics.gap;

    let pager_extra = if pages > 1 { metrics.pager_h } else { 0 };

    // Dock to the right screen edge.
    let x = (metrics.screen_w as i64 - grid_w as i64 - metrics.dock_margin_x as i64).max(0) as i32;
    let y = metrics.dock_top as i32;

    PanelLayout {
        width: grid_w,
        height: grid_h + pager_extra,
        x,
        y,
        pager_extra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> ScreenMetrics {
        // 2560x1600 -> cells 320x200, gap/margin 12, strip 32.
        ScreenMetrics::from_screen(2560, 1600)
    }

    #[test]
    fn container_grows_with_cell_count() {
        let m = metrics();
        let p1 = panel_layout(&m, 1, 1);
        let p2 = panel_layout(&m, 2, 1);
        let p3 = panel_layout(&m, 3, 1);
        // One cell: exactly the cell plus margins.
        assert_eq!(p1.width, 2 * m.margin + m.cell_w);
        assert_eq!(p1.height, 2 * m.margin + m.cell_h);
        // Two cells: two columns.
        assert_eq!(p2.width, 2 * m.margin + 2 * m.cell_w + m.gap);
        assert_eq!(p2.height, p1.height);
        // Three cells: still two columns, one more row.
        assert_eq!(p3.height, 2 * m.margin + 2 * m.cell_h + m.gap);
    }

    #[test]
    fn container_caps_at_2x4() {
        let m = metrics();
        let full = panel_layout(&m, 8, 1);
        // Page 2 shows 4 leftover cells; the grid never exceeds 2 columns.
        let page2 = panel_layout(&m, 4, 2);
        assert_eq!(page2.width, full.width);
        // 4 cells = 2 rows of grid, plus the pager bar.
        let expected = 2 * m.margin + 2 * m.cell_h + m.gap + m.pager_h;
        assert_eq!(page2.height, expected);
    }

    #[test]
    fn slots_tile_row_major() {
        let m = metrics();
        let slots = cell_slots(&m, 3, &[(320, 200); 3]);
        assert_eq!(slots.len(), 3);
        // Row-major: 0 and 1 share a row, 2 starts the next row.
        assert_eq!(slots[0].y, slots[1].y);
        assert!(slots[2].y > slots[0].y);
        assert_eq!(slots[1].x - slots[0].x, m.cell_w + m.gap);
    }

    #[test]
    fn removal_refills_from_front() {
        // Page 0 of 3 cells shows all; removing the first leaves 2 cells that
        // move up into slots 0 and 1 — positions come from the new count, so
        // the caller just re-applies layout.
        let m = metrics();
        let three = cell_slots(&m, 3, &[(320, 200); 3]);
        let two = cell_slots(&m, 2, &[(320, 200); 2]);
        assert_eq!(two[0], three[0]);
        assert_eq!(two[1], three[1]);
    }

    #[test]
    fn pagination_counts() {
        assert_eq!(total_pages(1), 1);
        assert_eq!(total_pages(8), 1);
        assert_eq!(total_pages(9), 2);
        assert_eq!(total_pages(17), 3);
        // Page 1 of 9 cells shows the single leftover cell.
        assert_eq!(visible_count(9, 1), 1);
        // Out-of-range page clamps to the last page.
        assert_eq!(visible_count(9, 5), 1);
    }
}
