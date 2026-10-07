//! One catalog opened on several computers, each with its own folder
//! locations (issue #187).
use super::locations::{Computer, Overrides, resolve_in};
use super::*;
use rusqlite::params;

fn computer(id: &str) -> Computer {
    Computer {
        id: id.into(),
        name: id.into(),
    }
}
fn mac() -> Computer {
    computer("mac")
}
fn linux() -> Computer {
    computer("linux")
}
/// A synthetic photo file at `dir/name`, with its folders.
fn photo(dir: &Path, name: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(name), format!("synthetic raw {name}"))?;
    Ok(())
}
fn new_catalog(dir: &Path) -> Result<PathBuf> {
    let path = dir.join("Shared.rawmakase");
    Catalog::create(&path)?;
    Ok(path)
}
/// Where each photo of the catalog resolves, by file name, on `who`.
fn paths(catalog: &Path, who: &Computer) -> Result<Vec<(String, PathBuf)>> {
    let mut photos: Vec<_> = Catalog::open_as(catalog, who)?
        .photos()?
        .into_iter()
        .filter(|p| p.master.is_none())
        .map(|p| (p.filename, p.path))
        .collect();
    photos.sort();
    Ok(photos)
}
fn path_of(catalog: &Path, who: &Computer, name: &str) -> Result<PathBuf> {
    Ok(paths(catalog, who)?
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("{name} is not in the catalog"))
        .1)
}
/// What an older release sees and writes: the legacy columns only.
fn legacy(catalog: &Path) -> Result<rusqlite::Connection> {
    Ok(rusqlite::Connection::open(catalog)?)
}
/// A folder by logical path; one an older release just added, by its own.
fn folder_id(catalog: &Path, relative: &str) -> Result<FolderId> {
    Ok(legacy(catalog)?.query_row(
        "SELECT f.id FROM folders f LEFT JOIN folder_paths p ON p.folder=f.id
         WHERE COALESCE(p.path, f.relative_path)=?",
        [relative],
        |r| r.get(0).map(FolderId),
    )?)
}
fn count(catalog: &Path, table: &str) -> Result<i64> {
    Ok(legacy(catalog)?.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?)
}

#[test]
fn mac_linux_mac_keeps_both_locations() -> Result<()> {
    let d = tempfile::tempdir()?;
    let on_mac = d.path().join("Volumes/Photos");
    let on_linux = d.path().join("mnt/nas/photos");
    for share in [&on_mac, &on_linux] {
        photo(&share.join("2026/Trip"), "DSC_1234.NEF")?;
    }
    let catalog = new_catalog(d.path())?;
    let mut cat = Catalog::open_as(&catalog, &mac())?;
    cat.add_folder(&on_mac)?;
    let root = cat.roots()?[0].0;
    drop(cat);
    let on_mac = on_mac.canonicalize()?;
    let on_linux = on_linux.canonicalize()?;
    let mac_photo = on_mac.join("2026/Trip/DSC_1234.NEF");
    let linux_photo = on_linux.join("2026/Trip/DSC_1234.NEF");

    Catalog::open_as(&catalog, &linux())?.relink_root(root, &on_linux)?;
    assert_eq!(path_of(&catalog, &linux(), "DSC_1234.NEF")?, linux_photo);
    assert_eq!(path_of(&catalog, &mac(), "DSC_1234.NEF")?, mac_photo);
    // Back on Linux, nothing moved.
    assert_eq!(path_of(&catalog, &linux(), "DSC_1234.NEF")?, linux_photo);
    // An older release still sees the last change anywhere.
    let mapped: Option<String> =
        legacy(&catalog)?.query_row("SELECT mapped_path FROM roots WHERE id=?", [root.0], |r| {
            r.get(0)
        })?;
    assert_eq!(mapped.map(PathBuf::from), Some(on_linux));
    Ok(())
}

