//! Coordinate math (pure, unit-tested on Linux).
//!
//! Frame convention (inferred, see README): `x`/`y` passed by the model are
//! window-relative pixels in the *screenshot image* the helper returned. We keep
//! a per-capture scale factor so that downscaled screenshots map back to
//! physical screen pixels. The process runs Per-Monitor-Aware V2, so every Win32
//! rect we read is already in physical pixels and no extra DPI conversion is needed.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Rect { left, top, right, bottom }
    }
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub fn is_empty(&self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect {
            left: self.left.max(o.left),
            top: self.top.max(o.top),
            right: self.right.min(o.right),
            bottom: self.bottom.min(o.bottom),
        }
    }
    pub fn area(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            self.width() as i64 * self.height() as i64
        }
    }
    pub fn center(&self) -> (i32, i32) {
        (self.left + self.width() / 2, self.top + self.height() / 2)
    }
}

/// Target image size for a capture of `w`x`h` given a maximum long edge
/// (0 = no limit). Returns (width, height, scale) where scale = physical/image.
pub fn fit_size(w: u32, h: u32, max_edge: u32) -> (u32, u32, f64) {
    if w == 0 || h == 0 {
        return (w, h, 1.0);
    }
    let long = w.max(h);
    if max_edge == 0 || long <= max_edge {
        return (w, h, 1.0);
    }
    let scale = long as f64 / max_edge as f64;
    let nw = ((w as f64 / scale).round() as u32).max(1);
    let nh = ((h as f64 / scale).round() as u32).max(1);
    (nw, nh, scale)
}

/// Crop of a window rect relative to the captured monitor's texture.
/// Errors when nothing of the window is on that monitor.
pub fn crop_in_monitor(window: &Rect, monitor: &Rect) -> Result<Rect, String> {
    let inter = window.intersect(monitor);
    if inter.is_empty() {
        return Err("window crop falls outside the captured monitor".into());
    }
    Ok(Rect::new(
        inter.left - monitor.left,
        inter.top - monitor.top,
        inter.right - monitor.left,
        inter.bottom - monitor.top,
    ))
}

/// Captured geometry remembered for a screenshot id.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CaptureGeometry {
    /// Physical screen rect of the main window at capture time.
    pub window: Rect,
    /// Physical pixels per screenshot-image pixel (1.0 if no downscale).
    pub scale: f64,
    /// Image size returned to the caller.
    pub image_w: u32,
    pub image_h: u32,
}

/// Convert a screenshot-relative (x, y) into a physical screen point.
/// Returns Err when the point falls outside the (possibly downscaled) image.
pub fn screenshot_to_screen(geo: &CaptureGeometry, x: f64, y: f64) -> Result<(i32, i32), String> {
    if !x.is_finite() || !y.is_finite() {
        return Err("coordinates have to be finite numbers".into());
    }
    if x < 0.0 || y < 0.0 || x >= geo.image_w as f64 || y >= geo.image_h as f64 {
        return Err(format!(
            "point ({x}, {y}) is outside the screenshot viewport {{ originX: 0, originY: 0, width: {}, height: {} }}",
            geo.image_w, geo.image_h
        ));
    }
    let sx = (geo.window.left as f64 + x * geo.scale).round() as i32;
    let sy = (geo.window.top as f64 + y * geo.scale).round() as i32;
    Ok((sx, sy))
}

/// Tolerance (physical pixels) when verifying that a live window rect still
/// matches the one used for a prior screenshot. Verified: the original rejects
/// "window bounds changed before coordinate input".
pub const BOUNDS_SLACK_PX: i32 = 2;

pub fn bounds_match(a: &Rect, b: &Rect, slack: i32) -> bool {
    (a.left - b.left).abs() <= slack
        && (a.top - b.top).abs() <= slack
        && (a.right - b.right).abs() <= slack
        && (a.bottom - b.bottom).abs() <= slack
}

/// Pick the monitor that covers the largest area of `window`.
pub fn dominant_monitor<'a>(window: &Rect, monitors: &'a [Rect]) -> Option<&'a Rect> {
    monitors.iter().max_by_key(|m| window.intersect(m).area())
}

/// Absolute SendInput mouse position (0..=65535 over the virtual desktop).
pub fn absolute_mouse(screen_x: i32, screen_y: i32, virtual_desk: &Rect) -> (u32, u32) {
    let w = virtual_desk.width().max(1) as f64;
    let h = virtual_desk.height().max(1) as f64;
    let ax = ((screen_x - virtual_desk.left) as f64 + 0.5) * 65535.0 / w;
    let ay = ((screen_y - virtual_desk.top) as f64 + 0.5) * 65535.0 / h;
    (ax.round().clamp(0.0, 65535.0) as u32, ay.round().clamp(0.0, 65535.0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_size_no_scale() {
        assert_eq!(fit_size(800, 600, 1600), (800, 600, 1.0));
        assert_eq!(fit_size(0, 0, 100), (0, 0, 1.0));
    }

    #[test]
    fn fit_size_downscales() {
        let (w, h, s) = fit_size(1920, 1080, 960);
        assert_eq!((w, h), (960, 540));
        assert!((s - 2.0).abs() < 1e-9);
    }

    #[test]
    fn crop_ok_and_out() {
        let mon = Rect::new(0, 0, 1920, 1080);
        let win = Rect::new(100, 100, 500, 400);
        assert_eq!(crop_in_monitor(&win, &mon).unwrap(), Rect::new(100, 100, 500, 400));
        let off = Rect::new(2000, 0, 2500, 500);
        assert!(crop_in_monitor(&off, &mon).is_err());
    }

    #[test]
    fn screenshot_to_screen_scaled() {
        let geo = CaptureGeometry {
            window: Rect::new(100, 200, 1100, 800),
            scale: 2.0,
            image_w: 500,
            image_h: 300,
        };
        assert_eq!(screenshot_to_screen(&geo, 10.0, 20.0).unwrap(), (120, 240));
        assert!(screenshot_to_screen(&geo, 500.0, 0.0).is_err());
        assert!(screenshot_to_screen(&geo, -1.0, 0.0).is_err());
    }

    #[test]
    fn bounds_slack() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(1, -1, 101, 99);
        assert!(bounds_match(&a, &b, 2));
        assert!(!bounds_match(&a, &Rect::new(5, 0, 100, 100), 2));
    }

    #[test]
    fn dominant() {
        let win = Rect::new(1800, 100, 2200, 500);
        let mons = [Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 3840, 1080)];
        assert_eq!(dominant_monitor(&win, &mons).unwrap(), &mons[1]);
    }

    #[test]
    fn absolute_mouse_corners() {
        let desk = Rect::new(0, 0, 1920, 1080);
        let (ax, ay) = absolute_mouse(0, 0, &desk);
        assert!(ax < 100 && ay < 100);
        let (ax, ay) = absolute_mouse(1919, 1079, &desk);
        assert!(ax > 65000 && ay > 65000);
    }
}
