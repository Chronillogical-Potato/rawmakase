use super::*;
use std::fs;

fn dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn an_exclusive_rename_keeps_the_file_object_and_its_time() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join("b.NEF"));
    fs::write(&a, b"synthetic raw").unwrap();
    let before = entry(&a).unwrap();
    let modified = fs::metadata(&a).unwrap().modified().unwrap();
    rename_exclusive(&a, &b).unwrap();
    assert!(matches!(entry(&a), Err(FsError::NotFound)));
    let after = entry(&b).unwrap();
    assert_eq!(after.id, before.id);
    assert_eq!(after.kind, EntryKind::File);
    assert_eq!(fs::metadata(&b).unwrap().modified().unwrap(), modified);
    assert_eq!(fs::read(&b).unwrap(), b"synthetic raw");
}

#[test]
fn an_exclusive_rename_never_replaces_a_file_and_keeps_both() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join("b.NEF"));
    fs::write(&a, b"first").unwrap();
    fs::write(&b, b"second").unwrap();
    assert!(matches!(
        rename_exclusive(&a, &b),
        Err(FsError::AlreadyExists)
    ));
    assert_eq!(fs::read(&a).unwrap(), b"first");
    assert_eq!(fs::read(&b).unwrap(), b"second");
}

#[test]
fn an_exclusive_rename_moves_a_directory_but_never_onto_one() {
    let d = dir();
    let (a, b, c) = (d.path().join("a"), d.path().join("b"), d.path().join("c"));
    fs::create_dir(&a).unwrap();
    fs::write(a.join("x.NEF"), b"x").unwrap();
    // A plain rename replaces an empty directory; this one doesn't.
    fs::create_dir(&c).unwrap();
    assert!(matches!(
        rename_exclusive(&a, &c),
        Err(FsError::AlreadyExists)
    ));
    assert!(a.join("x.NEF").is_file());
    rename_exclusive(&a, &b).unwrap();
    assert!(b.join("x.NEF").is_file());
    assert!(c.is_dir());
}

#[test]
fn a_folder_is_made_only_where_nothing_is() {
    let d = dir();
    let (a, file, link) = (d.path().join("a"), d.path().join("f"), d.path().join("l"));
    create_dir(&a).unwrap();
    assert_eq!(entry(&a).unwrap().kind, EntryKind::Directory);
    // Never adopted: a folder, a file, or a link there already.
    fs::write(a.join("x.NEF"), b"x").unwrap();
    assert!(matches!(create_dir(&a), Err(FsError::AlreadyExists)));
    assert!(a.join("x.NEF").is_file());
    fs::write(&file, b"file").unwrap();
    assert!(matches!(create_dir(&file), Err(FsError::AlreadyExists)));
    assert_eq!(fs::read(&file).unwrap(), b"file");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(d.path().join("nowhere"), &link).unwrap();
        assert!(matches!(create_dir(&link), Err(FsError::AlreadyExists)));
        assert!(!d.path().join("nowhere").exists());
    }
    let _ = link;
    // Its parent must be there; nothing is made on the way.
    assert!(matches!(
        create_dir(&d.path().join("b/c")),
        Err(FsError::NotFound)
    ));
    assert!(!d.path().join("b").exists());
}

#[test]
fn a_missing_source_is_not_found() {
    let d = dir();
    assert!(matches!(
        rename_exclusive(&d.path().join("gone"), &d.path().join("there")),
        Err(FsError::NotFound)
    ));
    assert!(matches!(
        entry(&d.path().join("gone")),
        Err(FsError::NotFound)
    ));
}

#[cfg(target_vendor = "apple")]
#[test]
fn a_case_only_rename_works_on_apfs() {
    let d = dir();
    let (upper, lower) = (d.path().join("ROUND.NEF"), d.path().join("round.NEF"));
    fs::write(&upper, b"r").unwrap();
    let id = entry(&upper).unwrap().id;
    rename_exclusive(&upper, &lower).unwrap();
    assert_eq!(list(d.path()).unwrap(), vec![OsString::from("round.NEF")]);
    assert_eq!(entry(&lower).unwrap().id, id);
}

#[cfg(target_vendor = "apple")]
#[test]
fn a_normalisation_only_rename_works_on_apfs() {
    let d = dir();
    let (nfc, nfd) = (
        d.path().join("caf\u{e9}.NEF"),
        d.path().join("cafe\u{301}.NEF"),
    );
    fs::write(&nfc, b"r").unwrap();
    let id = entry(&nfc).unwrap().id;
    rename_exclusive(&nfc, &nfd).unwrap();
    assert_eq!(
        list(d.path()).unwrap(),
        vec![OsString::from("cafe\u{301}.NEF")]
    );
    assert_eq!(entry(&nfd).unwrap().id, id);
}

