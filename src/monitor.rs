//! Single-monitor scope: overlap math and scope parsing (unit-tested on Linux).

use crate::geometry::Rect;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    pub index: u32,
    pub device_name: String,
    pub friendly_name: Option<String>,
    pub rect: Rect,
    pub work_area: Rect,
    pub dpi: u32,
    pub scale: f64,
    pub primary: bool,
}

impl DisplayInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "index": self.index,
            "deviceName": self.device_name,
            "friendlyName": self.friendly_name,
            "rect": {
                "left": self.rect.left, "top": self.rect.top,
                "right": self.rect.right, "bottom": self.rect.bottom,
                "width": self.rect.width(), "height": self.rect.height(),
            },
            "workArea": {
                "left": self.work_area.left, "top": self.work_area.top,
                "right": self.work_area.right, "bottom": self.work_area.bottom,
                "width": self.work_area.width(), "height": self.work_area.height(),
            },
            "dpi": self.dpi,
            "scale": self.scale,
            "primary": self.primary,
        })
    }
}

/// Intersection area of two rects (0 if none).
pub fn intersection_area(a: &Rect, b: &Rect) -> i64 {
    let left = a.left.max(b.left);
    let top = a.top.max(b.top);
    let right = a.right.min(b.right);
    let bottom = a.bottom.min(b.bottom);
    let w = (right - left).max(0) as i64;
    let h = (bottom - top).max(0) as i64;
    w * h
}

/// True when >50% of `window` area lies on `monitor`.
pub fn window_mostly_on_monitor(window: &Rect, monitor: &Rect) -> bool {
    let wa = (window.width().max(0) as i64) * (window.height().max(0) as i64);
    if wa <= 0 {
        return false;
    }
    let inter = intersection_area(window, monitor);
    inter * 2 > wa
}

/// Point must lie inside the monitor rect (inclusive left/top, exclusive right/bottom
/// is fine for pixel coords — we use inclusive on all edges for "inside").
pub fn point_in_monitor(x: i32, y: i32, monitor: &Rect) -> bool {
    x >= monitor.left && x < monitor.right && y >= monitor.top && y < monitor.bottom
}

pub fn out_of_scope_msg(what: &str) -> String {
    format!("out_of_scope: {what}")
}

/// Parse OCU_MONITOR / scope_monitor value: index, device name, or "primary".
#[derive(Debug, Clone, PartialEq)]
pub enum MonitorSelector {
    Primary,
    Index(u32),
    DeviceName(String),
}

pub fn parse_monitor_selector(s: &str) -> Result<MonitorSelector, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err("monitor selector must be non-empty".into());
    }
    if t.eq_ignore_ascii_case("primary") {
        return Ok(MonitorSelector::Primary);
    }
    if let Ok(n) = t.parse::<u32>() {
        return Ok(MonitorSelector::Index(n));
    }
    Ok(MonitorSelector::DeviceName(t.to_string()))
}

pub fn default_selector_from_env() -> Option<MonitorSelector> {
    std::env::var("OCU_MONITOR")
        .ok()
        .and_then(|s| parse_monitor_selector(&s).ok())
}

pub fn resolve_display<'a>(
    displays: &'a [DisplayInfo],
    sel: &MonitorSelector,
) -> Result<&'a DisplayInfo, String> {
    match sel {
        MonitorSelector::Primary => displays
            .iter()
            .find(|d| d.primary)
            .or_else(|| displays.first())
            .ok_or_else(|| "no displays available".into()),
        MonitorSelector::Index(i) => displays
            .iter()
            .find(|d| d.index == *i)
            .ok_or_else(|| format!("display index {i} not found")),
        MonitorSelector::DeviceName(name) => {
            let want = name.to_ascii_lowercase();
            displays
                .iter()
                .find(|d| d.device_name.eq_ignore_ascii_case(name)
                    || d.device_name.to_ascii_lowercase().contains(&want)
                    || d.friendly_name
                        .as_ref()
                        .map(|f| f.eq_ignore_ascii_case(name))
                        .unwrap_or(false))
                .ok_or_else(|| format!("display '{name}' not found"))
        }
    }
}

