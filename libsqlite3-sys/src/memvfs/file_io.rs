use std::os::raw;

use crate::{sqlite3_file, sqlite3_int64, SQLITE_IOERR_SHORT_READ, SQLITE_NOTFOUND, SQLITE_OK};
use log::trace;

#[no_mangle]
pub(super) unsafe extern "C" fn dss_close(_arg1: *mut sqlite3_file) -> raw::c_int {
    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_read(
    arg1: *mut sqlite3_file,
    arg2: *mut raw::c_void,
    i_amt: raw::c_int,
    i_ofst: sqlite3_int64,
) -> raw::c_int {
    let p = arg1 as *mut super::File;
    let file_name = &(*p).data.name;

    let guard = super::Fs::get_node(file_name).expect("db must exist");
    let file = guard.read();

    if file
        .copy_out(arg2, i_ofst as isize, i_amt as usize)
        .is_none()
    {
        return SQLITE_IOERR_SHORT_READ;
    }

    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_write(
    arg1: *mut sqlite3_file,
    arg2: *const raw::c_void,
    i_amt: raw::c_int,
    i_ofst: sqlite3_int64,
) -> raw::c_int {
    let p = arg1 as *mut super::File;
    let file_name = &(*p).data.name;

    let guard = super::Fs::get_node(file_name).expect("db must exist");
    let mut file = guard.write();
    file.write_in(arg2, i_ofst as isize, i_amt as usize);

    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_truncate(
    _arg1: *mut sqlite3_file,
    _size: sqlite3_int64,
) -> raw::c_int {
    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_sync(
    _arg1: *mut sqlite3_file,
    _flags: raw::c_int,
) -> raw::c_int {
    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_file_size(
    arg1: *mut sqlite3_file,
    p_size: *mut sqlite3_int64,
) -> raw::c_int {
    let p = arg1 as *mut super::File;
    let file_name = &(*p).data.name;

    let size = super::Fs::get_node(file_name)
        .expect("db must exist")
        .read()
        .size;

    *p_size = size as sqlite3_int64;

    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_lock(
    _arg1: *mut sqlite3_file,
    _arg2: raw::c_int,
) -> raw::c_int {
    trace!("file io lock");
    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_unlock(
    _arg1: *mut sqlite3_file,
    _arg2: raw::c_int,
) -> raw::c_int {
    trace!("file io unlock");
    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_check_reserved_lock(
    _arg1: *mut sqlite3_file,
    p_res_out: *mut raw::c_int,
) -> raw::c_int {
    (*p_res_out) = 0;

    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_file_control(
    _arg1: *mut sqlite3_file,
    _op: raw::c_int,
    _p_arg: *mut raw::c_void,
) -> raw::c_int {
    SQLITE_NOTFOUND
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_sector_size(_arg1: *mut sqlite3_file) -> raw::c_int {
    SQLITE_OK
}

#[no_mangle]
pub(super) unsafe extern "C" fn dss_device_characteristics(_arg1: *mut sqlite3_file) -> raw::c_int {
    SQLITE_OK
}
