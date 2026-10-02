use crate::{develop::Recipe, export::ExportOptions};
use std::path::PathBuf;
#[derive(Clone, Debug, Default)]
pub struct Photo {
    pub id: i64,
    pub folder: i64,
    pub path: PathBuf,
    pub filename: String,
    pub captured: String,
    pub rating: i32,
    pub flag: i32,
    pub label: String,
    pub format: String,
    pub copy_name: String,
    /// The photo this one is a virtual copy of; `None` for a master.
    pub master: Option<i64>,
    pub keywords: String,
    pub has_lightroom_edits: bool,
}
impl Photo {
    /// The capture time as Lightroom shows it, "29/06/2016 18:24:27.000";
    /// empty when there is none.
    pub fn capture_text(&self) -> String {
        let c = &self.captured;
        let part = |range: std::ops::Range<usize>| c.get(range).unwrap_or("");
        if c.len() < 19 {
            return c.clone();
        }
        format!(
            "{}/{}/{} {}",
            part(8..10),
            part(5..7),
            part(0..4),
            part(11..c.len())
        )
    }
}
#[derive(Clone, Debug)]
pub struct Folder {
    pub relative: String,
    pub id: i64,
    pub root: i64,
    pub name: String,
    pub path: PathBuf,
    pub count: usize,
}
#[derive(Clone, Debug)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub parent: Option<i64>,
    pub kind: CollectionKind,
    pub count: usize,
}
/// The name Lightroom gives its Quick Collection.
pub const QUICK_COLLECTION: &str = "quick collection";
/// What a Lightroom collection row is, from its `creationId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionKind {
    /// A collection set: holds collections and other sets, never photos.
    Set,
    /// A collection, or a saved print, book, slideshow or web creation.
    Collection,
    /// A smart collection, whose rules are not evaluated yet.
    Smart,
    /// Lightroom's own: the Quick Collection and unsaved creations.
    System,
}
impl CollectionKind {
    pub fn from_lightroom(creation_id: &str, name: &str) -> Self {
        match creation_id {
            "com.adobe.ag.library.group" => Self::Set,
            "com.adobe.ag.library.smart_collection" => Self::Smart,
            // Lightroom keeps its Quick Collection as a plain collection by this name.
            "com.adobe.ag.library.collection" if name == QUICK_COLLECTION => Self::System,
            id if id.ends_with(".unsaved") => Self::System,
            _ => Self::Collection,
        }
    }
}
/// A photo's camera settings and size, as Lightroom's Metadata panel shows
/// them. Each is `None` when unknown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhotoInfo {
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// Millimetres.
    pub focal: Option<f64>,
    /// The f-number.
    pub aperture: Option<f64>,
    /// Seconds.
    pub exposure: Option<f64>,
    pub iso: Option<f64>,
    /// As the photo is shown, after its orientation.
    pub dimensions: Option<(u32, u32)>,
}
impl PhotoInfo {
    /// From a RAW's metadata, as LibRaw reads it.
    pub fn from_metadata(m: &crate::raw::Metadata) -> Self {
        let positive = |v: f32| (v > 0.).then_some(v as f64);
        let text = |t: &str| (!t.trim().is_empty()).then(|| t.trim().to_string());
        // LibRaw's flip 5 and 6 are quarter turns.
        // The camera's default crop is the frame shown, when it has one.
        let (w, h) = if m.crop_width > 0 && m.crop_height > 0 {
            (m.crop_width, m.crop_height)
        } else {
            (m.width, m.height)
        };
        let (w, h) = if matches!(m.flip, 5 | 6) {
            (h, w)
        } else {
            (w, h)
        };
        Self {
            camera: text(&m.model).or_else(|| text(&m.make)),
            lens: text(&m.lens_model),
            focal: positive(m.focal),
            aperture: positive(m.aperture),
            exposure: positive(m.shutter),
            iso: positive(m.iso),
            dimensions: (w > 0 && h > 0).then_some((w, h)),
        }
    }
    /// "1/250 sec", or "2 sec" for long exposures.
    pub fn shutter_text(&self) -> Option<String> {
        let t = self.exposure.filter(|t| *t > 0.)?;
        // A fraction only where it is one: 1/250, but 0.8 sec.
        let r = 1. / t;
        Some(if t < 1. && r >= 1.5 && (r - r.round()).abs() < 0.03 * r {
            format!("1/{:.0} sec", r)
        } else {
            format!("{} sec", trim(t, 1))
        })
    }
    pub fn aperture_text(&self) -> Option<String> {
        let f = self.aperture.filter(|f| *f > 0.)?;
        Some(format!("f/{}", trim(f, 1)))
    }
    /// Lightroom's Exposure row: "1/250 sec at f/2.8".
    pub fn exposure_text(&self) -> Option<String> {
        match (self.shutter_text(), self.aperture_text()) {
            (Some(s), Some(a)) => Some(format!("{s} at {a}")),
            (s, a) => s.or(a),
        }
    }
    pub fn focal_text(&self) -> Option<String> {
        let mm = self.focal.filter(|mm| *mm > 0.)?;
        Some(format!("{} mm", trim(mm, 1)))
    }
    pub fn iso_text(&self) -> Option<String> {
        let iso = self.iso.filter(|iso| *iso > 0.)?;
        Some(format!("ISO {iso:.0}"))
    }
    pub fn dimensions_text(&self) -> Option<String> {
        let (w, h) = self.dimensions.filter(|(w, h)| *w > 0 && *h > 0)?;
        Some(format!("{w} × {h}"))
    }
}
/// `value` with at most `places` decimals and none that are zero.
fn trim(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').into()
    } else {
        text
    }
}
#[derive(Debug)]
pub struct SavedEdit {
    pub recipe: Recipe,
    pub export: ExportOptions,
}
