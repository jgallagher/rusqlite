//! Pragma helpers

use std::ops::Deref;

use {Connection, DatabaseName, Result, Row};
use error::Error;
use ffi;
use types::{ToSql, ToSqlOutput, ValueRef};

pub struct Sql {
    buf: String,
}

impl Sql {
    pub fn new() -> Sql {
        Sql { buf: String::new() }
    }

    pub fn push_pragma(&mut self,
                       schema_name: Option<DatabaseName>,
                       pragma_name: &str)
                       -> Result<()> {
        try!(self.push_keyword("PRAGMA"));
        self.push_space();
        if let Some(schema_name) = schema_name {
            self.push_schema_name(schema_name);
            self.push_dot();
        }
        self.push_keyword(pragma_name)

    }

    pub fn push_keyword(&mut self, keyword: &str) -> Result<()> {
        if !keyword.is_empty() && is_identifier(keyword) {
            self.buf.push_str(keyword);
            Ok(())
        } else {
            Err(Error::SqliteFailure(ffi::Error::new(ffi::SQLITE_MISUSE),
                                     Some(format!("Invalid keyword \"{}\"", keyword))))
        }
    }

    pub fn push_schema_name(&mut self, schema_name: DatabaseName) {
        match schema_name {
            DatabaseName::Main => self.buf.push_str("main"),
            DatabaseName::Temp => self.buf.push_str("temp"),
            DatabaseName::Attached(s) => self.push_identifier(s),
        };
    }

    pub fn push_identifier(&mut self, s: &str) {
        if is_identifier(s) {
            self.buf.push_str(s);
        } else {
            self.wrap_and_escape(s, '"');
        }
    }

    pub fn push_value(&mut self, value: &ToSql) -> Result<()> {
        let value = try!(value.to_sql());
        let value = match value {
            ToSqlOutput::Borrowed(v) => v,
            ToSqlOutput::Owned(ref v) => ValueRef::from(v),
            #[cfg(feature = "blob")]
            ToSqlOutput::ZeroBlob(_) => {
                return Err(Error::SqliteFailure(ffi::Error::new(ffi::SQLITE_MISUSE),
                                                Some(format!("Unsupported value \"{:?}\"",
                                                             value))));
            }
        };
        match value {
            ValueRef::Integer(i) => {
                self.push_int(i);
            }
            ValueRef::Real(r) => {
                self.push_real(r);
            }
            ValueRef::Text(s) => {
                self.push_string_literal(s);
            }
            _ => {
                return Err(Error::SqliteFailure(ffi::Error::new(ffi::SQLITE_MISUSE),
                                                Some(format!("Unsupported value \"{:?}\"", value))))
            }
        };
        Ok(())
    }

    pub fn push_string_literal(&mut self, s: &str) {
        self.wrap_and_escape(s, '\'');
    }

    pub fn push_int(&mut self, i: i64) {
        self.buf.push_str(&i.to_string());
    }

    pub fn push_real(&mut self, f: f64) {
        self.buf.push_str(&f.to_string());
    }

    pub fn push_space(&mut self) {
        self.buf.push(' ');
    }

    pub fn push_dot(&mut self) {
        self.buf.push('.');
    }

    pub fn push_equal_sign(&mut self) {
        self.buf.push('=');
    }

    pub fn open_brace(&mut self) {
        self.buf.push('(');
    }
    pub fn close_brace(&mut self) {
        self.buf.push(')');
    }

    pub fn as_str(&self) -> &str {
        &self.buf
    }

    fn wrap_and_escape(&mut self, s: &str, quote: char) {
        self.buf.push(quote);
        let chars = s.chars();
        for ch in chars {
            // escape `quote` by doubling it
            if ch == quote {
                self.buf.push(ch);
            }
            self.buf.push(ch)
        }
        self.buf.push(quote);
    }
}

impl Deref for Sql {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl Connection {
    /// Query the current value of `pragma_name`.
    ///
    /// Some pragmas will return multiple rows/values which cannot be retrieved with this method.
    pub fn pragma_query_value<T, F>(&self,
                                    schema_name: Option<DatabaseName>,
                                    pragma_name: &str,
                                    f: F)
                                    -> Result<T>
        where F: FnOnce(Row) -> Result<T>
    {
        let mut query = Sql::new();
        try!(query.push_pragma(schema_name, pragma_name));
        let mut stmt = try!(self.prepare(&query));
        let mut rows = try!(stmt.query(&[]));
        if let Some(result_row) = rows.next() {
            let row = try!(result_row);
            f(row)
        } else {
            Err(Error::QueryReturnedNoRows)
        }
    }

    /// Query the current rows/values of `pragma_name`.
    pub fn pragma_query<F>(&self,
                           schema_name: Option<DatabaseName>,
                           pragma_name: &str,
                           mut f: F)
                           -> Result<()>
        where F: FnMut(&Row) -> Result<()>
    {
        let mut query = Sql::new();
        try!(query.push_pragma(schema_name, pragma_name));
        let mut stmt = try!(self.prepare(&query));
        let mut rows = try!(stmt.query(&[]));
        while let Some(result_row) = rows.next() {
            let row = try!(result_row);
            try!(f(&row));
        }
        Ok(())
    }

