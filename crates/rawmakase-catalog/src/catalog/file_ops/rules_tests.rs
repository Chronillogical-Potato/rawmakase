//! The companion and name rules on their own.
use super::companions::{self, Companions, readers};
use super::conflicts::{Destination, Names, Taken};
use super::disk::Real;
use crate::storage::fsops::{Folding, NameRules};
use std::collections::HashSet;
use std::ffi::OsString;

fn names(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

fn carried(photo: &str, listing: &[&str]) -> Companions {
    // A volume that tells every two names apart.
    companions::of(photo.as_ref(), &names(listing), &|_, _| false)
}

#[test]
fn a_raws_sidecars_and_legacy_edits_go_with_it() {
    let found = carried(
        "A.NEF",
        &[
            "A.NEF",
            "A.NEF.xmp",
            "A.xmp",
            "A.NEF.rawmakase.json",
            "A.NEF.rawmakase-local.json",
            "B.NEF",
            "B.xmp",
        ],
    );
    assert_eq!(
        found.carried,
        names(&[
            "A.NEF.rawmakase-local.json",
            "A.NEF.rawmakase.json",
            "A.NEF.xmp",
            "A.xmp"
        ])
    );
    assert!(found.shared.is_empty());
}

#[test]
fn a_sidecar_is_matched_whatever_its_case() {
    // Lookup tries `.xmp` and `.XMP`; a folder may hold `.Xmp`, which is
    // still that photo's on a volume that ignores case.
    let found = carried("A.NEF", &["A.NEF", "a.nef.Xmp", "A.XMP"]);
    assert_eq!(found.carried, names(&["A.XMP", "a.nef.Xmp"]));
}

#[test]
fn a_sidecar_another_file_reads_too_stays() {
    // A.NEF.xmp is A.NEF's and A.NEF.DNG's (its stem sidecar).
    let found = carried("A.NEF", &["A.NEF", "A.NEF.DNG", "A.NEF.xmp"]);
    assert!(found.carried.is_empty());
    assert_eq!(found.shared, names(&["A.NEF.xmp"]));
    // A RAW's stem sidecar is not a JPEG's of the same name: the RAW's.
    let found = carried("A.NEF", &["A.NEF", "A.JPG", "A.xmp"]);
    assert_eq!(found.carried, names(&["A.xmp"]));
    let found = carried("A.JPG", &["A.NEF", "A.JPG", "A.xmp", "A.JPG.xmp"]);
    assert_eq!(found.carried, names(&["A.JPG.xmp"]));
    // Two RAWs of one stem share it.
    let found = carried("A.NEF", &["A.NEF", "A.DNG", "A.xmp"]);
    assert_eq!(found.shared, names(&["A.xmp"]));
}

#[test]
fn ownership_follows_the_rule_lookup_reads_sidecars_by() {
    // For every file of a folder, each sidecar lookup finds has it among
    // the readers ownership counts.
    let dir = tempfile::tempdir().unwrap();
    let list = [
        "A.NEF",
        "A.NEF.DNG",
        "A.NEF.xmp",
        "A.xmp",
        "B.JPG",
        "B.xmp",
        "B.JPG.XMP",
        "C.CR3",
        "C.XMP",
    ];
    for name in list {
        std::fs::write(dir.path().join(name), b"").unwrap();
    }
    let listing = names(&list);
    // A case-insensitive volume finds a name in either case as one file;
    // lookup asks for both cases, so dedupe what it found by name.
    for file in &listing {
        let found = crate::catalog::sidecar::sidecars(&dir.path().join(file));
        for sidecar in found {
            let name = sidecar.file_name().unwrap().to_os_string();
            let name = listing
                .iter()
                .find(|n| {
                    n.to_string_lossy()
                        .eq_ignore_ascii_case(&name.to_string_lossy())
                })
                .unwrap();
            assert!(
                readers(name, &listing).contains(&file),
                "{file:?} reads {name:?}"
            );
        }
    }
}

#[test]
fn arriving_companions_and_photos_must_not_read_anything_else() {
    let projected = names(&["A.DNG", "A.NEF", "A.xmp"]);
    assert_eq!(
        companions::arrival_conflict("A.NEF".as_ref(), &names(&["A.xmp"]), &projected),
        Some("A.DNG".into())
    );
    let projected = names(&["B.NEF.xmp", "B.NEF"]);
    assert_eq!(
        companions::arrival_conflict("B.NEF".as_ref(), &[], &projected),
        Some("B.NEF.xmp".into())
    );
    let projected = names(&["C.NEF", "C.NEF.xmp", "D.NEF"]);
    assert_eq!(
        companions::arrival_conflict("C.NEF".as_ref(), &names(&["C.NEF.xmp"]), &projected),
        None
    );
}

#[test]
fn names_are_compared_as_the_folder_compares_them() {
    let exact = Names(NameRules {
        case: Folding::Exact,
        normalization: Folding::Exact,
    });
    let loose = Names(NameRules {
        case: Folding::Unknown,
        normalization: Folding::Insensitive,
    });
    let (nfc, nfd) = ("caf\u{e9}.NEF", "cafe\u{301}.NEF");
    assert_ne!(exact.key("A.NEF".as_ref()), exact.key("a.nef".as_ref()));
    assert_ne!(exact.key(nfc.as_ref()), exact.key(nfd.as_ref()));
    assert_eq!(loose.key("A.NEF".as_ref()), loose.key("a.nef".as_ref()));
    assert_eq!(loose.key(nfc.as_ref()), loose.key(nfd.as_ref()));
}

#[test]
fn a_name_is_taken_on_disk_or_in_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("A.NEF"), b"").unwrap();
    let listing = names(&["A.NEF"]);
    let catalog: HashSet<String> = ["b.nef".to_string()].into();
    let exact = Names(NameRules {
        case: Folding::Exact,
        normalization: Folding::Exact,
    });
    let d = Destination {
        dir: dir.path(),
        names: &listing,
        rules: exact,
        catalog: &catalog,
    };
    assert_eq!(d.taken(&Real, "A.NEF".as_ref()), Some(Taken::OnDisk));
    assert_eq!(d.taken(&Real, "B.NEF".as_ref()), Some(Taken::InCatalog));
    assert_eq!(d.taken(&Real, "C.NEF".as_ref()), None);
    // Where the volume ignores case, looking the name up finds it even
    // when the rules said exact.
    let lower = d.taken(&Real, "a.NEF".as_ref());
    if dir.path().join("a.NEF").exists() {
        assert_eq!(lower, Some(Taken::OnDisk));
    } else {
        assert_eq!(lower, None);
    }
}
