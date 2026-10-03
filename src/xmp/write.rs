//! Writes a recipe back out as Camera Raw settings (`crs:`), the XMP Lightroom
//! embeds in its exports. The keys and scales mirror `apply`, so reading the
//! packet back reproduces the edit.
use crate::{develop::Recipe, develop::curve::ToneCurve, raw::Metadata};
use std::fmt::Write;

/// A keyword's path, top first, and which of its names an export writes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeywordPath {
    pub path: Vec<String>,
    pub exported: Vec<bool>,
}
impl KeywordPath {
    /// A path whose names all export.
    pub fn all(path: Vec<String>) -> Self {
        Self {
            exported: vec![true; path.len()],
            path,
        }
    }
}

/// Facts about the photo that go in the packet beside its settings.
#[derive(Clone, Debug, Default)]
pub struct Photo {
    /// The RAW's file name, e.g. "DSC07924.ARW".
    pub raw_name: String,
    /// Capture time as EXIF writes it, "2018:08:26 10:39:33".
    pub captured: Option<String>,
    /// Capture time as XMP writes it, with subseconds and offset when known;
    /// wins over `captured`.
    pub created: Option<String>,
    /// Export time in UTC, "2026-09-27T06:12:22Z".
    pub now: String,
    pub rating: i32,
    pub label: String,
    pub keywords: Vec<KeywordPath>,
    /// Languages of each, as (language, text), `x-default` first.
    pub title: Vec<(String, String)>,
    pub caption: Vec<(String, String)>,
    pub rights: Vec<(String, String)>,
    /// In order.
    pub creators: Vec<String>,
    /// The lens name (aux:Lens), part of the camera info.
    pub lens: bool,
    /// Include the develop settings, not only the descriptive metadata.
    pub settings: bool,
    /// "image/jpeg" or "image/tiff".
    pub format: String,
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}
/// "2018:08:26 10:39:33" as XMP's "2018-08-26T10:39:33".
fn xmp_date(exif: &str) -> Option<String> {
    let (date, time) = exif.trim().split_once(' ')?;
    (date.len() == 10).then(|| format!("{}T{time}", date.replace(':', "-")))
}

/// Lightroom's number style: integers for ×100 sliders, a sign on signed ones.
fn number(value: f32, decimals: usize, signed: bool) -> String {
    let text = format!("{value:.decimals$}");
    let zero = text.trim_start_matches(['-', '0', '.']).is_empty();
    if zero {
        format!("{:.decimals$}", 0.)
    } else if signed && value > 0. {
        format!("+{text}")
    } else {
        text
    }
}

struct Settings(Vec<(String, String)>);
impl Settings {
    /// `value` in recipe units, written as `value / scale`.
    fn put(&mut self, key: &str, value: f32, scale: f32, decimals: usize, signed: bool) {
        self.0
            .push((key.into(), number(value / scale, decimals, signed)));
    }
    fn text(&mut self, key: &str, value: impl Into<String>) {
        self.0.push((key.into(), value.into()));
    }
}

