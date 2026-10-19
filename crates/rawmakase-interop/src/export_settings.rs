//! The choices of Lightroom's Export dialog: where the file goes, its name, format,
//! size and metadata. Saved as `export.json` in the data folder, so the next
//! export (and Export with Previous) starts from them.
use crate::storage::RasterFormat;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The Export dialog's choice that takes its text from the photo's
/// copyright, as Lightroom's Simple Copyright Watermark does.
pub const SIMPLE_COPYRIGHT: &str = "<simple-copyright>";

/// A photo's own export choices, saved with its edit.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExportOptions {
    pub quality: u8,
    pub max_edge: u32,
}
impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            quality: 92,
            max_edge: 0,
        }
    }
}
impl ExportOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=100).contains(&self.quality),
            "JPEG quality must be 1–100"
        );
        ensure!(
            self.max_edge <= 30_000,
            "Export edge must not exceed 30000 pixels"
        );
        Ok(())
    }
}

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
    Png,
}
impl Format {
    /// The formats an export can be written in.
    pub const ALL: [Self; 3] = [Self::Jpeg, Self::Tiff, Self::Png];

    pub fn raster(self) -> RasterFormat {
        match self {
            Format::Jpeg => RasterFormat::Jpeg,
            Format::Tiff => RasterFormat::Tiff,
            Format::Png => RasterFormat::Png,
        }
    }
    /// The extension an export is named with.
    pub fn extension(self) -> &'static str {
        self.raster().extension()
    }
    pub fn mime_type(self) -> &'static str {
        self.raster().mime_type()
    }
    /// The format `path`'s extension names, in any case: .jpg, .jpeg, .tif,
    /// .tiff or .png.
    pub fn from_path(path: &Path) -> Option<Format> {
        let raster = RasterFormat::of(path)?;
        Self::ALL.into_iter().find(|f| f.raster() == raster)
    }
    /// Every extension an export can have, as a sentence lists them:
    /// ".jpg, .jpeg, .tif, .tiff or .png".
    pub fn extensions_listed() -> String {
        let extensions: Vec<String> = Self::ALL
            .into_iter()
            .flat_map(|f| f.raster().extensions())
            .map(|e| format!(".{e}"))
            .collect();
        match extensions.split_last() {
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
            None => String::new(),
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

/// Lightroom's Post-Processing: what happens once an export is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AfterExport {
    #[default]
    Nothing,
    /// The exported files, in the system's file manager.
    Show,
}
impl AfterExport {
    pub const ALL: [Self; 2] = [Self::Nothing, Self::Show];
    pub fn label(self) -> &'static str {
        match self {
            Self::Nothing => "Do Nothing",
            Self::Show if cfg!(target_os = "macos") => "Show in Finder",
            Self::Show if cfg!(windows) => "Show in Explorer",
            Self::Show => "Show in File Manager",
        }
    }
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
    /// JPEG when the saved format is one only a later release knows; see
    /// [`LaterFormat`].
    #[serde(deserialize_with = "known_or_default")]
    pub format: Format,
    #[serde(skip)]
    pub later_format: LaterFormat,
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
    /// Lightroom's After Export.
    pub after_export: AfterExport,
    /// Settings a later release added; see [`LaterFields`].
    #[serde(flatten)]
    pub later_fields: LaterFields,
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
            later_format: LaterFormat::default(),
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
            watermark_name: SIMPLE_COPYRIGHT.into(),
            after_export: AfterExport::Nothing,
            later_fields: LaterFields::default(),
        }
    }
}

/// The saved format when only a later release knows it. JPEG stands in for
/// it here, and it is saved back until a format is chosen, so that release
/// still finds its choice after this one saves the settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaterFormat(Option<serde_json::Value>);

/// The saved settings only a later release knows, as they were read. They
/// mean nothing here and are saved back untouched, so that release still finds
/// them after this one saves the settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LaterFields(serde_json::Map<String, serde_json::Value>);

/// A choice a later release added reads as the default, so an older one
/// keeps the rest of the settings.
fn known_or_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Read<T> {
        Known(T),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Read::deserialize(d)? {
        Read::Known(value) => value,
        Read::Other(_) => T::default(),
    })
}

fn path() -> PathBuf {
    crate::storage::data_dir().join("export.json")
}
/// The desktop's own Desktop and Pictures folders: on Linux those named in
/// user-dirs.dirs (in the desktop's language, or moved by the user), on
/// Windows those the shell reports (OneDrive backup moves them), and on macOS
/// those in the home folder. Without one, the folder of that name at home.
fn desktop() -> PathBuf {
    user_folder(directories::UserDirs::desktop_dir, "Desktop")
}
fn pictures() -> PathBuf {
    user_folder(directories::UserDirs::picture_dir, "Pictures")
}
fn user_folder(folder: fn(&directories::UserDirs) -> Option<&Path>, name: &str) -> PathBuf {
    let dirs = directories::UserDirs::new();
    match &dirs {
        Some(dirs) => folder(dirs).map_or_else(|| dirs.home_dir().join(name), Path::to_path_buf),
        None => PathBuf::from(name),
    }
}

