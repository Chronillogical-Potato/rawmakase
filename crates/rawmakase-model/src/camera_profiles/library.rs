use super::{
    CameraProfile,
    cube_look::{self, Fidelity, Names, Placement},
    enhanced::{LookBase, LookFile},
    from_bytes,
};
use crate::camera_data::Metadata;
use anyhow::{Context, Result, ensure};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
pub fn load(path: &Path, m: &Metadata) -> Result<Arc<CameraProfile>> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
    {
        ensure!(
            std::fs::metadata(path)?.len() <= 16_000_000,
            "Profile too large"
        );
        let look = LookFile::read(path)?;
        let (profiles, _) = installed(m);
        let base = look_base(&look, &profiles, builtin(m).as_deref())
            .context("Unsupported XMP look or missing matching base camera profile")?;
        return Ok(Arc::new(look.compose(base)?));
    }
    let p = read_dcp(path)?;
    p.ensure_camera(m)?;
    Ok(Arc::new(p))
}
fn read_dcp(path: &Path) -> Result<CameraProfile> {
    ensure!(
        std::fs::metadata(path)?.len() <= 16_000_000,
        "Profile too large"
    );
    from_bytes(&std::fs::read(path)?)
}
/// What tells that a profile file may have changed: its size, its modification time
/// and, on Unix, its status change time, which any write or permission change moves
/// even when the modification time is put back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
}
impl Stamp {
    fn read(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        Some(Self {
            size: m.len(),
            modified: m.modified().ok(),
            #[cfg(unix)]
            changed: {
                use std::os::unix::fs::MetadataExt;
                (m.ctime(), m.ctime_nsec())
            },
        })
    }
}
/// The camera each DCP in the library was made for, as last read, so a scan parses
/// only the files of the photo's camera: a library holding Adobe's profiles for
/// every camera has thousands. A profile's camera comes from its bytes alone; a
/// file whose [`Stamp`] changed is read again, and one that failed to read is not
/// remembered, so its error is reported every time.
#[derive(Default)]
struct Cameras {
    known: Mutex<HashMap<PathBuf, (Stamp, String)>>,
    /// Files read, for the tests.
    #[cfg(test)]
    reads: std::sync::atomic::AtomicUsize,
}
impl Cameras {
    fn get(&self, path: &Path, stamp: Stamp) -> Option<String> {
        let map = self.known.lock().unwrap_or_else(|e| e.into_inner());
        map.get(path)
            .filter(|(s, _)| *s == stamp)
            .map(|(_, camera)| camera.clone())
    }
    fn set(&self, path: &Path, stamp: Stamp, camera: &str) {
        #[cfg(test)]
        self.reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut map = self.known.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(path.to_path_buf(), (stamp, camera.to_string()));
    }
    /// Forgets the files a scan no longer found.
    fn keep(&self, found: &HashSet<&Path>) {
        let mut map = self.known.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|path, _| found.contains(path.as_path()));
    }
}
static CAMERAS: std::sync::LazyLock<Cameras> = std::sync::LazyLock::new(Cameras::default);
/// The profile a DNG embeds for its camera, which Lightroom lists as the file's own.
/// No Adobe profiles are bundled; RAWmakase's own profiles are in `open`.
pub fn builtin(m: &Metadata) -> Option<Arc<CameraProfile>> {
    let dcp = m.embedded_dcp.as_deref()?;
    super::from_bytes(dcp).ok().map(Arc::new)
}
/// User-installed profiles stay outside the source tree and are filtered by camera.
pub fn library_dirs() -> Vec<std::path::PathBuf> {
    crate::storage::asset_dirs()
        .into_iter()
        .map(|p| p.join("camera-profiles"))
        .collect()
}

