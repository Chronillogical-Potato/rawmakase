//! The Point Curve menu's curves: Lightroom's Linear, Medium Contrast and Strong
//! Contrast, and curves saved here. Saved curves are XMP files in the user library's
//! "Curves" folder, laid out as the ones Lightroom and Camera Raw save in theirs, so
//! either can read them; each file's name is the curve's.
use crate::develop::{Recipe, curve::ToneCurve};
use crate::storage::Replace;
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

/// A whole point curve: the RGB curve and the Red, Green and Blue curves.
#[derive(Clone, Debug, PartialEq)]
pub struct PointCurve {
    pub rgb: ToneCurve,
    pub channels: [ToneCurve; 3],
}

impl PointCurve {
    /// The point curve `r` has.
    pub fn of(r: &Recipe) -> Self {
        Self {
            rgb: r.curve.clone(),
            channels: r.effects.channels.clone(),
        }
    }
    /// Gives `r` this point curve.
    pub fn apply(&self, r: &mut Recipe) {
        r.curve = self.rgb.clone();
        r.effects.channels = self.channels.clone();
    }
    /// The curve on the 0–255 steps a curve file stores. Points that land on the same
    /// input there are merged, keeping the end points, so the file reads back.
    fn quantized(&self) -> Self {
        let quantize = |c: &ToneCurve| {
            let mut points: Vec<[f32; 2]> = Vec::with_capacity(c.points.len());
            let last = c.points.len().saturating_sub(1);
            for (i, p) in c.points.iter().enumerate() {
                let p = p.map(|v| (v * 255.).round() / 255.);
                match points.last_mut() {
                    Some(previous) if previous[0] == p[0] => {
                        if i == last {
                            *previous = p;
                        }
                    }
                    _ => points.push(p),
                }
            }
            ToneCurve {
                points,
                ..c.clone()
            }
        };
        Self {
            rgb: quantize(&self.rgb),
            channels: self.channels.each_ref().map(quantize),
        }
    }
    /// Whether the Red, Green and Blue curves leave colors as they are.
    fn channels_linear(&self) -> bool {
        self.channels.iter().all(|c| same(c, &ToneCurve::default()))
    }
    /// Whether `r` has this point curve, at the 8-bit steps curves are saved in.
    fn matches(&self, r: &Recipe) -> bool {
        same(&self.rgb, &r.curve)
            && self
                .channels
                .iter()
                .zip(&r.effects.channels)
                .all(|(a, b)| same(a, b))
    }
}

/// Whether two curves have the same points at the 0–255 steps Lightroom saves.
fn same(a: &ToneCurve, b: &ToneCurve) -> bool {
    let steps = |c: &ToneCurve| -> Vec<[i32; 2]> {
        c.points
            .iter()
            .map(|p| p.map(|v| (v * 255.).round() as i32))
            .collect()
    };
    steps(a) == steps(b)
}

/// Lightroom's built-in point curves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinCurve {
    Linear,
    MediumContrast,
    StrongContrast,
}

impl BuiltinCurve {
    pub const ALL: [Self; 3] = [Self::Linear, Self::MediumContrast, Self::StrongContrast];

    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::MediumContrast => "Medium Contrast",
            Self::StrongContrast => "Strong Contrast",
        }
    }
    /// The RGB curve's points, 0–255, as Lightroom Classic stores them in the catalog
    /// for edits whose `ToneCurveName2012` names the curve.
    fn points(self) -> &'static [[u8; 2]] {
        match self {
            Self::Linear => &[[0, 0], [255, 255]],
            Self::MediumContrast => &[
                [0, 0],
                [32, 22],
                [64, 56],
                [128, 128],
                [192, 196],
                [255, 255],
            ],
            Self::StrongContrast => &[
                [0, 0],
                [32, 16],
                [64, 50],
                [128, 128],
                [192, 202],
                [255, 255],
            ],
        }
    }
    pub fn curve(self) -> ToneCurve {
        ToneCurve {
            points: self
                .points()
                .iter()
                .map(|p| p.map(|v| f32::from(v) / 255.))
                .collect(),
            ..ToneCurve::default()
        }
    }
    /// Choosing it sets the RGB curve; the Red, Green and Blue curves stay.
    pub fn apply(self, r: &mut Recipe) {
        r.curve = self.curve();
    }
}

/// A curve saved in the Curves folder.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedCurve {
    pub name: String,
    pub curve: PointCurve,
}

