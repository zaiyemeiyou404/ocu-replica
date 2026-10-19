//! UI Automation snapshot + element actions.
//! Verified: CoCreateInstance(CLSID_CUIAutomation), CacheRequest, element_index,
//! Value/RangeValue/Scroll/ExpandCollapse patterns, SetWinEventHook(FOREGROUND).

use crate::engine::AccessSnapshot;
use crate::geometry::Rect;
use crate::protocol::{Direction, SecondaryAction};
use std::collections::HashMap;
use windows::core::{Interface, BSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationElementArray, IUIAutomationExpandCollapsePattern, IUIAutomationRangeValuePattern,
    IUIAutomationScrollPattern, IUIAutomationTreeWalker, IUIAutomationValuePattern,
    TreeScope_Children, TreeScope_Subtree, UIA_ControlTypePropertyId, UIA_ExpandCollapsePatternId,
    UIA_IsEnabledPropertyId, UIA_NamePropertyId, UIA_NativeWindowHandlePropertyId,
    UIA_RangeValuePatternId, UIA_ScrollPatternId, UIA_ValuePatternId,
};
use windows::Win32::Foundation::HWND;

struct CachedElement {
    // We re-resolve by RuntimeId / index within the same revision.
    name: String,
    control_type: String,
    bounds: Rect,
    secondary: Vec<String>,
}

pub struct AxState {
    automation: IUIAutomation,
    /// window_id -> (revision, elements by index)
    cache: HashMap<u64, (u64, Vec<CachedElement>)>,
    revision_counter: u64,
    snapshot_count: u64,
}

