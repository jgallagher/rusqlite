//! Rusqlite is an ergonomic wrapper for using SQLite from Rust. It attempts to expose
//! an interface similar to [rust-postgres](https://github.com/sfackler/rust-postgres).
//!
//! ```rust
//! extern crate rusqlite;
//! extern crate time;
//!
//! use time::Timespec;
//! use rusqlite::SqliteConnection;
//!
//! #[derive(Debug)]
//! struct Person {
//!     id: i32,
//!     name: String,
//!     time_created: Timespec,
//!     data: Option<Vec<u8>>
//! }
//!
//! fn main() {
//!     let conn = SqliteConnection::open_in_memory().unwrap();
//!
//!     conn.execute("CREATE TABLE person (
//!                   id              INTEGER PRIMARY KEY,
//!                   name            TEXT NOT NULL,
//!                   time_created    TEXT NOT NULL,
//!                   data            BLOB
//!                   )", &[]).unwrap();
//!     let me = Person {
//!         id: 0,
//!         name: "Steven".to_string(),
//!         time_created: time::get_time(),
//!         data: None
//!     };
//!     conn.execute("INSERT INTO person (name, time_created, data)
//!                   VALUES ($1, $2, $3)",
//!                  &[&me.name, &me.time_created, &me.data]).unwrap();
//!
//!     let mut stmt = conn.prepare("SELECT id, name, time_created, data FROM person").unwrap();
//!     let mut rows = stmt.query(&[], |row| {
//!         Person {
//!             id: row.get(0),
//!             name: row.get(1),
//!             time_created: row.get(2),
//!             data: row.get(3)
//!         }
//!     }).unwrap();
//!
//!     for person in rows {
//!         println!("Found person {:?}", person);
//!     }
//! }
//! ```
extern crate libc;
extern crate libsqlite3_sys as ffi;
#[macro_use] extern crate bitflags;

use std::mem;
use std::ptr;
use std::fmt;
use std::path::{Path};
use std::error;
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::str;
use libc::{c_int, c_void, c_char};

use types::{ToSql, FromSql};

pub use transaction::{SqliteTransaction};
pub use transaction::{SqliteTransactionBehavior,
                      SqliteTransactionDeferred,
                      SqliteTransactionImmediate,
                      SqliteTransactionExclusive};

#[cfg(feature = "load_extension")] pub use load_extension_guard::{SqliteLoadExtensionGuard};

pub mod types;
mod transaction;
#[cfg(feature = "load_extension")] mod load_extension_guard;

/// A typedef of the result returned by many methods.
pub type SqliteResult<T> = Result<T, SqliteError>;

unsafe fn errmsg_to_string(errmsg: *const c_char) -> String {
    let c_slice = CStr::from_ptr(errmsg).to_bytes();
    let utf8_str = str::from_utf8(c_slice);
    utf8_str.unwrap_or("Invalid string encoding").to_string()
}

/// Encompasses an error result from a call to the SQLite C API.
#[derive(Debug)]
pub struct SqliteError {
    /// The error code returned by a SQLite C API call. See [SQLite Result
    /// Codes](http://www.sqlite.org/rescode.html) for details.
    pub code: c_int,

    /// The error message provided by [sqlite3_errmsg](http://www.sqlite.org/c3ref/errcode.html),
    /// if possible, or a generic error message based on `code` otherwise.
    pub message: String,
}

impl fmt::Display for SqliteError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} (SQLite error {})", self.message, self.code)
    }
}

impl error::Error for SqliteError {
    fn description(&self) -> &str {
        &self.message
    }
}

impl SqliteError {
    fn from_handle(db: *mut ffi::Struct_sqlite3, code: c_int) -> SqliteError {
        let message = if db.is_null() {
            ffi::code_to_str(code).to_string()
        } else {
            unsafe { errmsg_to_string(ffi::sqlite3_errmsg(db)) }
        };
        SqliteError{ code: code, message: message }
    }
}

fn str_to_cstring(s: &str) -> SqliteResult<CString> {
    CString::new(s).map_err(|_| SqliteError{
        code: ffi::SQLITE_MISUSE,
        message: "Could not convert path to C-combatible string".to_string()
    })
}