    /// Query the current value(s) of `pragma_name` associated to `pragma_value`.
    ///
    /// This method can be used with query-only pragmas which need an argument
    /// (e.g. `table_info('one_tbl')`) or pragmas which return the updated value
    /// (e.g. `journal_mode`).
    pub fn pragma<F>(&self,
                     schema_name: Option<DatabaseName>,
                     pragma_name: &str,
                     pragma_value: &ToSql,
                     mut f: F)
                     -> Result<()>
        where F: FnMut(&Row) -> Result<()>
    {
        let mut sql = Sql::new();
        try!(sql.push_pragma(schema_name, pragma_name));
        // The argument is may be either in parentheses
        // or it may be separated from the pragma name by an equal sign.
        // The two syntaxes yield identical results.
        sql.open_brace();
        try!(sql.push_value(pragma_value));
        sql.close_brace();
        let mut stmt = try!(self.prepare(&sql));
        let mut rows = try!(stmt.query(&[]));
        while let Some(result_row) = rows.next() {
            let row = try!(result_row);
            try!(f(&row));
        }
        Ok(())
    }

    /// Set a new value to `pragma_name`.
    ///
    /// Some pragmas will return the updated value which cannot be retrieved with this method.
    pub fn pragma_update(&self,
                         schema_name: Option<DatabaseName>,
                         pragma_name: &str,
                         pragma_value: &ToSql)
                         -> Result<()> {
        let mut sql = Sql::new();
        try!(sql.push_pragma(schema_name, pragma_name));
        // The argument is may be either in parentheses
        // or it may be separated from the pragma name by an equal sign.
        // The two syntaxes yield identical results.
        sql.push_equal_sign();
        try!(sql.push_value(pragma_value));
        self.execute_batch(&sql)
    }

    /// Set a new value to `pragma_name` and return the updated value.
    ///
    /// Only few pragmas automatically return the updated value.
    pub fn pragma_update_and_check<F, T>(&self,
                                         schema_name: Option<DatabaseName>,
                                         pragma_name: &str,
                                         pragma_value: &ToSql,
                                         f: F)
                                         -> Result<T>
        where F: FnOnce(Row) -> Result<T>
    {
        let mut sql = Sql::new();
        try!(sql.push_pragma(schema_name, pragma_name));
        // The argument is may be either in parentheses
        // or it may be separated from the pragma name by an equal sign.
        // The two syntaxes yield identical results.
        sql.push_equal_sign();
        try!(sql.push_value(pragma_value));
        let mut stmt = try!(self.prepare(&sql));
        let mut rows = try!(stmt.query(&[]));
        if let Some(result_row) = rows.next() {
            let row = try!(result_row);
            f(row)
        } else {
            Err(Error::QueryReturnedNoRows)
        }
    }
}

fn is_identifier(s: &str) -> bool {
    let chars = s.char_indices();
    for (i, ch) in chars {
        if i == 0 {
            if !is_identifier_start(ch) {
                return false;
            }
        } else if !is_identifier_continue(ch) {
            return false;
        }
    }
    true
}

fn is_identifier_start(c: char) -> bool {
    (c >= 'A' && c <= 'Z') || c == '_' || (c >= 'a' && c <= 'z') || c > '\x7F'
}

fn is_identifier_continue(c: char) -> bool {
    c == '$' || (c >= '0' && c <= '9') || (c >= 'A' && c <= 'Z') || c == '_' ||
    (c >= 'a' && c <= 'z') || c > '\x7F'
}


#[cfg(test)]
mod test {
    use {Connection, DatabaseName};
    use pragma::{self, Sql};

    #[test]
    fn pragma_query_value() {
        let db = Connection::open_in_memory().unwrap();
        let user_version: i32 =
            db.pragma_query_value(None, "user_version", |row| row.get_checked(0))
                .unwrap();
        assert_eq!(0, user_version);
    }

    #[test]
    fn pragma_query_no_schema() {
        let db = Connection::open_in_memory().unwrap();
        let mut user_version = -1;
        db.pragma_query(None, "user_version", |row| {
                user_version = try!(row.get_checked(0));
                Ok(())
            })
            .unwrap();
        assert_eq!(0, user_version);
    }

    #[test]
    fn pragma_query_with_schema() {
        let db = Connection::open_in_memory().unwrap();
        let mut user_version = -1;
        db.pragma_query(Some(DatabaseName::Main), "user_version", |row| {
                user_version = try!(row.get_checked(0));
                Ok(())
            })
            .unwrap();
        assert_eq!(0, user_version);
    }

    #[test]
    fn pragma() {
        let db = Connection::open_in_memory().unwrap();
        let mut columns = Vec::new();
        db.pragma(None, "table_info", &"sqlite_master", |row| {
                let column: String = try!(row.get_checked(1));
                columns.push(column);
                Ok(())
            })
            .unwrap();
        assert_eq!(5, columns.len());
    }

    #[test]
    fn pragma_update() {
        let db = Connection::open_in_memory().unwrap();
        db.pragma_update(None, "user_version", &1).unwrap();
    }

    #[test]
    fn pragma_update_and_check() {
        let db = Connection::open_in_memory().unwrap();
        let journal_mode: String =
            db.pragma_update_and_check(None, "journal_mode", &"OFF", |row| row.get_checked(0))
                .unwrap();
        assert_eq!("off", &journal_mode);
    }

    #[test]
    fn is_identifier() {
        assert!(pragma::is_identifier("full"));
        assert!(pragma::is_identifier("r2d2"));
        assert!(!pragma::is_identifier("sp ce"));
        assert!(!pragma::is_identifier("semi;colon"));
    }

    #[test]
    fn double_quote() {
        let mut sql = Sql::new();
        sql.push_schema_name(DatabaseName::Attached(r#"schema";--"#));
        assert_eq!(r#""schema"";--""#, sql.as_str());
    }

    #[test]
    fn wrap_and_escape() {
        let mut sql = Sql::new();
        sql.push_string_literal("value'; --");
        assert_eq!("'value''; --'", sql.as_str());
    }
}