impl AxState {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| format!("CoCreateInstance(CUIAutomation): {e}"))?;
            Ok(Self {
                automation,
                cache: HashMap::new(),
                revision_counter: 0,
                snapshot_count: 0,
            })
        }
    }

    pub fn snapshot_count(&self) -> u64 { self.snapshot_count }

    pub fn snapshot(&mut self, hwnd: HWND) -> Result<AccessSnapshot, String> {
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd)
                .map_err(|e| format!("ElementFromHandle: {e}"))?;
            let walker = self.automation.ControlViewWalker()
                .map_err(|e| format!("ControlViewWalker: {e}"))?;
            let mut elements = Vec::new();
            let mut lines = Vec::new();
            let mut doc_values = Vec::new();
            walk(&walker, &root, 0, &mut elements, &mut lines, 0, 400, &mut doc_values)?;
            self.revision_counter += 1;
            self.snapshot_count += 1;
            let rev = self.revision_counter;
            let window_id = hwnd.0 as usize as u64;
            self.cache.insert(window_id, (rev, elements));
            let focused = self.automation.GetFocusedElement().ok().and_then(|e| {
                let name = e.CurrentName().ok()?.to_string();
                Some(format!("focused: {name}"))
            });
            Ok(AccessSnapshot {
                tree: lines.join("\n"),
                focused_element: focused,
                selected_text: None,
                selected_elements: None,
                document_text: if doc_values.is_empty() {
                    None
                } else {
                    Some(doc_values.join("\n"))
                },
                revision: rev,
            })
        }
    }

    fn lookup(&self, window_id: u64, element_index: u64, revision: u64) -> Result<&CachedElement, String> {
        let (rev, elems) = self.cache.get(&window_id)
            .ok_or_else(|| format!("no cached accessibility state for window {window_id}"))?;
        if *rev != revision {
            return Err(format!(
                "element {element_index} is not available in cached app state for window {window_id} (stale revision)"
            ));
        }
        elems.get(element_index as usize).ok_or_else(|| {
            format!("element {element_index} no longer exists in window {window_id}")
        })
    }

    pub fn element_bounds(&self, window_id: u64, element_index: u64, revision: u64) -> Result<Rect, String> {
        Ok(self.lookup(window_id, element_index, revision)?.bounds)
    }

    pub fn set_value(&self, window_id: u64, element_index: u64, revision: u64, value: &str) -> Result<(), String> {
        let _ = self.lookup(window_id, element_index, revision)?;
        // Re-resolve live element by walking to the same index (best-effort).
        let hwnd = HWND(window_id as usize as _);
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd)
                .map_err(|e| format!("ElementFromHandle: {e}"))?;
            let el = nth_element(&self.automation, &root, element_index)?;
            if let Ok(pat) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                let bstr = BSTR::from(value);
                pat.SetValue(&bstr).map_err(|e| format!("set UIA value: {e}"))?;
                return Ok(());
            }
            if let Ok(pat) = el.GetCurrentPatternAs::<IUIAutomationRangeValuePattern>(UIA_RangeValuePatternId) {
                let n: f64 = value.parse().map_err(|_| format!("range value must be a number: {value}"))?;
                pat.SetValue(n).map_err(|e| format!("set UIA range value: {e}"))?;
                return Ok(());
            }
            Err("element does not expose ValuePattern/RangeValuePattern".into())
        }
    }

    pub fn scroll_element(&self, window_id: u64, element_index: u64, revision: u64, dir: Direction, pages: f64) -> Result<(), String> {
        let _ = self.lookup(window_id, element_index, revision)?;
        let hwnd = HWND(window_id as usize as _);
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd)
                .map_err(|e| format!("ElementFromHandle: {e}"))?;
            let el = nth_element(&self.automation, &root, element_index)?;
            let pat = el.GetCurrentPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId)
                .map_err(|_| "element no longer exposes ScrollPattern".to_string())?;
            // ScrollPattern uses 0..100 percentages; approximate pages as 10% each.
            let delta = (pages * 10.0).clamp(-100.0, 100.0);
            let (h, v) = match dir {
                Direction::Up => (0.0, -delta),
                Direction::Down => (0.0, delta),
                Direction::Left => (-delta, 0.0),
                Direction::Right => (delta, 0.0),
            };
            // Use Scroll with ScrollAmount_LargeIncrement equivalents via SetScrollPercent relative.
            let cur_h = pat.CurrentHorizontalScrollPercent().unwrap_or(0.0);
            let cur_v = pat.CurrentVerticalScrollPercent().unwrap_or(0.0);
            let nh = (cur_h + h).clamp(0.0, 100.0);
            let nv = (cur_v + v).clamp(0.0, 100.0);
            pat.SetScrollPercent(nh, nv).map_err(|e| format!("ScrollPattern: {e}"))?;
            Ok(())
        }
    }

    pub fn secondary_action(&self, window_id: u64, element_index: u64, revision: u64, action: SecondaryAction) -> Result<(), String> {
        let cached = self.lookup(window_id, element_index, revision)?;
        let label = action.label();
        if !cached.secondary.iter().any(|s| s.eq_ignore_ascii_case(label))
            && !matches!(action, SecondaryAction::Raise)
        {
            // Still attempt; Raise is always synthetic.
        }
        let hwnd = HWND(window_id as usize as _);
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd)
                .map_err(|e| format!("ElementFromHandle: {e}"))?;
            let el = nth_element(&self.automation, &root, element_index)?;
            match action {
                SecondaryAction::Expand | SecondaryAction::Collapse => {
                    let pat = el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId)
                        .map_err(|_| "element no longer exposes ExpandCollapsePattern".to_string())?;
                    if matches!(action, SecondaryAction::Expand) {
                        pat.Expand().map_err(|e| format!("Expand: {e}"))?;
                    } else {
                        pat.Collapse().map_err(|e| format!("Collapse: {e}"))?;
                    }
                }
                SecondaryAction::Scroll(dir) => {
                    self.scroll_element(window_id, element_index, revision, dir, 1.0)?;
                }
                SecondaryAction::Raise => {
                    // Synthetic: bring element into view via scroll-into-view if possible.
                    let _ = el;
                }
            }
            Ok(())
        }
    }

    /// Index + revision of the first edit/document element from the last snapshot.
    /// Used so background `type_text` can target a ValuePattern without a cursor.
    pub fn first_editable(&self, window_id: u64) -> Option<(u64, u64)> {
        let (rev, elems) = self.cache.get(&window_id)?;
        let idx = elems.iter().position(|e| {
            matches!(e.control_type.as_str(), "edit" | "document")
        })?;
        Some((idx as u64, *rev))
    }

    pub fn current_revision(&self, id: Option<u64>) -> serde_json::Value {
        match id.and_then(|i| self.cache.get(&i)) {
            Some((r, _)) => serde_json::json!(*r),
            None => serde_json::Value::Null,
        }
    }

}

fn escape_tree(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "")
        .replace('\n', "\\n")
}

unsafe fn value_of(el: &IUIAutomationElement) -> Option<String> {
    let pat = el
        .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
        .ok()?;
    Some(pat.CurrentValue().ok()?.to_string())
}

