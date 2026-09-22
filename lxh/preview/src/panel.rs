//! Panel orchestration: owns the container and every preview cell, applies
//! the layout from [`crate::layout`], and reacts to events (cell closed,
//! display gone, pagination).
//!
//! This is the only file that knows about both cells and the container; the
//! public API mirrors what the daemon needs: `open`, `close`.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use lxh_core::LxhError;
use x11rb::rust_connection::RustConnection;

use crate::cell::{CellChrome, CellEvent, CellRect, PreviewCell};
use crate::container::{ContainerEvent, ContainerState, PreviewContainer};
use crate::geometry::ScreenMetrics;
use crate::layout::{
    cell_slots, panel_layout, total_pages, visible_count, CellSlot, PAGE_CAPACITY,
};
use crate::x11::{connect_user, root_size};

/// How often the panel event loop polls the cell/container channels.
const EVENT_POLL: Duration = Duration::from_millis(20);

/// Panel title shown as the container's WM_NAME.
const PANEL_TITLE: &str = "LXH Previews";

struct CellEntry {
    display_id: String,
    cell: PreviewCell,
    /// Fitted content size for the cell (from the target display's
    /// resolution), used by the layout.
    size: (u32, u32),
}

struct PanelInner {
    metrics: Option<ScreenMetrics>,
    container: Option<PreviewContainer>,
    cells: Vec<CellEntry>,
    page: usize,
    /// Set when the container died (user display gone); stops re-layout.
    broken: bool,
}

/// Public facade. The daemon holds one instance and calls `open`/`close`.
pub struct PreviewPanel {
    inner: Arc<Mutex<PanelInner>>,
    cell_tx: mpsc::Sender<CellEvent>,
    container_tx: mpsc::Sender<ContainerEvent>,
}

impl Default for PreviewPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewPanel {
    pub fn new() -> Self {
        let inner = Arc::new(Mutex::new(PanelInner {
            metrics: None,
            container: None,
            cells: Vec::new(),
            page: 0,
            broken: false,
        }));
        let (cell_tx, cell_rx) = mpsc::channel::<CellEvent>();
        let (container_tx, container_rx) = mpsc::channel::<ContainerEvent>();

        // Background loop: consume cell/container events and re-layout. Cells
        // and the container report through two channels; both are polled here
        // (all preview paths are polling-based already).
        let inner2 = Arc::clone(&inner);
        thread::spawn(move || loop {
            let mut dirty = false;
            {
                let mut guard = inner2.lock().unwrap();
                while let Ok(event) = cell_rx.try_recv() {
                    handle_cell_event(&mut guard, event);
                    dirty = true;
                }
                while let Ok(event) = container_rx.try_recv() {
                    handle_container_event(&mut guard, event);
                    dirty = true;
                }
                if dirty {
                    reflow(&mut guard);
                }
            }
            // The panel lives as long as the daemon; polling is fine.
            thread::sleep(EVENT_POLL);
        });

        Self {
            inner,
            cell_tx,
            container_tx,
        }
    }

    /// Show a preview for `display_id`; creates the container on first use.
    pub fn open(
        &self,
        display_id: &str,
        target_display: &str,
        title: &str,
    ) -> Result<(), LxhError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.broken {
            return Err(LxhError::DisplayUnavailable(
                "preview panel is unavailable (user display closed)".into(),
            ));
        }

        // Replace an existing preview for the same display.
        if let Some(pos) = inner.cells.iter().position(|c| c.display_id == display_id) {
            let entry = inner.cells.remove(pos);
            entry.cell.stop();
            reflow(&mut inner);
        }

        let metrics = match inner.metrics {
            Some(m) => m,
            None => {
                let m = screen_metrics()?;
                inner.metrics = Some(m);
                m
            }
        };

        // Query the target resolution to fit it into a cell.
        let size = target_size(target_display)?;
        let fitted = metrics.fit_cell(size.0, size.1);

        let total = inner.cells.len() + 1;
        let pages = total_pages(total);
        let page = pages - 1; // show the page that receives the new cell
        let layout = panel_layout(&metrics, visible_count(total, page), pages);
        let chrome = CellChrome {
            strip_h: metrics.strip_h as u16,
            btn_w: metrics.strip_h as u16,
        };

        if inner.container.is_none() {
            let container = PreviewContainer::start(
                PANEL_TITLE,
                ContainerState {
                    pager_visible: pages > 1,
                    page: page as u32 + 1,
                    pages: pages as u32,
                    pager_h: metrics.pager_h as u16,
                    x: layout.x,
                    y: layout.y,
                    width: layout.width,
                    height: layout.height,
                },
                self.container_tx.clone(),
            )?;
            inner.container = Some(container);
        }

        // Slot of the new cell: last position on the receiving page.
        let page_start = page * PAGE_CAPACITY;
        let mut sizes: Vec<(u32, u32)> = inner.cells[page_start..].iter().map(|e| e.size).collect();
        sizes.push(fitted);
        let CellSlot { x, y, w, h } = cell_slots(&metrics, sizes.len(), &sizes)[sizes.len() - 1];
        let rect = CellRect {
            x: x as i32,
            y: y as i32,
            width: w,
            height: h,
        };

