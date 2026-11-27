//! Slippy's completion notifications through `native/slippy_notify.m`
//! (`UNUserNotificationCenter`). An unbundled `hex` (no bundle identifier)
//! posts nothing; the Slippy tab's unread dot still shows.

use std::ffi::CString;

unsafe extern "C" {
    fn hex_slippy_notifications_install();
    fn hex_slippy_notifications_authorize();
    #[cfg(not(test))]
    fn hex_slippy_notify(
        identifier: *const std::ffi::c_char,
        title: *const std::ffi::c_char,
        body: *const std::ffi::c_char,
        link: *const std::ffi::c_char,
    );
}

/// Sets the notification delegate, so a click opens `hex://slippy/<ts>`
/// and a banner shows while Hex is in front. Called on the main thread
/// before Rune starts.
pub(crate) fn install() {
    // SAFETY: the shim takes no arguments and checks for a bundle itself.
    unsafe { hex_slippy_notifications_install() }
}

/// Asks once for permission to post, at the first send.
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
