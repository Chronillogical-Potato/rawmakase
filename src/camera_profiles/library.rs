use super::{CameraProfile, from_bytes};
use crate::raw::Metadata;
use anyhow::{Result, ensure};
use std::{path::Path, sync::Arc};
pub fn load(path: &Path, m: &Metadata) -> Result<Arc<CameraProfile>> {
    ensure!(
        std::fs::metadata(path)?.len() <= 16_000_000,
        "Profile too large"
    );
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
    {
        let (profiles, _) = installed(m);
        for base in profiles.iter().filter(|p| p.enhanced.is_none()) {
            if let Ok(profile) = super::enhanced::compose(path, base) {
                return Ok(Arc::new(profile));
            }
        }
        anyhow::bail!("Unsupported XMP look or missing matching base camera profile");
    }
    let p = from_bytes(&std::fs::read(path)?)?;
    p.ensure_camera(m)?;
    Ok(Arc::new(p))
}
/// No camera profiles are bundled. Without an imported profile, rendering uses the
/// camera matrix default (see `CameraProfile::camera_matrix_default`).
/// The profile a DNG embeds for its camera, which Lightroom lists as the file's own.
pub fn builtin(m: &Metadata) -> Option<Arc<CameraProfile>> {
    m.embedded_profile.clone()
}
/// User-installed profiles stay outside the source tree and are filtered by camera.
pub fn library_dirs() -> Vec<std::path::PathBuf> {
    crate::storage::asset_dirs()
        .into_iter()
        .map(|p| p.join("camera-profiles"))
        .collect()
}

pub fn installed(m: &Metadata) -> (Vec<Arc<CameraProfile>>, Vec<String>) {
    let mut profiles = Vec::new();
    let mut errors = Vec::new();
    if let Some(p) = builtin(m) {
        profiles.push(p);
    }
    let mut files = Vec::new();
    for dir in library_dirs() {
        collect(&dir, 0, &mut files);
    }
    let mut looks = Vec::new();
    for path in files {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
        {
            looks.push(path);
            continue;
        }
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
        {
            continue;
        }
        match load(&path, m) {
            Ok(p) => {
                if !profiles
                    .iter()
                    .any(|old| old.name == p.name && old.camera == p.camera)
                {
                    profiles.push(p);
                }
            }
            Err(e) => {
                // Other camera models are expected in a shared user library.
                if !e.to_string().starts_with("Profile belongs to") {
                    errors.push(format!("{}: {e:#}", path.display()));
                }
            }
        }
    }
    let bases = profiles.clone();
    for path in looks {
        let mut last_error = None;
        let mut loaded = false;
        for base in &bases {
            match super::enhanced::compose(&path, base) {
                Ok(p) => {
                    if !profiles
                        .iter()
                        .any(|old| old.name == p.name && old.camera == p.camera)
                    {
                        profiles.push(Arc::new(p));
                    }
                    loaded = true;
                    break;
                }
                Err(e) => {
                    if !e.to_string().starts_with("Missing base camera profile") {
                        last_error = Some(e);
                    }
                }
            }
        }
        if !loaded {
            errors.push(format!(
                "{}: {}",
                path.display(),
                last_error.map_or_else(
                    || "Missing matching base camera profile".into(),
                    |e| e.to_string()
                )
            ));
        }
    }
    errors.sort();
    errors.dedup();
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    (profiles, errors)
}
fn collect(dir: &Path, depth: usize, files: &mut Vec<std::path::PathBuf>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect(&entry.path(), depth + 1, files);
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
}

