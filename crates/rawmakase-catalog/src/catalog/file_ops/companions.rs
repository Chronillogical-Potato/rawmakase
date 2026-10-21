//! Which files go with a photo: its XMP sidecars, by the rule lookup reads
//! them with (`sidecar::sidecar_candidates`), and the edit files releases
//! before the catalog saved beside it. A companion goes with its photo only
//! while that photo is the one file in its folder that reads it, and only
//! into a folder where nothing else would read it, nor the photo read
//! anything already there.
//!
//! Two comparisons, for two questions. Whether another file reads a
//! companion, or the photo would read one already there, is asked loosely
//! (`sidecar::caseless`), so the answer errs towards leaving a companion in
//! place or refusing. Which files are the photo's companions at all is
//! asked strictly: a name lookup would find (one of its candidate names,
//! in another case or normalisation form), or one the volume itself takes
//! for a candidate (the same file object), never a name only alike in a
//! looser sense.
use super::super::folder_sync::folded;
use super::super::legacy_sidecar::companion_paths;
use super::super::sidecar::{caseless, readers_of, sidecar_names, sidecar_names_caseless};
use std::ffi::{OsStr, OsString};
use std::path::Path;

/// Every name a companion of the file named `photo` could have, by `sidecar::caseless`.
fn names_of(photo: &OsStr) -> Vec<String> {
    let mut names = sidecar_names_caseless(Path::new(photo));
    names.extend(
        companion_paths(Path::new(photo))
            .iter()
            .map(|p| caseless(p.as_os_str())),
    );
    names
}

/// Whether `photo` would read the file named `name` as a companion.
fn reads(photo: &OsStr, name: &OsStr) -> bool {
    photo != name && name.to_str().is_some() && names_of(photo).contains(&caseless(name))
}

/// The names in `names` that would read the file named `companion` as
/// theirs: its sidecar readers, and the file a legacy edit file is named
/// after.
pub(super) fn readers<'a>(companion: &OsStr, names: &'a [OsString]) -> Vec<&'a OsString> {
    let mut found = readers_of(companion, names);
    for name in names {
        let legacy = companion_paths(Path::new(name))
            .iter()
            .any(|p| caseless(p.as_os_str()) == caseless(companion));
        if legacy && companion.to_str().is_some() && !found.contains(&name) {
            found.push(name);
        }
    }
    found
}

/// Whether the file named `name` is an edit file an earlier release saved
/// for the file named `photo`, which names that file's `Identity` inside.
pub(super) fn is_legacy_edit(photo: &OsStr, name: &OsStr) -> bool {
    name.to_str().is_some()
        && companion_paths(Path::new(photo))
            .iter()
            .any(|p| caseless(p.as_os_str()) == caseless(name))
}

/// A photo's companions in its folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Companions {
    /// Read by the photo alone: they go with it.
    pub carried: Vec<OsString>,
    /// Read by another file there too: they stay.
    pub shared: Vec<OsString>,
}

/// Whether the file named `name` is a companion of the file named `photo`:
/// one of the names its companions have, in another case or normalisation
/// form at most, or a name `same_file` says the volume takes for one of
/// them.
fn is_companion(photo: &OsStr, name: &OsStr, same_file: &dyn Fn(&OsStr, &OsStr) -> bool) -> bool {
    if photo == name || name.to_str().is_none() {
        return false;
    }
    let mut candidates = sidecar_names(Path::new(photo));
    candidates.extend(
        companion_paths(Path::new(photo))
            .into_iter()
            .map(|p| p.into_os_string()),
    );
    candidates
        .iter()
        .any(|c| folded(name) == folded(c) || (caseless(name) == caseless(c) && same_file(name, c)))
}

/// The companions of the file named `photo` in a folder whose listing is
/// `names`; `same_file` says whether two names there are one file object.
pub(super) fn of(
    photo: &OsStr,
    names: &[OsString],
    same_file: &dyn Fn(&OsStr, &OsStr) -> bool,
) -> Companions {
    let mut found = Companions::default();
    for name in names.iter().filter(|n| is_companion(photo, n, same_file)) {
        if readers(name, names).iter().all(|r| r.as_os_str() == photo) {
            found.carried.push(name.clone());
        } else {
            found.shared.push(name.clone());
        }
    }
    found.carried.sort();
    found.shared.sort();
    found
}

/// The name companion `companion` of the file named `photo` takes when the
/// photo is renamed `renamed`, so the same rule finds it: the photo's new
/// name (or, for a RAW's stem sidecar, its new stem) and the companion's
/// own ending, in its own case (`.xmp`, `.XMP`). `None` when `companion`
/// isn't one of `photo`'s names.
pub(super) fn renamed(photo: &OsStr, renamed: &OsStr, companion: &OsStr) -> Option<OsString> {
    let (photo, renamed, companion) = (photo.to_str()?, renamed.to_str()?, companion.to_str()?);
    const ENDINGS: [&str; 3] = [".xmp", ".rawmakase.json", ".rawmakase-local.json"];
    let mut bases = vec![(photo.to_string(), renamed.to_string(), &ENDINGS[..])];
    if crate::storage::is_raw(Path::new(photo)) {
        let stem = |name: &str| {
            Path::new(name)
                .with_extension("")
                .to_str()
                .map(String::from)
        };
        bases.push((stem(photo)?, stem(renamed)?, &ENDINGS[..1]));
    }
    for (old, new, endings) in bases {
        for ending in endings {
            let Some(at) = companion.len().checked_sub(ending.len()) else {
                continue;
            };
            let (Some(head), Some(tail)) = (companion.get(..at), companion.get(at..)) else {
                continue;
            };
            if tail.eq_ignore_ascii_case(ending)
                && caseless(OsStr::new(head)) == caseless(OsStr::new(&old))
            {
                return Some(format!("{new}{tail}").into());
            }
        }
    }
    None
}

/// Why the file named `photo` and its `carried` companions can't arrive in a
/// folder whose names, once everything arriving there is in, will be
/// `projected`: the name of a file there the photo would read as its
/// companion, or of one that would read a companion arriving with it.
pub(super) fn arrival_conflict(
    photo: &OsStr,
    carried: &[OsString],
    projected: &[OsString],
) -> Option<OsString> {
    // Something already there, or arriving with another photo, the photo
    // would take for its own.
    let theirs = projected
        .iter()
        .filter(|n| !carried.contains(n) && n.as_os_str() != photo);
    for name in theirs {
        if reads(photo, name) {
            return Some(name.clone());
        }
    }
    // Its own companions, which another file there would read too.
    for companion in carried {
        let others = readers(companion, projected)
            .into_iter()
            .find(|r| r.as_os_str() != photo);
        if let Some(other) = others {
            return Some(other.clone());
        }
    }
    None
}
