//! `feature = "deserialize"` Serialize and deserialize interfaces, to use a `Vec<u8>`
//! as in-memory database file.
//!
//! This API is only available when SQLite is compiled with `SQLITE_ENABLE_DESERIALIZE`.
//! These functions create and read a serialized file in-memory, useful on platforms without
//! a real file system like WebAssembly or Cloud Functions. Another use case would be to encrypt
//! and decrypt databases in-memory.
//!
//! These methods are added to `Connection`:
//! [`Connection::serialize`], [`Connection::deserialize`], [`Connection::serialize_get_mut`].
//!
//! For large in-memory database files, you probably don't want to copy or reallocate
//! because that would temporarily double the required memory. Use the [`BorrowingConnection`]
//! methods to serialize and deserialize borrowed memory.
//!
//! ```
//! # use rusqlite::{Result, Connection, DatabaseName};
//! # fn main() -> Result<()> {
//! let db = Connection::open_in_memory()?;
//! db.execute_batch("CREATE TABLE one(x INTEGER);INSERT INTO one VALUES(44)")?;
//! let mem_file: Vec<u8> = db.serialize(DatabaseName::Main)?;
//! // This SQLite file could be send over the network and deserialized on the other side
//! // without touching the file system.
//! let mut db_clone = Connection::open_in_memory()?;
//! db_clone.deserialize(DatabaseName::Main, mem_file)?;
//! let row: u16 = db_clone.query_row("SELECT x FROM one", [], |r| r.get(0))?;
//! assert_eq!(44, row);
//! # Ok(())
//! # }
//! ```
//!
//! Alternatively, consider using the [Backup API](../backup/)
//! or the [`VACUUM INTO`](https://www.sqlite.org/lang_vacuum.html) command.

use std::marker::PhantomData;
use std::os::raw::{c_char, c_int, c_void};
use std::{alloc, borrow::Cow, convert::TryInto, fmt, mem, ops, panic, ptr, sync::Arc};

use crate::inner_connection::InnerConnection;
use crate::util::SmallCString;
use crate::{error, ffi, Connection, DatabaseName, OpenFlags, Result};

impl Connection {
    /// Disconnects from database and reopen as an in-memory database based on `Vec<u8>`.
    /// The vector should contain serialized database content (a main database file).
    /// This returns an error if `DatabaseName` was not attached or other failures occurred.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rusqlite::*;
    /// # fn main() -> Result<()> {
    /// # let db = Connection::open_in_memory()?;
    /// db.execute_batch("CREATE TABLE numbers(x INTEGER); ATTACH ':memory:' AS foo")?;
    /// db.deserialize(DatabaseName::Attached("foo"), db.serialize(DatabaseName::Main)?)?;
    /// db.execute_batch("INSERT INTO foo.numbers VALUES(74)")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn deserialize(&self, db: DatabaseName, data: Vec<u8>) -> Result<()> {
        self.deserialize_vec_db(db, MemFile::Owned(data))
    }

    /// Copies the serialization of a database to a `Vec<u8>`. Errors when
    /// `DatabaseName` does not exist or SQLite fails to read from the db.
    ///
    /// For an ordinary on-disk database file, the serialization is just a copy of the disk file.
    /// For an in-memory database or a `TEMP` database, the serialization is the same sequence of
    /// bytes which would be written to disk if that database where backed up to disk.
    ///
    /// If the database was created by one of the deserialize functions, consider
    /// [`BorrowingConnection::serialize_rc`] to read the serialization without copying.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rusqlite::{Connection, DatabaseName};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let path = "serialize-test.db";
    /// let db = Connection::open(path)?;
    /// db.execute_batch("CREATE TABLE foo(x INTEGER);")?;
    /// let mem_file: Vec<u8> = db.serialize(DatabaseName::Main)?;
    /// assert_eq!(mem_file, std::fs::read(path)?);
    /// # db.close().unwrap();
    /// # std::fs::remove_file(path)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn serialize(&self, db_name: DatabaseName) -> Result<Vec<u8>> {
        let schema = db_name.to_cstring()?;
        let file = file_ptr(&self.db.borrow(), &schema).ok_or_else(|| err_not_found(&schema))?;
        if let Some(vec_db) = VecDbFile::try_cast(file) {
            return Ok(vec_db.data.as_slice().to_vec());
        }

        // sqlite3_serialize is not used because it always uses the sqlite3_malloc allocator,
        // while this function returns a Vec<u8>.

        // Query the database size with pragma to allocate a vector.
        let schema_str = schema.as_str();
        let escaped = if schema_str.contains('\'') {
            Cow::Owned(schema_str.replace("'", "''"))
        } else {
            Cow::Borrowed(schema_str)
        };
        let sql = &format!(
            "SELECT page_count * page_size FROM pragma_page_count('{0}'), pragma_page_size('{0}')",
            escaped
        );
        let db_size: i64 = self.query_row(sql, [], |r| r.get(0))?;
        let db_size = db_size.try_into().unwrap();
        if db_size == 0 {
            return Ok(Vec::new());
        }
        let mut vec = Vec::with_capacity(db_size);

        // Unfortunately, sqlite3PagerGet and sqlite3PagerGetData are private APIs,
        // so the Backup API is used instead.
        backup_to_vec(&mut vec, self, db_name)?;
        assert_eq!(vec.len(), db_size, "serialize backup size mismatch");

        Ok(vec)
    }

    /// Returns a mutable reference into the `MemFile` attached as `DatabaseName`,
    /// or returns `None` if no in-memory file is present or if there are other
    /// `Arc` or `Weak` pointers to the same in-memory file.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rusqlite::{*, deserialize::MemFile};
    /// # fn main() -> Result<()> {
    /// let mut db = Connection::open_in_memory()?;
    /// db.deserialize(DatabaseName::Main, db.serialize(DatabaseName::Main)?)?;
    /// match db.serialize_get_mut(DatabaseName::Main).unwrap() {
    ///     MemFile::Owned(vec) => vec.shrink_to_fit(),
    ///     _ => unreachable!(),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn serialize_get_mut(&mut self, db: DatabaseName) -> Option<&mut MemFile> {
        let c = self.db.borrow_mut();
        let file = file_ptr(&c, &db.to_cstring().ok()?)?;
        let vec_db = VecDbFile::try_cast(file)?;
        if vec_db.memory_mapped == 0 && vec_db.lock == 0 {
            Arc::get_mut(&mut vec_db.data)
        } else {
            None
        }
    }

    /// Gets the size limit of an attached in-memory database.
    /// This upper bound is initially set to the `SQLITE_MEMDB_DEFAULT_MAXSIZE`
    /// compile-time option (default 1073741824).
    ///
    /// # Examples
    ///
    /// ```
    /// # use rusqlite::{*, deserialize::MemFile};
    /// # fn main() -> Result<()> {
    /// let mut db = Connection::open_in_memory()?;
    /// db.deserialize(DatabaseName::Main, db.serialize(DatabaseName::Main)?)?;
    /// assert_eq!(db.serialize_size_limit(DatabaseName::Main)?, 1_073_741_824);
    /// assert_eq!(db.serialize_set_size_limit(DatabaseName::Main, 2_000_000)?, 2_000_000);
    /// assert_eq!(db.serialize_size_limit(DatabaseName::Main)?, 2_000_000);
    /// # Ok(())
    /// # }
    /// ```
    pub fn serialize_size_limit(&self, db: DatabaseName) -> Result<usize> {
        let schema = &db.to_cstring()?;
        let file = file_ptr(&self.db.borrow(), schema).ok_or_else(|| err_not_found(schema))?;
        file_size_limit(file, -1).map(|s| s.try_into().unwrap())
    }

    /// Sets the size limit of an attached in-memory database to the larger of the argument
    /// and the current size. Returns the new size limit.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rusqlite::{*, deserialize::MemFile};
    /// # fn main() -> Result<()> {
    /// let mut db = Connection::open_in_memory()?;
    /// db.deserialize(DatabaseName::Main, db.serialize(DatabaseName::Main)?)?;
    /// assert_eq!(db.serialize_set_size_limit(DatabaseName::Main, 0)?, 0);
    /// let sql = "CREATE TABLE foo(x INTEGER)";
    /// db.execute_batch(sql).unwrap_err();
    /// assert_eq!(db.serialize_set_size_limit(DatabaseName::Main, 144_000)?, 144_000);
    /// db.execute_batch(sql).unwrap();
    /// # Ok(())
    /// # }
    /// ```
    pub fn serialize_set_size_limit(&self, db: DatabaseName, size_max: usize) -> Result<usize> {
        let schema = &db.to_cstring()?;
        let file = file_ptr(&self.db.borrow(), schema).ok_or_else(|| err_not_found(schema))?;
        file_size_limit(file, size_max.try_into().unwrap()).map(|s| s.try_into().unwrap())
    }

    /// Wraps the `Connection` in [`BorrowingConnection`] to serialize and
    /// deserialize within the lifetime of a connection.
    pub fn into_borrowing<'a>(self) -> BorrowingConnection<'a> {
        BorrowingConnection {
            conn: self,
            phantom: PhantomData,
        }
    }

    /// Deserialize using a [`VecDbFile`].
    /// # Safety
    /// The caller must make sure that `data` outlives the connection.
    fn deserialize_vec_db(&self, schema: DatabaseName, data: MemFile) -> Result<()> {
        let schema = schema.to_cstring()?;
        let mut c = self.db.borrow_mut();
        unsafe {
            let rc = ffi::sqlite3_deserialize(c.db(), schema.as_ptr(), ptr::null_mut(), 0, 0, 0);
            c.decode_result(rc)?;
            let file = file_ptr(&c, &schema).unwrap();
            assert_eq!(file.pMethods, *MEMDB_IO_METHODS);
            let size_max = file_size_limit(file, -1)?.try_into().unwrap();
            let file = file as *mut _ as _;
            ptr::write(file, VecDbFile::new(data, size_max));
            Ok(())
        }
    }
}