fn path_to_cstring(p: &Path) -> SqliteResult<CString> {
    let s = try!(p.to_str().ok_or(SqliteError{
        code: ffi::SQLITE_MISUSE,
        message: "Could not convert path to UTF-8 string".to_string()
    }));
    str_to_cstring(s)
}

/// A connection to a SQLite database.
///
/// ## Warning
///
/// Note that despite the fact that most `SqliteConnection` methods take an immutable reference to
/// `self`, `SqliteConnection` is NOT threadsafe, and using it from multiple threads may result in
/// runtime panics or data races. The SQLite connection handle has at least two pieces of internal
/// state (the last insertion ID and the last error message) that rusqlite uses, but wrapping these
/// APIs in a safe way from Rust would be too restrictive (for example, you would not be able to
/// prepare multiple statements at the same time).
pub struct SqliteConnection {
    db: RefCell<InnerSqliteConnection>,
}

unsafe impl Send for SqliteConnection {}

impl SqliteConnection {
    /// Open a new connection to a SQLite database.
    ///
    /// `SqliteConnection::open(path)` is equivalent to `SqliteConnection::open_with_flags(path,
    /// SQLITE_OPEN_READ_WRITE | SQLITE_OPEN_CREATE)`.
    pub fn open<P: AsRef<Path>>(path: &P) -> SqliteResult<SqliteConnection> {
        let flags = SQLITE_OPEN_READ_WRITE | SQLITE_OPEN_CREATE;
        SqliteConnection::open_with_flags(path, flags)
    }

    /// Open a new connection to an in-memory SQLite database.
    pub fn open_in_memory() -> SqliteResult<SqliteConnection> {
        let flags = SQLITE_OPEN_READ_WRITE | SQLITE_OPEN_CREATE;
        SqliteConnection::open_in_memory_with_flags(flags)
    }

    /// Open a new connection to a SQLite database.
    ///
    /// Database Connection](http://www.sqlite.org/c3ref/open.html) for a description of valid
    /// flag combinations.
    pub fn open_with_flags<P: AsRef<Path>>(path: &P, flags: SqliteOpenFlags)
            -> SqliteResult<SqliteConnection> {
        let c_path = try!(path_to_cstring(path.as_ref()));
        InnerSqliteConnection::open_with_flags(&c_path, flags).map(|db| {
            SqliteConnection{ db: RefCell::new(db) }
        })
    }

    /// Open a new connection to an in-memory SQLite database.
    ///
    /// Database Connection](http://www.sqlite.org/c3ref/open.html) for a description of valid
    /// flag combinations.
    pub fn open_in_memory_with_flags(flags: SqliteOpenFlags) -> SqliteResult<SqliteConnection> {
        let c_memory = try!(str_to_cstring(":memory:"));
        InnerSqliteConnection::open_with_flags(&c_memory, flags).map(|db| {
            SqliteConnection{ db: RefCell::new(db) }
        })
    }