#[test]
fn a_computer_adopts_the_legacy_mapping_once_and_ignores_later_ones() -> Result<()> {
    let d = tempfile::tempdir()?;
    let added = d.path().join("added");
    let first = d.path().join("first");
    let later = d.path().join("later");
    for share in [&added, &first, &later] {
        photo(share, "a.NEF")?;
    }
    let catalog = new_catalog(d.path())?;
    Catalog::open_as(&catalog, &mac())?.add_folder(&added)?;
    // An older release relinks the root before Linux first opens the catalog.
    legacy(&catalog)?.execute("UPDATE roots SET mapped_path=?", [first.to_string_lossy()])?;
    assert_eq!(path_of(&catalog, &linux(), "a.NEF")?, first.join("a.NEF"));
    // A later relink by an older release no longer moves Linux's photos.
    legacy(&catalog)?.execute("UPDATE roots SET mapped_path=?", [later.to_string_lossy()])?;
    assert_eq!(path_of(&catalog, &linux(), "a.NEF")?, first.join("a.NEF"));
    // The Mac adopted before either relink and keeps where it added the folder.
    assert_eq!(
        path_of(&catalog, &mac(), "a.NEF")?,
        added.canonicalize()?.join("a.NEF")
    );
    Ok(())
}

#[test]
fn changing_one_folder_keeps_every_sibling_where_it_was() -> Result<()> {
    let d = tempfile::tempdir()?;
    let added = d.path().join("added");
    let relinked = d.path().join("relinked");
    let elsewhere = d.path().join("elsewhere/Trip");
    for share in [&added, &relinked] {
        photo(&share.join("Trip"), "trip.NEF")?;
        photo(&share.join("Home"), "home.NEF")?;
    }
    photo(&elsewhere, "trip.NEF")?;
    let catalog = new_catalog(d.path())?;
    Catalog::open_as(&catalog, &mac())?.add_folder(&added)?;
    legacy(&catalog)?.execute(
        "UPDATE roots SET mapped_path=?",
        [relinked.to_string_lossy()],
    )?;
    let trip = folder_id(&catalog, "Trip")?;
    Catalog::open_as(&catalog, &linux())?.relink_folder(trip, &elsewhere)?;
    assert_eq!(
        path_of(&catalog, &linux(), "trip.NEF")?,
        elsewhere.join("trip.NEF")
    );
    assert_eq!(
        path_of(&catalog, &linux(), "home.NEF")?,
        relinked.join("Home/home.NEF")
    );
    Ok(())
}

#[test]
fn changing_a_root_keeps_or_clears_an_adopted_override() -> Result<()> {
    let d = tempfile::tempdir()?;
    let mac_root = d.path().join("Volumes/Photos");
    let mac_archive = d.path().join("Volumes/Archive/Trip");
    let linux_root = d.path().join("mnt/photos");
    photo(&mac_root.join("Home"), "home.NEF")?;
    photo(&mac_root.join("Trip"), "trip.NEF")?;
    photo(&mac_archive, "trip.NEF")?;
    photo(&linux_root.join("Home"), "home.NEF")?;
    photo(&linux_root.join("Trip"), "trip.NEF")?;
    let catalog = new_catalog(d.path())?;
    Catalog::open_as(&catalog, &computer("old"))?.add_folder(&mac_root)?;
    let trip = folder_id(&catalog, "Trip")?;
    // An older release on the Mac moved Trip to the archive.
    legacy(&catalog)?.execute(
        "INSERT INTO folder_mappings(folder,path) VALUES(?,?)",
        params![trip.0, mac_archive.to_string_lossy()],
    )?;
    let root = Catalog::open_as(&catalog, &linux())?.roots()?[0].0;

    let mut cat = Catalog::open_as(&catalog, &linux())?;
    let overrides = cat.root_overrides(root)?;
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].relative, "Trip");
    assert_eq!(overrides[0].path, mac_archive);
    cat.relink_root_with(root, &linux_root, Overrides::Keep)?;
    assert_eq!(
        path_of(&catalog, &linux(), "trip.NEF")?,
        mac_archive.join("trip.NEF")
    );
    assert_eq!(
        path_of(&catalog, &linux(), "home.NEF")?,
        linux_root.join("Home/home.NEF")
    );

    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.relink_root_with(root, &linux_root, Overrides::Clear)?;
    assert!(cat.root_overrides(root)?.is_empty());
    assert_eq!(
        path_of(&catalog, &linux(), "trip.NEF")?,
        linux_root.join("Trip/trip.NEF")
    );
    // Cleared for older releases too, in the same change.
    assert_eq!(count(&catalog, "folder_mappings")?, 0);
    Ok(())
}

