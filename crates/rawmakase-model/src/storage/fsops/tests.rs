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
        FsError::from(Error::from(ErrorKind::TimedOut)),
        FsError::Io(_)
    ));
}
