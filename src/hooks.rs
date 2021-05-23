//! `feature = "hooks"` Commit, Data Change and Rollback Notification Callbacks
#![allow(non_camel_case_types)]
use std::os::raw::c_void;

use crate::ffi;

/// `feature = "hooks"` Action Codes
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(i32)]
#[non_exhaustive]
#[allow(clippy::upper_case_acronyms)]
pub enum Action {
    /// Unsupported / unexpected action
    UNKNOWN = -1,
    /// DELETE command
    SQLITE_DELETE = ffi::SQLITE_DELETE,
    /// INSERT command
    SQLITE_INSERT = ffi::SQLITE_INSERT,
    /// UPDATE command
    SQLITE_UPDATE = ffi::SQLITE_UPDATE,
}

impl From<i32> for Action {
    #[inline]
    fn from(code: i32) -> Action {
        match code {
            ffi::SQLITE_DELETE => Action::SQLITE_DELETE,
            ffi::SQLITE_INSERT => Action::SQLITE_INSERT,
            ffi::SQLITE_UPDATE => Action::SQLITE_UPDATE,
            _ => Action::UNKNOWN,
        }
    }
}

unsafe fn free_boxed_hook<F>(p: *mut c_void) {
    drop(Box::from_raw(p as *mut F));
}

#[cfg(feature = "preupdate_hook")]
pub mod preupdate_hook {
    use super::free_boxed_hook;
    use super::Action;

    use std::os::raw::{c_char, c_int, c_void};
    use std::panic::catch_unwind;
    use std::ptr;

    use crate::ffi;
    use crate::types::ValueRef;
    use crate::{Connection, InnerConnection};

    /// `feature = "preupdate_hook"`
    /// The possible cases for when a PreUpdateHook gets triggered. Allows access to the relevant
    /// functions for each case through the contained values.
    pub enum PreUpdateCase {
        Insert(PreUpdateNewValueAccessor),
        Delete(PreUpdateOldValueAccessor),
        Update {
            old_value_accessor: PreUpdateOldValueAccessor,
            new_value_accessor: PreUpdateNewValueAccessor,
        },
    }

    impl From<PreUpdateCase> for Action {
        fn from(puc: PreUpdateCase) -> Action {
            match puc {
                PreUpdateCase::Insert(_) => Action::SQLITE_INSERT,
                PreUpdateCase::Delete(_) => Action::SQLITE_DELETE,
                PreUpdateCase::Update { .. } => Action::SQLITE_UPDATE,
            }
        }
    }

    /// `feature = "preupdate_hook"`
    /// An accessor to access the old values of the row being deleted/updated during the preupdate callback.
    pub struct PreUpdateOldValueAccessor {
        db: *mut ffi::sqlite3,
        old_row_id: i64,
    }

    impl PreUpdateOldValueAccessor {
        /// Get the amount of columns in the row being
        /// deleted/updated.
        pub fn get_column_count(&self) -> i32 {
            unsafe { ffi::sqlite3_preupdate_count(self.db) }
        }

        pub fn get_query_depth(&self) -> i32 {
            unsafe { ffi::sqlite3_preupdate_depth(self.db) }
        }

        pub fn get_old_row_id(&self) -> i64 {
            self.old_row_id
        }

        pub fn get_old_column_value(&self, i: i32) -> ValueRef {
            let mut p_value: *mut ffi::sqlite3_value = ptr::null_mut();
            unsafe {
                ffi::sqlite3_preupdate_old(self.db, i, &mut p_value);
                ValueRef::from_value(p_value)
            }
        }
    }

    /// `feature = "preupdate_hook"`
    /// An accessor to access the new values of the row being inserted/updated during the preupdate callback.
    pub struct PreUpdateNewValueAccessor {
        db: *mut ffi::sqlite3,
        new_row_id: i64,
    }

    impl PreUpdateNewValueAccessor {
        /// Get the amount of columns in the row being
        /// inserted/updated.
        pub fn get_column_count(&self) -> i32 {
            unsafe { ffi::sqlite3_preupdate_count(self.db) }
        }

        pub fn get_query_depth(&self) -> i32 {
            unsafe { ffi::sqlite3_preupdate_depth(self.db) }
        }