    /// Begin a new transaction with the default behavior (DEFERRED).
    ///
    /// The transaction defaults to rolling back when it is dropped. If you want the transaction to
    /// commit, you must call `commit` or `set_commit`.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// # use rusqlite::{SqliteConnection, SqliteResult};
    /// # fn do_queries_part_1(conn: &SqliteConnection) -> SqliteResult<()> { Ok(()) }
    /// # fn do_queries_part_2(conn: &SqliteConnection) -> SqliteResult<()> { Ok(()) }
    /// fn perform_queries(conn: &SqliteConnection) -> SqliteResult<()> {
    ///     let tx = try!(conn.transaction());
    ///
    ///     try!(do_queries_part_1(conn)); // tx causes rollback if this fails
    ///     try!(do_queries_part_2(conn)); // tx causes rollback if this fails
    ///
    ///     tx.commit()
    /// }
    /// ```
    pub fn transaction<'a>(&'a self) -> SqliteResult<SqliteTransaction<'a>> {
        SqliteTransaction::new(self, SqliteTransactionDeferred)
    }

    /// Begin a new transaction with a specified behavior.
    ///
    /// See `transaction`.
    pub fn transaction_with_behavior<'a>(&'a self, behavior: SqliteTransactionBehavior)
            -> SqliteResult<SqliteTransaction<'a>> {
        SqliteTransaction::new(self, behavior)
    }

    /// Convenience method to run multiple SQL statements (that cannot take any parameters).
    ///
    /// Uses [sqlite3_exec](http://www.sqlite.org/c3ref/exec.html) under the hood.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// # use rusqlite::{SqliteConnection, SqliteResult};
    /// fn create_tables(conn: &SqliteConnection) -> SqliteResult<()> {
    ///     conn.execute_batch("BEGIN;
    ///                         CREATE TABLE foo(x INTEGER);
    ///                         CREATE TABLE bar(y TEXT);
    ///                         COMMIT;")
    /// }
    /// ```
    pub fn execute_batch(&self, sql: &str) -> SqliteResult<()> {
        self.db.borrow_mut().execute_batch(sql)
    }

    /// Convenience method to prepare and execute a single SQL statement.
    ///
    /// On success, returns the number of rows that were changed or inserted or deleted (via
    /// `sqlite3_changes`).
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// # use rusqlite::{SqliteConnection};
    /// fn update_rows(conn: &SqliteConnection) {
    ///     match conn.execute("UPDATE foo SET bar = 'baz' WHERE qux = ?", &[&1i32]) {
    ///         Ok(updated) => println!("{} rows were updated", updated),
    ///         Err(err) => println!("update failed: {}", err),
    ///     }
    /// }
    /// ```
    pub fn execute(&self, sql: &str, params: &[&ToSql]) -> SqliteResult<c_int> {
        self.prepare(sql).and_then(|mut stmt| stmt.execute(params))
    }

    /// Get the SQLite rowid of the most recent successful INSERT.
    ///
    /// Uses [sqlite3_last_insert_rowid](https://www.sqlite.org/c3ref/last_insert_rowid.html) under
    /// the hood.
    pub fn last_insert_rowid(&self) -> i64 {
        self.db.borrow_mut().last_insert_rowid()
    }

    /// Convenience method to execute a query that is expected to return a single row.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// # use rusqlite::{SqliteResult,SqliteConnection};
    /// fn preferred_locale(conn: &SqliteConnection) -> SqliteResult<String> {
    ///     conn.query_row("SELECT value FROM preferences WHERE name='locale'", &[], |row| {
    ///         row.get(0)
    ///     })
    /// }
    /// ```
    ///
    /// If the query returns more than one row, all rows except the first are ignored.
    pub fn query_row<T, F>(&self, sql: &str, params: &[&ToSql], f: F) -> SqliteResult<T>
                           where F: FnMut(MappedRow) -> T,
                                 T: 'static {
        let mut stmt = try!(self.prepare(sql));
        let mut rows = try!(stmt.query(params, f));

        rows.next().unwrap_or(
            Err(SqliteError{
                code: ffi::SQLITE_NOTICE,
                message: "Query did not return a row".to_string(),
            }))
    }

    /// Prepare a SQL statement for execution.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// # use rusqlite::{SqliteConnection, SqliteResult};
    /// fn insert_new_people(conn: &SqliteConnection) -> SqliteResult<()> {
    ///     let mut stmt = try!(conn.prepare("INSERT INTO People (name) VALUES (?)"));
    ///     try!(stmt.execute(&[&"Joe Smith"]));
    ///     try!(stmt.execute(&[&"Bob Jones"]));
    ///     Ok(())
    /// }
    /// ```
    pub fn prepare<'a>(&'a self, sql: &str) -> SqliteResult<SqliteStatement<'a>> {
        self.db.borrow_mut().prepare(self, sql)
    }

    /// Close the SQLite connection.
    ///
    /// This is functionally equivalent to the `Drop` implementation for `SqliteConnection` except
    /// that it returns any error encountered to the caller.
    pub fn close(self) -> SqliteResult<()> {
        let mut db = self.db.borrow_mut();
        db.close()
    }

    /// Enable loading of SQLite extensions. Strongly consider using `SqliteLoadExtensionGuard`
    /// instead of this function.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// # use rusqlite::{SqliteConnection, SqliteResult};
    /// # use std::path::{Path};
    /// fn load_my_extension(conn: &SqliteConnection) -> SqliteResult<()> {
    ///     try!(conn.load_extension_enable());
    ///     try!(conn.load_extension(Path::new("my_sqlite_extension"), None));
    ///     conn.load_extension_disable()
    /// }
    /// ```
    #[cfg(feature = "load_extension")]
    pub fn load_extension_enable(&self) -> SqliteResult<()> {
        self.db.borrow_mut().enable_load_extension(1)
    }

    /// Disable loading of SQLite extensions.
    ///
    /// See `load_extension_enable` for an example.
    #[cfg(feature = "load_extension")]
    pub fn load_extension_disable(&self) -> SqliteResult<()> {
        self.db.borrow_mut().enable_load_extension(0)
    }

    /// Load the SQLite extension at `dylib_path`. `dylib_path` is passed through to
    /// `sqlite3_load_extension`, which may attempt OS-specific modifications if the file
    /// cannot be loaded directly.
    ///
    /// If `entry_point` is `None`, SQLite will attempt to find the entry point. If it is not
    /// `None`, the entry point will be passed through to `sqlite3_load_extension`.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// # use rusqlite::{SqliteConnection, SqliteResult, SqliteLoadExtensionGuard};
    /// # use std::path::{Path};
    /// fn load_my_extension(conn: &SqliteConnection) -> SqliteResult<()> {
    ///     let _guard = try!(SqliteLoadExtensionGuard::new(conn));
    ///
    ///     conn.load_extension(Path::new("my_sqlite_extension"), None)
    /// }
    #[cfg(feature = "load_extension")]
    pub fn load_extension<P: AsRef<Path>>(&self, dylib_path: &P, entry_point: Option<&str>) -> SqliteResult<()> {
        self.db.borrow_mut().load_extension(dylib_path, entry_point)
    }

    fn decode_result(&self, code: c_int) -> SqliteResult<()> {
        self.db.borrow_mut().decode_result(code)
    }

    fn changes(&self) -> c_int {
        self.db.borrow_mut().changes()
    }
}