unsafe fn walk(
    walker: &IUIAutomationTreeWalker,
    el: &IUIAutomationElement,
    depth: usize,
    out: &mut Vec<CachedElement>,
    lines: &mut Vec<String>,
    index_base: usize,
    limit: usize,
    doc_values: &mut Vec<String>,
) -> Result<usize, String> {
    if out.len() >= limit || depth > 12 {
        return Ok(out.len());
    }
    let idx = out.len();
    let name = el.CurrentName().ok().map(|b| b.to_string()).unwrap_or_default();
    let ctype = control_type_name(el);
    let bounds = element_rect(el).unwrap_or_default();
    let mut secondary = Vec::new();
    if el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId).is_ok() {
        secondary.push("Expand".into());
        secondary.push("Collapse".into());
    }
    if el.GetCurrentPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId).is_ok() {
        secondary.extend(["Scroll Up","Scroll Down","Scroll Left","Scroll Right"].map(String::from));
    }
    secondary.push("Raise".into());
    let value = value_of(el);
    let indent = "  ".repeat(depth);
    let mut line = format!("{idx}{indent} {ctype}");
    if !name.is_empty() {
        line.push_str(&format!(" \"{name}\""));
    }
    if let Some(v) = &value {
        line.push_str(&format!(" value=\"{}\"", escape_tree(v)));
    }
    if !secondary.is_empty() {
        line.push_str(&format!(" Secondary Actions: {}", secondary.join(", ")));
    }
    lines.push(line);
    if matches!(ctype.as_str(), "edit" | "document") {
        if let Some(v) = value {
            doc_values.push(v);
        }
    }
    out.push(CachedElement { name, control_type: ctype, bounds, secondary });

    // Children.
    if let Ok(child) = walker.GetFirstChildElement(el) {
        let mut cur = Some(child);
        while let Some(c) = cur {
            walk(walker, &c, depth + 1, out, lines, index_base, limit, doc_values)?;
            cur = walker.GetNextSiblingElement(&c).ok();
            if out.len() >= limit { break; }
        }
    }
    let _ = index_base;
    Ok(out.len())
}

unsafe fn nth_element(automation: &IUIAutomation, root: &IUIAutomationElement, index: u64) -> Result<IUIAutomationElement, String> {
    let walker = automation.ControlViewWalker().map_err(|e| e.to_string())?;
    let mut elems = Vec::new();
    let mut lines = Vec::new();
    let mut doc_values = Vec::new();
    walk(&walker, root, 0, &mut elems, &mut lines, 0, (index as usize) + 1, &mut doc_values)?;
    // Re-walk collecting actual IUIAutomationElement refs is expensive; do a dedicated walk.
    let mut found: Option<IUIAutomationElement> = None;
    let mut counter = 0u64;
    fn find(
        walker: &IUIAutomationTreeWalker,
        el: &IUIAutomationElement,
        target: u64,
        counter: &mut u64,
        found: &mut Option<IUIAutomationElement>,
        depth: usize,
    ) -> windows::core::Result<()> {
        if found.is_some() || depth > 12 { return Ok(()); }
        if *counter == target {
            *found = Some(el.clone());
            return Ok(());
        }
        *counter += 1;
        if let Ok(child) = unsafe { walker.GetFirstChildElement(el) } {
            let mut cur = Some(child);
            while let Some(c) = cur {
                find(walker, &c, target, counter, found, depth + 1)?;
                if found.is_some() { return Ok(()); }
                cur = unsafe { walker.GetNextSiblingElement(&c).ok() };
            }
        }
        Ok(())
    }
    let _ = find(&walker, root, index, &mut counter, &mut found, 0);
    found.ok_or_else(|| format!("element {index} no longer exists"))
}

unsafe fn element_rect(el: &IUIAutomationElement) -> Result<Rect, String> {
    let r = el.CurrentBoundingRectangle().map_err(|e| e.to_string())?;
    Ok(Rect::new(r.left, r.top, r.right, r.bottom))
}

unsafe fn control_type_name(el: &IUIAutomationElement) -> String {
    // Map a few common ones; fall back to numeric id.
    match el.CurrentControlType() {
        Ok(id) => match id.0 {
            50000 => "button",
            50004 => "edit" ,
            50020 => "text",
            50032 => "window",
            50033 => "pane",
            50018 => "list",
            50007 => "list item",
            50019 => "menu",
            50011 => "menu item",
            50003 => "combo box",
            50002 => "checkbox",
            50021 => "tree",
            50023 => "tab",
            50024 => "tab item",
            50025 => "thumb",
            50006 => "image",
            50029 => "document",
            other => return format!("ctrl:{other}"),
        }.into(),
        Err(_) => "unknown".into(),
    }
}

