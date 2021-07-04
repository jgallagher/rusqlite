use std::ffi::CStr;
use std::os::raw;

use crate::{sqlite3_file, sqlite3_int64, sqlite3_vfs, SQLITE_OK};

use super::helpers::cchar_to_str;
use super::File;

#[no_mangle]
pub(super) unsafe extern "C" fn dss_open(
    _arg1: *mut sqlite3_vfs,
    z_name: *const raw::c_char,
    arg2: *mut sqlite3_file,
    _flags: raw::c_int,
    _p_out_flags: *mut raw::c_int,
) -> raw::c_int {
    let name = cchar_to_str(z_name)
        .expect("meet non-utf8 db name")
        .to_string();

    log::trace!("open db: {}", name);

    let p = arg2 as *mut File;
    (*p).data.name = name.clone();

    let io_methods = Box::new(super::get_io_methods());
    let io_methods_r = io_methods.as_ref();
    (*arg2).pMethods = io_methods_r;
    std::mem::forget(io_methods);

    super::Fs::add_file(name);

    0
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_delete(
    _arg1: *mut sqlite3_vfs,
    _z_name: *const raw::c_char,
    _sync_dir: raw::c_int,
) -> raw::c_int {
    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_access(
    _arg1: *mut sqlite3_vfs,
    _z_name: *const raw::c_char,
    _flags: raw::c_int,
    p_res_out: *mut raw::c_int,
) -> raw::c_int {
    *p_res_out = 0;

    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_full_path_name(
    _arg1: *mut sqlite3_vfs,
    z_name: *const raw::c_char,
    _n_out: raw::c_int,
    z_out: *mut raw::c_char,
) -> raw::c_int {
    let s_len = CStr::from_ptr(z_name).to_bytes().len();

    std::ptr::copy(z_name, z_out, s_len);

    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_dl_open(
    _arg1: *mut sqlite3_vfs,
    _z_filename: *const raw::c_char,
) -> *mut raw::c_void {
    std::ptr::null_mut()
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_dl_error(
    _arg1: *mut sqlite3_vfs,
    _n_byte: raw::c_int,
    _z_err_msg: *mut raw::c_char,
) {
}

type SymFn = unsafe extern "C" fn(
    arg1: *mut sqlite3_vfs,
    arg2: *mut raw::c_void,
    z_symbol: *const raw::c_char,
);

#[no_mangle]
pub(super) unsafe extern "C" fn dss_dl_sym(
    _arg1: *mut sqlite3_vfs,
    _arg2: *mut raw::c_void,
    _z_symbol: *const raw::c_char,
) -> Option<SymFn> {
    None
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_dl_close(_arg1: *mut sqlite3_vfs, _arg2: *mut raw::c_void) {}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_randomness(
    _arg1: *mut sqlite3_vfs,
    _n_byte: raw::c_int,
    _z_out: *mut raw::c_char,
) -> raw::c_int {
    SQLITE_OK
}

// no need to sleep
#[no_mangle]
pub(super) unsafe extern "C" fn dss_sleep(
    _arg1: *mut sqlite3_vfs,
    _microseconds: raw::c_int,
) -> raw::c_int {
    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_current_time(
    _arg1: *mut sqlite3_vfs,
    _arg2: *mut f64,
) -> raw::c_int {
    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_get_last_error(
    _arg1: *mut sqlite3_vfs,
    _arg2: raw::c_int,
    _arg3: *mut raw::c_char,
) -> raw::c_int {
    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_current_time_int64(
    _arg1: *mut sqlite3_vfs,
    _arg2: *mut sqlite3_int64,
) -> raw::c_int {
    SQLITE_OK
}