impl fmt::Debug for SqliteConnection {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SqliteConnection()")
    }
}

struct InnerSqliteConnection {
    db: *mut ffi::Struct_sqlite3,
}

bitflags! {
    #[doc = "Flags for opening SQLite database connections."]
    #[doc = "See [sqlite3_open_v2](http://www.sqlite.org/c3ref/open.html) for details."]
    #[repr(C)]
    flags SqliteOpenFlags: c_int {
        const SQLITE_OPEN_READ_ONLY     = 0x00000001,
        const SQLITE_OPEN_READ_WRITE    = 0x00000002,
        const SQLITE_OPEN_CREATE        = 0x00000004,
        const SQLITE_OPEN_URI           = 0x00000040,
        const SQLITE_OPEN_MEMORY        = 0x00000080,
        const SQLITE_OPEN_NO_MUTEX      = 0x00008000,
        const SQLITE_OPEN_FULL_MUTEX    = 0x00010000,
        const SQLITE_OPEN_SHARED_CACHE  = 0x00020000,
        const SQLITE_OPEN_PRIVATE_CACHE = 0x00040000,
    }
}

impl InnerSqliteConnection {
    fn open_with_flags(c_path: &CString, flags: SqliteOpenFlags)
            -> SqliteResult<InnerSqliteConnection> {
        unsafe {
            let mut db: *mut ffi::sqlite3 = mem::uninitialized();
            let r = ffi::sqlite3_open_v2(c_path.as_ptr(), &mut db, flags.bits(), ptr::null());
            if r != ffi::SQLITE_OK {
                let e = if db.is_null() {
                    SqliteError{ code: r,
                                 message: ffi::code_to_str(r).to_string() }
                } else {
                    let e = SqliteError::from_handle(db, r);
                    ffi::sqlite3_close(db);
                    e
                };

                return Err(e);
            }
            let r = ffi::sqlite3_busy_timeout(db, 5000);
            if r != ffi::SQLITE_OK {
                let e = SqliteError::from_handle(db, r);
                ffi::sqlite3_close(db);
                return Err(e);
            }
            Ok(InnerSqliteConnection{ db: db })
        }
    }

    fn db(&self) -> *mut ffi::Struct_sqlite3 {
        self.db
    }

    fn decode_result(&mut self, code: c_int) -> SqliteResult<()> {
        if code == ffi::SQLITE_OK {
            Ok(())
        } else {
            Err(SqliteError::from_handle(self.db(), code))
        }
    }