fn settings(r: &Recipe) -> Settings {
    let mut s = Settings(Vec::new());
    s.text("ProcessVersion", "11.0");
    if let Some(profile) = &r.profile {
        s.text("CameraProfile", profile.name.clone());
    }
    s.text("WhiteBalance", "Custom");
    s.put("Temperature", r.temperature, 1., 0, false);
    s.put("Tint", r.tint, 1., 0, true);
    s.put("Exposure2012", r.exposure, 1., 2, true);
    for (key, value) in [
        ("Contrast2012", r.contrast),
        ("Highlights2012", r.highlights),
        ("Shadows2012", r.shadows),
        ("Whites2012", r.whites),
        ("Blacks2012", r.blacks),
        ("Texture", r.effects.texture),
        ("Clarity2012", r.effects.clarity),
        ("Dehaze", r.effects.dehaze),
        ("Vibrance", r.vibrance),
        ("Saturation", r.saturation),
    ] {
        s.put(key, value, 0.01, 0, true);
    }
    for (i, name) in ["Shadows", "Darks", "Lights", "Highlights"]
        .iter()
        .enumerate()
    {
        s.put(
            &format!("Parametric{name}"),
            r.effects.parametric[i],
            0.01,
            0,
            true,
        );
    }
    for (i, name) in ["Shadow", "Midtone", "Highlight"].iter().enumerate() {
        s.put(
            &format!("Parametric{name}Split"),
            r.effects.splits[i],
            0.01,
            0,
            false,
        );
    }
    s.put("Sharpness", r.sharpening, 1. / 150., 0, false);
    s.put("SharpenRadius", r.sharpening_radius, 1., 1, true);
    s.put("SharpenDetail", r.sharpening_detail, 0.01, 0, false);
    s.put("SharpenEdgeMasking", r.sharpening_masking, 0.01, 0, false);
    s.put("LuminanceSmoothing", r.noise_luma, 0.01, 0, false);
    s.put(
        "LuminanceNoiseReductionDetail",
        r.effects.luma_detail,
        0.01,
        0,
        false,
    );
    s.put(
        "LuminanceNoiseReductionContrast",
        r.effects.luma_contrast,
        0.01,
        0,
        false,
    );
    s.put("ColorNoiseReduction", r.noise_chroma, 0.01, 0, false);
    s.put(
        "ColorNoiseReductionDetail",
        r.effects.chroma_detail,
        0.01,
        0,
        false,
    );
    s.put(
        "ColorNoiseReductionSmoothness",
        r.effects.chroma_smoothness,
        0.01,
        0,
        false,
    );
    let bands = [
        "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
    ];
    for (j, control) in ["Hue", "Saturation", "Luminance"].iter().enumerate() {
        for (i, band) in bands.iter().enumerate() {
            s.put(
                &format!("{control}Adjustment{band}"),
                r.hsl[i][j],
                0.01,
                0,
                true,
            );
        }
    }
    s.text(
        "ConvertToGrayscale",
        if r.effects.monochrome {
            "True"
        } else {
            "False"
        },
    );
    if r.effects.monochrome {
        for (i, band) in bands.iter().enumerate() {
            s.put(
                &format!("GrayMixer{band}"),
                r.effects.gray_mix[i],
                0.01,
                0,
                true,
            );
        }
    }
    for (i, name) in [(0, "Shadow"), (2, "Highlight")] {
        s.put(
            &format!("SplitToning{name}Hue"),
            r.grading[i][0],
            1. / 360.,
            0,
            false,
        );
        s.put(
            &format!("SplitToning{name}Saturation"),
            r.grading[i][1],
            0.01,
            0,
            false,
        );
        s.put(
            &format!("ColorGrade{name}Lum"),
            r.grading[i][2],
            0.01,
            0,
            true,
        );
    }
    s.put("SplitToningBalance", r.effects.balance, 0.01, 0, true);
    s.put("ColorGradeMidtoneHue", r.grading[1][0], 1. / 360., 0, false);
    s.put("ColorGradeMidtoneSat", r.grading[1][1], 0.01, 0, false);
    s.put("ColorGradeMidtoneLum", r.grading[1][2], 0.01, 0, true);
    s.put("ColorGradeBlending", r.effects.blending, 0.01, 0, false);
    s.put(
        "ColorGradeGlobalHue",
        r.effects.global_grade[0],
        1. / 360.,
        0,
        false,
    );
    s.put(
        "ColorGradeGlobalSat",
        r.effects.global_grade[1],
        0.01,
        0,
        false,
    );
    s.put(
        "ColorGradeGlobalLum",
        r.effects.global_grade[2],
        0.01,
        0,
        true,
    );
    for (i, band) in ["Red", "Green", "Blue"].iter().enumerate() {
        s.put(
            &format!("{band}Hue"),
            r.effects.calibration[i][0],
            0.01,
            0,
            true,
        );
        s.put(
            &format!("{band}Saturation"),
            r.effects.calibration[i][1],
            0.01,
            0,
            true,
        );
    }
    s.put("ShadowTint", r.effects.shadow_tint, 0.01, 0, true);
    s.put("GrainAmount", r.effects.grain, 0.01, 0, false);
    if r.effects.grain > 0. {
        s.put("GrainSize", r.effects.grain_size, 0.01, 0, false);
        s.put("GrainFrequency", r.effects.grain_roughness, 0.01, 0, false);
        s.text("GrainSeed", r.effects.grain_seed.to_string());
    }
    s.put("PostCropVignetteAmount", r.effects.vignette, 0.01, 0, true);
    if r.effects.vignette != 0. {
        s.put(
            "PostCropVignetteMidpoint",
            r.effects.vignette_midpoint,
            0.01,
            0,
            false,
        );
        s.put(
            "PostCropVignetteRoundness",
            r.effects.vignette_roundness,
            0.01,
            0,
            true,
        );
        s.put(
            "PostCropVignetteFeather",
            r.effects.vignette_feather,
            0.01,
            0,
            false,
        );
        s.put(
            "PostCropVignetteHighlightContrast",
            r.effects.vignette_highlights,
            0.01,
            0,
            false,
        );
        s.text(
            "PostCropVignetteStyle",
            r.effects.vignette_style.to_string(),
        );
    }
    s.put("VignetteAmount", r.effects.lens_vignette, 0.01, 0, true);
    s.put(
        "VignetteMidpoint",
        r.effects.lens_vignette_midpoint,
        0.01,
        0,
        false,
    );
    for (i, name) in ["Purple", "Green"].iter().enumerate() {
        s.put(
            &format!("Defringe{name}Amount"),
            r.effects.defringe[i],
            0.05,
            0,
            false,
        );
        s.put(
            &format!("Defringe{name}HueLo"),
            r.effects.defringe_ranges[i][0],
            0.01,
            0,
            false,
        );
        s.put(
            &format!("Defringe{name}HueHi"),
            r.effects.defringe_ranges[i][1],
            0.01,
            0,
            false,
        );
    }
    s.text("LensProfileEnable", if r.lens_profile { "1" } else { "0" });
    s.text("AutoLateralCA", if r.lens_ca { "1" } else { "0" });
    s.put(
        "LensProfileDistortionScale",
        r.lens_distortion,
        0.01,
        0,
        false,
    );
    s.put(
        "LensProfileVignettingScale",
        r.lens_vignetting,
        0.01,
        0,
        false,
    );
    let t = &r.transform;
    s.put("PerspectiveVertical", t.vertical, 0.01, 0, true);
    s.put("PerspectiveHorizontal", t.horizontal, 0.01, 0, true);
    s.put("PerspectiveRotate", t.rotate, 1., 1, true);
    s.put("PerspectiveAspect", t.aspect, 0.01, 0, true);
    s.put("PerspectiveScale", t.scale, 0.01, 0, false);
    s.put("PerspectiveX", t.offset_x, 0.01, 2, true);
    s.put("PerspectiveY", t.offset_y, 0.01, 2, true);
    let u = &r.upright;
    s.text("PerspectiveUpright", u.mode.code().to_string());
    if !u.corrections.is_empty() {
        s.text("UprightTransformCount", u.corrections.len().to_string());
    }
    for (i, m) in u.corrections.iter().enumerate() {
        let m: Vec<String> = m.iter().map(|x| format!("{x:.9}")).collect();
        s.text(&format!("UprightTransform_{i}"), m.join(","));
    }
    for (key, value) in &u.lightroom {
        s.text(key, value.clone());
    }
    for (i, name) in ["Left", "Top", "Right", "Bottom"].iter().enumerate() {
        s.put(&format!("Crop{name}"), r.crop[i], 1., 6, false);
    }
    s.put("CropAngle", r.straighten, 1., 2, true);
    let cropped = r.crop != [0., 0., 1., 1.] || r.straighten != 0.;
    s.text("HasCrop", if cropped { "True" } else { "False" });
    s.text("HasSettings", "True");
    s
}