        pub fn get_new_row_id(&self) -> i64 {
            self.new_row_id
        }

        pub fn get_new_column_value(&self, i: i32) -> ValueRef {
            let mut p_value: *mut ffi::sqlite3_value = ptr::null_mut();
            unsafe {
                ffi::sqlite3_preupdate_new(self.db, i, &mut p_value);
                ValueRef::from_value(p_value)
            }
        }
    }

    impl Connection {
        ///
        /// `feature = "preupdate_hook"` Register a callback function to be invoked before
        /// a row is updated, inserted or deleted in a rowid table.
        ///
        /// The callback parameters are:
        ///
        /// - the name of the database ("main", "temp", ...),
        /// - the name of the table that is updated,
        /// - a variant of the PreUpdateCase enum which allows access to extra functions depending
        /// on whether it's an update, delete or insert.
        #[inline]
        pub fn preupdate_hook<'c, F>(&'c self, hook: Option<F>)
        where
            F: FnMut(Action, &str, &str, &PreUpdateCase) + Send + 'c,
        {
            self.db.borrow_mut().preupdate_hook(hook);
        }
    }

    impl InnerConnection {
        #[inline]
        pub fn remove_preupdate_hook(&mut self) {
            self.preupdate_hook(None::<fn(Action, &str, &str, &PreUpdateCase)>);
        }

        fn preupdate_hook<'c, F>(&'c mut self, hook: Option<F>)
        where
            F: FnMut(Action, &str, &str, &PreUpdateCase) + Send + 'c,
        {
            unsafe extern "C" fn call_boxed_closure<F>(
                p_arg: *mut c_void,
                sqlite: *mut ffi::sqlite3,
                action_code: c_int,
                db_str: *const c_char,
                tbl_str: *const c_char,
                old_row_id: i64,
                new_row_id: i64,
            ) where
                F: FnMut(Action, &str, &str, &PreUpdateCase),
            {
                use std::ffi::CStr;
                use std::str;

                let action = Action::from(action_code);
                let db_name = {
                    let c_slice = CStr::from_ptr(db_str).to_bytes();
                    str::from_utf8(c_slice)
                };
                let tbl_name = {
                    let c_slice = CStr::from_ptr(tbl_str).to_bytes();
                    str::from_utf8(c_slice)
                };

                let preupdate_hook_functions = match action {
                    Action::SQLITE_INSERT => PreUpdateCase::Insert(PreUpdateNewValueAccessor {
                        db: sqlite,
                        new_row_id,
                    }),
                    Action::SQLITE_DELETE => PreUpdateCase::Delete(PreUpdateOldValueAccessor {
                        db: sqlite,
                        old_row_id,
                    }),
                    Action::SQLITE_UPDATE => PreUpdateCase::Update {
                        old_value_accessor: PreUpdateOldValueAccessor {
                            db: sqlite,
                            old_row_id,
                        },
                        new_value_accessor: PreUpdateNewValueAccessor {
                            db: sqlite,
                            new_row_id,
                        },
                    },
                    _ => todo!(),
                };

                let _ = catch_unwind(|| {
                    let boxed_hook: *mut F = p_arg as *mut F;
                    (*boxed_hook)(
                        action,
                        db_name.expect("illegal db name"),
                        tbl_name.expect("illegal table name"),
                        &preupdate_hook_functions,
                    );
                });
            }

            let free_preupdate_hook = if hook.is_some() {
                Some(free_boxed_hook::<F> as unsafe fn(*mut c_void))
            } else {
                None
            };

            let previous_hook = match hook {
                Some(hook) => {
                    let boxed_hook: *mut F = Box::into_raw(Box::new(hook));
                    unsafe {
                        ffi::sqlite3_preupdate_hook(
                            self.db(),
                            Some(call_boxed_closure::<F>),
                            boxed_hook as *mut _,
                        )
                    }
                }
                _ => unsafe { ffi::sqlite3_preupdate_hook(self.db(), None, ptr::null_mut()) },
            };
            if !previous_hook.is_null() {
                if let Some(free_boxed_hook) = self.free_preupdate_hook {
                    unsafe { free_boxed_hook(previous_hook) };
                }
            }
            self.free_preupdate_hook = free_preupdate_hook;
        }
    }

    #[cfg(test)]
    mod test {
        use super::super::Action;
        use super::PreUpdateCase;
        use crate::{Connection, Result};

        #[test]
        fn test_preupdate_hook() -> Result<()> {
            let db = Connection::open_in_memory()?;

            let mut called = false;
            db.preupdate_hook(Some(
                |action, db: &str, tbl: &str, row_id, new_row_id, _func: &PreUpdateCase| {
                    assert_eq!(Action::SQLITE_INSERT, action);
                    assert_eq!("main", db);
                    assert_eq!("foo", tbl);
                    assert_eq!(1, row_id);
                    assert_eq!(1, new_row_id);
                    called = true;
                },
            ));
            db.execute_batch("CREATE TABLE foo (t TEXT)")?;
            db.execute_batch("INSERT INTO foo VALUES ('lisa')")?;
            assert!(called);
            Ok(())
        }
    }
}

