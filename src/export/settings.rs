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
impl Format {
    /// The extension an export is named with.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Tiff => "tif",
        }
    }
    pub fn mime_type(self) -> &'static str {
        match self {
            Format::Jpeg => "image/jpeg",
            Format::Tiff => "image/tiff",
        }
    }
    /// The format `path`'s extension names, in any case: .jpg, .jpeg, .tif
    /// or .tiff.
    pub fn from_path(path: &Path) -> Option<Format> {
        match path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase()
            .as_str()
        {
            "jpg" | "jpeg" => Some(Format::Jpeg),
            "tif" | "tiff" => Some(Format::Tiff),
            _ => None,
        }
    }
}

/// Lightroom's Rename To templates that RAWmakase can fill, and the
/// "Filename - text" renaming of earlier releases.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Naming {
    #[default]
    Filename,
    /// "DSC0001-text": earlier releases' Rename To, kept for settings saved
    /// with it.
    FilenameText,
    FilenameSequence,
    CustomName,
    CustomNameSequence,
    /// "Text (1 of 3)".
    CustomNameOf,
    /// "20260504-DSC0001", from the capture date.
    DateFilename,
}
impl Naming {
    pub const ALL: [Self; 7] = [
        Self::Filename,
        Self::FilenameSequence,
        Self::FilenameText,
        Self::CustomName,
        Self::CustomNameSequence,
        Self::CustomNameOf,
        Self::DateFilename,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Filename => "Filename",
            Self::FilenameText => "Filename - Custom Text",
            Self::FilenameSequence => "Filename - Sequence",
            Self::CustomName => "Custom Name",
            Self::CustomNameSequence => "Custom Name - Sequence",
            Self::CustomNameOf => "Custom Name (x of y)",
            Self::DateFilename => "Date - Filename",
        }
    }
    pub fn uses_text(self) -> bool {
        matches!(
            self,
            Self::FilenameText | Self::CustomName | Self::CustomNameSequence | Self::CustomNameOf
        )
    }
    pub fn uses_sequence(self) -> bool {
        matches!(
            self,
            Self::FilenameSequence | Self::CustomNameSequence | Self::CustomNameOf
        )
    }
}

/// Where a photo stands in its export, for the names that number or date it.
#[derive(Clone, Copy, Debug)]
pub struct NameContext<'a> {
    /// Its place in the export, from 0, in the order the photos were chosen.
    pub index: usize,
    pub total: usize,
    /// Its capture time as the catalog keeps it ("2026-05-04 10:21:33.000"),
    /// if known.
    pub captured: Option<&'a str>,
}
impl NameContext<'_> {
    /// One photo exported on its own, its date unknown.
    pub const ONE: NameContext<'static> = NameContext {
        index: 0,
        total: 1,
        captured: None,
    };
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
    /// Settings saved before the naming templates: "Filename - text" when on.
    pub rename: bool,
    pub custom_text: String,
    /// The Rename To template; `None` in settings saved before there was a
    /// choice, which follow `rename`.
    pub naming: Option<Naming>,
    /// Where Sequence starts.
    pub start_number: u32,
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
            naming: None,
            start_number: 1,
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
    /// The Rename To template: the one chosen, or what earlier releases'
    /// checkbox meant.
    pub fn naming(&self) -> Naming {
        self.naming.unwrap_or(if self.rename {
            Naming::FilenameText
        } else {
            Naming::Filename
        })
    }
    pub fn file_name(&self, source: &Path) -> String {
        self.file_name_for(source, &NameContext::ONE).0
    }
    /// `source`'s exported name, as the template makes it for its place in the
    /// export, and a note when it lacks what the template needs (a capture date).
    pub fn file_name_for(&self, source: &Path, at: &NameContext) -> (String, Option<String>) {
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        let text = name_part(&self.custom_text);
        let text = text.as_str();
        // A custom name left empty is the file's own.
        let custom = if text.is_empty() { &*stem } else { text };
        let sequence = self.start_number as usize + at.index;
        let mut note = None;
        let name = match self.naming() {
            Naming::Filename => stem.to_string(),
            Naming::FilenameText if text.is_empty() => stem.to_string(),
            Naming::FilenameText => format!("{stem}-{text}"),
            Naming::FilenameSequence => format!("{stem}-{sequence}"),
            Naming::CustomName => custom.to_string(),
            Naming::CustomNameSequence => format!("{custom}-{sequence}"),
            Naming::CustomNameOf => format!("{custom} ({sequence} of {})", at.total),
            Naming::DateFilename => match at.captured.and_then(date) {
                Some(date) => format!("{date}-{stem}"),
                // Never a file's modified date, which would look like a real
                // capture date.
                None => {
                    note = Some("no capture date: named undated".into());
                    format!("undated-{stem}")
                }
            },
        };
        // Windows' device names can't be file names, whatever follows a dot.
        let head = name.split('.').next().unwrap_or_default();
        let name = if reserved(head) {
            format!("{head}_{}", &name[head.len()..])
        } else {
            name
        };
        let extension = self.format.extension();
        let name = if self.uppercase {
            format!("{name}.{}", extension.to_uppercase())
        } else {
            format!("{name}.{extension}")
        };
        (name, note)
    }
    /// Where `source` exports to.
    pub fn target(&self, source: &Path) -> Option<PathBuf> {
        self.target_for(source, &NameContext::ONE)
            .map(|(path, _)| path)
    }
    /// Where `source` exports to at its place in the export, and what its name
    /// has to say.
    pub fn target_for(&self, source: &Path, at: &NameContext) -> Option<(PathBuf, Option<String>)> {
        let mut folder = self.base_folder(source)?;
        let name = self.subfolder_name.trim();
        if self.subfolder && !name.is_empty() {
            folder.push(name);
        }
        let (file, note) = self.file_name_for(source, at);
        Some((folder.join(file), note))
    }
    pub fn options(&self) -> ExportOptions {
        ExportOptions {
            quality: self.quality.clamp(1, 100),
            max_edge: if self.resize { self.long_edge } else { 0 },
        }
    }
    pub fn mime_type(&self) -> &'static str {
        self.format.mime_type()
    }
}

