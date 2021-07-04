#![allow(non_snake_case, non_camel_case_types)]
#![cfg_attr(test, allow(deref_nullptr))] // https://github.com/rust-lang/rust-bindgen/issues/2066

pub use self::error::*;

use std::default::Default;
use std::mem;

mod error;

pub fn SQLITE_STATIC() -> sqlite3_destructor_type {
    None
}

pub fn SQLITE_TRANSIENT() -> sqlite3_destructor_type {
    Some(unsafe { mem::transmute(-1isize) })
}

/// Run-Time Limit Categories
#[repr(i32)]
#[non_exhaustive]
#[allow(clippy::upper_case_acronyms)]
pub enum Limit {
    /// The maximum size of any string or BLOB or table row, in bytes.
    SQLITE_LIMIT_LENGTH = SQLITE_LIMIT_LENGTH,
    /// The maximum length of an SQL statement, in bytes.
    SQLITE_LIMIT_SQL_LENGTH = SQLITE_LIMIT_SQL_LENGTH,
    /// The maximum number of columns in a table definition or in the result set
    /// of a SELECT or the maximum number of columns in an index or in an
    /// ORDER BY or GROUP BY clause.
    SQLITE_LIMIT_COLUMN = SQLITE_LIMIT_COLUMN,
    /// The maximum depth of the parse tree on any expression.
    SQLITE_LIMIT_EXPR_DEPTH = SQLITE_LIMIT_EXPR_DEPTH,
    /// The maximum number of terms in a compound SELECT statement.
    SQLITE_LIMIT_COMPOUND_SELECT = SQLITE_LIMIT_COMPOUND_SELECT,
    /// The maximum number of instructions in a virtual machine program used to
    /// implement an SQL statement.
    SQLITE_LIMIT_VDBE_OP = SQLITE_LIMIT_VDBE_OP,
    /// The maximum number of arguments on a function.
    SQLITE_LIMIT_FUNCTION_ARG = SQLITE_LIMIT_FUNCTION_ARG,
    /// The maximum number of attached databases.
    SQLITE_LIMIT_ATTACHED = SQLITE_LIMIT_ATTACHED,
    /// The maximum length of the pattern argument to the LIKE or GLOB
    /// operators.
    SQLITE_LIMIT_LIKE_PATTERN_LENGTH = SQLITE_LIMIT_LIKE_PATTERN_LENGTH,
    /// The maximum index number of any parameter in an SQL statement.
    SQLITE_LIMIT_VARIABLE_NUMBER = SQLITE_LIMIT_VARIABLE_NUMBER,
    /// The maximum depth of recursion for triggers.
    SQLITE_LIMIT_TRIGGER_DEPTH = 10,
    /// The maximum number of auxiliary worker threads that a single prepared
    /// statement may start.
    SQLITE_LIMIT_WORKER_THREADS = 11,
}

#[allow(clippy::all)]
mod bindings {
    include!(concat!(env!("OUT_DIR"), "/bindgen.rs"));
}
pub use bindings::*;

pub type sqlite3_index_constraint = sqlite3_index_info_sqlite3_index_constraint;
pub type sqlite3_index_constraint_usage = sqlite3_index_info_sqlite3_index_constraint_usage;

impl Default for sqlite3_vtab {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl Default for sqlite3_vtab_cursor {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod allocator {
    use std::alloc::{alloc, dealloc, realloc as rs_realloc, Layout};

    #[no_mangle]
    pub unsafe fn sqlite_malloc(len: usize) -> *mut u8 {
        let align = std::mem::align_of::<usize>();
        let layout = Layout::from_size_align_unchecked(len, align);

        let ptr = alloc(layout);
        ptr
    }

    const SQLITE_PTR_SIZE: usize = 8;

    #[no_mangle]
    pub unsafe fn sqlite_free(ptr: *mut u8) -> i32 {
        let mut size_a = [0; SQLITE_PTR_SIZE];

        size_a.as_mut_ptr().copy_from(ptr, SQLITE_PTR_SIZE);

        let ptr_size: u64 = u64::from_le_bytes(size_a);

        let align = std::mem::align_of::<usize>();
        let layout = Layout::from_size_align_unchecked(ptr_size as usize, align);

        dealloc(ptr, layout);

        0
    }

    #[no_mangle]
    pub unsafe fn sqlite_realloc(ptr: *mut u8, size: usize) -> *mut u8 {
        let align = std::mem::align_of::<usize>();
        let layout = Layout::from_size_align_unchecked(size, align);

        rs_realloc(ptr, layout, size)
    }
}

#[cfg(feature = "sqlite-memvfs")]
mod memvfs;

#[cfg(feature = "sqlite-memvfs")]
mod vfs {
    use log::debug;

    #[no_mangle]
    pub unsafe fn sqlite3_os_init() -> std::os::raw::c_int {
        let mut mem_vfs = Box::new(super::memvfs::get_mem_vfs());

        let mem_vfs_ptr: *mut crate::sqlite3_vfs = mem_vfs.as_mut();
        let rc = crate::sqlite3_vfs_register(mem_vfs_ptr, 1);
        debug!("sqlite3 vfs register result: {}", rc);

        std::mem::forget(mem_vfs);

        rc
    }
}