/// The name the Point Curve menu shows for `r`'s curve: a saved curve it has (when
/// that curve's colors are what sets it apart), a built-in curve its RGB curve
/// matches, any saved curve it has, or Custom.
pub fn shown_name(r: &Recipe, saved: &[SavedCurve]) -> String {
    let saved_match = |colored: bool| {
        saved
            .iter()
            .find(|s| s.curve.matches(r) && (!colored || !s.curve.channels_linear()))
            .map(|s| s.name.clone())
    };
    saved_match(true)
        .or_else(|| {
            BuiltinCurve::ALL
                .into_iter()
                .find(|b| same(&b.curve(), &r.curve))
                .map(|b| b.name().to_string())
        })
        .or_else(|| saved_match(false))
        .unwrap_or_else(|| "Custom".into())
}

/// The folder of saved point curves, under the user library.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedCurves {
    pub dir: PathBuf,
}

impl Default for SavedCurves {
    fn default() -> Self {
        Self {
            dir: crate::storage::data_dir().join("curves"),
        }
    }
}

/// The saved curves, by name, and the files that could not be read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SavedCurveList {
    pub curves: Vec<SavedCurve>,
    pub errors: Vec<String>,
}

impl SavedCurves {
    /// Every saved curve, sorted by name as the menu lists them.
    pub fn list(&self) -> SavedCurveList {
        let mut list = SavedCurveList::default();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return list;
        };
        for path in entries.flatten().map(|e| e.path()) {
            if path
                .extension()
                .is_none_or(|e| !e.eq_ignore_ascii_case("xmp"))
            {
                continue;
            }
            match read(&path) {
                Ok(curve) => list.curves.push(curve),
                Err(e) => list.errors.push(format!("{}: {e:#}", path.display())),
            }
        }
        list.curves
            .sort_by_key(|c| (c.name.to_lowercase(), c.name.clone()));
        list
    }
    /// Saves `curve` as `name`; returns its file. A curve of that name is kept.
    pub fn save(&self, name: &str, curve: &PointCurve) -> Result<PathBuf> {
        use std::io::Write;
        let name = super::user::file_name(name.trim());
        ensure!(!name.is_empty(), "A curve needs a name");
        let path = self.dir.join(format!("{name}.xmp"));
        let curve = curve.quantized();
        let text = crate::xmp::preset_write::point_curve(&curve.rgb, &curve.channels);
        std::fs::create_dir_all(&self.dir)?;
        crate::storage::write_atomic(&path, Replace::NoClobber, |f| {
            Ok(f.write_all(text.as_bytes())?)
        })
        .with_context(|| format!("A curve named {name} is already saved"))?;
        Ok(path)
    }
}

