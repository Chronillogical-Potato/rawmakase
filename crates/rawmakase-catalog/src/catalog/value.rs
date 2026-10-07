//! The values the catalog stores and reads, in its own terms rather than a
//! database library's, so a second backend could carry them (issue #341).
//!
//! Decoding follows the rules the catalog has always read SQLite with:
//! integers only as integers, reals from integers too, any nonzero integer
//! as `true`, and narrower integers through checked conversions.
use crate::ids::{CollectionId, FolderId, PhotoId, RootId};
use std::fmt;

/// One stored value, borrowed: SQL's five storage classes. Text is bytes,
/// as a damaged catalog may hold text that isn't UTF-8.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::catalog) enum ValueRef<'a> {
    Null,
    Integer(i64),
    Real(f64),
    Text(&'a [u8]),
    Blob(&'a [u8]),
}

impl ValueRef<'_> {
    fn kind(&self) -> &'static str {
        match self {
            Self::Null => "NULL",
            Self::Integer(_) => "INTEGER",
            Self::Real(_) => "REAL",
            Self::Text(_) => "TEXT",
            Self::Blob(_) => "BLOB",
        }
    }
}

/// Why a stored value couldn't be read as the type asked for.
#[derive(Debug, PartialEq)]
pub(in crate::catalog) enum ValueError {
    /// A value of another storage class, such as text where a number belongs.
    InvalidType {
        expected: &'static str,
        found: &'static str,
    },
    /// An integer the type can't hold.
    OutOfRange(i64),
    /// Text that isn't UTF-8.
    InvalidText,
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidType { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            Self::OutOfRange(value) => write!(f, "integer {value} out of range"),
            Self::InvalidText => f.write_str("text is not UTF-8"),
        }
    }
}

impl std::error::Error for ValueError {}

/// A value the catalog can store.
pub(in crate::catalog) trait ToValue {
    fn to_value(&self) -> ValueRef<'_>;
}

/// A value the catalog can read from one column.
pub(in crate::catalog) trait FromValue: Sized {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError>;
}

fn invalid<T>(expected: &'static str, value: ValueRef<'_>) -> Result<T, ValueError> {
    Err(ValueError::InvalidType {
        expected,
        found: value.kind(),
    })
}

impl ToValue for i64 {
    fn to_value(&self) -> ValueRef<'_> {
        ValueRef::Integer(*self)
    }
}

impl FromValue for i64 {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        match value {
            ValueRef::Integer(i) => Ok(i),
            other => invalid("INTEGER", other),
        }
    }
}

/// Integers narrower than `i64`, written widened and read through a checked
/// conversion: an out-of-range value is an error, never truncated.
macro_rules! narrow_integer {
    ($($ty:ty),*) => {$(
        impl ToValue for $ty {
            fn to_value(&self) -> ValueRef<'_> {
                ValueRef::Integer(i64::from(*self))
            }
        }

        impl FromValue for $ty {
            fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
                let wide = i64::from_value(value)?;
                <$ty>::try_from(wide).map_err(|_| ValueError::OutOfRange(wide))
            }
        }
    )*};
}

narrow_integer!(i32, u32);

impl ToValue for f64 {
    fn to_value(&self) -> ValueRef<'_> {
        ValueRef::Real(*self)
    }
}

impl FromValue for f64 {
    /// A real, or an integer as a real (a photo's width and height are
    /// stored as integers and divided as reals).
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        match value {
            ValueRef::Integer(i) => Ok(i as f64),
            ValueRef::Real(f) => Ok(f),
            other => invalid("REAL", other),
        }
    }
}

impl ToValue for bool {
    /// 1 or 0.
    fn to_value(&self) -> ValueRef<'_> {
        ValueRef::Integer(i64::from(*self))
    }
}

impl FromValue for bool {
    /// Any nonzero integer is true: not every column holding a flag is
    /// constrained to 0 and 1.
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        i64::from_value(value).map(|i| i != 0)
    }
}

impl ToValue for str {
    fn to_value(&self) -> ValueRef<'_> {
        ValueRef::Text(self.as_bytes())
    }
}

impl ToValue for String {
    fn to_value(&self) -> ValueRef<'_> {
        self.as_str().to_value()
    }
}

impl FromValue for String {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        match value {
            ValueRef::Text(bytes) => std::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|_| ValueError::InvalidText),
            other => invalid("TEXT", other),
        }
    }
}

impl ToValue for std::borrow::Cow<'_, str> {
    fn to_value(&self) -> ValueRef<'_> {
        self.as_ref().to_value()
    }
}

impl ToValue for [u8] {
    fn to_value(&self) -> ValueRef<'_> {
        ValueRef::Blob(self)
    }
}

impl ToValue for Vec<u8> {
    fn to_value(&self) -> ValueRef<'_> {
        self.as_slice().to_value()
    }
}