#[test]
fn clearing_falls_back_to_the_nearest_remaining_location() -> Result<()> {
    let d = tempfile::tempdir()?;
    let added = d.path().join("added");
    let root_here = d.path().join("root");
    let year_here = d.path().join("year");
    let trip_here = d.path().join("trip");
    photo(&added.join("2026/Trip"), "a.NEF")?;
    let catalog = new_catalog(d.path())?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.add_folder(&added)?;
    let root = cat.roots()?[0].0;
    for dir in [&root_here, &year_here, &trip_here] {
        std::fs::create_dir_all(dir)?;
    }
    drop(cat);
    // The parent folder needs a row of its own to be relinked.
    legacy(&catalog)?.execute(
        "INSERT INTO folders(root,relative_path) VALUES(?, '2026')",
        [root.0],
    )?;
    let year = folder_id(&catalog, "2026")?;
    let trip = folder_id(&catalog, "2026/Trip")?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.relink_root(root, &root_here)?;
    cat.relink_folder(year, &year_here)?;
    cat.relink_folder(trip, &trip_here)?;
    // The most specific location wins.
    assert_eq!(
        path_of(&catalog, &linux(), "a.NEF")?,
        trip_here.join("a.NEF")
    );

    cat.clear_folder_location(root, "2026/Trip")?;
    assert_eq!(
        path_of(&catalog, &linux(), "a.NEF")?,
        year_here.join("Trip/a.NEF")
    );
    let legacy_rows: Vec<FolderId> = legacy(&catalog)?
        .prepare("SELECT folder FROM folder_mappings")?
        .query_map([], |r| r.get(0).map(FolderId))?
        .collect::<rusqlite::Result<_>>()?;
    assert_eq!(legacy_rows, [year]);

    cat.clear_folder_location(root, "2026")?;
    assert_eq!(
        path_of(&catalog, &linux(), "a.NEF")?,
        root_here.join("2026/Trip/a.NEF")
    );
    assert_eq!(count(&catalog, "folder_mappings")?, 0);

    // Clearing the root keeps the folder overrides below it.
    cat.relink_folder(trip, &trip_here)?;
    cat.clear_folder_location(root, "")?;
    assert_eq!(
        path_of(&catalog, &linux(), "a.NEF")?,
        trip_here.join("a.NEF")
    );
    assert_eq!(cat.root_overrides(root)?.len(), 1);
    let mapped: Option<String> =
        legacy(&catalog)?.query_row("SELECT mapped_path FROM roots WHERE id=?", [root.0], |r| {
            r.get(0)
        })?;
    assert_eq!(mapped, None);
    cat.clear_folder_location(root, "2026/Trip")?;
    assert_eq!(
        path_of(&catalog, &linux(), "a.NEF")?,
        added.canonicalize()?.join("2026/Trip/a.NEF")
    );
    Ok(())
}

#[test]
fn an_offline_location_stays_offline_without_falling_back() -> Result<()> {
    let d = tempfile::tempdir()?;
    let share = d.path().join("share");
    let unmounted = d.path().join("mnt/nas");
    photo(&share, "a.NEF")?;
    std::fs::create_dir_all(&unmounted)?;
    let catalog = new_catalog(d.path())?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.add_folder(&share)?;
    let root = cat.roots()?[0].0;
    cat.relink_root(root, &unmounted)?;
    drop(cat);
    std::fs::remove_dir(&unmounted)?;
    // The photo exists where it was added and where the Mac has it, but this
    // computer's own location wins.
    Catalog::open_as(&catalog, &mac())?.relink_root(root, &share)?;
    let path = path_of(&catalog, &linux(), "a.NEF")?;
    assert_eq!(path, unmounted.join("a.NEF"));
    assert!(!path.is_file());
    Ok(())
}