    unsafe fn decode_result_with_errmsg(&self, code: c_int, errmsg: *mut c_char) -> SqliteResult<()> {
        if code == ffi::SQLITE_OK {
            Ok(())
        } else {
            let message = errmsg_to_string(&*errmsg);
            ffi::sqlite3_free(errmsg as *mut c_void);
            Err(SqliteError{ code: code, message: message })
        }
    }

    fn close(&mut self) -> SqliteResult<()> {
        unsafe {
            let r = ffi::sqlite3_close(self.db());
            self.db = ptr::null_mut();
            self.decode_result(r)
        }
    }

    fn execute_batch(&mut self, sql: &str) -> SqliteResult<()> {
        let c_sql = try!(str_to_cstring(sql));
        unsafe {
            let mut errmsg: *mut c_char = mem::uninitialized();
            let r = ffi::sqlite3_exec(self.db(), c_sql.as_ptr(), None, ptr::null_mut(), &mut errmsg);
            self.decode_result_with_errmsg(r, errmsg)
        }
    }

    #[cfg(feature = "load_extension")]
    fn enable_load_extension(&mut self, onoff: c_int) -> SqliteResult<()> {
        let r = unsafe { ffi::sqlite3_enable_load_extension(self.db, onoff) };
        self.decode_result(r)
    }

    #[cfg(feature = "load_extension")]
    fn load_extension(&self, dylib_path: &Path, entry_point: Option<&str>) -> SqliteResult<()> {
        let dylib_str = try!(path_to_cstring(dylib_path));
        unsafe {
            let mut errmsg: *mut c_char = mem::uninitialized();
            let r = if let Some(entry_point) = entry_point {
                let c_entry = try!(str_to_cstring(entry_point));
                ffi::sqlite3_load_extension(self.db, dylib_str.as_ptr(), c_entry.as_ptr(), &mut errmsg)
            } else {
                ffi::sqlite3_load_extension(self.db, dylib_str.as_ptr(), ptr::null(), &mut errmsg)
            };
            self.decode_result_with_errmsg(r, errmsg)
        }
    }

    fn last_insert_rowid(&self) -> i64 {
        unsafe {
            ffi::sqlite3_last_insert_rowid(self.db())
        }
    }

    fn prepare<'a>(&mut self,
                   conn: &'a SqliteConnection,
                   sql: &str) -> SqliteResult<SqliteStatement<'a>> {
        let mut c_stmt: *mut ffi::sqlite3_stmt = unsafe { mem::uninitialized() };
        let c_sql = try!(str_to_cstring(sql));
        let r = unsafe {
            let len_with_nul = (sql.len() + 1) as c_int;
            ffi::sqlite3_prepare_v2(self.db(), c_sql.as_ptr(), len_with_nul, &mut c_stmt,
                                    ptr::null_mut())
        };
        self.decode_result(r).map(|_| {
            SqliteStatement::new(conn, c_stmt)
        })
    }

    fn changes(&mut self) -> c_int {
        unsafe{ ffi::sqlite3_changes(self.db()) }
    }
}

impl Drop for InnerSqliteConnection {
    #[allow(unused_must_use)]
    fn drop(&mut self) {
        self.close();
    }
}

/// A prepared statement.
pub struct SqliteStatement<'conn> {
    conn: &'conn SqliteConnection,
    stmt: *mut ffi::sqlite3_stmt,
    needs_reset: bool,
}

impl<'conn> SqliteStatement<'conn> {
    fn new(conn: &SqliteConnection, stmt: *mut ffi::sqlite3_stmt) -> SqliteStatement {
        SqliteStatement{ conn: conn, stmt: stmt, needs_reset: false }
    }

