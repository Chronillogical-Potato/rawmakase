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
    /// Rating, color label and keywords (XMP).
    pub descriptive: bool,
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
        }
    }
}

fn path() -> PathBuf {
    crate::storage::data_dir().join("export.json")
}
/// The user's profile folder; Windows sets USERPROFILE rather than HOME.
fn home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    PathBuf::from(std::env::var_os(var).unwrap_or_default())
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
            Destination::Desktop => Some(home().join("Desktop")),
            Destination::Pictures => Some(home().join("Pictures")),
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
