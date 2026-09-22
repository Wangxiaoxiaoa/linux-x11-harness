//! Screen-adaptive size metrics for the preview panel.
//!
//! Every pixel value the panel uses is derived from the user's screen
//! resolution; nothing is hard-coded. Pure math, no X11.

/// All sizes the panel needs, derived from one screen resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenMetrics {
    pub screen_w: u32,
    pub screen_h: u32,
    /// Preview cell box: `screen/8` on each axis. The cell content is fitted
    /// inside this box while preserving the target display's aspect ratio.
    pub cell_w: u32,
    pub cell_h: u32,
    /// Spacing between cells and around the grid: `cell_h / 16`, clamped to 8..=16.
    pub gap: u32,
    pub margin: u32,
    /// Hover title strip height: `cell_h / 6`, clamped to 28..=32.
    pub strip_h: u32,
    /// Pagination bar height (same as the title strip).
    pub pager_h: u32,
    /// Distance from the right screen edge to the panel.
    pub dock_margin_x: u32,
    /// Distance from the top screen edge to the panel.
    pub dock_top: u32,
}

impl ScreenMetrics {
    /// Derive all panel sizes from the user's screen resolution.
    pub fn from_screen(screen_w: u32, screen_h: u32) -> Self {
        let cell_w = (screen_w / 8).max(160);
        let cell_h = (screen_h / 8).max(100);
        let gap = (cell_h / 16).clamp(8, 16);
        let strip_h = (cell_h / 6).clamp(28, 32);
        Self {
            screen_w,
            screen_h,
            cell_w,
            cell_h,
            gap,
            margin: gap,
            strip_h,
            pager_h: strip_h,
            dock_margin_x: gap,
            dock_top: strip_h * 2,
        }
    }

    /// Fit a target display resolution into the cell box, preserving its
    /// aspect ratio and never upscaling beyond the source.
    pub fn fit_cell(&self, target_w: u32, target_h: u32) -> (u32, u32) {
        fit_inside(target_w, target_h, self.cell_w, self.cell_h)
    }
}

/// Fit `(w, h)` inside `(max_w, max_h)` preserving the aspect ratio and
/// never upscaling.
pub fn fit_inside(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (1, 1);
    }
    if w <= max_w && h <= max_h {
        return (w, h);
    }
    let scale = f64::min(max_w as f64 / w as f64, max_h as f64 / h as f64);
    let nw = ((w as f64 * scale).floor() as u32).max(1);
    let nh = ((h as f64 * scale).floor() as u32).max(1);
    (nw.min(max_w), nh.min(max_h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_scale_with_screen() {
        let m = ScreenMetrics::from_screen(2560, 1600);
        assert_eq!((m.cell_w, m.cell_h), (320, 200));
        assert_eq!(m.gap, 12);
        assert_eq!(m.margin, 12);
        // 200/6 = 33 -> clamped to 32.
        assert_eq!(m.strip_h, 32);
        assert_eq!(m.dock_top, 64);

        // A 4K screen gets proportionally larger cells.
        let m4k = ScreenMetrics::from_screen(3840, 2160);
        assert_eq!((m4k.cell_w, m4k.cell_h), (480, 270));
        assert!(m4k.cell_w > m.cell_w);
    }

    #[test]
    fn metrics_have_floors_for_tiny_screens() {
        let m = ScreenMetrics::from_screen(640, 400);
        assert_eq!((m.cell_w, m.cell_h), (160, 100));
        assert_eq!(m.gap, 8);
        assert_eq!(m.strip_h, 28);
    }

    #[test]
    fn fit_inside_preserves_aspect_and_caps() {
        let m = ScreenMetrics::from_screen(2560, 1600);
        // 1280x800 into 320x200 keeps the 1.6 ratio.
        assert_eq!(m.fit_cell(1280, 800), (320, 200));
        // Limited by height.
        assert_eq!(m.fit_cell(1280, 1600), (160, 200));
        // Never upscales beyond the source.
        assert_eq!(m.fit_cell(100, 50), (100, 50));
    }
}
