//! A photo's edit changes only through `edit_rows`, and a virtual copy takes
//! its master's edit exactly as stored.
use super::*;
use crate::{export_settings::ExportOptions, model::recipe::Recipe};
use std::path::Path;

/// The catalog's statements that write a photo's edit columns or rows.
fn writes_an_edit(statement: &str) -> bool {
    // Words as SQL reads them: any whitespace between them is one space.
    let s = statement
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let edit_column = ["recipe", "export_options", "identity", "edited_at"]
        .iter()
        .any(|c| s.contains(c));
    let photos = (s.contains("update photos") || s.contains("insert into photos")) && edit_column;
    let rows = ["local_edits", "develop_history"].iter().any(|t| {
        s.contains(&format!("insert into {t}"))
            || s.contains(&format!("update {t}"))
            || s.contains(&format!("delete from {t}"))
    });
    photos || rows
}

/// The string literals in `source`, in order, with their ends: enough to
/// read SQL.
fn literals(source: &str) -> Vec<(usize, usize, String)> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // A raw string: r"…" or r#"…"#.
        if bytes[i] == b'r' && matches!(bytes.get(i + 1), Some(b'"' | b'#')) {
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
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            let end = end.min(bytes.len());
            found.push((i, end + 1, source[start..end].to_string()));
            i = end + 1;
            continue;
        }
        i += 1;
    }
    found
}

/// Each statement given to `sql!` or `sqlite_sql!` in `source`, its
/// literals joined as `concat!` joins them. A statement can only run as one
/// of these, so they are all a write to an edit could be.
fn statements(source: &str) -> Vec<String> {
    let literals = literals(source);
    let mut found = Vec::new();
    for (at, _) in source.match_indices("sql!(") {
        // The macro's arguments end at the parenthesis that closes it,
        // outside any literal.
        let mut depth = 0;
        let mut end = at + "sql!".len();
        let mut i = end;
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
        found.push(
            literals
                .iter()
                .filter(|(start, _, _)| (at..end).contains(start))
                .map(|(_, _, text)| text.as_str())
                .collect(),
        );
    }
    found
}

#[test]
fn only_edit_rows_writes_a_photos_edit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/catalog");
    let mut offenders = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let tests = name == "tests.rs" || name.ends_with("_tests.rs");
            if !name.ends_with(".rs") || tests || name == "edit_rows.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let production = source
                .find("#[cfg(test)]\nmod ")
                .map_or(&source[..], |at| &source[..at]);
            for statement in statements(production) {
                if writes_an_edit(&statement) {
                    offenders.push(format!("{name}: {}", statement.trim()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "write edits through edit_rows: {offenders:#?}"
    );
}

#[test]
fn the_scan_finds_edit_writes() {
    assert!(writes_an_edit("UPDATE photos SET recipe=? WHERE id=?"));
    assert!(writes_an_edit("UPDATE\n    photos SET recipe=? WHERE id=?"));
    assert!(writes_an_edit("DELETE  FROM\tlocal_edits WHERE photo=?"));
    assert!(writes_an_edit("DELETE FROM local_edits WHERE photo=?"));
    assert!(writes_an_edit(
        "INSERT INTO develop_history(photo, data) VALUES (?, ?)"
    ));
    assert!(!writes_an_edit("UPDATE photos SET rating=? WHERE id=?"));
    assert!(!writes_an_edit(
        "UPDATE develop_snapshots SET recipe=? WHERE id=?"
    ));
    assert!(!writes_an_edit("SELECT recipe FROM photos WHERE id=?"));
    // Pieces joined by concat!, and raw strings.
    let source = r##"sql!(concat!("UPDATE photos ", "SET recipe=? WHERE id=?")) sql!(r#"DELETE FROM local_edits WHERE "photo"=?"#) sql!("SELECT 1")"##;
    let found = statements(source);
    assert_eq!(found.len(), 3);
    assert!(writes_an_edit(&found[0]) && writes_an_edit(&found[1]));
    assert!(!writes_an_edit(&found[2]));
}

/// A catalog with one photo carrying a saved edit and a History, its file and id.
fn edited() -> Result<(tempfile::TempDir, Catalog, PathBuf, PhotoId)> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let chart =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng");
    let file = photos.join("a.dng");
    std::fs::copy(chart, &file)?;
    let mut cat = Catalog::create(&dir.path().join("Copies.rawmakase"))?;
    cat.add_folder(&photos)?;
    let id = cat.photos()?[0].id;
    let recipe = Recipe {
        exposure: 0.5,
        ..Default::default()
    };
    cat.save_edit(
        id,
        &file,
        &recipe,
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    Ok((dir, cat, file, id))
}

/// The edit as stored: the photo's edit columns, its spots and masks and
/// its History bytes.
fn stored(cat: &Catalog, id: PhotoId) -> Result<[Option<Vec<u8>>; 6]> {
    let db = cat.db_for_tests();
    let column = |sql: &str| -> rusqlite::Result<Option<Vec<u8>>> {
        db.query_row(sql, [id.0], |r| {
            Ok(match r.get_ref(0)? {
                rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => {
                    Some(t.to_vec())
                }
                _ => None,
            })
        })
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })
    };
    Ok([
        column("SELECT recipe FROM photos WHERE id=?")?,
        column("SELECT export_options FROM photos WHERE id=?")?,
        column("SELECT identity FROM photos WHERE id=?")?,
        column("SELECT edited_at FROM photos WHERE id=?")?,
        column("SELECT data FROM local_edits WHERE photo=?")?,
        column("SELECT data FROM develop_history WHERE photo=?")?,
    ])
}