    /// Execute the prepared statement.
    ///
    /// On success, returns the number of rows that were changed or inserted or deleted (via
    /// `sqlite3_changes`).
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// # use rusqlite::{SqliteConnection, SqliteResult};
    /// fn update_rows(conn: &SqliteConnection) -> SqliteResult<()> {
    ///     let mut stmt = try!(conn.prepare("UPDATE foo SET bar = 'baz' WHERE qux = ?"));
    ///
    ///     try!(stmt.execute(&[&1i32]));
    ///     try!(stmt.execute(&[&2i32]));
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn execute(&mut self, params: &[&ToSql]) -> SqliteResult<c_int> {
        self.reset_if_needed();

        unsafe {
            assert!(params.len() as c_int == ffi::sqlite3_bind_parameter_count(self.stmt),
                    "incorrect number of parameters to execute(): expected {}, got {}",
                    ffi::sqlite3_bind_parameter_count(self.stmt),
                    params.len());

            for (i, p) in params.iter().enumerate() {
                try!(self.conn.decode_result(p.bind_parameter(self.stmt, (i + 1) as c_int)));
            }

            self.needs_reset = true;
            let r = ffi::sqlite3_step(self.stmt);
            match r {
                ffi::SQLITE_DONE => Ok(self.conn.changes()),
                ffi::SQLITE_ROW => Err(SqliteError{ code: r,
                    message: "Unexpected row result - did you mean to call query?".to_string() }),
                _ => Err(self.conn.decode_result(r).unwrap_err()),
            }
        }
    }

    /// Execute the prepared statement, returning an iterator over the resulting rows.
    pub fn query<'a, 'map, T, F>(&'a mut self, params: &[&ToSql], f: F)
                                 -> SqliteResult<MappedRows<'a, F>>
                                 where T: 'static,
                                       F: FnMut(MappedRow) -> T {
        self.reset_if_needed();
        try!(self.bind_parameters(params));

        Ok(MappedRows { stmt: self, map: f })
    }

    /// Consumes the statement.
    ///
    /// Functionally equivalent to the `Drop` implementation, but allows callers to see any errors
    /// that occur.
    pub fn finalize(mut self) -> SqliteResult<()> {
        self.finalize_()
    }

    fn bind_parameters(&mut self, params: &[&ToSql]) -> SqliteResult<()> {
        unsafe {
            assert!(params.len() as c_int == ffi::sqlite3_bind_parameter_count(self.stmt),
                    "incorrect number of parameters to query(): expected {}, got {}",
                    ffi::sqlite3_bind_parameter_count(self.stmt),
                    params.len());

            for (i, p) in params.iter().enumerate() {
                try!(self.conn.decode_result(p.bind_parameter(self.stmt, (i + 1) as c_int)));
            }
        }

        self.needs_reset = true;

        Ok(())
    }

    fn reset_if_needed(&mut self) {
        if self.needs_reset {
            unsafe { ffi::sqlite3_reset(self.stmt); };
            self.needs_reset = false;
        }
    }

    fn finalize_(&mut self) -> SqliteResult<()> {
        let r = unsafe { ffi::sqlite3_finalize(self.stmt) };
        self.stmt = ptr::null_mut();
        self.conn.decode_result(r)
    }
}

impl<'conn> fmt::Debug for SqliteStatement<'conn> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Statement( conn: {:?}, stmt: {:?} )", self.conn, self.stmt)
    }
}

impl<'conn> Drop for SqliteStatement<'conn> {
    #[allow(unused_must_use)]
    fn drop(&mut self) {
        self.finalize_();
    }
}

pub struct MappedRows<'stmt, F> {
    stmt: &'stmt SqliteStatement<'stmt>,
    map: F
}

impl<'stmt, T, F> Iterator for MappedRows<'stmt, F>
    where F: FnMut(MappedRow) -> T,
          T: 'static {
    type Item = SqliteResult<T>;

    fn next(&mut self) -> Option<SqliteResult<T>> {
        match unsafe { ffi::sqlite3_step(self.stmt.stmt) } {
            ffi::SQLITE_ROW => {
                let result = (self.map)(MappedRow(self.stmt));
                Some(Ok(result))
            },
            ffi::SQLITE_DONE => None,
            code => {
                Some(Err(self.stmt.conn.decode_result(code).unwrap_err()))
            }
        }
    }
}

/// An iterator over the resulting rows of a query.
pub struct MappedRow<'stmt>(&'stmt SqliteStatement<'stmt>);

impl<'stmt> MappedRow<'stmt> {
    /// Get the value of a particular column of the result row.
    ///
    /// ## Failure
    ///
    /// Can panic.
    pub fn get<'a, T: FromSql<'a>>(&'a self, idx: c_int) -> T {
        self.get_opt(idx).unwrap()
    }

