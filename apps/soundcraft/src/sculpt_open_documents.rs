//! Cocoa Open Document support (Finder, Dock and Open With).
//! Never touches the audio thread. Multiple files are queued for the GUI frame.
use std::ffi::{CStr, c_char};
use std::sync::{Mutex, OnceLock};

static PATHS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

unsafe extern "C" {
    fn sculpt_install_document_handler(callback: unsafe extern "C" fn(*const c_char));
}

unsafe extern "C" fn receive(path: *const c_char) {
    if path.is_null() {
        return;
    }
    // SAFETY: the Objective-C handler passes a temporary NUL-terminated UTF-8
    // filesystem path; we clone it before returning from the callback.
    let name = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
    if !name.is_empty()
        && let Ok(mut queue) = PATHS.get_or_init(|| Mutex::new(Vec::new())).lock()
    {
        queue.push(name);
    }
}

/// May be called more than once: AppKit may reinstall its own handlers on start.
pub fn install() {
    PATHS.get_or_init(|| Mutex::new(Vec::new()));
    // SAFETY: Objective-C implementation only registers an NSAppleEventManager
    // callback on the process main thread; the callback never retains its input.
    unsafe { sculpt_install_document_handler(receive) }
}

pub fn drain() -> Vec<String> {
    let Ok(mut queue) = PATHS.get_or_init(|| Mutex::new(Vec::new())).lock() else { return Vec::new() };
    // Filter duplicate odoc notifications within one frame, keep arrival order.
    let pending = std::mem::take(&mut *queue);
    let mut result = Vec::new();
    for path in pending {
        if !result.contains(&path) {
            result.push(path);
        }
    }
    result
}