fn err_not_found(db_name: &SmallCString) -> error::Error {
    error::error_from_sqlite_code(
        1,
        Some(format!("database {:?} not found", db_name.as_str())),
    )
}

fn backup_to_vec(vec: &mut Vec<u8>, src: &Connection, db_name: DatabaseName<'_>) -> Result<()> {
    let mut temp_db = Connection::open_with_flags_and_vfs("0", OpenFlags::default(), "memdb")?;
    unsafe {
        let temp_file = file_ptr(&temp_db.db.borrow_mut(), &SmallCString::new("main")?).unwrap();
        assert_eq!(temp_file.pMethods, *MEMDB_IO_METHODS);
        // At this point, MemFile->aData is null
        ptr::write(
            temp_file as *mut _ as _,
            VecDbFile::new(MemFile::Writable(vec), 0),
        );
    };

    use crate::backup::{
        Backup,
        StepResult::{Busy, Done, Locked, More},
    };
    let backup = Backup::new_with_names(src, db_name, &mut temp_db, DatabaseName::Main)?;
    let mut r = More;
    while r == More {
        r = backup.step(100)?;
    }
    match r {
        Done => Ok(()),
        Busy => Err(unsafe { error::error_from_handle(ptr::null_mut(), ffi::SQLITE_BUSY) }),
        Locked => Err(unsafe { error::error_from_handle(ptr::null_mut(), ffi::SQLITE_LOCKED) }),
        More => unreachable!(),
    }
}

/// Wrapper around [`Connection`] with lifetime constraint to serialize/deserialize
/// borrowed memory, returned from [`Connection::into_borrowing`].
pub struct BorrowingConnection<'a> {
    conn: Connection,
    // This RefCell phantom protects against user-after-frees.
    phantom: PhantomData<std::cell::RefCell<&'a [u8]>>,
}