#[test]
fn adding_the_same_share_on_a_second_computer_adds_nothing() -> Result<()> {
    let d = tempfile::tempdir()?;
    let on_mac = d.path().join("Volumes/Photos");
    let on_linux = d.path().join("mnt/nas");
    for share in [&on_mac, &on_linux] {
        photo(&share.join("2026/Trip"), "a.NEF")?;
        photo(&share.join("2026/Trip"), "b.NEF")?;
    }
    let catalog = new_catalog(d.path())?;
    let mut cat = Catalog::open_as(&catalog, &mac())?;
    cat.add_folder(&on_mac)?;
    let root = cat.roots()?[0].0;
    let a = cat
        .photos()?
        .iter()
        .find(|p| p.filename == "a.NEF")
        .unwrap()
        .id;
    cat.create_virtual_copy(a)?;
    drop(cat);
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.relink_root(root, &on_linux)?;
    assert_eq!(cat.add_folder(&on_linux)?, 0);
    assert_eq!(cat.add_folder(&on_linux.join("2026"))?, 0);
    assert_eq!(cat.roots()?.len(), 1);
    assert_eq!(cat.photos()?.len(), 3);
    // A new photo in the share joins the existing folder.
    photo(&on_linux.join("2026/Trip"), "c.NEF")?;
    assert_eq!(cat.add_folder(&on_linux.join("2026/Trip"))?, 1);
    assert_eq!(cat.roots()?.len(), 1);
    assert_eq!(count(&catalog, "folders")?, 1);
    Ok(())
}

#[test]
fn adding_a_root_again_where_it_was_added_adds_nothing() -> Result<()> {
    let d = tempfile::tempdir()?;
    let share = d.path().join("share");
    photo(&share.join("Trip"), "a.NEF")?;
    let catalog = new_catalog(d.path())?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.add_folder(&share)?;
    // No location row: the root's original path is the candidate.
    assert_eq!(cat.add_folder(&share)?, 0);
    assert_eq!(cat.add_folder(&share.join("Trip"))?, 0);
    photo(&share.join("Trip/Day2"), "b.NEF")?;
    assert_eq!(cat.add_folder(&share.join("Trip/Day2"))?, 1);
    assert_eq!(cat.roots()?.len(), 1);
    assert_eq!(
        path_of(&catalog, &linux(), "b.NEF")?,
        share.canonicalize()?.join("Trip/Day2/b.NEF")
    );
    Ok(())
}

#[test]
fn adding_a_parent_of_a_separately_located_folder() -> Result<()> {
    let d = tempfile::tempdir()?;
    let added = d.path().join("Volumes/Photos");
    let linux_root = d.path().join("mnt/photos");
    let other = d.path().join("mnt/other");
    photo(&added.join("2026/Trip"), "a.NEF")?;
    photo(&linux_root.join("2026/Home"), "home.NEF")?;
    photo(&other.join("Trip"), "a.NEF")?;
    let catalog = new_catalog(d.path())?;
    let mut cat = Catalog::open_as(&catalog, &mac())?;
    cat.add_folder(&added)?;
    let root = cat.roots()?[0].0;
    drop(cat);
    let trip = folder_id(&catalog, "2026/Trip")?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.relink_root(root, &linux_root)?;
    cat.relink_folder(trip, &other.join("Trip"))?;
    // New here: a photo in the relinked Trip and a folder no location contains.
    photo(&other.join("Trip"), "b.NEF")?;
    photo(&other.join("Misc"), "misc.NEF")?;
    assert_eq!(cat.add_folder(&other)?, 2);
    let roots = cat.roots()?;
    assert_eq!(roots.len(), 2);
    let photos = cat.photos()?;
    let b = photos.iter().find(|p| p.filename == "b.NEF").unwrap();
    let a = photos.iter().find(|p| p.filename == "a.NEF").unwrap();
    assert_eq!(b.folder, a.folder);
    let misc = photos.iter().find(|p| p.filename == "misc.NEF").unwrap();
    assert_ne!(
        cat.folders()?
            .iter()
            .find(|f| f.id == misc.folder)
            .unwrap()
            .root,
        root
    );
    assert_eq!(misc.path, other.canonicalize()?.join("Misc/misc.NEF"));
    Ok(())
}

