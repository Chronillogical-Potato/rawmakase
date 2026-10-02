//! Lightroom's sort orders for the grid and filmstrip. Photos come from the
//! catalog in capture order; another order sorts them by its key, stably,
//! so photos alike in it stay in capture order. Edit time and aspect ratio
//! are not on `Photo` and are read from the catalog as they are needed.
use crate::app::photo_metadata::LABELS;
use crate::catalog::{Catalog, Photo};
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Sort {
    #[default]
    CaptureTime,
    AddedOrder,
    EditTime,
    Rating,
    Pick,
    LabelColor,
    LabelText,
    FileName,
    Extension,
    AspectRatio,
}
impl Sort {
    pub(super) const ALL: [Self; 10] = [
        Self::CaptureTime,
        Self::AddedOrder,
        Self::EditTime,
        Self::Rating,
        Self::Pick,
        Self::LabelColor,
        Self::LabelText,
        Self::FileName,
        Self::Extension,
        Self::AspectRatio,
    ];
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::CaptureTime => "Capture Time",
            Self::AddedOrder => "Added Order",
            Self::EditTime => "Edit Time",
            Self::Rating => "Rating",
            Self::Pick => "Pick",
            Self::LabelColor => "Label Color",
            Self::LabelText => "Label Text",
            Self::FileName => "File Name",
            Self::Extension => "File Extension",
            Self::AspectRatio => "Aspect Ratio",
        }
    }
    /// Reads what this order needs beyond `Photo` from the catalog.
    pub(super) fn keys(self, catalog: &Catalog) -> Keys {
        match self {
            Self::EditTime => Keys::EditTimes(catalog.edit_times().unwrap_or_default()),
            Self::AspectRatio => Keys::Aspects(catalog.aspect_ratios().unwrap_or_default()),
            _ => Keys::None,
        }
    }
    /// Sorts `indices` into `photos` by this order; ties keep their order.
    pub(super) fn sort(self, photos: &[Photo], indices: &mut [usize], keys: &Keys) {
        if self == Self::CaptureTime {
            return;
        }
        indices.sort_by(|a, b| self.compare(&photos[*a], &photos[*b], keys));
    }
    fn compare(self, a: &Photo, b: &Photo, keys: &Keys) -> Ordering {
        match self {
            Self::CaptureTime => Ordering::Equal,
            Self::AddedOrder => a.id.cmp(&b.id),
            Self::EditTime => {
                let time = |p: &Photo| match keys {
                    Keys::EditTimes(times) => times.get(&p.id).cloned(),
                    _ => None,
                };
                // Photos never edited come first, as the oldest.
                time(a).cmp(&time(b))
            }
            Self::Rating => a.rating.cmp(&b.rating),
            Self::Pick => a.flag.cmp(&b.flag),
            Self::LabelColor => label_rank(&a.label).cmp(&label_rank(&b.label)),
            Self::LabelText => a.label.to_lowercase().cmp(&b.label.to_lowercase()),
            Self::FileName => natural(&a.filename, &b.filename),
            Self::Extension => extension(&a.filename).cmp(&extension(&b.filename)),
            Self::AspectRatio => {
                let aspect = |p: &Photo| match keys {
                    Keys::Aspects(aspects) => aspects.get(&p.id).copied(),
                    _ => None,
                };
                // Photos of unknown shape go last.
                match (aspect(a), aspect(b)) {
                    (Some(a), Some(b)) => a.total_cmp(&b),
                    (a, b) => a.is_none().cmp(&b.is_none()),
                }
            }
        }
    }
}

/// What an order needs beyond `Photo`, by photo.
#[derive(Debug, Default)]
pub(super) enum Keys {
    #[default]
    None,
    EditTimes(HashMap<i64, String>),
    Aspects(HashMap<i64, f32>),
}

/// Lightroom's label colour order: red, yellow, green, blue, purple, then
/// other labels, then none.
fn label_rank(label: &str) -> usize {
    match LABELS.iter().position(|l| *l == label) {
        Some(at) => at,
        None if label.is_empty() => LABELS.len() + 1,
        None => LABELS.len(),
    }
}
fn extension(filename: &str) -> String {
    std::path::Path::new(filename)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}
/// File names as people read them: case aside, and runs of digits by value,
/// so IMG_9 comes before IMG_10.
fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |chars: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = chars.next_if(char::is_ascii_digit) {
                        digits.push(c);
                    }
                    digits
                };
                let (x, y) = (number(&mut a), number(&mut b));
                let (tx, ty) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_sort_as_people_read_them() {
        let mut names = [
            "img_10.jpg",
            "IMG_9.jpg",
            "img_010b.jpg",
            "a.jpg",
            "IMG_9.jpg",
        ];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(
            names,
            [
                "a.jpg",
                "IMG_9.jpg",
                "IMG_9.jpg",
                "img_10.jpg",
                "img_010b.jpg"
            ]
        );
    }

    #[test]
    fn labels_sort_in_lightrooms_colour_order() {
        let mut labels = ["", "Purple", "Client", "Red", "Green"];
        labels.sort_by_key(|l| label_rank(l));
        assert_eq!(labels, ["Red", "Green", "Purple", "Client", ""]);
    }
}