impl<'a> BorrowingConnection<'a> {
    /// Obtains a reference counted serialization of a database, or returns `None` when
    /// [`DatabaseName`] does not exist or no in-memory file is present. The database is
    /// read-only while there are `Arc` or `Weak` pointers. Once the database is detached
    /// or closed, the strong reference count held by this connection is released.
    ///
    /// # Examples
    ///
    /// ```
    /// # use {std::sync::Arc, rusqlite::{*, deserialize::MemFile}};
    /// # fn main() -> Result<()> {
    /// let mut db = Connection::open_in_memory()?.into_borrowing();
    /// let name = DatabaseName::Attached("foo");
    /// db.execute_batch("ATTACH ':memory:' AS foo")?;
    /// db.deserialize(name, db.serialize(DatabaseName::Main)?)?;
    /// let sql = "CREATE TABLE foo.bar(x INTEGER)";
    /// {
    ///     let mem_file: Arc<MemFile> = db.serialize_rc(name).unwrap();
    ///     db.execute_batch(sql).expect_err("locked by Arc");
    /// }
    /// db.execute_batch(sql)?;
    /// let mem_file = db.serialize_rc(name).unwrap();
    /// assert!(mem_file.starts_with(b"SQLite format 3\0")); // Deref coercion to &[u8]
    /// assert_eq!(Arc::strong_count(&mem_file), 2);
    /// db.execute_batch("DETACH DATABASE foo")?;
    /// let _: Vec<u8> = match Arc::try_unwrap(mem_file) {
    ///     Ok(MemFile::Owned(v)) => v,
    ///     _ => panic!(),
    /// };
    /// # Ok(())
    /// # }
    /// ```
    pub fn serialize_rc(&self, db: DatabaseName) -> Option<Arc<MemFile<'a>>> {
        let c = self.conn.db.borrow_mut();
        let file = file_ptr(&c, &db.to_cstring().ok()?)?;
        VecDbFile::try_cast(file).map(|v| Arc::clone(&v.data))
    }

    /// Disconnects database and reopens it as an read-only in-memory database
    /// based on a slice. Errors when the `DatabaseName` is not attached.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rusqlite::*;
    /// # fn main() -> Result<()> {
    /// # let mut db = Connection::open_in_memory()?.into_borrowing();
    /// db.execute_batch("CREATE TABLE foo(x INTEGER); INSERT INTO foo VALUES(1)")?;
    /// let vec: Vec<u8> = db.serialize(DatabaseName::Main)?;
    /// db.deserialize_read_only(DatabaseName::Main, &vec)?;
    /// let count: u32 = db.query_row("SELECT COUNT(*) FROM foo", [], |r| r.get(0))?;
    /// assert!(count > 0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn deserialize_read_only(&self, db: DatabaseName, slice: &'a [u8]) -> Result<()> {
        self.deserialize_vec_db(db, MemFile::ReadOnly(slice))
    }

    /// Disconnects database and reopens it as an in-memory database based
    /// on a borrowed vector. Errors when the `DatabaseName` is not attached.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rusqlite::*;
    /// # fn main() -> Result<()> {
    /// # let mut db = Connection::open_in_memory()?.into_borrowing();
    /// let mut vec: Vec<u8> = db.serialize(DatabaseName::Main)?;
    /// db.deserialize_writable(DatabaseName::Main, &mut vec)?;
    /// db.execute_batch("CREATE TABLE foo(x INTEGER);")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn deserialize_writable(&self, db: DatabaseName, vec: &'a mut Vec<u8>) -> Result<()> {
        self.deserialize_vec_db(db, MemFile::Writable(vec))
    }

    /// Returns the wrapped connection.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rusqlite::*;
    /// # fn main() -> Result<()> {
    /// let mut db = Connection::open_in_memory()?.into_borrowing();
    /// let db: Connection = db.into_inner();
    /// db.close().unwrap();
    /// # Ok(())
    /// # }
    /// ```
    pub fn into_inner(self) -> Connection {
        self.conn
    }
}

impl ops::Deref for BorrowingConnection<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl ops::DerefMut for BorrowingConnection<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

impl fmt::Debug for BorrowingConnection<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BorrowingConnection")
            .field("conn", &self.conn)
            .finish()
    }
}

/// Byte array storing an in-memory database file, implementing `Deref<Target=[u8]>`.
///
/// The vector grows as needed, but is not automatically shrunk when the
/// database file is reduced in size.
#[non_exhaustive]
#[derive(PartialEq)]
pub enum MemFile<'a> {
    /// Owned vector.
    Owned(Vec<u8>),
    /// Mutably borrowed vector that can be written and resized.
    Writable(&'a mut Vec<u8>),
    /// Immutably borrowed slice for a read-only database.
    ReadOnly(&'a [u8]),
}

impl MemFile<'_> {
    fn as_slice(&self) -> &[u8] {
        match self {
            MemFile::Owned(d) => d,
            MemFile::Writable(d) => d,
            MemFile::ReadOnly(d) => d,
        }
    }

    fn get_mut_vec(this: &mut Arc<Self>) -> Option<&mut Vec<u8>> {
        match Arc::get_mut(this)? {
            MemFile::Owned(d) => Some(d),
            MemFile::Writable(d) => Some(*d),
            MemFile::ReadOnly(_) => None,
        }
    }
}

impl ops::Deref for MemFile<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

/// Print the fields of the vector/slice instead of the elements
/// because in-memory database are so large.
impl fmt::Debug for MemFile<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owned(v) => f
                .debug_struct("Owned")
                .field("ptr", &v.as_ptr())
                .field("length", &v.len())
                .field("capacity", &v.capacity())
                .finish(),
            Self::Writable(v) => f
                .debug_struct("Writable")
                .field("ptr", &v.as_ptr())
                .field("length", &v.len())
                .field("capacity", &v.capacity())
                .finish(),
            Self::ReadOnly(v) => f
                .debug_struct("ReadOnly")
                .field("ptr", &v.as_ptr())
                .field("length", &v.len())
                .finish(),
        }
    }
}

/// `sqlite3_file` subclass for the `vec_db` Virtual File System. It's inspired by
/// the `memdb` VFS but uses the Rust allocator instead of `sqlite3_malloc`.
/// The database is stored in an owned or borrowed `Vec<u8>`.
#[repr(C)]
#[derive(Debug)]
struct VecDbFile<'a> {
    methods: *const ffi::sqlite3_io_methods,
    data: Arc<MemFile<'a>>,
    size_max: usize,
    memory_mapped: u16,
    lock: u8,
}

impl<'a> VecDbFile<'a> {
    fn new(data: MemFile<'a>, size_max: usize) -> Self {
        VecDbFile {
            methods: &VEC_DB_IO_METHODS,
            data: Arc::new(data),
            size_max,
            memory_mapped: 0,
            lock: 0,
        }
    }

    fn try_cast(file: &mut ffi::sqlite3_file) -> Option<&mut Self> {
        if file.pMethods == &VEC_DB_IO_METHODS {
            unsafe { Some(&mut *(file as *mut _ as *mut VecDbFile)) }
        } else {
            None
        }
    }
}

/// IO Methods for the `vec_db` Virtual File System.
/// This can't be a const because the pointers are compared.
static VEC_DB_IO_METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 3,
    xClose: Some(x_close),
    xRead: Some(x_read),
    xWrite: Some(x_write),
    xTruncate: Some(x_truncate),
    xSync: Some(x_sync),
    xFileSize: Some(x_size),
    xLock: Some(x_lock),
    xUnlock: Some(x_lock),
    xCheckReservedLock: None,
    xFileControl: Some(x_file_control),
    xSectorSize: None,
    xDeviceCharacteristics: Some(x_device_characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: Some(x_fetch),
    xUnfetch: Some(x_unfetch),
};