#[cfg(feature = "hooks")]
mod datachanged_and_friends {
    use super::free_boxed_hook;
    use super::Action;

    use std::os::raw::{c_char, c_int, c_void};
    use std::panic::{catch_unwind, RefUnwindSafe};
    use std::ptr;

    use crate::ffi;
    use crate::{Connection, InnerConnection};

    impl Connection {
        /// `feature = "hooks"` Register a callback function to be invoked whenever
        /// a transaction is committed.
        ///
        /// The callback returns `true` to rollback.
        #[inline]
        pub fn commit_hook<'c, F>(&'c self, hook: Option<F>)
        where
            F: FnMut() -> bool + Send + 'c,
        {
            self.db.borrow_mut().commit_hook(hook);
        }

        /// `feature = "hooks"` Register a callback function to be invoked whenever
        /// a transaction is committed.
        ///
        /// The callback returns `true` to rollback.
        #[inline]
        pub fn rollback_hook<'c, F>(&'c self, hook: Option<F>)
        where
            F: FnMut() + Send + 'c,
        {
            self.db.borrow_mut().rollback_hook(hook);
        }

        /// `feature = "hooks"` Register a callback function to be invoked whenever
        /// a row is updated, inserted or deleted in a rowid table.
        ///
        /// The callback parameters are:
        ///
        /// - the type of database update (SQLITE_INSERT, SQLITE_UPDATE or
        /// SQLITE_DELETE),
        /// - the name of the database ("main", "temp", ...),
        /// - the name of the table that is updated,
        /// - the ROWID of the row that is updated.
        #[inline]
        pub fn update_hook<'c, F>(&'c self, hook: Option<F>)
        where
            F: FnMut(Action, &str, &str, i64) + Send + 'c,
        {
            self.db.borrow_mut().update_hook(hook);
        }

        /// `feature = "hooks"` Register a query progress callback.
        ///
        /// The parameter `num_ops` is the approximate number of virtual machine
        /// instructions that are evaluated between successive invocations of the
        /// `handler`. If `num_ops` is less than one then the progress handler
        /// is disabled.
        ///
        /// If the progress callback returns `true`, the operation is interrupted.
        pub fn progress_handler<F>(&self, num_ops: c_int, handler: Option<F>)
        where
            F: FnMut() -> bool + Send + RefUnwindSafe + 'static,
        {
            self.db.borrow_mut().progress_handler(num_ops, handler);
        }
    }

    impl InnerConnection {
        #[inline]
        pub fn remove_hooks(&mut self) {
            self.update_hook(None::<fn(Action, &str, &str, i64)>);
            self.commit_hook(None::<fn() -> bool>);
            self.rollback_hook(None::<fn()>);
            self.progress_handler(0, None::<fn() -> bool>);
        }

        fn commit_hook<'c, F>(&'c mut self, hook: Option<F>)
        where
            F: FnMut() -> bool + Send + 'c,
        {
            unsafe extern "C" fn call_boxed_closure<F>(p_arg: *mut c_void) -> c_int
            where
                F: FnMut() -> bool,
            {
                let r = catch_unwind(|| {
                    let boxed_hook: *mut F = p_arg as *mut F;
                    (*boxed_hook)()
                });
                if let Ok(true) = r {
                    1
                } else {
                    0
                }
            }

            // unlike `sqlite3_create_function_v2`, we cannot specify a `xDestroy` with
            // `sqlite3_commit_hook`. so we keep the `xDestroy` function in
            // `InnerConnection.free_boxed_hook`.
            let free_commit_hook = if hook.is_some() {
                Some(free_boxed_hook::<F> as unsafe fn(*mut c_void))
            } else {
                None
            };

            let previous_hook = match hook {
                Some(hook) => {
                    let boxed_hook: *mut F = Box::into_raw(Box::new(hook));
                    unsafe {
                        ffi::sqlite3_commit_hook(
                            self.db(),
                            Some(call_boxed_closure::<F>),
                            boxed_hook as *mut _,
                        )
                    }
                }
                _ => unsafe { ffi::sqlite3_commit_hook(self.db(), None, ptr::null_mut()) },
            };
            if !previous_hook.is_null() {
                if let Some(free_boxed_hook) = self.free_commit_hook {
                    unsafe { free_boxed_hook(previous_hook) };
                }
            }
            self.free_commit_hook = free_commit_hook;
        }

        fn rollback_hook<'c, F>(&'c mut self, hook: Option<F>)
        where
            F: FnMut() + Send + 'c,
        {
            unsafe extern "C" fn call_boxed_closure<F>(p_arg: *mut c_void)
            where
                F: FnMut(),
            {
                let _ = catch_unwind(|| {
                    let boxed_hook: *mut F = p_arg as *mut F;
                    (*boxed_hook)();
                });
            }

            let free_rollback_hook = if hook.is_some() {
                Some(free_boxed_hook::<F> as unsafe fn(*mut c_void))
            } else {
                None
            };

            let previous_hook = match hook {
                Some(hook) => {
                    let boxed_hook: *mut F = Box::into_raw(Box::new(hook));
                    unsafe {
                        ffi::sqlite3_rollback_hook(
                            self.db(),
                            Some(call_boxed_closure::<F>),
                            boxed_hook as *mut _,
                        )
                    }
                }
                _ => unsafe { ffi::sqlite3_rollback_hook(self.db(), None, ptr::null_mut()) },
            };
            if !previous_hook.is_null() {
                if let Some(free_boxed_hook) = self.free_rollback_hook {
                    unsafe { free_boxed_hook(previous_hook) };
                }
            }
            self.free_rollback_hook = free_rollback_hook;
        }

        fn update_hook<'c, F>(&'c mut self, hook: Option<F>)
        where
            F: FnMut(Action, &str, &str, i64) + Send + 'c,
        {
            unsafe extern "C" fn call_boxed_closure<F>(
                p_arg: *mut c_void,
                action_code: c_int,
                db_str: *const c_char,
                tbl_str: *const c_char,
                row_id: i64,
            ) where
                F: FnMut(Action, &str, &str, i64),
            {
                use std::ffi::CStr;
                use std::str;

                let action = Action::from(action_code);
                let db_name = {
                    let c_slice = CStr::from_ptr(db_str).to_bytes();
                    str::from_utf8(c_slice)
                };
                let tbl_name = {
                    let c_slice = CStr::from_ptr(tbl_str).to_bytes();
                    str::from_utf8(c_slice)
                };

                let _ = catch_unwind(|| {
                    let boxed_hook: *mut F = p_arg as *mut F;
                    (*boxed_hook)(
                        action,
                        db_name.expect("illegal db name"),
                        tbl_name.expect("illegal table name"),
                        row_id,
                    );
                });
            }

            let free_update_hook = if hook.is_some() {
                Some(free_boxed_hook::<F> as unsafe fn(*mut c_void))
            } else {
                None
            };

            let previous_hook = match hook {
                Some(hook) => {
                    let boxed_hook: *mut F = Box::into_raw(Box::new(hook));
                    unsafe {
                        ffi::sqlite3_update_hook(
                            self.db(),
                            Some(call_boxed_closure::<F>),
                            boxed_hook as *mut _,
                        )
                    }
                }
                _ => unsafe { ffi::sqlite3_update_hook(self.db(), None, ptr::null_mut()) },
            };
            if !previous_hook.is_null() {
                if let Some(free_boxed_hook) = self.free_update_hook {
                    unsafe { free_boxed_hook(previous_hook) };
                }
            }
            self.free_update_hook = free_update_hook;
        }

        fn progress_handler<F>(&mut self, num_ops: c_int, handler: Option<F>)
        where
            F: FnMut() -> bool + Send + RefUnwindSafe + 'static,
        {
            unsafe extern "C" fn call_boxed_closure<F>(p_arg: *mut c_void) -> c_int
            where
                F: FnMut() -> bool,
            {
                let r = catch_unwind(|| {
                    let boxed_handler: *mut F = p_arg as *mut F;
                    (*boxed_handler)()
                });
                if let Ok(true) = r {
                    1
                } else {
                    0
                }
            }

            match handler {
                Some(handler) => {
                    let boxed_handler = Box::new(handler);
                    unsafe {
                        ffi::sqlite3_progress_handler(
                            self.db(),
                            num_ops,
                            Some(call_boxed_closure::<F>),
                            &*boxed_handler as *const F as *mut _,
                        )
                    }
                    self.progress_handler = Some(boxed_handler);
                }
                _ => {
                    unsafe {
                        ffi::sqlite3_progress_handler(self.db(), num_ops, None, ptr::null_mut())
                    }
                    self.progress_handler = None;
                }
            };
        }
    }

    #[cfg(test)]
    mod test {
        use super::super::Action;
        use crate::{Connection, Result};
        use std::sync::atomic::{AtomicBool, Ordering};

        #[test]
        fn test_commit_hook() -> Result<()> {
            let db = Connection::open_in_memory()?;

            let mut called = false;
            db.commit_hook(Some(|| {
                called = true;
                false
            }));
            db.execute_batch("BEGIN; CREATE TABLE foo (t TEXT); COMMIT;")?;
            assert!(called);
            Ok(())
        }

        #[test]
        fn test_fn_commit_hook() -> Result<()> {
            let db = Connection::open_in_memory()?;

            fn hook() -> bool {
                true
            }

            db.commit_hook(Some(hook));
            db.execute_batch("BEGIN; CREATE TABLE foo (t TEXT); COMMIT;")
                .unwrap_err();
            Ok(())
        }

        #[test]
        fn test_rollback_hook() -> Result<()> {
            let db = Connection::open_in_memory()?;

            let mut called = false;
            db.rollback_hook(Some(|| {
                called = true;
            }));
            db.execute_batch("BEGIN; CREATE TABLE foo (t TEXT); ROLLBACK;")?;
            assert!(called);
            Ok(())
        }

        #[test]
        fn test_update_hook() -> Result<()> {
            let db = Connection::open_in_memory()?;

            let mut called = false;
            db.update_hook(Some(|action, db: &str, tbl: &str, row_id| {
                assert_eq!(Action::SQLITE_INSERT, action);
                assert_eq!("main", db);
                assert_eq!("foo", tbl);
                assert_eq!(1, row_id);
                called = true;
            }));
            db.execute_batch("CREATE TABLE foo (t TEXT)")?;
            db.execute_batch("INSERT INTO foo VALUES ('lisa')")?;
            assert!(called);
            Ok(())
        }

        #[test]
        fn test_progress_handler() -> Result<()> {
            let db = Connection::open_in_memory()?;

            static CALLED: AtomicBool = AtomicBool::new(false);
            db.progress_handler(
                1,
                Some(|| {
                    CALLED.store(true, Ordering::Relaxed);
                    false
                }),
            );
            db.execute_batch("BEGIN; CREATE TABLE foo (t TEXT); COMMIT;")?;
            assert!(CALLED.load(Ordering::Relaxed));
            Ok(())
        }

        #[test]
        fn test_progress_handler_interrupt() -> Result<()> {
            let db = Connection::open_in_memory()?;

            fn handler() -> bool {
                true
            }

            db.progress_handler(1, Some(handler));
            db.execute_batch("BEGIN; CREATE TABLE foo (t TEXT); COMMIT;")
                .unwrap_err();
            Ok(())
        }
    }
}
