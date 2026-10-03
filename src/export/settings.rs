//! The choices of Lightroom's Export dialog: where the file goes, its name, format,
//! size and metadata. Saved as `export.json` in the data folder, so the next
//! export (and Export with Previous) starts from them.
use super::ExportOptions;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Destination {
    #[default]
    SameFolder,
    Desktop,
    Pictures,
    Folder,
}
impl Destination {
    pub const ALL: [Self; 4] = [
        Self::SameFolder,
        Self::Desktop,
        Self::Pictures,
        Self::Folder,
    ];
}
/// What to do when the exported file already exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Existing {
    #[default]
    Ask,
    Unique,
    Overwrite,
    Skip,
}
impl Existing {
    pub const ALL: [Self; 4] = [Self::Ask, Self::Unique, Self::Overwrite, Self::Skip];
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Format {
    #[default]
    Jpeg,
    Tiff,
}

/// Lightroom's Metadata "Include" choice, or Custom: the four switches
/// earlier releases had, which settings saved by them become.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Include {
    CopyrightOnly,
    CopyrightAndContact,
    AllExceptCameraAndCameraRaw,
    AllExceptCameraRaw,
    All,
    #[default]
    Custom,
}
impl Include {
    pub const ALL: [Self; 6] = [
        Self::CopyrightOnly,
        Self::CopyrightAndContact,
        Self::AllExceptCameraRaw,
        Self::AllExceptCameraAndCameraRaw,
        Self::All,
        Self::Custom,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::CopyrightOnly => "Copyright Only",
            Self::CopyrightAndContact => "Copyright & Contact Info Only",
            Self::AllExceptCameraAndCameraRaw => "All Except Camera & Camera Raw Info",
            Self::AllExceptCameraRaw => "All Except Camera Raw Info",
            Self::All => "All Metadata",
            Self::Custom => "Custom",
        }
    }
    /// Lightroom always leaves the location out of these.
    pub fn removes_location(self) -> bool {
        matches!(self, Self::CopyrightOnly | Self::CopyrightAndContact)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub destination: Destination,
    pub folder: Option<PathBuf>,
    pub subfolder: bool,
    pub subfolder_name: String,
    pub existing: Existing,
    pub rename: bool,
    pub custom_text: String,
    pub uppercase: bool,
    pub format: Format,
    pub quality: u8,
    pub resize: bool,
    pub long_edge: u32,
    pub ppi: u32,
    /// Camera, capture settings, lens and dates (EXIF).
    pub capture: bool,
    pub location: bool,
    /// The edit as Camera Raw settings (XMP).
    pub develop: bool,
    /// Rating, color label, keywords, and the title, caption, creator and
    /// copyright set in RAWmakase (XMP, and EXIF with `capture`).
    pub descriptive: bool,
    /// Which metadata goes in; the four switches above apply to Custom.
    pub include: Include,
    /// Lightroom's Remove Location Info, for the modes other than Custom.
    pub remove_location: bool,
    /// Lightroom's Watermarking: whether to, and which: a preset's name or
    /// the Simple Copyright Watermark.
    pub watermark: bool,
    pub watermark_name: String,
}
impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            destination: Destination::SameFolder,
            folder: None,
            subfolder: false,
            subfolder_name: "Exported".into(),
            existing: Existing::Ask,
            rename: false,
            custom_text: "edited".into(),
            uppercase: false,
            format: Format::Jpeg,
            quality: 92,
            resize: false,
            long_edge: 2048,
            ppi: 240,
            capture: true,
            location: true,
            develop: true,
            descriptive: true,
            include: Include::Custom,
            remove_location: false,
            watermark: false,
            watermark_name: crate::watermark::SIMPLE_COPYRIGHT.into(),
        }
    }
}