#[test]
fn a_virtual_copy_takes_its_masters_edit_exactly_as_stored() -> Result<()> {
    let (_dir, mut cat, file, master) = edited()?;
    let db = cat.db_for_tests();
    // Saved long ago, with spots and a History no release can read yet.
    db.execute(
        "UPDATE photos SET edited_at='2020-01-02 03:04:05' WHERE id=?",
        [master.0],
    )?;
    db.execute(
        "INSERT INTO local_edits(photo, data) VALUES (?, '{\"spots\":\"from the future\"}')",
        [master.0],
    )?;
    db.execute(
        "INSERT INTO develop_history(photo, data) VALUES (?, X'00FF00FF')",
        [master.0],
    )?;
    // And the photo is offline.
    std::fs::remove_file(&file)?;
    let copy = cat.create_virtual_copy(master)?;
    let (from, to) = (stored(&cat, master)?, stored(&cat, copy)?);
    assert_eq!(to, from);
    assert_eq!(to[3].as_deref(), Some(&b"2020-01-02 03:04:05"[..]));
    assert_eq!(to[5].as_deref(), Some(&[0, 0xff, 0, 0xff][..]));
    Ok(())
}

#[test]
fn removing_a_virtual_copy_removes_its_edit_and_leaves_its_masters() -> Result<()> {
    let (_dir, mut cat, _file, master) = edited()?;
    cat.db_for_tests().execute(
        "INSERT INTO develop_history(photo, data) VALUES (?, X'01')",
        [master.0],
    )?;
    let copy = cat.create_virtual_copy(master)?;
    let kept = stored(&cat, master)?;
    cat.remove_virtual_copy(copy)?;
    assert_eq!(stored(&cat, copy)?, [None, None, None, None, None, None]);
    assert_eq!(stored(&cat, master)?, kept);
    Ok(())
}

#[test]
fn clearing_an_edit_leaves_nothing_of_it() -> Result<()> {
    let (_dir, mut cat, _file, id) = edited()?;
    cat.db_for_tests().execute(
        "INSERT INTO develop_history(photo, data) VALUES (?, X'01')",
        [id.0],
    )?;
    cat.change_edits(&[EditChange::Clear { id }])?;
    assert_eq!(stored(&cat, id)?, [None, None, None, None, None, None]);
    Ok(())
}