/// `text` as part of one file name, on any system: no folder separators or
/// characters Windows refuses, and no leading dots, so it can't name another
/// folder or a hidden file.
fn name_part(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' '])
        .trim()
        .to_string()
}

/// Whether `name` is one of Windows' device names (CON, NUL, COM1…), in any case.
fn reserved(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    matches!(name.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|device| {
            // A digit, or the superscript ¹ ² ³ Windows reserves too.
            name.strip_prefix(device).is_some_and(|n| {
                let mut chars = n.chars();
                matches!(
                    (chars.next(), chars.next()),
                    (Some('0'..='9' | '¹' | '²' | '³'), None)
                )
            })
        })
}

/// "20260504" from a capture time as the catalog keeps it ("2026-05-04
/// 10:21:33") or EXIF writes it ("2026:05:04 10:21:33"); `None` without a whole
/// date.
fn date(captured: &str) -> Option<String> {
    let day = captured.get(..10)?.as_bytes();
    let separated = [4, 7].iter().all(|&i| matches!(day[i], b'-' | b':'));
    let digits: String = [&day[..4], &day[5..7], &day[8..10]]
        .concat()
        .iter()
        .map(|&b| b as char)
        .collect();
    (separated && digits.chars().all(|c| c.is_ascii_digit()) && digits != "00000000")
        .then_some(digits)
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
    fn rename_templates_name_each_photo_as_lightrooms_do() {
        let source = Path::new("/photos/DSC0001.ARW");
        let mut s = ExportSettings {
            custom_text: "Concert".into(),
            start_number: 7,
            ..Default::default()
        };
        let at = NameContext {
            index: 2,
            total: 30,
            captured: Some("2026-05-04 10:21:33.000"),
        };
        let name = |s: &ExportSettings, naming| {
            let s = ExportSettings {
                naming: Some(naming),
                ..s.clone()
            };
            s.file_name_for(source, &at).0
        };
        assert_eq!(name(&s, Naming::Filename), "DSC0001.jpg");
        assert_eq!(name(&s, Naming::FilenameText), "DSC0001-Concert.jpg");
        assert_eq!(name(&s, Naming::FilenameSequence), "DSC0001-9.jpg");
        assert_eq!(name(&s, Naming::CustomName), "Concert.jpg");
        assert_eq!(name(&s, Naming::CustomNameSequence), "Concert-9.jpg");
        assert_eq!(name(&s, Naming::CustomNameOf), "Concert (9 of 30).jpg");
        assert_eq!(name(&s, Naming::DateFilename), "20260504-DSC0001.jpg");
        // EXIF's form of the date too.
        let exif = NameContext {
            captured: Some("2026:05:04 10:21:33"),
            ..at
        };
        s.naming = Some(Naming::DateFilename);
        assert_eq!(s.file_name_for(source, &exif).0, "20260504-DSC0001.jpg");
        // No capture date: "undated", said, never the file's modified date.
        let (name, note) = s.file_name_for(source, &NameContext::ONE);
        assert_eq!(name, "undated-DSC0001.jpg");
        assert!(note.is_some());
        // Custom text names one file, never another folder.
        s.naming = Some(Naming::CustomName);
        for text in ["/tmp/final", "../final", "a\\b:c"] {
            s.custom_text = text.into();
            let name = s.file_name_for(source, &at).0;
            assert!(
                !name.contains(['/', '\\', ':']) && !name.starts_with('.'),
                "{name}"
            );
        }
        // Windows' device names are not left as they are.
        s.custom_text = "con".into();
        assert_eq!(s.file_name_for(source, &at).0, "con_.jpg");
        s.custom_text = "CON.txt".into();
        assert_eq!(s.file_name_for(source, &at).0, "CON_.txt.jpg");
        s.custom_text = "lpt².txt".into();
        assert_eq!(s.file_name_for(source, &at).0, "lpt²_.txt.jpg");
        // An empty custom name is the file's own.
        s.custom_text = " ".into();
        s.naming = Some(Naming::CustomName);
        assert_eq!(s.file_name_for(source, &at).0, "DSC0001.jpg");
    }
    #[test]
    fn settings_saved_before_the_templates_keep_their_names() {
        let renamed: ExportSettings =
            serde_json::from_str(r#"{"rename": true, "custom_text": "web"}"#).unwrap();
        assert_eq!(renamed.naming(), Naming::FilenameText);
        assert_eq!(
            renamed.file_name(Path::new("/p/DSC0001.ARW")),
            "DSC0001-web.jpg"
        );
        let plain: ExportSettings = serde_json::from_str(r#"{"custom_text": "web"}"#).unwrap();
        assert_eq!(plain.naming(), Naming::Filename);
        assert_eq!(plain.start_number, 1);
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