pub fn installed(m: &Metadata) -> (Vec<Arc<CameraProfile>>, Vec<String>) {
    installed_in(&library_dirs(), m, &CAMERAS)
}
fn installed_in(
    dirs: &[PathBuf],
    m: &Metadata,
    cameras: &Cameras,
) -> (Vec<Arc<CameraProfile>>, Vec<String>) {
    if m.is_rendered() {
        return (vec![Arc::new(CameraProfile::embedded(m))], vec![]);
    }
    let mut profiles = Vec::new();
    let mut errors = Vec::new();
    if let Some(p) = builtin(m) {
        profiles.push(p);
    }
    profiles.extend(
        [super::open::standard(m), super::open::color(m)]
            .into_iter()
            .flatten()
            .map(Arc::new),
    );
    let mut files = Vec::new();
    for dir in dirs {
        collect(dir, 0, &mut files);
    }
    let mut looks = Vec::new();
    for path in &files {
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
        // Read before the file, so a change while reading it is read again next time.
        let stamp = Stamp::read(path);
        if let Some(camera) = stamp.and_then(|s| cameras.get(path, s))
            && !super::camera_fits(&camera, m)
        {
            continue;
        }
        let loaded = read_dcp(path).and_then(|p| {
            if let Some(stamp) = stamp {
                cameras.set(path, stamp, &p.camera);
            }
            p.ensure_camera(m)?;
            Ok(Arc::new(p))
        });
        match loaded {
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
                if e.downcast_ref::<super::OtherCamera>().is_none() {
                    errors.push(format!("{}: {e:#}", path.display()));
                }
            }
        }
    }
    cameras.keep(&files.iter().map(PathBuf::as_path).collect());
    let bases = profiles.clone();
    for path in looks {
        let composed = LookFile::read(path).and_then(|look| {
            let base = look_base(&look, &bases, builtin(m).as_deref()).with_context(|| {
                let name = match &look.base {
                    LookBase::Named(name) => name.as_str(),
                    LookBase::Any => "",
                };
                format!("Missing matching base camera profile {name}")
            })?;
            look.compose(base)
        });
        match composed {
            Ok(p) => {
                if !profiles
                    .iter()
                    .any(|old| old.name == p.name && old.camera == p.camera)
                {
                    profiles.push(Arc::new(p));
                }
            }
            Err(e) => errors.push(format!("{}: {e:#}", path.display())),
        }
    }
    errors.sort();
    errors.dedup();
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    (profiles, errors)
}
/// A look profile file over the profile it builds on among `bases` (see `look_base`),
/// without the user's library. `own` is the file's embedded profile, if any.
pub fn compose_look(
    path: &Path,
    bases: &[Arc<CameraProfile>],
    own: Option<&CameraProfile>,
) -> Result<CameraProfile> {
    let look = LookFile::read(path)?;
    look.compose(look_base(&look, bases, own).context("Missing matching base camera profile")?)
}
/// The profile a look goes over: the one it names, or for a creative look, which
/// names none, Adobe Standard as Lightroom uses, else the file's own profile (`own`),
/// else RAWmakase Standard, else any that fits. A look restricted to another camera
/// fits none.
pub(super) fn look_base<'a>(
    look: &LookFile,
    bases: &'a [Arc<CameraProfile>],
    own: Option<&CameraProfile>,
) -> Option<&'a CameraProfile> {
    let fitting = || {
        bases
            .iter()
            .filter(|b| look.fits(b) && look.compose(b).is_ok())
    };
    match &look.base {
        LookBase::Named(_) => fitting().next(),
        LookBase::Any => ["Adobe Standard"]
            .into_iter()
            .chain(own.map(|p| p.name.as_str()))
            .chain([super::open::STANDARD])
            .find_map(|name| fitting().find(|b| b.name == name))
            .or_else(|| fitting().next()),
    }
    .map(|b| &**b)
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

