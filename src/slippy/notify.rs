//! Slippy's completion notifications through `native/slippy_notify.m`
//! (`UNUserNotificationCenter`). An unbundled `hex` (no bundle identifier)
//! posts nothing; the Slippy tab's unread dot still shows. The shim reports
//! each request's settings and outcome through [`hex_slippy_notify_log`], so
//! `process.log` says whether macOS took it.

use std::ffi::{CStr, CString, c_char, c_int};

unsafe extern "C" {
    fn hex_slippy_notifications_install();
    fn hex_slippy_notifications_authorize();
    #[cfg(not(test))]
    fn hex_slippy_notify(
        identifier: *const c_char,
        title: *const c_char,
        body: *const c_char,
        link: *const c_char,
    );
}

/// Sets the notification delegate, so a click opens `hex://slippy/<ts>`
/// and a banner shows while Hex is in front. Called on the main thread
/// before Rune starts.
pub(crate) fn install() {
    // SAFETY: the shim takes no arguments and checks for a bundle itself.
    unsafe { hex_slippy_notifications_install() }
}

/// The shim's diagnostics: `level` 0 is info, anything else a warning.
#[unsafe(no_mangle)]
extern "C" fn hex_slippy_notify_log(level: c_int, message: *const c_char) {
    if message.is_null() {
        return;
    }
    // SAFETY: the shim passes a NUL-terminated string live for this call.
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    if level == 0 {
        tracing::info!(target: "hex::slippy", "{message}");
    } else {
        tracing::warn!(target: "hex::slippy", "{message}");
    }
}

/// Asks once for permission to post (alerts and sounds), when the Slippy
/// worker starts, so the prompt never races the first answer.
pub(super) fn authorize() {
    // SAFETY: the shim takes no arguments and runs its request once.
    unsafe { hex_slippy_notifications_authorize() }
}

pub(super) fn post(ts: &str, seq: u64, title: &str, body: &str) {
    let identifier = format!("hex.slippy.{ts}.{seq}");
    let link = format!("{}://slippy/{ts}", crate::rune_ui::story_link::scheme());
    let string = |value: &str| CString::new(value.replace('\0', "")).unwrap_or_default();
    let (identifier, title, body, link) = (
        string(&identifier),
        string(title),
        string(body),
        string(&link),
    );
    #[cfg(test)]
    {
        let _ = (title, link);
        POSTED.lock().unwrap().push((
            identifier.to_string_lossy().into_owned(),
            body.to_string_lossy().into_owned(),
        ));
    }
    // SAFETY: each pointer is a live NUL-terminated string the shim copies.
    #[cfg(not(test))]
    unsafe {
        hex_slippy_notify(
            identifier.as_ptr(),
            title.as_ptr(),
            body.as_ptr(),
            link.as_ptr(),
        )
    }
}

/// What tests posted: each notification's identifier and body.
#[cfg(test)]
pub(super) static POSTED: std::sync::Mutex<Vec<(String, String)>> =
    std::sync::Mutex::new(Vec::new());
