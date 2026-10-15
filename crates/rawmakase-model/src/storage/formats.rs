//! The rendered file formats RAWmakase opens beside RAWs, as facts with no
//! codec: their extensions, MIME types and names. The decoders are
//! `native::raster`'s, the encoders `export::encode`'s, and which formats
//! export writes is `export_settings::Format`.
use std::path::Path;

/// A rendered photo's file format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RasterFormat {
    Jpeg,
    Tiff,
    Png,
}

impl RasterFormat {
    pub const ALL: [Self; 3] = [Self::Jpeg, Self::Tiff, Self::Png];

    /// The extensions a file of this format has, lower case; the first is
    /// the one RAWmakase names files with.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Jpeg => &["jpg", "jpeg"],
            Self::Tiff => &["tif", "tiff"],
            Self::Png => &["png"],
        }
    }
    /// The extension RAWmakase names a file of this format with.
    pub fn extension(self) -> &'static str {
        self.extensions()[0]
    }
    pub fn mime_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Tiff => "image/tiff",
            Self::Png => "image/png",
        }
    }
    /// The format's name, as the app shows it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Tiff => "TIFF",
            Self::Png => "PNG",
        }
    }
    /// The format an extension names, in any case.
    pub fn from_extension(extension: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| {
            f.extensions()
                .iter()
                .any(|e| extension.eq_ignore_ascii_case(e))
        })
    }
    /// The format `path`'s extension names, in any case.
    pub fn of(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(Self::from_extension)
    }
}

/// A file RAWmakase takes as a photo: a RAW, or a JPEG, TIFF or PNG, and not
/// hidden.
pub fn is_photo(path: &Path) -> bool {
    super::is_raw(path) || (!super::is_hidden(path) && RasterFormat::of(path).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_format_is_known_by_any_of_its_extensions_in_any_case() {
        for (name, format) in [
            ("A.jpg", RasterFormat::Jpeg),
            ("A.JPEG", RasterFormat::Jpeg),
            ("A.Jpeg", RasterFormat::Jpeg),
            ("A.tif", RasterFormat::Tiff),
            ("A.TIFF", RasterFormat::Tiff),
            ("A.png", RasterFormat::Png),
            ("dir.png/A.PNG", RasterFormat::Png),
        ] {
            assert_eq!(RasterFormat::of(Path::new(name)), Some(format), "{name}");
        }
        for name in ["A.NEF", "A.dng", "A.webp", "A.jpg.xmp", "jpg", "A."] {
            assert_eq!(RasterFormat::of(Path::new(name)), None, "{name}");
        }
    }
    #[test]
    fn each_extension_is_lower_case_and_names_one_format_and_no_raw() {
        for format in RasterFormat::ALL {
            for extension in format.extensions() {
                assert_eq!(*extension, extension.to_ascii_lowercase());
                assert_eq!(RasterFormat::from_extension(extension), Some(format));
                assert!(!super::super::files::RAW_EXTENSIONS.contains(extension));
            }
        }
    }
    #[test]
    fn files_are_named_jpg_tif_and_png() {
        let named = RasterFormat::ALL.map(|f| (f.extension(), f.mime_type(), f.name()));
        assert_eq!(
            named,
            [
                ("jpg", "image/jpeg", "JPEG"),
                ("tif", "image/tiff", "TIFF"),
                ("png", "image/png", "PNG"),
            ]
        );
    }
    #[test]
    fn photos_are_raws_jpegs_tiffs_and_pngs_not_hidden() {
        for name in ["A.NEF", "A.jpeg", "A.TIF", "A.png"] {
            assert!(is_photo(Path::new(name)), "{name}");
        }
        for name in ["._A.jpg", ".A.png", "A.xmp", "A.webp", "A"] {
            assert!(!is_photo(Path::new(name)), "{name}");
        }
    }
}