    /// Attempt to get the value of a particular column of the result row.
    pub fn get_opt<'a, T: FromSql<'a>>(&'a self, idx: c_int) -> SqliteResult<T> {
        // Do assertions because these are logic errors.
        // We can probably skip them in release builds.
        assert!(idx >= 0);
        assert!(idx < unsafe { ffi::sqlite3_column_count(self.0.stmt) });

        FromSql::column_result(self, idx)
    }
}

#[cfg(test)]
mod test {
    extern crate libsqlite3_sys as ffi;
    extern crate tempdir;
    use super::*;
    use self::tempdir::TempDir;

    // this function is never called, but is still type checked; in
    // particular, calls with specific instantiations will require
    // that those types are `Send`.
    #[allow(dead_code, unconditional_recursion)]
    fn ensure_send<T: Send>() {
        ensure_send::<SqliteConnection>();
    }

    fn checked_memory_handle() -> SqliteConnection {
        SqliteConnection::open_in_memory().unwrap()
    }

    #[test]
    fn test_persistence() {
        let temp_dir = TempDir::new("test_open_file").unwrap();
        let path = temp_dir.path().join("test.db3");

        {
            let db = SqliteConnection::open(&path).unwrap();
            let sql = "BEGIN;
                   CREATE TABLE foo(x INTEGER);
                   INSERT INTO foo VALUES(42);
                   END;";
            db.execute_batch(sql).unwrap();
        }

        let path_string = path.to_str().unwrap();
        let db = SqliteConnection::open(&path_string).unwrap();
        let the_answer = db.query_row("SELECT x FROM foo",
                                      &[],
                                      |r| r.get::<i64>(0));

        assert_eq!(42i64, the_answer.unwrap());
    }

    #[test]
    fn test_open() {
        assert!(SqliteConnection::open_in_memory().is_ok());

        let db = checked_memory_handle();
        assert!(db.close().is_ok());
    }

    #[test]
    fn test_open_with_flags() {
        for bad_flags in [
            SqliteOpenFlags::empty(),
            SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_READ_WRITE,
            SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_CREATE,
        ].iter() {
            assert!(SqliteConnection::open_in_memory_with_flags(*bad_flags).is_err());
        }
    }

    #[test]
    fn test_execute_batch() {
        let db = checked_memory_handle();
        let sql = "BEGIN;
                   CREATE TABLE foo(x INTEGER);
                   INSERT INTO foo VALUES(1);
                   INSERT INTO foo VALUES(2);
                   INSERT INTO foo VALUES(3);
                   INSERT INTO foo VALUES(4);
                   END;";
        db.execute_batch(sql).unwrap();

        db.execute_batch("UPDATE foo SET x = 3 WHERE x < 3").unwrap();

        assert!(db.execute_batch("INVALID SQL").is_err());
    }

    #[test]
    fn test_execute() {
        let db = checked_memory_handle();
        db.execute_batch("CREATE TABLE foo(x INTEGER)").unwrap();

        assert_eq!(db.execute("INSERT INTO foo(x) VALUES (?)", &[&1i32]).unwrap(), 1);
        assert_eq!(db.execute("INSERT INTO foo(x) VALUES (?)", &[&2i32]).unwrap(), 1);

        assert_eq!(3i32, db.query_row("SELECT SUM(x) FROM foo", &[], |r| r.get(0)).unwrap());
    }

    #[test]
    fn test_prepare_execute() {
        let db = checked_memory_handle();
        db.execute_batch("CREATE TABLE foo(x INTEGER);").unwrap();

        let mut insert_stmt = db.prepare("INSERT INTO foo(x) VALUES(?)").unwrap();
        assert_eq!(insert_stmt.execute(&[&1i32]).unwrap(), 1);
        assert_eq!(insert_stmt.execute(&[&2i32]).unwrap(), 1);
        assert_eq!(insert_stmt.execute(&[&3i32]).unwrap(), 1);

        assert_eq!(insert_stmt.execute(&[&"hello".to_string()]).unwrap(), 1);
        assert_eq!(insert_stmt.execute(&[&"goodbye".to_string()]).unwrap(), 1);
        assert_eq!(insert_stmt.execute(&[&types::Null]).unwrap(), 1);

        let mut update_stmt = db.prepare("UPDATE foo SET x=? WHERE x<?").unwrap();
        assert_eq!(update_stmt.execute(&[&3i32, &3i32]).unwrap(), 2);
        assert_eq!(update_stmt.execute(&[&3i32, &3i32]).unwrap(), 0);
        assert_eq!(update_stmt.execute(&[&8i32, &8i32]).unwrap(), 3);
    }