/// Explicitly import files into RAWmakase's own library: DCPs and look XMPs are
/// copied, and a `.cube` becomes a look XMP (see `cube_look`). Validate the complete
/// batch before writing; never follow or discover Adobe application directories.
pub fn import_files(paths: &[std::path::PathBuf]) -> Result<Vec<ImportedProfile>> {
    import_into(paths, &crate::storage::data_dir().join("camera-profiles"))
}
/// One file [`import_files`] took into the library.
#[derive(Clone, Debug)]
pub struct ImportedProfile {
    /// The file chosen.
    pub source: PathBuf,
    /// The library's file: a DCP's or look's copy, or the look made from a `.cube`.
    pub path: PathBuf,
    /// For a `.cube`, the look it became.
    pub cube: Option<CubeImport>,
}
/// What became of a `.cube`.
#[derive(Clone, Debug)]
pub struct CubeImport {
    /// The look's name in the profile list.
    pub name: String,
    /// The cube's file stem, which the look is named after unless a different
    /// profile has that name.
    pub stem: String,
    /// In the library already, under `name`: nothing was written.
    pub already: bool,
    /// How faithfully the look holds the cube.
    pub fidelity: Fidelity,
}
impl CubeImport {
    /// Named other than its file, as a different profile has that name.
    pub fn renamed(&self) -> bool {
        self.name != self.stem
    }
}
impl std::fmt::Display for CubeImport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.already {
            return write!(f, "already imported as look \"{}\"", self.name);
        }
        write!(f, "imported as look \"{}\"", self.name)?;
        if self.renamed() {
            write!(f, ", as another profile is named \"{}\"", self.stem)?;
        }
        write!(f, ": {}", self.fidelity)
    }
}
/// Whether a `.cube` can be imported, and how faithfully: what [`import_files`] would
/// make of its bytes.
pub fn read_cube(bytes: &[u8]) -> Result<Fidelity> {
    ensure!(
        bytes.len() as u64 <= cube_look::MAX_BYTES,
        "the cube is too large"
    );
    Ok(cube_look::convert(bytes)?.fidelity)
}
/// What [`import_readable`] did.
#[derive(Debug, Default)]
pub struct Imported {
    /// The library's copies.
    pub imported: Vec<std::path::PathBuf>,
    /// Files left out, with why RAWmakase can't read them.
    pub skipped: Vec<(std::path::PathBuf, String)>,
}
/// [`import_files`] for a set the user didn't pick file by file (a camera's
/// profiles from Camera Raw's folder): DCPs RAWmakase can't read, such as Camera
/// Raw's HDR profiles, are skipped instead of failing the rest.
pub fn import_readable(paths: &[std::path::PathBuf]) -> Result<Imported> {
    import_readable_into(paths, &crate::storage::data_dir().join("camera-profiles"))
}
fn import_readable_into(paths: &[std::path::PathBuf], destination: &Path) -> Result<Imported> {
    let mut readable = Vec::new();
    let mut skipped = Vec::new();
    for path in paths {
        let dcp = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dcp"));
        match std::fs::read(path)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                if dcp {
                    from_bytes(&bytes)?;
                }
                Ok(())
            }) {
            Ok(()) => readable.push(path.clone()),
            Err(e) => skipped.push((path.clone(), format!("{e:#}"))),
        }
    }
    let imported = if readable.is_empty() {
        Vec::new()
    } else {
        import_into(&readable, destination)?
            .into_iter()
            .map(|done| done.path)
            .collect()
    };
    Ok(Imported { imported, skipped })
}
fn import_into(paths: &[std::path::PathBuf], destination: &Path) -> Result<Vec<ImportedProfile>> {
    use anyhow::Context;
    use std::io::Write;
    ensure!(
        !paths.is_empty() && paths.len() <= 1024,
        "Choose 1–1024 profile files"
    );
    let mut files = Vec::new();
    let mut bases = Vec::new();
    // The names a cube's look must not take: the library's profiles and looks, this
    // import's, and RAWmakase's own.
    let mut names = Names::default();
    // Lightroom edits naming Adobe's two fall back to RAWmakase's by name alone.
    for name in [
        super::open::STANDARD,
        super::open::COLOR,
        "Adobe Standard",
        "Adobe Color",
    ] {
        names.profile(name);
    }
    let mut existing = Vec::new();
    collect(destination, 0, &mut existing);
    let mut looks = Vec::new();
    for p in existing {
        if p.parent() == Some(destination)
            && let Some(file) = p.file_name()
        {
            names.file(&file.to_string_lossy());
        }
        let ext = |e: &str| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(e));
        if ext("dcp")
            && let Ok(bytes) = std::fs::read(&p)
            && let Ok(profile) = from_bytes(&bytes)
        {
            names.profile(&profile.name);
            bases.push(profile);
        } else if ext("xmp") {
            looks.push(p);
        }
    }
    let mut cubes = Vec::new();
    for path in paths {
        let ext = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        ensure!(
            matches!(ext.as_str(), "dcp" | "xmp" | "cube"),
            "Choose DCP or XMP profiles or .cube files"
        );
        let limit = if ext == "cube" {
            cube_look::MAX_BYTES
        } else {
            16_000_000
        };
        ensure!(
            std::fs::metadata(path)?.len() <= limit,
            "Profile too large: {}",
            path.display()
        );
        let bytes = std::fs::read(path)?;
        if ext == "cube" {
            cubes.push((path, bytes));
            continue;
        }
        if ext == "dcp" {
            let profile = from_bytes(&bytes).with_context(|| format!("{}", path.display()))?;
            names.profile(&profile.name);
            bases.push(profile);
        }
        let target = destination.join(path.file_name().context("Missing profile filename")?);
        if target.exists() {
            ensure!(
                std::fs::read(&target)? == bytes,
                "A different profile named {} is already imported",
                target.file_name().unwrap().to_string_lossy()
            );
        }
        if let Some((_, _, previous, _, _)) = files.iter().find(|(_, p, _, _, _)| *p == target) {
            ensure!(
                *previous == bytes,
                "Conflicting profile filenames in import"
            );
        } else {
            names.file(&target.file_name().unwrap().to_string_lossy());
            files.push((path.clone(), target, bytes, ext, None));
        }
    }
    for (_, path, bytes, ext, _) in &files {
        if ext == "xmp" {
            let file = path.file_name().unwrap().to_string_lossy();
            let text = std::str::from_utf8(bytes)?;
            let look = LookFile::parse(text).with_context(|| format!("{file}"))?;
            let (name, uuid) = LookFile::identity(text)?;
            names.look(&name, &uuid, path);
            if let LookBase::Named(name) = &look.base {
                // The base must also be for the camera the look is restricted to.
                if !bases
                    .iter()
                    .any(|base| look.fits(base) && look.compose(base).is_ok())
                {
                    return Err(super::MissingBase {
                        file: file.into_owned(),
                        name: name.clone(),
                    }
                    .into());
                }
            }
        }
    }
    // The library's looks, read only when a cube needs their names.
    if !cubes.is_empty() {
        for p in looks {
            if std::fs::metadata(&p).is_ok_and(|m| m.len() <= 16_000_000)
                && let Ok(text) = std::fs::read_to_string(&p)
                && let Ok((name, uuid)) = LookFile::identity(&text)
            {
                names.look(&name, &uuid, &p);
            }
        }
    }
    // Cubes last, so their names avoid every other profile's in this import.
    let mut imported = Vec::new();
    for (path, bytes) in cubes {
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let converted = cube_look::convert(&bytes).with_context(|| format!("{file}"))?;
        let (stem, group) = cube_look::stem_and_group(path)?;
        let report = |name: String, already| CubeImport {
            name,
            stem: stem.clone(),
            already,
            fidelity: converted.fidelity.clone(),
        };
        match names.place(&stem, &group, &converted.uuid)? {
            Placement::Already(name, library) => imported.push(ImportedProfile {
                source: path.clone(),
                path: library,
                cube: Some(report(name, true)),
            }),
            Placement::New(name) => {
                let file = names.file_for(&name);
                let target = destination.join(&file);
                names.file(&file);
                names.look(&name, &converted.uuid, &target);
                let xml = cube_look::look_xmp(&converted, &name, &group);
                files.push((
                    path.clone(),
                    target,
                    xml.into_bytes(),
                    "cube".into(),
                    Some(report(name, false)),
                ));
            }
        }
    }
    std::fs::create_dir_all(destination)?;
    let mut written = Vec::new();
    for (source, target, bytes, _, cube) in files {
        // A copy equal to the library's file is there already; a cube's look is
        // new, and a file that appeared under its name since is an error.
        if cube.is_some() || !target.exists() {
            crate::storage::write_atomic(&target, crate::storage::Replace::NoClobber, |f| {
                Ok(f.write_all(&bytes)?)
            })?;
        }
        written.push(ImportedProfile {
            source,
            path: target,
            cube,
        });
    }
    written.extend(imported);
    // In the order chosen.
    written.sort_by_key(|done| paths.iter().position(|p| *p == done.source));
    Ok(written)
}

