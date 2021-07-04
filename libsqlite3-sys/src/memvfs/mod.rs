use std::collections::HashMap;
use std::os::raw;
use std::sync::Arc;

use crate::{sqlite3_file, sqlite3_io_methods, sqlite3_vfs};
use lazy_static::lazy_static;
use parking_lot::RwLock;

mod file_io;
mod helpers;
mod vfs;
#[repr(C)]
struct File {
    base: sqlite3_file,
    data: FileData,
}
#[repr(C)]
struct FileData {
    name: String,
}
struct Node {
    size: usize,
    data: Vec<u8>,
}

impl Node {
    fn copy_out(&self, dst: *mut raw::c_void, offset: isize, count: usize) -> Option<()> {
        if self.size < offset as usize + count {
            log::trace!("handle invalid input offset");
            return None;
        }

        let ptr = self.data.as_ptr();

        let dst = dst as *mut u8;

        unsafe {
            let ptr = ptr.offset(offset);
            ptr.copy_to(dst, count);
        }

        Some(())
    }

    fn write_in(&mut self, src: *const raw::c_void, offset: isize, count: usize) {
        let new_end = offset as usize + count;
        let count_extend: isize = new_end as isize - self.data.len() as isize;
        if count_extend > 0 {
            self.data.extend(vec![0; count_extend as usize]);
        }

        if new_end > self.size {
            self.size = new_end;
        }

        let ptr = self.data.as_mut_ptr();

        unsafe {
            let ptr = ptr.offset(offset);
            ptr.copy_from(src as *const u8, count);
        }
    }
}

const FS_NODE_INITIAL_SIZE: usize = 8192;

lazy_static! {
    static ref FS: RwLock<HashMap<String, Arc<RwLock<Node>>>> = RwLock::new(HashMap::new());
}

struct Fs;

impl Fs {
    fn file_exists(name: &str) -> bool {
        FS.read().contains_key(name)
    }

    fn add_file(name: String) {
        if Fs::file_exists(&name) {
            return;
        }

        log::trace!("insert new fs node: {}", name);

        let node = Node {
            size: 0,
            data: vec![0; FS_NODE_INITIAL_SIZE],
        };
        FS.write().insert(name, Arc::new(RwLock::new(node)));
    }

    #[allow(dead_code)]
    fn del_file(name: &str) {
        log::trace!("remove fs node: {}", name);

        FS.write().remove(name);
    }

    fn get_node(name: &str) -> Option<Arc<RwLock<Node>>> {
        FS.read().get(name).cloned()
    }
}

pub(crate) fn get_mem_vfs() -> sqlite3_vfs {
    let version = 1;
    let file_size = std::mem::size_of::<File>() as raw::c_int;
    let max_path_len = 1024;
    let p_next = std::ptr::null_mut();

    let z_name = "memvfs-rs";
    let z_name = z_name.as_ptr() as *const raw::c_char;
    let p_app_data = std::ptr::null_mut();

    sqlite3_vfs {
        iVersion: version,
        szOsFile: file_size,
        mxPathname: max_path_len,
        pNext: p_next,
        zName: z_name,
        pAppData: p_app_data,
        xOpen: Some(vfs::dss_open),
        xDelete: Some(vfs::dss_delete),
        xAccess: Some(vfs::dss_access),
        xFullPathname: Some(vfs::dss_full_path_name),
        xDlOpen: Some(vfs::dss_dl_open),
        xDlError: Some(vfs::dss_dl_error),
        xDlSym: Some(vfs::dss_dl_sym),
        xDlClose: Some(vfs::dss_dl_close),
        xRandomness: Some(vfs::dss_randomness),
        xSleep: Some(vfs::dss_sleep),
        xCurrentTime: Some(vfs::dss_current_time),
        xGetLastError: Some(vfs::dss_get_last_error),
        xCurrentTimeInt64: None,
        xSetSystemCall: None,
        xGetSystemCall: None,
        xNextSystemCall: None,
    }
}

fn get_io_methods() -> sqlite3_io_methods {
    sqlite3_io_methods {
        iVersion: 1,
        xClose: Some(file_io::dss_close),
        xRead: Some(file_io::dss_read),
        xWrite: Some(file_io::dss_write),
        xTruncate: Some(file_io::dss_truncate),
        xSync: Some(file_io::dss_sync),
        xFileSize: Some(file_io::dss_file_size),
        xLock: Some(file_io::dss_lock),
        xUnlock: Some(file_io::dss_unlock),
        xCheckReservedLock: Some(file_io::dss_check_reserved_lock),
        xFileControl: Some(file_io::dss_file_control),
        xSectorSize: Some(file_io::dss_sector_size),
        xDeviceCharacteristics: Some(file_io::dss_device_characteristics),
        xShmMap: None,
        xShmLock: None,
        xShmBarrier: None,
        xShmUnmap: None,
        xFetch: None,
        xUnfetch: None,
    }
}