#[test]
fn a_volume_says_whether_a_rename_can_change_only_a_names_spelling() {
    // Temporary folders are on APFS on macOS; elsewhere it isn't known, and
    // no volume is taken to.
    let d = dir();
    let facts = volume_facts(d.path()).unwrap();
    assert_eq!(facts.renames_spelling, cfg!(target_vendor = "apple"));
}

#[test]
fn file_ids_tell_files_apart_and_hard_links_are_counted() {
    let d = dir();
    let (a, b, c) = (d.path().join("a"), d.path().join("b"), d.path().join("c"));
    fs::write(&a, b"same bytes").unwrap();
    fs::write(&b, b"same bytes").unwrap();
    assert_ne!(entry(&a).unwrap().id, entry(&b).unwrap().id);
    assert_eq!(entry(&a).unwrap().links, 1);
    fs::hard_link(&a, &c).unwrap();
    assert_eq!(entry(&c).unwrap().id, entry(&a).unwrap().id);
    assert_eq!(entry(&a).unwrap().links, 2);
    assert_eq!(entry(d.path()).unwrap().kind, EntryKind::Directory);
    assert_eq!(
        entry(&a).unwrap().id.volume(),
        entry(d.path()).unwrap().id.volume()
    );
}

#[test]
fn a_file_id_reads_back_from_its_text() {
    let d = dir();
    let a = d.path().join("a");
    fs::write(&a, b"a").unwrap();
    let id = entry(&a).unwrap().id;
    assert_eq!(id.to_string().parse::<FileId>().unwrap(), id);
    assert!("nonsense".parse::<FileId>().is_err());
    assert!("1:".parse::<FileId>().is_err());
}

#[cfg(unix)]
#[test]
fn an_entry_is_the_link_itself_not_what_it_points_at() {
    let d = dir();
    let (a, link) = (d.path().join("a"), d.path().join("link"));
    fs::write(&a, b"a").unwrap();
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let e = entry(&link).unwrap();
    assert_eq!(e.kind, EntryKind::Symlink);
    assert_ne!(e.id, entry(&a).unwrap().id);
}

#[test]
fn a_listing_has_every_name_exactly() {
    let d = dir();
    for name in ["A.NEF", "a.xmp", "B.dng"] {
        fs::write(d.path().join(name), b"").unwrap();
    }
    let mut names = list(d.path()).unwrap();
    names.sort();
    assert_eq!(names, ["A.NEF", "B.dng", "a.xmp"].map(OsString::from));
    assert!(matches!(
        list(&d.path().join("gone")),
        Err(FsError::NotFound)
    ));
}

#[test]
fn a_folder_is_synced() {
    let d = dir();
    sync_dir(d.path()).unwrap();
    assert!(matches!(
        sync_dir(&d.path().join("gone")),
        Err(FsError::NotFound)
    ));
}

#[test]
fn an_empty_folder_says_nothing_about_its_names() {
    let d = dir();
    let rules = probe(d.path(), &list(d.path()).unwrap());
    assert_eq!(rules.case, Folding::Unknown);
    assert_eq!(rules.normalization, Folding::Unknown);
}

#[test]
fn a_folder_is_probed_through_its_own_names() {
    let d = dir();
    fs::write(d.path().join("Photo.NEF"), b"").unwrap();
    // NFC "é", which a normalising volume also finds as NFD.
    fs::write(d.path().join("caf\u{e9}.NEF"), b"").unwrap();
    let rules = probe(d.path(), &list(d.path()).unwrap());
    if cfg!(any(target_vendor = "apple", windows)) {
        // APFS and NTFS as formatted by default.
        assert_eq!(rules.case, Folding::Insensitive);
    } else {
        assert_eq!(rules.case, Folding::Exact);
    }
    if cfg!(target_vendor = "apple") {
        assert_eq!(rules.normalization, Folding::Insensitive);
    }
}

#[test]
fn a_photo_below_its_root_is_contained() {
    let d = dir();
    let root = d.path().join("root");
    fs::create_dir_all(root.join("Trip/Day 1")).unwrap();
    fs::write(root.join("Trip/Day 1/a.NEF"), b"").unwrap();
    let canonical = canonical_below(&root, &root.join("Trip/Day 1/a.NEF")).unwrap();
    // Name by name: a canonical Windows path (`\\?\`) has no `/`.
    assert_eq!(
        canonical,
        fs::canonicalize(&root)
            .unwrap()
            .join("Trip")
            .join("Day 1")
            .join("a.NEF")
    );
    assert_eq!(
        canonical_below(&root, &root).unwrap(),
        fs::canonicalize(&root).unwrap()
    );
    assert!(matches!(
        canonical_below(&root, &d.path().join("elsewhere")),
        Err(Containment::Outside)
    ));
    assert!(matches!(
        canonical_below(&root, &root.join("Trip/../Trip/Day 1/a.NEF")),
        Err(Containment::Outside)
    ));
    assert!(matches!(
        canonical_below(&root, &root.join("Trip/gone.NEF")),
        Err(Containment::Fs(FsError::NotFound))
    ));
}

