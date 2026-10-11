//! Deliver a held Escape before a host's accelerator processing can dismiss its FX window.
//! `WM_GETDLGCODE` alone does not protect Escape in REAPER's CLAP window.

use std::cell::RefCell;
use std::ptr::null_mut;
use std::rc::{Rc, Weak};

use winapi::shared::minwindef::{LPARAM, LRESULT, WPARAM};
use winapi::shared::windef::HHOOK;
use winapi::um::processthreadsapi::GetCurrentThreadId;
use winapi::um::winuser::{
    CallNextHookEx, GetFocus, SendMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, MSG,
    PM_REMOVE, VK_ESCAPE, WH_GETMESSAGE, WM_KEYDOWN, WM_NULL,
};

use super::window::WindowState;

struct EscapeHook {
    handle: HHOOK,
    windows: Vec<Weak<WindowState>>,
}

impl Drop for EscapeHook {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: this GUI thread owns the hook. Tear it down even if another TLS
            // destructor destroys our windows only after this registry has gone away.
            unsafe { UnhookWindowsHookEx(self.handle) };
        }
    }
}

thread_local! {
    // Each plug-in DLL and GUI thread owns its own hook and only its own windows.
    static HOOK: RefCell<EscapeHook> = const { RefCell::new(EscapeHook {
        handle: null_mut(), windows: Vec::new(),
    }) };
}

pub(super) fn register(window: &Rc<WindowState>) {
    let _ = HOOK.try_with(|hook| {
        let mut hook = hook.borrow_mut();
        if hook.handle.is_null() {
            // SAFETY: this thread owns the window. The callback is in this process, and the
            // hook is removed on the last HWND's destruction and on GUI thread teardown.
            hook.handle = unsafe {
                SetWindowsHookExW(
                    WH_GETMESSAGE,
                    Some(get_message),
                    null_mut(),
                    GetCurrentThreadId(),
                )
            };
        }
        hook.windows.push(Rc::downgrade(window));
    });
}

pub(super) fn unregister(window: &WindowState) {
    let _ = HOOK.try_with(|hook| {
        let mut hook = hook.borrow_mut();
        hook.windows
            .retain(|weak| weak.upgrade().is_some_and(|registered| registered.hwnd != window.hwnd));
        if hook.windows.is_empty() && !hook.handle.is_null() {
            // SAFETY: our thread's hook, no longer needed by any live window.
            unsafe { UnhookWindowsHookEx(hook.handle) };
            hook.handle = null_mut();
        }
    });
}

unsafe extern "system" fn get_message(code: i32, removed: WPARAM, message: LPARAM) -> LRESULT {
    if code == HC_ACTION && removed == PM_REMOVE as WPARAM {
        // SAFETY: Windows supplies a valid MSG for HC_ACTION. Copy it so no reference to
        // the queue's message, or borrow of our registry, spans the window handler's call.
        let original = *(message as *const MSG);
        if original.message == WM_KEYDOWN && original.wParam == VK_ESCAPE as WPARAM {
            let window = HOOK
                .try_with(|hook| {
                    hook.borrow().windows.iter().filter_map(Weak::upgrade).find(|window| {
                        window.hwnd == original.hwnd
                            && window.wants_keys()
                            && GetFocus() == window.hwnd
                    })
                })
                .ok()
                .flatten();
            if let Some(_window) = window {
                // Consume before calling the handler: it may pump messages, return focus,
                // or destroy the window. Keep its state alive until that call returns.
                (*(message as *mut MSG)).message = WM_NULL;
                (*(message as *mut MSG)).wParam = 0;
                (*(message as *mut MSG)).lParam = 0;
                SendMessageW(original.hwnd, original.message, original.wParam, original.lParam);
            }
        }
    }
    // Other hooks still see every retrieval, including unchanged PM_NOREMOVE peeks.
    CallNextHookEx(null_mut(), code, removed, message)
}