lazy_static::lazy_static! {
    /// Get `memdb_io_methods` and `szOsFile` for the VFS defined in `memdb.c`
    static ref MEMDB_IO_METHODS: &'static ffi::sqlite3_io_methods = unsafe {
        let vfs = &mut *ffi::sqlite3_vfs_find("memdb\0".as_ptr() as _);
        let sz = vfs.szOsFile as usize;
        assert!(mem::size_of::<VecDbFile>() <= sz, "VecDbFile doesn't fit in allocation");
        let layout = alloc::Layout::from_size_align(sz, mem::align_of::<ffi::sqlite3_file>()).unwrap();
        #[allow(clippy::cast_ptr_alignment)]
        let file = alloc::alloc(layout) as *mut ffi::sqlite3_file;
        let rc = vfs.xOpen.unwrap()(vfs, ptr::null(), file, ffi::SQLITE_OPEN_MAIN_DB, &mut 0);
        assert_eq!(rc, ffi::SQLITE_OK);
        let methods = (*file).pMethods;
        assert!(!methods.is_null());
        alloc::dealloc(file as _, layout);
        &*methods
    };
}

fn file_ptr<'a>(c: &InnerConnection, schema: &SmallCString) -> Option<&'a mut ffi::sqlite3_file> {
    unsafe {
        let mut file = ptr::null_mut::<ffi::sqlite3_file>();
        let rc = ffi::sqlite3_file_control(
            c.db(),
            schema.as_ptr(),
            ffi::SQLITE_FCNTL_FILE_POINTER,
            &mut file as *mut _ as _,
        );
        if rc != ffi::SQLITE_OK || file.is_null() {
            None
        } else {
            Some(&mut *file)
        }
    }
}

fn file_size_limit(file: &mut ffi::sqlite3_file, mut size_max: i64) -> Result<i64> {
    unsafe {
        let rc = (*file.pMethods).xFileControl.unwrap()(
            file,
            ffi::SQLITE_FCNTL_SIZE_LIMIT,
            &mut size_max as *mut _ as _,
        );
        if rc == ffi::SQLITE_OK {
            Ok(size_max)
        } else {
            let message = Some("SQLITE_FCNTL_SIZE_LIMIT failed".to_string());
            Err(error::error_from_sqlite_code(rc, message))
        }
    }
}

unsafe fn catch_unwind_sqlite_error(
    file: *mut ffi::sqlite3_file,
    f: impl FnOnce(&mut VecDbFile) -> c_int + panic::UnwindSafe,
) -> c_int {
    panic::catch_unwind(|| f(&mut *(file as *mut VecDbFile))).unwrap_or(ffi::SQLITE_ERROR)
}

/// This will be called when dropping the `Connection` or
/// when the database gets detached.
unsafe extern "C" fn x_close(file: *mut ffi::sqlite3_file) -> c_int {
    catch_unwind_sqlite_error(file, |file| {
        ptr::drop_in_place(file);
        ffi::SQLITE_OK
    })
}

/// Read data from a memory file.
unsafe extern "C" fn x_read(
    file: *mut ffi::sqlite3_file,
    buf: *mut c_void,
    amt: c_int,
    ofst: i64,
) -> c_int {
    catch_unwind_sqlite_error(file, |file| {
        let data = &file.data;
        let buf = buf as *mut u8;
        let amt: usize = amt.try_into().unwrap();
        let ofst: usize = ofst.try_into().unwrap();
        if ofst + amt > data.len() {
            ptr::write_bytes(buf, 0, amt);
            if ofst < data.len() {
                ptr::copy_nonoverlapping(data.as_ptr().add(ofst), buf, data.len() - ofst);
            }
            return ffi::SQLITE_IOERR_SHORT_READ;
        }
        ptr::copy_nonoverlapping(data.as_ptr().add(ofst), buf, amt);
        ffi::SQLITE_OK
    })
}

/// Write data to a memory file.
unsafe extern "C" fn x_write(
    file: *mut ffi::sqlite3_file,
    buf: *const c_void,
    amt: c_int,
    ofst: i64,
) -> c_int {
    catch_unwind_sqlite_error(file, |file| {
        let data = match MemFile::get_mut_vec(&mut file.data) {
            Some(d) => d,
            _ => return ffi::SQLITE_READONLY,
        };
        let sz = data.len();
        let sz_alloc = data.capacity();
        let amt = amt as usize;
        let ofst = ofst as usize;
        if ofst + amt > sz {
            if ofst + amt > sz_alloc {
                if file.memory_mapped != 0 {
                    return ffi::SQLITE_FULL;
                }
                data.reserve(ofst + amt - sz);
                if data.capacity() > file.size_max {
                    return ffi::SQLITE_FULL;
                }
            }
            if ofst > sz {
                ptr::write_bytes(data.as_mut_ptr().add(sz), 0, ofst - sz);
            }
            data.set_len(ofst + amt);
        }
        ptr::copy_nonoverlapping(buf, data.as_mut_ptr().add(ofst).cast(), amt);
        ffi::SQLITE_OK
    })
}

/// Truncate a memory file.
///
/// In rollback mode (which is always the case for memdb, as it does not
/// support WAL mode) the truncate() method is only used to reduce
/// the size of a file, never to increase the size.
unsafe extern "C" fn x_truncate(file: *mut ffi::sqlite3_file, size: i64) -> c_int {
    catch_unwind_sqlite_error(file, |file| {
        if let Some(data) = MemFile::get_mut_vec(&mut file.data) {
            let size = size.try_into().unwrap();
            if size > data.len() {
                ffi::SQLITE_FULL
            } else {
                data.set_len(size);
                ffi::SQLITE_OK
            }
        } else {
            ffi::SQLITE_FULL
        }
    })
}

/// Sync a memory file.
unsafe extern "C" fn x_sync(_file: *mut ffi::sqlite3_file, _flags: c_int) -> c_int {
    ffi::SQLITE_OK
}

/// Return the current file-size of a memory file.
unsafe extern "C" fn x_size(file: *mut ffi::sqlite3_file, size: *mut i64) -> c_int {
    catch_unwind_sqlite_error(file, |file| {
        *size = file.data.len() as _;
        ffi::SQLITE_OK
    })
}

/// Lock a memory file.
unsafe extern "C" fn x_lock(file: *mut ffi::sqlite3_file, lock: c_int) -> c_int {
    catch_unwind_sqlite_error(file, |file| {
        let lock = lock as u8;
        if lock > 1 && MemFile::get_mut_vec(&mut file.data).is_none() {
            ffi::SQLITE_READONLY
        } else {
            file.lock = lock;
            ffi::SQLITE_OK
        }
    })
}

