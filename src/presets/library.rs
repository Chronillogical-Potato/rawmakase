use crate::xmp::{Preset, parse};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
#[derive(Default, Clone)]
pub struct Library {
    pub presets: Vec<Preset>,
    pub errors: Vec<String>,
}
pub fn library_dirs() -> Vec<PathBuf> {
    crate::storage::asset_dirs()
        .into_iter()
        .map(|p| p.join("xmp-presets"))
        .collect()
}
pub fn load_library() -> Library {
    let mut library = Library::default();
    let mut paths = Vec::new();
    fn scan(dir: &Path, paths: &mut Vec<PathBuf>, depth: usize) {
        if depth > 12 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(kind) = e.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !["Defaults", "GPU", "__MACOSX"]
                    .iter()
                    .any(|name| p.file_name().is_some_and(|n| n == *name))
                {
                    scan(&p, paths, depth + 1);
                }
            } else if kind.is_file() && p.extension().is_some_and(|s| s.eq_ignore_ascii_case("xmp"))
            {
                paths.push(p);
            }
        }
    }
    for dir in library_dirs() {
        scan(&dir, &mut paths, 0);
    }
    paths.sort();
    paths.dedup();
    for path in paths {
        let result = (|| -> Result<Preset> {
            ensure!(std::fs::metadata(&path)?.len() < 8_000_000, "XMP too large");
            parse(&path, &std::fs::read_to_string(&path)?)
        })();
        match result {
            Ok(p) => {
                if p.settings.get("ShowInPresets").is_none_or(|v| v != "False") {
                    library.presets.push(p);
                }
            }
            Err(e) => library.errors.push(format!("{}: {e:#}", path.display())),
        }
    }
    library
        .presets
        .sort_by(|a, b| a.group.cmp(&b.group).then(a.name.cmp(&b.name)));
    // Built-in presets come first, as in Lightroom.
    let (builtin, errors) = super::builtin::presets();
    library.presets.splice(0..0, builtin);
    library.errors.extend(errors);
    library
}
pub fn favorite_path() -> PathBuf {
    crate::storage::data_dir().join("preset-favorites.json")
}
pub fn load_favorites() -> BTreeSet<String> {
    crate::storage::read_json_or_default(&favorite_path())
}
pub fn save_favorites(favorites: &BTreeSet<String>) -> Result<()> {
    crate::storage::atomic_json(&favorite_path(), favorites)
}
pub fn import_file(path: &Path) -> Result<PathBuf> {
    let text = std::fs::read_to_string(path)?;
    parse(path, &text)?;
    let dir = crate::storage::data_dir().join("xmp-presets/Imported");
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(path.file_name().context("Missing filename")?);
    if target.exists() {
        ensure!(
            std::fs::read(&target)? == text.as_bytes(),
            "A different preset with this filename is already installed"
        );
    } else {
        use std::io::Write;
        crate::storage::write_atomic(&target, crate::storage::Replace::NoClobber, |f| {
            Ok(f.write_all(text.as_bytes())?)
        })?;
    }
    Ok(target)
}
pub fn display_name(name: &str) -> String {
    name.replace('⁺', "+")
        .replace('⁻', "-")
        .replace('¹', "1")
        .replace('²', "2")
        .replace('³', "3")
}
