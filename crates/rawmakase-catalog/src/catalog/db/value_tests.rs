//! The value traits read stored values exactly as rusqlite did.
use super::{Param, Row};
use crate::catalog::value::*;
use crate::ids::{FolderId, PhotoId};
use rusqlite::{Connection, types::FromSql};
use std::fmt;

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
        let ours = Row { row }.get::<T>(0).ok();
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
    T::from_row(&Row {
        row: rows.next()?.expect("one row"),
    })
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
    let stored: (String, i64) = db.query_row("SELECT typeof(?1), ?1", [Param(&true)], |r| {
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
        error.downcast_ref::<ValueError>(),
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
        db.query_row("SELECT typeof(?)", [Param(value)], |r| r.get(0))
    };
    assert_eq!(class(&PhotoId(7))?, "integer");
    assert_eq!(class(&7_u32)?, "integer");
    assert_eq!(class(&1.5)?, "real");
    assert_eq!(class(&"text")?, "text");
    assert_eq!(class(&vec![1_u8, 2])?, "blob");
    assert_eq!(class(&None::<i64>)?, "null");
    let id: FolderId = db.query_row("SELECT ?", [Param(&FolderId(42))], |r| {
        Ok(Row { row: r }.get(0))
    })??;
    assert_eq!(id, FolderId(42));
    Ok(())
}