/// File control method.
unsafe extern "C" fn x_file_control(
    file: *mut ffi::sqlite3_file,
    op: c_int,
    arg: *mut c_void,
) -> c_int {
    catch_unwind_sqlite_error(file, |file| match op {
        // This operation is intended for diagnostic use only.
        ffi::SQLITE_FCNTL_VFSNAME => {
            *(arg as *mut *const c_char) =
                crate::util::SqliteMallocString::from_str(&format!("{:?}", file)).into_raw();
            ffi::SQLITE_OK
        }
        // Set an upper bound on the size of the in-memory database.
        ffi::SQLITE_FCNTL_SIZE_LIMIT => {
            let arg = arg as *mut ffi::sqlite3_int64;
            let mut limit = *arg;
            let len = file.data.len() as _;
            if limit < len {
                if limit < 0 {
                    limit = file.size_max as _;
                } else {
                    limit = len;
                }
            }
            file.size_max = limit.try_into().expect("overflow size_max");
            *arg = limit;
            ffi::SQLITE_OK
        }
        _ => ffi::SQLITE_NOTFOUND,
    })
}

/// Return the device characteristic flags supported.
unsafe extern "C" fn x_device_characteristics(_file: *mut ffi::sqlite3_file) -> c_int {
    ffi::SQLITE_IOCAP_ATOMIC
        | ffi::SQLITE_IOCAP_POWERSAFE_OVERWRITE
        | ffi::SQLITE_IOCAP_SAFE_APPEND
        | ffi::SQLITE_IOCAP_SEQUENTIAL
}

/// Fetch a page of a memory-mapped file.
unsafe extern "C" fn x_fetch(
    file: *mut ffi::sqlite3_file,
    ofst: i64,
    amt: c_int,
    p: *mut *mut c_void,
) -> c_int {
    catch_unwind_sqlite_error(file, |file| {
        let data = &file.data;
        let amt: usize = amt.try_into().unwrap();
        let ofst: usize = ofst.try_into().unwrap();
        if ofst + amt > data.len() as _ {
            *p = ptr::null_mut();
        } else {
            // Safety: SQLite uses a read-only memory map <https://www.sqlite.org/mmap.html>,
            // so it is safe to cast this *const to *mut.
            *p = data.as_ptr() as *mut u8 as _;
            assert_ne!(file.memory_mapped, u16::MAX);
            file.memory_mapped += 1;
        }
        ffi::SQLITE_OK
    })
}

