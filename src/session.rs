//! Read-only logind seat gating, independent of desktop credentials and buses.
use std::ffi::CStr;
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

#[link(name = "systemd")]
extern "C" {
    fn sd_seat_get_active(seat: *const libc::c_char, session: *mut *mut libc::c_char, uid: *mut libc::uid_t) -> libc::c_int;
    fn sd_session_is_active(session: *const libc::c_char) -> libc::c_int;
    fn sd_session_is_remote(session: *const libc::c_char) -> libc::c_int;
    fn sd_session_get_class(session: *const libc::c_char, value: *mut *mut libc::c_char) -> libc::c_int;
    fn sd_session_get_type(session: *const libc::c_char, value: *mut *mut libc::c_char) -> libc::c_int;
    fn sd_login_monitor_new(category: *const libc::c_char, monitor: *mut *mut libc::c_void) -> libc::c_int;
    fn sd_login_monitor_unref(monitor: *mut libc::c_void) -> *mut libc::c_void;
    fn sd_login_monitor_get_fd(monitor: *mut libc::c_void) -> libc::c_int;
    fn sd_login_monitor_flush(monitor: *mut libc::c_void) -> libc::c_int;
}

struct LoginString(*mut libc::c_char);

impl LoginString {
    fn bytes(&self) -> &[u8] { unsafe { CStr::from_ptr(self.0).to_bytes() } }
}

impl PartialEq for LoginString {
    fn eq(&self, other: &Self) -> bool { self.bytes() == other.bytes() }
}

impl Drop for LoginString {
    fn drop(&mut self) { unsafe { libc::free(self.0.cast()) }; }
}

fn login_value(session: &CStr, getter: unsafe extern "C" fn(*const libc::c_char, *mut *mut libc::c_char) -> libc::c_int) -> Option<LoginString> {
    let mut value = std::ptr::null_mut();
    let result = unsafe { getter(session.as_ptr(), &mut value) };
    if result < 0 || value.is_null() {
        unsafe { libc::free(value.cast()) };
        None
    } else {
        Some(LoginString(value))
    }
}

fn valid_session_id(value: &[u8]) -> bool {
    !value.is_empty() && value.len() <= 128 && value.iter().all(|v| v.is_ascii_alphanumeric() || *v == b'_' || *v == b'-')
}

pub struct Gate {
    monitor: *mut libc::c_void,
    active: Option<LoginString>,
    checked: Instant,
}

impl Gate {
    pub fn new() -> Result<Self, String> {
        let mut gate = Self { monitor: std::ptr::null_mut(),
            active: None, checked: Instant::now() - Duration::from_secs(1) };
        let result = unsafe { sd_login_monitor_new(std::ptr::null(), &mut gate.monitor) };
        if result < 0 || gate.monitor.is_null() || gate.fd() < 0 {
            return Err("logind monitor unavailable; refusing cursor output".into());
        }
        Ok(gate)
    }

    pub fn fd(&self) -> RawFd { unsafe { sd_login_monitor_get_fd(self.monitor) } }
    pub fn allowed(&self) -> bool { self.active.is_some() }

    pub fn refresh(&mut self, notified: bool) -> Result<bool, String> {
        if !notified && self.checked.elapsed() < Duration::from_secs(1) { return Ok(false); }
        if notified && unsafe { sd_login_monitor_flush(self.monitor) } < 0 {
            self.active = None;
            return Err("logind monitor failed; refusing cursor output".into());
        }
        let active = self.query();
        self.checked = Instant::now();
        let changed = self.active != active;
        if changed { log::info!("local graphical session output {}", if active.is_some() { "enabled (fresh lift required)" } else { "paused" }); }
        self.active = active;
        Ok(changed)
    }

    fn query(&self) -> Option<LoginString> {
        let mut session = std::ptr::null_mut();
        let mut uid = 0;
        let result = unsafe { sd_seat_get_active(c"seat0".as_ptr(), &mut session, &mut uid) };
        if result < 0 || session.is_null() {
            unsafe { libc::free(session.cast()) };
            return None;
        }
        let session = LoginString(session);
        if uid == 0 || !valid_session_id(session.bytes()) { return None; }
        let session_cstr = unsafe { CStr::from_ptr(session.0) };
        let class = login_value(session_cstr, sd_session_get_class)?;
        let kind = login_value(session_cstr, sd_session_get_type)?;
        if unsafe { sd_session_is_active(session.0) } != 1 || unsafe { sd_session_is_remote(session.0) } != 0
            || class.bytes() != b"user" || !matches!(kind.bytes(), b"x11" | b"wayland")
        { return None; }
        // Lock screens may retain the active graphical session: no unlock is inferred.
        Some(session)
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        unsafe {
            sd_login_monitor_unref(self.monitor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_identifiers_cannot_inject_object_paths() {
        for value in [b"../seat0".as_slice(), b"/etc/passwd", b"", b"a\0b", b"a/b"] {
            assert!(!valid_session_id(value));
        }
        assert!(valid_session_id(b"c17"));
        assert!(!valid_session_id(&[b'a'; 129]));
    }
}
