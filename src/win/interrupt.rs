//! Physical Esc / mouse interruption via low-level hooks.
//!
//! Verified: WH_KEYBOARD_LL + WH_MOUSE_LL, ignore LLKHF_INJECTED, vkCode==0x1B.
//! Also feeds ActivityTracker for idle gating (non-injected input only).

use crate::activity::ActivityTracker;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG,
    MSLLHOOKSTRUCT, PM_NOREMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
    LLKHF_INJECTED, LLMHF_INJECTED,
};

static FLAG: Mutex<Option<Arc<Mutex<bool>>>> = Mutex::new(None);
static ACTIVITY: Mutex<Option<Arc<ActivityTracker>>> = Mutex::new(None);
static mut KBD_HOOK: HHOOK = HHOOK(std::ptr::null_mut());
static mut MOUSE_HOOK: HHOOK = HHOOK(std::ptr::null_mut());
/// Thread that owns the low-level hooks and pumps messages so the hooks can run.
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);

pub struct HookGuard;

impl Drop for HookGuard {
    fn drop(&mut self) {
        unsafe {
            if !KBD_HOOK.0.is_null() {
                let _ = UnhookWindowsHookEx(KBD_HOOK);
                KBD_HOOK = HHOOK(std::ptr::null_mut());
            }
            if !MOUSE_HOOK.0.is_null() {
                let _ = UnhookWindowsHookEx(MOUSE_HOOK);
                MOUSE_HOOK = HHOOK(std::ptr::null_mut());
            }
        }
        if let Ok(mut g) = FLAG.lock() {
            *g = None;
        }
        if let Ok(mut g) = ACTIVITY.lock() {
            *g = None;
        }
        let tid = HOOK_THREAD.swap(0, Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// Install WH_KEYBOARD_LL + WH_MOUSE_LL on a dedicated thread that runs a
/// message loop. Low-level hooks are invoked only while that thread pumps
/// messages; installing them on the stdio thread (blocked in ReadFile) makes
/// every key/mouse event on the desktop stall until the hook timeout.
pub fn install(flag: Arc<Mutex<bool>>, activity: Arc<ActivityTracker>) -> Result<HookGuard, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("ocu-hooks".into())
        .spawn(move || hook_thread(flag, activity, tx))
        .map_err(|e| format!("spawn hook thread: {e}"))?;
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(tid)) => {
            HOOK_THREAD.store(tid, Ordering::SeqCst);
            eprintln!("ocu-replica: low-level Esc hook installed");
            Ok(HookGuard)
        }
        Ok(Err(e)) => Err(e),
        Err(e) => Err(format!("hook thread did not report install: {e}")),
    }
}

fn hook_thread(
    flag: Arc<Mutex<bool>>,
    activity: Arc<ActivityTracker>,
    tx: std::sync::mpsc::Sender<Result<u32, String>>,
) {
    if let Ok(mut g) = FLAG.lock() {
        *g = Some(flag);
    } else {
        let _ = tx.send(Err("interrupt flag lock poisoned".into()));
        return;
    }
    if let Ok(mut g) = ACTIVITY.lock() {
        *g = Some(activity);
    } else {
        let _ = tx.send(Err("activity lock poisoned".into()));
        return;
    }
    unsafe {
        match SetWindowsHookExW(WH_KEYBOARD_LL, Some(kbd_proc), None, 0) {
            Ok(h) => KBD_HOOK = h,
            Err(e) => {
                let _ = tx.send(Err(format!("SetWindowsHookExW(WH_KEYBOARD_LL) failed: {e}")));
                return;
            }
        }
        match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) {
            Ok(h) => MOUSE_HOOK = h,
            Err(e) => {
                if !KBD_HOOK.0.is_null() {
                    let _ = UnhookWindowsHookEx(KBD_HOOK);
                    KBD_HOOK = HHOOK(std::ptr::null_mut());
                }
                let _ = tx.send(Err(format!("SetWindowsHookExW(WH_MOUSE_LL) failed: {e}")));
                return;
            }
        }
        // Create the thread message queue before publishing the thread id, so
        // PostThreadMessage(WM_QUIT) cannot race ahead of the first GetMessage.
        let mut warmup = MSG::default();
        let _ = PeekMessageW(&mut warmup as *mut MSG, None, 0, 0, PM_NOREMOVE);
        let tid = GetCurrentThreadId();
        let _ = tx.send(Ok(tid));
        let mut msg = MSG::default();
        while GetMessageW(&mut msg as *mut MSG, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg as *const MSG);
            DispatchMessageW(&msg as *const MSG);
        }
    }
}

fn bump_activity() {
    if let Ok(g) = ACTIVITY.lock() {
        if let Some(a) = g.as_ref() {
            a.on_physical_input();
        }
    }
}

unsafe extern "system" fn kbd_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wparam.0 as u32;
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let injected = (info.flags.0 & LLKHF_INJECTED.0) != 0;
            if !injected {
                bump_activity();
                if info.vkCode == 0x1B {
                    if let Ok(g) = FLAG.lock() {
                        if let Some(flag) = g.as_ref() {
                            if let Ok(mut f) = flag.lock() {
                                *f = true;
                            }
                        }
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let injected = (info.flags & LLMHF_INJECTED) != 0;
        if !injected {
            bump_activity();
        }
    }
    let _ = wparam;
    CallNextHookEx(None, code, wparam, lparam)
}
