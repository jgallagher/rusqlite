use log::warn;
use std::ffi::CStr;
use std::os::raw;

pub(super) fn cchar_to_str<'a>(s: *const raw::c_char) -> Option<&'a str> {
    match unsafe { CStr::from_ptr(s).to_str() } {
        Ok(s) => Some(s),
        Err(e) => {
            warn!("translate c string to rust failed: err= {}", e);
            None
        }
    }
}
