//! The tray thread's one wait: window messages and kernel handles together, through
//! `MsgWaitForMultipleObjectsEx`. Integrations register a handle (a pipe's overlapped event, a
//! socket's `WSAEventSelect` event) with a callback, and the loop sleeps until either a message
//! or one of them is signalled. No thread per integration, no polling.

use std::cell::RefCell;

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage, MSG, MWMO_INPUTAVAILABLE,
    PM_REMOVE, QS_ALLINPUT, WM_QUIT,
};

type Callback = Box<dyn FnMut()>;

thread_local! {
    static HANDLES: RefCell<Vec<(isize, Option<Callback>)>> = const { RefCell::new(Vec::new()) };
}

/// Call `callback` on this thread whenever `handle` is signalled, until [`unregister`].
/// Auto-reset events suit best: the callback must leave the handle unsignalled.
pub fn register(handle: HANDLE, callback: impl FnMut() + 'static) {
    HANDLES.with(|handles| handles.borrow_mut().push((handle.0 as isize, Some(Box::new(callback)))));
}

pub fn unregister(handle: HANDLE) {
    HANDLES.with(|handles| handles.borrow_mut().retain(|(h, _)| *h != handle.0 as isize));
}

/// Pump messages and serve handles until `WM_QUIT`.
pub fn run() {
    loop {
        let handles: Vec<HANDLE> = HANDLES.with(|h| h.borrow().iter().map(|(raw, _)| HANDLE(*raw as _)).collect());
        // At most 63 handles; the integrations use a handful.
        let woke = unsafe { MsgWaitForMultipleObjectsEx(Some(&handles), u32::MAX, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
        let index = woke.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
        if index < handles.len() {
            fire(handles[index]);
        }
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                return;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

/// Run a handle's callback, taken out of the table while it runs so it may (un)register.
fn fire(handle: HANDLE) {
    let raw = handle.0 as isize;
    let callback = HANDLES.with(|h| h.borrow_mut().iter_mut().find(|(r, _)| *r == raw).and_then(|(_, cb)| cb.take()));
    let Some(mut callback) = callback else { return };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut callback)).is_err() {
        eprintln!("wait: a handle callback panicked");
    }
    // Put it back unless it unregistered itself meanwhile.
    HANDLES.with(|h| {
        if let Some((_, slot)) = h.borrow_mut().iter_mut().find(|(r, cb)| *r == raw && cb.is_none()) {
            *slot = Some(callback);
        }
    });
}