/// Explicitly import files into RAWmakase's own library. Validate the complete batch
/// before writing; never follow or discover Adobe application directories.
pub fn import_files(paths: &[std::path::PathBuf]) -> Result<Vec<std::path::PathBuf>> {
    import_into(paths, &crate::storage::data_dir().join("camera-profiles"))
}
fn import_into(
    paths: &[std::path::PathBuf],
    destination: &Path,
) -> Result<Vec<std::path::PathBuf>> {
    use anyhow::Context;
    use std::io::Write;
    ensure!(
        !paths.is_empty() && paths.len() <= 1024,
        "Choose 1–1024 profile files"
    );
    let mut files = Vec::new();
    let mut bases = Vec::new();
    let mut existing = Vec::new();
    collect(destination, 0, &mut existing);
    for p in existing {
        if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
            && let Ok(bytes) = std::fs::read(&p)
            && let Ok(profile) = from_bytes(&bytes)
        {
            bases.push(profile);
        }
    }
    for path in paths {
        ensure!(
            std::fs::metadata(path)?.len() <= 16_000_000,
            "Profile too large: {}",
            path.display()
        );
        let ext = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        ensure!(
            matches!(ext.as_str(), "dcp" | "xmp"),
            "Choose DCP or XMP profiles"
        );
        let bytes = std::fs::read(path)?;
        if ext == "dcp" {
            bases.push(from_bytes(&bytes).with_context(|| format!("{}", path.display()))?);
        }
        let target = destination.join(path.file_name().context("Missing profile filename")?);
        if target.exists() {
            ensure!(
                std::fs::read(&target)? == bytes,
                "A different profile named {} is already imported",
                target.file_name().unwrap().to_string_lossy()
            );
        }
        if let Some((_, previous, _)) = files.iter().find(|(p, _, _)| *p == target) {
            ensure!(
                *previous == bytes,
                "Conflicting profile filenames in import"
            );
        } else {
            files.push((target, bytes, ext));
        }
    }
    for (path, bytes, ext) in &files {
        if ext == "xmp" {
            let text = std::str::from_utf8(bytes)?;
            let mut error = None;
            let supported =
                bases
                    .iter()
                    .any(|base| match super::enhanced::compose_text(text, base) {
                        Ok(_) => true,
                        Err(e) => {
                            if !e.to_string().starts_with("Missing base camera profile") {
                                error = Some(e);
                            }
                            false
                        }
                    });
            ensure!(
                supported,
                "{}: {}. Import its matching base DCP together with the XMP profile",
                path.file_name().unwrap().to_string_lossy(),
                error.map_or_else(|| "Missing base camera profile".into(), |e| e.to_string())
            );
        }
    }
    std::fs::create_dir_all(destination)?;
    let mut imported = Vec::new();
    for (target, bytes, _) in files {
        if !target.exists() {
            let mut staged = tempfile::NamedTempFile::new_in(destination)?;
            staged.write_all(&bytes)?;
            staged.as_file().sync_all()?;
            staged.persist_noclobber(&target).map_err(|e| e.error)?;
        }
        imported.push(target);
    }
    Ok(imported)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Needs a published DCP; set RAWMAKASE_TEST_DCP"]
    fn imports_are_explicit_persistent_and_do_not_overwrite_different_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("selected.dcp");
        let bytes = &std::fs::read(std::env::var_os("RAWMAKASE_TEST_DCP").unwrap())?[..];
        std::fs::write(&source, bytes)?;
        let dest = temp.path().join("rawmakase-library");
        let selected = vec![source.clone()];
        let result = import_into(&selected, &dest)?;
        assert_eq!(std::fs::read(&result[0])?, bytes);
        assert_eq!(std::fs::read(&source)?, bytes);
        assert_eq!(import_into(&selected, &dest)?, result);
        std::fs::write(&result[0], b"existing user data")?;
        assert!(import_into(&selected, &dest).is_err());
        assert_eq!(std::fs::read(&result[0])?, b"existing user data");
        let unsupported = temp.path().join("preset.xmp");
        std::fs::write(&unsupported, "<preset/>")?;
        let new_dest = temp.path().join("fresh-library");
        assert!(import_into(&[source, unsupported], &new_dest).is_err());
        assert!(!new_dest.exists());
        Ok(())
    }
}