/// Whether `name` is one of the photo's cameras followed by any phone module, so
/// the import offers the phone's other modules too.
fn module_of_camera(name: &str, m: &Metadata) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.match_indices(' ').any(|(i, _)| {
        super::PhoneModule::parse(&lower[i + 1..]).is_some() && super::camera_fits(&name[..i], m)
    })
}

/// Adobe's own profiles for this camera from a local Lightroom / Camera Raw
/// installation (Adobe Standard plus Camera Matching), skipping ones already
/// in the library, so the app can offer to import them. Nothing is read from
/// those folders otherwise; rendering only sees imported profiles.
pub fn adobe_installed(m: &Metadata) -> Vec<std::path::PathBuf> {
    let root = Path::new(if cfg!(target_os = "macos") {
        "/Library/Application Support/Adobe/CameraRaw/CameraProfiles"
    } else if cfg!(windows) {
        "C:\\ProgramData\\Adobe\\CameraRaw\\CameraProfiles"
    } else {
        return Vec::new();
    });
    // Every Adobe Standard whose camera fits the photo, under the names Adobe
    // uses for it (see `CameraProfile::ensure_camera`), every module's for a phone
    // so photos from its other cameras fit too, and its Camera Matching folder.
    let fits = |name: &str| super::camera_fits(name, m) || module_of_camera(name, m);
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("Adobe Standard")) {
        paths.extend(entries.flatten().map(|e| e.path()).filter(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| {
                    let lower = s.to_ascii_lowercase();
                    let end = lower
                        .rfind(" adobe standard")
                        .or_else(|| lower.rfind(" adobe_standard"))?;
                    Some(s[..end].to_string())
                })
                .is_some_and(|camera| fits(&camera))
        }));
    }
    if let Ok(entries) = std::fs::read_dir(root.join("Camera")) {
        for dir in entries.flatten().map(|e| e.path()) {
            if dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| super::camera_fits(n, m))
                && let Ok(files) = std::fs::read_dir(&dir)
            {
                paths.extend(files.flatten().map(|e| e.path()));
            }
        }
    }
    let installed = crate::storage::data_dir().join("camera-profiles");
    paths
        .into_iter()
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
                && p.file_name().is_some_and(|n| !installed.join(n).exists())
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    /// A minimal DCP: the camera it is for, its name and an identity forward matrix.
    fn dcp(camera: &str, name: &str) -> Vec<u8> {
        dcp_with(camera, name, [])
    }
    /// [`dcp`] with more tags after its own, in ascending order.
    fn dcp_with<const N: usize>(
        camera: &str,
        name: &str,
        more: [(u16, u16, Vec<u8>); N],
    ) -> Vec<u8> {
        let text = |s: &str| [s.as_bytes(), b"\0"].concat();
        let matrix: Vec<u8> = (0..9)
            .flat_map(|i| [i32::from(i % 4 == 0), 1])
            .flat_map(i32::to_le_bytes)
            .collect();
        let entries: Vec<_> = [
            (50708u16, 2u16, text(camera)),
            (50936, 2, text(name)),
            (50964, 10, matrix),
        ]
        .into_iter()
        .chain(more)
        .collect();
        let mut data_at = 8 + 2 + entries.len() * 12 + 4;
        let mut b = b"IIRC".to_vec();
        b.extend(8u32.to_le_bytes());
        b.extend((entries.len() as u16).to_le_bytes());
        let mut data: Vec<u8> = Vec::new();
        for (tag, kind, bytes) in &entries {
            let count = if *kind == 10 { 9 } else { bytes.len() };
            b.extend(tag.to_le_bytes());
            b.extend(kind.to_le_bytes());
            b.extend((count as u32).to_le_bytes());
            b.extend((data_at as u32).to_le_bytes());
            data_at += bytes.len();
            data.extend(bytes);
        }
        b.extend(0u32.to_le_bytes());
        b.extend(data);
        b
    }
    fn camera(model: &str) -> Metadata {
        Metadata {
            make: "Sony".into(),
            model: model.into(),
            ..Default::default()
        }
    }
    fn names(profiles: &[Arc<CameraProfile>]) -> Vec<&str> {
        profiles
            .iter()
            .filter(|p| p.camera.starts_with("Sony"))
            .map(|p| p.name.as_str())
            .collect()
    }
    #[test]
    fn a_rescan_reads_only_the_photos_cameras_profiles_and_changed_files() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let dirs = [dir.path().to_path_buf()];
        let (mine, other) = (dir.path().join("mine.dcp"), dir.path().join("other.dcp"));
        std::fs::write(&mine, dcp("Sony ILCE-7M2", "Mine"))?;
        std::fs::write(&other, dcp("Sony ILCE-7M4", "Other"))?;
        let cameras = Cameras::default();
        let m = camera("ILCE-7M2");
        let (profiles, errors) = installed_in(&dirs, &m, &cameras);
        assert_eq!((names(&profiles), errors.len()), (vec!["Mine"], 0));
        let reads = || cameras.reads.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(reads(), 2);
        // While unchanged, the other camera's file is not read again; this
        // camera's is, for its profile.
        let (profiles, errors) = installed_in(&dirs, &m, &cameras);
        assert_eq!((names(&profiles), errors.len()), (vec!["Mine"], 0));
        assert_eq!(reads(), 3);
        // Changed, it is read again: now made for this camera.
        std::fs::write(&other, dcp("Sony ILCE-7M2", "Now mine"))?;
        let (profiles, _) = installed_in(&dirs, &m, &cameras);
        assert_eq!(names(&profiles), ["Mine", "Now mine"]);
        // A file that cannot be read reports its error on every scan.
        std::fs::write(&other, b"not a profile")?;
        for _ in 0..2 {
            let (profiles, errors) = installed_in(&dirs, &m, &cameras);
            assert_eq!((names(&profiles), errors.len()), (vec!["Mine"], 1));
        }
        // A removed file is forgotten.
        std::fs::remove_file(&other)?;
        installed_in(&dirs, &m, &cameras);
        assert_eq!(cameras.known.lock().unwrap().len(), 1);
        // Another camera's photo gets its own profiles from the same index.
        std::fs::write(&other, dcp("Sony ILCE-7M4", "Other"))?;
        let (profiles, _) = installed_in(&dirs, &camera("ILCE-7M4"), &cameras);
        assert_eq!(names(&profiles), ["Other"]);
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn a_rescan_notices_changes_that_keep_the_size_and_modification_time() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let dirs = [dir.path().to_path_buf()];
        let other = dir.path().join("other.dcp");
        std::fs::write(&other, dcp("Sony ILCE-7M4", "Other"))?;
        let cameras = Cameras::default();
        let m = camera("ILCE-7M2");
        assert!(names(&installed_in(&dirs, &m, &cameras).0).is_empty());
        // Rewritten for this camera, same size, its modification time put back.
        let modified = std::fs::metadata(&other)?.modified()?;
        std::fs::write(&other, dcp("Sony ILCE-7M2", "Other"))?;
        std::fs::File::options()
            .write(true)
            .open(&other)?
            .set_modified(modified)?;
        assert_eq!(names(&installed_in(&dirs, &m, &cameras).0), ["Other"]);
        // Made unreadable, it is reported as the first scan would, for any camera.
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o000))?;
        let unreadable = std::fs::read(&other).is_err();
        let (_, errors) = installed_in(&dirs, &camera("ILCE-7M4"), &cameras);
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o600))?;
        // Root reads it anyway.
        if unreadable {
            assert_eq!(errors.len(), 1, "{errors:?}");
        }
        Ok(())
    }
    #[test]
    #[ignore = "Needs a published DCP; set RAWMAKASE_TEST_DCP"]
    fn imports_are_explicit_persistent_and_do_not_overwrite_different_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("selected.dcp");
        let bytes = &std::fs::read(std::env::var_os("RAWMAKASE_TEST_DCP").unwrap())?[..];
        std::fs::write(&source, bytes)?;
        let dest = temp.path().join("rawmakase-library");
        let selected = vec![source.clone()];
        let paths = |done: Vec<ImportedProfile>| -> Vec<PathBuf> {
            done.into_iter().map(|d| d.path).collect()
        };
        let result = paths(import_into(&selected, &dest)?);
        assert_eq!(std::fs::read(&result[0])?, bytes);
        assert_eq!(std::fs::read(&source)?, bytes);
        assert_eq!(paths(import_into(&selected, &dest)?), result);
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
    #[test]
    fn importing_a_cameras_adobe_profiles_skips_the_unreadable_ones() -> Result<()> {
        // Camera Raw files HDR Camera Matching profiles with the others for newer
        // bodies (Nikon's Z 8, say); one of them must not stop the rest.
        let temp = tempfile::tempdir()?;
        let standard = temp.path().join("NIKON Z 8 Adobe Standard.dcp");
        std::fs::write(&standard, dcp("NIKON Z 8", "Adobe Standard"))?;
        let hdr = temp.path().join("Nikon Z 8 Camera Flat HDR.dcp");
        let flag = (52551, 7, vec![1, 0, 0, 0, 0, 0, 0, 0]);
        std::fs::write(&hdr, dcp_with("NIKON Z 8", "Camera Flat HDR", [flag]))?;
        let dest = temp.path().join("library");
        let done = import_readable_into(&[standard, hdr.clone()], &dest)?;
        assert_eq!(done.imported, [dest.join("NIKON Z 8 Adobe Standard.dcp")]);
        assert_eq!(done.skipped.len(), 1, "{:?}", done.skipped);
        assert_eq!(done.skipped[0].0, hdr);
        assert!(done.skipped[0].1.contains("HDR"), "{}", done.skipped[0].1);
        assert!(!dest.join("Nikon Z 8 Camera Flat HDR.dcp").exists());
        Ok(())
    }
    #[test]
    fn a_rendered_photo_is_offered_only_the_embedded_profile() {
        let m = Metadata {
            source: crate::camera_data::Source::Rendered,
            make: "Apple".into(),
            model: "iPhone 15".into(),
            ..Default::default()
        };
        let (profiles, errors) = installed_in(&[], &m, &CAMERAS);
        assert_eq!(names_all(&profiles), ["Embedded"]);
        assert!(errors.is_empty());
    }
    fn names_all(profiles: &[Arc<CameraProfile>]) -> Vec<&str> {
        profiles.iter().map(|p| p.name.as_str()).collect()
    }

    /// A camera RAWmakase Standard is made for, so creative looks have a base.
    fn with_matrix() -> Metadata {
        Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        }
    }
    fn write_cube(dir: &Path, name: &str, f: impl Fn([f32; 3]) -> [f32; 3]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, super::super::cube_look::tests::cube_text(9, f)).unwrap();
        path
    }
    fn warm([r, g, b]: [f32; 3]) -> [f32; 3] {
        [0.05 + 0.9 * r, 0.02 + 0.93 * g, 0.85 * b]
    }
    fn cool([r, g, b]: [f32; 3]) -> [f32; 3] {
        [0.85 * r, 0.02 + 0.93 * g, 0.05 + 0.9 * b]
    }
    fn cube_of(done: &ImportedProfile) -> &CubeImport {
        done.cube.as_ref().expect("a cube's report")
    }

    /// A cube becomes a look XMP in the library, named after its file and grouped by
    /// its folder, which the profile list then offers over the photo's base profile.
    #[test]
    fn a_cube_imports_as_a_look_the_library_lists() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cube = write_cube(&temp.path().join("Film"), "Portra.cube", warm);
        let dest = temp.path().join("library");
        let done = import_into(std::slice::from_ref(&cube), &dest)?;
        assert_eq!(done.len(), 1);
        let report = cube_of(&done[0]);
        assert_eq!((report.name.as_str(), report.already), ("Portra", false));
        assert_eq!(report.fidelity, Fidelity::Exact);
        assert_eq!(done[0].path, dest.join("Portra.xmp"));
        let xml = std::fs::read_to_string(&done[0].path)?;
        assert!(xml.contains(">Film</rdf:li>") && xml.contains(">Test cube</rdf:li>"));
        // The cube itself is not copied: the look is what the library holds.
        assert_eq!(std::fs::read_dir(&dest)?.count(), 1);
        let (profiles, errors) = installed_in(&[dest], &with_matrix(), &Cameras::default());
        assert!(errors.is_empty(), "{errors:?}");
        let look = profiles
            .iter()
            .find(|p| p.name == "Portra")
            .expect("listed");
        let enhanced = look.enhanced.as_ref().unwrap();
        assert_eq!(enhanced.base_name, super::super::open::STANDARD);
        assert!(enhanced.rgb.is_some() && enhanced.amount.is_some());
        Ok(())
    }

    /// Importing the same cube again changes nothing and says so, whatever folder it
    /// comes from: its table, not its bytes or its group, is what identifies it.
    #[test]
    fn importing_a_cube_again_is_idempotent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dest = temp.path().join("library");
        let cube = write_cube(&temp.path().join("Film"), "Portra.cube", warm);
        import_into(std::slice::from_ref(&cube), &dest)?;
        let before = std::fs::read(dest.join("Portra.xmp"))?;
        let moved = write_cube(&temp.path().join("Elsewhere"), "Portra.cube", warm);
        // Rewritten with other whitespace and a comment: other bytes, the same table.
        let text = std::fs::read_to_string(&moved)?;
        std::fs::write(
            &moved,
            format!("# exported again\n{}", text.replace(' ', "  ")),
        )?;
        for path in [cube, moved] {
            let done = import_into(std::slice::from_ref(&path), &dest)?;
            let report = cube_of(&done[0]);
            assert!(report.already, "{report}");
            assert_eq!(done[0].path, dest.join("Portra.xmp"));
        }
        assert_eq!(std::fs::read(dest.join("Portra.xmp"))?, before);
        assert_eq!(std::fs::read_dir(&dest)?.count(), 1);
        Ok(())
    }

    /// A different cube under a name the library has is renamed after its folder,
    /// never shadowing or shadowed: another cube's look, a look XMP (an Adobe name
    /// included), a DCP or RAWmakase's own profiles. A renamed one imports again as
    /// already imported.
    #[test]
    fn a_name_collision_is_renamed_and_reported() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dest = temp.path().join("library");
        std::fs::create_dir_all(&dest)?;
        let warm_cube = write_cube(&temp.path().join("Warm"), "Portra.cube", warm);
        import_into(&[warm_cube], &dest)?;
        // An Adobe look's name, held by a look XMP whose file is named otherwise.
        let other = write_cube(&temp.path().join("x"), "Other.cube", warm);
        let converted = super::super::cube_look::convert(&std::fs::read(&other)?)?;
        std::fs::write(
            dest.join("adobe-vivid.xmp"),
            super::super::cube_look::look_xmp(&converted, "Adobe Vivid", "Adobe"),
        )?;
        std::fs::write(dest.join("mine.dcp"), dcp("Test Camera", "Camera Neutral"))?;
        let cool_dir = temp.path().join("Cool");
        let picks = [
            write_cube(&cool_dir, "Portra.cube", cool),
            write_cube(&cool_dir, "Adobe Vivid.cube", cool),
            write_cube(&cool_dir, "Camera Neutral.cube", cool),
            write_cube(&cool_dir, "RAWmakase Standard.cube", cool),
            // Not imported, but the name Lightroom edits fall back by.
            write_cube(&cool_dir, "Adobe Standard.cube", cool),
        ];
        let done = import_into(&picks, &dest)?;
        let names: Vec<_> = done.iter().map(|d| cube_of(d).name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Portra (Cool)",
                "Adobe Vivid (Cool)",
                "Camera Neutral (Cool)",
                "RAWmakase Standard (Cool)",
                "Adobe Standard (Cool)"
            ]
        );
        assert!(
            done.iter()
                .all(|d| cube_of(d).renamed() && !cube_of(d).already)
        );
        assert!(
            cube_of(&done[0])
                .to_string()
                .contains("as another profile is named \"Portra\"")
        );
        // A third Portra goes further; the renamed one imports again as itself.
        let third = write_cube(&temp.path().join("b/Cool"), "Portra.cube", |p| p);
        let done = import_into(&[third], &dest)?;
        assert_eq!(cube_of(&done[0]).name, "Portra (Cool 2)");
        let done = import_into(&picks[..1], &dest)?;
        assert!(cube_of(&done[0]).already);
        assert_eq!(cube_of(&done[0]).name, "Portra (Cool)");
        // The same cube from another folder is found under its new name.
        let copy = write_cube(&temp.path().join("Cool copy"), "Portra.cube", cool);
        let done = import_into(&[copy], &dest)?;
        assert!(cube_of(&done[0]).already, "{}", cube_of(&done[0]));
        assert_eq!(cube_of(&done[0]).name, "Portra (Cool)");
        // Every look lists under its own name.
        let (profiles, _) = installed_in(&[dest], &with_matrix(), &Cameras::default());
        let listed = names_all(&profiles);
        for name in [
            "Portra",
            "Portra (Cool)",
            "Portra (Cool 2)",
            "Adobe Vivid",
            "Adobe Vivid (Cool)",
        ] {
            assert_eq!(
                listed.iter().filter(|n| **n == name).count(),
                1,
                "{name}: {listed:?}"
            );
        }
        Ok(())
    }

    /// Something under the look's file name the library scan doesn't list (a
    /// folder, say) is an error, not a look reported as imported but never written.
    #[test]
    fn a_cubes_look_never_claims_a_file_it_did_not_write() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dest = temp.path().join("library");
        std::fs::create_dir_all(dest.join("Portra.xmp"))?;
        let cube = write_cube(&temp.path().join("Film"), "Portra.cube", warm);
        assert!(import_into(&[cube], &dest).is_err());
        Ok(())
    }

    /// A cube the table cannot hold refuses the batch, as any unreadable profile does,
    /// before anything is written.
    #[test]
    fn a_refused_cube_fails_the_batch_before_writing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let good = write_cube(&temp.path().join("Pack"), "Good.cube", warm);
        let bad = temp.path().join("Pack/Log.cube");
        std::fs::write(
            &bad,
            super::super::cube_look::tests::cube_text(2, |p| p).replace(
                "LUT_3D_SIZE 2",
                "LUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 4 4 4",
            ),
        )?;
        let dest = temp.path().join("library");
        let error = import_into(&[good, bad], &dest).unwrap_err();
        assert!(format!("{error:#}").starts_with("Log.cube: "), "{error:#}");
        assert!(!dest.join("Good.xmp").exists());
        assert!(read_cube(&std::fs::read(temp.path().join("Pack/Good.cube"))?).is_ok());
        Ok(())
    }
}