/// A root at /mnt/photos whose Trip is relinked to /mnt/archive/Trip, with an
/// old copy of Trip left at /mnt/photos/Trip.
fn stale_copy(d: &Path) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let photos = d.join("mnt/photos");
    let archive = d.join("mnt/archive/Trip");
    photo(&photos.join("Trip"), "trip.NEF")?;
    photo(&photos.join("Home"), "home.NEF")?;
    let catalog = new_catalog(d)?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.add_folder(&photos)?;
    std::fs::create_dir_all(archive.parent().unwrap())?;
    std::fs::rename(photos.join("Trip"), &archive)?;
    let trip = folder_id(&catalog, "Trip")?;
    cat.relink_folder(trip, &archive)?;
    // The old copy: the moved photo again, and one the archive never had.
    photo(&photos.join("Trip"), "trip.NEF")?;
    photo(&photos.join("Trip"), "stale.NEF")?;
    photo(&photos.join("Trip/Day2"), "stale2.NEF")?;
    Ok((catalog, photos, archive))
}

#[test]
fn importing_a_stale_folder_adds_nothing_and_says_why() -> Result<()> {
    let d = tempfile::tempdir()?;
    let (catalog, photos, archive) = stale_copy(d.path())?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    let added = cat.import_folder(&photos.join("Trip"), &Default::default(), &[])?;
    assert_eq!(added.added, 0);
    assert!(added.ambiguous.is_empty());
    let stale = photos.canonicalize()?.join("Trip");
    assert!(
        added
            .conflicts
            .iter()
            .any(|c| c.directory == stale && c.relative == "Trip" && c.located == archive)
    );
    assert_eq!(cat.roots()?.len(), 1);
    assert_eq!(cat.photos()?.len(), 2);
    assert_eq!(
        path_of(&catalog, &linux(), "trip.NEF")?,
        archive.join("trip.NEF")
    );
    Ok(())
}

#[test]
fn importing_the_parent_of_a_stale_folder_adds_only_new_photos() -> Result<()> {
    let d = tempfile::tempdir()?;
    let (catalog, photos, _) = stale_copy(d.path())?;
    photo(&photos.join("New"), "new.NEF")?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    let added = cat.import_folder(&photos, &Default::default(), &[])?;
    assert_eq!(added.added, 1);
    assert!(!added.conflicts.is_empty());
    assert_eq!(cat.roots()?.len(), 1);
    let names: Vec<_> = cat.photos()?.into_iter().map(|p| p.filename).collect();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"new.NEF".to_string()));
    assert!(!names.iter().any(|n| n.starts_with("stale")));
    let stale = photos.canonicalize()?.join("Trip");
    assert!(!cat.photos()?.iter().any(|p| p.path.starts_with(&stale)));
    Ok(())
}

#[test]
fn equally_specific_locations_ask_instead_of_choosing() -> Result<()> {
    let d = tempfile::tempdir()?;
    let share = d.path().join("share");
    let other = d.path().join("other");
    photo(&share, "a.NEF")?;
    photo(&other, "b.NEF")?;
    let catalog = new_catalog(d.path())?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.add_folder(&share)?;
    cat.add_folder(&other)?;
    let roots = cat.roots()?;
    // Both roots are now at the same place on this computer.
    cat.relink_root(roots[1].0, &share)?;
    photo(&share, "c.NEF")?;
    let asked = cat.import_folder(&share, &Default::default(), &[])?;
    assert_eq!(asked.added, 0);
    assert_eq!(asked.ambiguous.len(), 1);
    let options = &asked.ambiguous[0].options;
    let mut asked_roots: Vec<_> = options.iter().map(|o| o.root).collect();
    asked_roots.sort();
    assert_eq!(asked_roots, [roots[0].0, roots[1].0]);
    assert!(cat.add_folder(&share).is_err());
    assert_eq!(cat.photos()?.len(), 2);

    let chosen = [Choice {
        options: options.clone(),
        chosen: options
            .iter()
            .find(|o| o.root == roots[1].0)
            .unwrap()
            .clone(),
    }];
    // The folder is now the chosen root's: a.NEF is new to it too.
    let added = cat.import_folder(&share, &Default::default(), &chosen)?;
    assert_eq!(added.added, 2);
    let photos = cat.photos()?;
    let c = photos.iter().find(|p| p.filename == "c.NEF").unwrap();
    let folder = cat
        .folders()?
        .into_iter()
        .find(|f| f.id == c.folder)
        .unwrap();
    assert_eq!(folder.root, roots[1].0);
    Ok(())
}