/// Release a memory-mapped page.
unsafe extern "C" fn x_unfetch(file: *mut ffi::sqlite3_file, _ofst: i64, _p: *mut c_void) -> c_int {
    catch_unwind_sqlite_error(file, |file| {
        assert_ne!(file.memory_mapped, 0);
        file.memory_mapped -= 1;
        ffi::SQLITE_OK
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{Connection, DatabaseName, Error, Result};
    use std::ffi::CStr;
    use std::mem::MaybeUninit;

    #[test]
    pub fn test_serialize_deserialize() {
        let db = Connection::open_in_memory().unwrap().into_borrowing();
        let sql = "BEGIN;
            CREATE TABLE foo(x INTEGER);
            INSERT INTO foo VALUES(1);
            INSERT INTO foo VALUES(2);
            INSERT INTO foo VALUES(3);
            END;";
        db.execute_batch(sql).unwrap();
        let serialized = db.serialize(DatabaseName::Main).unwrap();

        // create a new db and import the serialized data
        let db2 = Connection::open_in_memory().unwrap().into_borrowing();
        db2.deserialize(DatabaseName::Main, serialized).unwrap();
        let mut query = db2.prepare("SELECT x FROM foo").unwrap();
        let results: Result<Vec<u16>> = query.query_map([], |row| row.get(0)).unwrap().collect();
        std::mem::drop(query);
        assert_eq!(vec![1, 2, 3], results.unwrap());
        // should not be read-only
        let sql = "INSERT INTO foo VALUES(4)";
        db2.execute_batch(sql).unwrap();

        // NO_COPY only works on db2
        assert!(db.serialize_rc(DatabaseName::Main).is_none());
        let borrowed_serialized = db2.serialize_rc(DatabaseName::Main).unwrap();
        let mut serialized = Vec::new();
        serialized.extend(borrowed_serialized.iter().cloned());

        // create a third db and import the serialized data
        let db3 = Connection::open_in_memory().unwrap();
        db3.deserialize(DatabaseName::Main, serialized).unwrap();
        let mut query = db3.prepare("SELECT x FROM foo").unwrap();
        let results: Result<Vec<u16>> = query.query_map([], |row| row.get(0)).unwrap().collect();
        assert_eq!(vec![1, 2, 3, 4], results.unwrap());
    }

    #[test]
    #[allow(clippy::redundant_clone)]
    pub fn test_serialize_rc() {
        // prepare two distinct files: a & b
        let db1 = Connection::open_in_memory().unwrap().into_borrowing();
        db1.execute_batch("CREATE TABLE a(x INTEGER);INSERT INTO a VALUES(1);")
            .unwrap();
        let file_a = db1.serialize(DatabaseName::Main).unwrap();
        db1.execute_batch("INSERT INTO a VALUES(2);").unwrap();
        let file_b = db1.serialize(DatabaseName::Main).unwrap();

        let db2 = Connection::open_in_memory().unwrap().into_borrowing();
        db2.deserialize(DatabaseName::Main, file_a.clone()).unwrap();
        let file_c = db2.serialize_rc(DatabaseName::Main).unwrap();
        let file_c_clone: MemFile = match &*file_c {
            MemFile::Owned(v) => MemFile::Owned(v.clone()),
            _ => panic!(),
        };
        assert_eq!(*file_c, file_c_clone);
        let sql = "INSERT INTO a VALUES(3)";
        db2.execute_batch(sql)
            .expect_err("should be write protected");
        mem::drop(file_c);
        db2.execute_batch(sql)
            .expect("should succeed after file_c is dropped");
        assert_eq!(
            2,
            db2.query_row("SELECT COUNT(x) FROM a", [], |r| r.get::<_, i32>(0))
                .unwrap()
        );

        let name_d = DatabaseName::Attached("d");
        let err = db2
            .deserialize_read_only(name_d, &file_a)
            .expect_err("name does not exist");
        assert_eq!(
            err,
            Error::SqliteFailure(
                ffi::Error {
                    code: ffi::ErrorCode::Unknown,
                    extended_code: 1
                },
                Some("not an error".to_string())
            )
        );
        db2.execute_batch("ATTACH DATABASE ':memory:' AS d")
            .unwrap();
        db2.deserialize(name_d, file_a.clone()).unwrap();
        let mut file_d = db2.serialize_rc(name_d).unwrap();
        // detach and attach other db, this should call xClose and decrease reference count
        assert_eq!(2, Arc::strong_count(&file_d));
        assert_eq!(0, Arc::weak_count(&file_d));
        db2.deserialize(name_d, file_b).unwrap();
        assert_eq!(1, Arc::strong_count(&file_d));
        assert_eq!(0, Arc::weak_count(&file_d));
        match Arc::get_mut(&mut file_d).unwrap() {
            MemFile::Owned(v) => v.shrink_to_fit(),
            _ => panic!(),
        };
        let file_d = Arc::try_unwrap(file_d).unwrap();
        // test whether file_d stayed intact
        db2.deserialize_read_only(DatabaseName::Main, &file_d)
            .unwrap();
        assert_eq!(
            1,
            db2.query_row("SELECT MAX(x) FROM main.a", [], |r| r.get::<_, i32>(0))
                .unwrap()
        );
        assert_eq!(
            2,
            db2.query_row("SELECT MAX(x) FROM d.a", [], |r| r.get::<_, i32>(0))
                .unwrap()
        );
        mem::drop(db2);
        // mem::drop(file_a); // uncommenting this line should not compile
        file_d.len();
    }

    #[test]
    pub fn test_serialize_rc_get_mut() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        db.execute_batch("CREATE TABLE a(x INTEGER);INSERT INTO a VALUES(1);")?;
        let mem_file = db.serialize(DatabaseName::Main)?;
        db.deserialize(DatabaseName::Main, mem_file)?;
        match db.serialize_get_mut(DatabaseName::Main).unwrap() {
            MemFile::Owned(f) => {
                f.reserve_exact(4096);
            }
            _ => unreachable!(),
        }
        db.execute_batch("INSERT INTO a VALUES (1);")?;
        Ok(())
    }

    #[test]
    pub fn test_deserialize_read_only_1() {
        let db = Connection::open_in_memory().unwrap();
        let sql = "BEGIN;
            CREATE TABLE foo(x INTEGER);
            INSERT INTO foo VALUES(1);
            INSERT INTO foo VALUES(2);
            INSERT INTO foo VALUES(3);
            END;";
        db.execute_batch(sql).unwrap();
        let serialized = db.serialize(DatabaseName::Main).unwrap();
        // copy to Vec and create new Vec
        let serialized_vec = Vec::from(&serialized[..]);
        let mut serialized = Vec::new();
        serialized.extend(serialized_vec);

        // create a new db and import the serialized data
        let db2 = Connection::open_in_memory().unwrap().into_borrowing();
        db2.deserialize_read_only(DatabaseName::Main, &serialized)
            .unwrap();
        let mut query = db2.prepare("SELECT x FROM foo").unwrap();
        let results: Result<Vec<u16>> = query.query_map([], |row| row.get(0)).unwrap().collect();
        assert_eq!(vec![1, 2, 3], results.unwrap());
        // should be read-only
        let sql = "INSERT INTO foo VALUES(4)";
        db2.execute_batch(sql).unwrap_err();
    }

    #[test]
    #[allow(clippy::redundant_clone)]
    pub fn test_deserialize_read_only() -> Result<()> {
        let sql = "BEGIN;
            CREATE TABLE hello(x INTEGER);
            INSERT INTO hello VALUES(1);
            INSERT INTO hello VALUES(2);
            INSERT INTO hello VALUES(3);
            END;";

        // prepare two named databases
        let one = Connection::open_in_memory()?;
        one.execute_batch(sql)?;
        let serialized_one = one.serialize(DatabaseName::Main)?;

        let two = Connection::open_in_memory()?;
        two.execute_batch(sql)?;
        let serialized_two = two.serialize(DatabaseName::Main)?;

        // create a new db and import the serialized data
        let db = Connection::open_in_memory()?.into_borrowing();
        db.execute_batch("ATTACH DATABASE ':memory:' AS foo; ATTACH DATABASE ':memory:' AS bar")?;
        db.deserialize_read_only(DatabaseName::Attached("foo"), &serialized_one)?;
        let serialized_three = serialized_two.clone();
        {
            let name_bar = "bar".to_string();
            db.deserialize_read_only(DatabaseName::Attached(&name_bar), &serialized_three)?;
        }
        // mem::drop(serialized_three); // uncommenting this should not compile
        let mut query = db.prepare("SELECT x FROM foo.hello")?;
        let results: Result<Vec<u16>> = query.query_map([], |row| row.get(0))?.collect();
        assert_eq!(vec![1, 2, 3], results?);
        let mut query = db.prepare("SELECT x FROM bar.hello")?;
        let results: Result<Vec<u16>> = query.query_map([], |row| row.get(0))?.collect();
        assert_eq!(vec![1, 2, 3], results?);
        // should be read-only
        let sql = "INSERT INTO foo VALUES(4)";
        db.execute_batch(sql).unwrap_err();
        Ok(())
    }

    //noinspection RsAssertEqual
    #[test]
    pub fn test_deserialize_writable() -> Result<()> {
        let sql = "BEGIN;
            CREATE TABLE hello(x INTEGER);
            INSERT INTO hello VALUES(1);
            INSERT INTO hello VALUES(2);
            INSERT INTO hello VALUES(3);
            END;";
        let db1 = Connection::open_in_memory()?;
        db1.execute_batch(sql)?;
        let mut serialized1 = db1.serialize(DatabaseName::Main)?;
        let initial_cap = serialized1.capacity();
        let initial_len = serialized1.len();

        // create a new db and mutably borrow the serialized data
        let mut db3 = Connection::open_in_memory()?.into_borrowing();
        db3.deserialize_writable(DatabaseName::Main, &mut serialized1)?;
        // update should not affect length
        db3.execute_batch("UPDATE hello SET x = 44 WHERE x = 3")?;
        let mut query = db3.prepare("SELECT x FROM hello")?;
        let results: Result<Vec<u16>> = query.query_map([], |row| row.get(0))?.collect();
        assert_eq!(vec![1, 2, 44], results?);
        mem::drop(query);
        assert_eq!(initial_len, serialize_len(&mut db3));

        // insert data until the length needs to grow
        let count_until_resize = std::iter::repeat(())
            .take_while(|_| {
                db3.execute_batch("INSERT INTO hello VALUES(44);").unwrap();
                serialize_len(&mut db3) == initial_len
            })
            .count();
        assert_eq!(524, count_until_resize);

        // connection close should update ptr, capacity, length of serialized1
        let new_len = serialize_len(&mut db3);
        assert!(new_len > initial_len);
        mem::drop(db3);
        assert_eq!(new_len, serialized1.len());
        assert!(serialized1.capacity() > initial_cap);
        // serialized1.as_ptr() may differ, but it could also have grown in place
        let mut serialized2 = serialized1.clone();

        // serializing again should work
        db1.execute_batch("ATTACH DATABASE ':memory:' AS three;")?;
        let db1 = db1.into_borrowing();
        db1.deserialize_writable(DatabaseName::Attached("three"), &mut serialized1)?;
        let count: u16 = db1.query_row("SELECT COUNT(*) FROM hello", [], |r| r.get(0))?;
        assert_eq!(3, count);
        let count: u16 = db1.query_row("SELECT COUNT(*) FROM three.hello", [], |r| r.get(0))?;
        assert_eq!(528, count);

        // test detach error handling for deserialize_writable
        db1.execute_batch("DETACH DATABASE three")?;
        mem::drop(db1);
        assert_ne!(0, serialized1.capacity());
        assert_eq!(new_len, serialized1.len());

        // test detach error handling for deserialize_mut
        assert_ne!(0, serialized2.capacity());
        assert!(serialized1[..] == serialized2[..]);
        let db4 = Connection::open_in_memory()?.into_borrowing();
        db4.execute_batch("ATTACH DATABASE ':memory:' AS hello")?;
        db4.deserialize_writable(DatabaseName::Attached("hello"), &mut serialized2)?;
        db4.execute_batch("DETACH DATABASE hello")?;
        let debug = format!("{:?}", db4);
        mem::drop(db4);
        assert_ne!(0, serialized2.capacity());
        assert!(serialized1[..] == serialized2[..]);

        // Debug impl
        assert_eq!(
            &debug,
            "BorrowingConnection { conn: Connection { path: Some(\":memory:\") } }"
        );

        Ok(())
    }

    #[test]
    fn test_serialize_non_existing_db_name() {
        let db = Connection::open_in_memory().unwrap().into_borrowing();
        let sql = "BEGIN;
        CREATE TABLE hello(x INTEGER);
        INSERT INTO hello VALUES(1);
        INSERT INTO hello VALUES(2);
        INSERT INTO hello VALUES(3);
        END;";
        db.execute_batch(sql).unwrap();
        assert!(db
            .serialize_rc(DatabaseName::Attached("does not exist"))
            .is_none());
        let err = db
            .serialize(DatabaseName::Attached("does not exist"))
            .unwrap_err();
        assert_eq!(
            err,
            Error::SqliteFailure(
                ffi::Error {
                    code: ffi::ErrorCode::Unknown,
                    extended_code: 1
                },
                Some(r#"database "does not exist" not found"#.to_string())
            )
        );
        let file = db.serialize(DatabaseName::Main).unwrap();
        db.deserialize(DatabaseName::Attached("does not exist"), file)
            .unwrap_err();
    }

    fn serialize_len(conn: &mut BorrowingConnection) -> usize {
        conn.serialize_rc(DatabaseName::Main).unwrap().len()
    }

    #[test]
    fn test_vec_db_vfs_name() {
        unsafe {
            let db = Connection::open_in_memory().unwrap();
            let vec = vec![1, 2, 3];
            let vec_buf = vec.as_ptr();
            db.deserialize(DatabaseName::Main, vec).unwrap();
            let mut name: *const c_char = ptr::null();
            let rc = ffi::sqlite3_file_control(
                db.db.borrow().db(),
                "main\0".as_ptr() as _,
                ffi::SQLITE_FCNTL_VFSNAME,
                &mut name as *mut _ as _,
            );
            assert_eq!(ffi::SQLITE_OK, rc);
            assert!(!name.is_null());
            let name_str = CStr::from_ptr(name).to_str().unwrap();
            assert_eq!(name_str, &format!("VecDbFile {{ methods: {:p}, data: Owned {{ ptr: {:p}, length: 3, capacity: 3 }}, size_max: 1073741824, memory_mapped: 0, lock: 0 }}", &VEC_DB_IO_METHODS, vec_buf));
            ffi::sqlite3_free(name as _);
        }
    }

    #[test]
    fn test_serialize_zero_pages() {
        let db = Connection::open_in_memory().unwrap().into_borrowing();
        let vec = db.serialize(DatabaseName::Main).unwrap();
        assert_eq!(vec.len(), 0);
    }

    #[test]
    fn test_serialize_vec_db() -> Result<()> {
        let db = Connection::open_in_memory().unwrap().into_borrowing();
        db.execute_batch("CREATE TABLE a(x INTEGER); ATTACH DATABASE ':memory:' AS a")?;
        let vec = db.serialize(DatabaseName::Main)?;
        let name_a = DatabaseName::Attached("a");
        db.deserialize_read_only(name_a, &vec)?;
        // code coverage reports shows this uses the optimized path
        let copy = db.serialize(name_a)?;
        assert_eq!(vec, copy);
        Ok(())
    }

    #[test]
    fn test_serialize_quoted() -> Result<()> {
        let db = Connection::open_in_memory().unwrap().into_borrowing();
        db.execute_batch(
            r#"ATTACH DATABASE ':memory:' AS "q'u""o'te";
            CREATE TABLE "q'u""o'te".a(x INTEGER);
            INSERT INTO "q'u""o'te".a VALUES (1);"#,
        )?;
        let name_a = DatabaseName::Attached(r#"q'u"o'te"#);
        let count: i64 = db.pragma_query_value(Some(name_a), "page_count", |r| r.get(0))?;
        assert_eq!(count, 2);
        let count: i64 = db.query_row(
            r#"SELECT page_count FROM pragma_page_count("q'u""o'te")"#,
            [],
            |r| r.get(0),
        )?;
        assert_eq!(count, 2);
        let vec = db.serialize(name_a)?;
        assert_eq!(vec.len(), 8192);
        Ok(())
    }

    #[test]
    fn test_serialize_page_size() -> Result<()> {
        let db = Connection::open_in_memory().unwrap().into_borrowing();
        db.execute_batch(r#"PRAGMA page_size = 512;CREATE TABLE a(x INTEGER);"#)?;
        let vec = db.serialize(DatabaseName::Main)?;
        assert_eq!(vec.len(), 512 * 2);
        Ok(())
    }

    #[test]
    fn test_vec_db_fetch() -> Result<()> {
        let db = Connection::open_in_memory().unwrap().into_borrowing();
        db.execute_batch("CREATE TABLE a(x INTEGER)")?;
        let mut vec = db.serialize(DatabaseName::Main)?;
        let size = vec.len();
        assert_ne!(0, size);
        db.deserialize_writable(DatabaseName::Main, &mut vec)?;
        let file = file_ptr(&db.db.borrow(), &DatabaseName::Main.to_cstring()?).unwrap();
        // fetch returns null on overflow
        assert!(file_fetch(file, 0, size + 1)?.is_null());
        assert!(file_fetch(file, 1, size)?.is_null());
        file_fetch(file, -1, 1).expect_err("should catch panic because of negative offset");
        let p = file_fetch(file, 0, size)?;
        assert!(!p.is_null());
        assert_eq!(p, db.serialize_rc(DatabaseName::Main).unwrap().as_ptr());
        // Won't resize unit unfetch
        let sql = "WITH RECURSIVE cnt(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM cnt WHERE x<100000) INSERT INTO a SELECT x FROM cnt";
        db.execute_batch(sql).expect_err("enlarge should fail");
        file_unfetch(file, p, 0);
        assert_eq!(size, db.serialize_rc(DatabaseName::Main).unwrap().len());
        db.execute_batch(sql).expect("enlarge should succeed");
        assert_ne!(size, db.serialize_rc(DatabaseName::Main).unwrap().len());
        Ok(())
    }

    fn file_fetch(file: &mut ffi::sqlite3_file, ofst: i64, amt: usize) -> Result<*const u8> {
        unsafe {
            let mut fetch = MaybeUninit::zeroed();
            let rc =
                (*file.pMethods).xFetch.unwrap()(file, ofst, amt as _, fetch.as_mut_ptr() as _);
            if rc != ffi::SQLITE_OK {
                Err(error::error_from_sqlite_code(ffi::SQLITE_LOCKED, None))
            } else {
                Ok(fetch.assume_init())
            }
        }
    }

    fn file_unfetch(file: &mut ffi::sqlite3_file, p: *const u8, ofst: i64) {
        let rc = unsafe { (*file.pMethods).xUnfetch.unwrap()(file, ofst, p as *mut u8 as _) };
        assert_eq!(rc, ffi::SQLITE_OK);
    }

    #[test]
    fn test_vec_db_read_short() -> Result<()> {
        let db = Connection::open_in_memory().unwrap().into_borrowing();
        db.execute_batch("CREATE TABLE a(x INTEGER)")?;
        db.deserialize(DatabaseName::Main, db.serialize(DatabaseName::Main)?)?;
        let file = file_ptr(&db.db.borrow(), &DatabaseName::Main.to_cstring()?).unwrap();

        // when reading past end, the buffer should be filled with zeros
        let mut buf = [1; 16];
        let end = file_len(file);
        let rc = file_read(file, &mut buf, end);
        assert_eq!(rc, ffi::SQLITE_IOERR_SHORT_READ);
        assert_eq!(&buf, &[0; 16]);

        // when reading partly past the end, the buffer should be filled with content
        let vec_db = VecDbFile::try_cast(file).unwrap();
        MemFile::get_mut_vec(&mut vec_db.data).unwrap()[end as usize - 1] = 0xab;
        let mut buf = [1; 16];
        let rc = file_read(file, &mut buf, end - 8);
        assert_eq!(rc, ffi::SQLITE_IOERR_SHORT_READ);
        assert_eq!(&buf, b"\0\0\0\0\0\0\0\xab\0\0\0\0\0\0\0\0");
        Ok(())
    }

    fn file_read(file: &mut ffi::sqlite3_file, buf: &mut [u8], ofst: i64) -> c_int {
        unsafe {
            (*file.pMethods).xRead.unwrap()(file, buf.as_mut_ptr() as _, buf.len() as _, ofst)
        }
    }

    fn file_len(file: &mut ffi::sqlite3_file) -> i64 {
        unsafe {
            let mut size = 0;
            let rc = (*file.pMethods).xFileSize.unwrap()(file, &mut size);
            assert_eq!(rc, ffi::SQLITE_OK);
            size
        }
    }

    #[test]
    fn test_vec_db_size_max() -> Result<()> {
        let db = Connection::open_in_memory().unwrap().into_borrowing();
        db.execute_batch("CREATE TABLE a(x INTEGER)")?;
        let mut vec = db.serialize(DatabaseName::Main)?;
        let size = vec.len() as i64;
        assert_ne!(0, size);
        db.deserialize_writable(DatabaseName::Main, &mut vec)?;
        let file = file_ptr(&db.db.borrow(), &DatabaseName::Main.to_cstring()?).unwrap();
        let cap = file_size_limit(file, -1)?;
        assert_eq!(cap, 1073741824, "default SQLITE_CONFIG_MEMDB_MAXSIZE");
        assert_eq!(size, file_size_limit(file, 200)?);
        assert_eq!(size, file_size_limit(file, -1)?);

        // trigger enlarge
        let sql = "WITH RECURSIVE cnt(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM cnt WHERE x<500) INSERT INTO a SELECT x FROM cnt";
        db.execute_batch(sql).expect_err("enlarge should fail");

        let new_cap = size * 2;
        assert_eq!(
            new_cap,
            db.serialize_set_size_limit(DatabaseName::Main, new_cap as _)? as i64
        );
        assert_eq!(new_cap, db.serialize_size_limit(DatabaseName::Main)? as i64);
        db.execute_batch(sql).expect("enlarge should succeed");

        // truncate
        assert_eq!(new_cap, file_len(file));
        db.execute_batch("DELETE FROM a; VACUUM;")?;
        assert_eq!(size, file_len(file));
        assert_eq!(new_cap, file_size_limit(file, -1)?);
        db.execute_batch("DROP TABLE a; VACUUM;")?;
        assert_eq!(4096, file_len(file));

        Ok(())
    }

    #[test]
    fn test_vec_db_write_zero_past_len() -> Result<()> {
        unsafe {
            let mut vec_db = VecDbFile::new(MemFile::Owned(Vec::new()), usize::MAX);
            let file = &mut vec_db as *mut _ as *mut ffi::sqlite3_file;
            let write = (*vec_db.methods).xWrite.unwrap();
            let buf = &[11u8, 22, 33];
            write(file, buf.as_ptr() as *const _, buf.len() as _, 4);
            assert_eq!(vec_db.data.as_slice(), &[0, 0, 0, 0, 11, 22, 33]);
        }
        Ok(())
    }
}