#[cfg(unix)]
#[test]
fn a_symlink_between_root_and_photo_is_not_contained() {
    let d = dir();
    let root = d.path().join("root");
    let real = d.path().join("real");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("a.NEF"), b"").unwrap();
    std::os::unix::fs::symlink(&real, root.join("linked")).unwrap();
    std::os::unix::fs::symlink(real.join("a.NEF"), root.join("b.NEF")).unwrap();
    assert!(matches!(
        canonical_below(&root, &root.join("linked/a.NEF")),
        Err(Containment::Symlink(p)) if p.ends_with("linked")
    ));
    assert!(matches!(
        canonical_below(&root, &root.join("b.NEF")),
        Err(Containment::Symlink(p)) if p.ends_with("b.NEF")
    ));
    // A root reached through a link is its canonical self.
    let alias = d.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    assert_eq!(
        canonical_below(&alias, &alias.join("a.NEF")).unwrap(),
        fs::canonicalize(&real).unwrap().join("a.NEF")
    );
}

#[test]
fn a_temporary_folder_is_on_a_local_writable_volume() {
    let d = dir();
    let facts = volume_facts(d.path()).unwrap();
    assert!(facts.local);
    assert!(!facts.read_only);
    assert!(facts.available > 0, "a volume tests write on has room");
    // APFS and NTFS keep file ids; Linux's temporary folder may be tmpfs.
    if cfg!(any(target_vendor = "apple", windows)) {
        assert!(facts.stable_ids, "{facts:?}");
    }
}

#[test]
fn a_file_is_made_empty_and_only_where_nothing_is() {
    let d = dir();
    let (a, b) = (d.path().join("a.part"), d.path().join("b.NEF"));
    create_file(&a).unwrap();
    assert_eq!(fs::read(&a).unwrap(), b"");
    fs::write(&b, b"kept").unwrap();
    assert!(matches!(create_file(&b), Err(FsError::AlreadyExists)));
    assert!(matches!(create_file(&a), Err(FsError::AlreadyExists)));
    assert_eq!(fs::read(&b).unwrap(), b"kept", "never truncated");
    assert!(matches!(
        create_file(&d.path().join("no/c")),
        Err(FsError::NotFound)
    ));
}

/// Bytes over several copy chunks, distinct per position.
fn big() -> Vec<u8> {
    (0..2 * COPY_CHUNK as u32 + 77)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect()
}

#[test]
fn a_copy_has_every_byte_and_the_hash_of_what_was_read() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join(".a.part"));
    let bytes = big();
    fs::write(&a, &bytes).unwrap();
    create_file(&b).unwrap();
    let mut seen = Vec::new();
    let hash = copy_into(&a, &b, entry(&b).unwrap().id, &mut |n| {
        seen.push(n);
        true
    })
    .unwrap();
    assert_eq!(fs::read(&b).unwrap(), bytes);
    assert_eq!(hash, hash_file(&a).unwrap());
    assert_eq!(hash, hash_file(&b).unwrap());
    let chunk = COPY_CHUNK as u64;
    assert_eq!(seen, [chunk, 2 * chunk, 2 * chunk + 77]);
    assert_eq!(fs::read(&a).unwrap(), bytes, "the source is untouched");
    // SHA-256 of nothing, as journals keep it, and back.
    fs::write(&a, b"").unwrap();
    let empty = hash_file(&a).unwrap();
    assert_eq!(
        empty.to_string(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(empty.to_string().parse::<ContentHash>().unwrap(), empty);
    assert!("e3b0".parse::<ContentHash>().is_err());
}

#[test]
fn a_copy_stops_between_chunks_when_asked() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join(".a.part"));
    fs::write(&a, big()).unwrap();
    create_file(&b).unwrap();
    let mut calls = 0;
    let stopped = copy_into(&a, &b, entry(&b).unwrap().id, &mut |_| {
        calls += 1;
        false
    });
    assert!(matches!(stopped, Err(CopyError::Stopped)));
    assert_eq!(calls, 1);
    assert_eq!(fs::metadata(&b).unwrap().len(), COPY_CHUNK as u64);
}

