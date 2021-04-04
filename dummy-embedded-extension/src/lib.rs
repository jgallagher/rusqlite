use std::marker::PhantomData;
use std::os::raw::{c_char, c_int};

use rusqlite::vtab::{
    eponymous_only_module, sqlite3_vtab, sqlite3_vtab_cursor, Context, IndexInfo, VTab,
    VTabConnection, VTabCursor, Values,
};
use rusqlite::{
    ffi,
    functions::FunctionFlags,
    types::{ToSqlOutput, Value},
};
use rusqlite::{to_sqlite_error, Connection, Result};

/// dummy_embedded_extension_init is the entry point for this library.
///
/// This crate produces a cdylib that is intended to be embedded within
/// (i.e. linked into) another library that implements the sqlite loadable
/// extension entrypoint.
///
/// In the case of this example code, refer to the `dummy-c-host-extension`
/// C code to find where this entry point is invoked.
///
/// Note that this interface is private between the host extension and this
/// library - it can have any signature as long as it passes the *sqlite3 db
/// pointer so we can use it to initialize our rusqlite::Connection.
///
/// It does *not* have to return sqlite status codes (such as SQLITE_OK), we
/// just do that here to keep the C extension simple.
#[no_mangle]
pub extern "C" fn dummy_embedded_extension_init(
    db: *mut ffi::sqlite3,
    pz_err_msg: *mut *mut c_char,
) -> c_int {
    let res = dummy_embedded_init(db);
    if let Err(err) = res {
        return unsafe { to_sqlite_error(&err, pz_err_msg) };
    }

    ffi::SQLITE_OK
}

#[repr(C)]
struct DummyEmbeddedTab {
    /// Base class. Must be first
    base: sqlite3_vtab,
}

unsafe impl<'vtab> VTab<'vtab> for DummyEmbeddedTab {
    type Aux = ();
    type Cursor = DummyEmbeddedTabCursor<'vtab>;

    fn connect(
        _: &mut VTabConnection,
        _aux: Option<&()>,
        _args: &[&[u8]],
    ) -> Result<(String, DummyEmbeddedTab)> {
        let vtab = DummyEmbeddedTab {
            base: sqlite3_vtab::default(),
        };
        Ok(("CREATE TABLE x(value TEXT)".to_owned(), vtab))
    }

    fn best_index(&self, info: &mut IndexInfo) -> Result<()> {
        info.set_estimated_cost(1.);
        Ok(())
    }

    fn open(&'vtab self) -> Result<DummyEmbeddedTabCursor<'vtab>> {
        Ok(DummyEmbeddedTabCursor::default())
    }
}

#[derive(Default)]
#[repr(C)]
struct DummyEmbeddedTabCursor<'vtab> {
    /// Base class. Must be first
    base: sqlite3_vtab_cursor,
    /// The rowid
    row_id: i64,
    phantom: PhantomData<&'vtab DummyEmbeddedTab>,
}

unsafe impl VTabCursor for DummyEmbeddedTabCursor<'_> {
    fn filter(
        &mut self,
        _idx_num: c_int,
        _idx_str: Option<&str>,
        _args: &Values<'_>,
    ) -> Result<()> {
        self.row_id = 1;
        Ok(())
    }

    fn next(&mut self) -> Result<()> {
        self.row_id += 1;
        Ok(())
    }

    fn eof(&self) -> bool {
        self.row_id > 1
    }

    fn column(&self, ctx: &mut Context, _: c_int) -> Result<()> {
        ctx.set_result(&"dummy_embedded_test_value".to_string())
    }

    fn rowid(&self) -> Result<i64> {
        Ok(self.row_id)
    }
}

fn dummy_embedded_init(db: *mut ffi::sqlite3) -> Result<()> {
    let conn = unsafe { Connection::from_handle(db)? };
    eprintln!("inited dummy embedded extension module {:?}", db);
    conn.create_scalar_function(
        "dummy_embedded_test_function",
        0,
        FunctionFlags::SQLITE_DETERMINISTIC,
        |_ctx| {
            Ok(ToSqlOutput::Owned(Value::Text(
                "Dummy embedded extension loaded correctly!".to_string(),
            )))
        },
    )?;
    conn.create_module::<DummyEmbeddedTab>("dummy_embedded", eponymous_only_module::<DummyEmbeddedTab>(), None)
}