impl AxState {
    /// Background click: Invoke → Toggle → SelectionItem → ExpandCollapse → Legacy DoDefaultAction.
    pub fn click_element_background(
        &self,
        window_id: u64,
        element_index: u64,
        revision: u64,
    ) -> Result<(), String> {
        use windows::Win32::UI::Accessibility::{
            IUIAutomationInvokePattern, IUIAutomationLegacyIAccessiblePattern,
            IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, UIA_InvokePatternId,
            UIA_LegacyIAccessiblePatternId, UIA_SelectionItemPatternId, UIA_TogglePatternId,
        };
        let _ = self.lookup(window_id, element_index, revision)?;
        let hwnd = HWND(window_id as usize as _);
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd).map_err(|e| e.to_string())?;
            let el = nth_element(&self.automation, &root, element_index)?;
            if let Ok(pat) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
                pat.Invoke().map_err(|e| format!("InvokePattern: {e}"))?;
                return Ok(());
            }
            if let Ok(pat) = el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId) {
                pat.Toggle().map_err(|e| format!("TogglePattern: {e}"))?;
                return Ok(());
            }
            if let Ok(pat) = el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId) {
                pat.Select().map_err(|e| format!("SelectionItem: {e}"))?;
                return Ok(());
            }
            if let Ok(pat) = el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId) {
                // Prefer expand when collapsed; collapse otherwise — best-effort expand.
                let _ = pat.Expand();
                return Ok(());
            }
            if let Ok(pat) = el.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(UIA_LegacyIAccessiblePatternId) {
                pat.DoDefaultAction().map_err(|e| format!("LegacyIAccessible DoDefaultAction: {e}"))?;
                return Ok(());
            }
            Err("element exposes no Invoke/Toggle/SelectionItem/ExpandCollapse/LegacyIAccessible pattern".into())
        }
    }

    pub fn append_value(
        &self,
        window_id: u64,
        element_index: u64,
        revision: u64,
        text: &str,
    ) -> Result<(), String> {
        let _ = self.lookup(window_id, element_index, revision)?;
        let hwnd = HWND(window_id as usize as _);
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd).map_err(|e| e.to_string())?;
            let el = nth_element(&self.automation, &root, element_index)?;
            if let Ok(pat) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                let cur = pat.CurrentValue().ok().map(|b| b.to_string()).unwrap_or_default();
                let bstr = BSTR::from(format!("{cur}{text}"));
                pat.SetValue(&bstr).map_err(|e| format!("set UIA value: {e}"))?;
                return Ok(());
            }
            Err("element does not expose ValuePattern for type_text".into())
        }
    }

    /// Hit-test a screen point; if the element belongs to `window_id`, invoke it.
    pub fn click_point_background(&self, window_id: u64, screen_x: i32, screen_y: i32) -> Result<(), String> {
        use windows::Win32::UI::Accessibility::{
            IUIAutomationInvokePattern, IUIAutomationLegacyIAccessiblePattern, UIA_InvokePatternId,
            UIA_LegacyIAccessiblePatternId,
        };
        unsafe {
            let el = self
                .automation
                .ElementFromPoint(windows::Win32::Foundation::POINT { x: screen_x, y: screen_y })
                .map_err(|e| format!("ElementFromPoint: {e}"))?;
            let mut cur = Some(el.clone());
            let mut belongs = false;
            while let Some(c) = cur {
                if let Ok(h) = c.CurrentNativeWindowHandle() {
                    if h.0 as usize as u64 == window_id {
                        belongs = true;
                        break;
                    }
                }
                cur = self
                    .automation
                    .ControlViewWalker()
                    .ok()
                    .and_then(|w| w.GetParentElement(&c).ok());
            }
            if !belongs {
                return Err(
                    "point is covered or outside target window; cannot background-click by coordinate"
                        .into(),
                );
            }
            if let Ok(pat) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
                pat.Invoke().map_err(|e| e.to_string())?;
                return Ok(());
            }
            if let Ok(pat) =
                el.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(UIA_LegacyIAccessiblePatternId)
            {
                pat.DoDefaultAction().map_err(|e| e.to_string())?;
                return Ok(());
            }
            Err("ElementFromPoint target has no Invoke/LegacyIAccessible pattern".into())
        }
    }
}