#[test]
fn a_windows_folder_and_the_same_folder_added_here_are_one_folder() -> Result<()> {
    let d = tempfile::tempdir()?;
    let here = d.path().join("nas");
    photo(&here.join("2024/Trip"), "a.NEF")?;
    let catalog = new_catalog(d.path())?;
    // Added on Windows by an older release.
    legacy(&catalog)?.execute_batch(
        r"INSERT INTO roots(id,original_path) VALUES(1,'D:\Photos');
          INSERT INTO folders(id,root,relative_path) VALUES(2,1,'2024\Trip');
          INSERT INTO photos(folder,filename,original_path) VALUES(2,'a.NEF','D:\Photos\2024\Trip\a.NEF');",
    )?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.relink_root(RootId(1), &here)?;
    assert_eq!(
        path_of(&catalog, &linux(), "a.NEF")?,
        here.join("2024/Trip/a.NEF")
    );
    photo(&here.join("2024/Trip"), "b.NEF")?;
    assert_eq!(cat.add_folder(&here)?, 1);
    assert_eq!(count(&catalog, "folders")?, 1);
    assert_eq!(cat.folders()?[0].relative, "2024/Trip");
    // A Unix name with a backslash stays one folder, even in a Windows root.
    if cfg!(unix) {
        photo(&here.join(r"odd\name"), "c.NEF")?;
        assert_eq!(cat.add_folder(&here)?, 1);
        assert_eq!(
            path_of(&catalog, &linux(), "c.NEF")?,
            here.join(r"odd\name").join("c.NEF")
        );
        let logical: String = legacy(&catalog)?.query_row(
            "SELECT p.path FROM folder_paths p JOIN folders f ON f.id=p.folder
             WHERE f.relative_path LIKE 'odd%'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(logical, r"odd\name");
    }
    Ok(())
}

#[test]
fn a_backslash_name_is_unavailable_on_windows_not_two_folders() {
    let base = Path::new("/mnt/nas");
    let rows = [(String::new(), base.to_path_buf())];
    assert_eq!(
        resolve_in("/added", &rows, r"odd\name/Day2", true),
        None,
        "a name Windows can't hold is unavailable there"
    );
    assert_eq!(
        resolve_in("/added", &rows, r"odd\name/Day2", false),
        Some(base.join(r"odd\name").join("Day2"))
    );
    assert_eq!(
        resolve_in("/added", &[], "Trip", false),
        Some(PathBuf::from("/added/Trip"))
    );
}

#[test]
fn a_root_an_older_release_adds_after_adoption_uses_where_it_was_added() -> Result<()> {
    let d = tempfile::tempdir()?;
    let catalog = new_catalog(d.path())?;
    Catalog::open_as(&catalog, &linux())?;
    legacy(&catalog)?.execute_batch(
        "INSERT INTO roots(id,original_path,mapped_path) VALUES(7,'/Volumes/New','/mnt/new');
         INSERT INTO folders(id,root,relative_path) VALUES(8,7,'Trip');
         INSERT INTO photos(folder,filename,original_path) VALUES(8,'a.NEF','/Volumes/New/Trip/a.NEF');",
    )?;
    assert_eq!(
        path_of(&catalog, &linux(), "a.NEF")?,
        PathBuf::from("/Volumes/New/Trip/a.NEF")
    );
    // Its folders get their logical path on the next open.
    assert_eq!(count(&catalog, "folder_paths")?, 1);
    Ok(())
}

#[test]
fn a_catalog_from_an_older_release_resolves_as_before() -> Result<()> {
    let d = tempfile::tempdir()?;
    let catalog = new_catalog(d.path())?;
    // Unix paths as Lightroom import writes them, with trailing separators.
    legacy(&catalog)?.execute_batch(
        "INSERT INTO roots(id,original_path,mapped_path) VALUES
            (1,'/Volumes/Photos/','/mnt/photos'),(2,'/Volumes/Other/',NULL);
         INSERT INTO folders(id,root,relative_path) VALUES
            (10,1,''),(11,1,'Trip/'),(12,1,'Trip/Day2/'),(13,2,'Home/'),(14,1,'Home/');
         INSERT INTO folder_mappings(folder,path) VALUES(11,'/mnt/archive/Trip');
         INSERT INTO photos(folder,filename,original_path) VALUES
            (10,'root.NEF',''),(11,'trip.NEF',''),(12,'day2.NEF',''),
            (13,'other.NEF',''),(14,'home.NEF','');",
    )?;
    let expected = [
        ("day2.NEF", "/mnt/archive/Trip/Day2/day2.NEF"),
        ("home.NEF", "/mnt/photos/Home/home.NEF"),
        ("other.NEF", "/Volumes/Other/Home/other.NEF"),
        ("root.NEF", "/mnt/photos/root.NEF"),
        ("trip.NEF", "/mnt/archive/Trip/trip.NEF"),
    ];
    for who in [mac(), linux()] {
        let found = paths(&catalog, &who)?;
        let expected: Vec<_> = expected
            .iter()
            .map(|(n, p)| (n.to_string(), PathBuf::from(p)))
            .collect();
        assert_eq!(found, expected);
    }
    let cat = Catalog::open_as(&catalog, &linux())?;
    let relative: Vec<_> = cat.folders()?.into_iter().map(|f| f.relative).collect();
    assert!(relative.contains(&"Trip/Day2".to_string()));
    Ok(())
}

#[test]
fn folder_locations_list_every_root_and_other_computers() -> Result<()> {
    let d = tempfile::tempdir()?;
    let a = d.path().join("a");
    let b = d.path().join("b");
    let mac_b = d.path().join("mac-b");
    photo(&a, "a.NEF")?;
    photo(&b.join("Trip"), "b.NEF")?;
    std::fs::create_dir_all(&mac_b)?;
    let catalog = new_catalog(d.path())?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.add_folder(&a)?;
    cat.add_folder(&b)?;
    let roots = cat.roots()?;
    drop(cat);
    Catalog::open_as(&catalog, &mac())?.relink_root(roots[1].0, &mac_b)?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    let trip = folder_id(&catalog, "Trip")?;
    cat.relink_folder(trip, &b.join("Trip"))?;
    let listed = cat.folder_locations()?;
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].location, None);
    assert_eq!(listed[0].path, a.canonicalize()?);
    assert_eq!(listed[1].overrides.len(), 1);
    assert_eq!(listed[1].overrides[0].relative, "Trip");
    assert_eq!(
        listed[1].elsewhere,
        [("mac".to_string(), String::new(), mac_b)]
    );
    Ok(())
}