impl ExportSettings {
    /// The last export's choices, if there was one.
    pub fn load() -> Option<Self> {
        Self::load_from(&path())
    }
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&path())
    }
    /// The format the user picks, which replaces one from a later release.
    pub fn choose_format(&mut self, format: Format) {
        self.format = format;
        self.later_format = LaterFormat::default();
    }
    fn load_from(path: &Path) -> Option<Self> {
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        let later = saved
            .get("format")
            .filter(|format| Format::deserialize(*format).is_err())
            .cloned();
        let mut settings = Self::deserialize(saved).ok()?;
        settings.later_format = LaterFormat(later);
        Some(settings)
    }
    fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        // Still the stand-in it was read as: the later release's choice.
        if let LaterFormat(Some(later)) = &self.later_format
            && self.format == Format::default()
        {
            let mut saved = serde_json::to_value(self)?;
            saved["format"] = later.clone();
            return crate::storage::atomic_json(path, &saved);
        }
        crate::storage::atomic_json(path, self)
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
    #[cfg(test)]
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
#[cfg(test)]
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
    /// The Desktop and Pictures destinations, printed for
    /// [`desktop_and_pictures_are_the_xdg_user_folders`], which runs this
    /// test in its own process with its own home and configuration.
    #[test]
    #[ignore = "run by desktop_and_pictures_are_the_xdg_user_folders"]
    fn print_desktop_and_pictures() {
        println!("desktop={}", desktop().display());
        println!("pictures={}", pictures().display());
    }
    /// A desktop in another language, or one the user moved, names its
    /// folders in user-dirs.dirs; without that file they are under home.
    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_and_pictures_are_the_xdg_user_folders() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;
        let config = home.path().join("config");
        let folders = |config: &Path| -> anyhow::Result<(String, String)> {
            let out = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "export_settings::tests::print_desktop_and_pictures",
                ])
                .args(["--ignored", "--nocapture", "--test-threads=1"])
                .env("HOME", home.path())
                .env("XDG_CONFIG_HOME", config)
                .output()?;
            let out = String::from_utf8(out.stdout)?;
            let value = |key: &str| {
                out.lines()
                    .find_map(|line| line.split_once(key).map(|(_, value)| value))
                    .map(str::to_owned)
                    .unwrap_or_default()
            };
            Ok((value("desktop="), value("pictures=")))
        };
        let under_home = |name: &str| home.path().join(name).display().to_string();
        assert_eq!(
            folders(&config)?,
            (under_home("Desktop"), under_home("Pictures"))
        );
        std::fs::create_dir_all(&config)?;
        std::fs::write(
            config.join("user-dirs.dirs"),
            "XDG_DESKTOP_DIR=\"$HOME/Pulpit\"\nXDG_PICTURES_DIR=\"/srv/photos\"\n",
        )?;
        assert_eq!(
            folders(&config)?,
            (under_home("Pulpit"), "/srv/photos".into())
        );
        Ok(())
    }
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
    fn each_format_is_named_by_the_extension_it_is_read_back_from() {
        for format in [Format::Jpeg, Format::Tiff, Format::Png] {
            let path = PathBuf::from(format!("out.{}", format.extension().to_uppercase()));
            assert_eq!(Format::from_path(&path), Some(format));
        }
        assert_eq!(Format::from_path(Path::new("out.webp")), None);
    }
    #[test]
    fn a_format_from_a_later_release_keeps_the_other_settings() {
        let saved = r#"{"destination":"Folder","folder":"/exports","format":"Webp","quality":80}"#;
        let read: ExportSettings = serde_json::from_str(saved).unwrap();
        assert_eq!(read.format, Format::Jpeg);
        assert_eq!(read.folder, Some(PathBuf::from("/exports")));
        assert_eq!(read.quality, 80);
        let read: ExportSettings = serde_json::from_str(r#"{"format":"Png"}"#).unwrap();
        assert_eq!(read.format, Format::Png);
    }
    #[test]
    fn a_format_from_a_later_release_is_saved_back_until_another_is_chosen() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("export.json");
        let saved_format = |file: &Path| -> anyhow::Result<serde_json::Value> {
            let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file)?)?;
            Ok(saved["format"].clone())
        };
        std::fs::write(&file, r#"{"format":"Webp","folder":"/x"}"#)?;
        // Exported as JPEG here, and saved again by Export with Previous or a
        // watermark's rename: the later release still finds its choice.
        let mut read = ExportSettings::load_from(&file).unwrap();
        assert_eq!(read.format, Format::Jpeg);
        read.quality = 70;
        read.save_to(&file)?;
        assert_eq!(saved_format(&file)?, "Webp");
        let read = ExportSettings::load_from(&file).unwrap();
        assert_eq!((read.format, read.quality), (Format::Jpeg, 70));
        assert_eq!(read.folder, Some(PathBuf::from("/x")));
        // A format chosen here replaces it, JPEG too.
        let mut png = read.clone();
        png.format = Format::Png;
        png.save_to(&file)?;
        assert_eq!(saved_format(&file)?, "Png");
        std::fs::write(&file, r#"{"format":"Webp"}"#)?;
        let mut jpeg = ExportSettings::load_from(&file).unwrap();
        jpeg.choose_format(Format::Jpeg);
        jpeg.save_to(&file)?;
        assert_eq!(saved_format(&file)?, "Jpeg");
        // The formats known here are saved as they were read.
        for format in Format::ALL {
            let name = serde_json::to_value(format)?;
            std::fs::write(&file, format!(r#"{{"format":{name}}}"#))?;
            ExportSettings::load_from(&file).unwrap().save_to(&file)?;
            assert_eq!(saved_format(&file)?, name);
        }
        Ok(())
    }
    #[test]
    fn settings_a_later_release_added_are_saved_back() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("export.json");
        let saved = |file: &Path| -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::from_str(&std::fs::read_to_string(file)?)?)
        };
        std::fs::write(
            &file,
            r#"{"folder":"/x","quality":80,"sharpen":{"amount":2},"newer_flag":true}"#,
        )?;
        let mut read = ExportSettings::load_from(&file).unwrap();
        assert_eq!(read.folder, Some(PathBuf::from("/x")));
        assert_eq!(read.quality, 80);
        // Saved again by an export here: the later release still finds its
        // settings, and what was changed here is saved.
        read.quality = 70;
        read.save_to(&file)?;
        let written = saved(&file)?;
        assert_eq!(written["sharpen"], serde_json::json!({"amount": 2}));
        assert_eq!(written["newer_flag"], true);
        assert_eq!(written["folder"], "/x");
        assert_eq!(written["quality"], 70);
        assert_eq!(ExportSettings::load_from(&file).unwrap(), read);
        // With a format only that release knows, both are saved back.
        std::fs::write(&file, r#"{"format":"Webp","newer_flag":true}"#)?;
        ExportSettings::load_from(&file).unwrap().save_to(&file)?;
        let written = saved(&file)?;
        assert_eq!(written["format"], "Webp");
        assert_eq!(written["newer_flag"], true);
        Ok(())
    }
    #[test]
    fn settings_this_release_knows_are_saved_as_they_were() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("export.json");
        let mut known = vec![
            "destination",
            "folder",
            "subfolder",
            "subfolder_name",
            "existing",
            "rename",
            "custom_text",
            "naming",
            "start_number",
            "uppercase",
            "format",
            "quality",
            "resize",
            "long_edge",
            "ppi",
            "capture",
            "location",
            "develop",
            "descriptive",
            "include",
            "remove_location",
            "watermark",
            "watermark_name",
            "after_export",
        ];
        known.sort_unstable();
        let keys = |value: &serde_json::Value| -> Vec<String> {
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort_unstable();
            keys
        };
        assert_eq!(
            keys(&serde_json::to_value(ExportSettings::default())?),
            known
        );
        std::fs::write(&file, r#"{"quality":80,"folder":"/x"}"#)?;
        ExportSettings::load_from(&file).unwrap().save_to(&file)?;
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
        assert_eq!(keys(&saved), known);
        assert_eq!(
            (saved["quality"].clone(), saved["folder"].clone()),
            (80.into(), "/x".into())
        );
        Ok(())
    }
    #[test]
    fn every_export_extension_and_mime_type_is_what_it_was() {
        for (name, format) in [
            ("a.jpeg", Format::Jpeg),
            ("a.JPEG", Format::Jpeg),
            ("a.tiff", Format::Tiff),
            ("a.Tiff", Format::Tiff),
            ("a.png", Format::Png),
        ] {
            assert_eq!(Format::from_path(Path::new(name)), Some(format), "{name}");
        }
        assert_eq!(
            Format::ALL.map(Format::mime_type),
            ["image/jpeg", "image/tiff", "image/png"]
        );
        assert_eq!(
            Format::extensions_listed(),
            ".jpg, .jpeg, .tif, .tiff or .png"
        );
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