    #[test]
    fn test_prepare_query() {
        let db = checked_memory_handle();
        db.execute_batch("CREATE TABLE foo(x INTEGER);").unwrap();

        let mut insert_stmt = db.prepare("INSERT INTO foo(x) VALUES(?)").unwrap();
        assert_eq!(insert_stmt.execute(&[&1i32]).unwrap(), 1);
        assert_eq!(insert_stmt.execute(&[&2i32]).unwrap(), 1);
        assert_eq!(insert_stmt.execute(&[&3i32]).unwrap(), 1);

        let mut query = db.prepare("SELECT x FROM foo WHERE x < ? ORDER BY x DESC").unwrap();
        {
            let v: SqliteResult<Vec<i32>> = query.query(&[&4i32], |r| r.get(0))
                                                 .unwrap()
                                                 .collect();

            assert_eq!(&[3i32, 2, 1][..], &v.unwrap()[..]);
        }

        {
            let v: SqliteResult<Vec<i32>> = query.query(&[&3i32], |r| r.get(0))
                                                 .unwrap()
                                                 .collect();

            assert_eq!(&[2i32, 1][..], &v.unwrap()[..]);
        }
    }

    #[test]
    fn test_query_map() {
        let db = checked_memory_handle();
        let sql = "BEGIN;
                   CREATE TABLE foo(x INTEGER, y TEXT);
                   INSERT INTO foo VALUES(4, \"hello\");
                   INSERT INTO foo VALUES(3, \", \");
                   INSERT INTO foo VALUES(2, \"world\");
                   INSERT INTO foo VALUES(1, \"!\");
                   END;";
        db.execute_batch(sql).unwrap();

        let mut query = db.prepare("SELECT x, y FROM foo ORDER BY x DESC").unwrap();
        let results: SqliteResult<Vec<String>> = query.query(&[], |row| row.get(1)).unwrap().collect();

        assert_eq!(results.unwrap().concat(), "hello, world!");
    }

    #[test]
    fn test_query_row() {
        let db = checked_memory_handle();
        let sql = "BEGIN;
                   CREATE TABLE foo(x INTEGER);
                   INSERT INTO foo VALUES(1);
                   INSERT INTO foo VALUES(2);
                   INSERT INTO foo VALUES(3);
                   INSERT INTO foo VALUES(4);
                   END;";
        db.execute_batch(sql).unwrap();

        assert_eq!(10i64, db.query_row("SELECT SUM(x) FROM foo", &[], |r| {
            r.get::<i64>(0)
        }).unwrap());

        let result = db.query_row("SELECT x FROM foo WHERE x > 5", &[], |r| r.get::<i64>(0));
        let error = result.unwrap_err();

        assert!(error.code == ffi::SQLITE_NOTICE);
        assert!(error.message == "Query did not return a row");

        let bad_query_result = db.query_row("NOT A PROPER QUERY; test123", &[], |_| ());

        assert!(bad_query_result.is_err());
    }

    #[test]
    fn test_prepare_failures() {
        let db = checked_memory_handle();
        db.execute_batch("CREATE TABLE foo(x INTEGER);").unwrap();

        let err = db.prepare("SELECT * FROM does_not_exist").unwrap_err();
        assert!(err.message.contains("does_not_exist"));
    }

    #[test]
    fn test_last_insert_rowid() {
        let db = checked_memory_handle();
        db.execute_batch("CREATE TABLE foo(x INTEGER PRIMARY KEY)").unwrap();
        db.execute_batch("INSERT INTO foo DEFAULT VALUES").unwrap();

        assert_eq!(db.last_insert_rowid(), 1);

        let mut stmt = db.prepare("INSERT INTO foo DEFAULT VALUES").unwrap();
        for _ in 0i32 .. 9 {
            stmt.execute(&[]).unwrap();
        }
        assert_eq!(db.last_insert_rowid(), 10);
    }
}
