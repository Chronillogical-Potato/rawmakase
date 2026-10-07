//! Finds the catalog's SQL statements in its source, for tests that check
//! them all: that only `edit_rows` writes a photo's edit, and that the
//! portable ones prepare on Postgres.
use std::path::{Path, PathBuf};

/// Which kind of statement a macro built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// `sql!`: portable.
    Portable,
    /// `sqlite_sql!`: SQLite only.
    Sqlite,
}

/// One statement found in the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Statement {
    pub file: String,
    pub kind: Kind,
    pub text: String,
}

/// Every statement in the catalog's production code (tests left out).
pub(super) fn catalog_statements() -> Vec<Statement> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/catalog");
    let mut files = Vec::new();
    sources(&root, &mut files);
    files.sort();
    let mut found = Vec::new();
    for path in files {
        let file = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let name = file.rsplit('/').next().unwrap_or(&file);
        if name == "tests.rs" || name.ends_with("_tests.rs") || name == "sql_scan.rs" {
            continue;
        }
        for (kind, text) in statements(&production(&path)) {
            found.push(Statement {
                file: file.clone(),
                kind,
                text,
            });
        }
    }
    found
}

/// `source` without its trailing inline test module (`#[cfg(test)] mod
/// tests { … }`). A `#[cfg(test)] mod tests;` declaration holds no code
/// and production code may follow it, so it is kept.
pub(super) fn without_tests(source: &str) -> &str {
    let mut from = 0;
    while let Some(found) = source[from..].find("#[cfg(test)]\nmod ") {
        let at = from + found;
        let line = source[at + "#[cfg(test)]\n".len()..]
            .lines()
            .next()
            .unwrap_or("");
        if line.trim_end().ends_with('{') {
            return &source[..at];
        }
        from = at + 1;
    }
    source
}

/// The production code of the file at `path`: without its inline test
/// module and without comments, whose examples don't run.
fn production(path: &Path) -> String {
    let source = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    without_tests(&source)
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// The string literals in `source`, in order, as (start, end, text).
fn literals(source: &str) -> Vec<(usize, usize, String)> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // A raw string: r"…" or r#"…"#.
        if bytes[i] == b'r'
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
            && matches!(bytes.get(i + 1), Some(b'"' | b'#'))
        {
            let hashes = bytes[i + 1..].iter().take_while(|b| **b == b'#').count();
            if bytes.get(i + 1 + hashes) == Some(&b'"') {
                let start = i + 2 + hashes;
                let end = format!("\"{}", "#".repeat(hashes));
                let close = source[start..]
                    .find(&end)
                    .map_or(bytes.len(), |n| start + n);
                found.push((i, close + end.len(), source[start..close].to_string()));
                i = close + end.len();
                continue;
            }
        }
        // A character literal can be a quote: '"'.
        if bytes[i] == b'\'' && bytes.get(i + 2) == Some(&b'\'') {
            i += 3;
            continue;
        }
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            let end = end.min(bytes.len());
            found.push((i, end + 1, unescape(&source[start..end])));
            i = end + 1;
            continue;
        }
        i += 1;
    }
    found
}

/// A string literal's text, with `\` line continuations and escapes read.
fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\n') => {
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// The text of each `macro_rules! name { () => { "…" }; }` in `source`:
/// SQL fragments that statements `concat!`.
fn fragments(source: &str) -> Vec<(String, String)> {
    let literals = literals(source);
    source
        .match_indices("macro_rules! ")
        .filter_map(|(at, _)| {
            let rest = &source[at + "macro_rules! ".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let body = literals.iter().find(|(start, _, _)| *start > at)?;
            Some((name, body.2.clone()))
        })
        .collect()
}

/// Each statement given to `sql!` or `sqlite_sql!` in `source`: its
/// literals and fragment macros joined in order, as `concat!` joins them.
/// Only these statements can run, so they are all the catalog's SQL.
pub(super) fn statements(source: &str) -> Vec<(Kind, String)> {
    let literals = literals(source);
    let fragments = fragments(source);
    let mut found = Vec::new();
    for (at, _) in source.match_indices("sql!(") {
        let kind = if source[..at].ends_with("sqlite_") {
            Kind::Sqlite
        } else {
            Kind::Portable
        };
        // The macro's arguments end at the parenthesis that closes it,
        // outside any literal.
        let mut depth = 0;
        let mut end = source.len();
        let mut i = at + "sql!".len();
        while i < source.len() {
            if let Some((_, after, _)) = literals.iter().find(|(start, _, _)| *start == i) {
                i = *after;
                continue;
            }
            match source.as_bytes()[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let mut pieces: Vec<(usize, &str)> = literals
            .iter()
            .filter(|(start, _, _)| (at..end).contains(start))
            .map(|(start, _, text)| (*start, text.as_str()))
            .collect();
        for (name, text) in &fragments {
            let call = format!("{name}!()");
            for (offset, _) in source[at..end].match_indices(&call) {
                pieces.push((at + offset, text));
            }
        }
        pieces.sort_by_key(|(start, _)| *start);
        found.push((kind, pieces.into_iter().map(|(_, text)| text).collect()));
    }
    found
}

#[test]
fn statements_are_read_whole() {
    let source = r##"
        macro_rules! tail {
            () => {
                " WHERE id=?"
            };
        }
        sql!(concat!("UPDATE photos ", "SET recipe=?", tail!()))
        sqlite_sql!(r#"DELETE FROM lr.x WHERE "photo"=?"#)
        sql!("SELECT 1 \
              FROM photos")
    "##;
    assert_eq!(
        statements(source),
        [
            (
                Kind::Portable,
                "UPDATE photos SET recipe=? WHERE id=?".into()
            ),
            (Kind::Sqlite, r#"DELETE FROM lr.x WHERE "photo"=?"#.into()),
            (Kind::Portable, "SELECT 1 FROM photos".into()),
        ]
    );
}

#[test]
fn only_inline_test_modules_are_left_out() {
    let source = "#[cfg(test)]\nmod tests;\nfn a() {}\n#[cfg(test)]\nmod more {\n}\n";
    assert_eq!(
        without_tests(source),
        "#[cfg(test)]\nmod tests;\nfn a() {}\n"
    );
}

#[test]
fn every_statement_in_the_catalog_is_found() {
    let found = catalog_statements();
    let mut written = 0;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/catalog");
    let mut files = Vec::new();
    sources(&root, &mut files);
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy();
        if name == "tests.rs" || name.ends_with("_tests.rs") || name == "sql_scan.rs" {
            continue;
        }
        written += production(&path).matches("sql!(").count();
    }
    assert_eq!(found.len(), written);
    assert!(found.len() > 100, "{}", found.len());
    for statement in &found {
        crate::catalog::db::assert_shape(&statement.text);
    }
}