fn curve(out: &mut String, name: &str, c: &ToneCurve) {
    let _ = write!(out, "   <crs:{name}>\n    <rdf:Seq>\n");
    for [x, y] in &c.points {
        let _ = writeln!(
            out,
            "     <rdf:li>{}, {}</rdf:li>",
            (x * 255.).round() as i32,
            (y * 255.).round() as i32
        );
    }
    let _ = write!(out, "    </rdf:Seq>\n   </crs:{name}>\n");
}

/// A language alternative (dc:title, dc:description, dc:rights); nothing
/// when it has no text.
fn lang_alt(out: &mut String, name: &str, langs: &[(String, String)]) {
    if langs.is_empty() {
        return;
    }
    let _ = write!(out, "   <{name}>\n    <rdf:Alt>\n");
    for (lang, text) in langs {
        let _ = writeln!(
            out,
            "     <rdf:li xml:lang=\"{}\">{}</rdf:li>",
            escape(lang),
            escape(text)
        );
    }
    let _ = write!(out, "    </rdf:Alt>\n   </{name}>\n");
}
/// An unordered (Bag) or ordered (Seq) list; nothing when empty.
fn list(out: &mut String, name: &str, kind: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    let _ = write!(out, "   <{name}>\n    <rdf:{kind}>\n");
    for item in items {
        let _ = writeln!(out, "     <rdf:li>{}</rdf:li>", escape(item));
    }
    let _ = write!(out, "    </rdf:{kind}>\n   </{name}>\n");
}
/// dc:subject, every keyword and its ancestors once, and
/// lr:hierarchicalSubject, each keyword's path joined with "|" (a top-level
/// keyword as a one-name path, as Lightroom writes it). Names Lightroom's
/// keyword options leave out of exports are left out of both: a keyword
/// whose parents do not all export goes in the hierarchy alone. "|" always
/// separates there, so a path with a name containing it is left out of it.
pub fn keyword_lists(keywords: &[KeywordPath]) -> (Vec<String>, Vec<String>) {
    let mut subject: Vec<String> = Vec::new();
    let mut hierarchical: Vec<String> = Vec::new();
    for keyword in keywords {
        let names: Vec<(&str, bool)> = keyword
            .path
            .iter()
            .zip(keyword.exported.iter().chain(std::iter::repeat(&true)))
            .map(|(n, e)| (n.trim(), *e))
            .filter(|(n, _)| !n.is_empty())
            .collect();
        if !names.last().is_some_and(|(_, e)| *e) {
            continue;
        }
        for (name, exported) in &names {
            if *exported && !subject.iter().any(|s| s == name) {
                subject.push(name.to_string());
            }
        }
        let path: Vec<&str> = if names.iter().all(|(_, e)| *e) {
            names.iter().map(|(n, _)| *n).collect()
        } else {
            names.last().map(|(n, _)| *n).into_iter().collect()
        };
        if !path.is_empty() && path.iter().all(|n| !n.contains('|')) {
            let joined = path.join("|");
            if !hierarchical.contains(&joined) {
                hierarchical.push(joined);
            }
        }
    }
    (subject, hierarchical)
}