fn path() -> PathBuf {
    crate::storage::data_dir().join("export.json")
}
#[cfg(not(windows))]
fn desktop() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Desktop")
}
#[cfg(not(windows))]
fn pictures() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Pictures")
}
/// Windows can move Desktop and Pictures (OneDrive backup does), so they are
/// asked for rather than assumed under the profile folder.
#[cfg(windows)]
fn desktop() -> PathBuf {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Desktop, "Desktop")
}
#[cfg(windows)]
fn pictures() -> PathBuf {
    known_folder(
        &windows_sys::Win32::UI::Shell::FOLDERID_Pictures,
        "Pictures",
    )
}
#[cfg(windows)]
fn known_folder(id: &windows_sys::core::GUID, fallback: &str) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::SHGetKnownFolderPath};
    let mut raw: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: on success `raw` is a NUL-terminated string the shell allocated;
    // it is read up to the NUL and then freed, as the API requires either way.
    let found = unsafe {
        let found = (SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut raw) >= 0
            && !raw.is_null())
        .then(|| {
            let len = (0..).take_while(|&i| *raw.add(i) != 0).count();
            PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(
                raw, len,
            )))
        });
        CoTaskMemFree(raw as *const _);
        found
    };
    found.unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join(fallback)
    })
}

impl ExportSettings {
    /// The last export's choices, if there was one.
    pub fn load() -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(path()).ok()?).ok()
    }
    pub fn save(&self) -> anyhow::Result<()> {
        crate::storage::atomic_json(&path(), self)
    }
    /// The chosen folder, before any subfolder; `None` until a specific
    /// folder is picked.
    pub fn base_folder(&self, source: &Path) -> Option<PathBuf> {
        match self.destination {
            Destination::SameFolder => source.parent().map(Path::to_path_buf),
            Destination::Desktop => Some(desktop()),
            Destination::Pictures => Some(pictures()),
            Destination::Folder => self.folder.clone(),
        }
    }
    pub fn file_name(&self, source: &Path) -> String {
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        let text = self.custom_text.trim();
        let name = if self.rename && !text.is_empty() {
            format!("{stem}-{text}")
        } else {
            stem.to_string()
        };
        let extension = match self.format {
            Format::Jpeg => "jpg",
            Format::Tiff => "tif",
        };
        if self.uppercase {
            format!("{name}.{}", extension.to_uppercase())
        } else {
            format!("{name}.{extension}")
        }
    }
    /// Where `source` exports to.
    pub fn target(&self, source: &Path) -> Option<PathBuf> {
        let mut folder = self.base_folder(source)?;
        let name = self.subfolder_name.trim();
        if self.subfolder && !name.is_empty() {
            folder.push(name);
        }
        Some(folder.join(self.file_name(source)))
    }
    pub fn options(&self) -> ExportOptions {
        ExportOptions {
            quality: self.quality.clamp(1, 100),
            max_edge: if self.resize { self.long_edge } else { 0 },
        }
    }
    pub fn mime_type(&self) -> &'static str {
        match self.format {
            Format::Jpeg => "image/jpeg",
            Format::Tiff => "image/tiff",
        }
    }
}

/// "DSC0001-2.jpg", "DSC0001-3.jpg"… for the first name not taken.
pub fn unique(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    (2..)
        .map(|n| path.with_file_name(format!("{stem}-{n}.{extension}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_follows_destination_subfolder_and_naming() {
        let source = Path::new("/photos/2026/DSC0001.ARW");
        let mut s = ExportSettings::default();
        assert_eq!(
            s.target(source),
            Some(PathBuf::from("/photos/2026/DSC0001.jpg"))
        );
        s.subfolder = true;
        s.rename = true;
        s.custom_text = "web".into();
        s.format = Format::Tiff;
        s.uppercase = true;
        assert_eq!(
            s.target(source),
            Some(PathBuf::from("/photos/2026/Exported/DSC0001-web.TIF"))
        );
        s.destination = Destination::Folder;
        assert_eq!(s.target(source), None);
    }
    #[test]
    fn older_settings_files_keep_the_other_defaults() {
        let old: ExportSettings = serde_json::from_str("{\"quality\": 80}").unwrap();
        assert_eq!(old.quality, 80);
        assert!(old.capture && old.develop);
        // Settings saved before the Include popup keep their switches.
        assert_eq!(old.include, Include::Custom);
    }
    #[test]
    fn unique_names_count_up_from_two() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("a.jpg");
        std::fs::write(&path, b"")?;
        std::fs::write(dir.path().join("a-2.jpg"), b"")?;
        assert_eq!(unique(&path), dir.path().join("a-3.jpg"));
        Ok(())
    }
}
