//! The portability check (issue #341): every portable statement must
//! prepare on Postgres against `schema.postgres.sql`, which must keep the
//! tables and columns `schema.sql` has. CI's "Portable catalog SQL" job
//! runs the script `writes_the_postgres_check` makes; Postgres parses and
//! type-checks each statement, with no data and no backend code.
use super::sql_scan::{Kind, catalog_statements};
use std::collections::BTreeMap;

/// Each table's columns, in order, and the indexes: what a schema defines.
fn shape(schema: &str) -> (BTreeMap<String, Vec<String>>, Vec<String>) {
    let mut tables = BTreeMap::new();
    let mut indexes = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in schema.lines().map(str::trim) {
        if line.starts_with("--") || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("CREATE TABLE IF NOT EXISTS ") {
            let name = rest.trim_end_matches(" (").trim().to_string();
            current = Some((name, Vec::new()));
        } else if let Some(rest) = line.strip_prefix("CREATE INDEX IF NOT EXISTS ") {
            indexes.push(
                rest.split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            );
        } else if line.starts_with(");") {
            if let Some((name, columns)) = current.take() {
                tables.insert(name, columns);
            }
        } else if let Some((_, columns)) = &mut current {
            let column = line.split_whitespace().next().unwrap_or_default();
            if !column.starts_with("PRIMARY") {
                columns.push(column.trim_matches('"').to_string());
            }
        }
    }
    (tables, indexes)
}

#[test]
fn the_postgres_schema_keeps_the_sqlite_tables_and_columns() {
    let sqlite = shape(include_str!("schema.sql"));
    let postgres = shape(include_str!("schema.postgres.sql"));
    assert!(sqlite.0.len() > 20, "{:?}", sqlite.0.keys());
    assert_eq!(postgres, sqlite);
}

/// `sql` with SQLite's placeholders as Postgres numbers them: `?N` is `$N`,
/// and a bare `?` one more than the largest number before it, as SQLite
/// counts. The catalog's SQL has no `?` inside a string or quoted name.
fn numbered(sql: &str) -> String {
    let mut out = String::new();
    let mut largest = 0;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '?' {
            out.push(c);
            continue;
        }
        let mut digits = String::new();
        while let Some(d) = chars.peek().filter(|d| d.is_ascii_digit()) {
            digits.push(*d);
            chars.next();
        }
        let n = if digits.is_empty() {
            largest + 1
        } else {
            digits.parse().unwrap()
        };
        largest = largest.max(n);
        out.push_str(&format!("${n}"));
    }
    out
}

#[test]
fn placeholders_are_numbered_as_sqlite_numbers_them() {
    assert_eq!(numbered("a=? AND b=?"), "a=$1 AND b=$2");
    assert_eq!(
        numbered("master_id=?1 WHERE x=?2 AND id<>?1"),
        "master_id=$1 WHERE x=$2 AND id<>$1"
    );
    assert_eq!(numbered("?2, ?, ?1"), "$2, $3, $1");
}

/// Writes the check CI runs: the Postgres schema, then a `PREPARE` of every
/// portable statement, to the file `RAWMAKASE_POSTGRES_CHECK` names.
#[test]
#[ignore = "makes CI's Postgres check; run with RAWMAKASE_POSTGRES_CHECK set"]
fn writes_the_postgres_check() {
    let path = std::env::var("RAWMAKASE_POSTGRES_CHECK").expect("RAWMAKASE_POSTGRES_CHECK");
    let mut script = String::from("\\set ON_ERROR_STOP on\n");
    script.push_str(include_str!("schema.postgres.sql"));
    let portable = catalog_statements()
        .into_iter()
        .filter(|s| s.kind == Kind::Portable);
    for (i, statement) in portable.enumerate() {
        script.push_str(&format!(
            "\n\\echo {}\nPREPARE s{i} AS {};\nDEALLOCATE s{i};\n",
            statement.file,
            numbered(&statement.text)
        ));
    }
    std::fs::write(path, script).unwrap();
}