/// The XMP packet for an exported photo.
pub fn packet(r: &Recipe, m: &Metadata, photo: &Photo) -> String {
    let mut attributes: Vec<(String, String)> = vec![
        ("xmp:CreatorTool".into(), crate::export::SOFTWARE.into()),
        ("xmp:ModifyDate".into(), photo.now.clone()),
        ("xmp:MetadataDate".into(), photo.now.clone()),
    ];
    let created = photo
        .created
        .clone()
        .or_else(|| photo.captured.as_deref().and_then(xmp_date));
    if let Some(date) = created {
        attributes.push(("xmp:CreateDate".into(), date.clone()));
        attributes.push(("photoshop:DateCreated".into(), date));
    }
    if photo.rating != 0 {
        attributes.push(("xmp:Rating".into(), photo.rating.to_string()));
    }
    if !photo.label.is_empty() {
        attributes.push(("xmp:Label".into(), photo.label.clone()));
    }
    if photo.lens && !m.lens_model.is_empty() {
        attributes.push(("aux:Lens".into(), m.lens_model.clone()));
    }
    if !photo.raw_name.is_empty() {
        attributes.push(("xmpMM:PreservedFileName".into(), photo.raw_name.clone()));
    }
    attributes.push(("dc:format".into(), photo.format.clone()));
    if photo.settings {
        if !photo.raw_name.is_empty() {
            attributes.push(("crs:RawFileName".into(), photo.raw_name.clone()));
        }
        attributes.extend(
            settings(r)
                .0
                .into_iter()
                .map(|(k, v)| (format!("crs:{k}"), v)),
        );
        attributes.push(("crs:AlreadyApplied".into(), "True".into()));
    }
    let mut out = String::from(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
         <rdf:Description rdf:about=\"\"\n    \
         xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n    \
         xmlns:aux=\"http://ns.adobe.com/exif/1.0/aux/\"\n    \
         xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\"\n    \
         xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\"\n    \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    \
         xmlns:lr=\"http://ns.adobe.com/lightroom/1.0/\"\n    \
         xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"",
    );
    for (key, value) in &attributes {
        let _ = write!(out, "\n   {key}=\"{}\"", escape(value));
    }
    out.push_str(">\n");
    lang_alt(&mut out, "dc:title", &photo.title);
    lang_alt(&mut out, "dc:description", &photo.caption);
    lang_alt(&mut out, "dc:rights", &photo.rights);
    list(&mut out, "dc:creator", "Seq", &photo.creators);
    let (subject, hierarchical) = keyword_lists(&photo.keywords);
    list(&mut out, "dc:subject", "Bag", &subject);
    list(&mut out, "lr:hierarchicalSubject", "Bag", &hierarchical);
    if photo.settings {
        curve(&mut out, "ToneCurvePV2012", &r.curve);
        for (i, name) in ["Red", "Green", "Blue"].iter().enumerate() {
            curve(
                &mut out,
                &format!("ToneCurvePV2012{name}"),
                &r.effects.channels[i],
            );
        }
    }
    out.push_str("  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>");
    out
}