/// Clamp a screen point into the monitor rect (for SendInput cursor keep-in-scope).
pub fn clamp_point_to_monitor(x: i32, y: i32, monitor: &Rect) -> (i32, i32) {
    let x = x.clamp(monitor.left, monitor.right.saturating_sub(1));
    let y = y.clamp(monitor.top, monitor.bottom.saturating_sub(1));
    (x, y)
}

/// Place a window of given size into the work area (keep size, top-left of work area).
pub fn place_window_in_work_area(work: &Rect, width: i32, height: i32) -> Rect {
    let w = width.min(work.width()).max(1);
    let h = height.min(work.height()).max(1);
    Rect::new(work.left, work.top, work.left + w, work.top + h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(l: i32, t: i32, rr: i32, b: i32) -> Rect {
        Rect::new(l, t, rr, b)
    }

    #[test]
    fn overlap_majority() {
        let mon = r(0, 0, 1920, 1080);
        // Fully inside
        assert!(window_mostly_on_monitor(&r(100, 100, 500, 400), &mon));
        // Mostly outside to the right
        assert!(!window_mostly_on_monitor(&r(1500, 0, 2500, 800), &mon));
        // Exactly 50% — requires >50%, so false
        let half = r(960, 0, 1920 + 960, 1080); // 1920x1080 straddling; inter = 960*1080, win=1920*1080 → 50%
        assert!(!window_mostly_on_monitor(&half, &mon));
        // Just over 50%
        let over = r(900, 0, 1920 + 900, 1080);
        assert!(window_mostly_on_monitor(&over, &mon));
    }

    #[test]
    fn point_scope() {
        let mon = r(1920, 0, 3840, 1080);
        assert!(point_in_monitor(2000, 100, &mon));
        assert!(!point_in_monitor(100, 100, &mon));
        assert!(!point_in_monitor(3840, 100, &mon)); // exclusive right
        assert_eq!(
            clamp_point_to_monitor(10, 10, &mon),
            (1920, 10)
        );
        assert_eq!(
            clamp_point_to_monitor(5000, 2000, &mon),
            (3839, 1079)
        );
    }

    #[test]
    fn parse_selector() {
        assert_eq!(parse_monitor_selector("primary").unwrap(), MonitorSelector::Primary);
        assert_eq!(parse_monitor_selector("2").unwrap(), MonitorSelector::Index(2));
        assert_eq!(
            parse_monitor_selector(r"\\.\DISPLAY1").unwrap(),
            MonitorSelector::DeviceName(r"\\.\DISPLAY1".into())
        );
    }

    #[test]
    fn resolve_and_place() {
        let displays = vec![
            DisplayInfo {
                index: 0,
                device_name: r"\\.\DISPLAY1".into(),
                friendly_name: Some("Main".into()),
                rect: r(0, 0, 1920, 1080),
                work_area: r(0, 0, 1920, 1040),
                dpi: 96,
                scale: 1.0,
                primary: true,
            },
            DisplayInfo {
                index: 1,
                device_name: r"\\.\DISPLAY2".into(),
                friendly_name: None,
                rect: r(1920, 0, 3840, 1080),
                work_area: r(1920, 0, 3840, 1040),
                dpi: 144,
                scale: 1.5,
                primary: false,
            },
        ];
        assert_eq!(resolve_display(&displays, &MonitorSelector::Primary).unwrap().index, 0);
        assert_eq!(resolve_display(&displays, &MonitorSelector::Index(1)).unwrap().index, 1);
        assert_eq!(
            resolve_display(&displays, &MonitorSelector::DeviceName("DISPLAY2".into()))
                .unwrap()
                .index,
            1
        );
        let placed = place_window_in_work_area(&displays[1].work_area, 800, 600);
        assert_eq!(placed.left, 1920);
        assert_eq!(placed.top, 0);
        assert_eq!(placed.width(), 800);
    }
}