impl FromValue for Vec<u8> {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        match value {
            ValueRef::Blob(bytes) => Ok(bytes.to_vec()),
            other => invalid("BLOB", other),
        }
    }
}

impl<T: ToValue + ?Sized> ToValue for &T {
    fn to_value(&self) -> ValueRef<'_> {
        (**self).to_value()
    }
}

impl<T: ToValue> ToValue for Option<T> {
    /// NULL for `None`.
    fn to_value(&self) -> ValueRef<'_> {
        self.as_ref().map_or(ValueRef::Null, ToValue::to_value)
    }
}

impl<T: FromValue> FromValue for Option<T> {
    /// `None` for NULL.
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        match value {
            ValueRef::Null => Ok(None),
            other => T::from_value(other).map(Some),
        }
    }
}

/// The bytes of a column some rows store as TEXT and others as BLOB, such
/// as Lightroom's settings text; `None` for a value of any other class,
/// which the reader skips or replaces as it sees fit.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::catalog) struct TextOrBlob(pub Option<Vec<u8>>);

impl FromValue for TextOrBlob {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        Ok(Self(match value {
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Some(bytes.to_vec()),
            _ => None,
        }))
    }
}

/// Typed ids are stored as their integer.
macro_rules! row_id {
    ($($id:ty),*) => {$(
        impl ToValue for $id {
            fn to_value(&self) -> ValueRef<'_> {
                ValueRef::Integer(self.0)
            }
        }

        impl FromValue for $id {
            fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
                i64::from_value(value).map(Self)
            }
        }
    )*};
}

row_id!(CollectionId, FolderId, PhotoId, RootId);

/// One row of a query's result, read by column index.
pub(in crate::catalog) struct Row<'a> {
    row: &'a rusqlite::Row<'a>,
}

impl<'a> Row<'a> {
    pub(in crate::catalog) fn new(row: &'a rusqlite::Row<'a>) -> Self {
        Self { row }
    }

    /// Column `index`, read as `T`.
    pub(in crate::catalog) fn get<T: FromValue>(&self, index: usize) -> anyhow::Result<T> {
        let value = self.row.get_ref(index)?.into();
        Ok(T::from_value(value).map_err(|error| ColumnError { index, error })?)
    }
}

/// A row the catalog can read: usually a named struct, or a single value
/// from a one-column query.
pub(in crate::catalog) trait FromRow: Sized {
    fn from_row(row: &Row<'_>) -> anyhow::Result<Self>;
}

impl<T: FromValue> FromRow for T {
    fn from_row(row: &Row<'_>) -> anyhow::Result<Self> {
        row.get(0)
    }
}

/// A value that couldn't be read, with the column it was in.
#[derive(Debug)]
struct ColumnError {
    index: usize,
    error: ValueError,
}

impl fmt::Display for ColumnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "column {}: {}", self.index, self.error)
    }
}

impl std::error::Error for ColumnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl<'a> From<rusqlite::types::ValueRef<'a>> for ValueRef<'a> {
    fn from(value: rusqlite::types::ValueRef<'a>) -> Self {
        use rusqlite::types::ValueRef as Sqlite;
        match value {
            Sqlite::Null => Self::Null,
            Sqlite::Integer(i) => Self::Integer(i),
            Sqlite::Real(f) => Self::Real(f),
            Sqlite::Text(bytes) => Self::Text(bytes),
            Sqlite::Blob(bytes) => Self::Blob(bytes),
        }
    }
}