/// A saved curve file. One without Red, Green and Blue curves sets them linear.
fn read(path: &Path) -> Result<SavedCurve> {
    let text = std::fs::read_to_string(path)?;
    let preset = crate::xmp::parse(path, &text)?;
    let get = |key: &str| preset.curves.get(key).cloned();
    let rgb = get("ToneCurvePV2012")
        .or_else(|| get("ToneCurve"))
        .context("No point curve in the file")?;
    let channel_set = |prefix: &str| -> Option<[ToneCurve; 3]> {
        let [red, green, blue] = ["Red", "Green", "Blue"].map(|c| get(&format!("{prefix}{c}")));
        Some([red?, green?, blue?])
    };
    let channels = channel_set("ToneCurvePV2012")
        .or_else(|| channel_set("ToneCurve"))
        .unwrap_or_default();
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(SavedCurve {
        name,
        curve: PointCurve { rgb, channels },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(c: &ToneCurve) -> Vec<[i32; 2]> {
        c.points
            .iter()
            .map(|p| p.map(|v| (v * 255.).round() as i32))
            .collect()
    }

    #[test]
    fn built_in_curves_have_lightrooms_points() {
        // As Lightroom Classic 15 stores them for edits naming each curve.
        assert_eq!(
            points(&BuiltinCurve::MediumContrast.curve()),
            [
                [0, 0],
                [32, 22],
                [64, 56],
                [128, 128],
                [192, 196],
                [255, 255]
            ]
        );
        assert_eq!(
            points(&BuiltinCurve::StrongContrast.curve()),
            [
                [0, 0],
                [32, 16],
                [64, 50],
                [128, 128],
                [192, 202],
                [255, 255]
            ]
        );
        assert_eq!(BuiltinCurve::Linear.curve(), ToneCurve::default());
        for b in BuiltinCurve::ALL {
            b.curve().validate().unwrap();
        }
    }

    #[test]
    fn built_in_curves_match_the_shipped_curve_presets() {
        // The Presets panel's Curve group applies the same curves.
        for b in BuiltinCurve::ALL {
            let file = format!("Curve/{}.xmp", b.name());
            let (_, text) = crate::presets::builtin::FILES
                .iter()
                .find(|(path, _)| *path == file)
                .unwrap();
            let preset = crate::xmp::parse(Path::new(&file), text).unwrap();
            assert_eq!(preset.curves["ToneCurvePV2012"], b.curve(), "{file}");
        }
    }

    #[test]
    fn a_built_in_curve_sets_the_rgb_curve_and_keeps_the_channels() {
        let mut r = Recipe::default();
        r.effects.channels[0].points = vec![[0., 0.1], [1., 1.]];
        let red = r.effects.channels[0].clone();
        BuiltinCurve::StrongContrast.apply(&mut r);
        assert_eq!(r.curve, BuiltinCurve::StrongContrast.curve());
        assert_eq!(r.effects.channels[0], red);
        assert_eq!(shown_name(&r, &[]), "Strong Contrast");
        r.curve.move_point(2, [0.25, 0.3]);
        assert_eq!(shown_name(&r, &[]), "Custom");
        assert_eq!(shown_name(&Recipe::default(), &[]), "Linear");
    }

    #[test]
    fn saved_curves_keep_all_four_curves_and_are_listed_by_name() -> Result<()> {
        let d = tempfile::tempdir()?;
        let store = SavedCurves {
            dir: d.path().join("Curves"),
        };
        assert_eq!(store.list(), SavedCurveList::default());
        let mut r = Recipe::default();
        r.curve.points = vec![[0., 0.1], [0.5, 0.45], [1., 0.95]];
        r.effects.channels[2].points = vec![[0., 0.], [0.5, 0.55], [1., 1.]];
        let curve = PointCurve::of(&r);
        let path = store.save(" faded/blue ", &curve)?;
        assert_eq!(path, store.dir.join("faded-blue.xmp"));
        store.save("Another", &PointCurve::of(&Recipe::default()))?;
        let err = store.save("Another", &curve).unwrap_err();
        assert!(format!("{err:#}").contains("already saved"), "{err:#}");
        assert!(store.save("  ", &curve).is_err());
        let list = store.list();
        assert!(list.errors.is_empty(), "{:?}", list.errors);
        let names: Vec<_> = list.curves.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Another", "faded-blue"]);
        let saved = &list.curves[1].curve;
        assert_eq!(points(&saved.rgb), points(&curve.rgb));
        assert_eq!(points(&saved.channels[2]), points(&curve.channels[2]));
        // Loading it gives the edit the whole curve, and the menu shows its name.
        let mut other = Recipe::default();
        saved.apply(&mut other);
        assert_eq!(shown_name(&other, &list.curves), "faded-blue");
        // A linear saved curve is still Linear: the built-in name comes first.
        assert_eq!(shown_name(&Recipe::default(), &list.curves), "Linear");
        Ok(())
    }

    #[test]
    fn points_closer_than_a_step_are_merged_so_the_file_reads_back() -> Result<()> {
        let d = tempfile::tempdir()?;
        let store = SavedCurves {
            dir: d.path().to_path_buf(),
        };
        // Two points dragged together, 0.0005 apart, and one beside the white end.
        let mut r = Recipe::default();
        r.curve.points = vec![[0., 0.], [0.5, 0.4], [0.5005, 0.45], [0.999, 0.9], [1., 1.]];
        r.curve.validate()?;
        store.save("Close", &PointCurve::of(&r))?;
        let list = store.list();
        assert!(list.errors.is_empty(), "{:?}", list.errors);
        assert_eq!(
            points(&list.curves[0].curve.rgb),
            [[0, 0], [128, 102], [255, 255]]
        );
        Ok(())
    }

    #[test]
    fn camera_raw_curve_files_are_read_and_bad_files_reported() -> Result<()> {
        let d = tempfile::tempdir()?;
        let store = SavedCurves {
            dir: d.path().to_path_buf(),
        };
        // A curve file as Camera Raw writes one, with the RGB curve alone.
        std::fs::write(
            d.path().join("Lift.xmp"),
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:Version="8.4" crs:ToneCurveName2012="Custom" crs:HasSettings="True">
   <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 20</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#,
        )?;
        std::fs::write(d.path().join("Broken.xmp"), "<not xmp")?;
        std::fs::write(d.path().join("notes.txt"), "ignored")?;
        let list = store.list();
        assert_eq!(list.curves.len(), 1);
        assert_eq!(list.curves[0].name, "Lift");
        assert_eq!(points(&list.curves[0].curve.rgb), [[0, 20], [255, 255]]);
        assert!(list.curves[0].curve.channels_linear());
        assert_eq!(list.errors.len(), 1);
        assert!(list.errors[0].contains("Broken.xmp"), "{:?}", list.errors);
        Ok(())
    }
}
