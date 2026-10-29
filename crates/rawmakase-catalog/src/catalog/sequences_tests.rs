//! What the cameras numbered is kept per master, read once per reader version.
use super::*;
use crate::storage::Stamp;
use rawmakase_interop::exif::{Mark, SEQUENCE_VERSION, Sequence};

fn catalog() -> Result<(tempfile::TempDir, Catalog, Vec<PhotoId>)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["a.arw", "b.arw", "c.arw", "d.arw", "e.arw"] {
        std::fs::write(folder.join(name), b"synthetic raw")?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let ids = cat.photos()?.iter().map(|p| p.id).collect();
    Ok((dir, cat, ids))
}
fn marked(mark: Mark) -> Option<Sequence> {
    Some(Sequence {
        camera: "SONY ILCE-1 1234".into(),
        mark,
    })
}
fn saved(sequence: Option<Sequence>) -> SavedSequence {
    SavedSequence {
        stamp: Stamp {
            size: 1234,
            modified_ns: 1_700_000_000_123_456_789,
        },
        sequence,
    }
}

#[test]
fn every_kind_of_mark_and_a_file_that_said_nothing_are_kept() -> Result<()> {
    let (_dir, mut cat, ids) = catalog()?;
    assert!(cat.sequences()?.is_empty());
    let read = [
        (ids[0], saved(marked(Mark::Frame(3)))),
        (ids[1], saved(marked(Mark::Single))),
        (ids[2], saved(marked(Mark::Burst("ABC-123".into())))),
        (ids[3], saved(None)),
        (ids[4], saved(marked(Mark::Continuous))),
    ];
    cat.save_sequences(&read)?;
    assert_eq!(cat.sequences()?, read.into_iter().collect());
    // Read again from a changed file: the new read replaces the old.
    let again = SavedSequence {
        stamp: Stamp {
            size: 99,
            modified_ns: 1,
        },
        ..saved(marked(Mark::Frame(1)))
    };
    cat.save_sequences(&[(ids[3], again.clone())])?;
    assert_eq!(cat.sequences()?[&ids[3]], again);
    Ok(())
}

#[test]
fn a_row_of_another_reader_version_is_read_again() -> Result<()> {
    let (_dir, mut cat, ids) = catalog()?;
    cat.save_sequences(&[(ids[0], saved(None)), (ids[1], saved(marked(Mark::Single)))])?;
    cat.db_for_tests().execute(
        "UPDATE photo_sequences SET version=? WHERE photo=?",
        [i64::from(SEQUENCE_VERSION) - 1, ids[0].0],
    )?;
    // And one no release writes.
    cat.db_for_tests().execute(
        "UPDATE photo_sequences SET burst='x' WHERE photo=?",
        [ids[1].0],
    )?;
    assert!(cat.sequences()?.is_empty());
    Ok(())
}

#[test]
fn a_virtual_copy_has_its_masters_and_set_copy_as_master_takes_it_over() -> Result<()> {
    let (_dir, mut cat, ids) = catalog()?;
    let copy = cat.create_virtual_copy(ids[0])?;
    // Read for the copy, as a copy's file is its master's: kept under the master.
    cat.save_sequences(&[(copy, saved(marked(Mark::Frame(2))))])?;
    assert_eq!(
        cat.sequences()?,
        [(ids[0], saved(marked(Mark::Frame(2))))]
            .into_iter()
            .collect()
    );
    cat.set_copy_as_master(copy)?;
    assert_eq!(
        cat.sequences()?,
        [(copy, saved(marked(Mark::Frame(2))))]
            .into_iter()
            .collect()
    );
    Ok(())
}

#[test]
fn a_removed_photo_takes_its_row_and_a_photo_removed_while_read_keeps_none() -> Result<()> {
    let (_dir, mut cat, ids) = catalog()?;
    cat.save_sequences(&[(ids[0], saved(marked(Mark::Single)))])?;
    cat.db.write(|w| super::copies::delete_photo(w, ids[0]))?;
    cat.save_sequences(&[(ids[0], saved(marked(Mark::Single)))])?;
    let rows: i64 =
        cat.db_for_tests()
            .query_row("SELECT count(*) FROM photo_sequences", [], |r| r.get(0))?;
    assert_eq!(rows, 0);
    Ok(())
}