impl rusqlite::ToSql for dyn ToValue + '_ {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, ValueRef as Sqlite};
        Ok(ToSqlOutput::Borrowed(match self.to_value() {
            ValueRef::Null => Sqlite::Null,
            ValueRef::Integer(i) => Sqlite::Integer(i),
            ValueRef::Real(f) => Sqlite::Real(f),
            ValueRef::Text(bytes) => Sqlite::Text(bytes),
            ValueRef::Blob(bytes) => Sqlite::Blob(bytes),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, types::FromSql};

    /// One value of each storage class SQLite stores, with edge cases for
    /// the narrower integers.
    const SAMPLES: &str = "SELECT NULL UNION ALL SELECT 0 UNION ALL SELECT 1
        UNION ALL SELECT 2 UNION ALL SELECT -1 UNION ALL SELECT 4294967296
        UNION ALL SELECT 2147483648 UNION ALL SELECT 2.5 UNION ALL SELECT 'text'
        UNION ALL SELECT CAST(X'FF' AS TEXT) UNION ALL SELECT X'0102'";

    /// Every sample read as `T` by these traits and by rusqlite.
    fn both<T: FromValue + FromSql>(db: &Connection) -> Vec<(Option<T>, Option<T>)> {
        let mut query = db.prepare(SAMPLES).unwrap();
        let mut rows = query.query([]).unwrap();
        let mut read = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            let ours = Row::new(row).get::<T>(0).ok();
            let theirs = row.get::<_, T>(0).ok();
            read.push((ours, theirs));
        }
        read
    }

    fn agree<T: FromValue + FromSql + PartialEq + fmt::Debug>(db: &Connection) {
        for (ours, theirs) in both::<T>(db) {
            assert_eq!(ours, theirs, "{}", std::any::type_name::<T>());
        }
    }

    #[test]
    fn values_decode_as_rusqlite_decodes_them() {
        let db = Connection::open_in_memory().unwrap();
        agree::<i64>(&db);
        agree::<i32>(&db);
        agree::<u32>(&db);
        agree::<f64>(&db);
        agree::<bool>(&db);
        agree::<String>(&db);
        agree::<Vec<u8>>(&db);
        agree::<Option<i64>>(&db);
        agree::<Option<f64>>(&db);
        agree::<Option<String>>(&db);
    }

    fn read<T: FromRow>(db: &Connection, sql: &str) -> anyhow::Result<T> {
        let mut query = db.prepare(sql)?;
        let mut rows = query.query([])?;
        T::from_row(&Row::new(rows.next()?.expect("one row")))
    }

    #[test]
    fn reals_read_integers_too() -> anyhow::Result<()> {
        let db = Connection::open_in_memory()?;
        // A photo's width and height are INTEGER columns, divided as reals.
        db.execute_batch(
            "CREATE TABLE info(width INTEGER, height INTEGER);
             INSERT INTO info VALUES (6000, 4000);",
        )?;
        let width: f64 = read(&db, "SELECT width FROM info")?;
        let height: f64 = read(&db, "SELECT height FROM info")?;
        assert_eq!(width / height, 1.5);
        assert!(read::<i64>(&db, "SELECT 1.5").is_err());
        Ok(())
    }

    #[test]
    fn any_nonzero_integer_is_true_and_true_is_stored_as_1() -> anyhow::Result<()> {
        let db = Connection::open_in_memory()?;
        // Keyword export flags have no 0/1 constraint.
        for (stored, flag) in [(0, false), (1, true), (2, true), (-1, true)] {
            assert_eq!(read::<bool>(&db, &format!("SELECT {stored}"))?, flag);
        }
        let stored: (String, i64) =
            db.query_row("SELECT typeof(?1), ?1", [&true as &dyn ToValue], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        assert_eq!(stored, ("integer".into(), 1));
        Ok(())
    }

    #[test]
    fn narrow_integers_refuse_values_out_of_their_range() -> anyhow::Result<()> {
        let db = Connection::open_in_memory()?;
        assert_eq!(read::<u32>(&db, "SELECT 4294967295")?, u32::MAX);
        let error = read::<u32>(&db, "SELECT 4294967296").unwrap_err();
        assert_eq!(
            error.downcast_ref::<ColumnError>().map(|e| &e.error),
            Some(&ValueError::OutOfRange(4_294_967_296))
        );
        assert!(read::<u32>(&db, "SELECT -1").is_err());
        assert!(read::<i32>(&db, "SELECT 2147483648").is_err());
        assert_eq!(read::<i32>(&db, "SELECT -2147483648")?, i32::MIN);
        Ok(())
    }

    #[test]
    fn text_or_blob_reads_either_and_nothing_else() -> anyhow::Result<()> {
        let db = Connection::open_in_memory()?;
        let read = |sql| read::<TextOrBlob>(&db, sql).map(|v| v.0);
        assert_eq!(read("SELECT 'settings'")?, Some(b"settings".to_vec()));
        assert_eq!(read("SELECT X'00789C'")?, Some(vec![0, 0x78, 0x9c]));
        assert_eq!(read("SELECT NULL")?, None);
        assert_eq!(read("SELECT 7")?, None);
        Ok(())
    }

    #[test]
    fn values_are_stored_in_their_own_class() -> anyhow::Result<()> {
        let db = Connection::open_in_memory()?;
        let class = |value: &dyn ToValue| -> rusqlite::Result<String> {
            db.query_row("SELECT typeof(?)", [value], |r| r.get(0))
        };
        assert_eq!(class(&PhotoId(7))?, "integer");
        assert_eq!(class(&7_u32)?, "integer");
        assert_eq!(class(&1.5)?, "real");
        assert_eq!(class(&"text")?, "text");
        assert_eq!(class(&vec![1_u8, 2])?, "blob");
        assert_eq!(class(&None::<i64>)?, "null");
        let id: FolderId = db.query_row("SELECT ?", [&FolderId(42) as &dyn ToValue], |r| {
            Ok(Row::new(r).get(0))
        })??;
        assert_eq!(id, FolderId(42));
        Ok(())
    }
}