#[test]
fn the_computer_id_is_made_once_and_kept() -> Result<()> {
    let d = tempfile::tempdir()?;
    let first = Computer::load_from(d.path())?;
    let again = Computer::load_from(d.path())?;
    assert_eq!(first.id, again.id);
    assert!(first.id.len() >= 32);
    assert!(!first.name.is_empty());
    Ok(())
}

#[test]
fn adding_where_a_relocated_root_used_to_be_is_a_stale_copy() -> Result<()> {
    let d = tempfile::tempdir()?;
    let old = d.path().join("photos");
    let new = d.path().join("archive/photos");
    photo(&old.join("Trip"), "a.NEF")?;
    photo(&new.join("Trip"), "a.NEF")?;
    let catalog = new_catalog(d.path())?;
    let mut cat = Catalog::open_as(&catalog, &linux())?;
    cat.add_folder(&old)?;
    let root = cat.roots()?[0].0;
    cat.relink_root(root, &new)?;
    // The old copy is still there; adding it again adds nothing.
    let added = cat.import_folder(&old, &Default::default(), &[])?;
    assert_eq!(added.added, 0);
    assert_eq!(added.conflicts.len(), 1);
    assert_eq!(added.conflicts[0].located, new.join("Trip"));
    assert_eq!(cat.roots()?.len(), 1);
    Ok(())
}
