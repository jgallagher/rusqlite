//! Prepared statements cache for faster execution.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use {Result, Connection, Statement};

/// Prepared statements LRU cache.
#[derive(Debug)]
pub struct StatementCache<'conn> {
    conn: &'conn Connection,
    cache: RefCell<VecDeque<Statement<'conn>>>, // back = LRU
}

/// Cacheable statement.
///
/// Statement will return automatically to the cache by default.
/// If you want the statement to be discarded, call `discard()` on it.
pub struct CachedStatement<'c: 's, 's> {
    stmt: Option<Statement<'c>>,
    cache: &'s StatementCache<'c>,
}

impl<'c, 's> Deref for CachedStatement<'c, 's> {
    type Target = Statement<'c>;

    fn deref(&self) -> &Statement<'c> {
        self.stmt.as_ref().unwrap()
    }
}

impl<'c, 's> DerefMut for CachedStatement<'c, 's> {
    fn deref_mut(&mut self) -> &mut Statement<'c> {
        self.stmt.as_mut().unwrap()
    }
}

impl<'c, 's> Drop for CachedStatement<'c, 's> {
    #[allow(unused_must_use)]
    fn drop(&mut self) {
        if let Some(stmt) = self.stmt.take() {
            self.cache.cache_stmt(stmt);
        }
    }
}

impl<'c, 's> CachedStatement<'c, 's> {
    fn new(stmt: Statement<'c>, cache: &'s StatementCache<'c>) -> CachedStatement<'c, 's> {
        CachedStatement {
            stmt: Some(stmt),
            cache: cache,
        }
    }

    pub fn discard(mut self) {
        self.stmt = None;
    }
}

impl<'conn> StatementCache<'conn> {
    /// Create a statement cache.
    pub fn new(conn: &'conn Connection, capacity: usize) -> StatementCache<'conn> {
        StatementCache {
            conn: conn,
            cache: RefCell::new(VecDeque::with_capacity(capacity)),
        }
    }

    /// Search the cache for a prepared-statement object that implements `sql`.
    /// If no such prepared-statement can be found, allocate and prepare a new one.
    ///
    /// # Failure
    ///
    /// Will return `Err` if no cached statement can be found and the underlying SQLite prepare call fails.
    pub fn get<'s>(&'s self, sql: &str) -> Result<CachedStatement<'conn, 's>> {
        let mut cache = self.cache.borrow_mut();
        let stmt = match cache.iter().rposition(|entry| entry.eq(sql)) {
            Some(index) => Ok(cache.swap_remove_front(index).unwrap()), // FIXME Not LRU compliant
            _ => self.conn.prepare(sql),
        };
        stmt.map(|stmt| CachedStatement::new(stmt, self))
    }

    // Return a statement to the cache.
    fn cache_stmt(&self, mut stmt: Statement<'conn>) {
        if stmt.is_busy() {
            return;
        }
        let mut cache = self.cache.borrow_mut();
        if cache.capacity() == cache.len() {
            // is full
            cache.pop_back(); // LRU dropped
        }
        stmt.clear_bindings();
        cache.push_front(stmt)
    }

    /// Flush the prepared statement cache
    pub fn clear(&self) {
        self.cache.borrow_mut().clear();
    }

    /// Return current cache size.
    pub fn len(&self) -> usize {
        self.cache.borrow().len()
    }

    /// Return maximum cache size.
    pub fn capacity(&self) -> usize {
        self.cache.borrow().capacity()
    }
}

#[cfg(test)]
mod test {
    use Connection;
    use super::StatementCache;

    #[test]
    fn test_cache() {
        let db = Connection::open_in_memory().unwrap();
        let cache = StatementCache::new(&db, 15);
        assert_eq!(0, cache.len());
        assert_eq!(15, cache.capacity());

        let sql = "PRAGMA schema_version";
        {
            let mut stmt = cache.get(sql).unwrap();
            assert_eq!(0, cache.len());
            assert_eq!(0,
                       stmt.query(&[]).unwrap().get_expected_row().unwrap().get::<i32,i64>(0));
        }
        assert_eq!(1, cache.len());

        {
            let mut stmt = cache.get(sql).unwrap();
            assert_eq!(0, cache.len());
            assert_eq!(0,
                       stmt.query(&[]).unwrap().get_expected_row().unwrap().get::<i32,i64>(0));
        }
        assert_eq!(1, cache.len());

        cache.clear();
        assert_eq!(0, cache.len());
        assert_eq!(15, cache.capacity());
    }

    #[test]
    fn test_discard() {
        let db = Connection::open_in_memory().unwrap();
        let cache = StatementCache::new(&db, 15);

        let sql = "PRAGMA schema_version";
        {
            let mut stmt = cache.get(sql).unwrap();
            assert_eq!(0, cache.len());
            assert_eq!(0,
                       stmt.query(&[]).unwrap().get_expected_row().unwrap().get::<i32,i64>(0));
            stmt.discard();
        }
        assert_eq!(0, cache.len());
    }
}