        let container_win = inner
            .container
            .as_ref()
            .expect("container exists")
            .window_id();
        // Stable short id so cells stay identifiable across re-layouts and
        // pagination (the agent knows the same id from `display_id`).
        let short_id: String = display_id
            .trim_start_matches("d-")
            .chars()
            .take(6)
            .collect();
        let cell_title = format!("{title} ({short_id})");
        let cell = PreviewCell::start(
            container_win,
            display_id.to_string(),
            target_display,
            &cell_title,
            rect,
            chrome,
            self.cell_tx.clone(),
        )?;

        inner.cells.push(CellEntry {
            display_id: display_id.to_string(),
            cell,
            size: fitted,
        });
        inner.page = page;
        reflow(&mut inner);
        Ok(())
    }

    /// Close the preview for `display_id`, if open.
    pub fn close(&self, display_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(pos) = inner.cells.iter().position(|c| c.display_id == display_id) {
            let entry = inner.cells.remove(pos);
            entry.cell.stop();
            reflow(&mut inner);
        }
    }
}

impl Drop for PreviewPanel {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        for entry in inner.cells.drain(..) {
            entry.cell.stop();
        }
        if let Some(container) = inner.container.take() {
            container.stop();
        }
    }
}

fn handle_cell_event(inner: &mut PanelInner, event: CellEvent) {
    let display_id = match &event {
        CellEvent::Closed { display_id } | CellEvent::DisplayGone { display_id } => display_id,
    };
    if let Some(pos) = inner.cells.iter().position(|c| c.display_id == *display_id) {
        let entry = inner.cells.remove(pos);
        entry.cell.stop();
    }
}

fn handle_container_event(inner: &mut PanelInner, event: ContainerEvent) {
    match event {
        ContainerEvent::PagePrev => inner.page = inner.page.saturating_sub(1),
        ContainerEvent::PageNext => {
            let pages = total_pages(inner.cells.len());
            inner.page = (inner.page + 1).min(pages - 1);
        }
    }
}

/// Recompute container geometry, chrome and every cell's rect/visibility.
fn reflow(inner: &mut PanelInner) {
    let container_alive = match inner.container.as_ref() {
        Some(c) => c.is_alive(),
        None => return,
    };
    if !container_alive {
        inner.broken = true;
        return;
    }
    let Some(metrics) = inner.metrics else {
        return;
    };

    // Drop entries whose threads already exited.
    inner.cells.retain(|entry| entry.cell.is_alive());

    let total = inner.cells.len();
    if total == 0 {
        // Nothing left: tear the container down entirely.
        if let Some(container) = inner.container.take() {
            container.stop();
        }
        inner.page = 0;
        return;
    }

    let pages = total_pages(total);
    inner.page = inner.page.min(pages - 1);
    let page = inner.page;
    let visible = visible_count(total, page);
    let layout = panel_layout(&metrics, visible, pages);

    let state = ContainerState {
        pager_visible: pages > 1,
        page: page as u32 + 1,
        pages: pages as u32,
        pager_h: metrics.pager_h as u16,
        x: layout.x,
        y: layout.y,
        width: layout.width,
        height: layout.height,
    };
    if let Some(container) = inner.container.as_ref() {
        container.set_state(state);
    }

    // Cells: position the ones on the current page, hide the rest.
    let page_start = page * PAGE_CAPACITY;
    let sizes: Vec<(u32, u32)> = inner.cells[page_start..page_start + visible]
        .iter()
        .map(|e| e.size)
        .collect();
    let slots = cell_slots(&metrics, visible, &sizes);
    for (i, entry) in inner.cells[page_start..page_start + visible]
        .iter_mut()
        .enumerate()
    {
        let slot = slots[i];
        entry.cell.set_rect(CellRect {
            x: slot.x as i32,
            y: slot.y as i32,
            width: slot.w,
            height: slot.h,
        });
        entry.cell.set_mapped(true);
    }
    for entry in inner
        .cells
        .iter_mut()
        .take(total)
        .skip(page_start + visible)
    {
        entry.cell.set_mapped(false);
    }
}

/// Screen metrics of the user's display.
fn screen_metrics() -> Result<ScreenMetrics, LxhError> {
    let (conn, screen_idx) = connect_user()?;
    let (w, h) = root_size(&conn, screen_idx);
    Ok(ScreenMetrics::from_screen(w, h))
}

/// Resolution of a harness display (its root window).
fn target_size(target_display: &str) -> Result<(u32, u32), LxhError> {
    let (conn, screen_idx) = RustConnection::connect(Some(target_display)).map_err(|e| {
        LxhError::DisplayUnavailable(format!("cannot connect to target display: {e}"))
    })?;
    Ok(root_size(&conn, screen_idx))
}