#[test]
fn a_copy_goes_only_into_the_empty_file_it_was_given() {
    let d = dir();
    let (a, b, c) = (
        d.path().join("a.NEF"),
        d.path().join(".b.part"),
        d.path().join(".c.part"),
    );
    fs::write(&a, b"synthetic raw").unwrap();
    create_file(&b).unwrap();
    create_file(&c).unwrap();
    // Another file object than the one given.
    let other = entry(&c).unwrap().id;
    assert!(matches!(
        copy_into(&a, &b, other, &mut |_| true),
        Err(CopyError::NotTheFile)
    ));
    // A file with bytes in it already.
    fs::write(&b, b"x").unwrap();
    let id = entry(&b).unwrap().id;
    assert!(matches!(
        copy_into(&a, &b, id, &mut |_| true),
        Err(CopyError::NotTheFile)
    ));
    assert_eq!(fs::read(&b).unwrap(), b"x");
    // Nothing is made where there was nothing.
    let gone = d.path().join(".gone.part");
    assert!(matches!(
        copy_into(&a, &gone, id, &mut |_| true),
        Err(CopyError::Fs(FsError::NotFound))
    ));
    assert!(!gone.exists());
}

#[test]
fn a_copy_takes_the_sources_time_and_permissions() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join(".a.part"));
    fs::write(&a, b"synthetic raw").unwrap();
    let when = std::time::UNIX_EPOCH + std::time::Duration::new(1_600_000_000, 123_456_700);
    fs::File::options()
        .write(true)
        .open(&a)
        .unwrap()
        .set_modified(when)
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&a, fs::Permissions::from_mode(0o440)).unwrap();
    }
    create_file(&b).unwrap();
    copy_into(&a, &b, entry(&b).unwrap().id, &mut |_| true).unwrap();
    sync_file(&b).unwrap();
    copy_times(&a, &b).unwrap();
    // Read-only now, and still synced.
    sync_file(&b).unwrap();
    let (source, copy) = (fs::metadata(&a).unwrap(), fs::metadata(&b).unwrap());
    assert_eq!(copy.modified().unwrap(), source.modified().unwrap());
    assert_eq!(copy.modified().unwrap(), when);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(copy.permissions().mode() & 0o777, 0o440);
    }
    assert_eq!(fs::read(&b).unwrap(), b"synthetic raw");
    // Only an unfinished copy is removed, by its caller's proof.
    remove_file(&b).unwrap();
    assert!(!b.exists() && a.is_file());
}

#[test]
fn errors_say_what_callers_branch_on() {
    use std::io::{Error, ErrorKind};
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::AlreadyExists)),
        FsError::AlreadyExists
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::NotFound)),
        FsError::NotFound
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::PermissionDenied)),
        FsError::PermissionDenied
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::CrossesDevices)),
        FsError::CrossDevice
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::Unsupported)),
        FsError::Unsupported
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::StorageFull)),
        FsError::NoSpace
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::QuotaExceeded)),
        FsError::NoSpace
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::TimedOut)),
        FsError::Io(_)
    ));
}

#[test]
#[ignore = "needs RAWMAKASE_TEST_FAT_VOLUMES: folders on FAT or exFAT volumes, `:` between"]
fn fat_and_exfat_volumes_have_no_stable_ids() {
    let Some(dirs) = std::env::var_os("RAWMAKASE_TEST_FAT_VOLUMES") else {
        return;
    };
    for dir in std::env::split_paths(&dirs) {
        let facts = volume_facts(&dir).unwrap();
        assert!(!facts.stable_ids, "{}: {facts:?}", dir.display());
    }
}

#[test]
fn only_kinds_known_to_keep_file_ids_are_taken_to() {
    #[cfg(target_vendor = "apple")]
    {
        for kind in [&b"apfs"[..], b"hfs"] {
            assert!(super::unix::keeps_ids(kind), "{kind:?}");
        }
        for kind in [&b"msdos"[..], b"exfat", b"smbfs", b"macfuse", b"nfs", b""] {
            assert!(!super::unix::keeps_ids(kind), "{kind:?}");
        }
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        for kind in [0xef53, 0x5846_5342, 0x9123_683e] {
            assert!(super::unix::keeps_ids(kind), "{kind:x}");
        }
        for kind in [0x4d44, 0x2011_bab0, 0x6573_5546, 0x0102_1994, 0x6969] {
            assert!(!super::unix::keeps_ids(kind), "{kind:x}");
        }
    }
    #[cfg(windows)]
    {
        assert!(super::windows::keeps_ids("NTFS"));
        for kind in ["ReFS", "FAT32", "exFAT", "FAT", ""] {
            assert!(!super::windows::keeps_ids(kind), "{kind}");
        }
    }
}
